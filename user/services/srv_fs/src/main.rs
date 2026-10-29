//! 内存文件 provider 的进程装配。

#![no_std]
#![feature(allocator_ext)]

extern crate libruntime;

extern crate alloc;

mod server;

use erhino_shared::object::{ObjectKind, Rights};
use libkernel::ipc::message::{Mailbox, MailboxSender};
use libprocess::birth::Birth;
use libservice::instance::{self, child::Reporter};
use srv_fs::startup::{AUTHORITY_INPUT, Config, POOL_INPUT};

fn main() {
    let mut birth =
        Birth::adopt(libruntime::startup::take().expect("provider startup context is missing"))
            .expect("provider birth frame invalid");
    let config = Config::parse(birth.settings()).expect("provider settings invalid");
    let reporter = Reporter::new(
        birth
            .require(
                instance::REPORT,
                instance::REPORT_KIND,
                instance::REPORT_RIGHTS,
            )
            .expect("provider report input invalid"),
    )
    .expect("provider report sender invalid");
    let stop = birth
        .require(instance::STOP, instance::STOP_KIND, instance::STOP_RIGHTS)
        .expect("provider stop input invalid");
    let pool = birth
        .require(POOL_INPUT, ObjectKind::MemoryPool, Rights::CREATE)
        .expect("provider Pool input invalid");
    // 不承载 registry 的宿主经 publication authority 登记自己的文件根。
    let authority = birth
        .take(
            AUTHORITY_INPUT,
            ObjectKind::MailboxSender,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .expect("provider publication authority invalid")
        .map(|capability| {
            MailboxSender::from_capability(capability)
                .map_err(|failure| failure.error)
                .expect("provider publication authority role invalid")
                .0
        });
    birth.finish().expect("provider birth has unclaimed inputs");
    assert_eq!(
        authority.is_some(),
        !config.registry,
        "publication authority must accompany exactly the non-registry host"
    );
    let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("provider mailbox creation failed");
    server::run(
        mailbox,
        server::Startup {
            reporter,
            stop,
            pool,
            config,
            authority,
        },
    );
}
