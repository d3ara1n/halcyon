//! 真实平台 DMA 角色与 RAM owner 验收；地址只读取，不交给设备。
use erhino_shared::{
    call::SystemCallError as Error,
    dma::*,
    memory_object::MemoryObjectState,
    object::{Handle, ObjectSignals, Rights},
    proc::PROCESS_PAGE_SIZE as PAGE,
    wait_set::MemberRequest,
};
use libkernel::{
    dma::{DmaDomainControl, DmaMapping},
    ipc::{object, wait, wait_set::WaitSet},
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
};
const FULL: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::CREATE.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
const MAP: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
const REFUND_MILLIS: u64 = 60_000;
/// 每个跨 reset 保留 fixture 固定保留的 DMA 映射页数（backing 两页）。
pub(super) const RETAINED_PAGES_PER_FIXTURE: u64 = 2;

fn done(handle: Handle) -> Result<(), &'static str> {
    let observed =
        wait::wait_one(handle, ObjectSignals::DONE, 0).map_err(|_| "DMA DONE wait failed")?;
    if !observed.observed.contains(ObjectSignals::DONE) {
        return Err("DMA wait omitted DONE");
    }
    Ok(())
}
fn root(handle: Handle) -> Result<DmaDomainControl, &'static str> {
    let handle = object::duplicate(handle, FULL).map_err(|_| "DMA root duplicate failed")?;
    // SAFETY: 来自已核平台根的新能力，移交唯一关闭责任。
    Ok(unsafe { DmaDomainControl::from_handle(handle) })
}
fn expose(mapping: &DmaMapping) -> Result<(), &'static str> {
    let total = mapping
        .query()
        .map_err(|_| "DMA mapping query failed")?
        .bytes;
    let mut offset = 0;
    let mut segments = [Segment::default(); 1];
    while offset < total {
        let result = mapping
            .read_segments(offset as usize, &mut segments)
            .map_err(|_| "DMA segment read failed")?;
        if result.count != 1
            || segments[0].address == 0
            || segments[0].bytes == 0
            || result.next_offset != offset + segments[0].bytes
            || result.next_offset > total
        {
            return Err("DMA segment geometry invalid");
        }
        offset = result.next_offset;
    }
    Ok(())
}
pub(super) fn run(handle: Handle) -> Result<(), &'static str> {
    let root = root(handle)?;
    let scope = root
        .create_control(FULL)
        .map_err(|_| "DMA scope create failed")?;
    let middle = scope
        .create_control(FULL)
        .map_err(|_| "DMA middle control create failed")?;
    let usage = middle
        .create_use(FULL)
        .map_err(|_| "DMA use create failed")?;
    let other = scope
        .create_use(FULL)
        .map_err(|_| "DMA sibling use create failed")?;
    let memory = MemoryObject::create(8 * PAGE).map_err(|_| "DMA memory create failed")?;
    let cpu = MappedRegion::map_object(
        &memory,
        0,
        8 * PAGE,
        0,
        0,
        erhino_shared::mem::MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "DMA CPU mapping failed")?;
    let mapping = usage
        .pin(&memory, PAGE, 6 * PAGE, DEVICE_WRITES, MAP)
        .map_err(|_| "DMA Pin failed")?;
    if memory
        .query()
        .map_err(|_| "DMA memory query failed")?
        .write_permits
        != 2
    {
        return Err("DMA write permission was not independent of CPU View");
    }
    expose(&mapping)?;
    cpu.unmap().map_err(|_| "DMA CPU Unmap failed")?;
    memory.seal().map_err(|_| "DMA memory Seal failed")?;
    let snapshot = memory.query().map_err(|_| "DMA sealing query failed")?;
    if snapshot.state != MemoryObjectState::Sealing as u32 || snapshot.write_permits != 1 {
        return Err("DMA did not block executable publication");
    }
    if !matches!(
        other.pin(&memory, 0, PAGE, DEVICE_WRITES, MAP),
        Err(Error::ObjectBusy)
    ) {
        return Err("sealed memory admitted a DMA writer");
    }
    // SAFETY: 本验收从未把地址交给硬件；这里只核不同 use 的权限拒绝。
    if unsafe { other.retire(&mapping) } != Err(Error::RightsDenied) {
        return Err("DMA sibling retired another use's mapping");
    }
    let observer = WaitSet::create(1).map_err(|_| "DMA lineage observer create failed")?;
    observer
        .register(MemberRequest::new(middle.handle(), ObjectSignals::DONE, 1))
        .map_err(|_| "DMA lineage observe failed")?;
    drop(middle);
    scope.seal().map_err(|_| "DMA domain Seal failed")?;
    let mut denied_segments = [Segment {
        address: u64::MAX,
        bytes: u64::MAX,
    }];
    // 祖先封口传播可使分步准入票失效而返回 Busy；两种拒绝均不得写出地址。
    let denied_read = mapping.read_segments(0, &mut denied_segments);
    if !matches!(denied_read, Err(Error::ObjectClosed | Error::ObjectBusy))
        || denied_segments[0].address != u64::MAX
        || denied_segments[0].bytes != u64::MAX
    {
        libkernel::debug!("DMA sealed read result: {:?}", denied_read);
        return Err("sealed DMA domain did not reject address delivery");
    }
    if mapping
        .query()
        .map_err(|_| "DMA retained mapping query failed")?
        .phase
        != MappingPhase::MayBeExposed as u32
    {
        return Err("DMA Seal retired an exposed mapping");
    }
    // SAFETY: 地址仅被此验收读取，没有设备访问，也不会再次提交。
    unsafe { scope.retire(&mapping) }.map_err(|_| "DMA ancestor Retire failed")?;
    done(mapping.handle())?;
    done(scope.handle())?;
    observer
        .wait(libkernel::time::Deadline::INFINITE)
        .map_err(|_| "DMA middle completion failed")?;
    // 中间 control 已关闭且完成；保留的不可变祖先身份仍能核对重复 Retire。
    unsafe { scope.retire(&mapping) }.map_err(|_| "DMA retired lineage authorization failed")?;
    let observed = wait::wait_one(memory.handle(), ObjectSignals::EXECUTABLE, 0)
        .map_err(|_| "DMA executable wait failed")?;
    if !observed.observed.contains(ObjectSignals::EXECUTABLE)
        || memory
            .query()
            .map_err(|_| "DMA retired memory query failed")?
            .write_permits
            != 0
    {
        return Err("DMA retirement retained write permission");
    }
    let plain = root
        .create_use(FULL)
        .map_err(|_| "DMA cancellation use failed")?;
    let memory = MemoryObject::create(PAGE).map_err(|_| "DMA cancellation memory failed")?;
    let unexposed = plain
        .pin(&memory, 0, PAGE, DEVICE_WRITES, MAP)
        .map_err(|_| "DMA unexposed Pin failed")?;
    let cancelled = WaitSet::create(1).map_err(|_| "DMA cancellation observer failed")?;
    cancelled
        .register(MemberRequest::new(
            unexposed.handle(),
            ObjectSignals::DONE,
            2,
        ))
        .map_err(|_| "DMA cancellation register failed")?;
    drop(unexposed);
    cancelled
        .wait(libkernel::time::Deadline::INFINITE)
        .map_err(|_| "DMA last Close did not cancel unexposed Pin")?;
    // 集合 READABLE/CLOSED 只提示最后 Close，写许可由原退休责任异步释放。
    // Seal 仅封新写；实际 EXECUTABLE 才证明原许可已释放，不代替 mapping/Pool DONE。
    memory
        .seal()
        .map_err(|_| "DMA cancellation memory Seal failed")?;
    let executable = wait::wait_one(memory.handle(), ObjectSignals::EXECUTABLE, REFUND_MILLIS)
        .map_err(|_| "DMA cancellation executable wait failed")?;
    if !executable.observed.contains(ObjectSignals::EXECUTABLE) {
        return Err("DMA cancellation did not release write permission before deadline");
    }
    if memory
        .query()
        .map_err(|_| "DMA cancellation memory query failed")?
        .write_permits
        != 0
    {
        return Err("DMA cancellation retained write permission");
    }
    plain.seal().map_err(|_| "DMA cancellation Seal failed")?;
    done(plain.handle())?;
    libkernel::debug!(
        "DMA platform acceptance passed: pin, SG, independent write permission, Seal, lineage Retire and cancellation"
    );
    Ok(())
}

