//! WaitContext：多对象等待的安装、完成仲裁、订阅清理与结果交付。

pub(crate) mod selftest;

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    time::Deadline,
    wait::{WAIT_MANY_MAX, WaitCookie, WaitItem, WaitManyRequest, WaitReason, WaitResult},
};
use num_traits::ToPrimitive;
use wait_context::{ArmResult, OfferResult, TimeoutRegistration, WaitCore, WaitEpoch};

use crate::{context::UserContext, sched, sync::Spinlock, uaccess};

use super::{
    Thread,
    object::{ObjectRef, ObjectWaitState},
};

/// 同一对象上的一个已解析等待输入。
#[derive(Clone)]
pub(crate) struct WaitInterest {
    pub signals: ObjectSignals,
    pub cookie: WaitCookie,
    pub index: u32,
    _authority: ObjectRef,
}

/// syscall 阶段按对象归并并保留授权的观察组。同一对象只登记一次，
/// 从而在一个对象更新快照内确定最小 item_index。
pub struct ResolvedWaitGroup {
    pub object: ObjectRef,
    items: Vec<WaitInterest>,
}

/// 线程离开执行点前登记的等待意图。
pub struct WaitPlan {
    cancel_on_drop: bool,
    pub groups: Vec<ResolvedWaitGroup>,
    pub action: WaitAction,
    pub expires_at: Option<u64>,
    /// Commit 前预构造的 context；普通对象等待由 install 阶段创建。
    prepared: Option<WaitIdentity>,
    operation: Option<Arc<dyn super::request::WaitOperation>>,
}

impl Drop for WaitPlan {
    fn drop(&mut self) {
        if let Some(identity) = self.prepared.take()
            && (identity.reusable || self.cancel_on_drop)
        {
            identity.abandon();
            self.operation.take();
            if matches!(identity.core.arm_in(identity.epoch), ArmResult::Complete(_)) {
                finish_offered(identity);
            }
        } else {
            self.operation.take();
        }
    }
}

impl WaitPlan {
    pub(crate) fn bind_operation(&mut self, operation: Arc<dyn super::request::WaitOperation>) {
        assert!(self.operation.replace(operation).is_none());
    }

    pub(crate) fn committed_reply(&mut self) {
        self.cancel_on_drop = true;
    }
}

/// 等待完成后如何写回用户现场。
pub enum WaitStart {
    Ready,
    Park(WaitPlan),
}

/// WaitMany syscall 入口：复制 ABI、解析 Handle/rights，并完成初始检查。
/// 结构化请求直接携带绝对期限，不从安装时刻重置预算。
pub fn prepare(thread: &Thread, request_ptr: usize) -> Result<WaitStart, SystemCallError> {
    let request: WaitManyRequest = {
        let mut space = thread.process.space.lock();
        // SAFETY: 请求只含固定宽整数，任意位型可读取，随后验证判别与 reserved。
        unsafe { uaccess::read_user_value(&mut space, request_ptr) }?
    };
    if request.reserved != 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    prepare_items(
        thread,
        request.items as usize,
        request.count as usize,
        request.result as usize,
        request.deadline,
    )
}

