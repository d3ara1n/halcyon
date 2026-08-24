//! IPC 原语：消息邮箱与对象信号状态（契约见 notes/ideas/message.md、signal.md）。
//!
//! 结构对应契约的三块：
//! - [`Mailbox`]：有界 FIFO 邮箱，投递永不阻塞，满箱即 `MailboxFull`；
//! - [`SignalState`]：对象的信号状态 = 粘滞余量 + 等待者队列（FIFO）；
//! - 等待发布路径（`publish_*`）：syscall 处理器只登记意图，真正的
//!   入队由调度循环在线程离开执行点后完成（时序纪律见 sched::park_publish）。
//!
//! 位义不变量：邮箱的 `NONEMPTY ⇔ 队列非空` 由本模块全部变更点维护，
//! 它是内核托管位——SignalWait 命中它**不清除**（队列排空才清）；进程级
//! 信号位是纯粘滞位，SignalWait 命中即原子测试并清除。终态位（隧道
//! `PEER_CLOSED` 类）随 tunnel 里程碑引入。
//!
//! 锁序：mailbox/signals → 目标线程的 space → 就绪队列（sched::enqueue）。
//! 全系统无反向获取者。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};

use erhino_shared::{
    call::SystemCallError,
    message::{MessageDigest, MAILBOX_CAPACITY, PAYLOAD_MAX},
    proc::Pid,
    signal::{NONEMPTY, SIGNAL_WAIT_MAX},
};

use crate::{
    sched,
    sync::Spinlock,
    task::{table, Thread},
    uaccess,
};

/// Pid=0 的内核邮箱（契约「内核作为消息终点」）：只进不出，内核侧
/// 消费者出现前在此积压（有界，满箱照常拒绝）。
static KERNEL_MAILBOX: Spinlock<Mailbox> = Spinlock::new(Mailbox::new());

/// 一条在途消息。负载在发送侧拷入内核堆、接收侧拷出，双方内存解耦。
pub(crate) struct Message {
    sender: Pid,
    kind: usize,
    payload: Vec<u8>,
}

/// 对象信号的等待请求：阻塞在对象上的线程要什么。
pub(crate) enum WaitRequest {
    /// Receive 阻塞形态：等队头消息，到达后拷入 (buf, len)。
    Recv { buf: usize, len: usize },
    /// SignalWait 的一个等待项：关注位集 + 项下标（唤醒回填用）。
    Signal { index: usize, interest: u64 },
}

/// 等待条目：强持有等待中的线程（引用随容器走，见 task.md「单一归属
/// 不变量」）；generation 是单次完成仲裁凭据——多源等待时同一线程的
/// 条目驻留多个对象队列，恰有一个经代数仲裁交付，其余为惰性丢弃的死条目。
pub(crate) struct ObjectWaiter {
    thread: Arc<Thread>,
    generation: u64,
    request: WaitRequest,
}

/// 对象的信号状态：粘滞余量 + FIFO 等待者队列（契约「状态机」节）。
///
/// 到达规则：命中首个匹配等待者则定向移交；无人匹配则按位或并入粘滞
/// 余量。公平性由队列结构保证，内核不做挑选。
pub struct SignalState {
    mask: u64,
    waiters: VecDeque<ObjectWaiter>,
}

impl SignalState {
    pub const fn new() -> Self {
        Self { mask: 0, waiters: VecDeque::new() }
    }

    /// 提交一个事件：置位 + 定向唤醒首个匹配的 Signal 等待者。
    ///
    /// `clear_on_deliver` 决定移交成功后是否从粘滞余量清除已交付的位：
    /// 进程级信号位是消费式（命中即清），邮箱 NONEMPTY 是内核托管位
    /// （随队列生灭，绝不在消费点清除）。返回是否有人被唤醒。
    fn submit(&mut self, bits: u64, clear_on_deliver: bool) -> bool {
        self.mask |= bits;
        let pos = self.waiters.iter().position(
            |w| matches!(&w.request, WaitRequest::Signal { interest, .. } if interest & bits != 0),
        );
        let Some(i) = pos else { return false };
        let w = self.waiters.remove(i).unwrap();
        let ObjectWaiter { thread, generation, request } = w;
        let (index, interest) = match &request {
            WaitRequest::Signal { index, interest } => (*index, *interest),
            _ => unreachable!("position() matched Signal variant"),
        };
        let delivered = interest & bits;
        // 代数仲裁失败 = 死条目（多源等待已在别处完成/取消），事件留在
        // 粘滞余量里等后来者，语义不丢。
        let woken = sched::fulfill(thread, generation, move |f| {
            f.x[10] = 0;
            f.x[11] = (((index as u64) << 56) | delivered) as u64;
            f.sepc += 4;
        });
        if woken && clear_on_deliver {
            self.mask &= !delivered;
        }
        woken
    }

