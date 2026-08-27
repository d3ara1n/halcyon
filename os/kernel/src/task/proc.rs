//! 进程与线程：资源容器 / 执行容器（见 notes/impls/task.md）。

use core::cell::UnsafeCell;
use core::sync::atomic::AtomicU64;

use alloc::{sync::Arc, vec::Vec};
use erhino_shared::proc::{Pid, ProcessMapFlags, Tid};
use page_table::{FrameMemory, FrameNumber, MapError, Ppn, TableTree, Vpn, flags};

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

impl From<MapError> for SpaceError {
    fn from(e: MapError) -> Self {
        match e {
            MapError::Conflict { .. } => SpaceError::Conflict,
            MapError::FrameExhausted => SpaceError::NoFrame,
            MapError::OutOfRange => SpaceError::BadSegment,
        }
    }
}

/// [`TableTree`] 的帧来源：表帧从帧池取、经 forget 交树持有，树 Drop 时归还。
struct TableMem;

impl FrameMemory for TableMem {
    fn alloc_frame(&mut self) -> Result<FrameNumber, page_table::FrameExhausted> {
        // SAFETY: 分配帧并 forget——所有权移交树，free_frame 时归还。
        frame::alloc_contiguous(1)
            .map(|t| {
                let base = t.base;
                core::mem::forget(t);
                base
            })
            .ok_or(page_table::FrameExhausted)
    }