fn prepare_items(
    thread: &Thread,
    items_ptr: usize,
    count: usize,
    result_ptr: usize,
    deadline: Deadline,
) -> Result<WaitStart, SystemCallError> {
    let expires_at = crate::clock::deadline_ticks(deadline)?;
    if count == 0 || count > WAIT_MANY_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let raw_len = count
        .checked_mul(core::mem::size_of::<WaitItem>())
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(raw_len)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    raw.resize(raw_len, 0);
    {
        let mut space = thread.process.space.lock();
        uaccess::copy_from_user(&mut space, &mut raw, items_ptr)?;
        space.check_range(result_ptr, core::mem::size_of::<WaitResult>(), true)?;
    }

    let mut abi_items = Vec::new();
    abi_items
        .try_reserve_exact(count)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for bytes in raw.as_chunks::<{ core::mem::size_of::<WaitItem>() }>().0 {
        // SAFETY: WaitItem 仅含整数 newtype；用户缓冲无需对齐。
        abi_items.push(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<WaitItem>()) });
    }

    let mut groups: Vec<ResolvedWaitGroup> = Vec::new();
    groups
        .try_reserve_exact(count)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    {
        let table = thread.process.handles.lock();
        for (index, item) in abi_items.iter().copied().enumerate() {
            if item.reserved != 0 || item.signals == ObjectSignals::NONE || !item.signals.is_known()
            {
                return Err(SystemCallError::IllegalArgument);
            }
            let entry = table
                .get(item.handle, erhino_shared::object::Rights::WAIT)
                .map_err(super::handle::map_error)?;
            let allowed = entry
                .object()
                .allowed_signals(*entry.role())
                .ok_or(SystemCallError::WrongObjectType)?;
            if item.signals.raw() & !allowed.raw() != 0 {
                return Err(SystemCallError::IllegalArgument);
            }
            let authority = entry.object().clone();
            let object = authority
                .observation_source()
                .unwrap_or_else(|| authority.clone());
            let interest = WaitInterest {
                signals: item.signals,
                cookie: item.cookie,
                index: index as u32,
                _authority: authority,
            };
            if let Some(group) = groups
                .iter_mut()
                .find(|group| Arc::ptr_eq(&group.object, &object))
            {
                group
                    .items
                    .try_reserve(1)
                    .map_err(|_| SystemCallError::OutOfMemory)?;
                group.items.push(interest);
            } else {
                let mut items = Vec::new();
                items
                    .try_reserve_exact(1)
                    .map_err(|_| SystemCallError::OutOfMemory)?;
                items.push(interest);
                groups.push(ResolvedWaitGroup { object, items });
            }
        }
    }

    let ready = groups
        .iter()
        .filter_map(|group| Subscription::outcome_for(&group.items, group.object.signals()))
        .min_by_key(|result| result.item_index);
    if let Some(result) = ready {
        let mut space = thread.process.space.lock();
        // SAFETY: 初始扫描按真实输入索引选择，结果完整初始化且无 padding。
        unsafe { uaccess::write_user_value(&mut space, result_ptr, &result) }?;
        return Ok(WaitStart::Ready);
    }

    let expired = if expires_at.is_some() {
        let now = crate::clock::now_ticks()?;
        expires_at.is_some_and(|expires| now >= expires)
    } else {
        false
    };
    if expired {
        let result = WaitResult::new(0, ObjectSignals::NONE, u32::MAX, WaitReason::Timeout);
        let mut space = thread.process.space.lock();
        // SAFETY: 固定宽结果及 reserved 完整初始化，失败不发布等待。
        unsafe { uaccess::write_user_value(&mut space, result_ptr, &result) }?;
        return Ok(WaitStart::Ready);
    }

    Ok(WaitStart::Park(WaitPlan {
        cancel_on_drop: false,
        groups,
        action: WaitAction::WaitMany { result_ptr },
        expires_at,
        prepared: None,
        operation: None,
    }))
}

pub fn sleep_plan(deadline: Deadline) -> Result<WaitPlan, SystemCallError> {
    let expires_at =
        crate::clock::deadline_ticks(deadline)?.ok_or(SystemCallError::IllegalArgument)?;
    Ok(WaitPlan {
        cancel_on_drop: false,
        groups: Vec::new(),
        action: WaitAction::Sleep,
        expires_at: Some(expires_at),
        prepared: None,
        operation: None,
    })
}

/// 等待完成后如何写回用户现场。
#[derive(Debug, Clone, Copy)]
pub enum WaitAction {
    WaitMany { result_ptr: usize },
    Sleep,
    KernelResult { value: usize },
}

#[derive(Debug, Clone, Copy)]
pub enum WaitOutcome {
    Object(WaitResult),
    Error(SystemCallError),
    KernelComplete,
    Timeout,
    Cancelled,
    /// 终止取消：线程不回用户态，随上下文消散（kill/abandonment 路径）。
    Abandoned,
}