    /// 注册一个等待条目（发布路径专用：调用方持对象锁、已做双检）。
    fn enroll(&mut self, waiter: ObjectWaiter) {
        self.waiters.push_back(waiter);
    }
}

/// 邮箱：有界 FIFO 消息队列 + 自身信号状态。
pub struct Mailbox {
    queue: VecDeque<Message>,
    sig: SignalState,
}

impl Mailbox {
    pub const fn new() -> Self {
        Self { queue: VecDeque::new(), sig: SignalState::new() }
    }

    /// NONEMPTY 内核托管不变量的唯一维护点：随队列状态同步粘滞位。
    /// 只改位不触发唤醒（唤醒由到达路径显式 submit 完成）。
    fn sync_nonempty(&mut self) {
        if self.queue.is_empty() {
            self.sig.mask &= !NONEMPTY;
        } else {
            self.sig.mask |= NONEMPTY;
        }
    }

    /// 投递一条消息（调用方持邮箱锁）：
    /// 1) 队首起扫描 Recv 等待者直接移交（缓冲不足/失效者就地得到错误，
    ///    继续尝试后继——消息不落地）；
    /// 2) 无接手者则入队（满则 MailboxFull）；
    /// 3) 提交 NONEMPTY 事件给 Signal 等待者。
    fn deliver(&mut self, msg: Message) -> Result<(), SystemCallError> {
        while let Some(front) = self.sig.waiters.front() {
            if !matches!(front.request, WaitRequest::Recv { .. }) {
                break; // Signal 等待者不接手载荷，消息走队列
            }
            let w = self.sig.waiters.pop_front().unwrap();
            let ObjectWaiter { thread, generation, request } = w;
            let (buf, len) = match &request {
                WaitRequest::Recv { buf, len } => (*buf, *len),
                _ => unreachable!("front() matched Recv variant"),
            };
            if len < msg.payload.len() {
                sched::respond_error_to(thread, generation, SystemCallError::IllegalArgument);
                continue;
            }
            let mut space = thread.process.space.lock();
            let copied = uaccess::put_user_indirect(&mut space, buf, &msg.payload);
            drop(space);
            match copied {
                Ok(()) => {
                    let n = msg.payload.len();
                    sched::fulfill(thread, generation, move |f| {
                        f.x[10] = 0;
                        f.x[11] = n as u64;
                        f.sepc += 4;
                    });
                    return Ok(());
                }
                Err(_) => {
                    // 缓冲在阻塞期间失效（理论不可达：单线程进程无法
                    // 边等待边改映射）；按访问错误交付，继续尝试后继。
                    sched::respond_error_to(thread, generation, SystemCallError::MemoryNotAccessible);
                }
            }
        }
        if self.queue.len() >= MAILBOX_CAPACITY {
            return Err(SystemCallError::MailboxFull);
        }
        self.queue.push_back(msg);
        self.sync_nonempty();
        self.sig.submit(NONEMPTY, false);
        Ok(())
    }

    /// 取队头消息（调用方持邮箱锁）；取空返回 None 并同步 NONEMPTY。
    fn take(&mut self) -> Option<Message> {
        let msg = self.queue.pop_front()?;
        self.sync_nonempty();
        Some(msg)
    }
}

// ---------------------------------------------------------------------------
// syscall 处理入口（syscall::dispatch 调用）
// ---------------------------------------------------------------------------

/// Send(target, kind, buf, len)：拷负载 → 解析目标 → 投递。永不阻塞。
pub fn send(
    thread: &Thread,
    buf: usize,
    len: usize,
    target: Pid,
    kind: usize,
) -> Result<usize, SystemCallError> {
    if len > PAYLOAD_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut payload = alloc::vec![0u8; len];
    {
        let mut space = thread.process.space.lock();
        uaccess::copy_from_user(&mut space, &mut payload, buf)?;
    }
    let message = Message { sender: thread.process.pid, kind, payload };
    if target == 0 {
        return KERNEL_MAILBOX
            .lock()
            .deliver(message)
            .map(|_| len);
    }
    match table::get(target) {
        Some(proc) => proc.mailbox.lock().deliver(message).map(|_| len),
        None => Err(SystemCallError::ObjectNotFound),
    }
}

