//! 单来源等待：成员锁只仲裁 epoch/pending/执行容器，执行 owner 复检并唯一 detach。

use alloc::sync::Arc;
use core::pin::Pin;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
    wait::WaitOneRequest,
};

use super::{
    Thread,
    process::members::ParkTransition,
    wait_source::{Hint, HintTarget, ObservationSource},
};
use crate::{
    sched::{self, AdmittedThread},
    uaccess,
};

#[derive(Clone, Copy)]
enum Return {
    WaitResult,
    KernelCompletion,
    RetryFault { cause: usize, address: usize },
}

pub struct WaitPlan {
    source: Option<Arc<dyn ObservationSource>>,
    interest: ObjectSignals,
    expires_at: Option<u64>,
    returns: Return,
}
impl WaitPlan {
    /// 内核 Work 消费同一完成来源；节点仍由实际等待 owner 持有与摘除。
    pub(crate) fn into_completion(self) -> Arc<dyn ObservationSource> {
        assert!(
            matches!(self.returns, Return::KernelCompletion),
            "non-completion plan used by kernel work"
        );
        self.source.expect("completion source missing")
    }

    pub(crate) fn completion(source: Arc<super::wait_source::SignalSource>) -> Self {
        Self {
            source: Some(source),
            interest: ObjectSignals::DONE,
            expires_at: None,
            returns: Return::KernelCompletion,
        }
    }

    pub(crate) fn retry_fault(
        source: Arc<dyn ObservationSource>,
        cause: usize,
        address: usize,
    ) -> Self {
        Self {
            source: Some(source),
            interest: ObjectSignals::DONE,
            expires_at: None,
            returns: Return::RetryFault { cause, address },
        }
    }
}

pub enum WaitStart {
    Ready(ObjectSignals),
    Park(WaitPlan),
}

/// Thread 出生预付节点；本结构只保存一轮参数，不另分配等待对象。
pub(crate) struct Entry {
    epoch: u64,
    source: Option<Arc<dyn ObservationSource>>,
    interest: ObjectSignals,
    expires_at: Option<u64>,
    source_attached: bool,
    timer_prepared: bool,
    returns: Return,
}

impl Drop for Entry {
    fn drop(&mut self) {
        assert!(
            !self.source_attached && !self.timer_prepared,
            "wait entry dropped before detach"
        );
    }
}

/// 单对象 ABI 返回当前观察电平；NONE 表示期限到达，CLOSED 位说明来源关闭。
/// 表锁只解析存储引用，观察借用和来源读取在表锁外。
pub fn prepare(thread: &Thread, request_ptr: usize) -> Result<WaitStart, SystemCallError> {
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let request: WaitOneRequest = {
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        // SAFETY: ABI 仅固定宽整数/整数 newtype，任意位型有效，随后验证语义。
        unsafe { uaccess::read_user_value(space, request_ptr) }?
    };
    if request.signals == ObjectSignals::NONE || !request.signals.is_known() {
        return Err(SystemCallError::IllegalArgument);
    }
    let object = {
        let table = body.handles.lock();
        table
            .get(request.handle, Rights::WAIT)
            .map_err(super::handle::map_error)?
            .object()
            .0
            .clone()
    };
    drop(body);
    let allowed = object
        .allowed_signals()
        .ok_or(SystemCallError::WrongObjectType)?;
    if request.signals.raw() & !allowed.raw() != 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let source = object
        .wait_source()
        .ok_or(SystemCallError::WrongObjectType)?;
    let plan = WaitPlan {
        source: Some(source),
        interest: request.signals,
        expires_at: crate::clock::deadline_ticks(request.deadline)?,
        returns: Return::WaitResult,
    };
    match probe(plan.source.as_deref(), plan.interest, plan.expires_at)? {
        Some(signals) => Ok(WaitStart::Ready(signals)),
        None => Ok(WaitStart::Park(plan)),
    }
}

pub fn sleep_plan(deadline: Deadline) -> Result<WaitPlan, SystemCallError> {
    Ok(WaitPlan {
        source: None,
        interest: ObjectSignals::NONE,
        expires_at: Some(
            crate::clock::deadline_ticks(deadline)?.ok_or(SystemCallError::IllegalArgument)?,
        ),
        returns: Return::WaitResult,
    })
}

/// 每次执行只取一次来源快照和一次时钟；不锁存曾经发生的电平。
fn probe(
    source: Option<&dyn ObservationSource>,
    interest: ObjectSignals,
    expires_at: Option<u64>,
) -> Result<Option<ObjectSignals>, SystemCallError> {
    if let Some(source) = source {
        let snapshot = source.snapshot();
        let observed = snapshot.signals & interest;
        if snapshot.closed {
            return Ok(Some(observed | ObjectSignals::CLOSED));
        }
        if observed != ObjectSignals::NONE {
            return Ok(Some(observed));
        }
    }
    if let Some(expires) = expires_at
        && crate::clock::now_ticks()? >= expires
    {
        return Ok(Some(ObjectSignals::NONE));
    }
    Ok(None)
}

/// # Safety 的 owner 论证集中于此：调用者持不可复制的调度资格，来源提示与
/// timer 只在成员锁中操作 pending，不访问此续体；发布资格后不得再使用借用。
#[allow(clippy::mut_from_ref)] // 调度资格唯一；普通 Thread/提示引用不能访问此 cell。
fn entry(owner: &AdmittedThread) -> &mut Option<Entry> {
    // SAFETY: 由函数参数持有唯一执行 owner，返回引用不能跨它的容器移交。
    unsafe { &mut *owner.wait_entry.get() }
}

pub(crate) fn has_entry(owner: &AdmittedThread) -> bool {
    entry(owner).is_some()
}

