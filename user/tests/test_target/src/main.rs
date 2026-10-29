//! target：kill 监督靶子——长寿命 sleep 循环，被外部 ProcessKill 终止
//! 是其唯一正常出口（验证 Waiting 取消与幂等竞争，见 test_kernel 剧本）。

#![no_std]

extern crate alloc;
extern crate libruntime;

use libkernel::{
    ipc::{capability::Capability, notification, wait_set::WaitSet},
    preclude::*,
    shared::{
        object::{ObjectSignals, Rights},
        wait_set::{MemberRequest, ReadyRecord},
    },
    sys_sleep,
};

fn main() {
    let startup = libruntime::startup::take().expect("target startup context is missing");
    let (_process, payload, mut grants) = startup.into_parts();
    if payload.as_bytes() == b"once-payer" {
        once_payer_target(&mut grants);
    }
    if matches!(payload.as_bytes(), b"irq-wait-exit" | b"irq-set-exit") {
        irq_exit_target(&mut grants, payload.as_bytes());
    }
    if matches!(
        payload.as_bytes(),
        b"dma-unexposed"
            | b"dma-exposed"
            | b"dma-output-fault"
            | b"dma-pin-before"
            | b"dma-pin-after"
            | b"dma-pin-exit"
            | b"dma-reader-retire"
            | b"dma-shared"
            | b"dma-pin-quota"
            | b"dma-pin-install-failure"
    ) {
        dma_target(&mut grants, payload.as_bytes());
    }
    if matches!(
        payload.as_bytes(),
        b"mmio-live"
            | b"mmio-close"
            | b"mmio-before"
            | b"mmio-after"
            | b"mmio-forced-queued"
            | b"mmio-forced-publishing"
    ) {
        mmio_target(&mut grants, payload.as_bytes());
    }
    if payload.as_bytes() == b"ipc-kill" {
        ipc_kill_target(&mut grants);
    }
    if payload.as_bytes() == b"retirement" {
        retirement_target(&mut grants);
    }
    debug!("target alive");
    loop {
        // SAFETY: 值参数；本进程只经 kill 退出。
        unsafe { sys_sleep(1000).expect("target sleep") };
    }
}

fn irq_exit_target(startup: &mut libruntime::startup::InitialGrants, mode: &[u8]) -> ! {
    let usage = startup.take(0).expect("IRQ exit target missing use");
    let description = usage
        .description()
        .expect("IRQ exit use description failed");
    assert_eq!(
        description.kind,
        libkernel::shared::object::ObjectKind::IrqUse as u32
    );
    assert_eq!(description.rights, Rights::READ | Rights::WAIT);
    let ready = startup
        .take(1)
        .expect("IRQ exit target missing readiness signal");
    assert_eq!(startup.remaining(), 0, "IRQ exit target has extra grants");
    let ready = notification::NotificationSignaler::from_capability(ready)
        .expect("IRQ exit target readiness role invalid")
        .0;
    if mode == b"irq-set-exit" {
        let set = WaitSet::create(1).expect("IRQ exit WaitSet creation failed");
        set.register(MemberRequest::new(
            usage.as_handle(),
            ObjectSignals::READABLE,
            1,
        ))
        .expect("IRQ exit Line member registration failed");
        ready.signal(1).expect("IRQ exit readiness failed");
        let mut records = [ReadyRecord {
            token: 0,
            cookie: 0,
            observed: ObjectSignals::NONE,
            reason: 0,
            reserved: 0,
        }];
        loop {
            // Register及后续提示只产生候选；Receive复检实际Line电平。
            // 本场景不产生IRQ，Set与成员必须持续持有到外部Kill。
            let count = set
                .receive_into(&mut records)
                .expect("IRQ exit WaitSet receive failed");
            assert_eq!(
                count, 0,
                "IRQ exit target observed an unexpected Line event"
            );
            let observed = set
                .wait(libkernel::time::Deadline::INFINITE)
                .expect("IRQ exit WaitSet wait failed");
            assert_eq!(
                observed.observed,
                ObjectSignals::READABLE,
                "IRQ exit target observed an unexpected WaitSet state"
            );
        }
    } else {
        ready.signal(1).expect("IRQ exit readiness failed");
        libkernel::ipc::wait::wait_until(
            usage.as_handle(),
            ObjectSignals::READABLE,
            libkernel::time::Deadline::INFINITE,
        )
        .expect("IRQ exit Line wait failed");
    }
    panic!("IRQ exit target unexpectedly resumed before process termination");
}

