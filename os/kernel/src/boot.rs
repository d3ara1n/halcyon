//! BootPackage bootstrap：验证固定 envelope，只装载唯一 initial ELF，并把
//! opaque payload 作为 StartupBlock 的只读 borrowed backing 交给 init。

use erhino_shared::{
    boot::{BootPackage, validate_boot_package},
    object::Rights,
};

use crate::{frame, mm, sched, task};

const PAGE_SIZE: usize = task::proc::PAGE_SIZE;

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
    let payload_pa = address
        .checked_add(package.header.payload_off as usize)
        .expect("BootPackage payload physical address overflow");
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
    let image = elf::parse(package.initial_elf).expect("BootPackage initial ELF is invalid");
    task::resources::init();
    task::resources::self_test();
    task::proc::building_cutoff_selftest();
    let root_pool = task::memory_pool::MemoryPool::initialize_root(frame::take_root_pool_seed());
    task::memory_pool::MemoryPool::self_test(&root_pool);
    frame::funded_selftest(&root_pool);
    let pid = task::alloc_pid();
    assert_eq!(pid, 1, "initial process must receive PID 1");
    let root_job = task::job::Job::root();
    let spawned = task::spawn_from_elf(
        pid,
        0,
        root_job.clone(),
        &image,
        package.initial_elf,
        root_pool.clone(),
    )
    .expect("initial process image cannot be constructed");
    let root_control = task::handle::entry(
        task::job::Job::object_ref(&root_job),
        task::object::HandleRole::JobControl,
        Rights::CREATE
            | Rights::MANAGE
            | Rights::READ
            | Rights::WAIT
            | Rights::DUPLICATE
            | Rights::TRANSIT
            | Rights::GRANT,
    )
    .expect("root JobControl rights are invalid");
    let system_reset = task::system_reset::SystemReset::new();
    let reset_control = task::handle::entry(
        task::system_reset::SystemReset::object_ref(&system_reset),
        task::object::HandleRole::SystemResetControl,
        Rights::MANAGE | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
    )
    .expect("SystemReset rights are invalid");
    let thread = task::launch_bootstrap(
        spawned,
        payload_held,
        package.payload,
        alloc::vec![root_control, reset_control],
    )
    .expect("initial process cannot be launched");
    task::memory_pool::MemoryPool::syscall_self_test(
        &thread,
        &root_pool,
        task::job::Job::object_ref(&root_job),
    );

    // initial ELF 与 StartupBlock prefix 均已复制完成；prefix owner 在此首次回投。
    // payload owner 已由 init AddressSpace 的 primordial funded backing 接管。
    drop(prefix_held);
    log!(
        Memory,
        "BootPackage prefix reclaim [{:#x}, {:#x})",
        address,
        payload_pa
    );

    sched::enqueue(thread);
    log!(
        Boot,
        "initial process pid {} started (payload {} bytes)",
        pid,
        payload_len
    );
}

fn view(address: usize, length: usize) -> BootPackage<'static> {
    assert!(
        address % PAGE_SIZE == 0,
        "BootPackage physical base is not page-aligned"
    );
    // SAFETY: DT/QEMU loader contract supplies the physical window; the formal direct map
    // covers it, and this module only reads within the declared capacity.
    let bytes =
        unsafe { core::slice::from_raw_parts(mm::phys_to_virt(address) as *const u8, length) };
    validate_boot_package(bytes, PAGE_SIZE).expect("BootPackage envelope is invalid")
}
