//! hammer：生命周期多核竞态矩阵的执行器与竞态靶（协议见
//! `libprocess::race`，剧本与断言归 test_kernel）。
//!
//! HAMMER 模式是受编排的 syscall 执行器：指令与 handle 经 Mailbox 运行
//! 投递，等发令枪 READABLE 后立即执行单条指令并回执——窗口密度即本
//! 负载的存在意义，锤内不做判定。TARGET 模式是竞态靶：等枪后按角色
//! 自灭（Suicide/Fault）或高频 park，与锤的 kill 同刻起跑。

#![no_std]

extern crate libruntime;

use alloc::{boxed::Box, sync::Arc};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use libkernel::{
    ipc::{
        capability::Capability,
        message::{send_raw, wait_message},
        notification,
        object::close,
        wait::wait_until,
    },
    mm::{MappedRegion, Placement},
    preclude::*,
    process,
    shared::{
        call::{SystemCall, SystemCallError},
        mem::{MemoryMapRequest, MemoryMapResult, MemoryPlacement, MemoryProtection},
        object::{Handle, ObjectSignals},
        proc::{
            JobMemberKind, PROCESS_MAX_THREADS, PROCESS_PAGE_SIZE, ThreadCreateResult,
            ThreadStartContext,
        },
    },
    sys_process_terminate, sys_sleep,
};
use libprocess::race::{
    self, ACTION_CLOSE, ACTION_CREATE, ACTION_CREATE_ABANDON, ACTION_ENUMERATE, ACTION_EXIT,
    ACTION_KILL, ACTION_OBSERVE_TERMINAL, ACTION_SEAL, ACTION_START, Cmd, HAMMER_CMD,
    HAMMER_CONTROL_RIGHTS, HAMMER_GUN, HAMMER_REPORT, MODE_HAMMER, MODE_TARGET, MSG_CMD,
    MSG_REPORT, Report, TARGET_FAULT, TARGET_GUARD_FAULT, TARGET_GUN, TARGET_INVITATION,
    TARGET_LAST_THREAD_EXIT_RACE, TARGET_MEMORY_CHURN, TARGET_PARK, TARGET_SUICIDE,
    TARGET_THREAD_SPAWN_RACE, TARGET_THREAD_SUITE, TARGET_TUNNEL_EXIT,
};
use libruntime::thread;
use libthread::ThreadOutcome;

fn main() {
    let startup = libruntime::startup::take().expect("hammer startup context is missing");
    let (_process, payload, mut grants) = startup.into_parts();
    let Some(words) = race::decode_payload(payload.as_bytes()) else {
        debug!("hammer: invalid payload");
        return;
    };
    match words.first().copied().unwrap_or(0) {
        MODE_HAMMER => hammer_mode(&mut grants),
        MODE_TARGET => target_mode(&mut grants, &words),
        other => debug!("hammer: unknown mode {}", other),
    }
}

fn take_thread_result<T>(outcome: ThreadOutcome<T, libthread::NoEnvironment>) -> T {
    match outcome {
        ThreadOutcome::Returned {
            value,
            leave: Ok(()),
        } => value,
        ThreadOutcome::Returned { leave: Err(()), .. }
        | ThreadOutcome::EnterFailed(())
        | ThreadOutcome::Incomplete => panic!("runtime worker did not complete"),
    }
}

/// 等发令枪 READABLE（电平不丢令）并清位转脉冲，使多轮复用同一把枪。
fn await_gun(gun: &notification::NotificationOwner) {
    let _ = wait_until(
        gun.as_handle(),
        ObjectSignals::READABLE,
        libkernel::time::Deadline::INFINITE,
    );
    let _ = gun.take(u64::MAX);
}

fn hammer_mode(startup: &mut libruntime::startup::InitialGrants) {
    let (Some(cmd_box), Some(report_box), Some(gun)) = (
        startup.take(HAMMER_CMD).map(Capability::into_raw),
        startup.take(HAMMER_REPORT).map(Capability::into_raw),
        startup.take(HAMMER_GUN).map(|cap| {
            notification::NotificationOwner::from_capability(cap)
                .map_err(|f| f.error)
                .expect("hammer gun role invalid")
                .0
        }),
    ) else {
        debug!("hammer: missing grants");
        return;
    };
    loop {
        let mut message = match wait_message(cmd_box) {
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
            let _ = unsafe {
                send_raw(
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
                )
            };
            return;
        }
        await_gun(&gun);
        // 时序变体：aux > 0 时醒后先延迟再打，把窗口让给对侧先行
        // （线协议见 race::Cmd::aux）。延迟失败不判定——窗口密度本就是
        // 尽力而为，断言全在 test_kernel。
        if cmd.aux != 0 {
            // SAFETY: 值参数；纯延迟，无副作用依赖。
            let _ = unsafe { sys_sleep(cmd.aux) };
        }
        let (report, tail) = execute(&cmd, &mut message.handles);
        let _ = unsafe {
            send_raw(
                report_box,
                MSG_REPORT,
                &race::encode_report(&report, &tail),
                &[],
            )
        };
    }
}