fn mmio_target(startup: &mut libruntime::startup::InitialGrants, mode: &[u8]) -> ! {
    let cap = startup.take(0).expect("MMIO target missing use");
    assert_eq!(
        cap.description().expect("MMIO use query failed").kind,
        libkernel::shared::object::ObjectKind::MmioUse as u32
    );
    // SAFETY: startup 已移交真实 MmioUse capability，唯一关闭责任移入 typed owner。
    let usage = unsafe { libkernel::mmio::MmioUse::from_handle(cap.into_raw()) };
    let ready = notification::NotificationSignaler::from_capability(
        startup.take(1).expect("MMIO target missing ready signaler"),
    )
    .map_err(|failure| failure.error)
    .expect("MMIO target ready role invalid")
    .0;
    let bytes = 32 * libkernel::shared::proc::PROCESS_PAGE_SIZE;
    if matches!(mode, b"mmio-before" | b"mmio-after") {
        let gate = notification::NotificationOwner::from_capability(
            startup.take(2).expect("MMIO publication gate missing"),
        )
        .map_err(|failure| failure.error)
        .expect("MMIO publication gate role invalid")
        .0;
        ready.signal(1).expect("MMIO publication readiness failed");
        let observed = libkernel::ipc::wait::wait_one(gate.as_handle(), ObjectSignals::READABLE, 0)
            .expect("MMIO publication gate wait failed");
        assert!(
            observed.observed.contains(ObjectSignals::READABLE),
            "MMIO publication gate missing"
        );
        let mapped = usage.map(0, bytes, false, libkernel::mm::Placement::Anywhere);
        if mode == b"mmio-before" {
            match &mapped {
                Ok(region) => debug!(
                    "MMIO publication before result observed: error=None, start={:#x}",
                    region.range().start
                ),
                Err(error) => debug!("MMIO publication before result observed: error={:?}", error),
            }
            assert!(
                matches!(
                    mapped,
                    Err(libkernel::shared::call::SystemCallError::ObjectBusy)
                ),
                "MMIO publication before permission did not reject Map"
            );
            let completed = libkernel::ipc::wait::wait_one(usage.handle(), ObjectSignals::DONE, 0)
                .expect("MMIO invalidated publication did not retire");
            assert!(
                completed.observed.contains(ObjectSignals::DONE),
                "MMIO invalidated use omitted DONE"
            );
            assert!(
                matches!(
                    usage.map(0, bytes, false, libkernel::mm::Placement::Anywhere),
                    Err(libkernel::shared::call::SystemCallError::ObjectClosed)
                ),
                "MMIO retired source accepted new Map"
            );
            debug!(
                "MMIO publication result: mode=before, error=ObjectBusy, fresh_after_done=ObjectClosed"
            );
        } else {
            let region = mapped.expect("MMIO accepted publication failed");
            debug!(
                "MMIO publication result: mode=after, error=None, start={:#x}, bytes={}",
                region.range().start,
                region.range().end - region.range().start
            );
            // 不再访问已撤销的IO，也不对可能已被强制退休的VA提交显式Unmap。
            core::hint::black_box(region);
        }
        ready
            .signal(2)
            .expect("MMIO publication result signal failed");
        loop {
            core::hint::black_box(&usage);
            // SAFETY: 无借用指针；父方先核真实Revoke DONE，再沿原Kill收束。
            unsafe { sys_sleep(1000).expect("MMIO publication target sleep failed") };
        }
    }
    let first = usage
        .map(0, bytes, false, libkernel::mm::Placement::Anywhere)
        .expect("MMIO target first Map failed");
    let second = usage
        .map(0, bytes, false, libkernel::mm::Placement::Anywhere)
        .expect("MMIO target second Map failed");
    if mode == b"mmio-close" {
        let _ = (first, second); // 放弃几何 token，不 Unmap；检查最后 capability 自主撤销。
        drop(usage);
        ready.signal(1).expect("MMIO close readiness failed");
        loop {
            // SAFETY: 值参数；父方观察最后 use 退休后通过正常 Kill 收束本进程。
            unsafe { sys_sleep(1000).expect("MMIO close target sleep failed") };
        }
    }
    ready.signal(1).expect("MMIO live readiness failed");
    loop {
        core::hint::black_box((&first, &second));
        // SAFETY: 无借用指针；映射/能力由父方 Revoke 与进程退出实际收束。
        unsafe { sys_sleep(1000).expect("MMIO live target sleep failed") };
    }
}

