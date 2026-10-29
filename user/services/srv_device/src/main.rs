//! 中央设备登记服务；进程只组合已有执行/运输机制，不选择或启动驱动。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use erhino_shared::object::{ObjectKind, Rights};
use libpci::platform::{DESCRIPTION_RIGHTS, DescriptionObject, Platform, ROOT_RIGHTS};
use libprocess::birth::Birth;
use libservice::instance::{self, child::Reporter};
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
    let reporter = Reporter::new(
        birth
            .require(
                instance::REPORT,
                instance::REPORT_KIND,
                instance::REPORT_RIGHTS,
            )
            .expect("device report input invalid"),
    )
    .expect("device report sender invalid");
    let stop = birth
        .require(instance::STOP, instance::STOP_KIND, instance::STOP_RIGHTS)
        .expect("device stop input invalid");
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
        reporter,
        stop,
        pool,
        platform,
        config,
    });
}
