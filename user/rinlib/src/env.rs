use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use erhino_shared::{
    object::Handle,
    proc::Pid,
    startup::{StartupBlockHeader, validate_startup_block},
};

/// 启动块基址。0 = 未初始化（启动契约：lang_start 在任何用户代码前写入）。
/// 块为只读映射的不可变快照，写入后仅读。
static BLOCK: AtomicUsize = AtomicUsize::new(0);
/// 0 = 未初始化（启动契约：lang_start 在任何用户代码前写入）。
static PID: AtomicU64 = AtomicU64::new(0);
static PARENT_PID: AtomicU64 = AtomicU64::new(0);

/// 解析并校验内核定义的 StartupBlock 外层。payload 内容完全不解释；
/// launcher 与当前进程自行约定其格式。
///
/// # Panics
/// 块指针为空、可信长度不足或不匹配、魔数/版本不符、区段几何不自洽、
/// reserved 非零时 panic，进程经运行时 panic 路径干净退出。
pub(crate) fn init(block: *const u8, block_len: usize) {
    assert!(!block.is_null(), "null startup block");
    // SAFETY: a0/a1 由内核 launch 同时设置并覆盖完整只读映射；运行时只读。
    let bytes = unsafe { core::slice::from_raw_parts(block, block_len) };
    let header = validate_startup_block(bytes)
        .unwrap_or_else(|error| panic!("invalid startup block: {:?}", error));

    PID.store(header.pid, Ordering::Relaxed);
    PARENT_PID.store(header.parent_pid, Ordering::Relaxed);
    BLOCK.store(block as usize, Ordering::Release);
}

fn block_and_header() -> (*const u8, StartupBlockHeader) {
    let base = BLOCK.load(Ordering::Acquire) as *const u8;
    assert!(!base.is_null(), "startup block not initialized");
    // SAFETY: init 已校验并发布不可变块，此后只重读完整 header。
    let header = unsafe { core::ptr::read_unaligned(base.cast::<StartupBlockHeader>()) };
    (base, header)
}

pub fn pid() -> Pid {
    PID.load(Ordering::Relaxed)
}

/// 仅表示创建关系，不产生管理、继承或回收权。
pub fn parent_pid() -> Pid {
    PARENT_PID.load(Ordering::Relaxed)
}

/// 本次 launch 在当前进程 HandleTable 中安装的实际 Handle 数组。
/// 数值只在当前进程内有效；业务语义由 payload 协议按数组索引关联。
pub fn startup_handles() -> &'static [Handle] {
    let (base, header) = block_and_header();
    // SAFETY: init 已验证 Handle 区位于完整块内；块基页对齐且 header 为
    // 48 字节，数组起点保持 Handle 所需的 8 字节对齐。
    unsafe {
        core::slice::from_raw_parts(
            base.byte_add(core::mem::size_of::<StartupBlockHeader>())
                .cast::<Handle>(),
            header.handle_count as usize,
        )
    }
}

/// 按 launcher/child 约定的数组序号取得一项启动 Handle。
pub fn startup_handle(index: usize) -> Option<Handle> {
    startup_handles().get(index).copied()
}

/// launcher 与当前进程自行解释的不透明 payload。
pub fn startup_payload() -> &'static [u8] {
    let (base, header) = block_and_header();
    // SAFETY: init 已验证 payload 规范区间完整落在只读块内。
    unsafe {
        core::slice::from_raw_parts(
            base.byte_add(header.payload_off as usize),
            header.payload_len as usize,
        )
    }
}
