//! init 的正式文件/服务消费者与启动交付核对。
pub(super) mod acceptance;
pub(super) mod copy;
pub(super) mod registry;
use super::RootSupervisor;
pub(super) mod hosting;
mod launch;
use alloc::sync::Arc;
use erhino_shared::{
    object::{Handle, ObjectSignals, Rights},
    time::Deadline,
};
pub(super) use hosting::Host;
use libfs::{FsRights, client::Client, node::NodeKind, value::Value};
use libkernel::ipc::{capability::Capability, message::MailboxSender, object::duplicate};
use liboal::{FollowLimits, Namespace};
use librpc::Exchange;
use libservice::{
    client::{Client as Services, Watch},
    protocol as service,
};
pub(super) fn limits() -> FollowLimits {
    limits_with_timeout(5_000_000_000)
}
fn limits_with_timeout(timeout_ns: u64) -> FollowLimits {
    FollowLimits::new(16, 4 * erhino_shared::message::PAYLOAD_MAX, timeout_ns)
        .expect("init follow policy invalid")
}
pub(super) fn deadline() -> Result<Deadline, &'static str> {
    libkernel::time::timeout_millis(5_000).map_err(|_| "filesystem deadline invalid")
}
pub(super) fn namespace(sender: &MailboxSender) -> Result<Namespace, &'static str> {
    let handle = duplicate(
        sender.as_handle(),
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .map_err(|_| "entry duplicate failed")?; // SAFETY: duplicate 交付唯一新、未映射 handle，由下面的 typed owner 接管。
    let cap = unsafe { Capability::from_raw(handle) };
    let (sender, _) = MailboxSender::from_capability(cap).map_err(|_| "entry role invalid")?;
    let mut namespace = Namespace::new();
    namespace
        .bind(
            "/",
            Arc::try_new(sender).map_err(|_| "namespace entry storage failed")?,
        )
        .map_err(|_| "namespace binding failed")?;
    Ok(namespace)
}