fn dma_target(startup: &mut libruntime::startup::InitialGrants, mode: &[u8]) -> ! {
    use libkernel::{
        dma::DmaUse,
        memory_object::MemoryObject,
        mm::{MappedRegion, Placement},
        shared::{
            call::SystemCall,
            dma::*,
            mem::MemoryProtection,
            object::{ObjectKind, Rights},
        },
    };
    let cap = startup.take(0).expect("DMA target missing use");
    assert_eq!(
        cap.description().expect("DMA use query failed").kind,
        ObjectKind::DmaUse as u32
    );
    // SAFETY: 已核实际 role，唯一能力 owner 移入。
    let usage = unsafe { DmaUse::from_handle(cap.into_raw()) };
    let cap = startup.take(1).expect("DMA target missing memory");
    assert_eq!(
        cap.description().expect("DMA memory query failed").kind,
        ObjectKind::MemoryObject as u32
    );
    // SAFETY: 实际 MemoryObject role 已核定，接管关闭责任。
    let memory = unsafe { MemoryObject::from_handle(cap.into_raw()) };
    let ready = notification::NotificationSignaler::from_capability(
        startup.take(2).expect("DMA readiness missing"),
    )
    .map_err(|failure| failure.error)
    .expect("DMA readiness role invalid")
    .0;
    if matches!(
        mode,
        b"dma-pin-before"
            | b"dma-pin-after"
            | b"dma-pin-exit"
            | b"dma-reader-retire"
            | b"dma-shared"
            | b"dma-pin-quota"
            | b"dma-pin-install-failure"
    ) {
        dma_race_target(startup, mode, usage, memory, ready);
    }
    let bytes = 2 * libkernel::shared::proc::PROCESS_PAGE_SIZE;
    let cpu = MappedRegion::map_object(
        &memory,
        0,
        bytes,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .expect("DMA target CPU Map failed");
    let mapping = usage
        .pin(
            &memory,
            0,
            bytes,
            DEVICE_WRITES,
            Rights::READ | Rights::WAIT,
        )
        .expect("DMA target Pin failed");
    if mode == b"dma-exposed" {
        let mut segments = [Segment::default(); 1];
        mapping
            .read_segments(0, &mut segments)
            .expect("DMA target segment read failed");
    }
    ready.signal(1).expect("DMA target readiness failed");
    if mode == b"dma-output-fault" {
        let mut result = ReadResult::default();
        let request = ReadRequest {
            offset: 0,
            segments: u64::MAX,
            capacity: 1,
            reserved: 0,
            result: core::ptr::addr_of_mut!(result) as u64,
        };
        // SAFETY: 输入请求存活；故意非法的输出只传整数，不构造无效 Rust 引用。
        unsafe {
            core::arch::asm!("ecall", in("a7") SystemCall::DmaReadSegments as usize,
                inlateout("a0") mapping.handle().raw() as usize => _, inlateout("a1") &request as *const _ as usize => _,
                in("a2") 0usize, in("a3") 0usize, in("a4") 0usize, in("a5") 0usize);
        }
        panic!("DMA output fault returned to user execution");
    }
    loop {
        core::hint::black_box((&usage, &memory, &cpu, &mapping));
        // SAFETY: 值参数；父方正常 Kill 清理 CPU View，DMA 由独立 owner 保活。
        unsafe { sys_sleep(1000).expect("DMA target sleep failed") };
    }
}

/// 沿原Pin/Read/Retire owner验证锁外交错；地址只被本测试读取，不交任何硬件。
fn dma_race_target(
    startup: &mut libruntime::startup::InitialGrants,
    mode: &[u8],
    usage: libkernel::dma::DmaUse,
    memory: libkernel::memory_object::MemoryObject,
    ready: notification::NotificationSignaler,
) -> ! {
    use libkernel::{
        dma::DmaMapping,
        ipc::wait,
        mm::{MappedRegion, Placement},
        shared::{
            call::SystemCallError,
            dma::{DEVICE_WRITES, MappingPhase, Segment},
            mem::MemoryProtection,
            object::Rights,
            proc::PROCESS_PAGE_SIZE as PAGE,
        },
    };
    let gate = notification::NotificationOwner::from_capability(
        startup.take(3).expect("DMA race gate missing"),
    )
    .map_err(|failure| failure.error)
    .expect("DMA race gate role invalid")
    .0;
    let caller_pool = if mode == b"dma-pin-quota" {
        let cap = startup.take(4).expect("DMA quota Pool observer missing");
        let info = cap.description().expect("DMA quota Pool role query failed");
        assert_eq!(
            info.kind,
            libkernel::shared::object::ObjectKind::MemoryPool as u32
        );
        assert_eq!(info.rights, Rights::READ);
        // SAFETY: 实际MemoryPool及精确READ已核，startup唯一owner移入typed观察角色。
        Some(unsafe { libkernel::memory_pool::MemoryPool::from_handle(cap.into_raw()) })
    } else {
        None
    };
    assert_eq!(startup.remaining(), 0, "DMA race grants have extra slots");
    // 首页面的只读use为同一Backing制造真实切界，使两页Pin存在非空、未完成前缀。
    let cpu = MappedRegion::map_object(
        &memory,
        0,
        PAGE,
        0,
        0,
        MemoryProtection::ReadOnly,
        Placement::Anywhere,
    )
    .expect("DMA race CPU readonly Map failed");
    ready.signal(1).expect("DMA race readiness failed");
    let observed =
        wait::wait_one(gate.as_handle(), ObjectSignals::READABLE, 0).expect("DMA race gate failed");
    assert!(
        observed.observed.contains(ObjectSignals::READABLE),
        "DMA race gate omitted readiness"
    );
    assert_eq!(gate.take(1).expect("DMA race start consume failed"), 1);
    if let Some(pool) = &caller_pool {
        let verified = dma_quota_target(&usage, &memory, pool);
        ready
            .signal(if verified { 2 } else { 16 })
            .expect("DMA quota result signal failed");
        loop {
            core::hint::black_box((&usage, &memory, &cpu, &caller_pool));
            // SAFETY: 未向设备交付任何地址，原Pin责任已取消；父方正常Kill收尾。
            unsafe { sys_sleep(1000).expect("DMA quota target sleep failed") };
        }
    }
    if mode == b"dma-shared" {
        dma_shared_target(&usage, &memory, &ready, &gate);
        ready
            .signal(2)
            .expect("DMA shared final result signal failed");
        loop {
            core::hint::black_box((&usage, &memory, &cpu));
            // SAFETY: 两份DMA责任均已真实退休，父方仅以正常Kill收CPU/View尾部。
            unsafe { sys_sleep(1000).expect("DMA shared target sleep failed") };
        }
    }
    let pinned = usage.pin(
        &memory,
        0,
        2 * PAGE,
        DEVICE_WRITES,
        Rights::READ | Rights::WAIT,
    );
    if mode == b"dma-pin-install-failure" {
        assert!(
            matches!(pinned, Err(SystemCallError::OutOfMemory)),
            "DMA final installation did not return the specified allocation failure"
        );
        let end = libkernel::time::timeout_millis(60_000)
            .expect("DMA installation cleanup deadline failed");
        while usage
            .query()
            .expect("DMA installation cleanup query failed")
            .members
            != 0
        {
            assert!(
                !libkernel::time::expired(end).expect("DMA installation cleanup clock failed"),
                "DMA rejected installation retained a member"
            );
            libkernel::thread::yield_now().expect("DMA installation cleanup yield failed");
        }
        assert_eq!(
            memory
                .query()
                .expect("DMA installation writer query failed")
                .write_permits,
            0
        );
        debug!(
            "DMA installation failure collected: error=OutOfMemory, handle=false, exposed=false, members=0, writer=0"
        );
        ready
            .signal(2)
            .expect("DMA installation result signal failed");
        loop {
            core::hint::black_box((&usage, &memory, &cpu));
            // SAFETY: Pin未交出handle或设备地址，原Work已退use/write；父方收CPU/View尾部。
            unsafe { sys_sleep(1000).expect("DMA installation target sleep failed") };
        }
    }
    let mapping: Option<alloc::sync::Arc<DmaMapping>> = if mode == b"dma-pin-before" {
        assert!(
            matches!(pinned, Err(SystemCallError::ObjectBusy)),
            "DMA Pin accepted a writer after Seal"
        );
        debug!("DMA race Pin result: mode=before, error=ObjectBusy");
        None
    } else {
        let mapping = alloc::sync::Arc::new(pinned.expect("DMA race Pin failed"));
        assert_ne!(
            mode, b"dma-pin-exit",
            "DMA preparing-exit window was missed"
        );
        assert_eq!(
            mapping
                .query()
                .expect("DMA race mapping query failed")
                .phase,
            MappingPhase::ReadyUnexposed as u32,
            "DMA race Pin exposed addresses"
        );
        if mode == b"dma-reader-retire" {
            let read_gate = notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL)
                .expect("DMA reader gate create failed");
            let reader_mapping = mapping.clone();
            let reader = libruntime::thread::Builder::new()
                .spawn(move || {
                    let observed =
                        wait::wait_one(read_gate.owner.as_handle(), ObjectSignals::READABLE, 0)
                            .expect("DMA reader gate wait failed");
                    assert!(
                        observed.observed.contains(ObjectSignals::READABLE),
                        "DMA reader gate omitted readiness"
                    );
                    read_gate
                        .owner
                        .take(1)
                        .expect("DMA reader start consume failed");
                    let mut segments = [Segment::default(); 1];
                    let result = reader_mapping
                        .read_segments(0, &mut segments)
                        .expect("DMA accepted reader failed");
                    assert_eq!(result.count, 1, "DMA reader returned no actual segment");
                    assert_ne!(
                        segments[0].address, 0,
                        "DMA reader returned an empty address"
                    );
                    assert_eq!(segments[0].bytes, PAGE as u64, "DMA cut boundary changed");
                    debug!(
                        "DMA race accepted reader completed: bytes={}",
                        segments[0].bytes
                    );
                    let observed =
                        wait::wait_one(read_gate.owner.as_handle(), ObjectSignals::READABLE, 0)
                            .expect("DMA reader completion gate failed");
                    assert!(observed.observed.contains(ObjectSignals::READABLE));
                    assert_eq!(
                        read_gate
                            .owner
                            .take(2)
                            .expect("DMA reader completion consume failed"),
                        2,
                        "DMA reader completion permission missing"
                    );
                })
                .expect("DMA reader thread spawn failed");
            read_gate.peer.signal(1).expect("DMA reader start failed");
            // SAFETY: 地址只在本测试的Read输出中出现，未交硬件，也不再提交；与在途
            // 地址读取的安全次序由原Exposure计数保证，Retire本身不能提前退use/许可。
            unsafe { usage.retire(&mapping) }.expect("DMA reader Retire failed");
            let snapshot = mapping.query().expect("DMA reader retiring query failed");
            assert_eq!(snapshot.phase, MappingPhase::Retiring as u32);
            assert_eq!(snapshot.readers, 1, "DMA reader/Retire window was missed");
            assert_eq!(
                memory
                    .query()
                    .expect("DMA reader writer query failed")
                    .write_permits,
                1,
                "DMA Retire released a writer before its accepted reader"
            );
            assert!(
                !wait::probe(mapping.handle(), ObjectSignals::DONE)
                    .expect("DMA reader DONE probe failed")
                    .contains(ObjectSignals::DONE),
                "DMA Retire completed before its accepted reader"
            );
            debug!("DMA race Retire waits for accepted reader: readers=1, writer=1, done=false");
            read_gate
                .peer
                .signal(2)
                .expect("DMA reader completion release failed");
            assert!(
                matches!(
                    reader.join(),
                    libthread::ThreadOutcome::Returned {
                        value: (),
                        leave: Ok(())
                    }
                ),
                "DMA reader thread did not return normally"
            );
            let observed = wait::wait_one(mapping.handle(), ObjectSignals::DONE, 0)
                .expect("DMA reader retirement wait failed");
            assert!(observed.observed.contains(ObjectSignals::DONE));
            let snapshot = mapping.query().expect("DMA reader retired query failed");
            assert_eq!(snapshot.phase, MappingPhase::Retired as u32);
            assert_eq!(snapshot.readers, 0);
            assert_eq!(
                memory
                    .query()
                    .expect("DMA reader final writer query failed")
                    .write_permits,
                0
            );
            debug!("DMA race reader retired: readers=0, writer=0, done=true");
        } else {
            debug!("DMA race Pin result: mode=after, phase=ReadyUnexposed");
        }
        Some(mapping)
    };
    ready.signal(2).expect("DMA race result signal failed");
    loop {
        core::hint::black_box((&usage, &memory, &cpu, &mapping));
        // SAFETY: 父方在核原结果后沿正常Kill收束，没有未交付设备的访问责任。
        unsafe { sys_sleep(1000).expect("DMA race target sleep failed") };
    }
}

