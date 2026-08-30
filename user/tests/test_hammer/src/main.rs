//! hammer：生命周期多核竞态矩阵的执行器与竞态靶（协议见
//! `libprocess::race`，剧本与断言归 init）。
//!
//! HAMMER 模式是受编排的 syscall 执行器：指令与 handle 经 Mailbox 运行
//! 投递，等发令枪 READABLE 后立即执行单条指令并回执——窗口密度即本
//! 负载的存在意义，锤内不做判定。TARGET 模式是竞态靶：等枪后按角色
//! 自灭（Suicide/Fault）或高频 park，与锤的 kill 同刻起跑。

#![no_std]

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use libprocess::race::{
    self, ACTION_CLOSE, ACTION_CREATE, ACTION_CREATE_ABANDON, ACTION_DRAIN, ACTION_ENUMERATE,
    ACTION_EXIT, ACTION_KILL, ACTION_SEAL, ACTION_START, Cmd, HAMMER_CMD, HAMMER_CONTROL_RIGHTS,
    HAMMER_GUN, HAMMER_REPORT, MODE_HAMMER, MODE_TARGET, MSG_CMD, MSG_REPORT, Report, TARGET_FAULT,
    TARGET_GUARD_FAULT, TARGET_GUN, TARGET_MEMORY_CHURN, TARGET_PARK, TARGET_SUICIDE,
    TARGET_THREAD_SUITE,
};
use rinlib::{
    env,
    ipc::{
        message::{send, wait_message},
        notification,
        object::close,
        wait::wait_many,
    },
    mm::{MappedRegion, Placement},
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        mem::MemoryProtection,
        object::{Handle, ObjectSignals},
        proc::{ExecutionProfile, JobMemberKind, PROCESS_PAGE_SIZE},
        wait::{WAIT_TIMEOUT_INFINITE, WaitItem},
    },
    sys_exit, sys_sleep, thread,
};

fn main() {
    let Some(words) = race::decode_payload(env::startup_payload()) else {
        debug!("hammer: invalid payload");
        return;
    };
    match words.first().copied().unwrap_or(0) {
        MODE_HAMMER => hammer_mode(),
        MODE_TARGET => target_mode(&words),
        other => debug!("hammer: unknown mode {}", other),
    }
}

/// 等发令枪 READABLE（电平不丢令）并清位转脉冲，使多轮复用同一把枪。
fn await_gun(gun: Handle) {
    let items = [WaitItem::new(gun, ObjectSignals::READABLE, 0)];
    let _ = wait_many(&items, WAIT_TIMEOUT_INFINITE);
    let _ = notification::take(gun, u64::MAX);
}

fn hammer_mode() {
    let (Some(cmd_box), Some(report_box), Some(gun)) = (
        env::startup_handle(HAMMER_CMD),
        env::startup_handle(HAMMER_REPORT),
        env::startup_handle(HAMMER_GUN),
    ) else {
        debug!("hammer: missing grants");
        return;
    };
    loop {
        let message = match wait_message(cmd_box) {
            Ok(message) => message,
            Err(_) => return,
        };
        if message.header.kind != MSG_CMD {
            continue;
        }
        let Some(cmd) = race::decode_cmd(&message.payload) else {
            continue;
        };
        if cmd.action == ACTION_EXIT {
            let _ = send(
                report_box,
                MSG_REPORT,
                &race::encode_report(
                    &Report {
                        status: 0,
                        aux0: 0,
                        aux1: 0,
                    },
                    &[],
                ),
                &[],
            );
            return;
        }
        await_gun(gun);
        // 时序变体：aux > 0 时醒后先延迟再打，把窗口让给对侧先行
        // （线协议见 race::Cmd::aux）。延迟失败不判定——窗口密度本就是
        // 尽力而为，断言全在 init。
        if cmd.aux != 0 {
            // SAFETY: 值参数；纯延迟，无副作用依赖。
            let _ = unsafe { sys_sleep(cmd.aux) };
        }
        let (report, tail) = execute(&cmd, &message.handles);
        let _ = send(
            report_box,
            MSG_REPORT,
            &race::encode_report(&report, &tail),
            &[],
        );
    }
}

