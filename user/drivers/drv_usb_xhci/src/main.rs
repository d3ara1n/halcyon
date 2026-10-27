//! 首批 xHCI provider；仅消费限定 xHCI function 供应，所有访问受原绑定停止合同约束。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
mod server;
fn main() {
    let startup = libruntime::startup::take().expect("xHCI startup missing");
    let (_, payload, mut grants) = startup.into_parts();
    assert!(grants.remaining() == 2, "xHCI startup invalid");
    let configuration = libdriver::boot::WorkerConfig::decode(payload.as_bytes())
        .expect("driver configuration invalid");
    assert!(
        configuration.capacity == 1,
        "driver activation capacity unsupported"
    );
    let (bootstrap, _) = libkernel::ipc::message::MailboxSender::from_capability(
        grants.take(0).expect("xHCI bootstrap missing"),
    )
    .map_err(|failure| failure.error)
    .expect("xHCI bootstrap invalid");
    let pool = grants.take(1).expect("xHCI Pool missing");
    let info = pool.description().expect("xHCI Pool query failed");
    assert!(
        info.kind == erhino_shared::object::ObjectKind::MemoryPool as u32
            && info.rights.contains(erhino_shared::object::Rights::CREATE),
        "xHCI Pool invalid"
    );
    server::run(bootstrap, pool);
}