    fn free_frame(&mut self, frame: FrameNumber) {
        // SAFETY: frame 由 alloc_frame 的 forget 产生，构造 tracker 恰好归还一帧。
        drop(FrameTracker { base: frame, count: 1 });
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

/// 一笔进行中的有界帧归还：区间与帧池扫描游标跨 drain 调用持久化。
struct PendingFree {
    base: page_table::FrameNumber,
    count: usize,
    scan: frame_pool::FreeScan,
}

/// 进程地址空间：页表树 + 数据帧所有权 + 布局记账。
///
/// 访问纪律：所属对象私有层——持 `Process.space` 锁访问；当前 hart 之外
/// 的 hart 仅经进程表取得引用后加锁访问（M3 无此路径，M4 IPC/procfs 需要）。
pub struct AddressSpace {
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
    /// 进行中的有界帧归还（数据帧/表帧/root 帧共用）。
    pending_free: Option<PendingFree>,
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        let Some(tree) = self.tree.as_mut() else {
            return; // drain 已完成（root 已释放）：无内核顶可剥。
        };
        // 先剥离共享的内核顶层项（直映射 + 栈窗口）：这些子树归内核，
        // 随后的树回收（free_subtree）只许触及用户部分。
        // 已知简化：配对纪律靠本调用点自觉；扩展共享分区或新增 teardown
        // 路径时收敛为 root 槽所有权登记（见 notes/impls/mm.md「Root 借用模型」）。
        let root = tree.root_frame();
        let (start, end, window) = mm::kernel_top_level_range();
        tree.clear_slots(root, start, end);
        tree.clear_slots(root, window, window + 1);
    }
}

impl AddressSpace {
    /// 新地址空间：建树 + 拷内核高半区顶层项（共享映射）。与 Drop
    /// 的剥离配对：拷入什么，teardown 前必须剥掉什么。
    pub fn new() -> Result<Self, SpaceError> {
        let tree = TableTree::new(TableMem).map_err(|_| SpaceError::NoFrame)?;
        // SAFETY: root 刚分配、尚未映射任何用户页。
        unsafe { mm::install_kernel_top_level(tree.root_frame()) };
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

    #[expect(dead_code, reason = "多线程/procfs 里程碑使用")]
    pub fn brk(&self) -> usize {
        self.brk
    }

    /// 申请 `count` 页（整段物理连续，利于大页），登记映射与帧所有权。
    fn alloc_map(&mut self, vaddr: usize, count: usize, flags: u64) -> Result<(), SpaceError> {
        self.frames.try_reserve(1).map_err(|_| SpaceError::NoFrame)?;
        let tracker = frame::alloc_contiguous(count).ok_or(SpaceError::NoFrame)?;
        let base_vpn = vaddr / PAGE_SIZE;
        let base_ppn = tracker.base.addr() / PAGE_SIZE;
        for index in 0..count {
            if let Err(error) = self.tt()
                .map(Vpn(base_vpn + index), 1, Ppn(base_ppn + index), flags)
            {
                for rollback in (0..index).rev() {
                    self.tt()
                        .unmap(Vpn(base_vpn + rollback), 1)
                        .expect("single-page allocation rollback cannot fail");
                }
                return Err(error.into());
            }
        }
        self.frames.push(tracker);
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
            || permissions.contains(ProcessMapFlags::WRITE) && !permissions.contains(ProcessMapFlags::READ)
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
        let committed = self.frames.len();
        for index in 0..pages {
            if let Err(error) = self.alloc_map(vaddr + index * PAGE_SIZE, 1, pte_flags) {
                for rollback in (0..index).rev() {
                    self.tt()
                        .unmap(Vpn(vaddr / PAGE_SIZE + rollback), 1)
                        .expect("single-page ProcessMap rollback cannot fail");
                }
                self.frames.truncate(committed);
                return Err(error);
            }
        }
        if end <= stack_base {
            self.brk = self.brk.max(end);
        }
        Ok(())
    }

    /// Building-only 回填；先验证完整目标区间已映射，再经物理直映射写入，
    /// 不要求目标最终 PTE 可写。
    pub fn write_building(&mut self, target: usize, source: &[u8]) -> Result<(), SpaceError> {
        let end = target.checked_add(source.len()).ok_or(SpaceError::BadSegment)?;
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
            let mapping = self.tt().translate(Vpn(va / PAGE_SIZE)).expect("prevalidated mapping");
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

    pub fn validate_initial_context(&mut self, entry: usize, stack_pointer: usize) -> Result<(), SpaceError> {
        if stack_pointer == 0 || stack_pointer % 16 != 0 || self.brk == 0 {
            return Err(SpaceError::BadSegment);
        }
        let entry_mapping = self.tt()
            .translate(Vpn(entry / PAGE_SIZE))
            .ok_or(SpaceError::BadSegment)?;
        let stack_mapping = self.tt()
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
    pub fn load_elf(&mut self, segments: &[elf::LoadSegment], file: &[u8]) -> Result<(), SpaceError> {
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
        })
        {
            return Err(SpaceError::BadSegment);
        }

        // 阶段二：逐页映射（记录 vpn → 物理帧，回填阶段查用）。
        // 两个 Vec 都在安装 PTE 前一次性预留，之后的记账不得因扩容 panic。
        let mut pages: Vec<(usize, usize)> = Vec::new();
        pages.try_reserve(plan.len()).map_err(|_| SpaceError::NoFrame)?;
        self.frames
            .try_reserve(plan.len())
            .map_err(|_| SpaceError::NoFrame)?;
        for (&vpn, &fl) in &plan {
            let tracker = frame::alloc_contiguous(1).ok_or(SpaceError::NoFrame)?;
            self.tt().map(Vpn(vpn), 1, Ppn(tracker.base.addr() / PAGE_SIZE), fl)?;
            pages.push((vpn, tracker.base.addr()));
            self.frames.push(tracker);
        }

        // 阶段三：回填段内容（跨页逐段拷，页内偏移生效）。
        for seg in segments {
            let start = seg.offset as usize;
            let src = file
                .get(start..start.checked_add(seg.filesz as usize).ok_or(SpaceError::BadSegment)?)
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

    /// 映射主线程栈：[USER_TOP - STACK_SIZE, USER_TOP)。
    pub fn map_stack(&mut self) -> Result<(), SpaceError> {
        self.alloc_map(USER_TOP - STACK_SIZE, STACK_SIZE / PAGE_SIZE, flags::USER_DATA)
    }

    /// 只读映射启动块：块基取当前 brk（ELF 尾页对齐处），字节经直映射
    /// 别名写入，brk 越过块尾——堆从块后扩展（sbrk 语义对块无感）。
    /// 块帧记入 `frames`，生命周期随进程地址空间。
    pub fn map_startup_block(&mut self, bytes: &[u8]) -> Result<usize, SpaceError> {
        if bytes.is_empty() || self.brk == 0 {
            return Err(SpaceError::BadSegment); // 空块或 ELF 未装载
        }
        let base = self.brk;
        let pages = bytes.len().div_ceil(PAGE_SIZE);
        let Some(span) = pages.checked_mul(PAGE_SIZE) else {
            return Err(SpaceError::BadSegment);
        };
        let Some(end) = base.checked_add(span) else {
            return Err(SpaceError::BadSegment);
        };
        if end > USER_TOP - STACK_SIZE {
            return Err(SpaceError::BadSegment);
        }
        self.frames.try_reserve(1).map_err(|_| SpaceError::NoFrame)?;
        let tracker = frame::alloc_contiguous(pages).ok_or(SpaceError::NoFrame)?;
        let base_vpn = base / PAGE_SIZE;
        let base_ppn = tracker.base.addr() / PAGE_SIZE;
        for index in 0..pages {
            if let Err(error) = self.tt().map(
                Vpn(base_vpn + index),
                1,
                Ppn(base_ppn + index),
                flags::USER_RODATA,
            ) {
                for rollback in (0..index).rev() {
                    self.tt()
                        .unmap(Vpn(base_vpn + rollback), 1)
                        .expect("single-page StartupBlock map rollback cannot fail");
                }
                return Err(error.into());
            }
        }
        // SAFETY: 刚分配的独占帧经直映射别名写入；用户侧 PTE 只读，
        // 进程对块内容也不可写。
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                mm::phys_to_virt(tracker.base.addr()) as *mut u8,
                bytes.len(),
            );
        }
        self.brk = end;
        self.frames.push(tracker);
        Ok(base)
    }

    /// 回滚尚未发布的普通 StartupBlock。调用者保证它是最后一次 owned
    /// mapping，且尚无线程可运行。
    pub fn rollback_startup_block(&mut self, base: usize, byte_len: usize) {
        let pages = byte_len.div_ceil(PAGE_SIZE);
        let span = pages * PAGE_SIZE;
        assert_eq!(self.brk, base + span, "startup rollback is not the latest mapping");
        for index in 0..pages {
                    self.tt()
                        .unmap(Vpn(base / PAGE_SIZE + index), 1)
                .expect("single-page StartupBlock rollback cannot fail");
        }
        let tracker = self.frames.pop().expect("startup mapping tracker missing");
        assert_eq!(tracker.count, pages, "startup mapping tracker size mismatch");
        self.brk = base;
    }

    /// Bootstrap 专用 StartupBlock：prefix 复制到地址空间自有只读页，
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

        self.frames
            .try_reserve(1 + usize::from(payload_pages > 0))
            .map_err(|_| SpaceError::NoFrame)?;
        let tracker = frame::alloc_contiguous(prefix_pages).ok_or(SpaceError::NoFrame)?;
        let base_vpn = base / PAGE_SIZE;
        let prefix_ppn = tracker.base.addr() / PAGE_SIZE;
        for index in 0..prefix_pages {
            if let Err(error) = self.tt().map(
                Vpn(base_vpn + index),
                1,
                Ppn(prefix_ppn + index),
                flags::USER_RODATA,
            ) {
                for rollback in (0..index).rev() {
                    self.tt()
                        .unmap(Vpn(base_vpn + rollback), 1)
                        .expect("single-page bootstrap prefix rollback cannot fail");
                }
                return Err(error.into());
            }
        }
        // SAFETY: prefix tracker 为本地址空间独占，用户映射只读。
        unsafe {
            core::ptr::copy_nonoverlapping(
                prefix.as_ptr(),
                mm::phys_to_virt(tracker.base.addr()) as *mut u8,
                prefix.len(),
            );
        }
        if payload_pages > 0 {
            for index in 0..payload_pages {
                if let Err(error) = self.tt().map(
                    Vpn(base_vpn + prefix_pages + index),
                    1,
                    Ppn(payload_pa / PAGE_SIZE + index),
                    flags::USER_RODATA,
                ) {
                    for rollback in (0..index).rev() {
                    self.tt()
                        .unmap(Vpn(base_vpn + prefix_pages + rollback), 1)
                            .expect("single-page bootstrap payload rollback cannot fail");
                    }
                    for rollback in 0..prefix_pages {
                    self.tt()
                        .unmap(Vpn(base_vpn + rollback), 1)
                            .expect("single-page bootstrap prefix rollback cannot fail");
                    }
                    return Err(error.into());
                }
            }
            // SAFETY: payload 帧来自帧池启动保留洞（从未入空闲链），
            // 收编为 owned tracker 后由 Drop 在地址空间销毁时首次归还池。
            self.frames.push(FrameTracker {
                base: FrameNumber(payload_pa / PAGE_SIZE),
                count: payload_pages,
            });
        }
        self.frames.push(tracker);
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
                    self.tt()
                        .unmap(Vpn(base_vpn_v + j), 1)
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
    pub(crate) fn check_range(&mut self, ptr: usize, len: usize, writable: bool) -> Result<(), crate::uaccess::AccessError> {
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

impl AddressSpace {
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
        self.tt()
            .map(Vpn(va / PAGE_SIZE), 1, Ppn(pa / PAGE_SIZE), flags::USER_DATA)?;
        self.external_mappings.push(va);
        // SAFETY: sfence 当前 ASID 冲刷 stale TLB 使新 PTE 生效。
        unsafe { core::arch::asm!("sfence.vma", options(preserves_flags)) };
        Ok(())
    }

    /// 只有持有对象 lease 的关闭路径才能解除外部 reservation。
    pub fn unmap_external(&mut self, va: usize) {
        let Some(index) = self.external_mappings.iter().position(|mapped| *mapped == va) else {
            return;
        };
        self.external_mappings.swap_remove(index);
                    self.tt()
                        .unmap(Vpn(va / PAGE_SIZE), 1)
            .expect("single-page external unmap cannot fail");
        // SAFETY: 同 map_external。
        unsafe { core::arch::asm!("sfence.vma", options(preserves_flags)) };
    }

    /// 推进一笔进行中的有界归还；返回 (消耗步数, 是否完成)。
    /// 预算耗尽时游标持久化在 `pending_free`，下次续扫；`budget == 0`
    /// 表示本次调用的工作已被前序步骤用尽（登记本身即本次进展）。
    fn step_pending(&mut self, budget: usize) -> (usize, bool) {
        let Some(pending) = self.pending_free.as_mut() else {
            return (0, true);
        };
        if budget == 0 {
            return (0, false);
        }
        crate::frame::dealloc_step(pending.base, pending.count, &mut pending.scan, budget)
    }

    /// 登记一笔新的有界归还（当前无在途归还时调用）。
    fn enqueue_free(&mut self, base: page_table::FrameNumber, count: usize) {
        debug_assert!(self.pending_free.is_none(), "pending free must be consumed before enqueuing");
        self.pending_free = Some(PendingFree {
            base,
            count,
            scan: frame_pool::FreeScan::default(),
        });
    }

    /// 有界收束一批资源。work unit 是**真实执行步数**：帧池链扫描每步、
    /// 完成插入、Handle/PTE 每个检查或摘除的槽位各 1——预算是硬执行
    /// 上界（单次 drain 调用绝不超过 budget 个基本步 + O(1) 收尾）。
    /// 仅在 REAPABLE 后（drain_gate 持有下）调用；返回 (work_done, complete)。
    pub fn drain(&mut self, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        if self.drain_stage == DrainStage::Idle {
            // Handle 阶段已先行：外部映射必须已由对象 close 回调清空。
            debug_assert!(self.external_mappings.is_empty(), "external mapping outlives its object");
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
                    // 绕过 FrameTracker::Drop（无界链扫描）走有界路径。
                    let tracker = core::mem::ManuallyDrop::new(tracker);
                    work += 1; // tracker 出栈 + 登记为 1 个计费步骤
                    self.enqueue_free(tracker.base, tracker.count);
                    let (used, done) = self.step_pending(budget - work);
                    work += used;
                    if !done {
                        return (work, false);
                    }
                    self.pending_free = None;
                }
                DrainStage::Tables { root, l1 } => {
                    let (kernel_start, _kernel_end, _window) = mm::kernel_top_level_range();
                    let mut root_slot = root;
                    let mut l1_slot = l1;
                    while root_slot < kernel_start {
                        if work >= budget {
                            self.drain_stage = DrainStage::Tables { root: root_slot, l1: l1_slot };
                            return (work, false);
                        }
                        // 读一个 root 槽（独立作用域，不跨 pending 步进持借用）。
                        let slot_frame = {
                            let Some(tree) = self.tree.as_mut() else {
                                unreachable!("tree exists until Root stage completes")
                            };
                            let top = tree.root_frame();
                            let entry = tree.mem_mut().table_mut(top)[root_slot];
                            if !entry.is_valid() {
                                None
                            } else {
                                debug_assert!(entry.is_branch(), "user top-level entries are always branches");
                                Some(entry.next_frame())
                            }
                        };
                        let Some(l1_frame) = slot_frame else {
                            root_slot += 1;
                            l1_slot = 0;
                            work += 1;
                            continue;
                        };
                        while l1_slot < page_table::ENTRIES {
                            if work >= budget {
                                self.drain_stage = DrainStage::Tables { root: root_slot, l1: l1_slot };
                                return (work, false);
                            }
                            let branch_frame = {
                                let Some(tree) = self.tree.as_mut() else {
                                    unreachable!("tree exists until Root stage completes")
                                };
                                let entry = tree.mem_mut().table_mut(l1_frame)[l1_slot];
                                if entry.is_branch() {
                                    tree.mem_mut().table_mut(l1_frame)[l1_slot] =
                                        page_table::Pte::invalid();
                                    Some(entry.next_frame())
                                } else {
                                    None
                                }
                            };
                            l1_slot += 1;
                            work += 1;
                            if let Some(frame) = branch_frame {
                                self.enqueue_free(frame, 1);
                                let (used, done) = self.step_pending(budget - work);
                                work += used;
                                if !done {
                                    self.drain_stage = DrainStage::Tables { root: root_slot, l1: l1_slot };
                                    return (work, false);
                                }
                                self.pending_free = None;
                            }
                        }
                        // L1 的全部 L0 已登记：清 root 槽 + 归还 L1 自身是
                        // 额外 2 个计费步骤（槽清理 1 + L1 归还链扫描），
                        // 预算不足则先持久化游标重入。
                        if work + 2 > budget {
                            // 预算不足：root 槽已清但 L1 未登记——回退重入点
                            // 为 root 已清的哨兵：root 记为已处理会导致跳过
                            // L1 归还。改为不动 root 槽（此分支位于清槽之前），
                            // 直接以当前游标持久化。
                            self.drain_stage = DrainStage::Tables { root: root_slot, l1: l1_slot };
                            return (work, false);
                        }
                        {
                            let Some(tree) = self.tree.as_mut() else {
                                unreachable!("tree exists until Root stage completes")
                            };
                            let top = tree.root_frame();
                            tree.mem_mut().table_mut(top)[root_slot] = page_table::Pte::invalid();
                        }
                        root_slot += 1;
                        l1_slot = 0;
                        work += 1;
                        self.enqueue_free(l1_frame, 1);
                        let (used, done) = self.step_pending(budget - work);
                        work += used;
                        if !done {
                            self.drain_stage = DrainStage::Tables { root: root_slot, l1: l1_slot };
                            return (work, false);
                        }
                        self.pending_free = None;
                    }
                    self.drain_stage = DrainStage::Root { slot: 0 };
                }
                DrainStage::Root { slot } => {
                    let (kernel_start, kernel_end, window) = mm::kernel_top_level_range();
                    let mut slot = slot;
                    while slot < page_table::ENTRIES {
                        if work >= budget {
                            self.drain_stage = DrainStage::Root { slot };
                            return (work, false);
                        }
                        {
                            let Some(tree) = self.tree.as_mut() else {
                                unreachable!("tree exists until Root stage completes")
                            };
                            let top = tree.root_frame();
                            let table = tree.mem_mut().table_mut(top);
                            let shared = slot >= kernel_start && slot < kernel_end || slot == window;
                            if shared {
                                // 剥离内核共享顶层项（子树归内核；与新建时的拷入配对）。
                                table[slot] = page_table::Pte::invalid();
                            } else {
                                debug_assert!(!table[slot].is_valid(), "user subtree outlives Tables stage");
                            }
                        }
                        slot += 1;
                        work += 1;
                    }
                    // 全部 512 槽已验证/剥离：交出 root 帧并转 RootFree
                    // （TableTree::Drop 的递归扫描被绕过——子表已全部释放；
                    // 预算中断后重入不再触碰 tree）。leak + 首次归还步
                    // 预先检查预算。
                    if work + 1 > budget {
                        self.drain_stage = DrainStage::Root { slot };
                        return (work, false);
                    }
                    let tree = self.tree.take().expect("tree exists until Root stage completes");
                    self.enqueue_free(tree.leak_root(), 1);
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
                    debug_assert!(self.pending_free.is_none(), "pending root free must be stepped at entry");
                    self.drain_stage = DrainStage::Done;
                    return (work, true);
                }
            }
        }
    }
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
    pub space: crate::sync::Spinlock<AddressSpace>,
    /// 新对象 ABI 的进程本地 Handle 表。
    pub(crate) handles: crate::sync::Spinlock<super::handle::ProcessHandleTable>,
    /// 生命周期状态机（顶级锁，见 lifecycle 模块锁序契约）。
    pub(crate) lifecycle: super::lifecycle::Lifecycle,
    /// 观察壳的 weak 回指（REAPABLE/Dead 发布触达；HandleTable 条目强持 shell）。
    control: crate::sync::Spinlock<Option<alloc::sync::Weak<super::process::ProcessControl>>>,
    /// Drain 并发批次仲裁（try_lock；持锁期间推进有界收束）。
    pub(crate) drain_gate: crate::sync::Spinlock<()>,
    /// HandleTable 收束游标（drain_gate + 本锁下推进）。
    drain_cursor: crate::sync::Spinlock<usize>,
}

impl Drop for Process {
    fn drop(&mut self) {
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
            space: crate::sync::Spinlock::new(AddressSpace::new()?),
            handles: crate::sync::Spinlock::new(super::handle::ProcessHandleTable::new()),
            lifecycle: super::lifecycle::Lifecycle::building(),
            control: crate::sync::Spinlock::new(None),
            drain_gate: crate::sync::Spinlock::new(()),
            drain_cursor: crate::sync::Spinlock::new(1),
        })
    }

    pub(crate) fn set_control(&self, control: alloc::sync::Weak<super::process::ProcessControl>) {
        let previous = self.control.lock().replace(control);
        debug_assert!(previous.is_none());
    }

    pub(crate) fn control(&self) -> Option<Arc<super::process::ProcessControl>> {
        self.control.lock().as_ref().and_then(alloc::sync::Weak::upgrade)
    }

    /// 所属 Job（生命周期根保证成员存续期 upgrade 必须成功）。
    pub(crate) fn job(&self) -> Arc<super::job::Job> {
        self.job.upgrade().expect("process outlives its job")
    }

    /// 有界收束一批（drain_gate 持有下调用）：先 HandleTable（对象 close
    /// 回调锁外执行，仍可用地址空间解除外部映射），后 AddressSpace。
    /// work unit 诚实计费：Handle 表每个扫描槽位（含空槽，take_next_bounded
    /// 硬性限制本次扫描量）与每次 close 各 1；地址空间部分见
    /// [`AddressSpace::drain`]。返回 (work_done, complete)。
    pub(crate) fn drain_batch(&self, budget: usize) -> (usize, bool) {
        let mut work = 0;
        while work < budget {
            // 预留 close callback 的 1 单位：扫描预算 = 剩余 - 1，
            // 单项（扫描 + 摘除 + close）总成本不趣 budget。
            let (outcome, scanned) = {
                let scan_budget = budget - work - 1;
                let mut cursor = self.drain_cursor.lock();
                let before = *cursor;
                let mut table = self.handles.lock();
                let outcome = table.take_next_bounded(&mut cursor, scan_budget);
                drop(table);
                (outcome, *cursor - before)
            };
            work += scanned; // 空槽扫描同计费（硬预算）
            match outcome {
                super::handle::TakeNext::Entry(entry) => {
                    super::handle::close_entry(entry, self, true);
                    work += 1;
                }
                super::handle::TakeNext::Progress => return (work, false),
                super::handle::TakeNext::Exhausted => {
                    let (space_work, complete) = self.space.lock().drain(budget - work);
                    return (work + space_work, complete);
                }
            }
        }
        (work, false)
    }
}