fn execute(cmd: &Cmd, handles: &[Handle]) -> (Report, alloc::vec::Vec<u64>) {
    let (report, tail) = match cmd.action {
        ACTION_KILL => {
            let result = process::kill(handles[0], cmd.code as i64);
            let _ = close(handles[0]);
            done(result)
        }
        ACTION_START => {
            let result = start_target(cmd, handles);
            // Start 失败（如 seal 后 ObjectClosed）时 builder 未被消费，
            // 随指令关闭——否则残留至锤退出才由内核收。
            if result.is_err() {
                let _ = close(handles[0]);
            }
            done(result)
        }
        ACTION_CREATE => create(handles, false),
        ACTION_CREATE_ABANDON => create(handles, true),
        ACTION_SEAL => {
            let result = process::seal_job(handles[0]);
            let _ = close(handles[0]);
            done(result)
        }
        ACTION_DRAIN => {
            let (report, tail) = drain(handles[0]);
            let _ = close(handles[0]);
            (report, tail)
        }
        ACTION_CLOSE => done(close(handles[0])),
        ACTION_ENUMERATE => {
            let (report, tail) = enumerate(handles[0]);
            let _ = close(handles[0]);
            (report, tail)
        }
        other => (
            Report {
                status: SystemCallError::IllegalArgument as i64,
                aux0: other,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ),
    };
    (report, tail)
}

fn done<T>(result: Result<T, SystemCallError>) -> (Report, alloc::vec::Vec<u64>) {
    let status = match result {
        Ok(_) => 0,
        Err(error) => error as i64,
    };
    (
        Report {
            status,
            aux0: 0,
            aux1: 0,
        },
        alloc::vec::Vec::new(),
    )
}

fn start_target(cmd: &Cmd, handles: &[Handle]) -> Result<(), SystemCallError> {
    let _ = (cmd.entry, cmd.sp);
    process::start(handles[0], ExecutionProfile::Base64 as u32)
}

fn create(handles: &[Handle], abandon: bool) -> (Report, alloc::vec::Vec<u64>) {
    let report = match process::create(handles[0], HAMMER_CONTROL_RIGHTS) {
        Ok(created) => {
            if abandon {
                let _ = close(created.builder);
                let _ = close(created.control);
                Report {
                    status: 0,
                    aux0: created.pid,
                    aux1: 0,
                }
            } else {
                Report {
                    status: 0,
                    aux0: created.pid,
                    aux1: 0,
                }
            }
        }
        Err(error) => Report {
            status: error as i64,
            aux0: 0,
            aux1: 0,
        },
    };
    // job handle 用毕即弃；builder/control 由 abandon 决定。
    let _ = close(handles[0]);
    (report, alloc::vec::Vec::new())
}

fn drain(control: Handle) -> (Report, alloc::vec::Vec<u64>) {
    match process::drain(control, 2) {
        Ok(result) => (
            Report {
                status: 0,
                aux0: result.work_done as u64,
                aux1: result.status as u64,
            },
            alloc::vec::Vec::new(),
        ),
        Err(error) => (
            Report {
                status: error as i64,
                aux0: 0,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ),
    }
}

fn enumerate(control: Handle) -> (Report, alloc::vec::Vec<u64>) {
    let mut ids = alloc::vec::Vec::new();
    let mut buf = [0u64; 16];
    let mut cursor = 0u64;
    // 占位屏障的重试上界：占位窗口是创建方单个 syscall 临界区，协作式
    // 内核下必然推进；上界只防剧本级挂死。
    for _ in 0..1024 {
        match process::enumerate_job(control, JobMemberKind::MemberProcesses, cursor, &mut buf) {
            Ok(result) => {
                ids.extend_from_slice(&buf[..result.actual as usize]);
                if result.more == 0 {
                    return (
                        Report {
                            status: 0,
                            aux0: 0,
                            aux1: 0,
                        },
                        ids,
                    );
                }
                cursor = result.next_cursor;
            }
            Err(error) => {
                return (
                    Report {
                        status: error as i64,
                        aux0: 0,
                        aux1: 0,
                    },
                    ids,
                );
            }
        }
    }
    (
        Report {
            status: SystemCallError::ReachLimit as i64,
            aux0: 0,
            aux1: 0,
        },
        ids,
    )
}

fn target_mode(words: &[u64]) {
    let Some(gun) = env::startup_handle(TARGET_GUN) else {
        debug!("hammer target: missing gun");
        return;
    };
    let role = words.get(1).copied().unwrap_or(0);
    if role == TARGET_MEMORY_CHURN {
        threaded_memory_churn(gun);
    }
    await_gun(gun);
    match role {
        TARGET_SUICIDE => {
            let code = words.get(2).copied().unwrap_or(0) as i64;
            debug!("hammer target: exiting with {}", code);
            // SAFETY: 值参数；本进程经 Exit 自灭。
            unsafe { sys_exit(code).expect("hammer target exit") };
        }
        TARGET_FAULT => {
            debug!("hammer target: faulting");
            // 用户映像从 VA 0 起铺，低位地址可读；用进程未映射的高位空洞
            // 触发 LoadAccess——用户可触发 fault 杀进程不崩内核。
            // SAFETY: 故意解引用未映射地址。
            let _ = unsafe { core::ptr::read_volatile(0x6000_0000usize as *const u64) };
        }
        TARGET_GUARD_FAULT => guard_fault(),
        TARGET_THREAD_SUITE => {
            thread_suite();
            thread::exit(0x8b);
        }
        TARGET_PARK => loop {
            // SAFETY: 值参数；高频 park 供 kill 取消线竞速。
            unsafe { sys_sleep(1).expect("hammer target park sleep") };
        },
        other => debug!("hammer target: unknown subrole {}", other),
    }
}

const THREAD_TUNNEL_VA: usize = 0x2000_0000;
const OLD_TRANSLATION_VALUE: u64 = 0x6f6c_642d_7472_616e;
const NEW_TRANSLATION_VALUE: u64 = 0x6e65_772d_7472_616e;

fn threaded_memory_churn(gun: Handle) -> ! {
    let _worker = thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| memory_churn())
        .expect("memory churn worker spawn failed");
    let _blocker = thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| large_memory_churn())
        .expect("memory churn blocker spawn failed");
    debug!("hammer target: threaded public memory churn started");
    await_gun(gun);
    loop {
        thread::yield_now().expect("memory churn coordinator yield failed");
    }
}

