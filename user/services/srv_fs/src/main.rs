//! FAL2 内存 provider 的正式进程装配。

#![no_std]

extern crate alloc;

mod server;

use erhino_shared::object::{Handle, Rights};
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender},
};

fn startup_handle(index: usize, missing: &'static str) -> Handle {
    rinlib::env::startup_handle(index).expect(missing)
}

fn startup_sender(index: usize, missing: &'static str) -> MailboxSender {
    let handle = startup_handle(index, missing);
    // SAFETY: StartupBlock transfers this capability exclusively into srv_fs.
    let capability = unsafe { Capability::from_raw(handle) };
    MailboxSender::from_capability(capability)
        .map(|(sender, _)| sender)
        .map_err(|failure| failure.error)
        .expect("provider bootstrap sender has an invalid role")
}

fn main() {
    let bootstrap = startup_sender(0, "provider bootstrap sender is missing");
    let release = startup_handle(1, "provider release owner is missing");
    let route = startup_handle(2, "provider route mailbox owner is missing");
    let registration = startup_handle(3, "provider registration endpoint is missing");
    let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("provider mailbox creation failed");
    server::run(mailbox, bootstrap, route, release, registration);
}
