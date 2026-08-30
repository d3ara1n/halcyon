//! 进程与线程：资源容器 / 执行容器（见 notes/impls/task.md）。

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    proc::{Pid, ProcessMapFlags, ProcessState, ThreadStartContext, Tid},
};
use page_table::{
    FrameMemory, FrameNumber, MapError, Ppn, ReservedTableFrame, RootSlotState, SlotState,
    TableTree, Vpn, flags,
};

use crate::{
    context::UserContext,
    frame::{self, FrameTracker},
    mm, sbi,
};

/// 页大小（字节）。
pub const PAGE_SIZE: usize = erhino_shared::proc::PROCESS_PAGE_SIZE;
const _: () = assert!(PAGE_SIZE == 1 << page_table::PAGE_BITS);

/// 用户半区顶（256GiB），主线程栈顶。
pub const USER_TOP: usize = erhino_shared::proc::PROCESS_USER_TOP;

/// 主线程栈大小（8MiB），钉在半区顶。
pub const STACK_SIZE: usize = erhino_shared::proc::PROCESS_MAIN_STACK_SIZE;

/// sv39 三级页表。
const LEVELS: usize = 3;

/// 进程地址空间构建/操作错误。
#[derive(Debug)]
pub enum SpaceError {
    /// 帧或表帧耗尽。
    NoFrame,
    /// 段未页对齐 / 参数非法。
    BadSegment,
    /// 映射冲突（重复装载同一区间）。
    Conflict,
}

#[derive(Debug)]
pub(crate) enum ThreadAttachError {
    Context(SpaceError),
    Closed,
    Limit,
    Oom,
}

impl From<MapError> for SpaceError {
    fn from(e: MapError) -> Self {
        match e {
            MapError::Conflict { .. } => SpaceError::Conflict,
            MapError::FrameExhausted | MapError::AllocationFailed => SpaceError::NoFrame,
            MapError::OutOfRange
            | MapError::InvalidFlags
            | MapError::NotMapped { .. }
            | MapError::ProtectionMismatch { .. } => SpaceError::BadSegment,
        }
    }
}

struct TableFrameToken(FrameTracker);

impl ReservedTableFrame for TableFrameToken {
    fn number(&self) -> FrameNumber {
        FrameNumber(self.0.base().addr() / PAGE_SIZE)
    }

    fn commit(self) -> FrameNumber {
        self.0.into_table_frame()
    }
}

/// [`TableTree`] 的帧来源：表帧通过显式 transfer 交树持有，树 Drop 时归还。
struct TableMem;

impl FrameMemory for TableMem {
    type ReservedFrame = TableFrameToken;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, page_table::FrameExhausted> {
        frame::alloc_order(0)
            .map(TableFrameToken)
            .ok_or(page_table::FrameExhausted)
    }

    fn free_frame(&mut self, frame: FrameNumber) {
        // SAFETY: FrameMemory 只回传此前由 alloc_frame 唯一移交给该树的表帧。
        drop(unsafe { FrameTracker::adopt_table_frame(frame) });
    }

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; page_table::ENTRIES] {
        // SAFETY: 表帧来自帧池（页对齐、已清零），经直映射访问。
        unsafe { &mut *(mm::phys_to_virt(frame.addr()) as *mut _) }
    }
}

/// 有界收束游标（REAPABLE 后由管理者分批驱动；见 lifecycle 模块）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainStage {
    /// 未进入收束（进程尚活）。
    Idle,
    /// 逐个归还 owned 数据帧 tracker。
    Frames,
    /// 逐批回收用户页表子树（L0/L1 表帧）。
    Tables { root: usize, l1: usize },
    /// 全部子表已空：逐项验证 root 512 槽后交出 root 帧。
    Root { slot: usize },
    /// root 帧已交出、有界归还在途（tree 已 None）。
    RootFree,
    /// 资源全空（root 已释放）；仅剩空壳。
    Done,
}

/// 地址空间稳定 epoch 快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EpochSnapshot {
    pub translation: u64,
    pub instruction: u64,
}

static NEXT_ADDRESS_SPACE_ID: AtomicUsize = AtomicUsize::new(1);

/// 进程地址空间的稳定外壳。identity 与 epoch 不随 ledger/页表状态锁借用而移动，
/// Remote Call 和 execution gate 可在不复制 active 集合的前提下引用它们。
pub struct AddressSpace {
    identity: usize,
    translation_epoch: AtomicU64,
    instruction_epoch: AtomicU64,
    state: crate::sync::Spinlock<AddressSpaceState>,
}

static SHOOTDOWN_SELFTEST_STARTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

struct ShootdownSelfTestCompletion;

impl crate::remote_call::Completion for ShootdownSelfTestCompletion {
    fn complete(&self) {
        log!(
            Memory,
            "epoch self-test passed: active snapshot and shootdown acknowledged"
        );
    }
}

/// Commit 前持有 execution snapshot、全部目标槽与完成引用。
pub(crate) struct PreparedShootdown {
    execution: super::lifecycle::ExecutionSnapshot,
    remote: Option<crate::remote_call::ReservedBatch>,
    immediate: Option<Arc<dyn crate::remote_call::Completion>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrepareShootdownError {
    NotRunning,
    Busy,
    InvalidTargets,
    OutOfMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShootdownChanged;

/// Commit 后唯一允许的推进：锁外敲门铃，或在目标集为空时直接完成。
#[must_use = "committed shootdown must start synchronization after releasing business locks"]
pub(crate) enum ShootdownSynchronization {
    Remote(crate::remote_call::Doorbell),
    Immediate(Arc<dyn crate::remote_call::Completion>),
}

impl ShootdownSynchronization {
    pub(crate) fn start(self) {
        match self {
            Self::Remote(doorbell) => doorbell.ring(),
            Self::Immediate(completion) => completion.complete(),
        }
    }
}

impl AddressSpace {
    pub fn new() -> Result<Self, SpaceError> {
        let identity = NEXT_ADDRESS_SPACE_ID.fetch_add(1, Ordering::Relaxed);
        assert!(
            identity != 0 && identity != usize::MAX,
            "address-space identity exhausted"
        );
        Ok(Self {
            identity,
            translation_epoch: AtomicU64::new(1),
            instruction_epoch: AtomicU64::new(1),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::ADDRESS_SPACE,
                AddressSpaceState::new()?,
            ),
        })
    }

    pub fn lock(&self) -> crate::sync::SpinlockGuard<'_, AddressSpaceState> {
        self.state.lock()
    }

    pub(crate) fn epochs(&self) -> EpochSnapshot {
        EpochSnapshot {
            translation: self.translation_epoch.load(Ordering::Acquire),
            instruction: self.instruction_epoch.load(Ordering::Acquire),
        }
    }

    pub(crate) fn synchronize_local(&self) -> EpochSnapshot {
        let epochs = self.epochs();
        crate::remote_call::synchronize_local(
            self.identity,
            epochs.translation,
            epochs.instruction,
        );
        epochs
    }

    pub(crate) fn local_is_current(&self, expected: EpochSnapshot) -> bool {
        self.epochs() == expected
            && crate::remote_call::local_observes(
                self.identity,
                expected.translation,
                expected.instruction,
            )
    }

