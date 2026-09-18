//! target：kill 监督靶子——长寿命 sleep 循环，被外部 ProcessKill 终止
//! 是其唯一正常出口（验证 Waiting 取消与幂等竞争，见 init 剧本）。

#![no_std]

use rinlib::{
    env,
    ipc::{notification, wait_set::WaitSet},
    preclude::*,
    shared::{
        call::SystemCallError, object::ObjectSignals, proc::ProcessDrainStatus, wait::WaitItem,
    },
    sys_sleep,
};

fn main() {
    if env::startup_payload() == b"ipc-kill" {
        ipc_kill_target();
    }
    if env::startup_payload() == b"retirement" {
        retirement_target();
    }
    debug!("target alive");
    loop {
        // SAFETY: 值参数；本进程只经 kill 退出。
        unsafe { sys_sleep(1000).expect("target sleep") };
    }
}

fn ipc_kill_target() -> ! {
    let owner = env::startup_handle(0).expect("IPC kill target missing Close owner");
    let control = env::startup_handle(1).expect("IPC kill target missing Drain control");
    let ready = env::startup_handle(2).expect("IPC kill target missing ready signaler");
    let done = env::startup_handle(3).expect("IPC kill target missing Drain completion signaler");
    let close = rinlib::thread::Builder::new()
        .spawn(move || {
            // SAFETY: startup 转入的 owner 只有本线程承担关闭责任。
            unsafe { rinlib::ipc::object::close(owner) }
                .expect("IPC target committed Close failed");
        })
        .expect("IPC Close thread spawn failed");
    let drain = rinlib::thread::Builder::new()
        .spawn(move || {
            loop {
                match rinlib::process::drain(control, 128) {
                    Ok(result) if result.status == ProcessDrainStatus::Complete as u32 => break,
                    Ok(_) => {}
                    Err(SystemCallError::ObjectBusy) => {
                        rinlib::thread::yield_now().expect("IPC Drain ownership retry failed");
                    }
                    Err(error) => panic!("IPC target captured Drain failed: {error:?}"),
                }
            }
            notification::signal(done, 1).expect("IPC target Drain completion signal failed");
        })
        .expect("IPC Drain thread spawn failed");
    notification::signal(ready, 1).expect("IPC target active publication failed");
    // 两个 JoinHandle 与未返回线程由正式进程退出路径接管；主线程进入用户 spin，
    // 外部 Kill 允许与 Close/Drain 的自然完成或取消竞速，精确 Waiting 窗口由 GDB 取证。
    loop {
        core::hint::black_box((&close, &drain));
        core::hint::spin_loop();
    }
}

fn retirement_target() -> ! {
    let source = env::startup_handle(0).expect("retirement target missing observation source");
    let ready = env::startup_handle(1).expect("retirement target missing ready signaler");
    let set = WaitSet::create(256).expect("retirement target WaitSet creation failed");
    for cookie in 0..256 {
        set.register(WaitItem::new(source, ObjectSignals::READABLE, cookie))
            .expect("retirement target registration failed");
    }
    notification::signal(ready, 1).expect("retirement target ready publication failed");
    loop {
        core::hint::black_box(set.handle());
        // SAFETY: 值参数；集合 owner 与来源由进程退出的正式退休路径接管。
        unsafe { sys_sleep(1000).expect("retirement target sleep failed") };
    }
}