/// 在途事件捕获某一轮等待，不通过可复用 context 的当前身份重新定位。
#[derive(Clone)]
pub struct WaitIdentity {
    context: Arc<WaitContext>,
    epoch: WaitEpoch,
}

#[derive(Clone)]
pub(crate) struct WeakWaitIdentity {
    context: Weak<WaitContext>,
    epoch: WaitEpoch,
}

impl WeakWaitIdentity {
    pub(crate) fn upgrade(&self) -> Option<WaitIdentity> {
        self.context.upgrade().map(|context| WaitIdentity {
            context,
            epoch: self.epoch,
        })
    }
}

impl WaitIdentity {
    fn new(context: Arc<WaitContext>) -> Self {
        let epoch = context.core.epoch();
        Self { context, epoch }
    }

    pub(crate) fn downgrade(&self) -> WeakWaitIdentity {
        WeakWaitIdentity {
            context: Arc::downgrade(&self.context),
            epoch: self.epoch,
        }
    }

    pub(crate) fn offer(&self, outcome: WaitOutcome) -> OfferResult {
        self.context.offer_in(self.epoch, outcome)
    }

    pub(crate) fn abandon(&self) -> OfferResult {
        if !self.core.abandon(self.epoch) {
            return OfferResult::Lost;
        }
        let result = self.offer(WaitOutcome::Abandoned);
        let executor = {
            let cancellation = self.context.request_cancellation.lock();
            cancellation.as_ref().and_then(Weak::upgrade)
        };
        if let Some(executor) = executor {
            super::request::WaitOperation::cancel(&*executor, self.key());
        }
        result
    }

    pub(crate) fn bind_cancellation(&self, executor: Weak<dyn super::request::WaitOperation>) {
        *self.context.request_cancellation.lock() = Some(executor);
    }

    pub(crate) fn is_abandoned(&self) -> bool {
        self.core.is_abandoned(self.epoch)
    }

    pub(crate) fn key(&self) -> crate::deferred_work::WaitKey {
        crate::deferred_work::WaitKey {
            context: Arc::as_ptr(&self.context) as usize,
            epoch: self.epoch.value(),
        }
    }

    /// token 与等待轮次共同限定到期事件，不能重新定位复用后的 context。
    pub(crate) fn expire(self, token: timer_queue::TimerToken) {
        if self.timeout_registration.retire(token)
            && self.offer(WaitOutcome::Timeout) == OfferResult::Complete
        {
            finish_offered(self);
        }
    }

    pub(crate) fn complete_kernel(self) {
        if self.offer(WaitOutcome::KernelComplete) == OfferResult::Complete {
            finish_offered(self);
        }
    }
}

impl core::ops::Deref for WaitIdentity {
    type Target = WaitContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

#[derive(Clone)]
pub(crate) enum ObserverSink {
    Thread(WaitIdentity),
    Persistent(Arc<super::wait_set::ArmCycle>),
}

impl From<WaitIdentity> for ObserverSink {
    fn from(context: WaitIdentity) -> Self {
        Self::Thread(context)
    }
}

impl ObserverSink {
    pub(crate) fn persistent(&self) -> bool {
        matches!(self, Self::Persistent(_))
    }

    pub(crate) fn reuses_finish(&self) -> bool {
        match self {
            Self::Thread(identity) => identity.reusable,
            Self::Persistent(_) => true,
        }
    }

    pub(crate) fn restart(&self) -> Result<u64, SystemCallError> {
        match self {
            Self::Persistent(cycle) => cycle.restart(),
            Self::Thread(_) => Err(SystemCallError::WrongObjectType),
        }
    }

    pub(crate) fn arm(&self) -> Option<ObserverSink> {
        match self {
            Self::Persistent(cycle) => cycle.arm().then(|| self.clone()),
            Self::Thread(_) => unreachable!("thread observer cannot be rearmed"),
        }
    }