fn dma_shared_target(
    usage: &libkernel::dma::DmaUse,
    memory: &libkernel::memory_object::MemoryObject,
    ready: &notification::NotificationSignaler,
    gate: &notification::NotificationOwner,
) {
    use libkernel::{
        ipc::{object, wait},
        shared::{
            dma::{DEVICE_WRITES, MappingPhase, Segment},
            memory_object::MemoryObjectState,
            object::Rights,
            proc::PROCESS_PAGE_SIZE as PAGE,
        },
    };
    let first = usage
        .pin(
            memory,
            0,
            2 * PAGE,
            DEVICE_WRITES,
            Rights::READ | Rights::WAIT,
        )
        .expect("DMA first shared Pin failed");
    let second = usage
        .pin(
            memory,
            0,
            2 * PAGE,
            DEVICE_WRITES,
            Rights::READ | Rights::WAIT,
        )
        .expect("DMA second shared Pin failed");
    let first_id = object::query(first.handle())
        .expect("DMA first shared role query failed")
        .object_id;
    let second_id = object::query(second.handle())
        .expect("DMA second shared role query failed")
        .object_id;
    assert_ne!(
        first_id, second_id,
        "DMA shared test duplicated one mapping role"
    );
    let mut first_segments = [Segment::default(); 2];
    let mut second_segments = [Segment::default(); 2];
    let first_read = first
        .read_segments(0, &mut first_segments)
        .expect("DMA first shared segments failed");
    let second_read = second
        .read_segments(0, &mut second_segments)
        .expect("DMA second shared segments failed");
    assert_eq!(first_read.count, second_read.count);
    assert!((1..=2).contains(&first_read.count));
    assert_eq!(first_read.next_offset, (2 * PAGE) as u64);
    assert_eq!(second_read.next_offset, first_read.next_offset);
    for (a, b) in first_segments
        .iter()
        .zip(&second_segments)
        .take(first_read.count as usize)
    {
        assert_ne!(a.address, 0, "DMA shared segment address missing");
        assert_ne!(a.bytes, 0, "DMA shared segment length missing");
        assert_eq!(
            (a.address, a.bytes),
            (b.address, b.bytes),
            "DMA mappings do not share the same physical backing"
        );
    }
    let memory_info = memory.query().expect("DMA shared object query failed");
    assert_eq!(memory_info.write_permits, 2);
    debug!(
        "DMA shared mappings exposed: identity={}, first={}, second={}, writers=2, segments={}, address0={:#x}, address1={:#x}",
        memory_info.identity,
        first_id,
        second_id,
        first_read.count,
        first_segments[0].address,
        first_segments[1].address
    );
    ready.signal(2).expect("DMA shared pair signal failed");
    dma_shared_gate(gate, 2);
    // SAFETY: 两份SG只在本测试中读取，从未交设备；关闭第一份不能代替另一份退休。
    unsafe { usage.retire(&first) }.expect("DMA first shared Retire failed");
    let observed = wait::wait_one(first.handle(), ObjectSignals::DONE, 0)
        .expect("DMA first shared DONE wait failed");
    assert!(observed.observed.contains(ObjectSignals::DONE));
    assert_eq!(
        first
            .query()
            .expect("DMA first shared retired query failed")
            .phase,
        MappingPhase::Retired as u32
    );
    let memory_info = memory
        .query()
        .expect("DMA first shared writer query failed");
    assert_eq!(memory_info.state, MemoryObjectState::Sealing as u32);
    assert_eq!(memory_info.write_permits, 1);
    assert_eq!(
        second
            .query()
            .expect("DMA second shared live query failed")
            .phase,
        MappingPhase::MayBeExposed as u32
    );
    let mut remaining = [Segment::default(); 2];
    let result = second
        .read_segments(0, &mut remaining)
        .expect("DMA remaining shared segments failed");
    assert_eq!(result.count, second_read.count);
    assert_eq!(result.next_offset, second_read.next_offset);
    for (old, current) in second_segments
        .iter()
        .zip(&remaining)
        .take(result.count as usize)
    {
        assert_eq!(
            (old.address, old.bytes),
            (current.address, current.bytes),
            "DMA first Retire changed the surviving physical backing"
        );
    }
    drop(first);
    debug!(
        "DMA shared first retired: identity={}, survivor={}, writers=1, physical_unchanged=true",
        memory_info.identity, second_id
    );
    ready
        .signal(4)
        .expect("DMA shared first retirement signal failed");
    dma_shared_gate(gate, 4);
    // SAFETY: 另一份地址同样从未提交；确认此份责任结束后才释放第二个use/写许可。
    unsafe { usage.retire(&second) }.expect("DMA second shared Retire failed");
    let observed = wait::wait_one(second.handle(), ObjectSignals::DONE, 0)
        .expect("DMA second shared DONE wait failed");
    assert!(observed.observed.contains(ObjectSignals::DONE));
    assert_eq!(
        second
            .query()
            .expect("DMA second shared retired query failed")
            .phase,
        MappingPhase::Retired as u32
    );
    let memory_info = memory
        .query()
        .expect("DMA final shared writer query failed");
    assert_eq!(memory_info.state, MemoryObjectState::Executable as u32);
    assert_eq!(memory_info.write_permits, 0);
    drop(second);
    debug!(
        "DMA shared second retired: identity={}, writers=0, state=Executable",
        memory_info.identity
    );
    ready
        .signal(8)
        .expect("DMA shared second retirement signal failed");
    dma_shared_gate(gate, 8);
}

