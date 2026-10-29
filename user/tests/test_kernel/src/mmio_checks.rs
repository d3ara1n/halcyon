//! 既有 VM 验收的真实 PCI ECAM 来源；只读配置头，不改变 BAR/解码/总线主控。
use erhino_shared::{
    call::{SystemCall, SystemCallError},
    mem::{MemoryMapRequest, MemoryMapResult, MemoryPlacement, MemoryProtection},
    object::{Handle, ObjectSignals, Rights},
    proc::PROCESS_PAGE_SIZE as PAGE,
};
use libkernel::{
    ipc::{object, wait},
    mm::{MappedRegion, Placement},
    mmio::MmioControl,
};

fn done(handle: Handle) -> Result<(), &'static str> {
    let result = wait::wait_one(handle, ObjectSignals::DONE, 0)
        .map_err(|_| "MMIO completion wait failed")?;
    if !result.observed.contains(ObjectSignals::DONE) {
        return Err("MMIO completion did not report DONE");
    }
    Ok(())
}
/// 故意非法输出/执行权限的 ABI 夹具不构造指向 IO 的 Rust 引用。
fn raw_map(request: &MemoryMapRequest) -> usize {
    let mut error = request as *const _ as usize;
    // SAFETY: request 位于本线程栈并覆盖 ecall；非法 result 地址应由内核前置检查拒绝。
    unsafe {
        core::arch::asm!("ecall", in("a7") SystemCall::MemoryMap as usize,
        inlateout("a0") error, lateout("a1") _, in("a2") 0usize, in("a3") 0usize,
        in("a4") 0usize, in("a5") 0usize);
    }
    error
}
pub(super) fn run(root: Handle) -> Result<(), &'static str> {
    let full = Rights::READ
        | Rights::WRITE
        | Rights::WAIT
        | Rights::MANAGE
        | Rights::CREATE
        | Rights::DUPLICATE
        | Rights::TRANSIT
        | Rights::GRANT;
    let root = object::duplicate(root, full).map_err(|_| "MMIO root duplication failed")?;
    // SAFETY: 来自已核验 ECAM control 的新 capability，唯一关闭责任移入。
    let root = unsafe { MmioControl::from_handle(root) };
    let scope = root
        .create_control(0, 3 * PAGE, full)
        .map_err(|_| "MMIO child control failed")?;
    let use_rights = full & !Rights::CREATE;
    let usage = scope
        .create_use(0, 3 * PAGE, use_rights)
        .map_err(|_| "MMIO use creation failed")?;
    if !matches!(
        scope.create_use(PAGE, PAGE, use_rights),
        Err(SystemCallError::AddressConflict)
    ) {
        return Err("overlapping MMIO siblings were admitted");
    }
    let region = usage
        .map(0, 3 * PAGE, true, Placement::Anywhere)
        .map_err(|_| "MMIO ECAM Map failed")?;
    let range = region.range();
    // SAFETY: 本线程是子控制的唯一操作者，尚未开始撤销；PCI vendor/device dword 可只读。
    let identity = unsafe { region.read_u32(0) }.map_err(|_| "MMIO ECAM read failed")?;
    if identity & 0xffff == 0xffff || identity & 0xffff == 0 {
        return Err("PCI host configuration is absent");
    }
    let invalid_output = MemoryMapRequest::new(
        PAGE as u64,
        0,
        0,
        0,
        (range.start + 2 * PAGE) as u64,
        1,
        MemoryProtection::ReadWrite,
        MemoryPlacement::Anywhere,
    );
    if raw_map(&invalid_output) != SystemCallError::MemoryNotAccessible as usize {
        return Err("MMIO was accepted as a normal result buffer");
    }
    let mut output = MemoryMapResult::empty();
    let executable = MemoryMapRequest::new_object_view(
        usage.handle().raw(),
        0,
        PAGE as u64,
        0,
        0,
        0,
        core::ptr::addr_of_mut!(output) as u64,
        1,
        MemoryProtection::ReadExecute,
        MemoryPlacement::Anywhere,
    );
    if raw_map(&executable) != SystemCallError::RightsDenied as usize {
        return Err("executable MMIO was admitted");
    }
    // SAFETY: 此时没有撤销或其他映射操作者，部分解除仍属于当前 region。
    let remainder = unsafe { region.unmap_range(range.start + PAGE..range.start + 2 * PAGE) }
        .map_err(|_| "MMIO partial Unmap failed")?;
    let ram = MappedRegion::map_anonymous(
        PAGE,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::FixedEmpty {
            usable_start: range.start + PAGE,
        },
    )
    .map_err(|_| "MMIO hole RAM reuse failed")?;
    // SAFETY: 新 RAM 页由本线程唯一持有；与仍存续的两个 IO 片段没有重叠。
    unsafe {
        ((range.start + PAGE) as *mut u64).write_volatile(0x1234_5678_9abc_def0);
    }
    scope.seal().map_err(|_| "MMIO Seal failed")?;
    if !matches!(
        usage.map(0, PAGE, false, Placement::Anywhere),
        Err(SystemCallError::ObjectClosed)
    ) {
        return Err("sealed MMIO ancestor admitted a new Map");
    }
    scope.revoke().map_err(|_| "MMIO Revoke failed")?;
    done(scope.handle())?;
    done(usage.handle())?;
    // SAFETY: Revoke 只能退休原 MappingId 的 IO 片段；新 RAM 的独立 owner 仍存续。
    if unsafe { ((range.start + PAGE) as *const u64).read_volatile() } != 0x1234_5678_9abc_def0 {
        return Err("MMIO Revoke damaged reused RAM");
    }
    let _ = remainder; // 已撤销片段只丢弃几何 token，不再调用 Unmap。
    for address in [range.start, range.start + 2 * PAGE] {
        let probe = MappedRegion::map_anonymous(
            PAGE,
            0,
            0,
            MemoryProtection::ReadWrite,
            Placement::FixedEmpty {
                usable_start: address,
            },
        )
        .map_err(|_| "MMIO Revoke retained old VA")?;
        probe.unmap().map_err(|_| "MMIO VA probe cleanup failed")?;
    }
    ram.unmap().map_err(|_| "MMIO RAM hole cleanup failed")?;
    drop(usage);
    drop(scope);
    // 完成的观察/控制壳不再占用父范围，原偏移可以交给新的 binding。
    let again = root
        .create_use(0, PAGE, use_rights)
        .map_err(|_| "MMIO scope was not released")?;
    let view = again
        .map(0, PAGE, false, Placement::Anywhere)
        .map_err(|_| "MMIO reuse Map failed")?;
    // SAFETY: 新一轮尚未撤销，主动 Unmap 使用原映射 owner。
    unsafe { view.unmap() }.map_err(|_| "MMIO normal Unmap failed")?;
    again.revoke().map_err(|_| "MMIO empty use Revoke failed")?;
    done(again.handle())?;
    libkernel::debug!(
        "MMIO platform acceptance passed: PCI {:#010x}, revoke preserved RAM hole",
        identity
    );
    Ok(())
}

