//! hammer：生命周期多核竞态矩阵的执行器与竞态靶（协议见
//! `libprocess::race`，剧本与断言归 init）。
//!
//! HAMMER 模式是受编排的 syscall 执行器：指令与 handle 经 Mailbox 运行
//! 投递，等发令枪 READABLE 后立即执行单条指令并回执——窗口密度即本
//! 负载的存在意义，锤内不做判定。TARGET 模式是竞态靶：等枪后按角色
//! 自灭（Suicide/Fault）或高频 park，与锤的 kill 同刻起跑。

#![no_std]

use libprocess::race::{
    self, Cmd, Report, HAMMER_CMD, HAMMER_CONTROL_RIGHTS, HAMMER_GUN, HAMMER_REPORT, MODE_HAMMER,
    MODE_TARGET, MSG_CMD, MSG_REPORT, TARGET_FAULT, TARGET_GUN, TARGET_PARK, TARGET_SUICIDE,
    ACTION_CLOSE, ACTION_CREATE, ACTION_CREATE_ABANDON, ACTION_DRAIN, ACTION_EXIT, ACTION_KILL,
    ACTION_SEAL, ACTION_START, ACTION_ENUMERATE,
};
use rinlib::{
    env,
    ipc::{
        message::{send, wait_message},
        notification,
        object::close,
        wait::wait_many,
    },
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        object::{Handle, ObjectSignals},
        proc::{ExecutionProfile, JobMemberKind, ProcessStartDescriptor},
        wait::{WaitItem, WAIT_DEADLINE_INFINITE},
    },
    sys_exit, sys_sleep,
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
    let _ = wait_many(&items, WAIT_DEADLINE_INFINITE);
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
                &race::encode_report(&Report { status: 0, aux0: 0, aux1: 0 }, &[]),
                &[],
            );
            return;
        }
        await_gun(gun);
        let (report, tail) = execute(&cmd, &message.handles);
        let _ = send(report_box, MSG_REPORT, &race::encode_report(&report, &tail), &[]);
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
            Report { status: SystemCallError::IllegalArgument as i64, aux0: other, aux1: 0 },
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
    (Report { status, aux0: 0, aux1: 0 }, alloc::vec::Vec::new())
}

fn start_target(cmd: &Cmd, handles: &[Handle]) -> Result<(), SystemCallError> {
    let descriptor = ProcessStartDescriptor {
        entry: cmd.entry,
        stack_pointer: cmd.sp,
        payload_ptr: 0,
        grants_ptr: 0,
        payload_len: 0,
        grant_count: 0,
        profile: ExecutionProfile::Base64 as u32,
        reserved: 0,
    };
    process::start(handles[0], &descriptor)
}

fn create(handles: &[Handle], abandon: bool) -> (Report, alloc::vec::Vec<u64>) {
    let report = match process::create(handles[0], HAMMER_CONTROL_RIGHTS) {
        Ok(created) => {
            if abandon {
                let _ = close(created.builder);
                let _ = close(created.control);
                Report { status: 0, aux0: created.pid, aux1: 0 }
            } else {
                Report { status: 0, aux0: created.pid, aux1: 0 }
            }
        }
        Err(error) => Report { status: error as i64, aux0: 0, aux1: 0 },
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
            Report { status: error as i64, aux0: 0, aux1: 0 },
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
                    return (Report { status: 0, aux0: 0, aux1: 0 }, ids);
                }
                cursor = result.next_cursor;
            }
            Err(error) => {
                return (Report { status: error as i64, aux0: 0, aux1: 0 }, ids);
            }
        }
    }
    (
        Report { status: SystemCallError::ReachLimit as i64, aux0: 0, aux1: 0 },
        ids,
    )
}

fn target_mode(words: &[u64]) {
    let Some(gun) = env::startup_handle(TARGET_GUN) else {
        debug!("hammer target: missing gun");
        return;
    };
    await_gun(gun);
    match words.get(1).copied().unwrap_or(0) {
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
        TARGET_PARK => loop {
            // SAFETY: 值参数；高频 park 供 kill 取消线竞速。
            unsafe { sys_sleep(1).expect("hammer target park sleep") };
        },
        other => debug!("hammer target: unknown subrole {}", other),
    }
}