fn dma_shared_gate(gate: &notification::NotificationOwner, bits: u64) {
    let observed = libkernel::ipc::wait::wait_one(gate.as_handle(), ObjectSignals::READABLE, 0)
        .expect("DMA shared gate wait failed");
    assert!(observed.observed.contains(ObjectSignals::READABLE));
    assert_eq!(
        gate.take(bits).expect("DMA shared gate consume failed"),
        bits
    );
}

fn dma_quota_target(
    usage: &libkernel::dma::DmaUse,
    memory: &libkernel::memory_object::MemoryObject,
    pool: &libkernel::memory_pool::MemoryPool,
) -> bool {
    use libkernel::shared::{
        call::SystemCallError,
        dma::{DEVICE_WRITES, MappingPhase},
        object::Rights,
        proc::PROCESS_PAGE_SIZE as PAGE,
    };
    let original = pool.query().expect("DMA quota original Pool query failed");
    let bound = usize::try_from(original.metadata.available)
        .expect("DMA quota metadata bound exceeds target usize");
    let mut pressure = alloc::vec::Vec::new();
    // 每份真实Notification至少付一槽非零metadata；先预留owner向量，避免满额时
    // 再为保存owner分配。向量只保存真实cap，不表示或替代内核额度账本。
    if pressure.try_reserve_exact(bound).is_err() {
        debug!("DMA quota fixture owner storage unavailable");
        return false;
    }
    loop {
        if pressure.len() == pressure.capacity() {
            debug!("DMA quota fixture owner bound reached before rejection");
            return false;
        }
        match notification::create(Rights::NONE, Rights::NONE) {
            Ok(pair) => pressure.push(pair),
            Err(SystemCallError::QuotaExceeded) => break,
            Err(error) => {
                debug!("DMA quota fixture stopped before Pin: error={:?}", error);
                return false;
            }
        }
    }
    let count = pressure.len();
    let Some(released) = pressure.pop() else {
        debug!("DMA quota fixture admitted no pressure owner");
        return false;
    };
    drop(released);
    let before = pool.query().expect("DMA quota Pin margin query failed");
    let verified = match usage.pin(
        memory,
        0,
        2 * PAGE,
        DEVICE_WRITES,
        Rights::READ | Rights::WAIT,
    ) {
        Err(SystemCallError::QuotaExceeded) => true,
        Err(error) => {
            debug!("DMA quota Pin had another result: error={:?}", error);
            false
        }
        Ok(mapping) => {
            // 从未读取地址，关闭ReadyUnexposed只接通原取消，不编造硬件静止。
            drop(mapping);
            debug!("DMA quota Pin margin admitted a mapping; rejection window not covered");
            false
        }
    };
    let after = pool.query().expect("DMA quota rejected Pool query failed");
    debug!(
        "DMA quota Pin observed: pool={}, owners={}, quota={}, available_before={}, available_after={}, allocated_before={}, allocated_after={}",
        before.identity,
        count,
        verified,
        before.metadata.available,
        after.metadata.available,
        before.metadata.allocated,
        after.metadata.allocated
    );
    drop(pressure);
    let end = libkernel::time::timeout_millis(60_000).expect("DMA quota cleanup deadline failed");
    while usage
        .query()
        .expect("DMA quota cancelled use query failed")
        .members
        != 0
    {
        assert!(
            !libkernel::time::expired(end).expect("DMA quota cleanup clock failed"),
            "DMA quota rejected Pin retained a member"
        );
        libkernel::thread::yield_now().expect("DMA quota cleanup yield failed");
    }
    assert_eq!(
        memory
            .query()
            .expect("DMA quota writer query failed")
            .write_permits,
        0
    );
    let fresh = usage
        .pin(
            memory,
            0,
            2 * PAGE,
            DEVICE_WRITES,
            Rights::READ | Rights::WAIT,
        )
        .expect("DMA source did not recover after releasing metadata pressure");
    assert_eq!(
        fresh
            .query()
            .expect("DMA quota fresh mapping query failed")
            .phase,
        MappingPhase::ReadyUnexposed as u32
    );
    assert_eq!(
        memory
            .query()
            .expect("DMA quota fresh writer query failed")
            .write_permits,
        1
    );
    drop(fresh);
    while usage
        .query()
        .expect("DMA quota fresh cleanup query failed")
        .members
        != 0
    {
        assert!(
            !libkernel::time::expired(end).expect("DMA quota final clock failed"),
            "DMA quota fresh unexposed mapping did not cancel"
        );
        libkernel::thread::yield_now().expect("DMA quota final yield failed");
    }
    assert_eq!(
        memory
            .query()
            .expect("DMA quota final writer query failed")
            .write_permits,
        0
    );
    debug!(
        "DMA quota source recovered: quota={}, members=0, writer=0, fresh=ReadyUnexposed",
        verified
    );
    verified
}