/// 线程：执行容器（用户现场 + 调度观测计数）。
pub struct Thread {
    #[expect(dead_code, reason = "多线程里程碑使用")]
    pub tid: Tid,
    pub process: Arc<Process>,
    /// 创建时刻（mtime tick），退出统计用。
    pub created_tick: u64,
    /// 被调度次数（公平性观测，见 notes/impls/task.md）。
    pub switches: AtomicU64,
    /// 用户执行需求（ELF 判定；eligibility 由 domain 能力另行核验）。
    pub requirement: elf::IsaRequirement,
    frame: UnsafeCell<UserContext>,
}

// SAFETY: UserContext 只在两种互斥状态下被访问：线程在本 hart 执行/
// 挂起期间（trap 路径与 dispatcher 经执行点独占写）；或线程已无容器
// （Waiting：发布时序保证完成方只见已离开一切 hart 引用的线程，见
// sched::park_publish）。其余字段原子或只读。
unsafe impl Sync for Thread {}

impl Thread {
    /// 创建主线程：a0 = 启动块基、a1 = 块字节数（rinlib 启动契约，见
    /// shared::startup），sp = 半区顶。FP 状态创建即全零——不存在依赖
    /// hart 残留的 valid 状态。
    fn new_main(
        process: Arc<Process>,
        entry: usize,
        requirement: elf::IsaRequirement,
        stack_pointer: usize,
        block_va: usize,
        block_len: usize,
    ) -> Self {
        let mut ctx = UserContext::zeroed();
        ctx.sepc = entry as u64;
        ctx.x[2] = stack_pointer as u64;
        ctx.x[10] = block_va as u64; // a0 = StartupBlock base
        ctx.x[11] = block_len as u64; // a1 = StartupBlock length
        Self {
            tid: 0,
            process,
            created_tick: sbi::read_time(),
            switches: AtomicU64::new(0),
            requirement,
            frame: UnsafeCell::new(ctx),
        }
    }

