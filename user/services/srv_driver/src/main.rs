//! 统一驱动选择服务；装配政策只来自出生输入与实例设定，worker 运行期回复不能修改。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use alloc::{string::String, vec::Vec};
use erhino_shared::object::{ObjectKind, Rights};
use libdriver::boot::Catalog;
use libkernel::ipc::message::MailboxSender;
use libprocess::{
    birth::Birth,
    image::{IMAGE_RIGHTS, Image},
};
use libservice::instance::{self, child::Reporter};
mod server;

/// 设备来源输入；由 srv_device 预签发、经组态路由。
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
    let reporter = Reporter::new(
        birth
            .require(
                instance::REPORT,
                instance::REPORT_KIND,
                instance::REPORT_RIGHTS,
            )
            .expect("driver report input invalid"),
    )
    .expect("driver report sender invalid");
    let stop = birth
        .require(instance::STOP, instance::STOP_KIND, instance::STOP_RIGHTS)
        .expect("driver stop input invalid");
    let pool = birth
        .require(
            "memory.pool",
            ObjectKind::MemoryPool,
            Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::GRANT,
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
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
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
        reporter,
        stop,
        pool,
        job,
        claim,
        catalog,
        images,
    });
}
