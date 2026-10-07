//! 稳定进程壳：Control 身份、终因与私有 Body 准入属于同一个 owner。

use alloc::sync::{Arc, Weak};
use core::{
    any::Any,
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, ObjectSignals, Rights},
    proc::{Pid, ProcessSnapshot, ProcessState, TerminationCause, TerminationDetail},
};

use crate::{
    maintenance::{self as mt, WakeToken, WorkRecord},
    sync::{Spinlock, ranks},
    vm::change::VmShell,
};

use super::{
    super::{
        job::Job,
        memory_pool::MemoryPool,
        object::{KernelObject, ObjectHeader, ObjectRef},
        wait_source::{ObservationSource, SignalSource},
    },
    body::ProcessBody,
    reclaim::ReclaimState,
};

struct ShellState {
    state: ProcessState,
    cause: TerminationCause,
    detail: TerminationDetail,
    body: Option<super::body::BodyStorage>,
    construction_pending: bool,
    receipt_leases: usize,
    member_count: usize,
    reserved_members: usize,
    running: usize,
    retiring: bool,
    ingress_waiter: Option<WakeToken>,
    quiescence_waiter: Option<WakeToken>,
}

const FROZEN: u64 = 1 << 63;
const INGRESS_COUNT: u64 = FROZEN - 1;
pub(crate) const CONTROL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

/// Job 与已提交 Work 持有的稳定壳；观察 capability 的消散不终止进程。
pub(crate) struct ProcessShell {
    pub(crate) holders: capability::Holders,
    pub(crate) header: ObjectHeader,
    pub(crate) pid: Pid,
    pub(crate) parent_pid: Pid,
    pub(crate) job: Weak<Job>,
    pub(crate) record: WorkRecord,
    ingress: AtomicU64,
    state: Spinlock<ShellState>,
    pub(crate) reclaim: Spinlock<ReclaimState>,
    pub(crate) source: Arc<SignalSource>,
    _metadata: super::super::resources::MetadataCharge,
}

pub(crate) struct MemberReservation<'a> {
    shell: &'a ProcessShell,
    body: &'a BodyLease,
    key: super::members::MemberKey,
    building: bool,
    committed: bool,
}

impl MemberReservation<'_> {
    pub(crate) fn key(&self) -> super::members::MemberKey {
        self.key
    }

    pub(crate) fn building(&self) -> bool {
        self.building
    }

    pub(crate) fn commit(mut self, member: super::members::MemberState) {
        self.shell.commit_member(self.body, self.key, member);
        self.committed = true;
    }
}

impl Drop for MemberReservation<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.body.shell.rollback_member(self.body, self.key);
        }
    }
}

enum Freeze {
    Already,
    Began(Option<(Arc<VmShell>, Arc<crate::vm::retire::RetireWork>)>),
}