fn thread_suite() {
    debug!("hammer target: same-address-space thread suite started");
    stale_translation_reuse();
    concurrent_tunnel_close();
    debug!("hammer target: same-address-space thread suite passed");
}

fn stale_translation_reuse() {
    let region = MappedRegion::map_anonymous(
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .expect("thread suite initial Map failed");
    let usable = region
        .usable()
        .expect("thread suite initial Map has no usable range");
    let address = usable.start;
    // SAFETY: usable 是当前持有的 RW mapping。
    unsafe { (address as *mut u64).write_volatile(OLD_TRANSLATION_VALUE) };

    let ready = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let child_ready = ready.clone();
    let child_release = release.clone();
    let worker = thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            // SAFETY: 控制线程在 release 前不解除 mapping。
            let old = unsafe { (address as *const u64).read_volatile() };
            child_ready.store(true, Ordering::Release);
            while !child_release.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
            // SAFETY: release 只在 FixedEmpty 重映射及新值写入后发布。
            let new = unsafe { (address as *const u64).read_volatile() };
            (old, new)
        })
        .expect("translation worker spawn failed");
    while !ready.load(Ordering::Acquire) {
        thread::yield_now().expect("translation coordinator yield failed");
    }

    region
        .unmap()
        .expect("thread suite old mapping Unmap failed");
    let replacement = MappedRegion::map_anonymous(
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        MemoryProtection::ReadWrite,
        Placement::FixedEmpty {
            usable_start: address,
        },
    )
    .expect("thread suite FixedEmpty remap failed");
    // SAFETY: replacement 恰好重新取得 address 的 RW usable 页。
    unsafe { (address as *mut u64).write_volatile(NEW_TRANSLATION_VALUE) };
    release.store(true, Ordering::Release);
    let observed = worker.join();
    assert_eq!(
        observed,
        (OLD_TRANSLATION_VALUE, NEW_TRANSLATION_VALUE),
        "remote hart observed a stale translation after address reuse"
    );
    replacement
        .unmap()
        .expect("thread suite replacement Unmap failed");
}