/// Peek(digest_buf)：非阻塞检查自身邮箱队头。不改邮箱状态。
pub fn peek(thread: &Thread, digest_buf: usize) -> Result<usize, SystemCallError> {
    let mb = thread.process.mailbox.lock();
    let Some(msg) = mb.queue.front() else {
        return Err(SystemCallError::ObjectNotAvailable);
    };
    let digest = MessageDigest::new(msg.sender, msg.kind, msg.payload.len());
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&digest as *const MessageDigest) as *const u8,
            core::mem::size_of::<MessageDigest>(),
        )
    };
    let mut space = thread.process.space.lock();
    uaccess::copy_to_user(&mut space, digest_buf, bytes)?;
    Ok(digest.payload_length)
}

/// Discard()：抛弃队头消息。
pub fn discard(thread: &Thread) -> Result<(), SystemCallError> {
    let mut mb = thread.process.mailbox.lock();
    match mb.take() {
        Some(_) => Ok(()),
        None => Err(SystemCallError::ObjectNotAvailable),
    }
}

/// Receive 的即时结果：Done = 已完成（含错误）；Block = 转阻塞登记。
pub enum RecvOutcome {
    Done(Result<usize, SystemCallError>),
    Block,
}

/// Receive(buf, len)：队头有消息则立即取（SUM 直访拷出）；空箱返回
/// Block，由调度循环发布为邮箱等待者。
pub fn receive(thread: &Thread, buf: usize, len: usize) -> RecvOutcome {
    if len == 0 {
        return RecvOutcome::Done(Err(SystemCallError::IllegalArgument));
    }
    let mut mb = thread.process.mailbox.lock();
    let Some(msg) = mb.take() else {
        drop(mb);
        return RecvOutcome::Block;
    };
    let n = msg.payload.len();
    if len < n {
        return RecvOutcome::Done(Err(SystemCallError::IllegalArgument));
    }
    let mut space = thread.process.space.lock();
    match uaccess::copy_to_user(&mut space, buf, &msg.payload) {
        Ok(()) => RecvOutcome::Done(Ok(n)),
        Err(e) => RecvOutcome::Done(Err(e.into())),
    }
}

/// SignalSend(pid, mask)：向目标进程的信号状态提交事件。返回是否有人
/// 被移交唤醒（false = 已并入粘滞余量）。
pub fn signal_send(_thread: &Thread, pid: Pid, mask: u64) -> Result<bool, SystemCallError> {
    if mask == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    if pid == 0 {
        // 内核信号状态暂无对外语义（无消费者），先按不存在处理。
        return Err(SystemCallError::ObjectNotFound);
    }
    match table::get(pid) {
        Some(proc) => Ok(proc.signals.lock().submit(mask, true)),
        None => Err(SystemCallError::ObjectNotFound),
    }
}

/// 可等待对象的内核侧引用（用户态以 [`erhino_shared::signal::ObjectKind`]
/// 表达，分发期解析为本枚举；隧道端点随 tunnel 里程碑加入）。
#[derive(Clone, Copy, Debug)]
pub enum SigTarget {
    Process,
    Mailbox,
}

/// SignalWait 的分流结果。
pub enum WaitPlan {
    /// 即时命中：某关注位已置，直接完成。
    Now { index: usize, bits: u64 },
    /// 无命中：携带目标表转阻塞登记。
    Park(Vec<(SigTarget, u64)>),
}

/// SignalWait(items, count)：解析等待项 → 快路径查各对象粘滞余量 →
/// 无命中则交出目标表供调度循环登记。
pub fn signal_wait(thread: &Thread, items_ptr: usize, count: usize) -> Result<WaitPlan, SystemCallError> {
    if count == 0 || count > SIGNAL_WAIT_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut raw = alloc::vec![0u8; count * core::mem::size_of::<erhino_shared::signal::SignalItem>()];
    {
        let mut space = thread.process.space.lock();
        uaccess::copy_from_user(&mut space, &mut raw, items_ptr)?;
    }
    let items: Vec<erhino_shared::signal::SignalItem> = raw
        .chunks_exact(core::mem::size_of::<erhino_shared::signal::SignalItem>())
        .map(|c| unsafe {
            // 用户缓冲不保证对齐，用 unaligned 读。
            core::ptr::read_unaligned(c.as_ptr() as *const _)
        })
        .collect();

    let mut plan: Vec<(SigTarget, u64)> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let interest = item.interest;
        if interest == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let target = match item.kind {
            0 => SigTarget::Process,
            1 => SigTarget::Mailbox,
            _ => return Err(SystemCallError::NotSupported), // 隧道端点随 tunnel 里程碑接入
        };
        // 快路径：粘滞余量已命中则即时完成。进程位消费式清除；邮箱位
        // 是内核托管位，只报告不清除（队列排空时由 take/discard 清）。
        let hit = match target {
            SigTarget::Process => {
                let mut sig = thread.process.signals.lock();
                if sig.mask & interest != 0 {
                    let bits = sig.mask & interest;
                    sig.mask &= !interest;
                    Some(bits)
                } else {
                    None
                }
            }
            SigTarget::Mailbox => {
                let mb = thread.process.mailbox.lock();
                (mb.sig.mask & interest != 0).then_some(mb.sig.mask & interest)
            }
        };
        if let Some(bits) = hit {
            return Ok(WaitPlan::Now { index, bits });
        }
        plan.push((target, interest));
    }
    Ok(WaitPlan::Park(plan))
}