    /// primordial process 首次 dispatch 的真实锁序/epoch 探针。调用点已登记 active；
    /// 本方法不等待，当前 hart 在返回用户态前有界消费自身请求。
    pub(crate) fn selftest_shootdown(&self, lifecycle: &super::lifecycle::Lifecycle) {
        if SHOOTDOWN_SELFTEST_STARTED.swap(true, Ordering::AcqRel) {
            return;
        }
        let completion: Arc<dyn crate::remote_call::Completion> =
            Arc::try_new(ShootdownSelfTestCompletion)
                .expect("shootdown self-test completion allocation failed");
        let prepared = self
            .prepare_shootdown(lifecycle, completion)
            .expect("shootdown self-test reservation failed");
        let (_, synchronization) = self
            .commit_shootdown(lifecycle, prepared, 0, 1, true, |_| ())
            .expect("shootdown self-test execution snapshot changed");
        synchronization.start();
        crate::remote_call::drain_current();
    }

    /// Reserve 阶段快照 active 集合并预留全部 Remote Call 槽。
    pub(crate) fn prepare_shootdown(
        &self,
        lifecycle: &super::lifecycle::Lifecycle,
        completion: Arc<dyn crate::remote_call::Completion>,
    ) -> Result<PreparedShootdown, PrepareShootdownError> {
        let execution = lifecycle
            .snapshot_running()
            .ok_or(PrepareShootdownError::NotRunning)?;
        let active = execution.active();
        if active == 0 {
            return Ok(PreparedShootdown {
                execution,
                remote: None,
                immediate: Some(completion),
            });
        }
        let remote =
            crate::remote_call::reserve(active, completion).map_err(|error| match error {
                crate::remote_call::ReserveError::Busy => PrepareShootdownError::Busy,
                crate::remote_call::ReserveError::InvalidTargets
                | crate::remote_call::ReserveError::EmptyTargets => {
                    PrepareShootdownError::InvalidTargets
                }
                crate::remote_call::ReserveError::AllocationFailed => {
                    PrepareShootdownError::OutOfMemory
                }
            })?;
        Ok(PreparedShootdown {
            execution,
            remote: Some(remote),
            immediate: None,
        })
    }

    /// 在 `ADDRESS_SPACE → LIFECYCLE → REMOTE_CALL` 锁序内完成不可失败 Publish。
    /// stale execution snapshot 在调用 publish 前失败，Prepared 资源自动回滚。
    pub(crate) fn commit_shootdown<R>(
        &self,
        lifecycle: &super::lifecycle::Lifecycle,
        prepared: PreparedShootdown,
        start_vpn: usize,
        page_count: usize,
        instruction: bool,
        publish: impl FnOnce(&mut AddressSpaceState) -> R,
    ) -> Result<(R, ShootdownSynchronization), ShootdownChanged> {
        assert!(page_count != 0, "shootdown range must be nonempty");
        let PreparedShootdown {
            execution,
            remote,
            immediate,
        } = prepared;
        let mut state = self.state.lock();
        lifecycle
            .commit_if_current(execution, |active| {
                debug_assert_eq!(active, execution.active());
                let result = publish(&mut state);
                let epochs = self.publish_epochs(instruction);
                let synchronization = if let Some(remote) = remote {
                    let request = crate::remote_call::FenceRequest::new(
                        self.identity,
                        epochs.translation,
                        if instruction { epochs.instruction } else { 0 },
                        start_vpn,
                        page_count,
                    );
                    ShootdownSynchronization::Remote(remote.publish(request))
                } else {
                    ShootdownSynchronization::Immediate(
                        immediate.expect("empty target shootdown must retain completion"),
                    )
                };
                (result, synchronization)
            })
            .map_err(|_| ShootdownChanged)
    }

    fn publish_epochs(&self, instruction: bool) -> EpochSnapshot {
        let translation = self
            .translation_epoch
            .fetch_add(1, Ordering::Release)
            .checked_add(1)
            .expect("address-space translation epoch exhausted");
        let instruction = if instruction {
            self.instruction_epoch
                .fetch_add(1, Ordering::Release)
                .checked_add(1)
                .expect("address-space instruction epoch exhausted")
        } else {
            self.instruction_epoch.load(Ordering::Acquire)
        };
        EpochSnapshot {
            translation,
            instruction,
        }
    }
}

/// 地址空间可变状态：页表树 + 过渡 backing/布局记账。后续迁移由
/// MemorySpace ledger/backing 逐项替换字段，稳定 identity/epoch 外壳不变。
pub(crate) struct AddressSpaceState {
    /// REAPABLE 屏障后由 drain 最终阶段 take 释放 root；之后任何访问
    /// 都是编程错误（Building 操作准入与 active 位图已消除可达性）。
    tree: Option<TableTree<TableMem, LEVELS>>,
    satp: usize,
    /// 堆顶（页对齐）；[brk, USER_TOP - STACK_SIZE) 为可扩展区。
    brk: usize,
    /// 全部用户数据帧（表帧归树）。
    frames: Vec<FrameTracker>,
    /// 对象拥有的外部映射 reservation；普通地址空间操作不得接管。
    external_mappings: Vec<usize>,
    /// 有界收束游标（drain_gate + space 锁双持下推进）。
    drain_stage: DrainStage,
    /// 已从拥有结构摘下、等待下一 work unit 归还的帧 extent。
    pending_free: Option<FrameTracker>,
}

impl AddressSpaceState {
    /// 新地址空间：建树后把内核高半区作为显式 shared root 槽挂接。
    pub fn new() -> Result<Self, SpaceError> {
        let mut tree = TableTree::new(TableMem).map_err(|_| SpaceError::NoFrame)?;
        mm::install_kernel_top_level(&mut tree);
        let satp = (8usize << 60) | tree.satp_ppn();
        Ok(Self {
            tree: Some(tree),
            satp,
            brk: 0,
            frames: Vec::new(),
            external_mappings: Vec::new(),
            drain_stage: DrainStage::Idle,
            pending_free: None,
        })
    }

    /// 本地址空间的 satp 组装值（含模式位）。
    pub fn satp(&self) -> usize {
        self.satp
    }

    /// 活树访问（drain 完成 root 释放后为零占位期，任何访问都是编程
    /// 错误——REAPABLE 后 Building 操作准入与线程 active 位图已消除
    /// 可达性）。
    fn tt(&mut self) -> &mut TableTree<TableMem, LEVELS> {
        self.tree.as_mut().expect("address space tree is live")
    }

