//! 用户访问：空间锁内 translate 预检与受保护 SUM 直访；跨空间交付使用同一页表翻译。

use crate::{mm::SumGuard, task::Thread, vm::space::AnySpace};
use erhino_shared::{call::SystemCallError, proc::PROCESS_PAGE_SIZE};

pub const MAX_USER_ACCESS: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    BadRange,
    NotMapped,
    Permission,
}

impl From<AccessError> for SystemCallError {
    fn from(_: AccessError) -> Self {
        Self::MemoryNotAccessible
    }
}

pub fn copy_from_user(space: &mut AnySpace, dst: &mut [u8], src: usize) -> Result<(), AccessError> {
    if dst.is_empty() {
        return Ok(());
    }
    if dst.len() > MAX_USER_ACCESS {
        return Err(AccessError::BadRange);
    }
    prepare_direct_access(space, src, dst.len(), false)?;
    // SAFETY: 调用方在空间锁内持有 space，translate 已逐页确认 U|R；
    // 已同步本地翻译，guard 内不调度，目标为独占内核缓冲。
    let _sum = unsafe { SumGuard::open() };
    unsafe { copy_bytes(src, dst.as_mut_ptr() as usize, dst.len()) };
    Ok(())
}

/// 用户 VA 0 可以是有效映射，不能经 Rust 要求 non-null 的指针原语访问。
/// 按 RV64I LB/SB（固定 ISA v20250508，「Load and Store Instructions」）直访。
///
/// # Safety
/// 调用者已校验并稳定映射、开启 SUM，输入/输出区间不重叠且长度有结构上界。
unsafe fn copy_bytes(src: usize, dst: usize, count: usize) {
    // SAFETY: 上述契约保证每一步均可访问；汇编不创建地址为零的 Rust 引用，
    // 默认 memory effect 防止编译器跨越访问重排，三个游标只在本调用中消耗。
    unsafe {
        core::arch::asm!(
            "beqz {count}, 3f",
            "2:",
            "lb {byte}, 0({src})",
            "sb {byte}, 0({dst})",
            "addi {src}, {src}, 1",
            "addi {dst}, {dst}, 1",
            "addi {count}, {count}, -1",
            "bnez {count}, 2b",
            "3:",
            src = inout(reg) src => _,
            dst = inout(reg) dst => _,
            count = inout(reg) count => _,
            byte = out(reg) _,
            options(nostack),
        );
    }
}

/// # Safety
/// T 任意位型均有效，且读取期间调用者持有空间锁。
pub unsafe fn read_user_value<T: Copy>(space: &mut AnySpace, src: usize) -> Result<T, AccessError> {
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: 未初始化存储可作为待写字节；成功读取覆盖所有字节。
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), core::mem::size_of::<T>())
    };
    copy_from_user(space, bytes, src)?;
    // SAFETY: 调用者保证任意位型有效，全部字节已写入。
    Ok(unsafe { value.assume_init() })
}

pub fn copy_to_user(space: &mut AnySpace, dst: usize, src: &[u8]) -> Result<(), AccessError> {
    if src.is_empty() {
        return Ok(());
    }
    if src.len() > MAX_USER_ACCESS {
        return Err(AccessError::BadRange);
    }
    prepare_direct_access(space, dst, src.len(), true)?;
    // SAFETY: 空间锁排除映射变更，已逐页确认 U|W 并同步本地翻译；guard 内不调度。
    let _sum = unsafe { SumGuard::open() };
    unsafe { copy_bytes(src.as_ptr() as usize, dst, src.len()) };
    Ok(())
}

/// 固定结果槽的发布字；调用者持空间锁和账本 UserWriteLease，payload 已先写完。
pub(crate) fn publish_u64(space: &mut AnySpace, dst: usize, value: u64) -> Result<(), AccessError> {
    if !dst.is_multiple_of(core::mem::align_of::<u64>()) {
        return Err(AccessError::BadRange);
    }
    prepare_direct_access(space, dst, core::mem::size_of::<u64>(), true)?;
    // SAFETY: 对齐/本地翻译同步/同空间锁保证 U|W 稳定；此字段 ABI 规定仅原子访问。
    let _sum = unsafe { SumGuard::open() };
    // SAFETY: 不能为合法用户 VA 0 构造 AtomicU64 引用。RVWMO 的 release
    // store 以 FENCE RW,W + 对齐 SD 实现，其他条件由上述校验和空间锁保证。
    unsafe {
        core::arch::asm!(
            "fence rw, w",
            "sd {value}, 0({dst})",
            value = in(reg) value,
            dst = in(reg) dst,
            options(nostack),
        );
    }
    Ok(())
}

