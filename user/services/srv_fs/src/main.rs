//! 内存文件 provider 的进程装配。

#![no_std]
#![feature(allocator_api)]

extern crate libruntime;

extern crate alloc;

mod server;

use erhino_shared::object::{Handle, Rights};
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender},
};

fn startup_handle(
    startup: &mut libruntime::startup::InitialGrants,
    index: usize,
    missing: &'static str,
) -> Handle {
    startup
        .take(index)
        .map(Capability::into_raw)
        .expect(missing)
}

fn startup_sender(
    startup: &mut libruntime::startup::InitialGrants,
    index: usize,
    missing: &'static str,
) -> MailboxSender {
    let capability = startup.take(index).expect(missing);
    MailboxSender::from_capability(capability)
        .map(|(sender, _)| sender)
        .map_err(|failure| failure.error)
        .expect("provider bootstrap sender has an invalid role")
}

fn main() {
    let startup = libruntime::startup::take().expect("provider startup context is missing");
    let (process, payload, mut grants) = startup.into_parts();
    let instance = match payload.as_bytes() {
        b"" => server::Instance::Primary,
        b"secondary" => server::Instance::Secondary {
            parent_pid: process.parent_pid(),
        },
        _ => panic!("provider instance role invalid"),
    };
    let bootstrap = startup_sender(&mut grants, 0, "provider bootstrap sender is missing");
    let release = startup_handle(&mut grants, 1, "provider release owner is missing");
    let route = startup_handle(&mut grants, 2, "provider route mailbox owner is missing");
    let registration = startup_handle(&mut grants, 3, "provider registration endpoint is missing");
    let pool = grants.take(4).expect("provider funding Pool is missing");
    let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("provider mailbox creation failed");
    server::run(
        mailbox,
        bootstrap,
        route,
        release,
        registration,
        pool,
        instance,
    );
}