fn ipc_kill_target(startup: &mut libruntime::startup::InitialGrants) -> ! {
    let owner = startup
        .take(0)
        .map(Capability::into_raw)
        .expect("IPC kill target missing Close owner");
    let control = startup
        .take(1)
        .map(Capability::into_raw)
        .expect("IPC kill target missing observed process control");
    let ready = startup
        .take(2)
        .map(|cap| {
            notification::NotificationSignaler::from_capability(cap)
                .map_err(|f| f.error)
                .expect("target ready role invalid")
                .0
        })
        .expect("IPC kill target missing ready signaler");
    let done = startup
        .take(3)
        .map(|cap| {
            notification::NotificationSignaler::from_capability(cap)
                .map_err(|f| f.error)
                .expect("target completion role invalid")
                .0
        })
        .expect("IPC kill target missing observed completion signaler");
    let (notification_observer, description) = notification::NotificationObserver::from_capability(
        startup
            .take(4)
            .expect("IPC target missing startup observer"),
    )
    .map_err(|f| f.error)
    .expect("IPC startup observer role invalid");
    assert_eq!(
        description.related_object_id,
        ready
            .description()
            .expect("target ready identity query failed")
            .related_object_id
    );
    let close = libruntime::thread::Builder::new()
        .spawn(move || {
            // SAFETY: startup 转入的 owner 只有本线程承担关闭责任。
            unsafe { libkernel::ipc::object::close(owner) }
                .expect("IPC target committed Close failed");
        })
        .expect("IPC Close thread spawn failed");
    let observer = libruntime::thread::Builder::new()
        .spawn(move || {
            libkernel::process::wait_done(control).expect("IPC target completion wait failed");
            done.signal(1).expect("IPC target completion signal failed");
        })
        .expect("IPC observation thread spawn failed");
    ready
        .signal(1)
        .expect("IPC target active publication failed");
    let observed = libkernel::ipc::wait::wait_until(
        notification_observer.as_handle(),
        ObjectSignals::READABLE,
        libkernel::time::timeout_millis(5_000).expect("startup observer deadline failed"),
    )
    .expect("startup observer wait failed");
    assert!(observed.observed.intersects(ObjectSignals::READABLE));
    libkernel::debug!("Notification observer startup grant passed");
    // 两个 ResultReceiver 与未返回线程由正式进程退出路径接管；主线程进入用户 spin，
    // 外部 Kill 允许与 Close/observation 的自然完成或取消竞速，精确 Waiting 窗口由 GDB 取证。
    loop {
        core::hint::black_box((&close, &observer));
        core::hint::spin_loop();
    }
}

