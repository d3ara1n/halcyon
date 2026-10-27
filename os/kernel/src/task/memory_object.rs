//! MemoryObject/Tunnel 的 seal 与 backing core。普通 Arc/观察 loan 只保活状态，
//! 物理使用由 BackingOwner 的 root use 和各 View 的 BackingUse 独立持有。

use super::{
    handle::{self, Authorization},
    memory_pool::MemoryPool,
    object::{self, KernelObject, ObjectHeader, ObjectKind},
    wait_source::{Hint, ObservationSource, Snapshot, Step, WaitNode, WaitQueue},
};
use crate::{
    frame,
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    vm::backing::{Backing, BackingOwner},
};
use alloc::sync::Arc;
use core::{any::Any, pin::Pin};
use erhino_shared::{
    call::SystemCallError,
    memory_object::{MemoryObjectSnapshot, MemoryObjectState as AbiObjectState},
    object::{Handle, ObjectSignals, Rights},
};
use memory_space::{
    ExecutableState, MemoryObjectState, ObjectError, ObjectId, ObjectViewAuthorization, PAGE_SIZE,
    PermitRequirement, Protection, SealOutcome, WritePermit,
};

fn mint_object_id() -> Result<ObjectId, SystemCallError> {
    let koid = object::try_mint_koid().ok_or(SystemCallError::ReachLimit)?;
    ObjectId::new(koid).ok_or(SystemCallError::InternalError)
}

pub(crate) fn map_object_error(error: ObjectError) -> SystemCallError {
    match error {
        ObjectError::CapacityChanged => SystemCallError::ReachLimit,
        ObjectError::PermitLimit | ObjectError::PermitOverflow => SystemCallError::ReachLimit,
        ObjectError::ViewDenied | ObjectError::PermitDenied => SystemCallError::ObjectBusy,
    }
}

pub(crate) struct MemoryObjectCore {
    backing: Arc<Backing>,
    state: Spinlock<MemoryObjectState>,
    source: WaitQueue,
    /// 来源 Work 使用稳定存储身份，不铸造 capability 或业务 use。
    this: Spinlock<alloc::sync::Weak<Self>>,
    _metadata: super::resources::MetadataCharge,
}