    fn publish_map(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<(), SpaceError> {
        let prepared = self.tt().prepare_map(vpn, count, ppn, flags)?;
        self.tt().publish(prepared);
        Ok(())
    }

    fn publish_unmap(&mut self, vpn: Vpn, count: usize) -> Result<(), SpaceError> {
        let prepared = self.tt().prepare_unmap(vpn, count)?;
        self.tt().publish(prepared);
        Ok(())
    }

    #[expect(dead_code, reason = "多线程/procfs 里程碑使用")]
    pub fn brk(&self) -> usize {
        self.brk
    }

    /// 申请 `count` 页，以若干 power-of-two extent 登记映射与帧所有权。
    fn alloc_map(&mut self, vaddr: usize, count: usize, flags: u64) -> Result<(), SpaceError> {
        debug_assert!(count > 0);
        let committed = self.frames.len();
        // 最坏碎片形态每页一个 extent；提交后 push 不再失败。
        self.frames
            .try_reserve(count)
            .map_err(|_| SpaceError::NoFrame)?;
        let base_vpn = vaddr / PAGE_SIZE;
        let mut mapped = 0usize;

        while mapped < count {
            let Some(tracker) = frame::alloc_largest(count - mapped) else {
                for rollback in (0..mapped).rev() {
                    self.publish_unmap(Vpn(base_vpn + rollback), 1)
                        .expect("anonymous allocation rollback cannot fail");
                }
                self.frames.truncate(committed);
                return Err(SpaceError::NoFrame);
            };
            let extent_count = tracker.count();
            let extent_ppn = tracker.base().addr() / PAGE_SIZE;
            for index in 0..extent_count {
                if let Err(error) = self.publish_map(
                    Vpn(base_vpn + mapped + index),
                    1,
                    Ppn(extent_ppn + index),
                    flags,
                ) {
                    for rollback in (0..mapped + index).rev() {
                        self.publish_unmap(Vpn(base_vpn + rollback), 1)
                            .expect("anonymous allocation rollback cannot fail");
                    }
                    self.frames.truncate(committed);
                    return Err(error);
                }
            }
            mapped += extent_count;
            self.frames.push(tracker);
        }
        Ok(())
    }

    /// 为 Building process 映射 anonymous zero pages。映像区与固定主栈
    /// 窗口不能由一次调用跨越；只有映像区推进 StartupBlock/heap 基准。
    pub fn map_anonymous(
        &mut self,
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
    ) -> Result<(), SpaceError> {
        if len == 0
            || vaddr % PAGE_SIZE != 0
            || len % PAGE_SIZE != 0
            || !permissions.is_known()
            || permissions.raw() == 0
            || permissions.contains(ProcessMapFlags::WRITE)
                && !permissions.contains(ProcessMapFlags::READ)
            || permissions.contains(ProcessMapFlags::WRITE | ProcessMapFlags::EXECUTE)
        {
            return Err(SpaceError::BadSegment);
        }
        let end = vaddr.checked_add(len).ok_or(SpaceError::BadSegment)?;
        let stack_base = USER_TOP - STACK_SIZE;
        if end > USER_TOP || vaddr < stack_base && end > stack_base {
            return Err(SpaceError::BadSegment);
        }
        let mut pte_flags = flags::V | flags::U | flags::A;
        if permissions.contains(ProcessMapFlags::READ) {
            pte_flags |= flags::R;
        }
        if permissions.contains(ProcessMapFlags::WRITE) {
            pte_flags |= flags::W | flags::D;
        }
        if permissions.contains(ProcessMapFlags::EXECUTE) {
            pte_flags |= flags::X;
        }

        let pages = len / PAGE_SIZE;
        self.alloc_map(vaddr, pages, pte_flags)?;
        if end <= stack_base {
            self.brk = self.brk.max(end);
        }
        Ok(())
    }

    /// Building-only 回填；先验证完整目标区间已映射，再经物理直映射写入，
    /// 不要求目标最终 PTE 可写。
    pub fn write_building(&mut self, target: usize, source: &[u8]) -> Result<(), SpaceError> {
        let end = target
            .checked_add(source.len())
            .ok_or(SpaceError::BadSegment)?;
        if end > USER_TOP {
            return Err(SpaceError::BadSegment);
        }
        if !source.is_empty() {
            for vpn in target / PAGE_SIZE..(end - 1) / PAGE_SIZE + 1 {
                let Some(mapping) = self.tt().translate(Vpn(vpn)) else {
                    return Err(SpaceError::BadSegment);
                };
                if mapping.flags & flags::U == 0 {
                    return Err(SpaceError::BadSegment);
                }
            }
        }

        let mut copied = 0;
        while copied < source.len() {
            let va = target + copied;
            let in_page = va % PAGE_SIZE;
            let count = (PAGE_SIZE - in_page).min(source.len() - copied);
            let mapping = self
                .tt()
                .translate(Vpn(va / PAGE_SIZE))
                .expect("prevalidated mapping");
            let pa = mapping.ppn.0 * PAGE_SIZE + in_page;
            // SAFETY: Building process 尚不可运行；目标映射完整验证且其 backing
            // 由本地址空间拥有。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source[copied..].as_ptr(),
                    mm::phys_to_virt(pa) as *mut u8,
                    count,
                );
            }
            copied += count;
        }
        Ok(())
    }

    pub fn validate_initial_context(
        &mut self,
        entry: usize,
        stack_pointer: usize,
    ) -> Result<(), SpaceError> {
        if stack_pointer == 0 || stack_pointer % 16 != 0 || self.brk == 0 {
            return Err(SpaceError::BadSegment);
        }
        let entry_mapping = self
            .tt()
            .translate(Vpn(entry / PAGE_SIZE))
            .ok_or(SpaceError::BadSegment)?;
        let stack_mapping = self
            .tt()
            .translate(Vpn((stack_pointer - 1) / PAGE_SIZE))
            .ok_or(SpaceError::BadSegment)?;
        if entry_mapping.flags & (flags::U | flags::X) != (flags::U | flags::X)
            || stack_mapping.flags & (flags::U | flags::W) != (flags::U | flags::W)
        {
            return Err(SpaceError::BadSegment);
        }
        Ok(())
    }

    /// 装载 ELF：先按页规划权限并集（相邻段共享页取并集，每页恰映射
    /// 一次，杜绝静默改写），再逐页回填段内容（BSS 尾随帧池清零）。
    pub fn load_elf(
        &mut self,
        segments: &[elf::LoadSegment],
        file: &[u8],
    ) -> Result<(), SpaceError> {
        use alloc::collections::BTreeMap;

        // 阶段一：页粒度权限规划。
        let mut plan: BTreeMap<usize, u64> = BTreeMap::new();
        let mut top = 0usize;
        for seg in segments {
            if seg.filesz > seg.memsz {
                return Err(SpaceError::BadSegment);
            }
            let start = seg.vaddr as usize;
            if start % PAGE_SIZE != seg.offset as usize % PAGE_SIZE {
                return Err(SpaceError::BadSegment);
            }
            let end = start
                .checked_add(seg.memsz as usize)
                .ok_or(SpaceError::BadSegment)?;
            if end > USER_TOP {
                return Err(SpaceError::BadSegment);
            }
            let mut fl = flags::V | flags::U | flags::A;
            if seg.readable {
                fl |= flags::R;
            }
            if seg.writable {
                fl |= flags::W | flags::D;
            }
            if seg.executable {
                fl |= flags::X;
            }
            for vpn in start / PAGE_SIZE..end.div_ceil(PAGE_SIZE) {
                *plan.entry(vpn).or_insert(0) |= fl;
            }
            top = top.max(end);
        }

        if plan.values().any(|fl| {
            fl & flags::W != 0 && fl & flags::R == 0
                || fl & (flags::W | flags::X) == (flags::W | flags::X)
        }) {
            return Err(SpaceError::BadSegment);
        }

        // 阶段二：逐页映射（记录 vpn → 物理帧，回填阶段查用）。
        // 两个 Vec 都在安装 PTE 前一次性预留，之后的记账不得因扩容 panic。
        let mut pages: Vec<(usize, usize)> = Vec::new();
        pages
            .try_reserve(plan.len())
            .map_err(|_| SpaceError::NoFrame)?;
        self.frames
            .try_reserve(plan.len())
            .map_err(|_| SpaceError::NoFrame)?;
        for (&vpn, &fl) in &plan {
            let tracker = frame::alloc_order(0).ok_or(SpaceError::NoFrame)?;
            self.publish_map(Vpn(vpn), 1, Ppn(tracker.base().addr() / PAGE_SIZE), fl)?;
            pages.push((vpn, tracker.base().addr()));
            self.frames.push(tracker);
        }

        // 阶段三：回填段内容（跨页逐段拷，页内偏移生效）。
        for seg in segments {
            let start = seg.offset as usize;
            let src = file
                .get(
                    start
                        ..start
                            .checked_add(seg.filesz as usize)
                            .ok_or(SpaceError::BadSegment)?,
                )
                .ok_or(SpaceError::BadSegment)?;
            let mut va = seg.vaddr as usize;
            let mut off = 0usize;
            while off < src.len() {
                let in_page = va % PAGE_SIZE;
                let n = (PAGE_SIZE - in_page).min(src.len() - off);
                let page_index = pages
                    .binary_search_by_key(&(va / PAGE_SIZE), |&(vpn, _)| vpn)
                    .expect("ELF page plan must cover every segment byte");
                let frame = pages[page_index].1;
                // SAFETY: 目标页为本空间独占（刚映射），直映射可写。
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src[off..].as_ptr(),
                        mm::phys_to_virt(frame + in_page) as *mut u8,
                        n,
                    );
                }
                va += n;
                off += n;
            }
        }

        let brk = top.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        if brk > self.brk {
            self.brk = brk;
        }
        Ok(())
    }

    /// Bootstrap 专用 init 栈映射：[USER_TOP - STACK_SIZE, USER_TOP)。
    /// 普通进程的栈由组装者（libprocess）经 ProcessMap 供给，内核不参与
    /// （bootstrap 例外：进程未启动、无用户代码可分配）。
    pub fn map_stack(&mut self) -> Result<(), SpaceError> {
        self.alloc_map(
            USER_TOP - STACK_SIZE,
            STACK_SIZE / PAGE_SIZE,
            flags::USER_DATA,
        )
    }

    /// Bootstrap 专用出生块：prefix 复制到地址空间自有只读页，
    /// 紧随其后的 opaque payload 页在映入时即移交为本地址空间 owned
    /// backing（自帧池启动保留洞收编，Drop 时首次归还池）。该入口不由
    /// syscall 暴露；payload 生命周期随 init 地址空间，无 pid 特判。
    pub fn map_bootstrap_block(
        &mut self,
        prefix: &[u8],
        payload_pa: usize,
        payload_len: usize,
    ) -> Result<usize, SpaceError> {
        if prefix.is_empty()
            || prefix.len() % PAGE_SIZE != 0
            || payload_pa % PAGE_SIZE != 0
            || self.brk == 0
        {
            return Err(SpaceError::BadSegment);
        }
        let base = self.brk;
        let prefix_pages = prefix.len() / PAGE_SIZE;
        let payload_pages = payload_len.div_ceil(PAGE_SIZE);
        let pages = prefix_pages
            .checked_add(payload_pages)
            .ok_or(SpaceError::BadSegment)?;
        let span = pages.checked_mul(PAGE_SIZE).ok_or(SpaceError::BadSegment)?;
        let end = base.checked_add(span).ok_or(SpaceError::BadSegment)?;
        if end > USER_TOP - STACK_SIZE {
            return Err(SpaceError::BadSegment);
        }

        let committed = self.frames.len();
        self.frames
            .try_reserve(prefix_pages + usize::from(payload_pages > 0))
            .map_err(|_| SpaceError::NoFrame)?;
        let base_vpn = base / PAGE_SIZE;
        self.alloc_map(base, prefix_pages, flags::USER_RODATA)?;
        if let Err(error) = self.write_building(base, prefix) {
            for rollback in 0..prefix_pages {
                self.publish_unmap(Vpn(base_vpn + rollback), 1)
                    .expect("bootstrap prefix rollback cannot fail");
            }
            self.frames.truncate(committed);
            return Err(error);
        }

        if payload_pages > 0 {
            for index in 0..payload_pages {
                if let Err(error) = self.publish_map(
                    Vpn(base_vpn + prefix_pages + index),
                    1,
                    Ppn(payload_pa / PAGE_SIZE + index),
                    flags::USER_RODATA,
                ) {
                    for rollback in (0..index).rev() {
                        self.publish_unmap(Vpn(base_vpn + prefix_pages + rollback), 1)
                            .expect("bootstrap payload rollback cannot fail");
                    }
                    for rollback in 0..prefix_pages {
                        self.publish_unmap(Vpn(base_vpn + rollback), 1)
                            .expect("bootstrap prefix rollback cannot fail");
                    }
                    self.frames.truncate(committed);
                    return Err(error.into());
                }
            }
            // payload 帧来自启动 reservation，从未发布为空闲；此处把其完整
            // 页范围唯一移交给地址空间 backing，Drop 时首次进入库存。
            // SAFETY: payload reservation 在启动移交中只执行一次，且尚未发布到 POOL。
            self.frames.push(unsafe {
                FrameTracker::adopt_reserved(FrameNumber(payload_pa / PAGE_SIZE), payload_pages)
            });
        }
        self.brk = end;
        Ok(base)
    }

    /// 堆扩展（sbrk 语义）：申请 `bytes` 字节，内核内部向上取整到页粒度，
    /// 返回新堆顶（页对齐字节地址）。页大小是实现细节，不经 ABI 泄漏；
    /// `bytes == 0` 为查询：返回当前堆顶。虚拟连续性由「从 brk 起步」
    /// 结构性保证；新 PTE 对当前 satp 立即生效。
    /// 映射事务要么全成要么全无：中途失败回滚本次已映页，brk 不前进。
    pub fn extend_heap(&mut self, bytes: usize) -> Result<usize, SpaceError> {
        const MAX_EXTEND_BYTES: usize = 256 << 20;
        if bytes == 0 {
            return Ok(self.brk);
        }
        if bytes > MAX_EXTEND_BYTES {
            return Err(SpaceError::BadSegment);
        }
        let pages = bytes.div_ceil(PAGE_SIZE);
        let delta = pages.checked_mul(PAGE_SIZE).ok_or(SpaceError::BadSegment)?;
        let new_brk = self.brk.checked_add(delta).ok_or(SpaceError::BadSegment)?;
        if new_brk > USER_TOP - STACK_SIZE {
            return Err(SpaceError::BadSegment);
        }
        let base_vpn_v = self.brk / PAGE_SIZE;
        let committed = self.frames.len();
        for i in 0..pages {
            if let Err(e) = self.alloc_map(self.brk + i * PAGE_SIZE, 1, flags::USER_DATA) {
                // 回滚：撤销本次已映页（帧随 FrameTracker 归还帧池），brk 不动。
                for j in (0..i).rev() {
                    self.publish_unmap(Vpn(base_vpn_v + j), 1)
                        .expect("single-page heap rollback cannot fail");
                }
                self.frames.truncate(committed);
                return Err(e);
            }
        }
        self.brk = new_brk;
        // SAFETY: sfence.vma 对当前 ASID 冲刷 stale TLB，使新 PTE 可见。
        unsafe { core::arch::asm!("sfence.vma", options(preserves_flags)) };
        Ok(new_brk)
    }

    /// 校验用户区间 [ptr, ptr+len) 逐页可访问：不溢出、不出用户半区、
    /// 每页已映射且含 U 标志与所需方向权限（读 R / 写 W）。
    /// 供 [`crate::uaccess`] 前置校验；限长由调用方先行把关。
    pub(crate) fn check_range(
        &mut self,
        ptr: usize,
        len: usize,
        writable: bool,
    ) -> Result<(), crate::uaccess::AccessError> {
        use crate::uaccess::AccessError;
        let Some(end) = ptr.checked_add(len) else {
            return Err(AccessError::BadRange);
        };
        if end > USER_TOP || ptr >= USER_TOP && len == 0 {
            return Err(AccessError::BadRange);
        }
        let need = if writable { flags::W } else { flags::R };
        if len == 0 {
            return Ok(());
        }
        for vpn in ptr / PAGE_SIZE..(end - 1) / PAGE_SIZE + 1 {
            match self.tt().translate(Vpn(vpn)) {
                Some(m) => {
                    if m.flags & flags::U == 0 || m.flags & need == 0 {
                        return Err(AccessError::Permission);
                    }
                }
                None => return Err(AccessError::NotMapped),
            }
        }
        Ok(())
    }
}

