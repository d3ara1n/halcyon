//! 内存映射、进程付款与 Pool 双维预算。
use super::*;

pub(super) fn pool_allocated() -> Result<u64, SystemCallError> {
    let handle = duplicate(memory_pool(), Rights::READ)?;
    // SAFETY: duplicate 新建唯一 raw owner，本作用域不保留 alias，Drop 负责关闭。
    let pool = unsafe { MemoryPool::from_handle(handle) };
    Ok(pool.query()?.allocated)
}

pub(super) fn test_process_funding(job: Handle) -> Result<(), &'static str> {
    // 独立付款域只包含本次 Building 与一页 Map；全局 Root 还可能推进其他对象的异步退款。
    let pool = libkernel::memory_pool::derive(
        memory_pool(),
        libkernel::shared::memory_pool::MemoryPoolBudget {
            pages: 16,
            // 空间的区域账本、View 与预付退休记录需要 metadata；页预算不代替这些费用。
            metadata: 1024 * 1024 / libkernel::shared::memory_pool::METADATA_SLOT_BYTES,
        },
        Rights::READ | Rights::CREATE | Rights::DUPLICATE,
    )
    .map_err(|_| "process funding Pool admission failed")?;
    let baseline = pool
        .query()
        .map_err(|_| "process funding Pool query failed")?
        .allocated;
    if baseline != 0 {
        return Err("process funding Pool is not empty");
    }
    let wrong_kind = duplicate(job, Rights::CREATE).map_err(|_| "Job duplicate failed")?;
    let rejected = process::create(job, wrong_kind, SUPERVISOR_RIGHTS);
    let _ = unsafe { close(wrong_kind) };
    if !matches!(rejected, Err(SystemCallError::WrongObjectType)) {
        if let Ok(created) = rejected {
            let _ = unsafe { process::abandon_to_completion(created) };
        }
        return Err("ProcessCreate accepted non-Pool funding authority");
    }
    let no_create =
        duplicate(pool.as_handle(), Rights::READ).map_err(|_| "Pool duplicate failed")?;
    let denied = process::create(job, no_create, SUPERVISOR_RIGHTS);
    let _ = unsafe { close(no_create) };
    if !matches!(denied, Err(SystemCallError::RightsDenied)) {
        if let Ok(created) = denied {
            let _ = unsafe { process::abandon_to_completion(created) };
        }
        return Err("ProcessCreate accepted Pool without CREATE");
    }
    let created = process::create(job, pool.as_handle(), SUPERVISOR_RIGHTS).map_err(|error| {
        debug!("Funded process creation failed: {:?}", error);
        "funded process create failed"
    })?;
    let result = process::map(
        created.builder,
        0x1000,
        libkernel::shared::proc::PROCESS_PAGE_SIZE,
        libkernel::shared::proc::ProcessMapFlags::READ,
    );
    // 创建不消费付款 authority；创建后的 Building 可直接映射。
    let pool_alive = pool
        .query()
        .is_ok_and(|snapshot| snapshot.allocated > baseline);
    let cleanup = unsafe { process::abandon_to_completion(created) };
    if result.is_err() || !pool_alive || cleanup.is_err() {
        return Err("ProcessCreate funding or cleanup failed");
    }
    let after = pool
        .query()
        .map_err(|_| "process funding Pool final query failed")?
        .allocated;
    if after != baseline {
        debug!(
            "ProcessCreate funding mismatch: baseline={}, after={}",
            baseline, after
        );
        return Err("ProcessCreate charge did not refund");
    }
    debug!("process creation funding acceptance passed");
    Ok(())
}