impl MemoryObjectCore {
    /// 准备返回两种不同 owner：共享状态 Arc 与仅一份完整 backing use。
    pub(crate) fn new(
        pool: Arc<MemoryPool>,
        pages: usize,
    ) -> Result<(Arc<Self>, BackingOwner), SystemCallError> {
        if pages == 0 || pages as u64 > erhino_shared::memory_object::MEMORY_OBJECT_MAX_PAGES {
            return Err(SystemCallError::ReachLimit);
        }
        let bytes = pages
            .checked_mul(PAGE_SIZE)
            .ok_or(SystemCallError::IllegalArgument)?;
        let identity = mint_object_id()?;
        let owner = Backing::fund(
            pool.clone(),
            pages,
            funded_frame::Limits {
                max_pages: erhino_shared::memory_object::MEMORY_OBJECT_MAX_PAGES as usize,
                max_extents: frame::MAX_FUNDED_EXTENTS,
            },
        )?;
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let core = Arc::try_new(Self {
            backing: owner.backing().clone(),
            // 并发可写 view 不决定单步工作量；每份许可有实际准备存储，保留计数表示上限。
            state: Spinlock::new(
                ranks::MEMORY_OBJECT,
                MemoryObjectState::new(identity, bytes, usize::MAX),
            ),
            source: WaitQueue::new(pool.maintenance_account().clone()),
            this: Spinlock::new(ranks::LEAF, alloc::sync::Weak::new()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *core.this.lock() = Arc::downgrade(&core);
        Ok((core, owner))
    }

    pub(crate) fn identity(&self) -> ObjectId {
        self.state.lock().object()
    }
    /// 在来源 capability 尚受表锁保护时取得独立 use；由 VM 操作持有到提交/回滚。
    pub(crate) fn acquire_use(
        &self,
        offset: usize,
        bytes: usize,
        storage: &mut crate::vm::backing::BackingStorage,
    ) -> Result<Option<crate::vm::backing::BackingUse>, SystemCallError> {
        if !offset.is_multiple_of(PAGE_SIZE) || !bytes.is_multiple_of(PAGE_SIZE) || bytes == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let lo = (offset / PAGE_SIZE) as u64;
        let hi = lo
            .checked_add((bytes / PAGE_SIZE) as u64)
            .ok_or(SystemCallError::IllegalArgument)?;
        self.backing.acquire(lo, hi, storage)
    }
    pub(crate) fn prepare_use(
        &self,
        pool: &Arc<MemoryPool>,
    ) -> Result<crate::vm::backing::BackingStorage, SystemCallError> {
        self.backing.prepare_storage(2, pool)
    }
    pub(crate) fn begin_pin(
        &self,
        offset: usize,
        bytes: usize,
        storage: &mut crate::vm::backing::BackingStorage,
    ) -> Result<Option<crate::vm::backing::BackingAcquisition>, SystemCallError> {
        if bytes == 0 || !offset.is_multiple_of(PAGE_SIZE) || !bytes.is_multiple_of(PAGE_SIZE) {
            return Err(SystemCallError::IllegalArgument);
        }
        let end = offset
            .checked_add(bytes)
            .ok_or(SystemCallError::IllegalArgument)?;
        self.backing.begin_acquire(
            (offset / PAGE_SIZE) as u64,
            (end / PAGE_SIZE) as u64,
            storage,
        )
    }
    /// 独立设备写者，不借 CPU View/replacement 的计数或资格。
    pub(crate) fn reserve_dma_write<C>(
        &self,
        output: &mut funded_storage::Buffer<WritePermit, C>,
    ) -> Result<(), ObjectError> {
        self.state.lock().reserve_writes(1, output)
    }
    pub(crate) fn authorize_view(
        &self,
        maximum: Protection,
    ) -> Result<ObjectViewAuthorization, ObjectError> {
        self.state.lock().authorize_view(maximum)
    }

    pub(crate) fn reserve_replacement_writes<C>(
        &self,
        requirement: PermitRequirement,
        output: &mut funded_storage::Buffer<WritePermit, C>,
    ) -> Result<(), ObjectError> {
        self.state
            .lock()
            .reserve_replacement_writes(requirement, output)
    }

    pub(crate) fn cancel_write(&self, permit: WritePermit) {
        let notify = {
            let mut state = self.state.lock();
            let published = state.cancel_write(permit);
            self.publish_locked(published)
        };
        if notify {
            self.kick();
        }
    }
    pub(crate) fn retire_write(&self, permit: WritePermit) {
        let notify = {
            let mut state = self.state.lock();
            let published = state.retire_write(permit);
            self.publish_locked(published)
        };
        if notify {
            self.kick();
        }
    }
    pub(crate) fn seal(&self) -> bool {
        let (published, notify) = {
            let mut state = self.state.lock();
            let published = state.seal() == SealOutcome::Published;
            (published, self.publish_locked(published))
        };
        if notify {
            self.kick();
        }
        published
    }
    /// 调用者持 MEMORY_OBJECT；machine 与公开电平同域提交。
    fn publish_locked(&self, published: bool) -> bool {
        if !published {
            return false;
        }
        // SAFETY: core 位于稳定 Arc，所有来源链与 machine 操作受同一锁保护。
        unsafe { self.source.update(ObjectSignals::EXECUTABLE, false) }
    }
    fn kick(&self) {
        let root: Arc<dyn Work> = self
            .this
            .lock()
            .upgrade()
            .expect("live memory object lost source root");
        mt::publish(root);
        mt::kick();
    }
    pub(crate) fn snapshot(&self) -> MemoryObjectSnapshot {
        let state = self.state.lock();
        let abi = match state.state() {
            ExecutableState::Mutable => AbiObjectState::Mutable,
            ExecutableState::Sealing => AbiObjectState::Sealing,
            ExecutableState::Executable => AbiObjectState::Executable,
        };
        MemoryObjectSnapshot {
            identity: state.object().get(),
            bytes: state.object_bytes() as u64,
            write_permits: state.permit_count() as u64,
            state: abi as u32,
            reserved0: 0,
            reserved: [0; 4],
        }
    }
}

impl ObservationSource for MemoryObjectCore {
    fn snapshot(&self) -> Snapshot {
        let _state = self.state.lock();
        // SAFETY: 持该来源唯一同步域。
        unsafe { self.source.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _state = self.state.lock();
        // SAFETY: 调用者保证 pin 节点/loan 至 detach 返回，来源锁串行链操作。
        unsafe { self.source.attach(node, interest, hint) }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _state = self.state.lock();
        // SAFETY: 同挂链契约；返还凭据不含节点借用。
        unsafe { self.source.detach(node) }
    }
}
impl Work for MemoryObjectCore {
    fn record(&self) -> &WorkRecord {
        self.source.record()
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let step = {
            let _state = self.state.lock();
            // SAFETY: 每步检查一个来源候选，只复制在途身份/强目标。
            unsafe { self.source.step() }
        };
        match step {
            Step::Idle => maintenance::record::Outcome::Idle,
            Step::Progress => maintenance::record::Outcome::Ready,
            Step::Hint(delivery) => {
                delivery.send();
                maintenance::record::Outcome::Ready
            }
        }
    }
}

pub struct MemoryObject {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<MemoryObjectCore>,
    owner: Spinlock<Option<BackingOwner>>,
    _metadata: super::resources::MetadataCharge,
}
impl MemoryObject {
    pub(crate) fn prepare_boot(
        pool: Arc<MemoryPool>,
        bytes: usize,
    ) -> Result<Arc<Self>, SystemCallError> {
        let (core, owner) = MemoryObjectCore::new(pool, bytes.div_ceil(PAGE_SIZE))?;
        Self::new(core, owner)
    }

    /// # Safety
    /// 仅允许 bootstrap 在初始进程 Start 前使用；全部已铸能力仍在不可执行的
    /// Building 进程中，没有用户映射或其他消费者。调用完成后才能 Start。
    pub(crate) unsafe fn initialize_boot_page(
        &self,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), SystemCallError> {
        let owner = self
            .owner
            .lock()
            .take()
            .ok_or(SystemCallError::ObjectClosed)?;
        // SAFETY: 完整域 owner 独占保活，调用者承担 Start 前独占初始化合同。
        // 不持 LEAF 锁进入 BACKING；此期间没有其他 capability 终结者。
        let result = unsafe { owner.initialize_page(offset, bytes) };
        *self.owner.lock() = Some(owner);
        result
    }

    fn new(core: Arc<MemoryObjectCore>, owner: BackingOwner) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(core._metadata.payer())?;
        Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            core,
            owner: Spinlock::new(ranks::LEAF, Some(owner)),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
    pub(crate) fn core(&self) -> &Arc<MemoryObjectCore> {
        &self.core
    }
}
const MEMORY_OBJECT_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw()
        | Rights::EXECUTE.raw(),
);

impl KernelObject for MemoryObject {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::MemoryObject
    }
    fn related_id(&self) -> u64 {
        self.core.identity().get()
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(MEMORY_OBJECT_RIGHTS)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::EXECUTABLE)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.core.clone())
    }
    fn on_last_capability(&self) {
        let owner = self.owner.lock().take();
        if let Some(owner) = owner {
            drop(owner);
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn resolve(
    thread: &super::Thread,
    handle: Handle,
    rights: Rights,
) -> Result<(Authorization, Arc<MemoryObjectCore>), SystemCallError> {
    let invocation = Authorization::accept(thread, handle, rights)?;
    let object = invocation
        .object()
        .as_any()
        .downcast_ref::<MemoryObject>()
        .ok_or(SystemCallError::WrongObjectType)?;
    let core = object.core.clone();
    Ok((invocation, core))
}

pub fn create(thread: &super::Thread, request_ptr: usize) -> Result<(), SystemCallError> {
    use erhino_shared::memory_object::{MEMORY_OBJECT_MAX_PAGES, MemoryObjectCreateRequest};
    let request: MemoryObjectCreateRequest = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        // SAFETY: 固定宽请求仅整数且无 padding，随后验证值和保留字段。
        unsafe { crate::uaccess::read_user_value(space, request_ptr) }?
    };
    if request.reserved != [0; 2] || request.bytes == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let pages = request.bytes.div_ceil(PAGE_SIZE as u64);
    if pages > MEMORY_OBJECT_MAX_PAGES {
        return Err(SystemCallError::ReachLimit);
    }
    let output =
        usize::try_from(request.result_address).map_err(|_| SystemCallError::IllegalArgument)?;
    let (core, owner) = MemoryObjectCore::new(thread.process.funding_pool(), pages as usize)?;
    let object = MemoryObject::new(core, owner)?;
    let cap = handle::mint(object, MEMORY_OBJECT_RIGHTS)?;
    let caps = [cap];
    handle::install(
        thread,
        caps,
        |handles| crate::uaccess::write_user(thread, output, &handles[0]),
        || {},
    )
    .map(|_| ())
}

pub fn query(thread: &super::Thread, handle: Handle, output: usize) -> Result<(), SystemCallError> {
    let (_authority, core) = resolve(thread, handle, Rights::READ)?;
    crate::uaccess::write_user(thread, output, &core.snapshot())
}

pub fn seal(thread: &super::Thread, handle: Handle) -> Result<(), SystemCallError> {
    let (_authority, core) = resolve(thread, handle, Rights::MANAGE)?;
    core.seal();
    Ok(())
}