impl AddressSpaceState {
    /// 查询单页物理地址（跨地址空间完成路径用，见 [`crate::uaccess`]）；
    /// 页必须已映射。仅取地址，权限校验仍由 check_range 承担。
    pub(crate) fn page_pa(&mut self, va: usize) -> Option<usize> {
        self.tt()
            .translate(Vpn(va / PAGE_SIZE))
            .map(|m| m.ppn.0 * PAGE_SIZE)
    }

    /// 将所有权归外部的物理页映射到指定用户 VA（共享内存机制专用，
    /// 如隧道）。帧生命周期归登记方：本空间的页表回收只清 PTE、不归还
    /// 该帧——Drop 链只释放 `frames` 里登记的帧，外部映射天然安全。
    /// 栈窗口与越界地址直接拒绝；冲突由 map 的 Conflict 报出。
    pub fn map_external(&mut self, va: usize, pa: usize) -> Result<(), SpaceError> {
        if va % PAGE_SIZE != 0 || va >= USER_TOP - STACK_SIZE {
            return Err(SpaceError::BadSegment);
        }
        if self.external_mappings.contains(&va) {
            return Err(SpaceError::Conflict);
        }
        self.external_mappings
            .try_reserve(1)
            .map_err(|_| SpaceError::NoFrame)?;
        self.publish_map(
            Vpn(va / PAGE_SIZE),
            1,
            Ppn(pa / PAGE_SIZE),
            flags::USER_DATA,
        )?;
        self.external_mappings.push(va);
        // SAFETY: sfence 当前 ASID 冲刷 stale TLB 使新 PTE 生效。
        unsafe { core::arch::asm!("sfence.vma", options(preserves_flags)) };
        Ok(())
    }