pub(super) fn test_memory_mapping() -> Result<(), &'static str> {
    let pool_baseline = if mode().stress() {
        None
    } else {
        Some(pool_allocated().map_err(|_| "memory Pool baseline query failed")?)
    };
    let page = libkernel::shared::proc::PROCESS_PAGE_SIZE;
    let region = MappedRegion::map_anonymous(
        3 * page,
        page,
        page,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "anonymous guarded Map failed")?;
    let usable = region.usable().ok_or("Map returned no usable range")?;
    // SAFETY: usable 是本线程刚取得的 RW anonymous mapping。
    unsafe {
        (usable.start as *mut u64).write_volatile(0x1122_3344_5566_7788);
        ((usable.end - core::mem::size_of::<u64>()) as *mut u64)
            .write_volatile(0x8877_6655_4433_2211);
    }
    region
        .protect(
            usable.start..usable.start + page,
            MemoryProtection::ReadOnly,
        )
        .map_err(|_| "MemoryProtect RW to R failed")?;
    region
        .protect(
            usable.start..usable.start + page,
            MemoryProtection::ReadWrite,
        )
        .map_err(|_| "MemoryProtect R to RW failed")?;

    let middle = usable.start + page..usable.start + 2 * page;
    let remainder = region
        .unmap_range(middle)
        .map_err(|_| "partial MemoryUnmap failed")?;
    remainder
        .left
        .ok_or("partial Unmap lost left fragment")?
        .unmap()
        .map_err(|_| "left fragment Unmap failed")?;
    remainder
        .right
        .ok_or("partial Unmap lost right fragment")?
        .unmap()
        .map_err(|_| "right fragment Unmap failed")?;

    let remapped = MappedRegion::map_anonymous(
        3 * page,
        page,
        page,
        MemoryProtection::ReadWrite,
        Placement::FixedEmpty {
            usable_start: usable.start,
        },
    )
    .map_err(|_| "fixed remap after Unmap failed")?;
    let remapped_usable = remapped
        .usable()
        .ok_or("fixed remap returned no usable range")?;
    if remapped_usable != usable {
        return Err("fixed remap geometry changed");
    }
    // SAFETY: remapped usable range 是新取得的 RW anonymous mapping。
    let zeroed = unsafe {
        (remapped_usable.start as *const u64).read_volatile() == 0
            && ((remapped_usable.end - core::mem::size_of::<u64>()) as *const u64).read_volatile()
                == 0
    };
    if !zeroed {
        return Err("remapped anonymous backing was not zeroed");
    }
    remapped
        .unmap()
        .map_err(|_| "fixed remap final Unmap failed")?;

    // 公共 MemoryObject：创建 → 快照 → 两个 view → Handle 先关仍可访问 → 撤销 → 守恒。
    let object = MemoryObject::create(4 * page).map_err(|_| "MemoryObject create failed")?;
    let snapshot = object.query().map_err(|_| "MemoryObject query failed")?;
    if snapshot.bytes != 4 * page as u64
        || snapshot.state() != Some(MemoryObjectState::Mutable)
        || snapshot.write_permits != 0
        || !snapshot.closes()
    {
        return Err("MemoryObject snapshot geometry or state invalid");
    }
    // 同一对象按不同 offset 与权限映入两个区间：对象 backing 只付一次。
    let writable = MappedRegion::map_object(
        &object,
        page,
        2 * page,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "MemoryObject writable view Map failed")?;
    let readable = MappedRegion::map_object(
        &object,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadOnly,
        Placement::Anywhere,
    )
    .map_err(|_| "MemoryObject read-only view Map failed")?;
    if object
        .query()
        .map_err(|_| "MemoryObject query after Map failed")?
        .write_permits
        != 1
    {
        return Err("MemoryObject did not account its writable view");
    }
    let writable_usable = writable
        .usable()
        .ok_or("MemoryObject writable view returned no usable range")?;
    // SAFETY: 刚取得的 RW object view；backing 由对象拥有。
    unsafe {
        (writable_usable.start as *mut u64).write_volatile(0x1357_9bdf);
    }
    // Handle 关闭不撤销既有 view，也不释放 backing：view 强引用独立保活对象。
    object.close();
    // SAFETY: Handle 已关闭，但 view 仍然有效。
    let survived = unsafe { (writable_usable.start as *const u64).read_volatile() };
    if survived != 0x1357_9bdf {
        return Err("MemoryObject view lost its data after Handle close");
    }
    readable
        .unmap()
        .map_err(|_| "MemoryObject read-only view Unmap failed")?;
    // 部分撤销只消费本 view 的区域，不切分对象数据 backing。
    let remainder = writable
        .unmap_range(writable_usable.start..writable_usable.start + page)
        .map_err(|_| "MemoryObject partial view Unmap failed")?;
    remainder
        .right
        .ok_or("MemoryObject partial Unmap lost its right fragment")?
        .unmap()
        .map_err(|_| "MemoryObject last view Unmap failed")?;

    // Sealing 允许既有 writable view 收缩/切分，但拒绝把只读片段重新升为 writable。
    let sealing = MemoryObject::create(3 * page).map_err(|_| "sealing object create failed")?;
    let sealing_view = MappedRegion::map_object(
        &sealing,
        0,
        3 * page,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "sealing writable view Map failed")?;
    let sealing_usable = sealing_view
        .usable()
        .ok_or("sealing writable view returned no usable range")?;
    sealing.seal().map_err(|_| "sealing request failed")?;
    sealing_view
        .protect(
            sealing_usable.start + page..sealing_usable.start + 2 * page,
            MemoryProtection::ReadOnly,
        )
        .map_err(|_| "Sealing partial writable downgrade failed")?;
    let snapshot = sealing
        .query()
        .map_err(|_| "sealing object query after downgrade failed")?;
    if snapshot.state() != Some(MemoryObjectState::Sealing) || snapshot.write_permits != 2 {
        return Err("Sealing writable successor accounting invalid");
    }
    match sealing_view.protect(
        sealing_usable.start + page..sealing_usable.start + 2 * page,
        MemoryProtection::ReadWrite,
    ) {
        Err(SystemCallError::ObjectBusy) => {}
        Err(_) => return Err("Sealing write re-enable returned the wrong error"),
        Ok(()) => return Err("Sealing admitted a new writable region"),
    }
    sealing_view
        .protect(sealing_usable.clone(), MemoryProtection::ReadOnly)
        .map_err(|_| "Sealing final writable downgrade failed")?;
    let snapshot = sealing
        .query()
        .map_err(|_| "sealing object final query failed")?;
    if snapshot.state() != Some(MemoryObjectState::Executable) || snapshot.write_permits != 0 {
        return Err("Sealing did not complete after the last writable successor retired");
    }
    sealing_view
        .unmap()
        .map_err(|_| "sealing view Unmap failed")?;
    sealing.close();

    // RX authority 与对象发布状态正交：完整 capability 在 Mutable 期仍不能执行，
    // Seal 后缺 EXECUTE 的裁剪副本也不能借对象状态绕过 rights。
    let executable = MemoryObject::create(page).map_err(|_| "executable object create failed")?;
    match MappedRegion::map_object(
        &executable,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadExecute,
        Placement::Anywhere,
    ) {
        Err(SystemCallError::ObjectBusy) => {}
        Err(_) => return Err("mutable object RX returned the wrong error"),
        Ok(mapping) => {
            mapping
                .unmap()
                .map_err(|_| "unexpected mutable RX cleanup failed")?;
            return Err("mutable object admitted an RX view");
        }
    }
    let no_execute_handle = duplicate(executable.handle(), Rights::READ)
        .map_err(|_| "non-execute object capability derive failed")?;
    let rx_handle = duplicate(executable.handle(), Rights::READ | Rights::EXECUTE)
        .map_err(|_| "execute object capability derive failed")?;
    // SAFETY: duplicate 返回两个新 Handle，本作用域分别建立唯一 typed owner。
    let no_execute = unsafe { MemoryObject::from_handle(no_execute_handle) };
    let executable_view = unsafe { MemoryObject::from_handle(rx_handle) };
    executable
        .seal()
        .map_err(|_| "executable object Seal failed")?;
    executable.close();
    match MappedRegion::map_object(
        &no_execute,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadExecute,
        Placement::Anywhere,
    ) {
        Err(SystemCallError::RightsDenied) => {}
        Err(_) => return Err("RX without EXECUTE returned the wrong error"),
        Ok(mapping) => {
            mapping
                .unmap()
                .map_err(|_| "unexpected unauthorized RX cleanup failed")?;
            return Err("RX view admitted without EXECUTE right");
        }
    }
    let rx = MappedRegion::map_object(
        &executable_view,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadExecute,
        Placement::Anywhere,
    )
    .map_err(|_| "authorized RX view Map failed")?;
    executable_view.close();
    no_execute.close();
    rx.unmap().map_err(|_| "authorized RX view Unmap failed")?;
    if !mode().stress()
        && pool_allocated().map_err(|_| "memory Pool final query failed")?
            != pool_baseline.expect("non-stress baseline was queried")
    {
        return Err("MemoryObject backing Pool charge did not refund");
    }
    debug!("public memory mapping acceptance passed");
    Ok(())
}

