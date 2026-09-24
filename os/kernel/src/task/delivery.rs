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
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            sender,
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Delivery)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(object)
    }
}

impl KernelObject for Delivery {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::Delivery
    }
    fn related_id(&self) -> u64 {
        self.sender.header().koid()
    }
    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::Delivery).then_some(Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self, _: HandleRole) -> Option<erhino_shared::object::ObjectSignals> {
        None
    }
    fn close_handle(&self, _: HandleRole, _: &Process, _: bool) {}
    fn close_transit(&self, _: HandleRole) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}
