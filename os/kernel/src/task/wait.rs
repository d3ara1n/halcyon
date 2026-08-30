//! WaitContext：多对象等待的安装、完成仲裁、订阅清理与结果交付。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    wait::{WAIT_MANY_MAX, WAIT_TIMEOUT_INFINITE, WaitCookie, WaitItem, WaitReason, WaitResult},
};
use num_traits::ToPrimitive;
use wait_context::{ArmResult, OfferResult, TimeoutRegistration, WaitCore};

use crate::{context::UserContext, sched, sync::Spinlock, uaccess};

use super::{
    Thread,
    object::{KernelObject, ObjectRef},
};

/// syscall 阶段已解析并保留授权的观察项。
pub struct ResolvedWaitItem {
    pub object: ObjectRef,
    pub signals: ObjectSignals,
    pub cookie: WaitCookie,
    pub index: u32,
}

/// 线程离开执行点前登记的等待意图。
pub struct WaitPlan {
    pub items: Vec<ResolvedWaitItem>,
    pub action: WaitAction,
    pub expires_at: Option<u64>,
    /// Commit 前预构造的 context；普通对象等待由 install 阶段创建。
    prepared: Option<Arc<WaitContext>>,
}

/// 等待完成后如何写回用户现场。
pub enum WaitStart {
    Ready,
    Park(WaitPlan),
}

/// WaitMany syscall 入口：复制 ABI、解析 Handle/rights，并完成初始检查。
/// `timeout_ms` 为相对毫秒超时，`0` 表示无限等待。
pub fn prepare(
    thread: &Thread,
    items_ptr: usize,
    count: usize,
    result_ptr: usize,
    timeout_ms: u64,
) -> Result<WaitStart, SystemCallError> {
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
    for bytes in raw.chunks_exact(core::mem::size_of::<WaitItem>()) {
        // SAFETY: WaitItem 仅含整数 newtype；用户缓冲无需对齐。
        abi_items.push(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<WaitItem>()) });
    }

    let mut items = Vec::new();
    items
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
            items.push(ResolvedWaitItem {
                object: entry.object().clone(),
                signals: item.signals,
                cookie: item.cookie,
                index: index as u32,
            });
        }
    }

    for item in &items {
        let current = item.object.signals();
        if current.intersects(item.signals) || current.intersects(ObjectSignals::CLOSED) {
            let closed = current.intersects(ObjectSignals::CLOSED);
            let observed = (current & item.signals)
                | if closed {
                    ObjectSignals::CLOSED
                } else {
                    ObjectSignals::NONE
                };
            let result = WaitResult::new(
                item.cookie,
                observed,
                item.index,
                if closed {
                    WaitReason::Closed
                } else {
                    WaitReason::Signaled
                },
            );
            let mut space = thread.process.space.lock();
            // SAFETY: WaitResult 字段和 reserved 全部初始化，结构无 padding。
            unsafe { uaccess::write_user_value(&mut space, result_ptr, &result) }?;
            return Ok(WaitStart::Ready);
        }
    }

    let expires_at = if timeout_ms == WAIT_TIMEOUT_INFINITE {
        None
    } else {
        Some(sched::expires_after_ms(timeout_ms))
    };

    Ok(WaitStart::Park(WaitPlan {
        items,
        action: WaitAction::WaitMany { result_ptr },
        expires_at,
        prepared: None,
    }))
}

pub fn sleep_plan(expires_at: u64) -> WaitPlan {
    WaitPlan {
        items: Vec::new(),
        action: WaitAction::Sleep,
        expires_at: Some(expires_at),
        prepared: None,
    }
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
    #[expect(dead_code, reason = "显式取消 ABI 接入后使用")]
    Cancelled,
    /// 终止取消：线程不回用户态，随上下文消散（kill/abandonment 路径）。
    Abandoned,
}

#[derive(Clone)]
pub(crate) struct Subscription {
    pub context: Arc<WaitContext>,
    pub interest: ObjectSignals,
    pub cookie: WaitCookie,
    pub item_index: u32,
}