impl ProcessShell {
    pub(crate) fn pool(&self) -> &Arc<MemoryPool> {
        self._metadata.payer()
    }
    pub(crate) fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        if object.kind() != ObjectKind::ProcessControl {
            return Err(SystemCallError::WrongObjectType);
        }
        let any: Arc<dyn Any + Send + Sync> = object.clone();
        any.downcast::<Self>()
            .map_err(|_| SystemCallError::WrongObjectType)
    }

    pub(crate) fn funding_pool(&self) -> Arc<MemoryPool> {
        self._metadata.payer().clone()
    }

    pub(crate) fn member_count(&self) -> usize {
        self.state.lock().member_count
    }

    /// 构造 lease 覆盖 Job 提交至 Body 安装的窗口；此前失败直接局部回滚。
    pub(crate) fn try_new(
        pid: Pid,
        parent_pid: Pid,
        job: Weak<Job>,
        pool: &Arc<MemoryPool>,
    ) -> Result<(Arc<Self>, ConstructionLease), SystemCallError> {
        let metadata = super::super::resources::MetadataCharge::arc::<Self>(pool)?;
        let shell = Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Remintable),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            pid,
            parent_pid,
            job,
            record: WorkRecord::new(pool.maintenance_account().clone()),
            ingress: AtomicU64::new(0),
            state: Spinlock::new(
                ranks::LIFECYCLE,
                ShellState {
                    state: ProcessState::Building,
                    cause: TerminationCause::None,
                    detail: TerminationDetail::NONE,
                    body: None,
                    construction_pending: true,
                    receipt_leases: 0,
                    member_count: 0,
                    reserved_members: 0,
                    running: 0,
                    retiring: false,
                    ingress_waiter: None,
                    quiescence_waiter: None,
                },
            ),
            reclaim: Spinlock::new(ranks::DRAIN_CURSOR, ReclaimState::new()),
            source: SignalSource::try_new(pool.clone())?,
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        let construction = ConstructionLease {
            shell: shell.clone(),
        };
        Ok((shell, construction))
    }

    /// 未冻结时借用稳定 Box；借用不能跨等待或返回用户态。
    pub(crate) fn lease_body(self: &Arc<Self>) -> Option<BodyLease> {
        self.borrow_body(false)
    }

    pub(crate) fn lease_building(self: &Arc<Self>) -> Option<BodyLease> {
        self.borrow_body(true)
    }

    fn borrow_body(self: &Arc<Self>, building: bool) -> Option<BodyLease> {
        let state = self.state.lock();
        if state.state == ProcessState::Terminating
            || state.state == ProcessState::Dead
            || (building && state.state != ProcessState::Building)
        {
            return None;
        }
        let body = NonNull::from(state.body.as_deref()?);
        assert!(
            self.register_ingress(),
            "Frozen won Shell gate before external lease"
        );
        Some(BodyLease {
            shell: self.clone(),
            body,
            kind: Some(LeaseKind::External),
            _local: PhantomData,
        })
    }

    /// Job 启动闸门内提交 Running；执行入口随后由 Builder 记录等 VM 静止后开放。
    pub(crate) fn begin_running(
        self: &Arc<Self>,
        expected_members: usize,
    ) -> Result<BodyLease, SystemCallError> {
        let (body, vm) = {
            let mut state = self.state.lock();
            if state.state == ProcessState::Terminating || state.state == ProcessState::Dead {
                return Err(SystemCallError::ObjectClosed);
            }
            if state.state != ProcessState::Building
                || state.member_count != expected_members
                || state.reserved_members != 0
                || self.ingress.load(Ordering::Acquire) & INGRESS_COUNT != 0
            {
                return Err(SystemCallError::ObjectBusy);
            }
            if expected_members == 0 {
                return Err(SystemCallError::ObjectNotAvailable);
            }
            if state.construction_pending {
                return Err(SystemCallError::ObjectBusy);
            }
            let body = state
                .body
                .as_deref()
                .ok_or(SystemCallError::ObjectNotAvailable)?;
            let ptr = NonNull::from(body);
            let vm = body.vm.clone();
            assert!(self.register_ingress(), "Frozen won start gate");
            state.state = ProcessState::Running;
            (ptr, vm)
        };
        let lease = BodyLease {
            shell: self.clone(),
            body,
            kind: Some(LeaseKind::External),
            _local: PhantomData,
        };
        // Job 链锁仍在外层；此处只有 ADDRESS_SPACE 锁，不再持 Shell 锁。
        vm.begin_start();
        Ok(lease)
    }

    /// ThreadStart 与单个成员的 Created→Ready 共同服从 Running/Frozen 判定。
    pub(crate) fn start_created(
        &self,
        body: &BodyLease,
        key: super::members::MemberKey,
    ) -> Result<Arc<super::super::Thread>, SystemCallError> {
        assert!(
            core::ptr::eq(&*body.shell, self),
            "thread belongs to another Shell"
        );
        let state = self.state.lock();
        if state.state != ProcessState::Running {
            return Err(SystemCallError::ObjectClosed);
        }
        body.members
            .start_created(key)
            .ok_or(SystemCallError::ObjectBusy)
    }

    /// 成员状态锁内登记 Ready 发布；调用前须已有必成的队列 admission。
    pub(crate) fn try_publish_ready(self: &Arc<Self>) -> Option<ReadyIngress> {
        self.register_ingress().then(|| ReadyIngress {
            shell: self.clone(),
            published: false,
        })
    }

    pub(crate) fn is_frozen(&self) -> bool {
        self.ingress.load(Ordering::Acquire) & FROZEN != 0
    }

    fn register_ingress(&self) -> bool {
        let mut current = self.ingress.load(Ordering::Acquire);
        loop {
            if current & FROZEN != 0 {
                return false;
            }
            assert!(current < INGRESS_COUNT, "ingress count exhausted");
            match self.ingress.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Acquire,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn finish_ingress(&self) {
        let previous = self.ingress.fetch_sub(1, Ordering::Release);
        assert!(previous & INGRESS_COUNT != 0, "ingress count underflow");
        if previous != (FROZEN | 1) {
            return;
        }
        let (ingress, settle) = {
            let mut state = self.state.lock();
            (
                self.ready_ingress_wake(&mut state),
                self.ready_wake(&mut state),
            )
        };
        Self::wake(ingress, settle);
    }

    /// Work arm 后在来源 gate 内复检；返回 Some 才能开始成员/队列扫描。
    pub(crate) fn wait_ingress(&self, token: WakeToken) -> Option<WakeToken> {
        let mut state = self.state.lock();
        assert!(
            state.state == ProcessState::Terminating,
            "ingress wait before Frozen"
        );
        if self.ingress_done(&state) {
            return Some(token);
        }
        assert!(state.ingress_waiter.is_none(), "duplicate ingress waiter");
        state.ingress_waiter = Some(token);
        None
    }

    fn ingress_done(&self, state: &ShellState) -> bool {
        !state.construction_pending && self.ingress.load(Ordering::Acquire) & INGRESS_COUNT == 0
    }

    fn ready_ingress_wake(&self, state: &mut ShellState) -> Option<WakeToken> {
        if state.state == ProcessState::Terminating && self.ingress_done(state) {
            state.ingress_waiter.take()
        } else {
            None
        }
    }

    fn wake(ingress: Option<WakeToken>, settle: Option<WakeToken>) {
        let ready = ingress.is_some() || settle.is_some();
        if let Some(token) = ingress {
            drop(mt::wake(token));
        }
        if let Some(token) = settle {
            drop(mt::wake(token));
        }
        if ready {
            mt::kick();
        }
    }

    /// 已登记离场回执及回收 Work 的短访问；Frozen 不撤销既有完成责任。
    pub(crate) fn lease_receipt(self: &Arc<Self>) -> Option<BodyLease> {
        let mut state = self.state.lock();
        if state.retiring || state.member_count == 0 {
            return None;
        }
        let body = NonNull::from(state.body.as_deref()?);
        state.receipt_leases = state
            .receipt_leases
            .checked_add(1)
            .expect("receipt lease count exhausted");
        Some(BodyLease {
            shell: self.clone(),
            body,
            kind: Some(LeaseKind::Receipt),
            _local: PhantomData,
        })
    }

    pub(super) fn lease_reclaim(self: &Arc<Self>) -> Option<BodyLease> {
        let mut state = self.state.lock();
        assert!(
            state.state == ProcessState::Terminating,
            "reclaim before Frozen"
        );
        if state.retiring {
            return None;
        }
        let body = NonNull::from(state.body.as_deref()?);
        state.receipt_leases = state
            .receipt_leases
            .checked_add(1)
            .expect("reclaim lease count exhausted");
        Some(BodyLease {
            shell: self.clone(),
            body,
            kind: Some(LeaseKind::Receipt),
            _local: PhantomData,
        })
    }

    /// 预留槽与 Shell 中的在途出生事实同 gate 提交；调用方持 external lease。
    pub(crate) fn reserve_member<'a>(
        &'a self,
        body: &'a BodyLease,
        prepared: &mut super::members::MemberStorage,
    ) -> Result<MemberReservation<'a>, SystemCallError> {
        assert!(
            core::ptr::eq(&*body.shell, self),
            "member belongs to another Shell"
        );
        let mut state = self.state.lock();
        if state.state == ProcessState::Terminating || state.state == ProcessState::Dead {
            return Err(SystemCallError::ObjectClosed);
        }
        let building = state.state == ProcessState::Building;
        let key = body
            .members
            .reserve(prepared)
            .map_err(|error| match error {
                super::members::AttachFault::Limit => SystemCallError::ReachLimit,
                super::members::AttachFault::CapacityChanged => SystemCallError::ObjectBusy,
            })?;
        state.reserved_members = state
            .reserved_members
            .checked_add(1)
            .expect("member reservations exhausted");
        Ok(MemberReservation {
            shell: self,
            body,
            key,
            building,
            committed: false,
        })
    }

    /// 已获出生预约：槽与 Shell 回执事实同段提交，Frozen 由扫描收束该成员。
    pub(crate) fn commit_member(
        &self,
        body: &BodyLease,
        key: super::members::MemberKey,
        member: super::members::MemberState,
    ) {
        assert!(
            core::ptr::eq(&*body.shell, self),
            "member belongs to another Shell"
        );
        let mut state = self.state.lock();
        // 预约已在 Job/Shell 门下准入，Frozen 不取消已提交创建责任。
        // external Body lease 仍存续，回收尚不能扫描或取走成员表。
        assert!(
            !state.retiring && state.state != ProcessState::Dead,
            "member committed after retirement"
        );
        assert!(
            state.reserved_members > 0,
            "member committed without reservation"
        );
        state.member_count = state
            .member_count
            .checked_add(1)
            .expect("member receipt count exhausted");
        body.members.commit(key, member);
        state.reserved_members -= 1;
    }

    /// 构造失败撤销已获准的预留；成功者仍可兑现预约，由 Frozen 后扫描收束。
    pub(crate) fn rollback_member(
        self: &Arc<Self>,
        body: &BodyLease,
        key: super::members::MemberKey,
    ) {
        assert!(
            Arc::ptr_eq(&body.shell, self),
            "member belongs to another Shell"
        );
        let mut state = self.state.lock();
        assert!(state.reserved_members > 0, "member reservation underflow");
        body.members.rollback(key);
        state.reserved_members -= 1;
        let freeze = self.complete_if_empty(&mut state);
        drop(state);
        self.finish_freeze(freeze);
    }

    /// 回执已摘 Body 成员且归还短访问后，最后一个回执在 Shell 锁外唤醒回收。
    pub(crate) fn member_departed(self: &Arc<Self>) {
        let (wake, freeze) = {
            let mut state = self.state.lock();
            assert!(state.member_count > 0, "member receipt underflow");
            state.member_count -= 1;
            let freeze = self.complete_if_empty(&mut state);
            (self.ready_wake(&mut state), freeze)
        };
        if let Some(wake) = wake {
            drop(mt::wake(wake));
            mt::kick();
        }
        self.finish_freeze(freeze);
    }

    fn complete_if_empty(&self, state: &mut ShellState) -> Freeze {
        if state.state == ProcessState::Running
            && state.member_count == 0
            && state.reserved_members == 0
        {
            self.freeze_locked(state, TerminationCause::Completed, TerminationDetail::NONE)
        } else {
            Freeze::Already
        }
    }

    /// 运行准入不借用 Body；跨用户态返回仅持稳定 Shell 与 VM。
    pub(crate) fn admit_running(self: &Arc<Self>) -> Option<RunPermit> {
        let mut state = self.state.lock();
        if state.state != ProcessState::Running {
            return None;
        }
        let vm = state.body.as_ref()?.vm.clone();
        state.running = state
            .running
            .checked_add(1)
            .expect("running count exhausted");
        Some(RunPermit {
            shell: self.clone(),
            vm: Some(vm),
            slot: None,
        })
    }

    /// 首次终止者完整封闭两个门并交出快照；之后方可发布回收 Work。
    pub(crate) fn freeze_and_seal(
        self: &Arc<Self>,
        cause: TerminationCause,
        detail: TerminationDetail,
    ) -> bool {
        let freeze = self.freeze(cause, detail);
        self.finish_freeze(freeze)
    }

    fn finish_freeze(self: &Arc<Self>, freeze: Freeze) -> bool {
        match freeze {
            Freeze::Already => return false,
            Freeze::Began(None) => {}
            Freeze::Began(Some((vm, retire))) => {
                let active = vm.close_entry();
                retire.seal_exit(active);
                if active != 0 {
                    let _ = crate::runtime_stop::notify_slots(active);
                }
            }
        }
        self.publish_reclaim();
        true
    }

    /// Shell gate 只仲裁终因与 Body 准入，不在其锁内进入 VM。
    fn freeze(&self, cause: TerminationCause, detail: TerminationDetail) -> Freeze {
        let mut state = self.state.lock();
        self.freeze_locked(&mut state, cause, detail)
    }

    fn freeze_locked(
        &self,
        state: &mut ShellState,
        cause: TerminationCause,
        detail: TerminationDetail,
    ) -> Freeze {
        if state.state == ProcessState::Terminating || state.state == ProcessState::Dead {
            return Freeze::Already;
        }
        state.state = ProcessState::Terminating;
        state.cause = cause;
        state.detail = detail;
        let previous = self.ingress.fetch_or(FROZEN, Ordering::AcqRel);
        assert!(previous & FROZEN == 0, "process frozen twice");
        Freeze::Began(state.body.as_ref().map(|body| {
            (
                body.vm.clone(),
                body.retire
                    .as_ref()
                    .expect("Body lost unactivated retirement")
                    .clone(),
            )
        }))
    }

    pub(crate) fn snapshot(&self) -> ProcessSnapshot {
        let state = self.state.lock();
        ProcessSnapshot {
            pid: self.pid,
            parent_pid: self.parent_pid,
            state: state.state as u32,
            cause: state.cause as u32,
            detail: state.detail,
            reserved: 0,
        }
    }

    /// 调用者已完成空间退休及私有实际退款，之后才允许 Job 摘成员。
    pub(crate) fn mark_dead(&self) {
        let mut state = self.state.lock();
        assert!(
            state.state == ProcessState::Terminating,
            "Dead without Frozen"
        );
        assert!(
            state.body.is_none()
                && !state.construction_pending
                && self.ingress.load(Ordering::Acquire) & INGRESS_COUNT == 0
                && state.receipt_leases == 0
                && state.member_count == 0
                && state.reserved_members == 0
                && state.running == 0,
            "Dead with private body"
        );
        state.state = ProcessState::Dead;
    }

    /// 已登记回执与短访问全部退尽；VM 自有访问租约另等自身门。
    pub(crate) fn wait_quiescent(&self, token: WakeToken) -> Option<WakeToken> {
        let mut state = self.state.lock();
        assert!(
            state.state == ProcessState::Terminating,
            "wait before Frozen"
        );
        if self.quiescent(&state) {
            return Some(token);
        }
        assert!(
            state.quiescence_waiter.is_none(),
            "duplicate body quiescence waiter"
        );
        state.quiescence_waiter = Some(token);
        None
    }

    pub(super) fn begin_retire(&self) -> bool {
        let mut state = self.state.lock();
        assert!(
            state.state == ProcessState::Terminating && self.quiescent(&state),
            "retire before Shell quiescence"
        );
        assert!(!state.retiring, "Body retired twice");
        state.retiring = true;
        state.body.is_some()
    }

    /// 仅回收 Work 调用；退休期间没有新外部/回执访问，Box 不会移动。
    pub(super) fn with_retiring_body<R>(&self, operation: impl FnOnce(&ProcessBody) -> R) -> R {
        let body = {
            let state = self.state.lock();
            assert!(
                state.retiring && self.quiescent(&state),
                "Body not exclusively retired"
            );
            NonNull::from(state.body.as_deref().expect("missing retired Body"))
        };
        // SAFETY: begin_retire 关闭两类新访问并确认原有短访问归零；
        // 唯一回收 Work 串行执行 step，只有它在最后一段能 take_body。
        operation(unsafe { body.as_ref() })
    }

    pub(super) fn take_retirement(&self) -> Arc<crate::vm::retire::RetireWork> {
        let mut body = {
            let mut state = self.state.lock();
            assert!(
                state.retiring && self.quiescent(&state),
                "Body not exclusively retired"
            );
            NonNull::from(state.body.as_deref_mut().expect("missing retired Body"))
        };
        // SAFETY: 两类访问已封口且归零；唯一回收 Work 串行，只有它可移出退休 owner。
        unsafe { body.as_mut() }
            .retire
            .take()
            .expect("space retired twice")
    }

    pub(crate) fn take_body(&self) -> Option<super::body::BodyStorage> {
        let mut state = self.state.lock();
        assert!(
            state.state == ProcessState::Terminating,
            "body retired before Frozen"
        );
        assert!(
            state.retiring && self.quiescent(&state),
            "body retired with live leases"
        );
        state.body.take()
    }

    fn release_body(&self) {
        self.finish_ingress();
    }

    fn release_receipt(&self) {
        let wake = {
            let mut state = self.state.lock();
            assert!(state.receipt_leases > 0, "receipt lease underflow");
            state.receipt_leases -= 1;
            self.ready_wake(&mut state)
        };
        if let Some(wake) = wake {
            drop(mt::wake(wake));
            mt::kick();
        }
    }

    fn release_running(&self) {
        let wake = {
            let mut state = self.state.lock();
            assert!(state.running > 0, "running admission underflow");
            state.running -= 1;
            self.ready_wake(&mut state)
        };
        if let Some(wake) = wake {
            drop(mt::wake(wake));
            mt::kick();
        }
    }
}