    /// 只有持有对象 lease 的关闭路径才能解除外部 reservation。
    pub fn unmap_external(&mut self, va: usize) {
        let Some(index) = self
            .external_mappings
            .iter()
            .position(|mapped| *mapped == va)
        else {
            return;
        };
        self.external_mappings.swap_remove(index);
        self.publish_unmap(Vpn(va / PAGE_SIZE), 1)
            .expect("single-page external unmap cannot fail");
        // SAFETY: 同 map_external。
        unsafe { core::arch::asm!("sfence.vma", options(preserves_flags)) };
    }

    /// 推进一笔已摘下的帧 extent 归还；分级库存归还具有地址位宽常数上界，
    /// 因此每个 extent 计一个 work unit，不再保存碎片链扫描游标。
    fn step_pending(&mut self, budget: usize) -> (usize, bool) {
        if self.pending_free.is_none() {
            return (0, true);
        }
        if budget == 0 {
            return (0, false);
        }
        drop(self.pending_free.take());
        (1, true)
    }

    /// 登记一笔新的 extent 归还（当前无在途归还时调用）。
    fn enqueue_free(&mut self, tracker: FrameTracker) {
        debug_assert!(
            self.pending_free.is_none(),
            "pending free must be consumed before enqueuing"
        );
        self.pending_free = Some(tracker);
    }

    /// 从页表结构收回一帧并登记延后归还。
    fn enqueue_table_frame(&mut self, frame: FrameNumber) {
        // SAFETY: 调用点先从唯一所属的页表槽或 root 摘除该帧，之后不再访问。
        self.enqueue_free(unsafe { FrameTracker::adopt_table_frame(frame) });
    }

    /// 有界收束一批资源。Handle/PTE 检查、所有权摘除与 extent 归还各计一个
    /// work unit；每个 extent 的库存操作另有只依赖地址位宽和 DT region 上限的
    /// 结构常数界，因此单次执行量受 `budget` 线性约束。
    /// 仅在 REAPABLE 后（drain_gate 持有下）调用；返回 (work_done, complete)。
    pub fn drain(&mut self, budget: usize) -> (usize, bool) {
        let (work, complete) = self.drain_inner(budget);
        debug_assert!(
            work <= budget,
            "space drain over budget: {} > {} complete={}",
            work,
            budget,
            complete
        );
        (work, complete)
    }