/// 同空间 SUM 直访的共同前置；调用者持有空间锁，随后访问期间不让出执行。
fn prepare_direct_access(
    space: &mut AnySpace,
    va: usize,
    len: usize,
    writable: bool,
) -> Result<(), AccessError> {
    space.check_range(va, len, writable)?;
    // SAFETY: 当前 S 态持空间锁，SFENCE.VMA 同步本 hart 对此前 PTE 写入的观察。
    // 事务每步可先发布 PTE 再推进最终 epoch，不能用最终 epoch 判断此处免 fence。
    // 全量 fence 是固定一条指令；默认 memory effect 阻止编译器把直访移到它之前。
    unsafe { core::arch::asm!("sfence.vma", options(nostack)) };
    Ok(())
}

/// 固定 ABI 输出的封闭白名单；安全写回不能接受任意 Rust T。
mod output {
    pub trait Sealed {}
    macro_rules! values {
        ($($ty:ty),* $(,)?) => { $(impl Sealed for $ty {})* };
    }
    // 各类型为 repr(C)/透明的整数布局，共享 ABI 的大小/对齐断言覆盖 padding。
    values!(
        u32,
        u64,
        erhino_shared::wait_set::ReadyRecord,
        erhino_shared::object::Handle,
        erhino_shared::object::HandlePair,
        erhino_shared::object::HandleDescription,
        erhino_shared::object::SenderResult,
        erhino_shared::message::MessageHeader,
        erhino_shared::message::ReceiveResult,
        erhino_shared::memory_object::MemoryObjectSnapshot,
        erhino_shared::mem::MemoryMapResult,
        erhino_shared::proc::ThreadCreateResult,
        erhino_shared::proc::ProcessCreateResult,
        erhino_shared::proc::ProcessSnapshot,
        erhino_shared::proc::JobSnapshot,
        erhino_shared::tunnel::TunnelCreateResult,
        erhino_shared::memory_pool::MemoryPoolSnapshot,
        erhino_shared::time::ClockSnapshot,
    );
}

/// 当前执行线程的安全定宽输出；Frozen 后不准入新的 syscall 输出。
pub fn write_user<T: Copy + output::Sealed>(
    thread: &Thread,
    dst: usize,
    value: &T,
) -> Result<(), SystemCallError> {
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let mut vm = body.vm.space.lock();
    let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
    // SAFETY: Sealed 只开放已验证无 padding 的固定 ABI 类型，持 VM 空间锁。
    unsafe { deliver_output(thread, space, dst, value) }
}

/// # Safety
/// T 的完整对象表示已初始化且无 padding；调用者持有空间锁。
pub unsafe fn deliver_output<T: Copy>(
    thread: &Thread,
    space: &mut AnySpace,
    dst: usize,
    value: &T,
) -> Result<(), SystemCallError> {
    // SAFETY: 同调用者契约。
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    };
    deliver_bytes(thread, space, dst, bytes)
}

/// 副作用后的输出复检失败记录到执行线程，由 trap 在业务锁退出后冻结。
pub fn deliver_bytes(
    thread: &Thread,
    space: &mut AnySpace,
    dst: usize,
    bytes: &[u8],
) -> Result<(), SystemCallError> {
    copy_to_user(space, dst, bytes).map_err(|error| {
        thread.defer_output_fault(dst);
        error.into()
    })
}

/// 仅供持 Building 准入的映像构造者；最终 RX 页在首次执行前可初始化。
pub(crate) fn put_initial(space: &mut AnySpace, dst: usize, src: &[u8]) -> Result<(), AccessError> {
    put_indirect(space, dst, src, false)
}

fn put_indirect(
    space: &mut AnySpace,
    dst: usize,
    src: &[u8],
    writable: bool,
) -> Result<(), AccessError> {
    if src.len() > MAX_USER_ACCESS {
        return Err(AccessError::BadRange);
    }
    space.check_range(dst, src.len(), writable)?;
    let mut offset = 0;
    while offset < src.len() {
        let va = dst + offset;
        let in_page = va % PROCESS_PAGE_SIZE;
        let count = (PROCESS_PAGE_SIZE - in_page).min(src.len() - offset);
        let pa = space.page_pa(va).ok_or(AccessError::NotMapped)?;
        // SAFETY: 空间锁稳定译址；check_range 确认 U|W 或尚未执行的 Building 页 U|R，
        // 内核输入缓冲与用户帧互不重叠。
        unsafe {
            core::ptr::copy_nonoverlapping(
                src[offset..].as_ptr(),
                crate::mm::phys_to_virt(pa + in_page) as *mut u8,
                count,
            );
        }
        offset += count;
    }
    Ok(())
}
