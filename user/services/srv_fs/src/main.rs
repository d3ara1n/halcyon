//! 内存文件 provider 的进程装配。

#![no_std]
#![feature(allocator_ext)]

extern crate libruntime;

extern crate alloc;

mod server;

use erhino_shared::object::Rights;
use libkernel::ipc::message::{Mailbox, MailboxSender};

fn main() {
    let startup = libruntime::startup::take().expect("provider startup context is missing");
    let (process, payload, mut grants) = startup.into_parts();
    let profile = srv_fs::startup::Profile::decode(payload.as_bytes())
        .expect("provider startup profile invalid");
    assert_eq!(
        grants.remaining(),
        srv_fs::startup::GRANTS,
        "provider startup grant count invalid"
    );
    let expected = [
        (
            erhino_shared::object::ObjectKind::MailboxSender,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        ),
        (
            erhino_shared::object::ObjectKind::NotificationOwner,
            Rights::READ | Rights::WAIT,
        ),
        (
            erhino_shared::object::ObjectKind::MailboxReceiver,
            Rights::READ | Rights::WAIT | Rights::MANAGE,
        ),
        (
            erhino_shared::object::ObjectKind::MailboxReceiver,
            Rights::READ | Rights::WAIT | Rights::MANAGE,
        ),
        (
            erhino_shared::object::ObjectKind::MemoryPool,
            Rights::CREATE,
        ),
    ];
    let mut caps: [_; srv_fs::startup::GRANTS] = core::array::from_fn(|index| grants.take(index));
    for (index, (kind, rights)) in expected.into_iter().enumerate() {
        let capability = caps[index]
            .as_ref()
            .expect("provider startup grant missing");
        let description = libkernel::ipc::object::query(capability.as_handle())
            .expect("provider grant query failed");
        assert!(
            description.kind == kind as u32 && description.rights == rights,
            "provider startup grant role or rights invalid"
        );
    }
    let bootstrap =
        MailboxSender::from_capability(caps[0].take().expect("validated bootstrap grant"))
            .map_err(|f| f.error)
            .expect("provider bootstrap sender invalid")
            .0;
    let release = caps[1].take().expect("validated release grant").into_raw();
    let route = caps[2].take().expect("validated route grant").into_raw();
    let registration = caps[3]
        .take()
        .expect("validated registration grant")
        .into_raw();
    let pool = caps[4].take().expect("validated Pool grant");
    let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("provider mailbox creation failed");
    server::run(
        mailbox,
        server::Startup {
            bootstrap,
            route,
            release,
            registration,
            pool,
            profile,
            parent_pid: process.parent_pid(),
        },
    );
}
