//! 隔离 Building 地址空间中的真实 Tunnel 失败路径与库存守恒自检。
//! 全员 Online、用户调度尚未开始时执行；无全局故障开关，不影响其它进程。

use super::*;
use crate::task::{lifecycle, memory_pool::MemoryPool, proc, resources};
use core::alloc::Layout;
use erhino_shared::{
    memory_pool::MemoryPoolSnapshot,
    proc::{ProcessExitReason, ThreadStartContext},
};

const VA: usize = 0x6000_0000;
const TEST_PAGES: usize = 3;

fn create_request(va: usize, output: usize) -> TunnelCreateRequest {
    TunnelCreateRequest::new(
        (TEST_PAGES * proc::PAGE_SIZE) as u64,
        va as u64,
        output as u64,
        erhino_shared::mem::MemoryPlacement::FixedEmpty,
    )
}

fn attach_request(invitation: Handle, va: usize, output: usize) -> TunnelAttachRequest {
    TunnelAttachRequest::new(
        invitation,
        va as u64,
        output as u64,
        erhino_shared::mem::MemoryPlacement::FixedEmpty,
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Inventory {
    pool: MemoryPoolSnapshot,
    frames: usize,
    metadata: [usize; 16],
}

impl Inventory {
    fn read(pool: &MemoryPool) -> Self {
        Self {
            pool: pool.snapshot(),
            frames: crate::frame::free_frames(),
            metadata: resources::admission_usage(),
        }
    }
}

/// 仅自检使用的真实堆压力 owner；链节点就存在所持分配中，无额外元数据分配。
/// 不改变 allocator 行为，不伪造分配返回值。释放所有节点后才能执行诊断格式化。
struct HeapPressure(*mut usize);

impl HeapPressure {
    fn acquire() -> Self {
        let mut pressure = Self(core::ptr::null_mut());
        for size in [65536, 8192, 1024, 128, 16] {
            let layout = Layout::from_size_align(size, 8).unwrap();
            loop {
                // SAFETY: 合法非零 Layout；返回 null 表示真实 OOM。成功块至少可容纳
                // 两个 usize，独占持有并记录 exact Layout，Drop 恰一次归还。
                let ptr = unsafe { alloc::alloc::alloc(layout) }.cast::<usize>();
                if ptr.is_null() {
                    break;
                }
                unsafe {
                    ptr.write(pressure.0 as usize);
                    ptr.add(1).write(size);
                }
                pressure.0 = ptr;
            }
        }
        pressure
    }
}

impl Drop for HeapPressure {
    fn drop(&mut self) {
        while !self.0.is_null() {
            // SAFETY: acquire 建立的独占链；先取出链接/Layout 再 dealloc，不解引用
            // 已归还内存。所有块 alignment 都为 8，大小按节点保存。
            unsafe {
                let ptr = self.0;
                self.0 = ptr.read() as *mut usize;
                let size = ptr.add(1).read();
                alloc::alloc::dealloc(ptr.cast(), Layout::from_size_align(size, 8).unwrap());
            }
        }
    }
}

fn prepare(connection: &Connection, thread: &Thread, va: usize) -> PreparedMemoryChange {
    let (authorization, permits) = reserve_mapping(connection).expect("Tunnel test permit failed");
    let (plan, pool) = plan_side_mapping(
        connection,
        thread,
        memory_space::MapPlacement::FixedEmpty { usable_start: va },
        authorization,
        permits,
    )
    .unwrap_or_else(|_| panic!("Tunnel test planning failed"));
    let owners =
        proc::fund_table_preflights(&pool, plan.preflights()).expect("Tunnel test funding failed");
    thread
        .process
        .space
        .lock()
        .complete_object_change(plan, owners)
        .unwrap_or_else(|_| panic!("Tunnel test preparation failed"))
}

fn assert_failed(
    result: Result<super::super::wait::WaitPlan, SystemCallError>,
    expected: SystemCallError,
) {
    match result {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("Tunnel failure test unexpectedly committed"),
    }
}

/// 自检只构造 Building fixture，不把私有 Thread 发布给调度器；所有退出经真实终止/drain。
pub(crate) fn run(root: &Arc<MemoryPool>) {
    while crate::deferred_work::drain_current() != 0 {}
    let initial = Inventory::read(root);
    let process = Arc::new(
        Process::new(
            0,
            0,
            Weak::new(),
            resources::ProcessResources::try_new().expect("Tunnel test sponsor failed"),
        )
        .expect("Tunnel test process failed"),
    );
    super::super::process::bind_memory_internal(&process, root.clone())
        .expect("Tunnel test Bind failed");
    process
        .space
        .map_stack()
        .expect("Tunnel test output mapping failed");
    let output = proc::USER_TOP - 64;
    let mut thread = None;
    process
        .lifecycle
        .attach_member(|_, member| {
            let value = Arc::try_new(
                Thread::new_thread(
                    member,
                    &process,
                    ThreadStartContext {
                        entry: 0,
                        stack_pointer: output as u64,
                        arg1: 0,
                        arg2: 0,
                    },
                )
                .map_err(|_| lifecycle::AttachFault::Oom)?,
            )
            .map_err(|_| lifecycle::AttachFault::Oom)?;
            thread = Some(value.clone());
            Ok(value)
        })
        .expect("Tunnel test Thread failed");
    let thread = thread.unwrap();
    let baseline = Inventory::read(root);
    let original = core::mem::replace(
        &mut *process.handles.lock(),
        handle::ProcessHandleTable::with_limit(1),
    );
    assert_failed(
        create(&thread, create_request(VA, output)),
        SystemCallError::ReachLimit,
    );
    let limited = core::mem::replace(&mut *process.handles.lock(), original);
    assert_eq!(limited.len(), 0);
    drop(limited);
    assert_eq!(
        Inventory::read(root),
        baseline,
        "Tunnel Handle reservation failure leaked resources"
    );
    check_geometry(root, &thread);
    for bytes in [0, u64::MAX, (TUNNEL_MAX_PAGES + 1) * proc::PAGE_SIZE as u64] {
        let mut request = create_request(VA, output);
        request.bytes = bytes;
        assert_failed(
            create(&thread, request),
            if bytes == (TUNNEL_MAX_PAGES + 1) * proc::PAGE_SIZE as u64 {
                SystemCallError::ReachLimit
            } else {
                SystemCallError::IllegalArgument
            },
        );
        assert_eq!(Inventory::read(root), baseline);
    }
    for (address, placement, reserved) in [
        (1, 0, 0),
        (VA as u64 + 1, 1, 0),
        (VA as u64, 2, 0),
        (VA as u64, 1, 1),
    ] {
        let mut request = create_request(VA, output);
        request.address = address;
        request.placement = placement;
        request.reserved = reserved;
        assert_failed(create(&thread, request), SystemCallError::IllegalArgument);
        assert_eq!(Inventory::read(root), baseline);
    }
    for (va, destination, expected) in [
        (
            (output & !(proc::PAGE_SIZE - 1)) - (TEST_PAGES - 1) * proc::PAGE_SIZE,
            output,
            SystemCallError::AddressConflict,
        ),
        (VA, 0, SystemCallError::MemoryNotAccessible),
        // Building fixture 无 Running 提交资格；映射完整 Prepare 后必须显式回滚。
        (VA, output, SystemCallError::ObjectClosed),
    ] {
        assert_failed(create(&thread, create_request(va, destination)), expected);
        assert_eq!(
            Inventory::read(root),
            baseline,
            "TunnelCreate failure changed inventory"
        );
        assert_eq!(process.handles.lock().len(), 0);
        assert_eq!(process.space.lock().page_pa(VA), None);
    }

    let connection = new_connection(&thread, TEST_PAGES).expect("Tunnel test connection failed");
    assert!(
        connection.core.backing.projection_capacity() > 1,
        "three-page Tunnel must use multiple power-of-two extents"
    );
    let endpoint = Endpoint::new(
        connection.clone(),
        0,
        resources::MetadataSponsor::reserve_endpoint(process.resources.metadata()).unwrap(),
    )
    .unwrap();
    let invitation = Invitation::new(
        connection.clone(),
        1,
        resources::MetadataSponsor::reserve_invitation(process.resources.metadata()).unwrap(),
    )
    .unwrap();
    // fixture 创建端实际安装 view；Building 从未激活，无需远端失效。
    let mapping = prepare(&connection, &thread, VA);
    let (published, lease) = process.space.lock().commit_view_map(mapping);
    let mut retiring = process
        .space
        .lock()
        .begin_retire_published_change(published);
    while !retiring.advance(&process.space, None) {}
    drop(retiring);
    {
        let mut state = connection.state.lock();
        install_mapping(&mut state, 0, lease);
        state.sides = [
            SideState::Alive(Arc::downgrade(&endpoint)),
            SideState::Invited(Arc::downgrade(&invitation)),
        ];
    }
    let entry = handle::entry(
        Invitation::object_ref(&invitation),
        HandleRole::TunnelInvitation,
        Rights::MAP,
    )
    .unwrap();
    let token = handle::transaction_token().unwrap();
    let reservation = process.handles.lock().reserve(1, token).unwrap();
    let invitation_handle = reservation.handles()[0];
    process
        .handles
        .lock()
        .commit(reservation, alloc::vec![entry])
        .unwrap();
    let attached_baseline = Inventory::read(root);
    let target = VA + 0x4000_0000;
    for (va, destination, expected) in [
        (VA, output, SystemCallError::AddressConflict),
        (target, 0, SystemCallError::MemoryNotAccessible),
        (target, output, SystemCallError::ObjectClosed),
    ] {
        assert_failed(
            attach(&thread, attach_request(invitation_handle, va, destination)),
            expected,
        );
        assert_eq!(
            Inventory::read(root),
            attached_baseline,
            "TunnelAttach failure changed inventory"
        );
        assert!(
            process
                .handles
                .lock()
                .get(invitation_handle, Rights::MAP)
                .is_ok(),
            "failed Attach consumed Invitation"
        );
        assert_eq!(process.space.lock().page_pa(target), None);
        assert_eq!(connection.core.snapshot().write_views, 1);
    }

    fail_resources(
        root,
        &thread,
        &connection,
        invitation_handle,
        output,
        target,
    );

    fail_revoked_output(&thread, &connection, invitation_handle, output, target);
    let departure = thread.departure();
    drop(thread);
    departure.request(super::super::thread::DepartureKind::Terminated);
    drop(departure);
    while crate::deferred_work::drain_current() != 0 {}
    assert!(process.lifecycle.is_reapable());
    while !process.drain_batch(1).1 {}
    drop(process);
    drop(invitation);
    drop(endpoint);
    drop(connection);
    while crate::deferred_work::drain_current() != 0 {}
    assert_eq!(
        Inventory::read(root),
        initial,
        "Tunnel test did not refund all owners"
    );
    log!(
        Memory,
        "Tunnel failure rollback and inventory checks passed"
    );
}

/// 在真实 ledger/PTE/owner 上完成多页发布与撤销，fixture 不进入用户调度。
#[inline(never)]
fn check_geometry(root: &Arc<MemoryPool>, thread: &Thread) {
    let initial = Inventory::read(root);
    let limits = funded_frame::Limits {
        max_pages: 1,
        max_extents: 1,
    };
    let padding = crate::frame::fund_object_backing(root, 1, limits).unwrap();
    let barrier = crate::frame::fund_object_backing(root, 1, limits).unwrap();
    // 持有第二个 claim、归还第一个，在最早库存位置留下不能与 buddy 合并的洞。
    // 三页 funding 先取两页 extent，再取最早单页洞，投影不能假设 PA 连续。
    drop(padding);
    let baseline = Inventory::read(root);
    for pages in [1, 2, 3, TUNNEL_MAX_PAGES as usize] {
        let connection = new_connection(thread, pages).expect("Tunnel geometry backing failed");
        let (authorization, permits) = reserve_mapping(&connection).unwrap();
        let (plan, pool) = plan_side_mapping(
            &connection,
            thread,
            memory_space::MapPlacement::Anywhere,
            authorization,
            permits,
        )
        .unwrap_or_else(|_| panic!("Tunnel geometry planning failed"));
        let owners = proc::fund_table_preflights(&pool, plan.preflights()).unwrap();
        let mapping = thread
            .process
            .space
            .lock()
            .complete_object_change(plan, owners)
            .unwrap_or_else(|_| panic!("Tunnel geometry prepare failed"));
        let lease = mapping.published_lease();
        assert_eq!(lease.range.pages(), pages);
        let pressure = (pages == 3).then(HeapPressure::acquire);
        let (published, installed) = thread.process.space.lock().commit_view_map(mapping);
        assert_eq!(installed, lease);
        let mut retiring = thread
            .process
            .space
            .lock()
            .begin_retire_published_change(published);
        while !retiring.advance(&thread.process.space, None) {}
        drop(retiring);
        drop(pressure);
        let mut spans = Vec::with_capacity(connection.core.backing.projection_capacity());
        connection.core.backing.project(0, pages, &mut spans);
        if pages == 3 {
            assert!(
                spans
                    .windows(2)
                    .any(|pair| pair[0].0.0 + pair[0].1 != pair[1].0.0),
                "fragmented Tunnel fixture unexpectedly projected contiguous physical pages"
            );
        }
        let mut cursor = lease.range.start();
        for (base, count) in spans {
            for page in 0..count {
                assert_eq!(
                    thread.process.space.lock().page_pa(cursor),
                    Some((base.0 + page) * proc::PAGE_SIZE)
                );
                cursor += proc::PAGE_SIZE;
            }
        }
        assert_eq!(cursor, lease.range.end());
        let plan = thread
            .process
            .space
            .lock()
            .plan_object_unmap(lease)
            .unwrap();
        let owners = proc::fund_table_preflights(&pool, plan.preflights()).unwrap();
        let unmap = thread
            .process
            .space
            .lock()
            .complete_memory_change(plan, owners)
            .unwrap_or_else(|_| panic!("Tunnel geometry Unmap prepare failed"));
        let pressure = (pages == 3).then(HeapPressure::acquire);
        let published = thread.process.space.lock().commit_change(unmap);
        let mut retiring = thread
            .process
            .space
            .lock()
            .begin_retire_published_change(published);
        while !retiring.advance(&thread.process.space, None) {}
        drop(retiring);
        drop(pressure);
        for page in 0..pages {
            assert_eq!(
                thread
                    .process
                    .space
                    .lock()
                    .page_pa(lease.range.start() + page * proc::PAGE_SIZE),
                None
            );
        }
        drop(connection);
        assert_eq!(
            Inventory::read(root),
            baseline,
            "Tunnel geometry did not refund exact inventory"
        );
    }
    drop(barrier);
    assert_eq!(
        Inventory::read(root),
        initial,
        "Tunnel fragmentation fixture did not refund inventory"
    );
    log!(
        Memory,
        "Tunnel multi-page geometry and projection checks passed"
    );
}

#[inline(never)]
fn fail_revoked_output(
    thread: &Thread,
    connection: &Connection,
    invitation_handle: Handle,
    output: usize,
    target: usize,
) {
    let process = &thread.process;
    // 初次输出验证后，另一笔真实 Unmap 撤销输出页；固定该交错再完成 Tunnel
    // Prepare，最终写回复检必须杀进程且由生产 abandon_mapping 归还所有准备资源。
    process
        .space
        .lock()
        .check_range(output, core::mem::size_of::<Handle>(), true)
        .unwrap();
    let mut staged = Vec::with_capacity(1);
    assert!(process.lifecycle.enter_building_op());
    process
        .lifecycle
        .begin_running(1, &mut staged)
        .expect("Tunnel test Start failed");
    drop(staged);
    let slot = crate::hart::current().slot();
    assert!(matches!(
        process
            .lifecycle
            .enter_running_if(thread.member(), slot, || true),
        lifecycle::EnterRunning::Entered
    ));
    struct UnpublishedCompletion;
    impl crate::remote_call::Completion for UnpublishedCompletion {
        fn complete(self: Arc<Self>) {
            panic!("reserved test Remote batch must never publish");
        }
    }
    let sink: Arc<dyn crate::remote_call::Completion> = Arc::new(UnpublishedCompletion);
    let mut batches = Vec::new();
    loop {
        match crate::remote_call::reserve(1u64 << slot, sink.clone()) {
            Ok(batch) => batches.push(batch),
            Err(crate::remote_call::ReserveError::Busy) => break,
            Err(_) => panic!("Remote pressure fixture failed before reaching capacity"),
        }
    }
    assert!(!batches.is_empty());
    let pool = { Arc::clone(process.space.lock().pool()) };
    let inventory = Inventory::read(&pool);
    assert_failed(
        attach(thread, attach_request(invitation_handle, target, output)),
        SystemCallError::ObjectBusy,
    );
    assert_eq!(
        Inventory::read(&pool),
        inventory,
        "Tunnel Remote reservation failure leaked resources"
    );
    assert!(
        process
            .handles
            .lock()
            .get(invitation_handle, Rights::MAP)
            .is_ok()
    );
    drop(batches);
    drop(sink);
    assert!(process.lifecycle.clear_active_if(slot, || true));
    let unmap = proc::memory_unmap(
        thread,
        (output & !(proc::PAGE_SIZE - 1)) as u64,
        proc::PAGE_SIZE as u64,
    )
    .expect("Tunnel test output revocation failed");
    while crate::deferred_work::drain_current() != 0 {}
    drop(unmap);
    let prepared = prepare(connection, thread, target);
    let slot = crate::hart::current().slot();
    assert!(matches!(
        process
            .lifecycle
            .enter_running_if(thread.member(), slot, || true),
        lifecycle::EnterRunning::Entered
    ));
    let result = {
        let mut space = process.space.lock();
        // SAFETY: Handle 无 padding；输出页已撤销，校验必须在实际 store 前返回错误。
        unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &invitation_handle) }
    };
    assert_eq!(result, Err(SystemCallError::MemoryNotAccessible));
    abandon_mapping(&process.space, connection, prepared);
    thread.finish_output_termination();
    assert_eq!(connection.core.snapshot().write_views, 1);
    assert_eq!(process.space.lock().page_pa(target), None);
    assert!(
        process
            .handles
            .lock()
            .get(invitation_handle, Rights::MAP)
            .is_ok()
    );
    assert_eq!(process.lifecycle.snapshot().1, ProcessExitReason::Fault);

    assert!(process.lifecycle.clear_active_if(slot, || true));
}

