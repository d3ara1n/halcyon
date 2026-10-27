//! 当前进程的真实 VM 修改入口。统一出生操作 owner，预约后准备所有资源，
//! payload/cookie 发布后必成提交，失败由同一个 ChangeWork 分步回滚。

use super::{
    backing::Backing,
    change::Operation,
    space::{AnySpace, ChangeProgress},
};
use crate::{
    frame,
    task::{
        Thread,
        handle::Authorization,
        memory_object::{MemoryObject, MemoryObjectCore, map_object_error},
        process::shell::BodyLease,
        wait::WaitPlan,
    },
    uaccess,
};
use alloc::sync::Arc;
use erhino_shared::{
    call::SystemCallError,
    mem::{MemoryMapRequest, MemoryMapResult, MemoryPlacement, MemoryProtection},
    object::{Handle, Rights},
};
use memory_space::{
    AddressRange, AnonymousClass, ChangeError, MapPlacement, MapRequest, MappingSource, PAGE_SIZE,
    PageRange, ProtectRequest, Protection, RegionOwner, UnmapRequest, UserWriteLeaseRequest,
};

fn error(error: ChangeError) -> SystemCallError {
    use ChangeError as C;
    match error {
        C::Conflict => SystemCallError::AddressConflict,
        C::NotCovered | C::Guard => SystemCallError::NotMapped,
        C::OwnerDenied | C::PermissionDenied => SystemCallError::RightsDenied,
        C::Busy | C::Stale | C::CapacityChanged | C::TransactionLimit => {
            SystemCallError::ObjectBusy
        }
        C::PageLimit | C::RegionLimit | C::KeyExhausted => SystemCallError::ReachLimit,
        _ => SystemCallError::IllegalArgument,
    }
}
fn protection(raw: usize) -> Result<Protection, SystemCallError> {
    match u32::try_from(raw).ok().and_then(MemoryProtection::from_raw) {
        Some(MemoryProtection::ReadOnly) => Ok(Protection::ReadOnly),
        Some(MemoryProtection::ReadWrite) => Ok(Protection::ReadWrite),
        Some(MemoryProtection::ReadExecute) => Ok(Protection::ReadExecute),
        None => Err(SystemCallError::IllegalArgument),
    }
}
fn number(raw: u64) -> Result<usize, SystemCallError> {
    usize::try_from(raw).map_err(|_| SystemCallError::IllegalArgument)
}