fn retirement_target(startup: &mut libruntime::startup::InitialGrants) -> ! {
    let (source, source_description) = notification::NotificationOwner::from_capability(
        startup
            .take(0)
            .expect("retirement target missing observation source"),
    )
    .map_err(|f| f.error)
    .expect("retirement source role invalid");
    let (notification_observer, description) = notification::NotificationObserver::from_capability(
        startup
            .take(2)
            .expect("retirement target missing startup observer"),
    )
    .map_err(|f| f.error)
    .expect("retirement observer role invalid");
    assert_eq!(description.related_object_id, source_description.object_id);
    let ready = startup
        .take(1)
        .map(|cap| {
            notification::NotificationSignaler::from_capability(cap)
                .map_err(|f| f.error)
                .expect("retirement ready role invalid")
                .0
        })
        .expect("retirement target missing ready signaler");
    let set = WaitSet::create(256).expect("retirement target WaitSet creation failed");
    for cookie in 0..256 {
        set.register(MemberRequest::new(
            source.as_handle(),
            ObjectSignals::READABLE,
            cookie,
        ))
        .expect("retirement target registration failed");
    }
    libkernel::debug!("Notification observer startup grant passed");
    ready
        .signal(1)
        .expect("retirement target ready publication failed");
    loop {
        core::hint::black_box((&source, &notification_observer, set.handle()));
        // SAFETY: 值参数；集合 owner 与来源由进程退出的正式退休路径接管。
        unsafe { sys_sleep(1000).expect("retirement target sleep failed") };
    }
}

