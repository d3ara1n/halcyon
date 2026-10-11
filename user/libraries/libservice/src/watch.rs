//! 服务名字/目录订阅；sequence/pending 在名字提交同一步改变，通知只作重查提示。
use crate::{
    authority::DirectoryAccess,
    protocol::{AccessRights, INITIAL, Status, TERMINAL, WatchInfo, WatchReason, WatchState},
    resource::ServiceResource,
};
use alloc::{string::String, sync::Arc};
use core::mem::ManuallyDrop;
use erhino_shared::{call::SystemCallError, object::Handle};
use libbudget::Charge;
use libkernel::ipc::notification::{NotificationOwner, NotificationSignaler};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};
#[derive(Clone, Copy)]
pub struct Sequence {
    info: WatchInfo,
}
impl Sequence {
    pub fn new() -> Self {
        Self {
            info: WatchInfo {
                sequence: 1,
                pending: INITIAL,
                state: WatchState::Active,
                reason: WatchReason::Active,
            },
        }
    }
    pub fn info(&self) -> WatchInfo {
        self.info
    }
    pub fn change(&mut self, bits: u64) {
        if self.info.state == WatchState::Terminal {
            return;
        }
        if self.info.sequence == u64::MAX - 1 {
            self.terminate(WatchReason::SequenceExhausted);
            return;
        }
        self.info.sequence += 1;
        self.info.pending |= bits;
    }
    pub fn terminate(&mut self, reason: WatchReason) {
        if self.info.state == WatchState::Terminal {
            return;
        }
        self.info.sequence = self
            .info
            .sequence
            .checked_add(1)
            .expect("active watch exhausted terminal sequence");
        self.info.pending |= TERMINAL;
        self.info.state = WatchState::Terminal;
        self.info.reason = reason;
    }
    pub fn ack(&mut self, sequence: u64) -> Result<WatchInfo, Status> {
        if sequence > self.info.sequence {
            return Err(Status::Invalid);
        }
        if sequence == self.info.sequence {
            self.info.pending = 0;
        }
        Ok(self.info)
    }
}
impl Default for Sequence {
    fn default() -> Self {
        Self::new()
    }
}
struct Watch {
    access: Option<DirectoryAccess>,
    funding: crate::Funding,
    name: Option<String>,
    sequence: Sequence,
    signaled: u64,
    notification_closed: bool,
    signaler: NotificationSignaler,
    notification: NotificationOwner,
    _slot: Permit,
    _unit: Charge,
    _bytes: Charge,
    _source: Charge,
}
pub struct PreparedWatch {
    entry: PreparedEntry<Watch, ()>,
}
impl PreparedWatch {
    pub fn context(&self) -> u64 {
        self.entry.key()
    }
    pub fn info(&self) -> WatchInfo {
        self.entry.value().sequence.info()
    }
    pub fn notification_handle(&self) -> Handle {
        self.entry.value().notification.as_handle()
    }
}
pub struct WatchTable {
    records: ManuallyDrop<OrderedTable<Watch, ()>>,
    slots: Arc<Counter>,
    sealed: bool,
}
fn system(e: SystemCallError) -> Status {
    match e {
        SystemCallError::QuotaExceeded => Status::Quota,
        _ => Status::Resource,
    }
}
impl WatchTable {
    pub fn new(limit: usize) -> Result<Self, Status> {
        if limit == 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            records: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?,
            sealed: false,
        })
    }
    /// signer/控制Lifetime/source归当前宿主任务；这里预留真实索引和订阅额度。
    pub fn prepare(
        &self,
        access: &DirectoryAccess,
        context: u64,
        name: Option<&str>,
        signaler: NotificationSignaler,
        notification: NotificationOwner,
    ) -> Result<PreparedWatch, Status> {
        if self.sealed {
            return Err(Status::Revoked);
        }
        access.check(name, AccessRights::WATCH)?;
        if let Some(name) = name
            && !crate::protocol::valid_name(name)
        {
            return Err(Status::Invalid);
        }
        if context == 0 || self.records.get(context).is_some() {
            return Err(Status::Invalid);
        }
        let mut owned = None;
        if let Some(name) = name {
            let mut s = String::new();
            s.try_reserve_exact(name.len())
                .map_err(|_| Status::Resource)?;
            s.push_str(name);
            owned = Some(s);
        }
        let account = access.account();
        let slot = Counter::try_acquire(&self.slots).map_err(|_| Status::Quota)?;
        let unit = account.acquire(ServiceResource::Watch, 1).map_err(system)?;
        let source = account
            .acquire(ServiceResource::WaitSource, 1)
            .map_err(system)?;
        let size = PreparedEntry::<Watch, ()>::allocation_bytes()
            .checked_add(owned.as_ref().map_or(0, String::capacity))
            .ok_or(Status::Resource)?;
        let bytes = account
            .acquire(ServiceResource::Bytes, size)
            .map_err(system)?;
        let entry = self
            .records
            .prepare_insert(
                context,
                Watch {
                    access: Some(access.clone()),
                    funding: access.funding().clone(),
                    name: owned,
                    sequence: Sequence::new(),
                    signaled: 0,
                    notification_closed: false,
                    signaler,
                    notification,
                    _slot: slot,
                    _unit: unit,
                    _bytes: bytes,
                    _source: source,
                },
                &libbudget::AllocatorBacked,
            )
            .map_err(|e| match e {
                ordered_table::InsertError::Limit(_) => Status::Quota,
                ordered_table::InsertError::Storage(e, _) => system(libbudget::allocator_error(e)),
            })?;
        Ok(PreparedWatch { entry })
    }
    pub fn install(&mut self, prepared: PreparedWatch) -> Result<WatchInfo, PreparedWatch> {
        if self.sealed
            || !prepared
                .entry
                .value()
                .access
                .as_ref()
                .is_some_and(DirectoryAccess::active)
        {
            return Err(prepared);
        }
        let info = prepared.entry.value().sequence.info();
        self.records.insert_prepared(prepared.entry);
        Ok(info)
    }
    pub fn publish(&mut self, name: &str, bits: u64) {
        let mut after = None;
        while let Some((&context, _)) = self.records.next_after(after.as_ref()) {
            after = Some(context);
            let watch = self
                .records
                .get_mut(context)
                .expect("selected watch missing");
            let Some(access) = watch.access.as_ref() else {
                continue;
            };
            if !access.active() {
                watch.sequence.terminate(WatchReason::Revoked);
            } else if access.scope().permits(name)
                && watch.name.as_ref().is_none_or(|expected| expected == name)
            {
                watch.sequence.change(bits);
            }
        }
    }
    pub fn query(&mut self, context: u64) -> Result<WatchInfo, Status> {
        let watch = self.records.get_mut(context).ok_or(Status::Permission)?;
        if watch.access.as_ref().is_some_and(|access| !access.active()) {
            watch.sequence.terminate(WatchReason::Revoked);
        }
        Ok(watch.sequence.info())
    }
    /// 控制按出生时的可信付款账户收束，与观察目标是否仍在无关。
    pub fn control_funding(&self, context: u64) -> Option<&crate::Funding> {
        self.records.get(context).map(|watch| &watch.funding)
    }
    pub fn ack(&mut self, context: u64, sequence: u64) -> Result<WatchInfo, Status> {
        self.query(context)?;
        let watch = self.records.get_mut(context).ok_or(Status::Permission)?;
        let info = watch.sequence.ack(sequence)?;
        if info.pending == 0 {
            match watch.notification.take(u64::MAX) {
                Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                Err(error) => return Err(system(error)),
            }
        }
        Ok(info)
    }
    pub fn cancel(&mut self, context: u64) -> Result<WatchInfo, Status> {
        let watch = self.records.get_mut(context).ok_or(Status::Permission)?;
        watch.sequence.terminate(WatchReason::Cancelled);
        Ok(watch.sequence.info())
    }
    pub fn signal(&mut self, context: u64) -> Result<(), SystemCallError> {
        let Some(watch) = self.records.get_mut(context) else {
            return Ok(());
        };
        if watch.notification_closed {
            return Ok(());
        }
        if watch.access.as_ref().is_some_and(|access| !access.active()) {
            watch.sequence.terminate(WatchReason::Revoked);
        }
        if watch.sequence.info().pending != 0 && watch.signaled != watch.sequence.info().sequence {
            watch.signaler.signal(1)?;
            watch.signaled = watch.sequence.info().sequence;
        }
        Ok(())
    }
    /// 通知的永久关闭取消观察目标；终态控制及其 owner 仍按自身 Lifetime 收束。
    pub fn notification_closed(&mut self, context: u64) {
        if let Some(watch) = self.records.get_mut(context) {
            watch.notification_closed = true;
            watch.sequence.terminate(WatchReason::Cancelled);
            watch.access = None;
        }
    }
    pub fn next_context(&self, after: Option<u64>) -> Option<u64> {
        self.records
            .next_after(after.as_ref())
            .map(|(&context, _)| context)
    }
    pub fn handle(&self, context: u64) -> Option<Handle> {
        self.records.get(context).map(|w| w.signaler.as_handle())
    }
    pub fn remove(&mut self, context: u64) -> bool {
        self.records.remove(context).is_some()
    }
    pub fn seal(&mut self) {
        self.sealed = true;
        let mut after = None;
        while let Some((&context, _)) = self.records.next_after(after.as_ref()) {
            after = Some(context);
            self.records
                .get_mut(context)
                .expect("selected watch missing")
                .sequence
                .terminate(WatchReason::ProviderStopping);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}
impl Drop for WatchTable {
    fn drop(&mut self) {
        if self.records.is_empty() {
            // SAFETY: 表未曾提前析构，空表没有通知/订阅 owner。
            unsafe {
                ManuallyDrop::drop(&mut self.records);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_ack_preserves_replacement_and_future_ack_is_rejected() {
        // WatchName 的同一订阅累计替换事件；读取旧 snapshot 后 ack 不清后来的变化。
        let mut sequence = Sequence::new();
        let observed = sequence.info().sequence;
        sequence.change(crate::protocol::REPLACED);
        let changed = sequence.info();
        assert_eq!(sequence.ack(observed).unwrap(), changed);
        assert_eq!(sequence.ack(changed.sequence + 1), Err(Status::Invalid));
        let acknowledged = sequence.ack(changed.sequence).unwrap();
        assert_eq!(acknowledged.pending, 0);
        assert_eq!(sequence.ack(changed.sequence).unwrap(), acknowledged);
        assert_eq!(sequence.ack(observed).unwrap(), acknowledged);
    }
    #[test]
    fn exhausted_sequence_retains_one_terminal_event_without_wrap() {
        // u64 最后一个值只用作终态；反复变更/取消不能回绕或覆盖原原因。
        let mut sequence = Sequence::new();
        sequence.info.sequence = u64::MAX - 1;
        sequence.change(crate::protocol::REGISTERED);
        let terminal = sequence.info();
        assert_eq!(terminal.sequence, u64::MAX);
        assert_eq!(terminal.reason, WatchReason::SequenceExhausted);
        terminal.validate().unwrap();
        sequence.change(crate::protocol::WITHDRAWN);
        sequence.terminate(WatchReason::Cancelled);
        assert_eq!(sequence.info(), terminal);
        sequence.ack(u64::MAX).unwrap();
        sequence.info().validate().unwrap();
    }
}