fn execute(
    cmd: &Cmd,
    handles: &mut libkernel::ipc::capability::HandleSet,
) -> (Report, alloc::vec::Vec<u64>) {
    let creating = matches!(cmd.action, ACTION_CREATE | ACTION_CREATE_ABANDON);
    if handles.len() != if creating { 2 } else { 1 } {
        return done::<()>(Err(SystemCallError::IllegalArgument));
    }
    let capability = match handles.take(0) {
        Ok(capability) => capability,
        Err(error) => return done::<()>(Err(error)),
    };
    match cmd.action {
        ACTION_KILL => match process::kill(capability.as_handle()) {
            Ok(process::KillOutcome::Accepted) => (
                Report {
                    status: 0,
                    aux0: 0,
                    aux1: 0,
                },
                alloc::vec::Vec::new(),
            ),
            Ok(process::KillOutcome::AlreadyTerminating) => (
                Report {
                    status: 0,
                    aux0: 1,
                    aux1: 0,
                },
                alloc::vec::Vec::new(),
            ),
            Err(error) => done::<()>(Err(error)),
        },
        ACTION_START => {
            let builder = capability.into_raw();
            let result = process::start(builder);
            if result.is_err() {
                // SAFETY: 从运输 owner 取出的 builder，失败未消费且无其他 owner。
                let _ = unsafe { close(builder) };
            }
            done(result)
        }
        ACTION_CREATE | ACTION_CREATE_ABANDON => {
            let pool = match handles.take(1) {
                Ok(pool) => pool,
                Err(error) => return done::<()>(Err(error)),
            };
            create(
                capability.as_handle(),
                pool.as_handle(),
                cmd.action == ACTION_CREATE_ABANDON,
            )
        }
        ACTION_SEAL => done(process::seal_job(capability.as_handle())),
        ACTION_OBSERVE_TERMINAL => observe_terminal(capability.as_handle()),
        ACTION_CLOSE => {
            let handle = capability.into_raw();
            // SAFETY: 原始 Close 验收保留内核错误；失败 entry 由进程自动退休接管。
            done(unsafe { close(handle) })
        }
        ACTION_ENUMERATE => enumerate(capability.as_handle()),
        other => (
            Report {
                status: SystemCallError::IllegalArgument as i64,
                aux0: other,
                aux1: 0,
            },
            alloc::vec::Vec::new(),
        ),
    }
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

fn create(job: Handle, pool: Handle, abandon: bool) -> (Report, alloc::vec::Vec<u64>) {
    let report = match process::create(job, pool, HAMMER_CONTROL_RIGHTS) {
        Ok(created) => {
            if abandon {
                let _ = unsafe { close(created.builder) };
                let _ = unsafe { close(created.control) };
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
    // job 由命令的运输 owner 持有；新 builder/control 由 abandon 政策决定。
    (report, alloc::vec::Vec::new())
}

fn observe_terminal(control: Handle) -> (Report, alloc::vec::Vec<u64>) {
    let status = match process::query(control) {
        Ok(snapshot) if snapshot.state == libkernel::shared::proc::ProcessState::Dead as u32 => 0,
        Ok(_) => SystemCallError::ObjectBusy as i64,
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

fn target_mode(startup: &mut libruntime::startup::InitialGrants, words: &[u64]) {
    let Some(gun) = startup.take(TARGET_GUN).map(|cap| {
        notification::NotificationOwner::from_capability(cap)
            .map_err(|f| f.error)
            .expect("target gun role invalid")
            .0
    }) else {
        debug!("hammer target: missing gun");
        return;
    };
    let role = words.get(1).copied().unwrap_or(0);
    if role == TARGET_MEMORY_CHURN {
        threaded_memory_churn(gun);
    }
    if role == TARGET_THREAD_SPAWN_RACE {
        thread_spawn_race(gun);
    }
    if role == TARGET_LAST_THREAD_EXIT_RACE {
        last_thread_exit_target(gun, words.get(2).copied().unwrap_or(0));
    }
    if role == TARGET_TUNNEL_EXIT {
        tunnel_exit_target(
            startup,
            gun,
            words
                .get(2)
                .copied()
                .expect("Tunnel mapped bytes are missing") as usize,
        );
    }
    await_gun(&gun);
    match role {
        TARGET_SUICIDE => {
            debug!("hammer target: terminating");
            // SAFETY: 值参数；本进程主动结束。
            unsafe { sys_process_terminate() }
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
            thread_suite(
                startup
                    .take(1)
                    .expect("thread suite MemoryPool is missing")
                    .into_raw(),
            );
            unsafe { libkernel::sys_thread_exit() }
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

fn threaded_memory_churn(gun: notification::NotificationOwner) -> ! {
    let _worker = thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| memory_churn())
        .expect("memory churn worker spawn failed");
    let _blocker = thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| large_memory_churn())
        .expect("memory churn blocker spawn failed");
    debug!("hammer target: threaded public memory churn started");
    await_gun(&gun);
    loop {
        thread::yield_now().expect("memory churn coordinator yield failed");
    }
}

fn thread_suite(pool: Handle) {
    debug!("hammer target: same-address-space thread suite started");
    stale_translation_reuse();
    committed_mapping_access();
    fragmented_backing_retire();
    concurrent_tunnel_close(pool);
    tunnel_close_attach(pool);
    debug!("acceptance progress: phase=thread-suite step=join-publication-start");
    join_publication();
    debug!("acceptance progress: phase=thread-suite step=registry-reclaim-start");
    runtime_registry_reclaims_after_receiver_drop();
    debug!("acceptance progress: phase=thread-suite step=raw-thread-storm-start");
    raw_thread_storm();
    debug!("hammer target: same-address-space thread suite passed");
}

const TUNNEL_RESPONSE_MASK: u64 = 0xa5a5_5a5a_f0f0_0f0f;

fn tunnel_exit_target(
    startup: &mut libruntime::startup::InitialGrants,
    gun: notification::NotificationOwner,
    bytes: usize,
) -> ! {
    let invitation = startup
        .take(TARGET_INVITATION)
        .map(Capability::into_raw)
        .expect("Tunnel exit target missing invitation");
    // SAFETY: 初始表项的唯一 owner 从 startup 交付，转换后不再以原句柄关闭。
    let invitation = libkernel::ipc::invitation::Invitation::from_capability(unsafe {
        libkernel::ipc::capability::Capability::from_raw(invitation)
    })
    .unwrap();
    let endpoint = invitation.attach().expect("tunnel exit Attach failed");

    let mapping = librunnel::MappedEndpoint::map(endpoint, bytes, Placement::Anywhere)
        .expect("tunnel exit Map failed");
    {
        let shared = mapping.memory();
        let request = shared.load_u64(0, Ordering::Acquire);
        assert_ne!(
            request, 0,
            "Tunnel exit target observed an unpublished request"
        );
        shared.store_u64(0, request ^ TUNNEL_RESPONSE_MASK, Ordering::Release);
        // 独立 guest 进程中的非合作字节访问，保留原跨端混合尺寸场景。
        for round in 0..512usize {
            shared.write(8, &[round as u8; 32]);
        }
    }
    core::mem::forget(mapping); // 明确让进程回收接管 VM/View 和 Endpoint，而非用户 Drop。
    let _ = wait_until(
        gun.as_handle(),
        ObjectSignals::READABLE,
        libkernel::time::Deadline::INFINITE,
    );
    unsafe { libkernel::sys_thread_exit() }
}

type RawThreadEntry = extern "C" fn(usize, usize) -> !;

fn raw_spawn(
    entry: RawThreadEntry,
    stack_pointer: usize,
    arg1: usize,
    arg2: usize,
) -> Result<ThreadCreateResult, SystemCallError> {
    let context = ThreadStartContext {
        entry: entry as *const () as usize as u64,
        stack_pointer: stack_pointer as u64,
        arg1: arg1 as u64,
        arg2: arg2 as u64,
    };
    let mut result = ThreadCreateResult {
        tid: 0,
        reserved: 0,
        control: Handle::INVALID,
        start: Handle::INVALID,
    };
    // SAFETY: context/result live for the synchronous call; the caller owns a mapped stack.
    unsafe { libkernel::thread::create(Handle::INVALID, &context, &mut result) }?;
    let started = unsafe { libkernel::thread::start(result.start) };
    if let Err(error) = started {
        // SAFETY: 失败返还原票据，本函数独占它；成功则已由 ThreadStart 消费。
        unsafe { close(result.start) }.expect("raw ThreadStartTicket rollback failed");
        unsafe { close(result.control) }.expect("raw ThreadControl rollback failed");
        return Err(error);
    }
    assert!(
        result.tid != 0
            && result.reserved == 0
            && result.control.is_valid()
            && result.start.is_valid(),
        "raw ThreadCreate returned an invalid result"
    );
    Ok(result)
}

fn raw_stack(bytes: usize) -> MappedRegion {
    MappedRegion::map_anonymous(
        bytes,
        PROCESS_PAGE_SIZE,
        PROCESS_PAGE_SIZE,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .expect("raw thread stack Map failed")
}

extern "C" fn parked_thread_entry(_arg1: usize, _arg2: usize) -> ! {
    loop {
        thread::yield_now().expect("parked raw thread yield failed");
    }
}

fn thread_spawn_race(gun: notification::NotificationOwner) -> ! {
    let stack = raw_stack(PROCESS_PAGE_SIZE);
    let stack_pointer = stack
        .usable()
        .expect("raw spawn stack has no usable page")
        .end;
    await_gun(&gun);
    if let Ok(result) = raw_spawn(parked_thread_entry, stack_pointer, 0, 0) {
        unsafe { close(result.control) }.expect("spawn-race ThreadControl close failed");
    }
    // raw stack mapping stays owned by this process address space until process drain.
    loop {
        thread::yield_now().expect("spawn-race coordinator yield failed");
    }
}

extern "C" fn last_thread_exit_entry(gun: usize, _token: usize) -> ! {
    // 让创建者主线程先完成 ThreadExit；随后再消费电平枪进入末线程竞速。
    // SAFETY: 值参数；sleep 后仍由 Notification 电平保存发令。
    unsafe { sys_sleep(20).expect("last secondary setup sleep failed") };
    // SAFETY: 原始出生参数独占移交枪的关闭责任；创建者已转出 typed owner。
    let cap = unsafe { Capability::from_raw(Handle::from_raw(gun as u64)) };
    let (gun, _) = notification::NotificationOwner::from_capability(cap)
        .map_err(|f| f.error)
        .expect("secondary gun role invalid");
    await_gun(&gun);
    unsafe { libkernel::sys_thread_exit() }
}

fn last_thread_exit_target(gun: notification::NotificationOwner, token: u64) -> ! {
    let stack = raw_stack(PROCESS_PAGE_SIZE);
    let stack_pointer = stack
        .usable()
        .expect("last-thread stack has no usable page")
        .end;
    let result = raw_spawn(
        last_thread_exit_entry,
        stack_pointer,
        gun.into_raw().raw() as usize,
        token as usize,
    )
    .expect("last-thread raw spawn failed");
    unsafe { close(result.control) }.expect("last-thread ThreadControl close failed");
    // raw stack mapping stays owned by this process address space until process drain.
    debug!("hammer target: last secondary thread ready");
    unsafe { libkernel::sys_thread_exit() }
}

struct DropMarker(Arc<AtomicBool>);

impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn join_publication() {
    for sequence in 0..32usize {
        debug!(
            "acceptance progress: phase=join-publication round={}/32 step=start",
            sequence + 1
        );
        let worker = thread::Builder::new()
            .stack_size(16 * 1024)
            .spawn(move || (sequence, sequence.rotate_left(7) ^ 0x5a5a_5a5a))
            .expect("join publication spawn failed");
        assert_eq!(
            take_thread_result(worker.join()),
            (sequence, sequence.rotate_left(7) ^ 0x5a5a_5a5a),
            "join observed an incompletely published result"
        );
        debug!(
            "acceptance progress: phase=join-publication round={}/32 step=complete",
            sequence + 1
        );
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let child_dropped = dropped.clone();
    let worker = thread::Builder::new()
        .stack_size(16 * 1024)
        .spawn(move || DropMarker(child_dropped))
        .expect("structured Drop worker spawn failed");
    drop(take_thread_result(worker.join()));
    assert!(
        dropped.load(Ordering::Acquire),
        "joined result did not destroy the published value"
    );
}

fn runtime_registry_reclaims_after_receiver_drop() {
    // 每个 worker 持有独立放行标志：只放行探针 worker，其余保持存活，
    // 保证后续准入确实处于满槽，且只能靠回收放弃后登记的已 DONE record 成功。
    let mut workers = alloc::vec::Vec::new();
    loop {
        let hold = Arc::new(AtomicBool::new(false));
        let worker_hold = Arc::clone(&hold);
        match thread::Builder::new().stack_size(16 * 1024).spawn(move || {
            while !worker_hold.load(Ordering::Acquire) {
                thread::yield_now().expect("round5 registry worker yield failed");
            }
        }) {
            Ok(worker) => {
                workers.push((hold, worker));
                if workers.len().is_multiple_of(8) {
                    debug!(
                        "acceptance progress: phase=registry-reclaim step=filled live={}",
                        workers.len()
                    );
                }
            }
            Err(SystemCallError::ReachLimit) => break,
            Err(error) => panic!("round5 registry fill failed: {error:?}"),
        }
    }
    assert!(
        workers.len() >= 2,
        "round5 registry fill needs at least two live slots"
    );
    debug!(
        "acceptance progress: phase=registry-reclaim step=full live={}",
        workers.len()
    );

    // 释放恰好一个槽：join 等待 DONE 并协作推进回收。
    let (hold, worker) = workers.pop().expect("round5 registry probe worker missing");
    hold.store(true, Ordering::Release);
    debug!("acceptance progress: phase=registry-reclaim step=probe-join-start");
    take_thread_result(worker.join());
    debug!("acceptance progress: phase=registry-reclaim step=probe-join-complete");

    struct PacketDropProbe(Arc<AtomicUsize>);
    impl Drop for PacketDropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Release);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    // 失败的准入尝试会析构其 closure；只统计被接纳的 PreparedThread 的 Drop 增量。
    debug!("acceptance progress: phase=registry-reclaim step=prepare-drop-start");
    let prepared = prepare_until_admitted(|| {
        let packet_probe = PacketDropProbe(Arc::clone(&drops));
        move || {
            let _ = packet_probe.0.load(Ordering::Relaxed);
        }
    });
    let before = drops.load(Ordering::Acquire);
    // 未启动的 PreparedThread Drop 在放弃者线程上析构 packet/F。
    drop(prepared);
    debug!("acceptance progress: phase=registry-reclaim step=prepare-drop-complete");
    assert_eq!(
        drops.load(Ordering::Acquire) - before,
        1,
        "round5 PreparedThread drop did not destroy its packet exactly once"
    );

    // 此时放弃后登记的已 DONE record 占用唯一空槽，其余 worker 仍存活：
    // 准入成功只能来自 prepare 对该 record 的回收。
    for round in 0..3 {
        let prepared = prepare_until_admitted(|| || ());
        drop(prepared);
        debug!("round5 test=runtime_registry_abandoned_reclaim round={round} admitted");
    }

    for (hold, _) in &workers {
        hold.store(true, Ordering::Release);
    }
    for (_, worker) in workers {
        take_thread_result(worker.join());
    }
    debug!("round5 test=runtime_registry_real_prepared_drop_reap passed");
}

/// 满槽下重复准入：ReachLimit 仅允许来自栈 Unmap 暂时 Busy 的有界等待，
/// 超过上界即判定回收失败，不再把失败当作通过。prepare 失败会消费 closure，
/// 因此每次尝试由 `make` 重建。
fn prepare_until_admitted<M, F>(
    mut make: M,
) -> libthread::PreparedThread<F, (), libthread::NoEnvironment>
where
    M: FnMut() -> F,
    F: FnOnce() + Send + 'static,
{
    for _ in 0..64 {
        match thread::Builder::new().stack_size(16 * 1024).prepare(make()) {
            Ok(prepared) => return prepared,
            Err(SystemCallError::ReachLimit) => {
                thread::yield_now().expect("round5 reap progress yield failed");
            }
            Err(error) => panic!("round5 prepare failed: {error:?}"),
        }
    }
    panic!("round5 prepare never reclaimed the abandoned slot");
}

struct StormGate {
    release: AtomicBool,
    started: AtomicUsize,
    exiting: AtomicUsize,
}

extern "C" fn storm_thread_entry(gate: usize, _unused: usize) -> ! {
    // SAFETY: gate is leaked for the target process lifetime and shared only through atomics.
    let gate = unsafe { &*(gate as *const StormGate) };
    gate.started.fetch_add(1, Ordering::Release);
    while !gate.release.load(Ordering::Acquire) {
        thread::yield_now().expect("storm worker yield failed");
    }
    gate.exiting.fetch_add(1, Ordering::Release);
    unsafe { libkernel::sys_thread_exit() }
}

extern "C" fn probe_thread_entry(_arg1: usize, _arg2: usize) -> ! {
    unsafe { libkernel::sys_thread_exit() }
}

fn raw_thread_storm() {
    let child_count = PROCESS_MAX_THREADS - 1;
    let stack = raw_stack((PROCESS_MAX_THREADS + 1) * PROCESS_PAGE_SIZE);
    let usable = stack
        .usable()
        .expect("storm stack arena has no usable range");
    let gate = Box::leak(Box::new(StormGate {
        release: AtomicBool::new(false),
        started: AtomicUsize::new(0),
        exiting: AtomicUsize::new(0),
    }));
    let gate_pointer = gate as *mut StormGate as usize;
    let mut last_tid = 0;

    for index in 0..child_count {
        let stack_pointer = usable.start + (index + 1) * PROCESS_PAGE_SIZE;
        let result = raw_spawn(storm_thread_entry, stack_pointer, gate_pointer, 0)
            .expect("thread storm failed before the member limit");
        assert!(result.tid > last_tid, "thread ids are not monotonic");
        last_tid = result.tid;
        unsafe { close(result.control) }.expect("storm ThreadControl close failed");
    }

    let probe_stack = usable.start + PROCESS_MAX_THREADS * PROCESS_PAGE_SIZE;
    assert_eq!(
        raw_spawn(storm_thread_entry, probe_stack, gate_pointer, 0).unwrap_err(),
        SystemCallError::ReachLimit,
        "ThreadCreate did not enforce PROCESS_MAX_THREADS"
    );

    while gate.started.load(Ordering::Acquire) != child_count {
        thread::yield_now().expect("storm coordinator start yield failed");
    }
    gate.release.store(true, Ordering::Release);
    while gate.exiting.load(Ordering::Acquire) != child_count {
        thread::yield_now().expect("storm coordinator exit yield failed");
    }

    let probe = loop {
        match raw_spawn(probe_thread_entry, probe_stack, 0, 0) {
            Ok(result) => break result,
            Err(SystemCallError::ReachLimit) => {
                thread::yield_now().expect("storm capacity recovery yield failed");
            }
            Err(error) => panic!("storm capacity recovery failed: {error:?}"),
        }
    };
    assert!(
        probe.tid > last_tid,
        "thread id regressed after capacity recovery"
    );
    assert_eq!(
        wait_until(
            probe.control,
            ObjectSignals::CLOSED,
            libkernel::time::timeout_millis(1).unwrap()
        ),
        Err(SystemCallError::IllegalArgument),
        "ThreadControl accepted unreachable CLOSED interest"
    );
    let observed = wait_until(
        probe.control,
        ObjectSignals::DONE,
        libkernel::time::Deadline::INFINITE,
    )
    .expect("storm probe wait failed");
    assert!(
        observed.observed.contains(ObjectSignals::DONE),
        "storm probe control closed before DONE"
    );
    unsafe { close(probe.control) }.expect("storm probe ThreadControl close failed");
    // raw stack mapping stays owned by this process address space until process drain.
    debug!(
        "hammer target: thread storm passed: {} concurrent members",
        PROCESS_MAX_THREADS
    );
}

fn stale_translation_reuse() {
    let mut region = MappedRegion::map_anonymous(
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

    loop {
        match region.unmap() {
            Ok(()) => break,
            Err((returned, SystemCallError::ObjectBusy)) => {
                region = returned;
                thread::yield_now().expect("thread suite Unmap retry yield failed");
            }
            Err((_, error)) => panic!("thread suite old mapping Unmap failed: {error:?}"),
        }
    }
    let replacement = loop {
        match MappedRegion::map_anonymous(
            PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            MemoryProtection::ReadWrite,
            Placement::FixedEmpty {
                usable_start: address,
            },
        ) {
            Ok(region) => break region,
            Err(SystemCallError::ObjectBusy) => {
                thread::yield_now().expect("thread suite remap retry yield failed");
            }
            Err(error) => panic!("thread suite FixedEmpty remap failed: {error:?}"),
        }
    };
    // SAFETY: replacement 恰好重新取得 address 的 RW usable 页。
    unsafe { (address as *mut u64).write_volatile(NEW_TRANSLATION_VALUE) };
    release.store(true, Ordering::Release);
    let observed = take_thread_result(worker.join());
    assert_eq!(
        observed,
        (OLD_TRANSLATION_VALUE, NEW_TRANSLATION_VALUE),
        "remote hart observed a stale translation after address reuse"
    );
    replacement
        .unmap()
        .expect("thread suite replacement Unmap failed");
}

fn close_churn_endpoint(mut endpoint: librunnel::MappedEndpoint) -> Result<(), SystemCallError> {
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        match endpoint.close() {
            Ok(()) => {
                debug!(
                    "acceptance progress: phase=tunnel-close step=complete attempts={}",
                    attempts
                );
                return Ok(());
            }
            Err((returned, SystemCallError::ObjectBusy)) => {
                endpoint = returned;
                if attempts.is_multiple_of(64) {
                    debug!(
                        "acceptance progress: phase=tunnel-close step=retry attempts={}",
                        attempts
                    );
                }
                // SAFETY: 完整失败事务返回 owner，随后退避。
                unsafe { sys_sleep(1).expect("Tunnel close retry sleep failed") };
            }
            Err((_returned, error)) => return Err(error),
        }
    }
}

fn concurrent_tunnel_close(pool: Handle) {
    for round in 0..8usize {
        debug!(
            "acceptance progress: phase=concurrent-tunnel-close round={}/8 step=start",
            round + 1
        );
        let (endpoint_owner, invitation, bytes) =
            libkernel::ipc::invitation::Invitation::create(pool, 3 * PROCESS_PAGE_SIZE)
                .expect("churn Tunnel create failed");
        let endpoint = librunnel::MappedEndpoint::map(
            endpoint_owner,
            bytes,
            Placement::FixedEmpty {
                usable_start: THREAD_TUNNEL_VA,
            },
        )
        .expect("churn Tunnel Map failed");
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
        let closer = thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || {
                child_ready.store(true, Ordering::Release);
                while !child_release.load(Ordering::Acquire) {
                    core::hint::spin_loop();
                }
                close_churn_endpoint(endpoint)
            })
            .expect("Tunnel close worker spawn failed");
        let mut ready_waits = 0usize;
        while !ready.load(Ordering::Acquire) {
            ready_waits += 1;
            if ready_waits.is_multiple_of(256) {
                debug!(
                    "acceptance progress: phase=concurrent-tunnel-close round={}/8 step=wait-worker waits={}",
                    round + 1,
                    ready_waits
                );
            }
            thread::yield_now().expect("Tunnel close coordinator yield failed");
        }
        release.store(true, Ordering::Release);
        unmap_churn_region(region);
        take_thread_result(closer.join()).expect("concurrent Endpoint close failed");
        invitation.close().expect("Tunnel invitation close failed");
        debug!(
            "acceptance progress: phase=concurrent-tunnel-close round={}/8 step=complete",
            round + 1
        );
    }
}

/// 两个有序结果与真实跨执行点竞争共用同一套断言；不靠某轮恰好命中时序。
fn tunnel_close_attach(pool: Handle) {
    use core::sync::atomic::AtomicUsize;
    const PEER_VA: usize = THREAD_TUNNEL_VA + 0x20_0000;
    for round in 0..24 {
        debug!(
            "acceptance progress: phase=tunnel-close-attach round={}/24 step=start policy={}",
            round + 1,
            round / 8
        );
        let (endpoint_owner, invitation, bytes) =
            libkernel::ipc::invitation::Invitation::create(pool, 3 * PROCESS_PAGE_SIZE)
                .expect("churn Tunnel create failed");
        let creator = librunnel::MappedEndpoint::map(
            endpoint_owner,
            bytes,
            Placement::FixedEmpty {
                usable_start: THREAD_TUNNEL_VA,
            },
        )
        .expect("churn Tunnel Map failed");
        // SAFETY: 仅用于 Attach 后原表项 generation 断言，不另取得 owner。
        let invitation_handle = unsafe { invitation.raw_handle() };
        // SAFETY: 保存值仅用于 owner 关闭后的 generation 退休断言。
        let creator_handle = creator.events().wait_item(ObjectSignals::NONE, 0).handle;
        let phase = Arc::new(AtomicUsize::new(0));
        let child_phase = phase.clone();
        let policy = round / 8;
        let worker = thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || {
                let mut gate_waits = 0usize;
                while child_phase.load(Ordering::Acquire) == 0 {
                    gate_waits += 1;
                    if gate_waits.is_multiple_of(256) {
                        debug!(
                            "acceptance progress: phase=tunnel-close-attach round={}/24 step=wait-release waits={}",
                            round + 1,
                            gate_waits
                        );
                    }
                    thread::yield_now().expect("Attach gate yield failed");
                }
                let mut invitation = invitation;
                let result = loop {
                    match invitation.attach() {
                        Err(failure) if failure.error == SystemCallError::ObjectBusy => {
                            invitation = failure.invitation;
                            thread::yield_now().expect("Attach retry yield failed");
                        }
                        other => break other,
                    }
                };
                match result {
                    Ok(endpoint) => {
                        let endpoint = librunnel::MappedEndpoint::map(endpoint, bytes, Placement::FixedEmpty { usable_start: PEER_VA }).expect("attached Tunnel Map failed");
                        assert_ne!(policy, 1, "Attach succeeded after creator close completed");
                        assert_eq!(
                            unsafe { close(invitation_handle) },
                            Err(SystemCallError::StaleHandle),
                            "Attach did not consume Invitation"
                        );
                        child_phase.store(2, Ordering::Release);
                        let mut close_waits = 0usize;
                        while child_phase.load(Ordering::Acquire) != 3 {
                            close_waits += 1;
                            if close_waits.is_multiple_of(256) {
                                debug!(
                                    "acceptance progress: phase=tunnel-close-attach round={}/24 step=wait-peer-close waits={}",
                                    round + 1,
                                    close_waits
                                );
                            }
                            thread::yield_now().expect("peer close gate yield failed");
                        }
                        // SAFETY: 保存值仅用于成功关闭后的重复 close 验证。
                        let handle = endpoint.events().wait_item(ObjectSignals::NONE, 0).handle;
                        close_churn_endpoint(endpoint).expect("attached Endpoint close failed");
                        assert_eq!(unsafe { close(handle) }, Err(SystemCallError::StaleHandle));
                    }
                    Err(failure) => {
                        assert_eq!(failure.error, SystemCallError::ObjectClosed);
                        assert_ne!(policy, 0, "ordered Attach unexpectedly lost to close");
                        failure.invitation.close().expect("failed Attach lost Invitation owner");
                        assert_eq!(
                            unsafe { close(invitation_handle) },
                            Err(SystemCallError::StaleHandle)
                        );
                        child_phase.store(2, Ordering::Release);
                    }
                }
            })
            .expect("close/Attach worker spawn failed");
        if policy == 1 {
            close_churn_endpoint(creator).expect("ordered creator close failed");
            phase.store(1, Ordering::Release);
        } else {
            phase.store(1, Ordering::Release);
            if policy == 0 {
                while phase.load(Ordering::Acquire) != 2 {
                    thread::yield_now().expect("ordered Attach gate yield failed");
                }
            }
            close_churn_endpoint(creator).expect("racing creator close failed");
        }
        assert_eq!(
            unsafe { close(creator_handle) },
            Err(SystemCallError::StaleHandle)
        );
        debug!(
            "acceptance progress: phase=tunnel-close-attach round={}/24 step=creator-closed",
            round + 1
        );
        // worker 的状态 2 不能在主线程状态 3 后覆盖，先确认 Attach 已结束。
        let mut attach_waits = 0usize;
        while phase.load(Ordering::Acquire) != 2 {
            attach_waits += 1;
            if attach_waits.is_multiple_of(256) {
                debug!(
                    "acceptance progress: phase=tunnel-close-attach round={}/24 step=wait-attach waits={}",
                    round + 1,
                    attach_waits
                );
            }
            thread::yield_now().expect("Attach completion gate yield failed");
        }
        phase.store(3, Ordering::Release);
        take_thread_result(worker.join());
        debug!(
            "acceptance progress: phase=tunnel-close-attach round={}/24 step=worker-joined",
            round + 1
        );
        // 每一轮都用普通映射复用两端 VA，直接确认两笔 lease/PTE 已撤销。
        for address in [THREAD_TUNNEL_VA, PEER_VA] {
            let region = MappedRegion::map_anonymous(
                3 * PROCESS_PAGE_SIZE,
                0,
                0,
                MemoryProtection::ReadWrite,
                Placement::FixedEmpty {
                    usable_start: address,
                },
            )
            .expect("closed Tunnel left a mapping behind");
            unmap_churn_region(region);
        }
        debug!(
            "acceptance progress: phase=tunnel-close-attach round={}/24 step=complete",
            round + 1
        );
    }
    debug!("hammer target: Tunnel close/Attach failure matrix passed: 24 rounds");
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

fn fragmented_backing_retire() {
    const PAGES: usize = 260;

    let region = map_churn_region(PAGES * PROCESS_PAGE_SIZE);
    let usable = region
        .usable()
        .expect("fragmented backing Map has no usable range");
    for page in (0..PAGES).step_by(2) {
        let start = usable.start + page * PROCESS_PAGE_SIZE;
        protect_churn_region(
            &region,
            start..start + PROCESS_PAGE_SIZE,
            MemoryProtection::ReadOnly,
        );
    }
    unmap_churn_region(region);
    debug!(
        "hammer target: fragmented backing retire passed: {} pages",
        PAGES
    );
}

/// 真实consumer在release cookie后访问Map尾页，不等创建者syscall返回。
/// 允许命中尚未发布的PTE；内核必须保留原fault并等待对应事务DONE后重试。
fn committed_mapping_access() {
    // SDK的affine MappedRegion在DONE后交付；此ABI用例直接观察release cookie，
    // 以真实正式syscall验证更早的已承诺访问，不给生产封装添加测试专用入口。
    unsafe fn mapping_call(call: SystemCall, arg0: usize, arg1: usize) -> usize {
        let error;
        // SAFETY: 调用点分别证明Map结果槽与Unmap owner；寄存器合同与libkernel一致，
        // 默认memory effect禁止跨cookie/用户访问重排，内核恢复a0/a1外的用户寄存器。
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a7") call as usize,
                inlateout("a0") arg0 => error,
                inlateout("a1") arg1 => _,
                in("a2") 0usize,
                in("a3") 0usize,
                in("a4") 0usize,
                in("a5") 0usize,
            );
        }
        error
    }

    struct PublishedMap {
        result: UnsafeCell<MemoryMapResult>,
        returned: AtomicBool,
    }
    // SAFETY: cookie之外字段只由内核在release发布前写；worker只经acquire
    // 观察cookie后读这些稳定字段，主线程只在join后解除mapping。
    unsafe impl Sync for PublishedMap {}

    const COOKIE: u64 = 0x6d61_702d_6661_756c;
    const BYTES: usize = 260 * PROCESS_PAGE_SIZE;
    let published = Arc::new(PublishedMap {
        result: UnsafeCell::new(MemoryMapResult::empty()),
        returned: AtomicBool::new(false),
    });
    let worker_slot = published.clone();
    let ready = Arc::new(AtomicBool::new(false));
    let worker_ready = ready.clone();
    let worker = thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || {
            let output = worker_slot.result.get();
            // SAFETY: result所在Arc地址稳定且8字节对齐，只有末尾cookie经原子访问；
            // 此处不为并发写入中的整个结果槽创建共享引用。
            let committed =
                unsafe { AtomicU64::from_ptr(core::ptr::addr_of_mut!((*output).committed)) };
            worker_ready.store(true, Ordering::Release);
            while committed.load(Ordering::Acquire) != COOKIE {
                core::hint::spin_loop();
            }
            let early = !worker_slot.returned.load(Ordering::Acquire);
            // SAFETY: release cookie已交付完整稳定几何；已承诺RW范围直到join后才Unmap。
            let base = unsafe { core::ptr::addr_of!((*output).usable_base).read() } as usize;
            let last = base + BYTES - core::mem::size_of::<u64>();
            // SAFETY: 位于已承诺usable末页；尚无PTE时应走真实fault续体。
            assert_eq!(unsafe { (last as *const u64).read_volatile() }, 0);
            // SAFETY: 同一范围RW，创建者仍持mapping；也覆盖写fault访问合同。
            unsafe { (last as *mut u64).write_volatile(COOKIE) };
            early
        })
        .expect("committed mapping worker spawn failed");
    while !ready.load(Ordering::Acquire) {
        thread::yield_now().expect("committed mapping coordinator yield failed");
    }
    let request = MemoryMapRequest::new(
        BYTES as u64,
        PROCESS_PAGE_SIZE as u64,
        PROCESS_PAGE_SIZE as u64,
        0,
        published.result.get() as u64,
        COOKIE,
        MemoryProtection::ReadWrite,
        MemoryPlacement::Anywhere,
    );
    // SAFETY: request与结果Arc覆盖整个syscall；结果cookie非零且唯一属于本槽。
    assert_eq!(
        unsafe { mapping_call(SystemCall::MemoryMap, &request as *const _ as usize, 0) },
        0,
        "committed mapping Map failed"
    );
    published.returned.store(true, Ordering::Release);
    let early = take_thread_result(worker.join());
    // SAFETY: syscall完成后结果全部稳定，worker已结束，本线程独占解除责任。
    let result = unsafe { published.result.get().read() };
    let base = result.usable_base as usize;
    // SAFETY: mapping仍有效，worker已join，无并发访问。
    assert_eq!(
        unsafe { ((base + BYTES - 8) as *const u64).read_volatile() },
        COOKIE
    );
    loop {
        // SAFETY: 本线程拥有刚提交的完整reservation，成功前持续保留其责任。
        match unsafe {
            mapping_call(
                SystemCall::MemoryUnmap,
                result.reservation_base as usize,
                result.reservation_bytes as usize,
            )
        } {
            0 => break,
            error if error == SystemCallError::ObjectBusy as usize => {
                thread::yield_now().expect("committed mapping Unmap retry failed");
            }
            error => panic!("committed mapping Unmap failed: {error}"),
        }
    }
    debug!("hammer target: committed mapping access passed: cookie before return={early}");
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