/// 未暴露 Pin 随目标退出取消；已暴露及输出 fault 的 Pin 保留到本次隔离 QEMU 复位。
pub(super) fn process_retirement(
    handle: Handle,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    let modes: &[&[u8]] = if crate::mode().has(crate::Mode::DMA_INSTALL_FAILURE) {
        &[b"dma-pin-install-failure"]
    } else if crate::mode().has(crate::Mode::DMA_QUOTA) {
        &[
            b"dma-unexposed",
            b"dma-exposed",
            b"dma-output-fault",
            b"dma-pin-quota",
        ]
    } else if crate::mode().has(crate::Mode::DMA_SHARED) {
        &[
            b"dma-unexposed",
            b"dma-exposed",
            b"dma-output-fault",
            b"dma-shared",
        ]
    } else if crate::mode().has(crate::Mode::DMA_RACES) {
        &[
            b"dma-unexposed",
            b"dma-exposed",
            b"dma-output-fault",
            b"dma-pin-before",
            b"dma-pin-after",
            b"dma-pin-exit",
            b"dma-reader-retire",
        ]
    } else {
        &[b"dma-unexposed", b"dma-exposed", b"dma-output-fault"]
    };
    for &mode in modes {
        if matches!(mode, b"dma-exposed" | b"dma-output-fault") {
            super::startup::retained_fixture(job, mode)?;
        } else {
            process_fixture(handle, job, image, mode)?;
        }
    }
    libkernel::debug!(
        "DMA process retirement acceptance passed: unexposed cancellation, exposed retention and output fault"
    );
    Ok(())
}

