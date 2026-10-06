//! 内核侧 capability 拼装：`capability` crate 的 Authority 由内核对象引用实现。
//!
//! 设计见 plans/todo-2026-09-28-capability-last-reference.md（内核拼装草图）：
//! 公共层只报告最后持有事实；`LastCapability` 在表锁外交对象
//! [`KernelObject::on_last_capability`] 执行。

use super::object::{KernelObject, ObjectRef};

/// 内核 authority：对象存储引用（Clone 不产生 authority）。
#[derive(Clone)]
pub(crate) struct ObjectAuthority(pub(crate) ObjectRef);

impl capability::Authority for ObjectAuthority {
    fn holders(&self) -> &capability::Holders {
        self.0.holders()
    }
}

pub(crate) type Cap = capability::Cap<ObjectAuthority>;
pub(crate) type CapTable = capability::CapTable<ObjectAuthority, super::resources::MetadataCharge>;

/// 表锁外投递最后持有事件。
pub(crate) fn deliver(last: Option<capability::LastCapability<ObjectAuthority>>) {
    if let Some(capability::LastCapability(ObjectAuthority(object))) = last {
        KernelObject::on_last_capability(&*object);
    }
}
