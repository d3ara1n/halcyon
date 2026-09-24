//! 持久观察 owner；普通 Close 由内核完成有界退休，不在用户态遍历。

use crate::{call, time::Deadline};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    wait::{WaitItem, WaitResult},
    wait_set::{RECEIVE_MAX, ReadyRecord},
};

static ABANDONED: AtomicUsize = AtomicUsize::new(0);

pub struct WaitSet {
    handle: Option<Handle>,
}

impl WaitSet {
    pub fn create(limit: usize) -> Result<Self, SystemCallError> {
        Self::create_with_rights(limit, Rights::READ | Rights::WAIT | Rights::MANAGE)
    }

    pub fn create_with_rights(limit: usize, rights: Rights) -> Result<Self, SystemCallError> {
        let mut handle = Handle::INVALID;
        // SAFETY: output 在调用期间有效，结果建立唯一 owner。
        unsafe { call::sys_wait_set_create(limit, rights, &mut handle) }?;
        Ok(Self {
            handle: Some(handle),
        })
    }

    pub fn handle(&self) -> Handle {
        self.handle.expect("WaitSet owner already closed")
    }

    pub fn into_capability(mut self) -> super::capability::Capability {
        super::capability::Capability::owned(
            self.handle.take().expect("WaitSet owner already consumed"),
        )
    }

    pub fn register(&self, item: WaitItem) -> Result<u64, SystemCallError> {
        let mut token = 0;
        // SAFETY: 输入和输出在调用复制期间有效，注册只取得观察授权。
        unsafe { call::sys_wait_set_register(self.handle(), &item, &mut token) }?;
        Ok(token)
    }

    pub fn rearm(&self, token: u64) -> Result<u64, SystemCallError> {
        // SAFETY: owner 存活，token 是值，错误身份由内核拒绝。
        unsafe { call::sys_wait_set_rearm(self.handle(), token) }
    }

    pub fn remove(&self, token: u64) -> Result<(), SystemCallError> {
        // SAFETY: owner 存活，仅撤销本集合注册。
        unsafe { call::sys_wait_set_remove(self.handle(), token) }
    }

    pub fn receive(&self, capacity: usize) -> Result<Vec<ReadyRecord>, SystemCallError> {
        if capacity == 0 || capacity > RECEIVE_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut records = Vec::new();
        records
            .try_reserve_exact(capacity)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        records.resize(
            capacity,
            ReadyRecord {
                token: 0,
                arm_generation: 0,
                cookie: 0,
                observed: ObjectSignals::NONE,
                reason: 0,
                error: 0,
            },
        );
        let count = self.receive_into(&mut records)?;
        records.truncate(count);
        Ok(records)
    }

    pub fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
        if records.is_empty() || records.len() > RECEIVE_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut count = 0;
        // SAFETY: 已初始化切片在调用期间有效，失败不消费 ready 记录。
        unsafe { call::sys_wait_set_receive(self.handle(), records, &mut count) }?;
        if count as usize > records.len() {
            return Err(SystemCallError::InternalError);
        }
        Ok(count as usize)
    }

    pub fn wait(&self, deadline: Deadline) -> Result<WaitResult, SystemCallError> {
        super::wait::wait_until(
            &[WaitItem::new(
                self.handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                0,
            )],
            deadline,
        )
    }

    pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: self 独占关闭权；提交前错误保留 owner，成功时自身责任已退休。
        match unsafe { super::object::close(self.handle()) } {
            Ok(()) => {
                self.handle = None;
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
}

impl Drop for WaitSet {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: Drop 已排除安全借用；内核负责已提交退休的必成尾段。
            if unsafe { super::object::close(handle) }.is_err() {
                let _ = ABANDONED.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(1))
                });
            }
        }
    }
}

pub fn abandoned_count() -> usize {
    ABANDONED.load(Ordering::Acquire)
}
