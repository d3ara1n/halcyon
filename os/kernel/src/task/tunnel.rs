//! 隧道机制（契约见 notes/ideas/tunnel.md）：登记表 + 页帧所有权 + 门铃。
//!
//! 职责边界：内核只管「一页零态物理帧、两个端点、一根门铃」，不理解
//! 页内协议（Runnel 规格归 librunnel 与 notes/ideas/runnel.md）。
//!
//! 所有权核心不变量：**帧生命周期 = 双端全亡**。单端死亡（Dispose 或
//! 进程退出）只注销该端记录并给幸存端提交 `PEER_CLOSED` 终态位；幸存端
//! 的映射保留至其自行 Dispose——绝不拆活人映射（惊杀）、绝不提前还帧
//! （UAF）。`FrameTracker` 随登记项存放，双端皆亡时随 Drop 链归还帧池。
//! 进程退出路径的页表回收只清 PTE，不触达本结构的帧——外部映射天然安全。
//!
//! 锁序：TUNNELS 外层 → 单条目锁 → 端点所属线程的 space → 就绪队列。
//! 与 ipc/mailbox 同向，无反向获取者。

use alloc::{sync::Arc, vec::Vec};

use erhino_shared::{call::SystemCallError, proc::Pid, signal::TUNNEL_PEER_CLOSED};

use crate::{
    frame::{self, FrameTracker},
    rand,
    sync::Spinlock,
    task::{ipc, Thread},
};

/// 登记上限：同时存在的隧道条目数。
const TUNNEL_LIMIT: usize = 65536;

/// 单个端点：挂接进程 + 映射地址 + 自身信号状态（终态位 PEER_CLOSED）。
struct Endpoint {
    pid: Pid,
    va: usize,
    sig: ipc::SignalState,
}

/// 一条隧道的登记项。frame 由本结构独占持有，双端皆亡时随 Drop 归还帧池。
pub struct Entry {
    id: u64,
    pa: usize,
    /// 帧所有权载体：从不读取，存在本身即语义——Drop 时归还帧池。
    #[expect(dead_code, reason = "所有权字段，Drop 即归还")]
    frame: FrameTracker,
    ends: [Option<Endpoint>; 2],
}

impl Entry {
    /// 取指定进程端点的信号状态（SignalWait 快路径/发布路径用）。
    pub(crate) fn endpoint_sig_of(&mut self, pid: Pid) -> Option<&mut ipc::SignalState> {
        self.ends.iter_mut().flatten().find(|e| e.pid == pid).map(|e| &mut e.sig)
    }

    fn endpoint_index_of(&self, pid: Pid) -> Option<usize> {
        self.ends.iter().position(|e| e.as_ref().is_some_and(|e| e.pid == pid))
    }
}

static TUNNELS: Spinlock<Vec<Arc<Spinlock<Entry>>>> = Spinlock::new(Vec::new());

/// 按 id 查找登记项（克隆 Arc 后即可释放外层锁）。
pub(crate) fn lookup(id: u64) -> Option<Arc<Spinlock<Entry>>> {
    TUNNELS.lock().iter().find(|e| e.lock().id == id).cloned()
}

/// TunnelCreate(addr)：零态页 + 登记项 + 映射进创建方空间，返回 id。
pub fn create(thread: &Thread, addr: usize) -> Result<u64, SystemCallError> {
    if TUNNELS.lock().len() >= TUNNEL_LIMIT {
        return Err(SystemCallError::ReachLimit);
    }
    let tracker = frame::alloc_contiguous(1).ok_or(SystemCallError::OutOfMemory)?;
    let pa = tracker.base.addr();
    // 帧所有权即刻移交登记项；后续任何失败路径经 entry Drop 还帧。
    let mut entry = Entry { id: 0, pa, frame: tracker, ends: [None, None] };
    // 先映射后登记：映射失败直接还帧返回，不留半成品。
    if let Err(e) = thread.process.space.lock().map_external(addr, pa) {
        drop(entry);
        return Err(match e {
            crate::task::proc::SpaceError::BadSegment => SystemCallError::IllegalArgument,
            _ => SystemCallError::InvalidAddress,
        });
    }
    entry.ends[0] = Some(Endpoint {
        pid: thread.process.pid,
        va: addr,
        sig: ipc::SignalState::with_terminal(TUNNEL_PEER_CLOSED),
    });
    let handle = Arc::new(Spinlock::new(entry));
    // id 抽取 + 查重（48bit 空间碰撞罕见，循环是仪式性保险）。
    let mut tunnels = TUNNELS.lock();
    let id = loop {
        let candidate = rand::next_id48();
        if !tunnels.iter().any(|e| e.lock().id == candidate) {
            handle.lock().id = candidate;
            break candidate;
        }
    };
    tunnels.push(handle);
    Ok(id)
}

