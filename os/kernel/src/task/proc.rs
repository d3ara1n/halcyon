//! 进程与线程：资源容器 / 执行容器（见 notes/task.md）。

use core::cell::UnsafeCell;
use core::sync::atomic::AtomicU64;

use alloc::{sync::Arc, vec::Vec};
use erhino_shared::proc::{Pid, Tid};
use page_table::{FrameMemory, FrameNumber, MapError, Ppn, TableTree, Vpn, flags};

use crate::{
    context::UserContext,
    frame::{self, FrameTracker},
    mm, sbi,
};

/// 页大小（字节）。
pub const PAGE_SIZE: usize = 1 << page_table::PAGE_BITS;

/// 用户半区顶（256GiB），主线程栈顶。
pub const USER_TOP: usize = 1 << 38;

/// 主线程栈大小（8MiB），钉在半区顶。
pub const STACK_SIZE: usize = 8 << 20;

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
            _ => SpaceError::BadSegment,
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

/// 进程地址空间：页表树 + 数据帧所有权 + 布局记账。
///
/// 访问纪律：所属对象私有层——持 `Process.space` 锁访问；当前 hart 之外
/// 的 hart 仅经进程表取得引用后加锁访问（M3 无此路径，M4 IPC/procfs 需要）。
pub struct AddressSpace {
    tree: TableTree<TableMem, LEVELS>,
    satp: usize,
    /// 堆顶（页对齐）；[brk, USER_TOP - STACK_SIZE) 为可扩展区。
    brk: usize,
    /// 全部用户数据帧（表帧归树）。
    frames: Vec<FrameTracker>,
}

impl AddressSpace {
    /// 新地址空间：建树 + 拷内核高半区顶层项（共享映射）。
    pub fn new() -> Result<Self, SpaceError> {
        let tree = TableTree::new(TableMem).map_err(|_| SpaceError::NoFrame)?;
        // SAFETY: root 刚分配、尚未映射任何用户页。
        unsafe { mm::install_kernel_top_level(tree.root_frame()) };
        let satp = (8usize << 60) | tree.satp_ppn();
        Ok(Self { tree, satp, brk: 0, frames: Vec::new() })
    }

    /// 本地址空间的 satp 组装值（含模式位）。
    /// 本地址空间的 satp 组装值（含模式位）。
    pub fn satp(&self) -> usize {
        self.satp
    }

    #[expect(dead_code, reason = "多线程/procfs 里程碑使用")]
    pub fn brk(&self) -> usize {
        self.brk
    }

