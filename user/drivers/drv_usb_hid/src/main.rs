//! 首批 HID worker；只消费限定USB会话，不持物理资源。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
mod server;
fn main() {
    let startup = libruntime::startup::take().expect("HID startup missing");
    let (_, payload, mut grants) = startup.into_parts();
    assert!(grants.remaining() == 2, "HID startup invalid");
    let configuration = libdriver::boot::WorkerConfig::decode(payload.as_bytes())
        .expect("driver configuration invalid");
    assert!(
        configuration.capacity <= 2,
        "driver activation capacity unsupported"
    );
    let (bootstrap, _) = libkernel::ipc::message::MailboxSender::from_capability(
        grants.take(0).expect("HID bootstrap missing"),
    )
    .map_err(|failure| failure.error)
    .expect("HID bootstrap invalid");
    let pool = grants.take(1).expect("HID Pool missing");
    let info = pool.description().expect("HID Pool query failed");
    assert!(
        info.kind == erhino_shared::object::ObjectKind::MemoryPool as u32
            && info.rights.contains(erhino_shared::object::Rights::CREATE),
        "HID Pool invalid"
    );
    server::run(bootstrap, pool, configuration.capacity);
}