impl Subscription {
    pub fn outcome(&self, current: ObjectSignals) -> WaitOutcome {
        let closed = current.intersects(ObjectSignals::CLOSED);
        let observed = (current & self.interest)
            | if closed {
                ObjectSignals::CLOSED
            } else {
                ObjectSignals::NONE
            };
        WaitOutcome::Object(WaitResult::new(
            self.cookie,
            observed,
            self.item_index,
            if closed {
                WaitReason::Closed
            } else {
                WaitReason::Signaled
            },
        ))
    }
}

struct Registration {
    object: Weak<dyn KernelObject>,
    id: u64,
}

/// 一次 Waiting 的唯一线程所有者和完成仲裁点。
pub struct WaitContext {
    core: WaitCore<WaitOutcome>,
    thread: Spinlock<Option<Arc<Thread>>>,
    registrations: Spinlock<Vec<Registration>>,
    /// 原子注册状态：未登记、稳定 token 或 Closed。
    timeout_registration: TimeoutRegistration,
    action: WaitAction,
}

impl WaitContext {
    fn new(action: WaitAction, registration_capacity: usize) -> Result<Arc<Self>, SystemCallError> {
        let mut registrations = Vec::new();
        registrations
            .try_reserve(registration_capacity)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::try_new(Self {
            core: WaitCore::new(),
            thread: Spinlock::new(crate::sync::ranks::LEAF, None),
            registrations: Spinlock::new(crate::sync::ranks::LEAF, registrations),
            timeout_registration: TimeoutRegistration::new(),
            action,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn offer(&self, outcome: WaitOutcome) -> OfferResult {
        let result = self.core.offer(outcome);
        if result != OfferResult::Lost {
            // 只退休原子状态；对象锁内的完成方不得在此获取 owner queue 锁。
            self.timeout_registration.close();
        }
        result
    }

    /// 内核事务在完成其业务所有权收束后提交唯一成功结果。
    pub(crate) fn complete_kernel(self: Arc<Self>) {
        if self.offer(WaitOutcome::KernelComplete) == OfferResult::Complete {
            finish_offered(self);
        }
    }

    /// 在本 hart timer queue 弹出到期项后调用。只有仍发布该 token 的
    /// context 能退休它并竞争 Timeout outcome。
    pub(crate) fn expire(self: Arc<Self>, token: timer_queue::TimerToken) {
        if self.timeout_registration.retire(token)
            && self.offer(WaitOutcome::Timeout) == OfferResult::Complete
        {
            finish_offered(self);
        }
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

    fn remember(&self, object: &ObjectRef, id: u64) {
        let mut registrations = self.registrations.lock();
        debug_assert!(registrations.len() < registrations.capacity());
        registrations.push(Registration {
            object: Arc::downgrade(object),
            id,
        });
    }

    fn cleanup(&self) {
        let registrations = {
            let mut held = self.registrations.lock();
            core::mem::take(&mut *held)
        };
        for registration in registrations {
            if let Some(object) = registration.object.upgrade() {
                object.unsubscribe(registration.id);
            }
        }
    }

    fn finish(self: &Arc<Self>, outcome: WaitOutcome) {
        self.close_timeout_registration();
        // 先切断 WaitContext → Thread，再触碰任一对象锁。
        let thread = self.thread.lock().take();
        self.cleanup();

        if let Some(thread) = thread {
            if !matches!(outcome, WaitOutcome::Abandoned) {
                self.deliver(&thread, outcome);
                sched::enqueue(thread);
            } else {
                // 终止取消：线程永不回用户态，随本上下文消散；
                // 离场确认与 REAPABLE 电平由 process 侧统一发布。
                let process = thread.process.clone();
                let tid = thread.tid;
                drop(thread);
                super::process::confirm_departure(&process, tid);
            }
        }
        self.core.mark_done();
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

/// 为 Commit 后必成的内核事务预构造 Installing context。它不持线程；
/// completion 与 park plan 各持一个强引用，线程所有权只在离开执行点后安装。
pub fn prepare_kernel(value: usize) -> Result<(Arc<WaitContext>, WaitPlan), SystemCallError> {
    let context = WaitContext::new(WaitAction::KernelResult { value }, 0)?;
    let plan = WaitPlan {
        items: Vec::new(),
        action: WaitAction::KernelResult { value },
        expires_at: None,
        prepared: Some(context.clone()),
    };
    Ok((context, plan))
}

/// 调度循环在线程离开执行点后安装一次 WaitMany：Waiting 记录与
/// 可取消性在 lifecycle 锁内线性化；已 Terminating 则不发布等待，
/// 直接以 Abandoned 取消（线程不回用户态）。
pub fn install(thread: Arc<Thread>, mut plan: WaitPlan) {
    let context = match plan.prepared.take() {
        Some(context) => context,
        None => match WaitContext::new(plan.action, plan.items.len()) {
            Ok(context) => context,
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
        let (process, tid) = context_thread_identity(&context);
        if !process.lifecycle.park_waiting(tid, &context) {
            // 终止取得 park 线性化点后，业务 completion 即使已经到达也只
            // 能代表事务完成，不能恢复已经放弃回复权的线程。安装者统一取得
            // Installing 完成权并以 Abandoned 执行 departure confirmation。
            let _ = context.offer(WaitOutcome::Abandoned);
            context
                .core
                .finish_installing()
                .expect("installing owner must finish rejected park");
            context.finish(WaitOutcome::Abandoned);
            return;
        }
    }

    if let Some(expires_at) = plan.expires_at {
        match sched::register_wait_timeout(expires_at, context.clone()) {
            Ok(token) => context.publish_timeout_registration(token),
            Err(()) => {
                context.offer(WaitOutcome::Error(SystemCallError::OutOfMemory));
            }
        }
    }

    for item in plan.items {
        if context.core.has_outcome() {
            break;
        }
        let subscription = Subscription {
            context: context.clone(),
            interest: item.signals,
            cookie: item.cookie,
            item_index: item.index,
        };
        match item.object.subscribe(subscription) {
            super::object::SubscribeResult::Ready(outcome) => {
                context.offer(outcome);
            }
            super::object::SubscribeResult::Registered(id) => {
                context.remember(&item.object, id);
            }
            super::object::SubscribeResult::ReachLimit => {
                context.offer(WaitOutcome::Error(SystemCallError::ReachLimit));
            }
            super::object::SubscribeResult::OutOfMemory => {
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
            .finish_installing()
            .expect("Installing owner must finish an existing outcome");
        context.finish(outcome);
        return;
    }

    match context.core.arm() {
        ArmResult::Armed => {}
        ArmResult::Complete(outcome) => context.finish(outcome),
        ArmResult::ExternalCompleter => {
            // offer 方已取得完成权并负责清理/交付。
        }
    }
}

/// 安装中的上下文必持有发起线程；取其进程引用与 tid 做 lifecycle 线性化。
fn context_thread_identity(
    context: &Arc<WaitContext>,
) -> (
    alloc::sync::Arc<super::proc::Process>,
    erhino_shared::proc::Tid,
) {
    let guard = context.thread.lock();
    let thread = guard.as_ref().expect("installing context holds its thread");
    (thread.process.clone(), thread.tid)
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

fn deliver_install_error(thread: Arc<Thread>, _action: WaitAction, error: SystemCallError) {
    // SAFETY: install 只在线程离开执行点后调用，本 hart 独占尚未发布的现场。
    let frame = unsafe { &mut *thread.frame_ptr() };
    frame.x[10] = error.to_usize().unwrap_or(1) as u64;
    frame.sepc += 4;
    sched::enqueue(thread);
}

/// 对象信号更新在释放对象锁后调用；只有 Complete 方可进入。
pub(crate) fn finish_offered(context: Arc<WaitContext>) {
    let outcome = context.core.outcome();
    context.finish(outcome);
}