    /// 申请 `count` 页（整段物理连续，利于大页），登记映射与帧所有权。
    fn alloc_map(&mut self, vaddr: usize, count: usize, flags: u64) -> Result<(), SpaceError> {
        let tracker = frame::alloc_contiguous(count).ok_or(SpaceError::NoFrame)?;
        self.tree
            .map(Vpn(vaddr / PAGE_SIZE), count, Ppn(tracker.base.addr() / PAGE_SIZE), flags)?;
        self.frames.push(tracker);
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
            let end = start + seg.memsz as usize;
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

        // 阶段二：逐页映射（记录 vpn → 物理帧，回填阶段查用）。
        let mut pages: BTreeMap<usize, usize> = BTreeMap::new();
        for (&vpn, &fl) in &plan {
            let tracker = frame::alloc_contiguous(1).ok_or(SpaceError::NoFrame)?;
            self.tree.map(Vpn(vpn), 1, Ppn(tracker.base.addr() / PAGE_SIZE), fl)?;
            pages.insert(vpn, tracker.base.addr());
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
                let frame = pages[&(va / PAGE_SIZE)];
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
                    self.tree.unmap(Vpn(base_vpn_v + j), 1);
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
            match self.tree.translate(Vpn(vpn)) {
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

/// 进程：资源容器（地址空间、父子关系；邮箱/信号/隧道随 IPC 里程碑挂入）。
///
/// 所有权方向：线程强持有进程（Thread.process: Arc<Process>）；一切
/// 「从等待对象/表结构找线程」的反向引用一律持 Weak<Thread>，不在此
/// 处回指——强引用环会让 reap 永不释放帧。
pub struct Process {
    pub pid: Pid,
    pub parent: Pid,
    pub space: crate::sync::Spinlock<AddressSpace>,
    /// 信号配置（SignalSet 记录式实现：接受设置，注入/返回语义随信号里程碑交付）。
    pub signal: crate::sync::Spinlock<SignalConfig>,
}

/// 信号处理配置（rinlib 启动契约：main 前注册 Terminate 处理器）。
#[derive(Clone, Copy)]
pub struct SignalConfig {
    pub mask: u64,
    pub handler: usize,
}

impl Process {
    fn new(pid: Pid, parent: Pid) -> Result<Self, SpaceError> {
        Ok(Self {
            pid,
            parent,
            space: crate::sync::Spinlock::new(AddressSpace::new()?),
            signal: crate::sync::Spinlock::new(SignalConfig { mask: 0, handler: 0 }),
        })
    }
}

/// 线程：执行容器（用户现场 + 调度观测计数）。
pub struct Thread {
    #[expect(dead_code, reason = "多线程里程碑使用")]
    pub tid: Tid,
    pub process: Arc<Process>,
    /// 创建时刻（mtime tick），退出统计用。
    pub created_tick: u64,
    /// 被调度次数（公平性观测，见 notes/task.md）。
    pub switches: AtomicU64,
    /// 退出码（Exit / 异常终止共用；回收时打印）。锁内 Option，
    /// 写于本 hart 的退出路径，读于回收（同 hart 顺序发生）。
    pub(crate) exit_code: crate::sync::Spinlock<Option<i64>>,
    /// 等待代数：每次登记新等待自增；等待条目携带登记时的值，完成方
    /// 以 CAS(gen → gen+1) 消费——单次完成与取消仲裁的唯一凭据
    /// （见 sched::complete）。消费后线程离开 Waiting，再次登记得到
    /// 全新代数，历史条目永不误中。
    pub(crate) wait_gen: AtomicU64,
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
    /// 创建主线程：a0 = pid、a1 = parent（rinlib 启动契约），sp = 半区顶。
    /// FP 状态创建即全零——不存在依赖 hart 残留的 valid 状态。
    fn new_main(process: Arc<Process>, entry: usize, requirement: elf::IsaRequirement) -> Self {
        let mut ctx = UserContext::zeroed();
        ctx.sepc = entry as u64;
        ctx.x[2] = USER_TOP as u64; // sp
        ctx.x[10] = process.pid as u64; // a0
        ctx.x[11] = process.parent as u64; // a1
        Self {
            tid: 0,
            process,
            created_tick: sbi::read_time(),
            switches: AtomicU64::new(0),
            exit_code: crate::sync::Spinlock::new(None),
            wait_gen: AtomicU64::new(0),
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

/// 从 ELF 装载一个进程并创建主线程（initfs 启动路径）。
///
/// 执行需求由 ELF `e_flags` 与 `.riscv.attributes` 判定；F-only/Q/V/TSO/
/// 未建模状态扩展在 load 时明确拒绝，不降级为 Base。W^X：段内容写入
/// 尚不可执行的地址空间，装载完成后经入队 Release 发布（见 sched::enqueue）。
pub fn spawn_from_elf(pid: Pid, parent: Pid, image: &elf::Elf, file: &[u8]) -> Result<Arc<Thread>, SpaceError> {
    let requirement = elf::isa_requirement(file).expect("用户执行需求被拒绝");
    let process = Arc::new(Process::new(pid, parent)?);
    {
        let mut space = process.space.lock();
        space.load_elf(&image.segments, file)?;
        space.map_stack()?;
    }
    let thread = Arc::new(Thread::new_main(
        process.clone(),
        image.entry as usize,
        requirement,
    ));
    super::table::insert(process);
    Ok(thread)
}
