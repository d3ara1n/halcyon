//! 统一驱动选择服务；装配政策只来自出生输入与实例设定，worker 运行期回复不能修改。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use alloc::{string::String, vec::Vec};
use erhino_shared::object::{ObjectKind, Rights};
use libdriver::boot::Catalog;
use libkernel::ipc::message::{Mailbox, MailboxSender, MintedSender};
use liboal::Namespace;
use libprocess::{
    birth::Birth,
    image::{IMAGE_RIGHTS, Image},
};
mod server;

/// 设备来源输入；由组态接线、srv_device 采用其领域权限。
const DEVICE_SOURCE: &str = "device.source";
/// worker 所在 Job。
const WORKERS: &str = "driver.workers";
/// 实现映像输入前缀：`driver.image.<组件名>`，组件名即声明的实现键。
const IMAGE_PREFIX: &str = "driver.image";

fn main() {
    let mut birth =
        Birth::adopt(libruntime::startup::take().expect("driver startup context missing"))
            .expect("driver birth frame invalid");
    let catalog = Catalog::from_settings(birth.settings()).expect("driver declarations invalid");
    let namespace = Namespace::adopt_birth(&mut birth).expect("driver Namespace input invalid");
    let control = adopt_endpoint(&mut birth, "driver.control");
    let observer = adopt_endpoint(&mut birth, "driver.observer");
    let pool = birth
        .require(
            "memory.pool",
            ObjectKind::MemoryPool,
            Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::TRANSFER,
        )
        .expect("driver Pool input invalid");
    let job = birth
        .require(
            WORKERS,
            ObjectKind::Job,
            Rights::READ | Rights::WAIT | Rights::CREATE | Rights::MANAGE,
        )
        .expect("driver worker Job input invalid");
    let (claim, _) = MailboxSender::from_capability(
        birth
            .require(
                DEVICE_SOURCE,
                ObjectKind::MailboxSender,
                Rights::WRITE | Rights::WAIT | Rights::TRANSFER,
            )
            .expect("driver device source input invalid"),
    )
    .map_err(|failure| failure.error)
    .expect("driver device source role invalid");
    let mut images = Vec::new();
    images
        .try_reserve_exact(catalog.count())
        .expect("driver image records allocation failed");
    let mut name = String::new();
    for declaration in catalog.records() {
        let declaration = declaration.expect("validated driver declaration changed");
        name.clear();
        core::fmt::write(
            &mut name,
            format_args!("{IMAGE_PREFIX}.{}", declaration.key),
        )
        .expect("String write cannot fail");
        let capability = birth
            .require(&name, ObjectKind::MemoryObject, IMAGE_RIGHTS)
            .expect("driver image input invalid");
        images.push(Image::adopt_sealed(capability).expect("driver image authority invalid"));
    }
    let process = birth.process();
    birth.finish().expect("driver birth has unclaimed inputs");
    server::run(server::Startup {
        process,
        namespace,
        control,
        observer,
        pool,
        job,
        claim,
        catalog,
        images,
    });
}

/// 接收真实的独立队列与完整身份 pair；错误时 owner 沿致命出生失败释放。
fn adopt_endpoint(birth: &mut Birth, prefix: &str) -> server::Endpoint {
    let (mailbox, _) = Mailbox::from_capability(
        birth
            .require(
                &alloc::format!("{prefix}.receiver"),
                ObjectKind::MailboxReceiver,
                Rights::READ | Rights::WAIT,
            )
            .expect("driver receiver input invalid"),
    )
    .map_err(|failure| failure.error)
    .expect("driver receiver role invalid");
    let sender = birth
        .require(
            &alloc::format!("{prefix}.sender"),
            ObjectKind::MailboxSender,
            Rights::WRITE | Rights::WAIT,
        )
        .expect("driver sender input invalid");
    let lifetime = birth
        .require(
            &alloc::format!("{prefix}.lifetime"),
            ObjectKind::LifetimeObserver,
            Rights::WAIT,
        )
        .expect("driver Lifetime input invalid");
    let pair =
        MintedSender::from_capabilities(&mailbox, sender, lifetime, Rights::WRITE | Rights::WAIT)
            .map_err(|failure| failure.error)
            .expect("driver sender pair invalid");
    server::Endpoint { mailbox, pair }
}
