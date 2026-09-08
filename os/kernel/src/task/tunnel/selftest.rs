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
    let (plan, pool) = plan_side_mapping(connection, thread, va, authorization, permits)
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
    for (va, destination, expected) in [
        (
            output & !(proc::PAGE_SIZE - 1),
            output,
            SystemCallError::AddressConflict,
        ),
        (VA, 0, SystemCallError::MemoryNotAccessible),
        // Building fixture 无 Running 提交资格；映射完整 Prepare 后必须显式回滚。
        (VA, output, SystemCallError::ObjectClosed),
    ] {
        assert_failed(create(&thread, va, destination), expected);
        assert_eq!(
            Inventory::read(root),
            baseline,
            "TunnelCreate failure changed inventory"
        );
        assert_eq!(process.handles.lock().len(), 0);
        assert_eq!(process.space.lock().page_pa(VA), None);
    }

    let connection = new_connection(&thread).expect("Tunnel test connection failed");
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
            attach(&thread, invitation_handle, va, destination),
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
    // 额度耗尽发生在 planner 之后、表页 funding 之前，实际 reserve 失败必须归还 permit。
    let (authorization, permits) = reserve_mapping(connection).unwrap();
    let (plan, pool) = plan_side_mapping(connection, thread, target, authorization, permits)
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
    let failure = plan_side_mapping(connection, thread, target, authorization, permits)
        .err()
        .expect("Tunnel metadata preparation unexpectedly succeeded");
    let attach_result = attach(thread, invitation_handle, target, output);
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