    pub(crate) fn complete_finish(
        &self,
        reservation: Option<super::notify_work::FinishReservation>,
    ) {
        match self {
            Self::Persistent(cycle) => cycle.return_finish(
                reservation.expect("persistent finish lost its prepaid reservation"),
            ),
            Self::Thread(context) => {
                context.complete_finish(context.epoch, reservation);
            }
        }
    }

    pub(crate) fn offer(&self, outcome: WaitOutcome) -> OfferResult {
        match self {
            Self::Thread(context) => context.offer(outcome),
            Self::Persistent(cycle) => cycle.offer(outcome),
        }
    }

    pub(crate) fn finish_step(&self, budget: usize) -> (usize, bool) {
        match self {
            Self::Thread(context) => context.finish_step(context.epoch, budget),
            Self::Persistent(cycle) => cycle.finish_step(budget),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Subscription {
    pub sink: ObserverSink,
    /// 订阅所属对象；通知债务通过它交给目标 drain owner。
    pub object: ObjectRef,
    items: Vec<WaitInterest>,
}

impl Subscription {
    pub(crate) fn single(
        sink: ObserverSink,
        source: ObjectRef,
        authority: ObjectRef,
        item: WaitItem,
    ) -> Result<Self, SystemCallError> {
        let mut items = Vec::new();
        items
            .try_reserve_exact(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        items.push(WaitInterest {
            signals: item.signals,
            cookie: item.cookie,
            index: 0,
            _authority: authority,
        });
        Ok(Self {
            sink,
            object: source,
            items,
        })
    }

    pub(crate) fn interest(&self) -> ObjectSignals {
        self.items
            .iter()
            .fold(ObjectSignals::NONE, |all, item| all | item.signals)
    }

    pub(crate) fn outcome_for(
        items: &[WaitInterest],
        current: ObjectSignals,
    ) -> Option<WaitResult> {
        let closed = current.intersects(ObjectSignals::CLOSED);
        items
            .iter()
            .filter(|item| closed || current.intersects(item.signals))
            .min_by_key(|item| item.index)
            .map(|item| {
                let observed = (current & item.signals)
                    | if closed {
                        ObjectSignals::CLOSED
                    } else {
                        ObjectSignals::NONE
                    };
                WaitResult::new(
                    item.cookie,
                    observed,
                    item.index,
                    if closed {
                        WaitReason::Closed
                    } else {
                        WaitReason::Signaled
                    },
                )
            })
    }

    pub fn outcome(&self, current: ObjectSignals) -> Option<WaitOutcome> {
        Self::outcome_for(&self.items, current).map(WaitOutcome::Object)
    }
}

struct Registration {
    /// 保留已验证观察来源直到注销/完成；Waiting 不依赖最后一个 Handle
    /// 或对象外部 owner 维持这条引用。
    object: ObjectRef,
    id: u64,
}

struct FinishState {
    outcome: WaitOutcome,
    delivered: bool,
    delivery: Option<sched::AdmittedThread>,
}

/// 一次 Waiting 的唯一线程所有者和完成仲裁点。
pub struct WaitContext {
    core: WaitCore<WaitOutcome>,
    thread: Spinlock<Option<sched::AdmittedThread>>,
    registrations: Spinlock<Vec<Registration>>,
    /// 原子注册状态：未登记、稳定 token 或 Closed。
    timeout_registration: TimeoutRegistration,
    action: WaitAction,
    finish_reservation: Spinlock<Option<super::notify_work::FinishReservation>>,
    finish_state: Spinlock<Option<FinishState>>,
    _metadata: Option<super::resources::KernelWaitPermit>,
    reusable: bool,
    request_cancellation: Spinlock<Option<Weak<dyn super::request::WaitOperation>>>,
}

impl WaitContext {
    fn new(
        action: WaitAction,
        registration_capacity: usize,
        metadata: Option<super::resources::KernelWaitPermit>,
        reusable: bool,
    ) -> Result<Arc<Self>, SystemCallError> {
        let mut registrations = Vec::new();
        registrations
            .try_reserve(registration_capacity)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let finish_class = if metadata.is_some() {
            super::notify_work::FinishClass::Kernel
        } else {
            super::notify_work::FinishClass::Thread
        };
        let finish_reservation = super::notify_work::reserve_finish(finish_class)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::try_new(Self {
            core: WaitCore::new(),
            thread: Spinlock::new(crate::sync::ranks::LEAF, None),
            registrations: Spinlock::new(crate::sync::ranks::LEAF, registrations),
            timeout_registration: TimeoutRegistration::new(),
            action,
            finish_reservation: Spinlock::new(crate::sync::ranks::LEAF, Some(finish_reservation)),
            finish_state: Spinlock::new(crate::sync::ranks::LEAF, None),
            _metadata: metadata,
            reusable,
            request_cancellation: Spinlock::new(crate::sync::ranks::LEAF, None),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn prepare_reusable_wait(
        self: &Arc<Self>,
        first_use: bool,
    ) -> Result<(WaitIdentity, WaitPlan), SystemCallError> {
        assert!(self.reusable, "kernel request used a single-use wait");
        if !first_use {
            if !self.core.is_done() || self.finish_reservation.lock().is_none() {
                return Err(SystemCallError::ObjectBusy);
            }
            self.core
                .epoch()
                .next()
                .ok_or(SystemCallError::ReachLimit)?;
            // SAFETY: 批次许可唯一；旧请求、线程和容量已在 DONE 前退役。
            assert!(
                unsafe { self.core.restart_done() },
                "request wait failed to restart"
            );
        }
        let identity = WaitIdentity::new(self.clone());
        let plan = WaitPlan {
            cancel_on_drop: true,
            groups: Vec::new(),
            action: self.action,
            expires_at: None,
            prepared: Some(identity.clone()),
            operation: None,
        };
        Ok((identity, plan))
    }

    fn offer_in(&self, epoch: WaitEpoch, outcome: WaitOutcome) -> OfferResult {
        let result = self.core.offer_in(epoch, outcome);
        if result != OfferResult::Lost {
            // 只退休原子状态；对象锁内的完成方不得在此获取 owner queue 锁。
            self.timeout_registration.close();
        }
        result
    }

    /// queue token 先产生，随后以 CAS 发布；若完成者已关闭 context，立即
    /// 注销刚登记的项，不能留下强持 context 的期限项。
    fn publish_timeout_registration(&self, token: timer_queue::TimerToken) {
        if !self.timeout_registration.publish(token) {
            sched::unregister_wait_timeout(token);
        }
    }

    /// 任何完成路径在触碰对象订阅前关闭 token；跨 hart 仅删除 owner
    /// queue 项，不远程重编程时钟。
    fn close_timeout_registration(&self) {
        self.timeout_registration.close();
        if let Some(token) = self.timeout_registration.take_cancellation() {
            sched::unregister_wait_timeout(token);
        }
    }

    fn remember(&self, object: ObjectRef, id: u64) {
        let mut registrations = self.registrations.lock();
        debug_assert!(registrations.len() < registrations.capacity());
        registrations.push(Registration { object, id });
    }

    pub(crate) fn begin_finish(&self, outcome: WaitOutcome) {
        self.close_timeout_registration();
        let previous = self.finish_state.lock().replace(FinishState {
            outcome,
            delivered: false,
            delivery: None,
        });
        assert!(
            previous.is_none(),
            "wait context completion installed twice"
        );
    }

    /// 推进一个已获完成权的上下文；每次只注销一个 registration 或执行一次
    /// 最终线程交付，完成责任由预付 finish slot 持续承载。
    fn finish_step(&self, epoch: WaitEpoch, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        assert_eq!(
            self.core.epoch(),
            epoch,
            "finish payload references a stale wait epoch"
        );
        let mut used = 0;
        while used < budget {
            let registration = self.registrations.lock().pop();
            if let Some(registration) = registration {
                registration.object.unsubscribe(registration.id);
                used += 1;
                continue;
            }
            let mut state = self.finish_state.lock();
            let finish = state
                .as_mut()
                .expect("wait completion step without finish state");
            if finish.delivered {
                return (used.max(1), true);
            }
            finish.delivered = true;
            drop(state);
            let thread = self.thread.lock().take();
            self.finish_state
                .lock()
                .as_mut()
                .expect("finish state disappeared")
                .delivery = thread;
            used += 1;
            return (used, true);
        }
        (used, false)
    }

    /// 队列已完成槽位与 Pending 交接，才开放终态并在锁外交付线程。
    fn complete_finish(
        &self,
        epoch: WaitEpoch,
        reservation: Option<super::notify_work::FinishReservation>,
    ) {
        if self.reusable {
            let reservation = reservation.expect("request wait lost its prepaid capacity");
            assert!(
                self.finish_reservation
                    .lock()
                    .replace(reservation)
                    .is_none(),
                "request wait returned finish capacity twice"
            );
        } else {
            assert!(
                reservation.is_none(),
                "single-use wait retained finish capacity"
            );
        }
        let finish = self
            .finish_state
            .lock()
            .take()
            .expect("finish completed without a delivery");
        let outcome = if self.core.is_abandoned(epoch) {
            WaitOutcome::Abandoned
        } else {
            finish.outcome
        };
        assert!(
            self.core.mark_done_in(epoch),
            "wait completion epoch changed"
        );
        if let Some(thread) = finish.delivery {
            if matches!(outcome, WaitOutcome::Abandoned)
                || thread.process.lifecycle.is_terminating()
            {
                let departure = thread.departure();
                drop(thread);
                departure.request(super::thread::DepartureKind::Terminated);
            } else {
                self.deliver(&thread, outcome);
                sched::enqueue(thread);
            }
        }
    }

    fn deliver(&self, thread: &Thread, outcome: WaitOutcome) {
        let frame = unsafe { &mut *thread.frame_ptr() };
        match (self.action, outcome) {
            (WaitAction::WaitMany { result_ptr }, WaitOutcome::Object(result)) => {
                deliver_wait_result(thread, frame, result_ptr, result);
            }
            (WaitAction::WaitMany { result_ptr }, WaitOutcome::Timeout) => {
                deliver_wait_result(
                    thread,
                    frame,
                    result_ptr,
                    WaitResult::new(0, ObjectSignals::NONE, u32::MAX, WaitReason::Timeout),
                );
            }
            (WaitAction::WaitMany { .. }, WaitOutcome::Error(error)) => {
                frame.x[10] = error.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            // 已知简化：占位语义——显式取消 ABI 接入前本分支不可达；
            // 接入时需给正式完成语义（notes/impls/ipc.md「等待与期限」）。
            (WaitAction::WaitMany { .. }, WaitOutcome::Cancelled) => {
                frame.x[10] = SystemCallError::FunctionNotAvailable
                    .to_usize()
                    .unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            (WaitAction::Sleep, WaitOutcome::Timeout) => {
                frame.x[10] = 0;
                frame.x[11] = 0;
                frame.sepc += 4;
            }
            (WaitAction::Sleep, WaitOutcome::Error(error)) => {
                frame.x[10] = error.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            (WaitAction::KernelResult { value }, WaitOutcome::KernelComplete) => {
                frame.x[10] = SystemCallError::NoError as u64;
                frame.x[11] = value as u64;
                frame.sepc += 4;
            }
            (WaitAction::KernelResult { .. }, WaitOutcome::Error(error)) => {
                frame.x[10] = error.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            (_, WaitOutcome::Abandoned) => unreachable!("abandoned waits are never delivered"),
            (
                WaitAction::Sleep,
                WaitOutcome::Object(_) | WaitOutcome::Cancelled | WaitOutcome::KernelComplete,
            )
            | (WaitAction::WaitMany { .. }, WaitOutcome::KernelComplete)
            | (
                WaitAction::KernelResult { .. },
                WaitOutcome::Object(_) | WaitOutcome::Timeout | WaitOutcome::Cancelled,
            ) => {
                frame.x[10] = SystemCallError::InternalError.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
        }
    }
}

/// 为 Commit 后必成的内核操作预构造 Installing context。metadata permit 随
/// WaitContext 的真实析构退款，不随 creator 或 completion 提前消散。
pub fn prepare_kernel(
    value: usize,
    metadata: super::resources::KernelWaitPermit,
) -> Result<(WaitIdentity, WaitPlan), SystemCallError> {
    let context = WaitIdentity::new(WaitContext::new(
        WaitAction::KernelResult { value },
        0,
        Some(metadata),
        false,
    )?);
    let plan = WaitPlan {
        cancel_on_drop: false,
        groups: Vec::new(),
        action: WaitAction::KernelResult { value },
        expires_at: None,
        prepared: Some(context.clone()),
        operation: None,
    };
    Ok((context, plan))
}

pub(crate) fn prepare_request(
    metadata: super::resources::KernelWaitPermit,
) -> Result<Arc<WaitContext>, SystemCallError> {
    WaitContext::new(
        WaitAction::KernelResult { value: 0 },
        0,
        Some(metadata),
        true,
    )
}

/// 调度循环在线程离开执行点后安装一次 WaitMany：Waiting 记录与
/// 可取消性在 lifecycle 锁内线性化；已 Terminating 则不发布等待，
/// 直接以 Abandoned 取消（线程不回用户态）。
pub fn install(thread: sched::AdmittedThread, mut plan: WaitPlan) {
    let operation = plan.operation.take();
    let context = match plan.prepared.take() {
        Some(context) => context,
        None => match WaitContext::new(plan.action, plan.groups.len(), None, false) {
            Ok(context) => WaitIdentity::new(context),
            Err(error) => {
                deliver_install_error(thread, plan.action, error);
                return;
            }
        },
    };
    let previous = context.thread.lock().replace(thread);
    assert!(
        previous.is_none(),
        "wait context received thread ownership twice"
    );
    {
        let (process, member) = context_thread_identity(&context);
        if !process.lifecycle.park_waiting(member, &context) {
            // 终止取得 park 线性化点后，业务 completion 即使已经到达也只
            // 能代表事务完成，不能恢复已经放弃回复权的线程。安装者统一取得
            // Installing 完成权并以 Abandoned 执行 departure confirmation。
            let _ = context.abandon();
            drop(operation);
            match context.core.arm_in(context.epoch) {
                ArmResult::Complete(_) => finish_offered(context),
                ArmResult::Armed | ArmResult::ExternalCompleter => (),
            }
            return;
        }
    }
    if let Some(operation) = operation {
        operation.start(context.key());
    }

    if let Some(expires_at) = plan.expires_at {
        let now = match crate::clock::now_ticks() {
            Ok(now) => now,
            Err(_) => {
                crate::runtime_stop::check();
                unreachable!("runtime stop did not park after clock failure");
            }
        };
        if now >= expires_at {
            context.offer(WaitOutcome::Timeout);
        } else {
            match sched::register_wait_timeout(expires_at, context.clone()) {
                Ok(token) => context.publish_timeout_registration(token),
                Err(()) => {
                    context.offer(WaitOutcome::Error(SystemCallError::OutOfMemory));
                }
            }
        }
    }

    for group in core::mem::take(&mut plan.groups) {
        if context.core.has_outcome() {
            break;
        }
        let object = group.object;
        let subscription = Subscription {
            sink: ObserverSink::Thread(context.clone()),
            object: object.clone(),
            items: group.items,
        };
        match object.subscribe(subscription) {
            super::object::SubscribeResult::Ready { outcome, retired } => {
                drop(retired);
                context.offer(outcome);
            }
            super::object::SubscribeResult::Registered(id) => {
                context.remember(object, id);
            }
            super::object::SubscribeResult::ReachLimit(retired) => {
                drop(retired);
                context.offer(WaitOutcome::Error(SystemCallError::ReachLimit));
            }
            super::object::SubscribeResult::OutOfMemory(retired) => {
                drop(retired);
                context.offer(WaitOutcome::Error(SystemCallError::OutOfMemory));
            }
        }
        if context.core.has_outcome() {
            break;
        }
    }

    if context.core.has_outcome() {
        let outcome = context
            .core
            .finish_installing_in(context.epoch)
            .expect("Installing owner must finish an existing outcome");
        context.begin_finish(outcome);
        let reservation = context
            .finish_reservation
            .lock()
            .take()
            .expect("installing wait lost finish reservation");
        super::notify_work::publish_finish(reservation, context.clone());
        return;
    }

    match context.core.arm_in(context.epoch) {
        ArmResult::Armed => {}
        ArmResult::Complete(outcome) => {
            context.begin_finish(outcome);
            let reservation = context
                .finish_reservation
                .lock()
                .take()
                .expect("armed wait lost finish reservation");
            super::notify_work::publish_finish(reservation, context.clone());
        }
        ArmResult::ExternalCompleter => {
            // offer 方已取得完成权并负责清理/交付。
        }
    }
}

/// 安装中的上下文必持有发起线程；取其进程引用与 tid 做 lifecycle 线性化。
fn context_thread_identity(
    context: &WaitIdentity,
) -> (
    alloc::sync::Arc<super::proc::Process>,
    super::lifecycle::MemberKey,
) {
    let guard = context.thread.lock();
    let thread = guard.as_ref().expect("installing context holds its thread");
    (thread.process.clone(), thread.member())
}

/// 将 WaitResult 写回用户现场并推进 sepc；写回失败则携带错误返回。
fn deliver_wait_result(
    thread: &Thread,
    frame: &mut UserContext,
    result_ptr: usize,
    result: WaitResult,
) {
    // SAFETY: WaitResult 字段和 reserved 全部初始化，结构无 padding。
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&result as *const WaitResult).cast::<u8>(),
            core::mem::size_of::<WaitResult>(),
        )
    };
    let copied = {
        let mut space = thread.process.space.lock();
        uaccess::put_user_indirect(&mut space, result_ptr, bytes)
    };
    match copied {
        Ok(()) => {
            frame.x[10] = 0;
            frame.x[11] = 0;
        }
        Err(error) => {
            let error = SystemCallError::from(error);
            frame.x[10] = error.to_usize().unwrap_or(1) as u64;
        }
    }
    frame.sepc += 4;
}

fn deliver_install_error(
    thread: sched::AdmittedThread,
    _action: WaitAction,
    error: SystemCallError,
) {
    // SAFETY: install 只在线程离开执行点后调用，本 hart 独占尚未发布的现场。
    let frame = unsafe { &mut *thread.frame_ptr() };
    frame.x[10] = error.to_usize().unwrap_or(1) as u64;
    frame.sepc += 4;
    sched::enqueue(thread);
}

/// 更新对象电平后交出一次已支付通知债务；不在发布者栈上排水全部等待者。
pub(crate) fn schedule_waiters(wait: &Spinlock<ObjectWaitState>) {
    let Some((reservation, target)) = wait.lock().take_notification() else {
        return;
    };
    reservation.publish(target);
}

/// 对象信号更新在释放对象锁后调用；只有 Complete 方可进入。
pub(crate) fn finish_offered(sink: impl Into<ObserverSink>) {
    match sink.into() {
        ObserverSink::Thread(context) => {
            let outcome = context.core.outcome_in(context.epoch);
            context.begin_finish(outcome);
            let reservation = context
                .finish_reservation
                .lock()
                .take()
                .expect("offered wait lost finish reservation");
            super::notify_work::publish_finish(reservation, context);
        }
        ObserverSink::Persistent(cycle) => cycle.publish_finish(),
    }
}