impl ProcessShell {
    fn quiescent(&self, state: &ShellState) -> bool {
        self.ingress_done(state)
            && state.receipt_leases == 0
            && state.member_count == 0
            && state.reserved_members == 0
            && state.running == 0
    }

    fn ready_wake(&self, state: &mut ShellState) -> Option<WakeToken> {
        if state.state == ProcessState::Terminating && self.quiescent(state) {
            state.quiescence_waiter.take()
        } else {
            None
        }
    }
}

/// 仅初始构造者持有，不跨等待或返回用户态；Frozen 会拒绝安装但等它回滚。
pub(crate) struct ConstructionLease {
    shell: Arc<ProcessShell>,
}

impl ConstructionLease {
    pub(crate) fn install_body(
        &self,
        body: super::body::BodyStorage,
    ) -> Result<(), super::body::BodyStorage> {
        let mut state = self.shell.state.lock();
        assert!(
            state.construction_pending,
            "body installed after constructor left"
        );
        if state.state != ProcessState::Building {
            return Err(body);
        }
        assert!(state.body.is_none(), "body installed twice");
        state.body = Some(body);
        Ok(())
    }
}

impl Drop for ConstructionLease {
    fn drop(&mut self) {
        let (ingress, settle) = {
            let mut state = self.shell.state.lock();
            assert!(
                state.construction_pending,
                "construction lease released twice"
            );
            state.construction_pending = false;
            (
                self.shell.ready_ingress_wake(&mut state),
                self.shell.ready_wake(&mut state),
            )
        };
        ProcessShell::wake(ingress, settle);
    }
}