fn concurrent_tunnel_close() {
    for round in 0..8usize {
        let pair = rinlib::ipc::tunnel::create(THREAD_TUNNEL_VA)
            .expect("thread suite TunnelCreate failed");
        let region = MappedRegion::map_anonymous(
            2 * PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            MemoryProtection::ReadWrite,
            Placement::Anywhere,
        )
        .expect("thread suite concurrent Map failed");
        let ready = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let child_ready = ready.clone();
        let child_release = release.clone();
        let endpoint = pair.owner;
        let closer = thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || {
                child_ready.store(true, Ordering::Release);
                while !child_release.load(Ordering::Acquire) {
                    core::hint::spin_loop();
                }
                close(endpoint)
            })
            .expect("Tunnel close worker spawn failed");
        while !ready.load(Ordering::Acquire) {
            thread::yield_now().expect("Tunnel close coordinator yield failed");
        }
        release.store(true, Ordering::Release);
        region.unmap().expect("concurrent ordinary Unmap failed");
        closer.join().expect("concurrent Endpoint close failed");
        close(pair.peer).expect("Tunnel invitation close failed");
        debug!(
            "hammer target: concurrent Tunnel close round {} passed",
            round
        );
    }
}

fn map_churn_region(bytes: usize) -> MappedRegion {
    loop {
        match MappedRegion::map_anonymous(
            bytes,
            PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            MemoryProtection::ReadWrite,
            Placement::Anywhere,
        ) {
            Ok(region) => return region,
            Err(SystemCallError::ObjectBusy) => {
                // SAFETY: 只在一次完整失败事务之后退避。
                unsafe { sys_sleep(1).expect("memory churn Map retry sleep failed") };
            }
            Err(error) => panic!("memory churn Map failed: {error:?}"),
        }
    }
}

fn protect_churn_region(
    region: &MappedRegion,
    range: core::ops::Range<usize>,
    protection: MemoryProtection,
) {
    loop {
        match region.protect(range.clone(), protection) {
            Ok(()) => return,
            Err(SystemCallError::ObjectBusy) => {
                // SAFETY: 只在一次完整失败事务之后退避。
                unsafe { sys_sleep(1).expect("memory churn Protect retry sleep failed") };
            }
            Err(error) => panic!("memory churn Protect failed: {error:?}"),
        }
    }
}

fn unmap_churn_region(mut region: MappedRegion) {
    loop {
        match region.unmap() {
            Ok(()) => return,
            Err((returned, SystemCallError::ObjectBusy)) => {
                region = returned;
                // SAFETY: affine token 已原样返回，只在完整失败事务之后退避。
                unsafe { sys_sleep(1).expect("memory churn Unmap retry sleep failed") };
            }
            Err((_returned, error)) => panic!("memory churn Unmap failed: {error:?}"),
        }
    }
}

fn memory_churn() -> ! {
    debug!("hammer target: public memory churn started");
    // 先让大映射线程进入 backing 清零，再提交小映射以拉长 shootdown ack。
    // SAFETY: 值参数；只在第一轮事务前让出。
    unsafe { sys_sleep(1).expect("memory churn initial sleep failed") };
    loop {
        let region = map_churn_region(2 * PROCESS_PAGE_SIZE);
        let usable = region
            .usable()
            .expect("memory churn Map has no usable range");
        // SAFETY: usable 是本轮持有的 RW anonymous mapping。
        unsafe { (usable.start as *mut u64).write_volatile(0x6d65_6d6f_7279_2d38) };
        protect_churn_region(&region, usable.clone(), MemoryProtection::ReadOnly);
        protect_churn_region(&region, usable, MemoryProtection::ReadWrite);
        unmap_churn_region(region);
        // 单 hart Base64 域也必须给编排 hammer 公平执行机会；多 hart common
        // 配置中的 kill 仍可与上面的异步 MemoryChange 真并行。
        // SAFETY: 值参数；只在完整事务边界让出。
        unsafe { sys_sleep(1).expect("memory churn yield sleep failed") };
    }
}

fn large_memory_churn() -> ! {
    loop {
        let region = map_churn_region(16 * 1024 * 1024);
        let usable = region
            .usable()
            .expect("large memory churn Map has no usable range");
        // SAFETY: usable 是本轮持有的 RW anonymous mapping。
        unsafe { (usable.start as *mut u64).write_volatile(0x6c61_7267_652d_6d61) };
        protect_churn_region(&region, usable.clone(), MemoryProtection::ReadOnly);
        unmap_churn_region(region);
    }
}

fn guard_fault() {
    let region = MappedRegion::map_anonymous(
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .expect("guard fault Map failed");
    let guard = region.reservation().start;
    debug!("hammer target: touching memory guard");
    // SAFETY: 故意访问本次 reservation 的前 guard 页，必须产生用户 load fault。
    let _ = unsafe { core::ptr::read_volatile(guard as *const u64) };
    core::hint::black_box(region);
}