/// 完全离开 VM 后安装；节点及 timer 都由出生存储承载，登记阶段不分配。
pub(crate) fn install(owner: AdmittedThread, plan: WaitPlan) {
    let slot = crate::hart::current().slot();
    let epoch = owner
        .process
        .lease_receipt()
        .and_then(|body| body.members.begin_wait(owner.member(), slot));
    let Some(epoch) = epoch else {
        if owner.process.is_frozen() {
            sched::reap(owner);
        } else {
            set_result(&owner, Err(SystemCallError::ReachLimit));
            sched::requeue(owner, slot);
        }
        return;
    };
    assert!(
        entry(&owner).is_none(),
        "thread installed a second continuation"
    );
    *entry(&owner) = Some(Entry {
        epoch,
        source: plan.source,
        interest: plan.interest,
        expires_at: plan.expires_at,
        source_attached: false,
        timer_prepared: false,
        returns: plan.returns,
    });
    let state = entry(&owner)
        .as_mut()
        .expect("installed wait entry vanished");
    if let Some(source) = state.source.as_ref() {
        let target: Arc<dyn HintTarget> = Arc::<Thread>::clone(&owner);
        // SAFETY: Thread Arc/出生节点稳定，Entry 强持来源直至唯一 detach；目标只存 Weak。
        unsafe {
            source.attach(
                Pin::new_unchecked(&owner.wait_node),
                state.interest,
                Hint::new(Arc::downgrade(&target), epoch),
            );
        }
        state.source_attached = true;
    }
    if let Some(expires) = state.expires_at {
        sched::register_wait_timeout(expires, Arc::<Thread>::clone(&owner), epoch);
        state.timer_prepared = true;
    }
    // 登记后复检，覆盖此前已经 ready 的来源以及登记窗口内的到期。
    if let Some(owner) = resume(owner, slot) {
        sched::requeue(owner, slot);
    }
}

/// 新一次执行资格先处理续体；未完成者只停驻/入队一次，不原地循环。
pub(crate) fn resume(owner: AdmittedThread, slot: usize) -> Option<AdmittedThread> {
    if owner.process.is_frozen() {
        sched::reap(owner);
        return None;
    }
    let state = entry(&owner)
        .as_ref()
        .expect("wait resume without continuation");
    let epoch = state.epoch;
    {
        let body = owner
            .process
            .lease_receipt()
            .expect("wait owner lost member receipt");
        body.members.take_pending(owner.member(), epoch);
    }
    let result = probe(state.source.as_deref(), state.interest, state.expires_at);
    let returns = state.returns;
    match result {
        Ok(None) => {
            let shell = owner.process.clone();
            let body = shell
                .lease_receipt()
                .expect("park owner lost member receipt");
            let action = body.members.park(&shell, owner, slot, epoch);
            drop(body);
            match action {
                ParkTransition::Parked => {}
                ParkTransition::Publish(owner, ingress) => ingress.publish_admitted(owner),
                ParkTransition::Exit(owner) => sched::reap(owner),
            }
            None
        }
        result => {
            detach(&owner);
            let result = result.map(|signals| signals.expect("completed probe lost result"));
            match returns {
                Return::WaitResult => set_result(&owner, result),
                Return::KernelCompletion => {
                    set_result(&owner, result);
                    // SAFETY: 当前执行owner独占现场，内部完成返回单位值。
                    unsafe { &mut *owner.frame_ptr() }.x[11] = 0;
                }
                Return::RetryFault { cause, address } => {
                    if result.is_err() {
                        owner.process.freeze_and_seal(
                            erhino_shared::proc::TerminationCause::Faulted,
                            erhino_shared::proc::TerminationDetail::fault(
                                owner.member().tid(),
                                erhino_shared::proc::ProcessFaultCode::from_scause(cause),
                                address as u64,
                            ),
                        );
                    }
                    // 原指令续体：不改sepc，不伪造syscall返回寄存器。
                }
            }
            Some(owner)
        }
    }
}

/// 正常完成和 Frozen 退出共用：timer → 来源节点 → 观察 loan → epoch/pending。
/// 不写用户结果，不请求 Departure；调用方仍持执行 owner。
pub(crate) fn detach(owner: &AdmittedThread) {
    let Some(mut state) = entry(owner).take() else {
        return;
    };
    if state.timer_prepared {
        sched::unregister_wait_timeout(owner, state.epoch);
        state.timer_prepared = false;
    }
    if state.source_attached {
        let source = state
            .source
            .as_ref()
            .expect("attached wait lost source loan");
        // SAFETY: 来源/节点仍稳定存活，唯一执行 owner 仲裁本轮摘除；返回后无节点借用。
        drop(unsafe { source.detach(Pin::new_unchecked(&owner.wait_node)) });
        state.source_attached = false;
    }
    drop(state.source.take());
    let body = owner
        .process
        .lease_receipt()
        .expect("detach lost member receipt");
    body.members.end_wait(owner.member(), state.epoch);
}

fn set_result(owner: &AdmittedThread, result: Result<ObjectSignals, SystemCallError>) {
    // SAFETY: 只由执行 owner 在内核页表下修改自己的保存现场；提示方不访问现场。
    let frame = unsafe { &mut *owner.frame_ptr() };
    // 唯一正常完成出口推进 ecall；未就绪的续体不改 PC，退出 detach 也不返回用户。
    frame.sepc += 4;
    match result {
        Ok(signals) => {
            frame.x[10] = SystemCallError::NoError as u64;
            frame.x[11] = signals.raw();
        }
        Err(error) => {
            frame.x[10] = error as u64;
            frame.x[11] = 0;
        }
    }
}