/// TunnelAttach(id, addr)：凭 id 挂接第二端点。
pub fn attach(thread: &Thread, id: u64, addr: usize) -> Result<(), SystemCallError> {
    let entry = lookup(id).ok_or(SystemCallError::ObjectNotFound)?;
    let mut e = entry.lock();
    let pid = thread.process.pid;
    if e.endpoint_index_of(pid).is_some() {
        return Err(SystemCallError::ObjectNotAccessible); // 同进程重复挂接
    }
    let slot = e.ends.iter().position(|end| end.is_none()).ok_or(SystemCallError::ReachLimit)?;
    // 先映射后落登记：映射失败则条目未变，无回滚负担。
    thread.process.space.lock().map_external(addr, e.pa).map_err(|err| match err {
        crate::task::proc::SpaceError::BadSegment => SystemCallError::IllegalArgument,
        _ => SystemCallError::InvalidAddress,
    })?;
    e.ends[slot] = Some(Endpoint {
        pid,
        va: addr,
        sig: ipc::SignalState::with_terminal(TUNNEL_PEER_CLOSED),
    });
    Ok(())
}

/// TunnelDispose(id)：主动拆除本端。双端皆亡则还帧。
pub fn dispose(thread: &Thread, id: u64) -> Result<(), SystemCallError> {
    let pid = thread.process.pid;
    let entry = lookup(id).ok_or(SystemCallError::ObjectNotFound)?;
    let mut e = entry.lock();
    let idx = e.endpoint_index_of(pid).ok_or(SystemCallError::ObjectNotFound)?;
    let dead = e.ends[idx].take().expect("index_of 保证 Some");
    // 解除自己空间里的映射（锁序：entry → space，与投递路径同向）。
    thread.process.space.lock().unmap_external(dead.va);
    notify_survivor_or_release(&mut e, idx, &entry);
    Ok(())
}

/// 进程退出清扫（reap 路径调用）：注销该进程全部端点并通知幸存端。
/// 不解除死者映射——其地址空间正随回收链整体拆除，PTE 自灭；
/// 也不在此触达死者空间（此刻可能已被 Drop）。
pub(crate) fn process_died(pid: Pid) {
    let entries: Vec<_> = TUNNELS.lock().clone();
    for entry in entries {
        let mut e = entry.lock();
        let Some(idx) = e.endpoint_index_of(pid) else { continue };
        if e.ends[idx].take().is_some() {
            notify_survivor_or_release(&mut e, idx, &entry);
        }
    }
}

/// 端点注销后的收尾：幸存端获 PEER_CLOSED 终态位；双端皆亡则把条目
/// 从登记表摘除，帧随 Entry Drop 归还帧池。
fn notify_survivor_or_release(e: &mut Entry, gone_idx: usize, handle: &Arc<Spinlock<Entry>>) {
    let survivor_slot = 1 - gone_idx;
    match &mut e.ends[survivor_slot] {
        Some(survivor) => {
            survivor.sig.submit(TUNNEL_PEER_CLOSED, true);
        }
        None => {
            let mut tunnels = TUNNELS.lock();
            if let Some(pos) = tunnels.iter().position(|t| Arc::ptr_eq(t, handle)) {
                tunnels.remove(pos);
            }
        }
    }
}

/// TunnelNotify(id)：本端摇门铃——在对端信号状态上提交 DATA 事件
/// （消费式清除）。对端未挂接或 id 不含本端时报 ObjectNotFound。
pub fn notify(thread: &Thread, id: u64) -> Result<(), SystemCallError> {
    let pid = thread.process.pid;
    let entry = lookup(id).ok_or(SystemCallError::ObjectNotFound)?;
    let mut e = entry.lock();
    let mine = e.endpoint_index_of(pid).ok_or(SystemCallError::ObjectNotFound)?;
    let peer_slot = 1 - mine;
    match &mut e.ends[peer_slot] {
        Some(peer) => {
            peer.sig.submit(erhino_shared::signal::TUNNEL_DATA, true);
            Ok(())
        }
        None => Err(SystemCallError::ObjectNotAvailable),
    }
}