/// Building 与 boot 使用同一准备/提交链；空 fence 目标由 VM 准入证明。
pub(crate) fn initial_map(
    body: &BodyLease,
    address: usize,
    bytes: usize,
    protection: Protection,
    backing: Option<super::backing::BackingOwner>,
) -> Result<WaitPlan, SystemCallError> {
    let range = PageRange::new(address, bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    let access = body
        .vm
        .admit_building()
        .ok_or(SystemCallError::ObjectClosed)?;
    let pool = body
        .vm
        .space
        .lock()
        .space
        .as_ref()
        .ok_or(SystemCallError::ObjectClosed)?
        .account()
        .clone();
    let mut operation =
        Operation::with_access(pool.clone(), body.vm.clone(), body.views.clone(), access)?;
    let root = match backing {
        Some(root) => root,
        None => {
            let pages = body
                .vm
                .space
                .lock()
                .space
                .as_ref()
                .ok_or(SystemCallError::ObjectClosed)?
                .ledger()
                .limits()
                .max_pages_per_change;
            Backing::fund(
                pool,
                range.pages(),
                funded_frame::Limits {
                    max_pages: pages,
                    max_extents: frame::MAX_FUNDED_EXTENTS,
                },
            )?
        }
    };
    let identity = root.backing().id();
    operation.prep.usage = Some(root.into_use().into());
    prepare(
        body,
        &mut operation,
        memory_space::ChangeRequest::Map(MapRequest {
            bytes,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty {
                usable_start: address,
            },
            current: protection,
            maximum: protection,
            owner: RegionOwner::AddressSpace,
            backing: MappingSource::Anonymous {
                identity,
                class: if protection == Protection::ReadExecute {
                    AnonymousClass::InitialExecutable
                } else {
                    AnonymousClass::Data
                },
            },
            result: None,
        }),
    )?;
    finish(body, operation, None, None)
}

/// 初始映像允许填充最终为 RX 的页；短 Building 借用与 VM lease 排除执行进入。
pub(crate) fn initial_write(
    body: &BodyLease,
    target: usize,
    bytes: &[u8],
) -> Result<(), SystemCallError> {
    let _access = body
        .vm
        .admit_building()
        .ok_or(SystemCallError::ObjectClosed)?;
    let mut vm = body.vm.space.lock();
    let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
    uaccess::put_initial(space, target, bytes).map_err(Into::into)
}

pub(crate) fn memory_map(
    thread: &Thread,
    input: usize,
) -> Result<crate::task::request::Pending, SystemCallError> {
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let request = {
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        // SAFETY: 固定整数 ABI；请求复制后不再借用用户输入缓冲。
        unsafe { uaccess::read_user_value::<MemoryMapRequest>(space, input) }?
    };
    crate::task::request::Pending::new(
        MapCall {
            request,
            phase: Some(MapPhase::Start),
            operation: None,
        },
        &thread.process.funding_pool(),
    )
}

struct MapCall {
    request: MemoryMapRequest,
    phase: Option<MapPhase>,
    operation: Option<Operation>,
}
enum MapPhase {
    Start,
    Admit(
        Arc<crate::resource::mmio::MmioNode>,
        resource_tree::Admission,
    ),
    Publish(PageRange),
}
impl crate::task::request::Request for MapCall {
    fn advance(&mut self, thread: &Thread) -> Result<crate::task::request::Step, SystemCallError> {
        match self
            .phase
            .take()
            .expect("map request advanced after completion")
        {
            MapPhase::Start => self.start(thread),
            MapPhase::Admit(node, admission) => self.admit(thread, node, admission),
            MapPhase::Publish(footprint) => self.publish(thread, footprint),
        }
    }
}
impl MapCall {
    fn start(&mut self, thread: &Thread) -> Result<crate::task::request::Step, SystemCallError> {
        if self.request.source != 0 {
            let protection = protection(self.request.protection as usize)?;
            let rights = match protection {
                Protection::ReadOnly => Rights::READ,
                Protection::ReadWrite => Rights::READ | Rights::WRITE,
                Protection::ReadExecute => Rights::READ | Rights::EXECUTE,
            };
            let (_, mmio) = Authorization::accept_with(
                thread,
                Handle::from_raw(self.request.source),
                rights,
                |object, _| {
                    if let Some(usage) = object
                        .as_any()
                        .downcast_ref::<crate::resource::mmio::object::MmioUse>()
                    {
                        if protection == Protection::ReadExecute {
                            return Err(SystemCallError::RightsDenied);
                        }
                        Ok(Some((usage.node().clone(), usage.node().begin()?)))
                    } else {
                        Ok(None)
                    }
                },
            )?;
            if let Some((node, admission)) = mmio {
                self.phase = Some(MapPhase::Admit(node, admission));
                return Ok(crate::task::request::Step::Pending);
            }
        }
        let (operation, footprint) = prepare_memory_map(thread, self.request, None)?;
        self.operation = Some(operation);
        self.complete(thread, footprint)
    }
    fn admit(
        &mut self,
        thread: &Thread,
        node: Arc<crate::resource::mmio::MmioNode>,
        mut admission: resource_tree::Admission,
    ) -> Result<crate::task::request::Step, SystemCallError> {
        if !node.advance(&mut admission)? {
            self.phase = Some(MapPhase::Admit(node, admission));
        } else {
            let (operation, footprint) =
                prepare_memory_map(thread, self.request, Some((node, admission)))?;
            self.operation = Some(operation);
            self.phase = Some(MapPhase::Publish(footprint));
        }
        Ok(crate::task::request::Step::Pending)
    }
    fn publish(
        &mut self,
        thread: &Thread,
        footprint: PageRange,
    ) -> Result<crate::task::request::Step, SystemCallError> {
        if !self
            .operation
            .as_mut()
            .expect("MMIO request lost operation")
            .advance_publication()?
        {
            self.phase = Some(MapPhase::Publish(footprint));
            return Ok(crate::task::request::Step::Pending);
        }
        self.complete(thread, footprint)
    }
    fn complete(
        &mut self,
        thread: &Thread,
        footprint: PageRange,
    ) -> Result<crate::task::request::Step, SystemCallError> {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let operation = self.operation.take().expect("map request lost operation");
        commit(
            &body,
            operation,
            footprint,
            Some((
                thread,
                number(self.request.result_address)?,
                self.request.cookie,
            )),
        )
        .map(crate::task::request::Step::Wait)
    }
}

fn prepare_memory_map(
    thread: &Thread,
    request: MemoryMapRequest,
    mmio: Option<(
        Arc<crate::resource::mmio::MmioNode>,
        resource_tree::Admission,
    )>,
) -> Result<(Operation, PageRange), SystemCallError> {
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let page_limit = body
        .vm
        .space
        .lock()
        .space
        .as_ref()
        .ok_or(SystemCallError::ObjectClosed)?
        .ledger()
        .limits()
        .max_pages_per_change;
    if request.reserved != [0] || request.cookie == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let bytes = number(request.bytes)?;
    let bytes = PageRange::rounded(0, bytes)
        .map_err(|_| SystemCallError::IllegalArgument)?
        .bytes();
    let round_guard = |raw| -> Result<usize, SystemCallError> {
        Ok(number(raw)?
            .checked_add(PAGE_SIZE - 1)
            .ok_or(SystemCallError::IllegalArgument)?
            / PAGE_SIZE
            * PAGE_SIZE)
    };
    let before = round_guard(request.guard_before)?;
    let after = round_guard(request.guard_after)?;
    let total = (bytes / PAGE_SIZE)
        .checked_add(before / PAGE_SIZE)
        .and_then(|n| n.checked_add(after / PAGE_SIZE))
        .ok_or(SystemCallError::ReachLimit)?;
    if total > page_limit {
        return Err(SystemCallError::ReachLimit);
    }
    let placement = match MemoryPlacement::from_raw(request.placement) {
        Some(MemoryPlacement::Anywhere) if request.address == 0 => MapPlacement::Anywhere,
        Some(MemoryPlacement::FixedEmpty) => {
            let address = number(request.address)?;
            if !address.is_multiple_of(PAGE_SIZE)
                || address >= erhino_shared::proc::PROCESS_USER_TOP
            {
                return Err(SystemCallError::IllegalArgument);
            }
            MapPlacement::FixedEmpty {
                usable_start: address,
            }
        }
        _ => return Err(SystemCallError::IllegalArgument),
    };
    let protection = protection(request.protection as usize)?;
    let output = number(request.result_address)?;
    if !output.is_multiple_of(core::mem::align_of::<MemoryMapResult>()) {
        return Err(SystemCallError::IllegalArgument);
    }
    let result_range = AddressRange::new(output, core::mem::size_of::<MemoryMapResult>())
        .map_err(|_| SystemCallError::IllegalArgument)?;
    {
        let mut vm = body.vm.space.lock();
        // SAFETY: 输出快照全为整数，检查初始化/保留字段；预约稍后复检整个 RW 投影。
        let initial: MemoryMapResult = unsafe {
            uaccess::read_user_value(
                vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
                output,
            )
        }?;
        if initial.reserved != [0; 3] || initial.committed != 0 {
            return Err(SystemCallError::IllegalArgument);
        }
    }
    let pool = thread.process.funding_pool();
    let mut operation = Operation::new(pool.clone(), body.vm.clone(), body.views.clone())?;
    let (backing, source) = if let Some((node, admission)) = mmio {
        (
            operation.prepare_mmio(node, &admission, number(request.source_offset)?, bytes)?,
            None,
        )
    } else if request.source == 0 {
        if request.source_offset != 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        if protection == Protection::ReadExecute {
            return Err(SystemCallError::RightsDenied);
        }
        let root = Backing::fund(
            pool.clone(),
            bytes / PAGE_SIZE,
            funded_frame::Limits {
                max_pages: page_limit,
                max_extents: frame::MAX_FUNDED_EXTENTS,
            },
        )?;
        let identity = root.backing().id();
        operation.prep.usage = Some(root.into_use().into());
        (
            MappingSource::Anonymous {
                identity,
                class: AnonymousClass::Data,
            },
            None,
        )
    } else {
        let rights = match protection {
            Protection::ReadOnly => Rights::READ,
            Protection::ReadWrite => Rights::READ | Rights::WRITE,
            Protection::ReadExecute => Rights::READ | Rights::EXECUTE,
        };
        let offset = number(request.source_offset)?;
        let borrowed = Authorization::accept(thread, Handle::from_raw(request.source), rights)?;
        let any = borrowed.object().as_any();
        let core = if let Some(object) = any.downcast_ref::<MemoryObject>() {
            object.core().clone()
        } else if let Some(endpoint) = any.downcast_ref::<crate::task::tunnel::Endpoint>() {
            endpoint.core()
        } else {
            return Err(SystemCallError::WrongObjectType);
        };
        let (core, authorization, usage) = loop {
            let (_authority, (accepted, authorization, usage)) = Authorization::accept_with(
                thread,
                Handle::from_raw(request.source),
                rights,
                |source, _| {
                    if !Arc::ptr_eq(source, borrowed.object()) {
                        return Err(SystemCallError::ObjectClosed);
                    }
                    let authorization =
                        core.authorize_view(protection).map_err(map_object_error)?;
                    authorization
                        .view_pages(offset, bytes, PAGE_SIZE)
                        .ok_or(SystemCallError::IllegalArgument)?;
                    let usage =
                        core.acquire_use(offset, bytes, &mut operation.prep.backing_storage)?;
                    Ok((core.clone(), authorization, usage))
                },
            )?;
            if let Some(usage) = usage {
                break (accepted, authorization, usage);
            }
            // 首次复用；落败后只备完整增长块，不能再次用空候选振荡。
            drop(core::mem::take(&mut operation.prep.backing_storage));
            operation.prep.backing_storage = core.prepare_use(&pool)?;
        };
        operation.prep.usage = Some(usage.into());
        (
            MappingSource::Object {
                authorization,
                offset,
            },
            Some(core),
        )
    };
    prepare(
        &body,
        &mut operation,
        memory_space::ChangeRequest::Map(MapRequest {
            bytes,
            guard_before: before,
            guard_after: after,
            placement,
            current: protection,
            maximum: protection,
            owner: RegionOwner::AddressSpace,
            backing,
            result: Some(UserWriteLeaseRequest {
                range: result_range,
            }),
        }),
    )?;
    let footprint = reserve(&body, &mut operation, source)?;
    Ok((operation, footprint))
}

pub(crate) fn memory_unmap(
    thread: &Thread,
    address: usize,
    bytes: usize,
) -> Result<WaitPlan, SystemCallError> {
    modify(thread, address, bytes, None)
}
pub(crate) fn memory_protect(
    thread: &Thread,
    address: usize,
    bytes: usize,
    raw: usize,
) -> Result<WaitPlan, SystemCallError> {
    modify(thread, address, bytes, Some(protection(raw)?))
}
fn modify(
    thread: &Thread,
    address: usize,
    bytes: usize,
    protection: Option<Protection>,
) -> Result<WaitPlan, SystemCallError> {
    let range = PageRange::new(address, bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let mut operation = Operation::new(
        thread.process.funding_pool(),
        body.vm.clone(),
        body.views.clone(),
    )?;
    let funding_pool = operation.pool().clone();
    operation
        .prep
        .boundaries
        .try_reserve_exact(2, &crate::task::resources::MetadataFunding(&funding_pool))
        .map_err(crate::task::resources::map_storage_error)?;
    for at in [range.start(), range.end()] {
        if let Some(boundary) = body.views.prepare_boundary(at, operation.pool())? {
            operation.prep.boundaries.push(boundary);
        }
    }
    {
        let mut vm = loop {
            let vm = body.vm.space.lock();
            let ledger = vm
                .space
                .as_ref()
                .ok_or(SystemCallError::ObjectClosed)?
                .ledger();
            // 两个端点各在删除前预约两替换槽，2*2为物化准备上界。
            if ledger
                .storage_ready(4, &operation.prep.region_storage)
                .map_err(error)?
            {
                break vm;
            }
            let capacity = ledger.growth_capacity(4).map_err(error)?;
            drop(vm);
            drop(core::mem::take(&mut operation.prep.region_storage));
            operation.prep.region_storage = memory_space::RegionStorage::try_empty(
                capacity,
                &crate::task::resources::MetadataFunding(operation.pool()),
            )
            .map_err(crate::task::resources::map_storage_error)?;
        };
        let ledger = vm
            .space
            .as_mut()
            .ok_or(SystemCallError::ObjectClosed)?
            .ledger_mut();
        if ledger.span_reserved(range.address_range()) {
            return Err(SystemCallError::ObjectBusy);
        }
        if !body
            .views
            .boundaries_ready(range, &mut operation.prep.boundaries)
        {
            return Err(SystemCallError::ObjectBusy);
        }
        // 空间锁排除新的预约。实物先切界再切 ledger：后者 OOM/拒绝时实物至多
        // 比几何更细，整空间 drain 仍可逐项覆盖，不能留下相反方向的粗 View。
        for boundary in operation.prep.boundaries.iter_mut() {
            body.views.commit_boundary(boundary);
        }
        // 物化后再捕获快照，避免自己制造 Stale。
        ledger
            .split_at_boundaries(range, &mut operation.prep.region_storage)
            .map_err(error)?;
    }
    operation.prep.boundaries.clear();
    let request = match protection {
        Some(protection) => memory_space::ChangeRequest::Protect(ProtectRequest {
            range,
            protection,
            authority: RegionOwner::AddressSpace,
        }),
        None => memory_space::ChangeRequest::Unmap(UnmapRequest {
            range,
            authority: RegionOwner::AddressSpace,
        }),
    };
    prepare(&body, &mut operation, request)?;
    finish(&body, operation, None, None)
}

/// 候选块始终由失败根持有；同门复检不足才锁外补齐，不留局部付款 owner。
fn prepare(
    body: &BodyLease,
    operation: &mut Operation,
    request: memory_space::ChangeRequest,
) -> Result<(), SystemCallError> {
    let pool = operation.pool().clone();
    loop {
        let vm = body.vm.space.lock();
        let ledger = vm
            .space
            .as_ref()
            .ok_or(SystemCallError::ObjectClosed)?
            .ledger();
        match ledger.validate(&request, operation.prep.plan.candidate_mut()) {
            Ok(validated) => {
                drop(vm);
                operation.prep.plan = super::change::PreparationPlan::Validated(validated);
                return Ok(());
            }
            Err(ChangeError::CapacityChanged) => {
                let capacity = ledger.validation_capacity(&request).map_err(error)?;
                drop(vm);
                operation
                    .prep
                    .plan
                    .candidate_mut()
                    .prepare_validation(capacity, &crate::task::resources::MetadataFunding(&pool))
                    .map_err(crate::task::resources::map_storage_error)?;
            }
            Err(value) => return Err(error(value)),
        }
    }
}

fn finish(
    body: &BodyLease,
    mut operation: Operation,
    source: Option<Arc<MemoryObjectCore>>,
    output: Option<(&Thread, usize, u64)>,
) -> Result<WaitPlan, SystemCallError> {
    let footprint = reserve(body, &mut operation, source)?;
    commit(body, operation, footprint, output)
}

fn reserve(
    body: &BodyLease,
    operation: &mut Operation,
    source: Option<Arc<MemoryObjectCore>>,
) -> Result<PageRange, SystemCallError> {
    let funding_pool = operation.pool().clone();
    let funding = crate::task::resources::MetadataFunding(&funding_pool);
    operation
        .prep
        .plan
        .validated_mut()
        .prepare_reservation(&funding)
        .map_err(crate::task::resources::map_storage_error)?;
    let validated = operation.prep.plan.validated();
    let permit_count = validated
        .permit_requirements()
        .iter()
        .try_fold(0usize, |count, requirement| {
            count.checked_add(requirement.count())
        })
        .ok_or(SystemCallError::ReachLimit)?;
    operation
        .prep
        .permits
        .try_reserve_exact(permit_count, &funding)
        .map_err(crate::task::resources::map_storage_error)?;
    for requirement in validated.permit_requirements() {
        let object = source
            .as_ref()
            .filter(|object| object.identity() == requirement.object())
            .cloned()
            .or_else(|| body.views.object(requirement.object()))
            .ok_or(SystemCallError::ObjectBusy)?;
        let suffix = operation.prep.permits.len();
        object
            .reserve_replacement_writes(*requirement, &mut operation.prep.permits)
            .map_err(map_object_error)?;
        operation.prep.unindexed = Some((object.clone(), suffix));
        body.views
            .hold_writes(object, requirement.count(), &funding_pool)?;
        operation.prep.unindexed.take();
    }
    let footprint = validated.footprint();
    let prepared = loop {
        let reserved = {
            let mut vm = body.vm.space.lock();
            let ledger = vm
                .space
                .as_mut()
                .ok_or(SystemCallError::ObjectClosed)?
                .ledger_mut();
            let result = ledger.reserve(
                operation.prep.plan.take_validated(),
                &mut operation.prep.permits,
                &mut operation.prep.region_storage,
            );
            if let Ok(prepared) = &result {
                vm.dependencies
                    .push((prepared.key(), operation.completion_source()));
            }
            result
        };
        match reserved {
            Ok(prepared) => break prepared,
            Err(failure) => {
                let (value, plan) = failure.into_parts();
                operation.prep.plan = super::change::PreparationPlan::Validated(plan);
                if value != ChangeError::CapacityChanged {
                    return Err(error(value));
                }
                let capacity = {
                    let vm = body.vm.space.lock();
                    vm.space
                        .as_ref()
                        .ok_or(SystemCallError::ObjectClosed)?
                        .ledger()
                        .change_growth_capacity(operation.prep.plan.validated())
                        .map_err(error)?
                };
                drop(core::mem::take(&mut operation.prep.region_storage));
                operation.prep.region_storage =
                    memory_space::RegionStorage::try_empty(capacity, &funding)
                        .map_err(crate::task::resources::map_storage_error)?;
            }
        }
    };
    operation.prep.plan = super::change::PreparationPlan::Reserved(prepared);
    let prepared = operation.prep.plan.reserved();
    let boundary_count = prepared
        .retiring_fragments()
        .len()
        .checked_mul(2)
        .ok_or(SystemCallError::ReachLimit)?;
    operation
        .prep
        .boundaries
        .try_reserve_exact(boundary_count, &funding)
        .map_err(crate::task::resources::map_storage_error)?;
    for fragment in prepared.retiring_fragments() {
        for at in [fragment.range.start(), fragment.range.end()] {
            if let Some(boundary) = body.views.prepare_boundary(at, operation.pool())? {
                operation.prep.boundaries.push(boundary);
            }
        }
    }
    let (cursors, frames) =
        AnySpace::plan_cursors(prepared.translation_intents(), &funding, |_, page| {
            let usage = operation.prep.usage.as_ref()?;
            usage.physical_span(usage.range().0 + page as u64)
        })?;
    operation.prep.cursors = cursors;
    let pool = body
        .vm
        .space
        .lock()
        .space
        .as_ref()
        .ok_or(SystemCallError::ObjectClosed)?
        .account()
        .clone();
    AnySpace::fund_reserve(&pool, frames, &funding, &mut operation.prep.frames)?;
    let layout = prepared.map_result();
    if layout.is_some() {
        let intent = prepared
            .translation_intents()
            .iter()
            .find_map(|intent| match *intent {
                memory_space::TranslationIntent::Install { .. } => Some(*intent),
                _ => None,
            })
            .expect("map reservation has no install intent");
        let usage = operation
            .prep
            .usage
            .take()
            .expect("map preparation has no backing use");
        match body
            .views
            .prepare_map(intent, usage, source, operation.pool())
        {
            Ok(view) => operation.prep.view = Some(view),
            Err((error, usage)) => {
                operation.prep.usage = Some(usage);
                return Err(error);
            }
        }
    }
    Ok(footprint)
}

fn commit(
    body: &BodyLease,
    mut operation: Operation,
    footprint: PageRange,
    output: Option<(&Thread, usize, u64)>,
) -> Result<WaitPlan, SystemCallError> {
    let layout = operation.prep.plan.reserved().map_result();
    // 资源发布许可在 VM 门外取得；先行的 Seal/Revoke 使原操作回滚。
    operation.grant_publication()?;
    let progress = {
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        if let Some((thread, address, cookie)) = output {
            let layout = layout.expect("map result has no geometry");
            let result = MemoryMapResult {
                usable_base: layout.usable.start() as u64,
                usable_bytes: layout.usable.bytes() as u64,
                reservation_base: layout.reservation.start() as u64,
                reservation_bytes: layout.reservation.bytes() as u64,
                reserved: [0; 3],
                committed: 0,
            };
            // SAFETY: payload 是固定整数 ABI 无 padding 的前缀，不以非原子方式覆写 committed。
            let payload = unsafe {
                core::slice::from_raw_parts(
                    (&result as *const MemoryMapResult).cast::<u8>(),
                    core::mem::offset_of!(MemoryMapResult, committed),
                )
            };
            uaccess::deliver_bytes(thread, space, address, payload)?;
            let commit_address = address + core::mem::offset_of!(MemoryMapResult, committed);
            uaccess::publish_u64(space, commit_address, cookie).map_err(|error| {
                thread.defer_output_fault(commit_address);
                SystemCallError::from(error)
            })?;
        }
        // cookie 之后所有步骤必成；冻结只等已接受操作，不撤销出生/索引责任。
        for boundary in operation.prep.boundaries.iter_mut() {
            body.views.commit_boundary(boundary);
        }
        if let Some(view) = operation.prep.view.as_mut() {
            body.views.commit_map(view);
        }
        let committed = space
            .ledger_mut()
            .commit(operation.prep.plan.take_reserved());
        ChangeProgress::new(
            committed,
            footprint,
            core::mem::take(&mut operation.prep.cursors),
            core::mem::take(&mut operation.prep.frames),
        )
    };
    // 许可后若 Stop 已到达，仍由同一已提交操作完成发布，再交强制退休收束。
    operation.commit_publication();
    operation.prep.boundaries.clear();
    drop(operation.prep.view.take());
    Ok(operation.submit(progress))
}