    fn drain_inner(&mut self, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        if self.drain_stage == DrainStage::Idle {
            // Handle 阶段已先行：外部映射必须已由对象 close 回调清空。
            debug_assert!(
                self.external_mappings.is_empty(),
                "external mapping outlives its object"
            );
            self.drain_stage = DrainStage::Frames;
        }
        let mut work = 0;

        // 在途归还最优先：完成后才允许推进任何阶段。
        if self.pending_free.is_some() {
            let (used, done) = self.step_pending(budget);
            work += used;
            if !done {
                return (work, false);
            }
            self.pending_free = None;
        }

        loop {
            match self.drain_stage {
                DrainStage::Idle | DrainStage::Done => {
                    return (work, self.drain_stage == DrainStage::Done);
                }
                DrainStage::Frames => {
                    if work + 1 > budget {
                        return (work, false);
                    }
                    let Some(tracker) = self.frames.pop() else {
                        self.drain_stage = DrainStage::Tables { root: 0, l1: 0 };
                        continue;
                    };
                    work += 1; // tracker 出栈 + 登记为 1 个计费步骤
                    self.pending_free = Some(tracker);
                    let (used, done) = self.step_pending(budget - work);
                    work += used;
                    if !done {
                        return (work, false);
                    }
                    self.pending_free = None;
                }
                DrainStage::Tables { root, l1 } => {
                    let mut root_slot = root;
                    let mut l1_slot = l1;
                    while root_slot < page_table::ENTRIES {
                        if work >= budget {
                            self.drain_stage = DrainStage::Tables {
                                root: root_slot,
                                l1: l1_slot,
                            };
                            return (work, false);
                        }

                        let root_state = self
                            .tree
                            .as_mut()
                            .expect("tree exists until Root stage completes")
                            .root_slot_state(root_slot);
                        let l1_frame = match root_state {
                            RootSlotState::Shared | RootSlotState::Empty => {
                                root_slot += 1;
                                l1_slot = 0;
                                work += 1;
                                continue;
                            }
                            RootSlotState::Leaf => {
                                self.tree
                                    .as_mut()
                                    .expect("tree exists until Root stage completes")
                                    .detach_root_slot(root_slot);
                                root_slot += 1;
                                l1_slot = 0;
                                work += 1;
                                continue;
                            }
                            RootSlotState::Branch(frame) => frame,
                        };

                        while l1_slot < page_table::ENTRIES {
                            if work >= budget {
                                self.drain_stage = DrainStage::Tables {
                                    root: root_slot,
                                    l1: l1_slot,
                                };
                                return (work, false);
                            }
                            let state = self
                                .tree
                                .as_mut()
                                .expect("tree exists until Root stage completes")
                                .slot_state(l1_frame, l1_slot);
                            let branch_frame = match state {
                                SlotState::Branch(_) => self
                                    .tree
                                    .as_mut()
                                    .expect("tree exists until Root stage completes")
                                    .detach_branch(l1_frame, l1_slot),
                                SlotState::Empty | SlotState::Leaf => None,
                            };
                            l1_slot += 1;
                            work += 1;
                            if let Some(frame) = branch_frame {
                                self.enqueue_table_frame(frame);
                                let (used, done) = self.step_pending(budget - work);
                                work += used;
                                if !done {
                                    self.drain_stage = DrainStage::Tables {
                                        root: root_slot,
                                        l1: l1_slot,
                                    };
                                    return (work, false);
                                }
                                self.pending_free = None;
                            }
                        }

                        if work >= budget {
                            self.drain_stage = DrainStage::Tables {
                                root: root_slot,
                                l1: l1_slot,
                            };
                            return (work, false);
                        }
                        let detached = self
                            .tree
                            .as_mut()
                            .expect("tree exists until Root stage completes")
                            .detach_root_slot(root_slot);
                        assert_eq!(
                            detached,
                            Some(l1_frame),
                            "root branch changed during address-space drain"
                        );
                        root_slot += 1;
                        l1_slot = 0;
                        work += 1;
                        self.enqueue_table_frame(l1_frame);
                        let (used, done) = self.step_pending(budget - work);
                        work += used;
                        self.drain_stage = DrainStage::Tables {
                            root: root_slot,
                            l1: l1_slot,
                        };
                        if !done {
                            return (work, false);
                        }
                        self.pending_free = None;
                    }
                    self.drain_stage = DrainStage::Root { slot: 0 };
                }
                DrainStage::Root { slot } => {
                    let mut slot = slot;
                    while slot < page_table::ENTRIES {
                        if work >= budget {
                            self.drain_stage = DrainStage::Root { slot };
                            return (work, false);
                        }
                        let state = self
                            .tree
                            .as_mut()
                            .expect("tree exists until Root stage completes")
                            .root_slot_state(slot);
                        assert!(
                            matches!(state, RootSlotState::Empty | RootSlotState::Shared),
                            "owned subtree outlives Tables stage"
                        );
                        slot += 1;
                        work += 1;
                    }
                    if work + 1 > budget {
                        self.drain_stage = DrainStage::Root { slot };
                        return (work, false);
                    }
                    let tree = self
                        .tree
                        .take()
                        .expect("tree exists until Root stage completes");
                    self.enqueue_table_frame(tree.finish_drain());
                    self.drain_stage = DrainStage::RootFree;
                    let (used, done) = self.step_pending(budget - work);
                    work += used;
                    if !done {
                        return (work, false);
                    }
                    self.pending_free = None;
                    self.drain_stage = DrainStage::Done;
                    return (work, true);
                }
                DrainStage::RootFree => {
                    // root 归还在途（顶部 pending 逻辑已完成或已提前返回）。
                    debug_assert!(
                        self.pending_free.is_none(),
                        "pending root free must be stepped at entry"
                    );
                    self.drain_stage = DrainStage::Done;
                    return (work, true);
                }
            }
        }
    }
}

/// 由 drain_gate 串行的 HandleTable 收束状态。pending entry 已推进表
/// 游标、尚待锁外 close；下一批必须优先消费它。
struct DrainState {
    cursor: usize,
    pending_close: Option<super::handle::ProcessHandleEntry>,
}

/// 进程资源容器：地址空间、父子身份与进程本地 HandleTable。
///
/// 线程强持 Process；对象与 WaitContext 只在操作期间持线程或进程引用。
/// HandleTable drain 先摘项再执行对象 callback，避免生命周期回调反向进入表锁。
pub struct Process {
    pub pid: Pid,
    /// 仅用于诊断的创建关系；不产生管理、继承或回收权。
    pub parent: Pid,
    /// 创建域仅维持归属（weak；生命周期根是 Job 直接成员表）。
    job: alloc::sync::Weak<super::job::Job>,
    pub space: AddressSpace,
    /// 新对象 ABI 的进程本地 Handle 表。
    pub(crate) handles: crate::sync::Spinlock<super::handle::ProcessHandleTable>,
    /// 生命周期状态机（顶级锁，见 lifecycle 模块锁序契约）。
    pub(crate) lifecycle: super::lifecycle::Lifecycle,
    /// 观察壳的 weak 回指（REAPABLE/Dead 发布触达；HandleTable 条目强持 shell）。
    control: crate::sync::Spinlock<Option<alloc::sync::Weak<super::process::ProcessControl>>>,
    /// Drain 并发批次仲裁（try_lock；持锁期间推进有界收束）。
    pub(crate) drain_gate: crate::sync::Spinlock<()>,
    /// HandleTable 收束游标与待关闭项（均由 drain_gate 串行）。
    drain_state: crate::sync::Spinlock<DrainState>,
    /// ProcessStart 提交点一次性冻结的执行绑定：非零域编号与执行需求；
    /// 0 唯一表示尚未绑定，避免 Base64 与哨兵重合。
    execution: AtomicUsize,
}

impl Drop for Process {
    fn drop(&mut self) {
        // 防御性兜底：预算恰在摘项后耗尽时，entry 已不在表中，必须先
        // 关闭它才能继续收束地址空间。
        if let Some(entry) = self.drain_state.get_mut().pending_close.take() {
            super::handle::close_entry(entry, self, true);
        }
        // 进程已无外部引用，唯一借用下逐项摘除；对象回调发生在表项
        // 已移除之后，且不持 HandleTable 锁。
        let mut cursor = 1;
        loop {
            let entry = self.handles.get_mut().take_next(&mut cursor);
            let Some(entry) = entry else { break };
            super::handle::close_entry(entry, self, true);
        }
    }
}

impl Process {
    pub(crate) fn new(
        pid: Pid,
        parent: Pid,
        job: alloc::sync::Weak<super::job::Job>,
    ) -> Result<Self, SpaceError> {
        Ok(Self {
            pid,
            parent,
            job,
            space: AddressSpace::new()?,
            handles: crate::sync::Spinlock::chained(
                crate::sync::ranks::HANDLE_TABLE,
                pid,
                super::handle::ProcessHandleTable::new(),
            ),
            lifecycle: super::lifecycle::Lifecycle::building(),
            control: crate::sync::Spinlock::new(crate::sync::ranks::OBJECT_WAIT, None),
            drain_gate: crate::sync::Spinlock::new(crate::sync::ranks::DRAIN_GATE, ()),
            drain_state: crate::sync::Spinlock::new(
                crate::sync::ranks::DRAIN_CURSOR,
                DrainState {
                    cursor: 1,
                    pending_close: None,
                },
            ),
            execution: AtomicUsize::new(0),
        })
    }

    /// 显式附入一条 Building 线程。syscall 与 bootstrap 共用此出生路径；
    /// 调用者负责持有 Building 操作登记，Start 只发布这里已存在的线程。
    pub(crate) fn attach_thread(
        self: &Arc<Self>,
        context: ThreadStartContext,
    ) -> Result<Tid, ThreadAttachError> {
        self.space
            .lock()
            .validate_initial_context(context.entry as usize, context.stack_pointer as usize)
            .map_err(ThreadAttachError::Context)?;
        self.lifecycle
            .attach_member(|tid| {
                Arc::try_new(Thread::new_thread(tid, self, context))
                    .map_err(|_| super::lifecycle::AttachFault::Oom)
            })
            .map_err(|fault| match fault {
                super::lifecycle::AttachFault::Closed => ThreadAttachError::Closed,
                super::lifecycle::AttachFault::Limit => ThreadAttachError::Limit,
                super::lifecycle::AttachFault::Oom => ThreadAttachError::Oom,
            })
    }