    pub fn frame_ptr(&self) -> *mut UserContext {
        self.frame.get()
    }

    /// pre-sret FP 档位：D64 线程完整恢复，Base 恒 FS=Off。
    pub fn uses_fp(&self) -> bool {
        self.requirement == elf::IsaRequirement::D64
    }

    /// 用户 satp（进程地址空间不变，直接读缓存）。
    pub fn satp(&self) -> usize {
        self.process.space.lock().satp()
    }
}

/// launch 前的进程骨架：ELF 已装载、栈已映射、尚未入表 runnable。
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
    let process = Arc::new(Process::new(pid, parent, alloc::sync::Arc::downgrade(&job))?);
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

pub(crate) fn prepare_main_thread(
    process: Arc<Process>,
    entry: usize,
    requirement: elf::IsaRequirement,
    stack_pointer: usize,
    block_va: usize,
    block_len: usize,
) -> Result<Arc<Thread>, SpaceError> {
    Arc::try_new(Thread::new_main(
        process,
        entry,
        requirement,
        stack_pointer,
        block_va,
        block_len,
    ))
    .map_err(|_| SpaceError::NoFrame)
}

/// Bootstrap launch 事务：为 init 预留真实 Handle → 构造 prefix 并把
/// BootPackage payload 借入同一 StartupBlock VA → 原子安装 Handle → 创建
/// 主线程并入进程表。普通 ProcessStart 走 `task::process` 的 copied payload。
///
/// 失败全量回滚：临时 Handle 数值随 reservation 作废，输入 entries 按目标
/// 进程退出语义关闭，进程表不出现半初始化项。W^X 发布边界仍是后续
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
    let control = super::process::ProcessControl::new(&process)
        .map_err(|_| SpaceError::NoFrame)?;
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

    let thread = prepare_main_thread(
        process.clone(),
        entry,
        requirement,
        USER_TOP,
        block_va,
        block_len,
    )?;
    // 成员表插入即启动提交（boot 路径失败不可恢复，直接提交不留 marker）。
    let job = process.job();
    let member = job.reserve_member(process.pid).map_err(|_| SpaceError::NoFrame)?;
    job.commit_member(member, process.clone());
    debug_assert!(
        process.lifecycle.enter_building_op(),
        "bootstrap process cannot be terminating"
    );
    process
        .lifecycle
        .begin_running()
        .then_some(())
        .expect("bootstrap process cannot be terminating");
    Ok(thread)
}
