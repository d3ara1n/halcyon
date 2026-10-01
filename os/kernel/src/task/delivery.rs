//! 每条消息的 affine 交付责任，Receive 后仍保活被调用授权。

use super::{
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    proc::Process,
    resources::{IpcPermit, MetadataSponsor},
};
use alloc::sync::Arc;
use core::any::Any;
use erhino_shared::{call::SystemCallError, object::Rights};

pub struct Delivery {
    /// capability 持有计数（ZeroPolicy 按类型固定）。
    holders: capability::Holders,
    header: ObjectHeader,
    sender: ObjectRef,
    _permit: IpcPermit,
}

impl Delivery {
    pub fn create(
        sender: ObjectRef,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<ObjectRef, SystemCallError> {
        let object = Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            sender,
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Delivery)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(object)
    }
}

impl KernelObject for Delivery {
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
        self.sender.header().koid()
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<erhino_shared::object::ObjectSignals> {
        None
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