    /// 冻结进程级执行绑定（需求 + 兼容域），不可重复。
    pub(crate) fn bind_execution(
        &self,
        requirement: elf::IsaRequirement,
        domain: &'static crate::sched::SchedDomain,
    ) {
        const REQUIREMENT_BIT: usize = 1;
        let requirement_bit = match requirement {
            elf::IsaRequirement::Base64 => 0,
            elf::IsaRequirement::D64 => REQUIREMENT_BIT,
        };
        let encoded = ((domain.index() + 1) << 1) | requirement_bit;
        self.execution
            .compare_exchange(0, encoded, Ordering::Release, Ordering::Relaxed)
            .expect("execution binding frozen twice");
    }

    fn execution(&self) -> usize {
        let execution = self.execution.load(Ordering::Acquire);
        assert_ne!(
            execution, 0,
            "process execution must be bound before dispatch"
        );
        execution
    }

    /// 执行需求（trap FP 档位判定）。
    pub fn requirement(&self) -> elf::IsaRequirement {
        if self.execution() & 1 == 0 {
            elf::IsaRequirement::Base64
        } else {
            elf::IsaRequirement::D64
        }
    }

    /// 域归属（enqueue/pick 路径）。
    pub fn domain(&self) -> &'static crate::sched::SchedDomain {
        let index = (self.execution() >> 1)
            .checked_sub(1)
            .expect("execution binding lost its scheduler domain");
        crate::sched::domain_by_index(index)
    }

    pub(crate) fn set_control(&self, control: alloc::sync::Weak<super::process::ProcessControl>) {
        let previous = self.control.lock().replace(control);
        debug_assert!(previous.is_none());
    }

    pub(crate) fn control(&self) -> Option<Arc<super::process::ProcessControl>> {
        self.control
            .lock()
            .as_ref()
            .and_then(alloc::sync::Weak::upgrade)
    }

    /// 取存活 ProcessControl shell；已消散则从 core 铸造新 shell，并在
    /// 铸造点重放已达成的电平——派生兑底由此接上 drain 入口。单一 shell
    /// 身份：铸造在 control 槽锁内完成，并发派生只会得到同一对象
    /// （两个 shell 的 wait 电平会分叉，绝不允许）。
    ///
    /// 电平重放含 Dead 补冻结：枚举先于移表的竞争窗口内 core 可能已
    /// Dead——只补 REAPABLE 会漏终态冻结，后续 Query 命中「dead 未
    /// 冻结」不变量升级失败。铸造路径上 snapshot 之后无并发 drain
    /// （无任何存活 shell 可持 MANAGE），两步判定无翻转窗口。
    pub(crate) fn revive_control(
        self: &Arc<Self>,
    ) -> Result<Arc<super::process::ProcessControl>, SystemCallError> {
        let control = {
            let mut slot = self.control.lock();
            if let Some(control) = slot.as_ref().and_then(alloc::sync::Weak::upgrade) {
                return Ok(control);
            }
            let control = super::process::ProcessControl::new(self)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            *slot = Some(Arc::downgrade(&control));
            control
        };
        let (state, reason, code) = self.lifecycle.snapshot();
        if state == ProcessState::Dead {
            control.publish_dead(self.pid, self.parent, reason, code);
        } else if self.lifecycle.is_reapable() {
            control.publish_reapable();
        }
        Ok(control)
    }

    /// 所属 Job（生命周期根保证成员存续期 upgrade 必须成功）。
    pub(crate) fn job(&self) -> Arc<super::job::Job> {
        self.job.upgrade().expect("process outlives its job")
    }

    /// 有界收束一批（drain_gate 持有下调用）：先 HandleTable（对象 close
    /// 回调锁外执行，仍可用地址空间解除外部映射），后 AddressSpace。
    /// work unit 诚实计费：Handle 表每个扫描槽位（含空槽，take_next_bounded
    /// 硬性限制本次扫描量）与每次 close 各 1；地址空间部分见
    /// [`AddressSpaceState::drain`]。返回 (work_done, complete)。
    pub(crate) fn drain_batch(&self, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        let mut work = 0;

        // 先关闭上一批在预算边界摘出的项。该项的扫描已计入前一批，当前
        // 只消耗一次 close callback work unit。
        let pending = self.drain_state.lock().pending_close.take();
        if let Some(entry) = pending {
            super::handle::close_entry(entry, self, true);
            work += 1;
            if work == budget {
                return (work, false);
            }
        }

        while work < budget {
            // 本次扫描可用全部剩余预算；若恰好摘到 entry 而已无 close
            // 预算，就把它持久化为 pending。游标已经推进，下一批必先 close。
            let (outcome, scanned) = {
                let mut state = self.drain_state.lock();
                let before = state.cursor;
                let outcome = self
                    .handles
                    .lock()
                    .take_next_bounded(&mut state.cursor, budget - work);
                (outcome, state.cursor - before)
            };
            work += scanned;
            match outcome {
                super::handle::TakeNext::Entry(entry) if work == budget => {
                    self.drain_state.lock().pending_close = Some(entry);
                    return (work, false);
                }
                super::handle::TakeNext::Entry(entry) => {
                    super::handle::close_entry(entry, self, true);
                    work += 1;
                }
                super::handle::TakeNext::Progress => return (work, false),
                super::handle::TakeNext::Exhausted if work == budget => return (work, false),
                super::handle::TakeNext::Exhausted => {
                    let (space_work, complete) = self.space.lock().drain(budget - work);
                    return (work + space_work, complete);
                }
            }
        }
        (work, false)
    }
}

/// 线程：执行容器（用户现场 + 调度观测计数）。执行需求是进程级属性
/// （ELF 判定，Building 期冻结于 Process.requirement），线程经 process
/// 间接持有——同一进程的线程共享同一执行需求。
pub struct Thread {
    /// 进程内线程号（成员表键；tid 从 1 起，0 保留为非身份值）。
    pub tid: Tid,
    pub process: Arc<Process>,
    /// 创建时刻（mtime tick），退出统计用。
    pub created_tick: u64,
    /// 被调度次数（公平性观测，见 notes/impls/task.md）。
    pub switches: AtomicU64,
    frame: UnsafeCell<UserContext>,
}

// SAFETY: UserContext 只在两种互斥状态下被访问：线程在本 hart 执行/
// 挂起期间（trap 路径与 dispatcher 经执行点独占写）；或线程已无容器
// （Waiting：发布时序保证完成方只见已离开一切 hart 引用的线程，见
// sched::park_publish）。其余字段原子或只读。
unsafe impl Sync for Thread {}