/// 真实子进程在有 IO View 时关闭最后 use，或接续提交父 Revoke 与进程退出。
pub(super) fn process_retirement(
    root: Handle,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    use erhino_shared::proc::HandleGrant;
    use libkernel::{ipc::notification, process};
    use libprocess::{GrantOutcome, SpawnRequest, spawn};
    let full = Rights::READ
        | Rights::WRITE
        | Rights::WAIT
        | Rights::MANAGE
        | Rights::CREATE
        | Rights::DUPLICATE
        | Rights::TRANSIT
        | Rights::GRANT;
    let handle =
        object::duplicate(root, full).map_err(|_| "MMIO retirement root duplicate failed")?;
    // SAFETY: 已核验 root 的新副本，由本作用域唯一关闭。
    let root = unsafe { MmioControl::from_handle(handle) };
    let modes: &[&str] = if crate::mode().has(crate::Mode::MMIO_FORCED) {
        &["mmio-forced-queued", "mmio-forced-publishing"]
    } else if crate::mode().has(crate::Mode::MMIO_PUBLICATION) {
        &["mmio-before", "mmio-after"]
    } else {
        &["mmio-close", "mmio-live"]
    };
    for &mode in modes {
        let close_use = mode == "mmio-close";
        let publication = matches!(mode, "mmio-before" | "mmio-after");
        let forced = mode.starts_with("mmio-forced-");
        let pool = if publication || forced {
            Some(
                libkernel::memory_pool::derive(
                    super::memory_pool(),
                    libdriver::boot::WORKER_POOL,
                    Rights::READ | Rights::CREATE,
                )
                .map_err(|_| "MMIO publication Pool admission failed")?,
            )
        } else {
            None
        };
        let scope = root
            .create_control(0, 32 * PAGE, full)
            .map_err(|_| "MMIO retirement scope failed")?;
        let rights = Rights::READ | Rights::WAIT | Rights::GRANT;
        let usage = scope
            .create_use(0, 32 * PAGE, rights)
            .map_err(|_| "MMIO retirement use failed")?;
        let ready =
            notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT)
                .map_err(|_| "MMIO retirement readiness creation failed")?;
        let (mut gate_owner, gate_signaler) = if publication {
            let gate =
                notification::create(Rights::READ | Rights::WAIT | Rights::GRANT, Rights::SIGNAL)
                    .map_err(|_| "MMIO publication gate failed")?;
            (Some(gate.owner), Some(gate.peer))
        } else {
            (None, None)
        };
        let mut grants = alloc::vec![
            HandleGrant {
                handle: usage.handle(),
                rights: Rights::READ | Rights::WAIT,
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
        let launched = spawn(SpawnRequest {
            memory_pool: pool
                .as_ref()
                .map_or(super::memory_pool(), |pool| pool.as_handle()),
            parent: super::current_process(),
            job,
            image,
            payload: mode.as_bytes(),
            grants: &grants,
            control_rights: super::SUPERVISOR_RIGHTS,
        });
        let started = match launched {
            Ok(started) => {
                usage.into_handle();
                ready.peer.into_raw();
                if let Some(gate) = gate_owner.take() {
                    gate.into_raw();
                }
                started
            }
            Err(failure) => {
                if failure.grants == GrantOutcome::Consumed {
                    usage.into_handle();
                    ready.peer.into_raw();
                    if let Some(gate) = gate_owner.take() {
                        gate.into_raw();
                    }
                }
                return Err("MMIO retirement target spawn failed");
            }
        };
        if publication || forced {
            libkernel::debug!(
                "MMIO publication target admitted: mode={}, pid={}, scope={}",
                mode,
                started.pid,
                scope.handle().raw()
            );
        }
        let result = (|| {
            let observed = wait::wait_one(ready.owner.as_handle(), ObjectSignals::READABLE, 0)
                .map_err(|_| "MMIO target readiness wait failed")?;
            if !observed.observed.contains(ObjectSignals::READABLE) {
                return Err("MMIO target did not become ready");
            }
            if let Some(pool) = &pool {
                ready
                    .owner
                    .take(1)
                    .map_err(|_| "MMIO publication readiness consume failed")?;
                log_pool(
                    mode,
                    if forced {
                        "mapped-before-revoke"
                    } else {
                        "before-map"
                    },
                    pool,
                )?;
            }
            if close_use {
                // 子进程仍存活；没有父 Revoke 帮它完成。范围可重新派生证明最后 Close 已退休。
                loop {
                    match scope.create_use(
                        0,
                        32 * PAGE,
                        Rights::READ | Rights::WAIT | Rights::MANAGE,
                    ) {
                        Ok(probe) => {
                            probe
                                .revoke()
                                .map_err(|_| "MMIO scope probe Revoke failed")?;
                            done(probe.handle())?;
                            break;
                        }
                        Err(SystemCallError::AddressConflict) => libkernel::thread::yield_now()
                            .map_err(|_| "MMIO retirement yield failed")?,
                        Err(_) => return Err("MMIO scope probe failed"),
                    }
                }
            }
            let alive =
                process::query(started.control).map_err(|_| "MMIO target liveness query failed")?;
            if alive.state != erhino_shared::proc::ProcessState::Running as u32
                || alive.cause != erhino_shared::proc::TerminationCause::None as u32
                || alive.detail != erhino_shared::proc::TerminationDetail::NONE
            {
                return Err("MMIO target exited before the parent Kill");
            }
            if let Some(gate) = &gate_signaler {
                gate.signal(1)
                    .map_err(|_| "MMIO publication start signal failed")?;
            }
            scope.revoke().map_err(|_| "MMIO parent Revoke failed")?;
            if forced {
                libkernel::debug!(
                    "MMIO forced Revoke returned before Kill: mode={}, pid={}",
                    mode,
                    started.pid
                );
            }
            if publication {
                let pool = pool.as_ref().ok_or("MMIO publication Pool missing")?;
                let observed = wait::wait_one(ready.owner.as_handle(), ObjectSignals::READABLE, 0)
                    .map_err(|_| "MMIO publication result wait failed")?;
                if !observed.observed.contains(ObjectSignals::READABLE)
                    || ready
                        .owner
                        .take(2)
                        .map_err(|_| "MMIO publication result consume failed")?
                        != 2
                {
                    return Err("MMIO publication result missing");
                }
                done(scope.handle())?;
                let snapshot = scope
                    .query()
                    .map_err(|_| "MMIO publication DONE query failed")?;
                if snapshot.flags & erhino_shared::mmio::MmioSnapshot::DONE == 0
                    || snapshot.members != 0
                {
                    return Err("MMIO publication scope not retired");
                }
                let status = process::query(started.control)
                    .map_err(|_| "MMIO publication live query failed")?;
                if status.state != erhino_shared::proc::ProcessState::Running as u32 {
                    return Err("MMIO publication completed after target death");
                }
                log_pool(mode, "after-revoke", pool)?;
                libkernel::debug!(
                    "MMIO publication scope DONE while target Running: mode={}, pid={}, members=0",
                    mode,
                    started.pid
                );
            }
            process::kill(started.control).map_err(|_| "MMIO target Kill failed")?;
            process::wait_done(started.control).map_err(|_| "MMIO target DONE wait failed")?;
            done(scope.handle())?;
            let status =
                process::query(started.control).map_err(|_| "MMIO target terminal query failed")?;
            if status.state != erhino_shared::proc::ProcessState::Dead as u32
                || status.cause != erhino_shared::proc::TerminationCause::Killed as u32
                || status.detail != erhino_shared::proc::TerminationDetail::PROCESS_CONTROL_KILL
            {
                return Err("MMIO target did not terminate through the parent Kill");
            }
            if forced {
                let snapshot = scope
                    .query()
                    .map_err(|_| "MMIO forced scope query failed")?;
                if snapshot.members != 0
                    || snapshot.flags & erhino_shared::mmio::MmioSnapshot::DONE == 0
                {
                    return Err("MMIO forced exit retained a mapping responsibility");
                }
                libkernel::debug!(
                    "MMIO forced Frozen retirement passed: mode={}, pid={}, members=0, cpu=Killed",
                    mode,
                    started.pid
                );
            }
            if let Some(pool) = &pool {
                log_pool(mode, "cpu-done", pool)?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = scope.revoke();
            let _ = process::kill(started.control);
            let _ = process::wait_done(started.control);
        }
        // SAFETY: 唯一父方 ProcessControl，完成或失败路径均已收束目标。
        unsafe { object::close(started.control) }
            .map_err(|_| "MMIO target control close failed")?;
        result?;
        if let Some(pool) = &pool {
            let end = libkernel::time::timeout_millis(60_000)
                .map_err(|_| "MMIO publication refund deadline failed")?;
            loop {
                let snapshot = pool
                    .query()
                    .map_err(|_| "MMIO publication refund query failed")?;
                if snapshot.allocated == 0
                    && snapshot.delegated == 0
                    && snapshot.reserved == 0
                    && snapshot.metadata.allocated == 0
                    && snapshot.metadata.delegated == 0
                    && snapshot.metadata.reserved == 0
                {
                    break;
                }
                if libkernel::time::expired(end)
                    .map_err(|_| "MMIO publication refund clock failed")?
                {
                    return Err("MMIO publication Pool did not refund");
                }
                libkernel::thread::yield_now()
                    .map_err(|_| "MMIO publication refund yield failed")?;
            }
            log_pool(mode, "closed", pool)?;
        }
    }
    libkernel::debug!(
        "MMIO process retirement acceptance passed: last use Close and Revoke/exit submissions"
    );
    Ok(())
}

fn log_pool(
    mode: &str,
    phase: &str,
    pool: &libkernel::memory_pool::MemoryPool,
) -> Result<(), &'static str> {
    let snapshot = pool
        .query()
        .map_err(|_| "MMIO publication Pool query failed")?;
    libkernel::debug!(
        "MMIO publication Pool: mode={}, phase={}, id={}, pages_allocated={}, pages_delegated={}, metadata_allocated={}, metadata_delegated={}",
        mode,
        phase,
        snapshot.identity,
        snapshot.allocated,
        snapshot.delegated,
        snapshot.metadata.allocated,
        snapshot.metadata.delegated
    );
    Ok(())
}