pub(super) fn process_fixture(
    handle: Handle,
    job: Handle,
    image: &[u8],
    mode: &[u8],
) -> Result<(), &'static str> {
    use erhino_shared::proc::{
        HandleGrant, ProcessFaultCode, ProcessState, TerminationCause, TerminationDetail,
    };
    use libkernel::{ipc::notification, process};
    use libprocess::{GrantOutcome, SpawnRequest, spawn};
    let root = root(handle)?;
    let race = matches!(
        mode,
        b"dma-pin-before"
            | b"dma-pin-after"
            | b"dma-pin-exit"
            | b"dma-reader-retire"
            | b"dma-shared"
            | b"dma-pin-quota"
            | b"dma-pin-install-failure"
    );
    let pool = if race {
        Some(
            libkernel::memory_pool::derive(
                super::memory_pool(),
                libdriver::boot::WORKER_POOL,
                if mode == b"dma-pin-quota" {
                    Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::GRANT
                } else {
                    Rights::READ | Rights::CREATE
                },
            )
            .map_err(|_| "DMA race Pool admission failed")?,
        )
    } else {
        None
    };
    let scope = root
        .create_control(FULL)
        .map_err(|_| "DMA process scope failed")?;
    let usage = scope
        .create_use(FULL)
        .map_err(|_| "DMA process use failed")?;
    let memory = MemoryObject::create(2 * PAGE).map_err(|_| "DMA process memory failed")?;
    let grant = object::duplicate(
        memory.handle(),
        Rights::READ | Rights::WRITE | Rights::GRANT,
    )
    .map_err(|_| "DMA memory grant failed")?;
    // SAFETY: 新 duplicate 的 MemoryObject 关闭责任移入，随后按 Spawn 结果交接。
    let granted_memory = unsafe { MemoryObject::from_handle(grant) };
    let ready = notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT)
        .map_err(|_| "DMA process readiness failed")?;
    let observer = WaitSet::create(1).map_err(|_| "DMA process observer failed")?;
    observer
        .register(MemberRequest::new(usage.handle(), ObjectSignals::DONE, 1))
        .map_err(|_| "DMA process registration failed")?;
    let (mut gate_owner, gate_signaler) = if race {
        let gate =
            notification::create(Rights::READ | Rights::WAIT | Rights::GRANT, Rights::SIGNAL)
                .map_err(|_| "DMA race gate create failed")?;
        (Some(gate.owner), Some(gate.peer))
    } else {
        (None, None)
    };
    let mut granted_pool = if mode == b"dma-pin-quota" {
        let handle = object::duplicate(
            pool.as_ref().ok_or("DMA quota Pool missing")?.as_handle(),
            Rights::READ | Rights::GRANT,
        )
        .map_err(|_| "DMA quota Pool observer alias failed")?;
        // SAFETY: 已核派生Pool母本的新缩权alias，唯一关闭责任转入，随后按GrantOutcome交接。
        Some(unsafe { libkernel::memory_pool::MemoryPool::from_handle(handle) })
    } else {
        None
    };
    let mut grants = alloc::vec![
        HandleGrant {
            handle: usage.handle(),
            rights: FULL & !Rights::GRANT,
        },
        HandleGrant {
            handle: granted_memory.handle(),
            rights: Rights::READ | Rights::WRITE,
        },
        HandleGrant {
            handle: ready.peer.as_handle(),
            rights: Rights::SIGNAL,
        },
    ];
    if let Some(gate) = &gate_owner {
        grants.push(HandleGrant {
            handle: gate.as_handle(),
            rights: Rights::READ | Rights::WAIT,
        });
    }
    if let Some(pool) = &granted_pool {
        grants.push(HandleGrant {
            handle: pool.as_handle(),
            rights: Rights::READ,
        });
    }
    let started = match spawn(SpawnRequest {
        memory_pool: pool
            .as_ref()
            .map_or(super::memory_pool(), |pool| pool.as_handle()),
        parent: super::current_process(),
        job,
        image,
        payload: mode,
        grants: &grants,
        control_rights: super::SUPERVISOR_RIGHTS,
    }) {
        Ok(started) => {
            usage.into_handle();
            granted_memory.into_handle();
            ready.peer.into_raw();
            if let Some(gate) = gate_owner.take() {
                gate.into_raw();
            }
            if let Some(pool) = granted_pool.take() {
                pool.into_handle();
            }
            started
        }
        Err(failure) => {
            if failure.grants == GrantOutcome::Consumed {
                usage.into_handle();
                granted_memory.into_handle();
                ready.peer.into_raw();
                if let Some(gate) = gate_owner.take() {
                    gate.into_raw();
                }
                if let Some(pool) = granted_pool.take() {
                    pool.into_handle();
                }
            }
            return Err("DMA target spawn failed");
        }
    };
    let label = core::str::from_utf8(mode).map_err(|_| "DMA target mode is invalid")?;
    if race {
        libkernel::debug!(
            "DMA race target admitted: mode={}, pid={}, scope={}, memory={}, identity={}",
            label,
            started.pid,
            scope.handle().raw(),
            memory.handle().raw(),
            memory
                .query()
                .map_err(|_| "DMA race identity query failed")?
                .identity
        );
        if mode == b"dma-pin-install-failure" {
            libkernel::debug!(
                "DMA install target Pool: pid={}, pool={}",
                started.pid,
                pool.as_ref()
                    .ok_or("DMA install Pool missing")?
                    .query()
                    .map_err(|_| "DMA install Pool identity query failed")?
                    .identity
            );
        }
    }
    let mut shared_parent = None;
    let result = (|| {
        let observed = wait::wait_one(ready.owner.as_handle(), ObjectSignals::READABLE, 0)
            .map_err(|_| "DMA target readiness wait failed")?;
        if !observed.observed.contains(ObjectSignals::READABLE) {
            return Err("DMA target did not publish readiness");
        }
        if super::startup::retention_child() {
            super::startup::log_retention("pinned");
        }
        if let Some(pool) = &pool {
            ready
                .owner
                .take(1)
                .map_err(|_| "DMA race readiness consume failed")?;
            log_pool(label, "before-pin", pool)?;
            gate_signaler
                .as_ref()
                .ok_or("DMA race gate is absent")?
                .signal(1)
                .map_err(|_| "DMA race start signal failed")?;
            if matches!(mode, b"dma-pin-before" | b"dma-pin-after") {
                memory.seal().map_err(|_| "DMA race Seal failed")?;
                let snapshot = memory.query().map_err(|_| "DMA race sealed query failed")?;
                let expected = if mode == b"dma-pin-before" {
                    (MemoryObjectState::Executable, 0)
                } else {
                    (MemoryObjectState::Sealing, 1)
                };
                if snapshot.state != expected.0 as u32 || snapshot.write_permits != expected.1 {
                    return Err("DMA Pin/Seal ordering window was missed");
                }
                libkernel::debug!(
                    "DMA race Seal committed: mode={}, state={}, writers={}",
                    label,
                    snapshot.state,
                    snapshot.write_permits
                );
            }
            if mode != b"dma-pin-exit" {
                let result = wait::wait_one(ready.owner.as_handle(), ObjectSignals::READABLE, 0)
                    .map_err(|_| "DMA race result wait failed")?;
                if !result.observed.contains(ObjectSignals::READABLE)
                    || ready
                        .owner
                        .take(2)
                        .map_err(|_| "DMA race result consume failed")?
                        != 2
                {
                    return Err("DMA race result is missing");
                }
            }
            if mode == b"dma-shared" {
                let end = libkernel::time::timeout_millis(REFUND_MILLIS)
                    .map_err(|_| "DMA shared stage deadline failed")?;
                let parent = libkernel::memory_pool::query(super::memory_pool())
                    .map_err(|_| "DMA shared parent Pool query failed")?;
                let object = memory
                    .query()
                    .map_err(|_| "DMA shared object query failed")?;
                if object.state != MemoryObjectState::Mutable as u32 || object.write_permits != 2 {
                    return Err("DMA shared mappings did not hold two independent writers");
                }
                let pages = object.bytes / PAGE as u64;
                let expected_parent = parent
                    .allocated
                    .checked_sub(pages)
                    .ok_or("DMA shared object charge exceeds parent allocation")?;
                shared_parent = Some(expected_parent);
                let mut previous = pool
                    .query()
                    .map_err(|_| "DMA shared pair Pool query failed")?
                    .metadata
                    .allocated;
                log_pool(label, "both-pins", pool)?;
                libkernel::debug!(
                    "DMA shared parent RAM: identity={}, phase=both-pins, pool={}, allocated_pages={}, object_pages={}",
                    object.identity,
                    parent.identity,
                    parent.allocated,
                    pages
                );
                memory.seal().map_err(|_| "DMA shared Seal failed")?;
                gate_signaler
                    .as_ref()
                    .ok_or("DMA shared gate missing")?
                    .signal(2)
                    .map_err(|_| "DMA shared first retirement start failed")?;
                for (bits, writers, phase) in [(4, 1, "first-retired"), (8, 0, "second-retired")] {
                    wait_stage(&ready.owner, bits, end)?;
                    let current = loop {
                        let snapshot = pool
                            .query()
                            .map_err(|_| "DMA shared partial refund query failed")?;
                        if snapshot.metadata.allocated < previous {
                            break snapshot;
                        }
                        if libkernel::time::expired(end)
                            .map_err(|_| "DMA shared refund clock failed")?
                        {
                            return Err("DMA retired mapping metadata did not refund");
                        }
                        libkernel::thread::yield_now()
                            .map_err(|_| "DMA shared refund yield failed")?;
                    };
                    let snapshot = memory
                        .query()
                        .map_err(|_| "DMA shared surviving object query failed")?;
                    let state = if writers == 0 {
                        MemoryObjectState::Executable
                    } else {
                        MemoryObjectState::Sealing
                    };
                    if snapshot.write_permits != writers || snapshot.state != state as u32 {
                        return Err("DMA shared writer retirement was not independent");
                    }
                    let remaining = libkernel::memory_pool::query(super::memory_pool())
                        .map_err(|_| "DMA shared surviving parent Pool query failed")?;
                    if remaining.allocated != parent.allocated {
                        return Err("DMA Pin retirement changed still-owned parent RAM charge");
                    }
                    libkernel::debug!(
                        "DMA shared partial refund: phase={}, metadata_before={}, metadata_after={}, parent_pages={}, writers={}",
                        phase,
                        previous,
                        current.metadata.allocated,
                        remaining.allocated,
                        writers
                    );
                    previous = current.metadata.allocated;
                    gate_signaler
                        .as_ref()
                        .ok_or("DMA shared gate missing")?
                        .signal(bits)
                        .map_err(|_| "DMA shared next stage release failed")?;
                }
                wait_stage(&ready.owner, 2, end)?;
            }
        }
        let fault = mode == b"dma-output-fault";
        if !fault {
            let alive =
                process::query(started.control).map_err(|_| "DMA target liveness query failed")?;
            if alive.state != ProcessState::Running as u32
                || alive.cause != TerminationCause::None as u32
                || alive.detail != TerminationDetail::NONE
            {
                return Err("DMA target exited before parent Kill");
            }
            if !race
                && memory
                    .query()
                    .map_err(|_| "DMA live write count failed")?
                    .write_permits
                    != 2
            {
                return Err("DMA target did not hold CPU and device writers");
            }
            process::kill(started.control).map_err(|_| "DMA target Kill failed")?;
        }
        process::wait_done(started.control).map_err(|_| "DMA target DONE failed")?;
        let status =
            process::query(started.control).map_err(|_| "DMA target terminal query failed")?;
        if status.state != ProcessState::Dead as u32 {
            return Err("DMA target did not reach Dead");
        }
        if fault {
            if status.cause != TerminationCause::Faulted as u32
                || status.detail.word0 >> 32 != ProcessFaultCode::UserAccess as u64
                || status.detail.word1 != u64::MAX
            {
                return Err("DMA output fault had the wrong terminal cause");
            }
        } else if status.cause != TerminationCause::Killed as u32
            || status.detail != TerminationDetail::PROCESS_CONTROL_KILL
        {
            return Err("DMA target did not exit through parent Kill");
        }
        memory.seal().map_err(|_| "DMA post-exit Seal failed")?;
        scope
            .seal()
            .map_err(|_| "DMA post-exit domain Seal failed")?;
        if mode == b"dma-unexposed" || race {
            observer
                .wait(libkernel::time::Deadline::INFINITE)
                .map_err(|_| "DMA unexposed exit cleanup failed")?;
            done(scope.handle())?;
            let snapshot = memory
                .query()
                .map_err(|_| "DMA cancelled exit query failed")?;
            if snapshot.write_permits != 0 || snapshot.state != MemoryObjectState::Executable as u32
            {
                return Err("DMA unexposed exit retained a writer");
            }
            if race {
                let snapshot = scope.query().map_err(|_| "DMA race domain query failed")?;
                if snapshot.flags & DONE == 0 || snapshot.members != 0 {
                    return Err("DMA race domain retained an unfinished mapping");
                }
                log_pool(
                    label,
                    "cpu-done",
                    pool.as_ref().ok_or("DMA race Pool is absent")?,
                )?;
                libkernel::debug!("DMA race scope DONE: mode={}, members=0, writer=0", label);
            }
        } else {
            let snapshot = memory
                .query()
                .map_err(|_| "DMA retained exit query failed")?;
            if snapshot.write_permits != 1 || snapshot.state != MemoryObjectState::Sealing as u32 {
                return Err("DMA process exit released an exposed pin");
            }
            if scope
                .query()
                .map_err(|_| "DMA retained domain query failed")?
                .flags
                & DONE
                != 0
            {
                return Err("DMA retained domain reported DONE");
            }
            libkernel::debug!(
                "DMA isolated retention confirmed: mode={}, pages={}, writer=1",
                if fault {
                    "output-fault"
                } else {
                    "process-exit"
                },
                RETAINED_PAGES_PER_FIXTURE
            );
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = process::kill(started.control);
        let _ = process::wait_done(started.control);
    }
    // SAFETY: 唯一父方 ProcessControl，目标已经收束。
    unsafe { object::close(started.control) }.map_err(|_| "DMA target control close failed")?;
    result?;
    // 父MemoryObject仍可能保留由子Pin付款的切界节点；真实对象关闭后才要求子账全退。
    drop(memory);
    if let Some(pool) = &pool {
        let end = libkernel::time::timeout_millis(REFUND_MILLIS)
            .map_err(|_| "DMA race refund deadline failed")?;
        loop {
            let snapshot = pool.query().map_err(|_| "DMA race refund query failed")?;
            if snapshot.allocated == 0
                && snapshot.delegated == 0
                && snapshot.reserved == 0
                && snapshot.metadata.allocated == 0
                && snapshot.metadata.delegated == 0
                && snapshot.metadata.reserved == 0
            {
                break;
            }
            if libkernel::time::expired(end).map_err(|_| "DMA race refund clock failed")? {
                return Err("DMA race Pool did not refund");
            }
            libkernel::thread::yield_now().map_err(|_| "DMA race refund yield failed")?;
        }
        log_pool(label, "closed", pool)?;
    }
    if let Some(expected) = shared_parent {
        let end = libkernel::time::timeout_millis(REFUND_MILLIS)
            .map_err(|_| "DMA shared parent refund deadline failed")?;
        loop {
            let parent = libkernel::memory_pool::query(super::memory_pool())
                .map_err(|_| "DMA shared parent refund query failed")?;
            if parent.allocated == expected {
                libkernel::debug!(
                    "DMA shared parent RAM refunded: pool={}, allocated_pages={}",
                    parent.identity,
                    expected
                );
                break;
            }
            if libkernel::time::expired(end).map_err(|_| "DMA shared parent refund clock failed")? {
                return Err("DMA shared parent object RAM charge did not refund exactly");
            }
            libkernel::thread::yield_now().map_err(|_| "DMA shared parent refund yield failed")?;
        }
        libkernel::debug!(
            "DMA shared Backing retirement passed: independent writers, per-mapping metadata and original parent RAM refund"
        );
    }
    Ok(())
}

fn wait_stage(
    ready: &libkernel::ipc::notification::NotificationOwner,
    bits: u64,
    end: libkernel::time::Deadline,
) -> Result<(), &'static str> {
    let observed = wait::wait_until(ready.as_handle(), ObjectSignals::READABLE, end)
        .map_err(|_| "DMA shared stage wait failed")?;
    if !observed.observed.contains(ObjectSignals::READABLE)
        || ready
            .take(bits)
            .map_err(|_| "DMA shared stage consume failed")?
            != bits
    {
        return Err("DMA shared stage result missing");
    }
    Ok(())
}

fn log_pool(
    mode: &str,
    phase: &str,
    pool: &libkernel::memory_pool::MemoryPool,
) -> Result<(), &'static str> {
    let snapshot = pool.query().map_err(|_| "DMA race Pool query failed")?;
    libkernel::debug!(
        "DMA race Pool: mode={}, phase={}, id={}, pages={}/{}/{}, metadata={}/{}/{}",
        mode,
        phase,
        snapshot.identity,
        snapshot.allocated,
        snapshot.delegated,
        snapshot.reserved,
        snapshot.metadata.allocated,
        snapshot.metadata.delegated,
        snapshot.metadata.reserved
    );
    Ok(())
}
