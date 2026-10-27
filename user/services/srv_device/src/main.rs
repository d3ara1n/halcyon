//! 中央设备登记服务；进程只组合已有执行/运输机制，不选择或启动驱动。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use alloc::vec::Vec;
use libdevice::startup::{self, Delegation};
use libpci::platform::{DescriptionObject, Platform};
mod server;

fn main() {
    let startup = libruntime::startup::take().expect("device startup context missing");
    let (_, payload, mut grants) = startup.into_parts();
    let config = Delegation::decode(payload.as_bytes(), grants.remaining())
        .expect("device platform delegation invalid");
    let description = DescriptionObject::adopt(
        grants
            .take(startup::DESCRIPTION)
            .expect("device platform description missing"),
    )
    .expect("device platform description invalid");
    assert_eq!(
        config.count(),
        description.description().header().resource_count,
        "device platform delegation count mismatch"
    );
    let mut roots = Vec::new();
    roots
        .try_reserve_exact(config.count())
        .expect("device platform input storage failed");
    for index in 0..config.count() {
        roots.push(
            grants
                .take(config.grant(index).expect("device platform record missing"))
                .expect("device platform root missing"),
        );
    }
    let platform =
        Platform::adopt(description, roots).expect("device platform root binding invalid");
    let bootstrap = grants
        .take(startup::BOOTSTRAP)
        .expect("device bootstrap sender missing");
    let (bootstrap, description) =
        libkernel::ipc::message::MailboxSender::from_capability(bootstrap)
            .map_err(|failure| failure.error)
            .expect("device bootstrap sender invalid");
    assert!(
        description
            .rights
            .contains(erhino_shared::object::Rights::WRITE),
        "device bootstrap sender rights invalid"
    );
    let pool = grants.take(startup::POOL).expect("device Pool missing");
    assert_eq!(
        pool.description().expect("device Pool query failed").kind,
        erhino_shared::object::ObjectKind::MemoryPool as u32,
        "device Pool role invalid"
    );
    server::run(bootstrap, pool, platform);
}