/// 发行者使用独立Process Pool；目标Mailbox/原Sender与实际发送者均属于test_kernel。
fn once_payer_target(startup: &mut libruntime::startup::InitialGrants) -> ! {
    use libkernel::ipc::{
        message::{self, MailboxSender},
        packet::Packet,
    };
    use libkernel::shared::{object::Rights, time::Deadline};
    let sender =
        MailboxSender::from_capability(startup.take(0).expect("Once issuer source missing"))
            .map_err(|f| f.error)
            .expect("Once issuer source invalid")
            .0;
    let ready = notification::NotificationSignaler::from_capability(
        startup.take(1).expect("Once issuer ready missing"),
    )
    .map_err(|f| f.error)
    .expect("Once issuer ready invalid")
    .0;
    let mut packet =
        Packet::new(u64::from_le_bytes(*b"ONCEPAY1"), &[]).expect("Once issuer packet failed");
    let mut observations = [None, None, None, None];
    for observation in &mut observations {
        let permit = message::send_once(&sender, Rights::WRITE | Rights::TRANSIT)
            .expect("Once issuance failed");
        packet
            .push(permit.reply.into_capability(), Rights::WRITE)
            .map_err(|f| f.error)
            .expect("Once handoff failed");
        *observation = Some(permit.observation);
    }
    packet
        .try_send(&sender, Deadline::INFINITE)
        .map_err(|f| f.error)
        .expect("Once issuer transport failed");
    ready
        .signal(1)
        .expect("Once issuer completion signal failed");
    loop {
        core::hint::black_box(&observations);
        // SAFETY: 独立发行者保留观察壳，父账本通过原Kill/collector收束。
        unsafe { sys_sleep(1000).expect("Once issuer sleep failed") };
    }
}