// ---------------------------------------------------------------------------
// 发布路径（sched::park_publish 在线程离开执行点后调用）
// ---------------------------------------------------------------------------

/// 发布 Receive 阻塞：双检后注册为邮箱等待者。
pub(crate) fn publish_recv(t: &Arc<Thread>, buf: usize, len: usize) {
    let generation = t.wait_gen.fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
    let mut mb = t.process.mailbox.lock();
    // 双检：登记窗口内可能有消息到达（粘滞位/队列已就绪）。
    if let Some(msg) = mb.take() {
        drop(mb);
        let code_and_len: Result<usize, SystemCallError> = if len < msg.payload.len() {
            Err(SystemCallError::IllegalArgument)
        } else {
            let n = msg.payload.len();
            let mut space = t.process.space.lock();
            let copied = uaccess::put_user_indirect(&mut space, buf, &msg.payload);
            drop(space);
            copied.map(|_| n).map_err(SystemCallError::from)
        };
        // SAFETY: gen 已消费 ⇒ 本 hart 独占该 Waiting 线程的帧；先写帧
        // 后入队（入队即可能被其他 hart 拾取）。
        let frame = unsafe { &mut *t.frame_ptr() };
        match code_and_len {
            Ok(n) => {
                frame.x[10] = 0;
                frame.x[11] = n as u64;
            }
            Err(e) => {
                frame.x[10] = num_traits::ToPrimitive::to_usize(&e).unwrap_or(1) as u64;
            }
        }
        frame.sepc += 4;
        sched::enqueue(t.clone());
        return;
    }
    mb.sig.enroll(ObjectWaiter {
        thread: t.clone(),
        generation,
        request: WaitRequest::Recv { buf, len },
    });
}

/// 发布 SignalWait 阻塞：逐对象双检 + 注册；任一即时命中则作废已推送
/// 条目（再进一代数）并直接完成。
pub(crate) fn publish_signal_wait(t: &Arc<Thread>, targets: Vec<(SigTarget, u64)>) {
    let generation = t.wait_gen.fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
    for (index, (target, interest)) in targets.iter().enumerate() {
        let hit = match target {
            SigTarget::Process => {
                let mut sig = t.process.signals.lock();
                if sig.mask & interest != 0 {
                    let bits = sig.mask & interest;
                    sig.mask &= !interest;
                    Some(bits)
                } else {
                    sig.enroll(ObjectWaiter {
                        thread: t.clone(),
                        generation,
                        request: WaitRequest::Signal { index, interest: *interest },
                    });
                    None
                }
            }
            SigTarget::Mailbox => {
                let mut mb = t.process.mailbox.lock();
                if mb.sig.mask & interest != 0 {
                    Some(mb.sig.mask & interest) // 内核托管位：报告不清除
                } else {
                    mb.sig.enroll(ObjectWaiter {
                        thread: t.clone(),
                        generation,
                        request: WaitRequest::Signal { index, interest: *interest },
                    });
                    None
                }
            }
        };
        if let Some(bits) = hit {
            // 作废本轮已推送的同代条目：代数再进一步，它们成为死条目，
            // 由各自队列下次触碰时惰性清扫。
            t.wait_gen.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            // SAFETY: 线程处于 Park 分支，本 hart 独占其帧；先写帧后入队。
            let frame = unsafe { &mut *t.frame_ptr() };
            frame.x[10] = 0;
            frame.x[11] = (((index as u64) << 56) | bits) as u64;
            frame.sepc += 4;
            sched::enqueue(t.clone());
            return;
        }
    }
}