/// 已准入的 Ready 发布；入队（或 Frozen 拒绝后的显式放弃）后锁外归还。
pub(crate) struct ReadyIngress {
    shell: Arc<ProcessShell>,
    published: bool,
}

impl ReadyIngress {
    /// 现成 admission 不重新取容量；入队后门铃覆盖跨域 WFI 的窗口。
    pub(crate) fn publish_admitted(mut self, thread: crate::sched::AdmittedThread) {
        crate::sched::enqueue(thread);
        self.published = true;
    }
}

impl Drop for ReadyIngress {
    fn drop(&mut self) {
        assert!(self.published, "Ready ingress abandoned before publication");
        self.shell.finish_ingress();
    }
}

/// 指针只在 Shell 中的 Box 不可拆除期间借用，不拥有 Body。
pub(crate) struct BodyLease {
    shell: Arc<ProcessShell>,
    body: NonNull<ProcessBody>,
    kind: Option<LeaseKind>,
    _local: PhantomData<*mut ()>,
}

enum LeaseKind {
    External,
    Receipt,
}

/// 已提交内核责任的完成回执；只保留准入计数，不携带 Body 指针或借用。
pub(crate) struct WorkReceipt {
    shell: Arc<ProcessShell>,
}

impl BodyLease {
    pub(super) fn into_work_receipt(mut self) -> WorkReceipt {
        assert!(
            matches!(self.kind, Some(LeaseKind::External)),
            "receipt requires external ingress"
        );
        self.kind = None;
        WorkReceipt {
            shell: self.shell.clone(),
        }
    }
}

