use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use erhino_shared::{
    object::Handle,
    proc::Pid,
    startup::{
        NO_HANDLE, STARTUP_BLOCK_MAGIC, STARTUP_VERSION, StartupBlockHeader, StartupDescriptor,
        startup_handle as handle_from_slot,
    },
};

/// 启动块基址。0 = 未初始化（启动契约：lang_start 在任何用户代码前写入）。
/// 块为只读映射的不可变快照，写入后仅读。
static BLOCK: AtomicUsize = AtomicUsize::new(0);
/// 0 = 未初始化（启动契约：lang_start 在任何用户代码前写入）。
static PID: AtomicU32 = AtomicU32::new(0);
static PARENT_PID: AtomicU32 = AtomicU32::new(0);

/// 解析并校验启动块（launch 契约：a0 指向块基、a1 为块字节数——内核
/// 提供的可信长度，与块头 `block_len` 必须相等，截断在入口即被确定性
/// 拒绝而非依赖页 fault）。失败即拒绝启动——块是授权方与本库之间的
/// 版本化协议，不满足契约没有可退让的默认形态。
///
/// # Panics
/// 块指针为空、长度不匹配、魔数/版本不符、长度不自洽、reserved 非零、
/// handle_index 越界或 tag 重复时 panic（进程干净退出，见 rt 的 panic
/// handler）。
pub(crate) fn init(block: *const u8, block_len: usize) {
    assert!(!block.is_null(), "null startup block");
    // SAFETY: 块位于 launch 只读映射内；本函数是唯一写者路径的读者，
    // 先校验后发布（BLOCK 发布前无并发读者）。结构只有整数字段，
    // read_unaligned 不要求块内对齐。
    let header = unsafe { core::ptr::read_unaligned(block.cast::<StartupBlockHeader>()) };
    assert_eq!(header.block_len as usize, block_len, "startup block length mismatch");
    assert_eq!(header.magic, STARTUP_BLOCK_MAGIC, "bad startup block magic");
    assert_eq!(header.version, STARTUP_VERSION, "unsupported startup block version");
    assert!(
        header.reserved0 == 0 && header.reserved == [0; 2],
        "nonzero startup block reserved fields"
    );

    let descriptors_len = (header.descriptor_count as usize)
        .checked_mul(core::mem::size_of::<StartupDescriptor>())
        .expect("startup descriptor length overflow");
    let header_len = core::mem::size_of::<StartupBlockHeader>();
    let descriptors_base = header_len
        .checked_add(descriptors_len)
        .expect("startup block length overflow");
    assert!(
        header.block_len as usize >= descriptors_base,
        "truncated startup block"
    );

    // SAFETY: descriptor 数组位于 [块基 + header_len, 块基 + descriptors_base)，
    // 已由 block_len 覆盖；逐项 unaligned 读取。
    let descriptors = unsafe {
        core::slice::from_raw_parts(
            block.byte_add(header_len).cast::<StartupDescriptor>(),
            header.descriptor_count as usize,
        )
    };
    for (index, descriptor) in descriptors.iter().enumerate() {
        assert_eq!(descriptor.reserved, 0, "nonzero startup descriptor reserved");
        assert!(
            descriptor.handle_index == NO_HANDLE || descriptor.handle_index < header.handle_count,
            "startup descriptor handle index out of range"
        );
        let data_end = descriptor
            .data_off
            .checked_add(descriptor.data_len)
            .expect("startup payload range overflow");
        // payload 只能落在 payload 段（descriptor 表之后），不得回指
        // header/descriptor 区；零长度项的规范偏移即 payload 段基。
        assert!(
            descriptor.data_off as usize >= descriptors_base
                && data_end as usize <= header.block_len as usize,
            "startup payload out of segment"
        );
        // tag 是查找键，重复即契约错误（授权方侧唯一性由组装器保证）。
        for prior in &descriptors[..index] {
            assert_ne!(prior.tag, descriptor.tag, "duplicate startup tag");
        }
    }

    PID.store(header.pid, Ordering::Relaxed);
    PARENT_PID.store(header.parent_pid, Ordering::Relaxed);
    BLOCK.store(block as usize, Ordering::Release);
}

fn with_descriptors<R>(f: impl FnOnce(&StartupBlockHeader, &[StartupDescriptor]) -> R) -> R {
    let base = BLOCK.load(Ordering::Acquire) as *const u8;
    assert!(!base.is_null(), "startup block not initialized");
    // SAFETY: 块不可变且 init 已校验布局（含可信长度等值检查）；此后仅重读。
    let header = unsafe { core::ptr::read_unaligned(base.cast::<StartupBlockHeader>()) };
    let descriptors = unsafe {
        core::slice::from_raw_parts(
            base.byte_add(core::mem::size_of::<StartupBlockHeader>())
                .cast::<StartupDescriptor>(),
            header.descriptor_count as usize,
        )
    };
    f(&header, descriptors)
}

pub fn pid() -> Pid {
    PID.load(Ordering::Relaxed)
}

pub fn parent_pid() -> Pid {
    PARENT_PID.load(Ordering::Relaxed)
}

/// 按 tag 取启动授予的 Handle（槽位约定见 `shared::startup`）。
/// 纯 payload descriptor（`NO_HANDLE`）与未知 tag 一律返回 `None`。
pub fn startup_handle(tag: u64) -> Option<Handle> {
    with_descriptors(|_, descriptors| {
        descriptors.iter().find(|d| d.tag == tag).and_then(|d| {
            (d.handle_index != NO_HANDLE).then(|| handle_from_slot(d.handle_index))
        })
    })
}

/// 按 tag 取启动 payload 切片（args、路由表、归档字节等授权方协议数据）。
/// 块随进程地址空间存活，返回值具有 `'static` 生命周期且内容不可变。
pub fn startup_payload(tag: u64) -> Option<&'static [u8]> {
    with_descriptors(|_, descriptors| {
        descriptors.iter().find(|d| d.tag == tag).map(|d| {
            let base = BLOCK.load(Ordering::Acquire) as *const u8;
            // SAFETY: init 已校验 data_off/data_len 落在块内；块不可变。
            unsafe { core::slice::from_raw_parts(base.byte_add(d.data_off as usize), d.data_len as usize) }
        })
    })
}