impl Thread {
    /// 创建线程执行基底：sepc = entry，sp = stack_pointer，a0/a1 = 出生
    /// 参数（首线程为出生块地址与长度，见 rinlib 启动契约）。FP 状态
    /// 创建即全零——不存在依赖 hart 残留的 valid 状态。tid 由
    /// lifecycle 锁内的 attach_member 分配并注入（构造随闭包进入锁内，
    /// Arc 分配取 HEAP 锁为 LIFECYCLE→HEAP 合法秩）。
    pub(super) fn new_thread(
        tid: Tid,
        process: &Arc<Process>,
        context: ThreadStartContext,
    ) -> Self {
        let mut ctx = UserContext::zeroed();
        ctx.sepc = context.entry;
        ctx.x[2] = context.stack_pointer;
        ctx.x[10] = context.arg1; // a0
        ctx.x[11] = context.arg2; // a1
        Self {
            tid,
            process: process.clone(),
            created_tick: sbi::read_time(),
            switches: AtomicU64::new(0),
            frame: UnsafeCell::new(ctx),
        }
    }

    pub fn frame_ptr(&self) -> *mut UserContext {
        self.frame.get()
    }

    /// pre-sret FP 档位：D64 进程完整恢复，Base 恒 FS=Off。
    pub fn uses_fp(&self) -> bool {
        self.process.requirement() == elf::IsaRequirement::D64
    }

    /// 用户 satp（进程地址空间不变，直接读缓存）。
    pub fn satp(&self) -> usize {
        self.process.space.lock().satp()
    }
}

/// launch 前的进程骨架：ELF 已装载、执行需求已判定、栈已映射、
/// 尚未附线程或入表 runnable。
pub struct SpawnedProcess {
    process: Arc<Process>,
    entry: usize,
    requirement: elf::IsaRequirement,
}

pub fn spawn_from_elf(
    pid: Pid,
    parent: Pid,
    job: alloc::sync::Arc<super::job::Job>,
    image: &elf::Elf,
    file: &[u8],
) -> Result<SpawnedProcess, SpaceError> {
    // 执行需求由 ELF `e_flags` 与 `.riscv.attributes` 判定；F-only/Q/V/
    // TSO/未建模状态扩展在 load 时明确拒绝，不降级为 Base。
    let requirement = elf::isa_requirement(file).expect("userspace execution requirement rejected");
    let process = Arc::new(Process::new(
        pid,
        parent,
        alloc::sync::Arc::downgrade(&job),
    )?);
    {
        let mut space = process.space.lock();
        space.load_elf(&image.segments, file)?;
        space.map_stack()?;
    }
    Ok(SpawnedProcess {
        process,
        entry: image.entry as usize,
        requirement,
    })
}

/// Bootstrap launch 事务：为 init 预留真实 Handle → 构造 prefix 并把
/// BootPackage payload 借入同一 StartupBlock VA → 原子安装 Handle → 创建
/// 主线程并加入 root Job 成员表。普通 ProcessStart 走 `task::process` 的 copied payload。
///
/// 失败全量回滚：临时 Handle 数值随 reservation 作废，输入 entries 按目标
/// 进程退出语义关闭，Job 成员表不出现半初始化项。W^X 发布边界是后续
/// `sched::enqueue` 的 Release。
pub fn launch_bootstrap(
    spawned: SpawnedProcess,
    payload_pa: usize,
    payload: &[u8],
    handles: Vec<super::handle::ProcessHandleEntry>,
) -> Result<Arc<Thread>, SpaceError> {
    let SpawnedProcess {
        process,
        entry,
        requirement,
    } = spawned;

    // init 同样获得 Building 起即存在的 ProcessControl（完整 rights，
    // 显式自杀/查询可用；无结构特例）。
    let control = super::process::ProcessControl::new(&process).map_err(|_| SpaceError::NoFrame)?;
    process.set_control(alloc::sync::Arc::downgrade(&control));
    let control_handle = super::handle::entry(
        super::process::ProcessControl::object_ref(&control),
        super::object::HandleRole::ProcessControl,
        erhino_shared::object::Rights::READ
            | erhino_shared::object::Rights::WAIT
            | erhino_shared::object::Rights::MANAGE
            | erhino_shared::object::Rights::DUPLICATE
            | erhino_shared::object::Rights::TRANSIT
            | erhino_shared::object::Rights::GRANT,
    )
    .map_err(|_| SpaceError::NoFrame)?;

    let mut handles = handles;
    handles.try_reserve(1).map_err(|_| SpaceError::NoFrame)?;
    handles.push(control_handle);
    assert_eq!(
        handles.len(),
        erhino_shared::startup::initial::HANDLE_COUNT,
        "initial capability graph has an unexpected handle count"
    );

    let token = super::handle::transaction_token();
    let reservation = {
        let mut table = process.handles.lock();
        match table.reserve(handles.len(), token) {
            Ok(reservation) => reservation,
            Err(_) => {
                drop(table);
                for handle in handles {
                    super::handle::close_entry(handle, &process, true);
                }
                return Err(SpaceError::NoFrame);
            }
        }
    };

    let block = match erhino_shared::startup::build_startup_prefix(
        process.pid,
        process.parent,
        reservation.handles(),
        PAGE_SIZE,
        payload.len(),
    ) {
        Ok(block) => block,
        Err(error) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry(handle, &process, true);
            }
            return Err(match error {
                erhino_shared::startup::StartupBuildError::Overflow => SpaceError::BadSegment,
                erhino_shared::startup::StartupBuildError::AllocationFailed => SpaceError::NoFrame,
            });
        }
    };

    let block_len = block.len() + payload.len();
    let block_va = match process
        .space
        .lock()
        .map_bootstrap_block(&block, payload_pa, payload.len())
    {
        Ok(va) => va,
        Err(error) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry(handle, &process, true);
            }
            return Err(error);
        }
    };

    process
        .handles
        .lock()
        .commit(reservation, handles)
        .expect("launch reservation count matches entries");

    // 内嵌 ProcessAttach：出生现场 = 出生块地址与长度（rinlib 启动契约）。
    match process.attach_thread(ThreadStartContext {
        entry: entry as u64,
        stack_pointer: USER_TOP as u64,
        arg1: block_va as u64,
        arg2: block_len as u64,
    }) {
        Ok(_) => {}
        Err(ThreadAttachError::Context(error)) => return Err(error),
        Err(ThreadAttachError::Oom) => return Err(SpaceError::NoFrame),
        Err(ThreadAttachError::Closed | ThreadAttachError::Limit) => {
            unreachable!("bootstrap attach must target an empty Building process")
        }
    }
    // 内嵌 ProcessStart（boot 路径失败不可恢复，直接提交不留 marker）：
    // 成员表插入即启动提交；eligibility 无解属 boot fatal（域表在初始
    // 任务装载前已由 bring_up_runtime 构造）。
    let job = process.job();
    let member = job
        .reserve_member(process.pid)
        .map_err(|_| SpaceError::NoFrame)?;
    job.commit_member(member, process.clone());
    debug_assert!(
        process.lifecycle.enter_building_op(),
        "bootstrap process cannot be terminating"
    );
    // Bootstrap 内嵌同构序列的提交段：冻结需求与域、活体门（1 条
    // 预育线程）与预育提取在同一 gate 临界区内完成（普通 Start 的
    // begin_running(expected, staged) 同构——boot 路径无并发，直接
    // expect）。
    let domain =
        crate::sched::resolve_domain(requirement).expect("initial process has no compatible hart");
    let mut staged = Vec::new();
    staged
        .try_reserve_exact(1)
        .map_err(|_| SpaceError::NoFrame)?;
    process
        .lifecycle
        .begin_running(1, &mut staged)
        .expect("bootstrap process cannot be terminating");
    process.bind_execution(requirement, domain);
    let thread = staged.pop().expect("bootstrap staging thread missing");
    Ok(thread)
}
