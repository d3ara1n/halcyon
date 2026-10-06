//! 已接受消息的仿射 use；存储引用不延长发送授权，观察与调用上下文只保留数值身份。

use super::{
    lifetime::LifetimeUse,
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef},
};
use alloc::sync::Arc;
use core::any::Any;
use erhino_shared::{call::SystemCallError, object::Rights};

pub struct Delivery {
    holders: capability::Holders,
    header: ObjectHeader,
    sender_id: u64,
    usage: crate::sync::Spinlock<Option<LifetimeUse>>,
    _metadata: super::resources::MetadataCharge,
}

impl Delivery {
    pub fn create(
        sender_id: u64,
        usage: LifetimeUse,
        pool: Arc<MemoryPool>,
    ) -> Result<ObjectRef, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let delivery = Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            sender_id,
            usage: crate::sync::Spinlock::new(crate::sync::ranks::LEAF, Some(usage)),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(delivery)
    }
}

impl KernelObject for Delivery {
    fn allowed_signals(&self) -> Option<erhino_shared::object::ObjectSignals> {
        None
    }
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::Delivery
    }
    fn related_id(&self) -> u64 {
        self.sender_id
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::TRANSIT | Rights::GRANT)
    }
    fn on_last_capability(&self) {
        let usage = self.usage.lock().take();
        drop(usage);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