fn fail_admission<P>(
    root: &Arc<MemoryPool>,
    thread: &Thread,
    invitation: Handle,
    output: usize,
    target: usize,
    mut reserve: impl FnMut() -> Result<P, SystemCallError>,
    attach_fails: bool,
) {
    let initial = Inventory::read(root);
    let mut held = Vec::new();
    while let Ok(permit) = reserve() {
        held.push(permit);
    }
    let exhausted = Inventory::read(root);
    assert_failed(
        create(thread, create_request(target, output)),
        SystemCallError::ReachLimit,
    );
    if attach_fails {
        assert_failed(
            attach(thread, attach_request(invitation, target, output)),
            SystemCallError::ReachLimit,
        );
        assert!(
            thread
                .process
                .handles
                .lock()
                .get(invitation, Rights::MAP)
                .is_ok()
        );
    }
    assert_eq!(
        Inventory::read(root),
        exhausted,
        "Tunnel admission failure leaked resources"
    );
    drop(held);
    assert_eq!(Inventory::read(root), initial);
}

#[inline(never)]
fn fail_resources(
    root: &Arc<MemoryPool>,
    thread: &Thread,
    connection: &Connection,
    invitation_handle: Handle,
    output: usize,
    target: usize,
) {
    let process = &thread.process;
    let attached_baseline = Inventory::read(root);
    let sponsor = process.resources.metadata();
    macro_rules! exhaust {
        ($reserve:ident, $attach:expr) => {
            fail_admission(
                root,
                thread,
                invitation_handle,
                output,
                target,
                || resources::MetadataSponsor::$reserve(sponsor),
                $attach,
            );
        };
    }
    exhaust!(reserve_connection, false);
    exhaust!(reserve_object_backing, false);
    exhaust!(reserve_endpoint, true);
    exhaust!(reserve_invitation, false);
    exhaust!(reserve_object_view, true);
    for component in 0..3 {
        fail_admission(
            root,
            thread,
            invitation_handle,
            output,
            target,
            || {
                let (change, wait, remote) =
                    resources::MetadataSponsor::reserve_memory_operation(sponsor)?.into_parts();
                // 保持一种独立准入，归还另外两种，覆盖三段实际预留失败。
                Ok(match component {
                    0 => (Some(change), None, None),
                    1 => (None, Some(wait), None),
                    _ => (None, None, Some(remote)),
                })
            },
            true,
        );
    }
    fail_admission(
        root,
        thread,
        invitation_handle,
        output,
        target,
        || crate::deferred_work::reserve().map_err(|_| SystemCallError::ReachLimit),
        true,
    );
    // 额度耗尽发生在 planner 之后、表页 funding 之前，实际 reserve 失败必须归还 permit。
    let (authorization, permits) = reserve_mapping(connection).unwrap();
    let (plan, pool) = plan_side_mapping(
        connection,
        thread,
        memory_space::MapPlacement::FixedEmpty {
            usable_start: target,
        },
        authorization,
        permits,
    )
    .unwrap_or_else(|_| panic!("Tunnel quota test planning failed"));
    let held = MemoryPool::reserve_charge(root, root.snapshot().available as usize).unwrap();
    let error = proc::fund_table_preflights(&pool, plan.preflights())
        .err()
        .expect("Tunnel table funding unexpectedly succeeded");
    drop(held);
    assert!(matches!(error, proc::SpaceError::QuotaExceeded));
    let mut reclaimed = process.space.lock().rollback_memory_change_plan(plan);
    let permits = reclaimed.take_permits();
    drop(reclaimed);
    cancel_writes(connection, permits);
    assert_eq!(Inventory::read(root), attached_baseline);

    // 堆被真实占满时，投影/owner Prepare 返回 NoFrame，生产 Attach 返回 OutOfMemory。
    let (authorization, permits) = reserve_mapping(connection).unwrap();
    let pressure = HeapPressure::acquire();
    let failure = plan_side_mapping(
        connection,
        thread,
        memory_space::MapPlacement::FixedEmpty {
            usable_start: target,
        },
        authorization,
        permits,
    )
    .err()
    .expect("Tunnel metadata preparation unexpectedly succeeded");
    let attach_result = attach(thread, attach_request(invitation_handle, target, output));
    drop(pressure);
    assert!(matches!(failure.error, proc::SpaceError::NoFrame));
    cancel_writes(connection, failure.permits);
    assert_failed(attach_result, SystemCallError::OutOfMemory);
    assert_eq!(Inventory::read(root), attached_baseline);
    assert!(
        process
            .handles
            .lock()
            .get(invitation_handle, Rights::MAP)
            .is_ok()
    );
}
