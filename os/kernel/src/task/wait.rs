//! WaitContext：多对象等待的安装、完成仲裁、订阅清理与结果交付。

use alloc::{sync::{Arc, Weak}, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    wait::{WAIT_MANY_MAX, WaitCookie, WaitItem, WaitReason, WaitResult},
};
use num_traits::ToPrimitive;
use wait_context::{ArmResult, OfferResult, WaitCore};

use crate::{sched, sync::Spinlock, uaccess};

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
    pub deadline: Option<u64>,
}

/// 等待完成后如何写回用户现场。
pub enum WaitStart {
    Ready,
    Park(WaitPlan),
}

/// WaitMany syscall 入口：复制 ABI、解析 Handle/rights，并完成初始检查。
pub fn prepare(
    thread: &Thread,
    items_ptr: usize,
    count: usize,
    result_ptr: usize,
) -> Result<WaitStart, SystemCallError> {
    if count == 0 || count > WAIT_MANY_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let raw_len = count
        .checked_mul(core::mem::size_of::<WaitItem>())
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(raw_len).map_err(|_| SystemCallError::OutOfMemory)?;
    raw.resize(raw_len, 0);
    {
        let mut space = thread.process.space.lock();
        uaccess::copy_from_user(&mut space, &mut raw, items_ptr)?;
        space.check_range(result_ptr, core::mem::size_of::<WaitResult>(), true)?;
    }

    let mut abi_items = Vec::new();
    abi_items.try_reserve_exact(count).map_err(|_| SystemCallError::OutOfMemory)?;
    for bytes in raw.chunks_exact(core::mem::size_of::<WaitItem>()) {
        // SAFETY: WaitItem 仅含整数 newtype；用户缓冲无需对齐。
        abi_items.push(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<WaitItem>()) });
    }

    let mut items = Vec::new();
    items.try_reserve_exact(count).map_err(|_| SystemCallError::OutOfMemory)?;
    {
        let table = thread.process.handles.lock();
        for (index, item) in abi_items.iter().copied().enumerate() {
            if item.reserved != 0 || item.signals == ObjectSignals::NONE || !item.signals.is_known() {
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
                | if closed { ObjectSignals::CLOSED } else { ObjectSignals::NONE };
            let result = WaitResult::new(
                item.cookie,
                observed,
                item.index,
                if closed { WaitReason::Closed } else { WaitReason::Signaled },
            );
            let mut space = thread.process.space.lock();
            // SAFETY: WaitResult 字段和 reserved 全部初始化，结构无 padding。
            unsafe { uaccess::write_user_value(&mut space, result_ptr, &result) }?;
            return Ok(WaitStart::Ready);
        }
    }

    Ok(WaitStart::Park(WaitPlan {
        items,
        action: WaitAction::WaitMany { result_ptr },
        deadline: None,
    }))
}

pub fn sleep_plan(deadline: u64) -> WaitPlan {
    WaitPlan {
        items: Vec::new(),
        action: WaitAction::Sleep,
        deadline: Some(deadline),
    }
}

/// 等待完成后如何写回用户现场。
#[derive(Debug, Clone, Copy)]
pub enum WaitAction {
    WaitMany { result_ptr: usize },
    Sleep,
}

#[derive(Debug, Clone, Copy)]
pub enum WaitOutcome {
    Object(WaitResult),
    Error(SystemCallError),
    Deadline,
    #[expect(dead_code, reason = "显式取消 ABI 接入后使用")]
    Cancelled,
    #[expect(dead_code, reason = "多线程 kill/exit 清理接入后使用")]
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
            | if closed { ObjectSignals::CLOSED } else { ObjectSignals::NONE };
        WaitOutcome::Object(WaitResult::new(
            self.cookie,
            observed,
            self.item_index,
            if closed { WaitReason::Closed } else { WaitReason::Signaled },
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
    action: WaitAction,
}

impl WaitContext {
    fn new(
        thread: Arc<Thread>,
        action: WaitAction,
        registration_capacity: usize,
    ) -> Result<Arc<Self>, Arc<Thread>> {
        let mut registrations = Vec::new();
        if registrations.try_reserve(registration_capacity).is_err() {
            return Err(thread);
        }
        Ok(Arc::new(Self {
            core: WaitCore::new(),
            thread: Spinlock::new(Some(thread)),
            registrations: Spinlock::new(registrations),
            action,
        }))
    }

    pub(crate) fn offer(&self, outcome: WaitOutcome) -> OfferResult {
        self.core.offer(outcome)
    }

    pub(crate) fn expire(self: Arc<Self>) {
        if self.offer(WaitOutcome::Deadline) == OfferResult::Complete {
            finish_offered(self);
        }
    }

    fn remember(&self, object: &ObjectRef, id: u64) {
        let mut registrations = self.registrations.lock();
        debug_assert!(registrations.len() < registrations.capacity());
        registrations.push(Registration { object: Arc::downgrade(object), id });
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
        // 先切断 WaitContext → Thread，再触碰任一对象锁。
        let thread = self.thread.lock().take();
        self.cleanup();

        if let Some(thread) = thread {
            if !matches!(outcome, WaitOutcome::Abandoned) {
                self.deliver(&thread, outcome);
                sched::enqueue(thread);
            }
        }
        self.core.mark_done();
    }

    fn deliver(&self, thread: &Thread, outcome: WaitOutcome) {
        let frame = unsafe { &mut *thread.frame_ptr() };
        match (self.action, outcome) {
            (WaitAction::WaitMany { result_ptr }, WaitOutcome::Object(result)) => {
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
            (WaitAction::WaitMany { .. }, WaitOutcome::Error(error)) => {
                frame.x[10] = error.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            (WaitAction::WaitMany { .. }, WaitOutcome::Cancelled | WaitOutcome::Deadline) => {
                frame.x[10] = SystemCallError::FunctionNotAvailable.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            (WaitAction::Sleep, WaitOutcome::Deadline) => {
                frame.x[10] = 0;
                frame.x[11] = 0;
                frame.sepc += 4;
            }
            (WaitAction::Sleep, WaitOutcome::Error(error)) => {
                frame.x[10] = error.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
            (_, WaitOutcome::Abandoned) => unreachable!("abandoned waits are never delivered"),
            (WaitAction::Sleep, WaitOutcome::Object(_) | WaitOutcome::Cancelled) => {
                frame.x[10] = SystemCallError::InternalError.to_usize().unwrap_or(1) as u64;
                frame.sepc += 4;
            }
        }
    }
}

/// 调度循环在线程离开执行点后安装一次 WaitMany。
pub fn install(thread: Arc<Thread>, plan: WaitPlan) {
    let context = match WaitContext::new(thread, plan.action, plan.items.len()) {
        Ok(context) => context,
        Err(thread) => {
            deliver_install_error(thread, plan.action, SystemCallError::OutOfMemory);
            return;
        }
    };

    if let Some(deadline) = plan.deadline {
        if sched::register_wait_deadline(deadline, context.clone()).is_err() {
            context.offer(WaitOutcome::Error(SystemCallError::OutOfMemory));
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
