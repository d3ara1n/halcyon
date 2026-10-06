//! BootPackage bootstrap：验证固定 envelope，只装载唯一 initial ELF，并把
//! opaque payload 作为 StartupBlock 的资金化只读 View 交给 init。

use erhino_shared::boot::{BootPackage, validate_boot_package};

use crate::{frame, mm, task};

mod bootstrap;

const PAGE_SIZE: usize = erhino_shared::proc::PROCESS_PAGE_SIZE;

/// 在 DT 声明的最大窗口内验证 envelope，返回应保留的实际总长。
/// 调用点位于正式直映射启用后、帧池注册前。
pub fn inspect(address: usize, capacity: usize) -> usize {
    let package = view(address, capacity);
    package.header.total_len as usize
}

/// 装载并发布唯一 initial process。失败属于不可恢复的 boot failure。
pub fn load(address: usize, length: usize) {
    let package = view(address, length);
    assert_eq!(
        package.header.total_len as usize, length,
        "BootPackage runtime length differs from inspected envelope"
    );
    let payload_len = package.payload.len();
    let reserved_pages = length.div_ceil(PAGE_SIZE);
    let payload_offset_pages = package.header.payload_off as usize / PAGE_SIZE;
    // SAFETY: inspect 后的 BootPackage reservation 尚未进入 free inventory；本函数是
    // 唯一消费方，并立即按 page-aligned payload_off 拆成不重叠 owners。
    let held = unsafe {
        frame::BootHeldExtent::adopt(page_table::FrameNumber(address / PAGE_SIZE), reserved_pages)
    };
    let (prefix_held, payload_held) = if payload_len == 0 {
        (held, None)
    } else {
        let (prefix, payload) = held.split_at(payload_offset_pages);
        (prefix, Some(payload))
    };
    let image = elf::validate(
        package.initial_elf,
        elf::LoadLimits {
            page_size: erhino_shared::proc::PROCESS_PAGE_SIZE as u64,
            image_limit: (erhino_shared::proc::PROCESS_USER_TOP
                - erhino_shared::proc::PROCESS_MAIN_STACK_SIZE) as u64,
        },
    )
    .expect("BootPackage initial ELF admission failed");
    crate::clock::selftest::run();
    let root_pool = task::memory_pool::MemoryPool::initialize_root(frame::take_root_pool_seed());
    task::memory_pool::MemoryPool::self_test(&root_pool);
    frame::selftest::run(&root_pool);
    bootstrap::publish(
        image,
        package.initial_elf,
        prefix_held,
        payload_held,
        payload_len,
        root_pool,
    )
    .expect("initial process construction cannot be admitted");
}

fn view(address: usize, length: usize) -> BootPackage<'static> {
    assert!(
        address.is_multiple_of(PAGE_SIZE),
        "BootPackage physical base is not page-aligned"
    );
    // SAFETY: DT/QEMU loader contract supplies the physical window; the formal direct map
    // covers it, and this module only reads within the declared capacity.
    let bytes =
        unsafe { core::slice::from_raw_parts(mm::phys_to_virt(address) as *const u8, length) };
    validate_boot_package(bytes, PAGE_SIZE).expect("BootPackage envelope is invalid")
}
