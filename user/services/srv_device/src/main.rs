//! 中央设备登记服务；进程只组合已有执行/运输机制，不选择或启动驱动。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use erhino_shared::object::{ObjectKind, Rights};
use libkernel::ipc::message::{Mailbox, MintedSender};
use liboal::Namespace;
use libpci::platform::{DESCRIPTION_RIGHTS, DescriptionObject, Platform, ROOT_RIGHTS};
use libprocess::birth::Birth;
mod config;
mod server;

/// 平台根列表中允许的控制种类；具体资源几何由 `Platform::adopt` 核验。
const ROOT_KINDS: [ObjectKind; 3] = [
    ObjectKind::MmioControl,
    ObjectKind::IrqControl,
    ObjectKind::DmaDomainControl,
];

fn main() {
    let mut birth =
        Birth::adopt(libruntime::startup::take().expect("device startup context missing"))
            .expect("device birth frame invalid");
    let config = config::Config::parse(birth.settings())
        .unwrap_or_else(|invalid| panic!("device settings invalid: {}", invalid.0));
    let namespace = Namespace::adopt_birth(&mut birth).expect("device Namespace input invalid");
    let (mailbox, _) = Mailbox::from_capability(
        birth
            .require(
                "device.receiver",
                ObjectKind::MailboxReceiver,
                Rights::READ | Rights::WAIT | Rights::MANAGE,
            )
            .expect("device receiver input invalid"),
    )
    .map_err(|failure| failure.error)
    .expect("device receiver role invalid");
    let mut sources = alloc::vec::Vec::new();
    sources
        .try_reserve_exact(config.sources.len())
        .expect("device static source allocation failed");
    let mut contexts = alloc::vec::Vec::new();
    contexts
        .try_reserve_exact(config.sources.len() * 2)
        .expect("device static context allocation failed");
    for index in 0..config.sources.len() {
        let source = adopt_pair(
            &mut birth,
            &mailbox,
            "device.source",
            "device.source_lifetime",
            index,
        );
        let admin = adopt_pair(
            &mut birth,
            &mailbox,
            "device.admin",
            "device.admin_lifetime",
            index,
        );
        for pair in [&source, &admin] {
            let context = pair
                .sender
                .description()
                .expect("device static source identity invalid")
                .object_id;
            assert!(
                !contexts.contains(&context),
                "device static source context duplicated"
            );
            contexts.push(context);
        }
        sources.push(server::SourcePair { source, admin });
    }
    let pool = birth
        .require("memory.pool", ObjectKind::MemoryPool, Rights::CREATE)
        .expect("device Pool input invalid");
    let description = DescriptionObject::adopt(
        birth
            .require(
                "platform.description",
                ObjectKind::MemoryObject,
                DESCRIPTION_RIGHTS,
            )
            .expect("device platform description input invalid"),
    )
    .expect("device platform description invalid");
    let roots = birth
        .take_list("platform.root", &ROOT_KINDS, ROOT_RIGHTS)
        .expect("device platform root input invalid");
    birth.finish().expect("device birth has unclaimed inputs");
    assert_eq!(
        roots.len(),
        description.description().header().resource_count,
        "device platform root count mismatch"
    );
    let platform =
        Platform::adopt(description, roots).expect("device platform root binding invalid");
    server::run(server::Startup {
        namespace,
        mailbox,
        sources,
        pool,
        platform,
        config,
    });
}

/// 运输关联由 libkernel 的完整 pair 采用器核验；本地角色冲突另在全部输入领取后检查。
fn adopt_pair(
    birth: &mut Birth,
    mailbox: &Mailbox,
    sender_prefix: &str,
    lifetime_prefix: &str,
    index: usize,
) -> MintedSender {
    let sender = birth
        .require(
            &alloc::format!("{sender_prefix}.{index}"),
            ObjectKind::MailboxSender,
            Rights::WRITE | Rights::WAIT,
        )
        .expect("device static sender input invalid");
    let lifetime = birth
        .require(
            &alloc::format!("{lifetime_prefix}.{index}"),
            ObjectKind::LifetimeObserver,
            Rights::WAIT,
        )
        .expect("device static Lifetime input invalid");
    MintedSender::from_capabilities(mailbox, sender, lifetime, Rights::WRITE | Rights::WAIT)
        .map_err(|failure| failure.error)
        .expect("device static sender pair invalid")
}
