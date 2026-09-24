//! 非映射对象能力 owner；Handle 数值不复制关闭责任，运输权限由 role/rights 决定。

use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandleDescription, Rights},
};

#[derive(Debug)]
pub struct Capability {
    handle: Option<Handle>,
}

impl Capability {
    /// # Safety
    /// 调用者须独占合法非映射 entry 的关闭责任，不能是依赖 MappingLease 的
    /// Tunnel Endpoint。Mailbox/Notification/WaitSet 等 affine owner 可以由
    /// 正式构造或 GRANT 转入；Close 允许等待已预付的内核退休，但不等待用户观察者。
    /// 本类型不授予 TRANSIT/GRANT，也不令 affine role 变成可复制。
    pub unsafe fn from_raw(handle: Handle) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    pub(crate) fn owned(handle: Handle) -> Self {
        // SAFETY: 私有调用者取得正式构造、Receive 或 duplicate 的合法非映射 entry，
        // 唯一关闭责任尚未交付；权限和 affine 约束仍由原 role 保持。
        unsafe { Self::from_raw(handle) }
    }

    pub fn as_handle(&self) -> Handle {
        self.handle.expect("capability already consumed")
    }

    pub fn description(&self) -> Result<HandleDescription, SystemCallError> {
        super::object::query(self.as_handle())
    }

    pub fn duplicate(&self, rights: Rights) -> Result<Self, SystemCallError> {
        let handle = super::object::duplicate(self.as_handle(), rights)?;
        Ok(Self::owned(handle))
    }

    pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: self 独占合法非映射对象 entry 的关闭责任，失败仍保留原 owner。
        match unsafe { super::object::close(self.as_handle()) } {
            Ok(()) => {
                self.handle = None;
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }

    /// 消费 owner 并显式移交原始关闭责任，不复制表项。
    pub fn into_raw(mut self) -> Handle {
        self.handle.take().expect("capability already consumed")
    }

    pub(crate) fn transferred(&mut self) {
        self.handle = None;
    }
}

impl Drop for Capability {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            super::object::close_object_owner(handle);
        }
    }
}

#[derive(Debug)]
pub struct HandleSet {
    slots: Vec<Option<Capability>>,
}

impl HandleSet {
    pub(crate) fn prepared(count: usize) -> Result<Self, SystemCallError> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(count)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        slots.resize_with(count, || None);
        Ok(Self { slots })
    }

    pub(crate) fn install(&mut self, handles: &[Handle]) {
        assert!(
            handles.len() <= self.slots.len(),
            "received capabilities exceed prepared slots"
        );
        self.slots.truncate(handles.len());
        for (slot, handle) in self.slots.iter_mut().zip(handles) {
            *slot = Some(Capability::owned(*handle));
        }
    }

    pub(crate) fn reset_prepared(&mut self, count: usize) {
        assert!(
            count <= self.slots.capacity(),
            "capability reset exceeds prepared capacity"
        );
        self.slots.clear();
        self.slots.resize_with(count, || None);
    }

    pub(crate) fn transfer_from(&mut self, source: &mut Self) {
        assert!(
            source.len() <= self.slots.len(),
            "capability handoff exceeds prepared slots"
        );
        self.slots.truncate(source.len());
        for (destination, source) in self.slots.iter_mut().zip(&mut source.slots) {
            *destination = source.take();
        }
    }

    /// 线格式槽数量不因某个槽被提取而改变。
    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn get(&self, slot: usize) -> Result<&Capability, SystemCallError> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .ok_or(SystemCallError::IllegalArgument)
    }

    pub fn take(&mut self, slot: usize) -> Result<Capability, SystemCallError> {
        self.slots
            .get_mut(slot)
            .and_then(Option::take)
            .ok_or(SystemCallError::IllegalArgument)
    }

    pub fn remaining(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// 消费所有仍由集合持有的能力 owner。
    ///
    /// 槽位布局属于已验证的消息结构；调用方只在完成协议校验后使用该出口，
    /// 因而不会重新构造或复制任何 Handle。
    pub fn take_all(&mut self) -> Vec<Capability> {
        self.slots.iter_mut().filter_map(Option::take).collect()
    }
}

impl IntoIterator for HandleSet {
    type Item = Capability;
    type IntoIter = core::iter::Flatten<alloc::vec::IntoIter<Option<Capability>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.slots.into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_each_slot_once_and_reports_missing() {
        let mut set = HandleSet::prepared(3).unwrap();
        set.install(&[
            Handle::from_parts(1, 1),
            Handle::from_parts(2, 1),
            Handle::from_parts(3, 1),
        ]);
        assert_eq!(set.len(), 3);
        assert_eq!(set.take(3).unwrap_err(), SystemCallError::IllegalArgument);
        // 取出的 owner 在 host 上无 syscall，显式移出原始责任避免 Drop 关闭。
        let _ = set.take(0).unwrap().into_raw();
        assert_eq!(set.take(0).unwrap_err(), SystemCallError::IllegalArgument);
        assert_eq!(set.remaining(), 2);
        assert!(set.get(2).is_ok());
        assert!(set.get(0).is_err());
        let drained: alloc::vec::Vec<Capability> = set.into_iter().collect();
        assert_eq!(drained.len(), 2);
        for capability in drained {
            let _ = capability.into_raw();
        }
    }
}
