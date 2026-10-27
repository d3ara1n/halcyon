//! Registry 的有限发现与观察记录；身份、准入及变更均在同一 actor 提交。
use super::*;
use crate::watch::{Info, Reason, Sequence};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchTarget {
    Scope(Scope),
    Binding(BindingRef),
}
pub(super) struct Watch {
    source: u64,
    target: WatchTarget,
    task: u64,
    sequence: Sequence,
    _unit: Charge,
}
impl WatchTarget {
    fn scope(self) -> Scope {
        match self {
            Self::Scope(s) => s,
            Self::Binding(r) => Scope::Subtree(r.device()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scan {
    pub end: u64,
    pub next: u64,
    pub device: Option<DeviceId>,
}
impl<C> Registry<C> {
    pub fn check_child_source(&self, issuer: u64, reference: BindingRef) -> Result<(), Error> {
        self.authorize(
            issuer,
            Access::CHILD_REGISTER,
            Scope::Subtree(reference.device()),
            true,
        )?;
        if self.binding_snapshot(reference)?.subject.context() != issuer {
            return Err(Error::Permission);
        }
        self.binding_usable(reference)
    }
    pub fn derive_child_source(
        &mut self,
        issuer: u64,
        reference: BindingRef,
        ids: SourceIds,
        task: u64,
    ) -> Result<(), Error> {
        self.check_child_source(issuer, reference)?;
        let funding = self.funding(issuer)?.clone();
        self.install_source(
            Some((issuer, Some(reference))),
            ids,
            task,
            Scope::Subtree(reference.device()),
            Access::REGISTER.union(Access::OBSERVE),
            funding,
        )
    }
    /// 原来源任务在父来源或父轮封口后沿同一记录完成本地封口。
    pub fn synchronize_source(&mut self, context: u64) -> Result<(), Error> {
        if !self.source(context)?.sealed && self.source_open(context).is_err() {
            let admin = self.source(context)?.ids.admin_context;
            self.seal_source(admin)?;
        }
        Ok(())
    }
    pub fn enumerate(
        &self,
        context: u64,
        scope: Scope,
        after: u64,
        end: u64,
    ) -> Result<Scan, Error> {
        self.authorize(context, Access::OBSERVE, scope, false)?;
        self.source_open(context)?;
        let end = if end == 0 {
            if after != 0 {
                return Err(Error::Invalid);
            }
            self.scan_end()
        } else {
            if end > self.scan_end() || after > end {
                return Err(Error::Invalid);
            }
            end
        };
        let mut cursor = after;
        while let Some((serial, node)) = self.devices.next_after(Some(&cursor)) {
            if *serial > end {
                break;
            }
            cursor = *serial;
            if self.scope_contains(scope, Scope::Subtree(node.value.id))? {
                return Ok(Scan {
                    end,
                    next: cursor,
                    device: Some(node.value.id),
                });
            }
        }
        Ok(Scan {
            end,
            next: end,
            device: None,
        })
    }
    pub fn observe_scope(&self, context: u64, scope: Scope) -> Result<(), Error> {
        self.authorize(context, Access::OBSERVE, scope, true)
    }
    /// Binding target 只能由宿主已核定的 BindingControl/Use 入口调用，不接受 payload 自授。
    pub fn create_watch(
        &mut self,
        source: u64,
        target: WatchTarget,
        context: u64,
        task: u64,
    ) -> Result<Info, Error> {
        if context == 0
            || task == 0
            || self.context_exists(context)
            || self.watches.contains_key(context)
        {
            return Err(Error::Conflict);
        }
        match target {
            WatchTarget::Scope(scope) => self.observe_scope(source, scope)?,
            WatchTarget::Binding(reference) => {
                if self.binding_snapshot(reference)?.subject.context() != source {
                    return Err(Error::Permission);
                }
            }
        }
        self.source(source)?
            .retained
            .checked_add(1)
            .ok_or(Error::Quota)?;
        if let Scope::Subtree(id) = target.scope() {
            self.device(id)?
                .retained
                .checked_add(1)
                .ok_or(Error::Quota)?;
        }
        let account = self.funding(source)?.domain().clone();
        let record = Watch {
            source,
            target,
            task,
            sequence: Sequence::new(),
            _unit: account.acquire(DeviceResource::Watch, 1)?,
        };
        let entry = prepare(&self.watches, context, record, &account)?;
        self.watches.insert_prepared(entry);
        self.retain_source(source)?;
        if let Scope::Subtree(id) = target.scope() {
            self.retain_device(id)?;
        }
        self.discovery_dirty = true;
        self.query_watch(context)
    }
    fn watch_reason(&self, watch: &Watch) -> Option<Reason> {
        match watch.target {
            // 已有绑定观察在来源封口后继续观察停止，直到明确 Released。
            WatchTarget::Binding(reference) => self
                .binding_snapshot(reference)
                .err()
                .map(|_| Reason::Released),
            WatchTarget::Scope(_) => self
                .source_open(watch.source)
                .err()
                .map(|_| Reason::Revoked),
        }
    }
    pub fn query_watch(&mut self, context: u64) -> Result<Info, Error> {
        let watch = &self.watches.get(context).ok_or(Error::Permission)?.value;
        let reason = self.watch_reason(watch);
        let watch = &mut self
            .watches
            .get_mut(context)
            .expect("watch disappeared in actor")
            .value;
        if let Some(reason) = reason {
            watch.sequence.terminate(reason);
        }
        Ok(watch.sequence.info())
    }
    pub fn ack_watch(&mut self, context: u64, sequence: u64) -> Result<Info, Error> {
        self.query_watch(context)?;
        self.watches
            .get_mut(context)
            .ok_or(Error::Permission)?
            .value
            .sequence
            .ack(sequence)
    }
    pub fn cancel_watch(&mut self, context: u64) -> Result<Info, Error> {
        let watch = &mut self
            .watches
            .get_mut(context)
            .ok_or(Error::Permission)?
            .value;
        watch.sequence.terminate(Reason::Cancelled);
        self.discovery_dirty = true;
        Ok(watch.sequence.info())
    }
    pub fn retire_watch(&mut self, context: u64) -> Result<(), Error> {
        let watch = self
            .watches
            .remove(context)
            .ok_or(Error::Permission)?
            .into_value()
            .value;
        self.release_source(watch.source)?;
        if let Scope::Subtree(id) = watch.target.scope() {
            self.release_device(id)?;
        }
        self.discovery_dirty = true;
        Ok(())
    }
    pub fn watch_tasks(&self, after: u64) -> Option<(u64, u64)> {
        self.watches
            .next_after(Some(&after))
            .map(|(id, record)| (*id, record.value.task))
    }
    pub fn take_discovery_change(&mut self) -> bool {
        core::mem::take(&mut self.discovery_dirty)
    }
    /// 所有领域状态改变都从其原提交点调用；后台只负责通知投影，不能补造序号。
    pub(super) fn changed(&mut self, id: DeviceId) {
        if !self.device(id).is_ok_and(|d| {
            d.seat
                .as_ref()
                .and_then(Seat::active)
                .is_some_and(|b| self.binding_usable(b.reference).is_ok())
        }) {
            self.remove_functions(id);
        }
        self.discovery_dirty = true;
        let mut after = 0;
        while let Some((context, record)) = self.watches.next_after(Some(&after)) {
            let context = *context;
            let watch = &record.value;
            let reason = self.watch_reason(watch);
            let matches = match watch.target {
                WatchTarget::Binding(reference) => reference.device() == id,
                WatchTarget::Scope(scope) => {
                    self.scope_contains(scope, Scope::Subtree(id))
                        .unwrap_or(false)
                        || self
                            .scope_contains(Scope::Subtree(id), scope)
                            .unwrap_or(false)
                }
            };
            let sequence = &mut self
                .watches
                .get_mut(context)
                .expect("selected watch disappeared")
                .value
                .sequence;
            if let Some(reason) = reason {
                sequence.terminate(reason);
            } else if matches {
                sequence.change();
            }
            after = context;
        }
    }
    pub(super) fn sources_changed(&mut self) {
        self.discovery_dirty = true;
        let mut after = 0;
        while let Some((context, _)) = self.watches.next_after(Some(&after)) {
            let context = *context;
            self.query_watch(context)
                .expect("selected watch disappeared");
            after = context;
        }
        let mut after = 0;
        while let Some((serial, device)) = self.devices.next_after(Some(&after)) {
            let serial = *serial;
            let id = device.value.id;
            let claimant_closed = device
                .value
                .seat
                .as_ref()
                .and_then(Seat::active)
                .is_some_and(|binding| self.source_open(binding.subject.context()).is_err());
            if self.device_open(id).is_err() || claimant_closed {
                self.changed(id);
            }
            after = serial;
        }
    }
}
