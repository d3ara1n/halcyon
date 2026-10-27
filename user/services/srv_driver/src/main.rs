//! 统一驱动选择服务；装配政策只来自 init 的出生输入，worker 运行期回复不能修改。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use alloc::vec::Vec;
use erhino_shared::object::{ObjectKind, Rights};
use libdriver::boot::{self, Catalog};
use libkernel::ipc::{capability::Capability, message::MailboxSender};
use libprocess::image::Image;
mod server;

fn authority(cap: Capability, kind: ObjectKind, rights: Rights) -> Capability {
    let description = cap
        .description()
        .expect("driver startup authority query failed");
    assert!(
        description.kind == kind as u32 && description.rights.contains(rights),
        "driver startup authority invalid"
    );
    cap
}
fn main() {
    let startup = libruntime::startup::take().expect("driver startup context missing");
    let (process, payload, mut grants) = startup.into_parts();
    let catalog = Catalog::decode(payload.as_bytes(), grants.remaining())
        .expect("driver boot declaration pairing invalid");
    let (bootstrap, _) = MailboxSender::from_capability(
        grants
            .take(boot::BOOTSTRAP)
            .expect("driver bootstrap missing"),
    )
    .map_err(|failure| failure.error)
    .expect("driver bootstrap invalid");
    let pool = authority(
        grants.take(boot::POOL).expect("driver Pool missing"),
        ObjectKind::MemoryPool,
        Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::GRANT,
    );
    let job = authority(
        grants.take(boot::DRIVER_JOB).expect("driver Job missing"),
        ObjectKind::Job,
        Rights::READ | Rights::WAIT | Rights::CREATE | Rights::MANAGE,
    );
    let (claim, _) = MailboxSender::from_capability(
        grants
            .take(boot::CLAIM_SOURCE)
            .expect("driver ClaimSource missing"),
    )
    .map_err(|failure| failure.error)
    .expect("driver ClaimSource invalid");
    let mut images = Vec::new();
    images
        .try_reserve_exact(catalog.count())
        .expect("driver image records allocation failed");
    for (index, declaration) in catalog.records().enumerate() {
        let declaration = declaration.expect("validated driver declaration changed");
        let cap = grants
            .take(
                catalog
                    .image_slot(index)
                    .expect("driver image slot invalid"),
            )
            .expect("driver image missing");
        images.push(
            Image::adopt(
                cap,
                usize::try_from(declaration.image_bytes).expect("driver image length invalid"),
            )
            .expect("driver image authority invalid"),
        );
    }
    assert_eq!(
        grants.remaining(),
        0,
        "driver startup left unbound authority"
    );
    server::run(process, bootstrap, pool, job, claim, catalog, images);
}