impl Drop for WorkReceipt {
    fn drop(&mut self) {
        self.shell.release_body();
    }
}

impl Deref for BodyLease {
    type Target = ProcessBody;

    fn deref(&self) -> &Self::Target {
        // SAFETY: 创建时在 Shell gate 下递增 lease；Body 仅在 Frozen 且所有
        // lease 归零后从 Box 取出。此指针只暴露不可变借用，Drop 先结束借用。
        unsafe { self.body.as_ref() }
    }
}

impl Drop for BodyLease {
    fn drop(&mut self) {
        match self.kind {
            Some(LeaseKind::External) => self.shell.release_body(),
            Some(LeaseKind::Receipt) => self.shell.release_receipt(),
            None => {}
        }
    }
}

/// Shell 运行准入先成立，再由 VM 门决定能否进入；退出必须先注销 VM。
pub(crate) struct RunPermit {
    shell: Arc<ProcessShell>,
    vm: Option<Arc<VmShell>>,
    slot: Option<usize>,
}

impl RunPermit {
    pub(crate) fn enter(&mut self, slot: usize) -> bool {
        assert!(self.slot.is_none(), "running permit entered twice");
        if !self
            .vm
            .as_ref()
            .expect("running permit lost VM")
            .enter(slot)
        {
            return false;
        }
        self.slot = Some(slot);
        true
    }

    /// 调用点已切回内核 root、发布显式访问；随后 VM → Shell 分段归还。
    pub(crate) fn leave(mut self) {
        let slot = self.slot.take().expect("running permit never entered");
        self.vm
            .as_ref()
            .expect("running permit lost VM")
            .leave(slot);
    }
}

impl Drop for RunPermit {
    fn drop(&mut self) {
        assert!(self.slot.is_none(), "running permit dropped before VM exit");
        // running 仍非零，Body 保有 VM；退本强根后才能让父回收通过 Shell 屏障。
        drop(self.vm.take());
        self.shell.release_running();
    }
}

impl KernelObject for ProcessShell {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::ProcessControl
    }

    fn allowed_rights(&self) -> Option<Rights> {
        Some(CONTROL_RIGHTS)
    }

    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::DONE)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.source.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