/// 双维预算源自父账户；关闭外部授权不丢失子树与已付存储的退款责任。
pub(super) fn test_pool_budget() {
    use erhino_shared::memory_pool::MemoryPoolBudget;
    use libkernel::memory_pool::MemoryPool;
    let rights = Rights::CREATE | Rights::READ | Rights::DUPLICATE;
    let root = duplicate(memory_pool(), rights).unwrap();
    // SAFETY: duplicate 返回一份唯一 raw owner，交 typed 管理。
    let funding = unsafe { MemoryPool::from_handle(root) };
    let supply = funding.query().unwrap();
    // 测试域独立记账；调用者表增长等合法成本不属于本域，不以全局 Root 当退款基线。
    let root = funding
        .derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: supply.metadata.available / 4,
            },
            rights,
        )
        .unwrap();
    let before = root.query().unwrap();
    assert!(matches!(
        root.derive(MemoryPoolBudget::default(), rights),
        Err(SystemCallError::IllegalArgument)
    ));
    let pages = root
        .derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: 0,
            },
            rights,
        )
        .unwrap();
    let snapshot = pages.query().unwrap();
    assert!(matches!(
        pages.derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: 1
            },
            rights
        ),
        Err(SystemCallError::QuotaExceeded)
    ));
    assert_eq!(
        pages.query().unwrap(),
        snapshot,
        "failed second dimension retained page reservation"
    );
    // 非空对象必须支付出生存储；页供给充足也不能隐式借用调用者 metadata。
    let page_owner = pages.into_handle();
    assert!(matches!(
        libkernel::ipc::invitation::Invitation::create(page_owner, 4096),
        Err(libkernel::ipc::invitation::CreateFailure::System(
            SystemCallError::QuotaExceeded
        ))
    ));
    // SAFETY: Create 只接受 Pool 授权，不消费该表项；raw 使用已结束，恢复唯一 owner。
    let pages = unsafe { MemoryPool::from_handle(page_owner) };
    assert_eq!(
        pages.query().unwrap(),
        snapshot,
        "metadata-denied birth retained page or metadata credit"
    );
    pages.close();
    assert_eq!(
        root.query().unwrap(),
        before,
        "page-only Pool retained parent metadata charge"
    );
    // 额度从实际供给取一部分，只检查守恒，不复制内核结构大小或旧对象个数限额。
    let budget = before.metadata.available / 4;
    assert!(budget > 0);
    let child = root
        .derive(
            MemoryPoolBudget {
                pages: 0,
                metadata: budget,
            },
            rights,
        )
        .unwrap();
    let child_snapshot = child.query().unwrap();
    assert_eq!(child_snapshot.total, 0);
    assert_eq!(child_snapshot.metadata.available, budget);
    let grand = child
        .derive(
            MemoryPoolBudget {
                pages: 0,
                metadata: 1,
            },
            rights,
        )
        .unwrap();
    assert!(
        child.query().unwrap().metadata.allocated > 0,
        "grandchild stored no parent-paid metadata"
    );
    child.close();
    assert_eq!(
        root.query().unwrap().metadata.delegated,
        before.metadata.delegated + budget
    );
    assert_eq!(
        grand.query().unwrap().metadata.available,
        1,
        "parent Handle close revoked child budget"
    );
    grand.close();
    assert_eq!(
        root.query().unwrap(),
        before,
        "Pool tree lost page or metadata credit"
    );
    // 最大资金父链通过真实 API 释放；不复制内核 header 的私有布局。
    let payer = root
        .derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: budget,
            },
            rights,
        )
        .unwrap();
    let baseline = payer.query().unwrap();
    let mut last: Option<MemoryPool> = None;
    for _ in baseline.depth..erhino_shared::memory_pool::MEMORY_POOL_MAX_DEPTH {
        let parent = last.as_ref().unwrap_or(&payer);
        let snapshot = parent.query().unwrap();
        let mut metadata = snapshot.metadata.available;
        let child = loop {
            match parent.derive(MemoryPoolBudget { pages: 1, metadata }, rights) {
                Ok(child) => break child,
                Err(SystemCallError::QuotaExceeded) => {
                    assert_eq!(
                        parent.query().unwrap(),
                        snapshot,
                        "rejected Pool birth retained credit"
                    );
                    metadata = metadata
                        .checked_sub(1)
                        .expect("Pool birth could not reserve storage");
                }
                Err(error) => panic!("maximum Pool chain preparation failed: {error:?}"),
            }
        };
        if let Some(old) = last.replace(child) {
            old.close();
        }
    }
    let leaf = last.unwrap();
    assert_eq!(
        leaf.query().unwrap().depth,
        erhino_shared::memory_pool::MEMORY_POOL_MAX_DEPTH
    );
    let depth_before = leaf.query().unwrap();
    assert!(matches!(
        leaf.derive(
            MemoryPoolBudget {
                pages: 0,
                metadata: 1
            },
            rights
        ),
        Err(SystemCallError::ReachLimit)
    ));
    assert_eq!(leaf.query().unwrap(), depth_before);
    let raw = leaf.into_handle();
    let (endpoint, invitation, _) =
        libkernel::ipc::invitation::Invitation::create(raw, 4096).unwrap();
    // 普通 View 是独立的真实 use；两端和Pool cap都先关，再由Unmap最后解脱。
    let mapping = libkernel::mm::MappedRegion::map_endpoint(
        &endpoint,
        4096,
        libkernel::shared::mem::MemoryProtection::ReadWrite,
        libkernel::mm::Placement::Anywhere,
    )
    .unwrap();
    // SAFETY: Create 未消费 Pool 表项，raw 调用结束后恢复唯一 owner。
    let leaf = unsafe { MemoryPool::from_handle(raw) };
    leaf.close();
    invitation.close().unwrap();
    endpoint.close().unwrap();
    assert_ne!(
        payer.query().unwrap(),
        baseline,
        "live View lost its funding chain before retirement"
    );
    mapping.unmap().unwrap();
    // 真实 Backing 退休记录是最后 payer，最终父链在 M 段解脱。
    let deadline = libkernel::time::timeout_millis(5_000).unwrap();
    while payer.query().unwrap() != baseline {
        assert!(
            !libkernel::time::expired(deadline).unwrap(),
            "maximum Pool chain retained credit after Close"
        );
        libkernel::time::sleep_until(libkernel::time::timeout_millis(1).unwrap()).unwrap();
    }
    payer.close();
    debug!(
        "Pool metadata budget checks passed: dual rollback, no implicit quota, parent close, complete refund, maximum chain"
    );
}
