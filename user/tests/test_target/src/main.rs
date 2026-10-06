//! target：kill 监督靶子——长寿命 sleep 循环，被外部 ProcessKill 终止
//! 是其唯一正常出口（验证 Waiting 取消与幂等竞争，见 init 剧本）。

#![no_std]

extern crate libruntime;

use libkernel::{
    ipc::{capability::Capability, notification, wait_set::WaitSet},
    preclude::*,
    shared::{object::ObjectSignals, wait_set::MemberRequest},
    sys_sleep,
};

fn main() {
    let startup = libruntime::startup::take().expect("target startup context is missing");
    let (_process, payload, mut grants) = startup.into_parts();
    if payload.as_bytes() == b"ipc-kill" {
        ipc_kill_target(&mut grants);
    }
    if payload.as_bytes() == b"retirement" {
        retirement_target(&mut grants);
    }
    debug!("target alive");
    loop {
        // SAFETY: 值参数；本进程只经 kill 退出。
        unsafe { sys_sleep(1000).expect("target sleep") };
    }
}

fn ipc_kill_target(startup: &mut libruntime::startup::InitialGrants) -> ! {
    let owner = startup
        .take(0)
        .map(Capability::into_raw)
        .expect("IPC kill target missing Close owner");
    let control = startup
        .take(1)
        .map(Capability::into_raw)
        .expect("IPC kill target missing observed process control");
    let ready = startup
        .take(2)
        .map(Capability::into_raw)
        .expect("IPC kill target missing ready signaler");
    let done = startup
        .take(3)
        .map(Capability::into_raw)
        .expect("IPC kill target missing observed completion signaler");
    let close = libruntime::thread::Builder::new()
        .spawn(move || {
            // SAFETY: startup 转入的 owner 只有本线程承担关闭责任。
            unsafe { libkernel::ipc::object::close(owner) }
                .expect("IPC target committed Close failed");
        })
        .expect("IPC Close thread spawn failed");
    let observer = libruntime::thread::Builder::new()
        .spawn(move || {
            libkernel::process::wait_done(control).expect("IPC target completion wait failed");
            notification::signal(done, 1).expect("IPC target completion signal failed");
        })
        .expect("IPC observation thread spawn failed");
    notification::signal(ready, 1).expect("IPC target active publication failed");
    // 两个 ResultReceiver 与未返回线程由正式进程退出路径接管；主线程进入用户 spin，
    // 外部 Kill 允许与 Close/observation 的自然完成或取消竞速，精确 Waiting 窗口由 GDB 取证。
    loop {
        core::hint::black_box((&close, &observer));
        core::hint::spin_loop();
    }
}

fn retirement_target(startup: &mut libruntime::startup::InitialGrants) -> ! {
    let source = startup
        .take(0)
        .map(Capability::into_raw)
        .expect("retirement target missing observation source");
    let ready = startup
        .take(1)
        .map(Capability::into_raw)
        .expect("retirement target missing ready signaler");
    let set = WaitSet::create(256).expect("retirement target WaitSet creation failed");
    for cookie in 0..256 {
        set.register(MemberRequest::new(source, ObjectSignals::READABLE, cookie))
            .expect("retirement target registration failed");
    }
    notification::signal(ready, 1).expect("retirement target ready publication failed");
    loop {
        core::hint::black_box(set.handle());
        // SAFETY: 值参数；集合 owner 与来源由进程退出的正式退休路径接管。
        unsafe { sys_sleep(1000).expect("retirement target sleep failed") };
    }
}
