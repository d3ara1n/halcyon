//! 公共 MemoryObject 与 Tunnel 的统一对象 core。
//!
//! `MemoryObjectCore` 同时持：ObjectBacking（固定长度、堆化多 extent 的资金化
//! backing）、MemoryObjectState（Mutable → Sealing → Executable 状态机，内含对象身份
//! 与固定长度）、对象自身的等待面与 metadata owner（sponsor 强引用 + backing permit）。
//!
//! 对象身份从内核对象身份序列（`object::try_mint_koid`）铸造后交给状态机保管，不在
//! core 上另存一份——身份、长度与可执行状态同属对象的逻辑状态，单一真值点避免二者
//! 失步。
//!
//! 等待面按信号归属分层：`EXECUTABLE` 是对象自身的电平，与状态机共用同一把对象锁；
//! Tunnel 的 `DATA`/`PEER_CLOSED`/`CLOSED` 属于 Endpoint 关系状态，由 Endpoint 各自
//! 拥有，不进入 core。

use alloc::{sync::Arc, vec::Vec};

use memory_space::{
    ExecutableState, MemoryObjectState, ObjectError, ObjectId, ObjectViewAuthorization, Protection,
    SealOutcome, WritePermit,
};

use crate::{
    frame::{self, ObjectBacking},
    sync::Spinlock,
    task::{
        memory_pool::MemoryPool,
        object::{self, ObjectWaitState, SubscribeResult},
        resources::{ConnectionPermit, MetadataSponsor, ObjectBackingPermit},
        wait::Subscription,
    },
};

use erhino_shared::{
    call::SystemCallError,
    memory_object::{MemoryObjectSnapshot, MemoryObjectState as AbiObjectState},
    object::ObjectSignals,
};

fn mint_object_id() -> Result<ObjectId, SystemCallError> {
    let koid = object::try_mint_koid().ok_or(SystemCallError::ReachLimit)?;
    ObjectId::new(koid).ok_or(SystemCallError::InternalError)
}

/// 把资金化失败分类为系统调用错误：额度不足与物理/metadata 不足不同，
/// 结构硬上限也单独区分，不得统一折成 `OutOfMemory`。
fn map_fund_error(
    error: funded_frame::FundError<memory_pool::PoolError, frame::UserClaimError>,
) -> SystemCallError {
    match error {
        funded_frame::FundError::Quota(memory_pool::PoolError::QuotaExceeded) => {
            SystemCallError::QuotaExceeded
        }
        funded_frame::FundError::Quota(_) => SystemCallError::OutOfMemory,
        funded_frame::FundError::PageLimit | funded_frame::FundError::ExtentLimit => {
            SystemCallError::ReachLimit
        }
        funded_frame::FundError::ZeroPages
        | funded_frame::FundError::InvalidClaim
        | funded_frame::FundError::Physical(_) => SystemCallError::OutOfMemory,
    }
}

pub(crate) fn map_object_error(error: ObjectError) -> SystemCallError {
    match error {
        ObjectError::AllocationFailed => SystemCallError::OutOfMemory,
        ObjectError::PermitLimit => SystemCallError::ReachLimit,
        ObjectError::ViewDenied | ObjectError::PermitDenied => SystemCallError::ObjectBusy,
        ObjectError::PermitOverflow => SystemCallError::InternalError,
    }
}

/// 状态机与等待面共用同一把对象锁：可执行发布与 `EXECUTABLE` 电平必须原子推进。
struct CoreState {
    machine: MemoryObjectState,
    wait: ObjectWaitState,
}

/// 公共 MemoryObject 与 Tunnel Connection 的统一 core。
///
/// 持有对象身份、backing、状态机、对象电平与 metadata 生命周期 owner。状态经
/// `MEMORY_OBJECT` 锁秩包裹；任何 view 事务都在 AddressSpace 锁外与它交互。
pub(crate) struct MemoryObjectCore {
    pub(crate) backing: ObjectBacking,
    state: Spinlock<CoreState>,
    _sponsor: Arc<MetadataSponsor>,
    _backing_permit: ObjectBackingPermit,
}

impl MemoryObjectCore {
    fn new(
        pool: &Arc<MemoryPool>,
        sponsor: &Arc<MetadataSponsor>,
        pages: usize,
        permit_limit: usize,
    ) -> Result<Self, SystemCallError> {
        let backing_permit = MetadataSponsor::reserve_object_backing(sponsor)?;
        let identity = mint_object_id()?;
        let backing = frame::fund_object_backing(
            pool,
            pages,
            funded_frame::Limits {
                max_pages: pages,
                max_extents: frame::MAX_FUNDED_EXTENTS,
            },
        )
        .map_err(map_fund_error)?;
        let object_bytes = backing.pages() * super::proc::PAGE_SIZE;
        Ok(Self {
            backing,
            state: Spinlock::new(
                crate::sync::ranks::MEMORY_OBJECT,
                CoreState {
                    machine: MemoryObjectState::new(identity, object_bytes, permit_limit),
                    wait: ObjectWaitState::new(ObjectSignals::NONE),
                },
            ),
            _sponsor: Arc::clone(sponsor),
            _backing_permit: backing_permit,
        })
    }

    /// 公共 MemoryObject：长度由调用者声明，permit 上限跟随可同时存在的 view 数。
    pub(crate) fn new_public(
        pool: &Arc<MemoryPool>,
        sponsor: &Arc<MetadataSponsor>,
        pages: usize,
    ) -> Result<Arc<Self>, SystemCallError> {
        let core = Self::new(pool, sponsor, pages, MAX_WRITE_VIEWS_PER_OBJECT)?;
        Arc::try_new(core).map_err(|_| SystemCallError::OutOfMemory)
    }

    /// 为 Tunnel Connection 创建内部对象 core（单页，可变）。
    ///
    /// 除了 backing 长度固定为单页外，其余与公共 MemoryObject 完全相同——同一套
    /// ObjectId 铸造、状态机与 metadata admission。Connection 两端 Endpoint 共享
    /// 同一对象，因此最多两个可写 view。
    pub(crate) fn new_tunnel_connection(
        pool: &Arc<MemoryPool>,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<(Self, ConnectionPermit), SystemCallError> {
        let connection_permit = MetadataSponsor::reserve_connection(sponsor)?;
        let core = Self::new(pool, sponsor, 1, 2)?;
        Ok((core, connection_permit))
    }

    pub(crate) fn identity(&self) -> ObjectId {
        self.state.lock().machine.object()
    }

    /// 在对象锁内取得 view 准入；含 W 的 view 还必须取得等量 [`WritePermit`]。
    pub(crate) fn authorize_view(
        &self,
        maximum: Protection,
    ) -> Result<ObjectViewAuthorization, ObjectError> {
        self.state.lock().machine.authorize_view(maximum)
    }

    /// view 准入与写许可在同一次加锁内取得，与 seal 在对象锁上线性化。
    pub(crate) fn authorize_write_view(
        &self,
        maximum: Protection,
    ) -> Result<(ObjectViewAuthorization, Vec<WritePermit>), ObjectError> {
        let mut state = self.state.lock();
        let authorization = state.machine.authorize_view(maximum)?;
        let permits = state.machine.reserve_writes(1)?;
        Ok((authorization, permits))
    }

    /// 放弃尚未提交的写许可；Commit 前的回滚路径专用。permit 可逐个归还——
    /// Unmap/Protect 的预留失败路径会按来源逐个退还。
    pub(crate) fn cancel_writes(&self, permits: Vec<WritePermit>) {
        for permit in permits {
            self.cancel_write(permit);
        }
    }

    pub(crate) fn cancel_write(&self, permit: WritePermit) {
        let published = self.state.lock().machine.cancel_write(permit);
        self.finish_seal(published);
    }

    /// 在对象锁内预留一批写许可（view 准入与 seal 线性化共用同一把锁）。
    pub(crate) fn reserve_writes(&self, count: usize) -> Result<Vec<WritePermit>, ObjectError> {
        self.state.lock().machine.reserve_writes(count)
    }

    /// 地址翻译确认后退役单个写许可；最后一个 permit 的退役发布 `EXECUTABLE`。
    /// 退役路径不得失败，因此按 permit 逐个归还，不构造中间容器。
    pub(crate) fn retire_write(&self, permit: WritePermit) {
        let published = self.state.lock().machine.retire_write(permit);
        self.finish_seal(published);
    }

    /// 单向请求可执行发布。返回 true 表示已进入 Executable 终态。
    pub(crate) fn seal(&self) -> bool {
        let outcome = self.state.lock().machine.seal();
        let published = outcome == SealOutcome::Published;
        self.finish_seal(published);
        published
    }

    pub(crate) fn snapshot(&self) -> MemoryObjectSnapshot {
        let state = self.state.lock();
        let abi = match state.machine.state() {
            ExecutableState::Mutable => AbiObjectState::Mutable,
            ExecutableState::Sealing => AbiObjectState::Sealing,
            ExecutableState::Executable => AbiObjectState::Executable,
        };
        MemoryObjectSnapshot {
            identity: state.machine.object().get(),
            bytes: state.machine.object_bytes() as u64,
            write_views: state.machine.permit_count() as u64,
            state: abi as u32,
            reserved0: 0,
            reserved: [0; 4],
        }
    }

    pub(crate) fn signals(&self) -> ObjectSignals {
        self.state.lock().wait.signals()
    }

    pub(crate) fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.state.lock().wait.subscribe(subscription)
    }

    pub(crate) fn unsubscribe(&self, id: u64) {
        self.state.lock().wait.unsubscribe(id);
    }

    /// 发布 `EXECUTABLE` 电平并在对象锁外完成等待者。电平置位后持续为真，
    /// 因此发起 seal 的线程消散不影响已完成的状态转换。
    fn finish_seal(&self, published: bool) {
        if !published {
            return;
        }
        self.state
            .lock()
            .wait
            .update(ObjectSignals::NONE, ObjectSignals::EXECUTABLE);
        let pending = {
            let mut state = self.state.lock();
            state.wait.take_notification()
        };
        if let Some((reservation, target)) = pending {
            reservation.publish(target);
        }
    }
}

/// 单个对象可同时存在的可写 view 上限。它与 ObjectView admission 一起给协作式
/// 退役路径明确的工作上界，不是对象总 view 数上限。
pub(crate) const MAX_WRITE_VIEWS_PER_OBJECT: usize = 64;

/// 公共 MemoryObject 的用户可见对象壳。
///
/// 它不持有独立状态：身份、长度、可执行状态与等待面都在 core 上，壳只提供 Handle
/// 类型、rights 上限与可观察电平。MemoryObject 没有 owner role——全部 Handle 是同一
/// capability，因此 close 不产生终态信号，对象在最后一个引用（Handle 或 view）消散
/// 时自然析构。
pub struct MemoryObject {
    header: object::ObjectHeader,
    core: Arc<MemoryObjectCore>,
}

impl MemoryObject {
    fn new(core: Arc<MemoryObjectCore>) -> Result<Arc<Self>, SystemCallError> {
        let header = object::ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?;
        Arc::try_new(Self { header, core }).map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(this: &Arc<Self>) -> object::ObjectRef {
        this.clone()
    }

    pub(crate) fn core(&self) -> &Arc<MemoryObjectCore> {
        &self.core
    }
}

/// MemoryObject Handle 的 rights 上限。`MAP` 与数据 rights 正交：只读 view 要求
/// `MAP|READ`，可写 view 要求 `MAP|READ|WRITE`，执行 view 要求 `MAP|READ|EXECUTE`。
const MEMORY_OBJECT_RIGHTS: erhino_shared::object::Rights = {
    use erhino_shared::object::Rights;
    Rights::from_raw(
        Rights::MAP.raw()
            | Rights::READ.raw()
            | Rights::WRITE.raw()
            | Rights::WAIT.raw()
            | Rights::MANAGE.raw()
            | Rights::DUPLICATE.raw()
            | Rights::TRANSIT.raw()
            | Rights::GRANT.raw()
            | Rights::EXECUTE.raw(),
    )
};

impl object::KernelObject for MemoryObject {
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.core
            .state
            .lock()
            .wait
            .complete_notification(reservation)
    }

    fn drain_waiters(&self, budget: usize) -> (usize, bool) {
        let mut used = 0;
        while used < budget {
            let advance = { self.core.state.lock().wait.advance_waiter() };
            match advance {
                super::object::WaitAdvance::Progress => used += 1,
                super::object::WaitAdvance::Complete(context) => {
                    super::wait::finish_offered(context);
                    used += 1;
                }
                super::object::WaitAdvance::Done => return (used, true),
            }
        }
        (used, false)
    }

    fn header(&self) -> &object::ObjectHeader {
        &self.header
    }

    fn kind(&self) -> object::ObjectKind {
        object::ObjectKind::MemoryObject
    }

    fn allowed_rights(&self, role: object::HandleRole) -> Option<erhino_shared::object::Rights> {
        (role == object::HandleRole::MemoryObject).then_some(MEMORY_OBJECT_RIGHTS)
    }

    fn allowed_signals(&self, role: object::HandleRole) -> Option<ObjectSignals> {
        (role == object::HandleRole::MemoryObject).then_some(ObjectSignals::EXECUTABLE)
    }

    fn signals(&self) -> ObjectSignals {
        self.core.signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.core.subscribe(subscription)
    }

    fn unsubscribe(&self, id: u64) {
        self.core.unsubscribe(id);
    }

    fn close_handle(
        &self,
        role: object::HandleRole,
        _owner: &super::proc::Process,
        _exiting: bool,
    ) {
        debug_assert!(role == object::HandleRole::MemoryObject);
    }

    fn close_transit(&self, role: object::HandleRole) {
        debug_assert!(role == object::HandleRole::MemoryObject);
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

fn resolve(
    thread: &super::Thread,
    handle: erhino_shared::object::Handle,
    required: erhino_shared::object::Rights,
) -> Result<Arc<MemoryObject>, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table
        .get(handle, required)
        .map_err(super::handle::map_error)?;
    if *entry.role() != object::HandleRole::MemoryObject
        || entry.object().kind() != object::ObjectKind::MemoryObject
    {
        return Err(SystemCallError::WrongObjectType);
    }
    let any: Arc<dyn core::any::Any + Send + Sync> = entry.object().clone();
    any.downcast::<MemoryObject>()
        .map_err(|_| SystemCallError::WrongObjectType)
}

/// 从当前进程绑定池创建固定长度对象，并把完整 rights 的 Handle 写出。
pub fn create(thread: &super::Thread, request_ptr: usize) -> Result<(), SystemCallError> {
    use erhino_shared::memory_object::{MEMORY_OBJECT_MAX_PAGES, MemoryObjectCreateRequest};

    let (request, pool, sponsor) = {
        let mut space = thread.process.space.lock();
        // SAFETY: MemoryObjectCreateRequest 只含整数且无 padding，任意位型均有效。
        let request: MemoryObjectCreateRequest =
            unsafe { crate::uaccess::read_user_value(&mut space, request_ptr) }?;
        let bound = space.bound()?;
        (
            request,
            Arc::clone(bound.pool()),
            Arc::clone(bound.sponsor()),
        )
    };
    if request.reserved != [0; 2] {
        return Err(SystemCallError::IllegalArgument);
    }
    let bytes = usize::try_from(request.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    if bytes == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let pages = bytes.div_ceil(super::proc::PAGE_SIZE);
    // 可由普通 Handle close 触发最终析构的对象必须受硬容量上限约束。
    if u64::try_from(pages).map_err(|_| SystemCallError::IllegalArgument)? > MEMORY_OBJECT_MAX_PAGES
    {
        return Err(SystemCallError::ReachLimit);
    }
    let result_address =
        usize::try_from(request.result_address).map_err(|_| SystemCallError::IllegalArgument)?;

    let core = MemoryObjectCore::new_public(&pool, &sponsor, pages)?;
    let object = MemoryObject::new(core)?;
    let entry = super::handle::entry(
        MemoryObject::object_ref(&object),
        object::HandleRole::MemoryObject,
        MEMORY_OBJECT_RIGHTS,
    )
    .map_err(super::handle::map_error)?;
    super::handle::install_one(thread, entry, result_address, || {})
}

/// 读固定宽对象快照。identity 只作诊断。
pub fn query(
    thread: &super::Thread,
    handle: erhino_shared::object::Handle,
    output: usize,
) -> Result<(), SystemCallError> {
    use erhino_shared::{memory_object::MemoryObjectSnapshot, object::Rights};

    let object = resolve(thread, handle, Rights::READ)?;
    let snapshot = object.core.snapshot();
    let mut space = thread.process.space.lock();
    space.check_range(output, core::mem::size_of::<MemoryObjectSnapshot>(), true)?;
    // SAFETY: MemoryObjectSnapshot 无 padding；check_range 已验证当前可写范围。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &snapshot) }
}

/// 单向发布对象为可执行。要求 `MANAGE`；在 Executable 上幂等成功。等待完成经
/// WaitMany 观察 `EXECUTABLE` 电平，本调用不阻塞也不登记等待者。
pub fn seal(
    thread: &super::Thread,
    handle: erhino_shared::object::Handle,
) -> Result<(), SystemCallError> {
    use erhino_shared::object::Rights;

    let object = resolve(thread, handle, Rights::MANAGE)?;
    object.core.seal();
    Ok(())
}

/// 为当前 Running process 建立公共 MemoryObject 的 view。
///
/// rights 与 view 权限正交：只读 view 要求 `MAP|READ`，可写 view 追加 `WRITE`，
/// 读执行 view 追加 `EXECUTE`。view 由地址空间拥有（普通 `MemoryUnmap` 可撤销），
/// 并以强引用独立保活对象——Handle 先关闭不影响已建立的 view。
pub(crate) fn map_view(
    thread: &super::Thread,
    process: Arc<super::proc::Process>,
    intent: super::proc::MapIntent,
    handle: erhino_shared::object::Handle,
    offset: usize,
    sponsor: &Arc<MetadataSponsor>,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    use erhino_shared::object::Rights;

    let protection = intent.protection();
    let required = match protection {
        Protection::ReadOnly => Rights::MAP | Rights::READ,
        Protection::ReadWrite => Rights::MAP | Rights::READ | Rights::WRITE,
        Protection::ReadExecute => Rights::MAP | Rights::READ | Rights::EXECUTE,
    };
    let object = resolve(thread, handle, required)?;
    let core = Arc::clone(object.core());

    // 含 W 的 view 与 seal 在对象锁上线性化；只读/读执行 view 不取 permit。
    let (authorization, permits) = if protection == Protection::ReadWrite {
        core.authorize_write_view(protection)
            .map_err(map_object_error)?
    } else {
        (
            core.authorize_view(protection).map_err(map_object_error)?,
            Vec::new(),
        )
    };

    let map_result = (|| {
        let pages = authorization
            .view_pages(offset, intent.bytes(), super::proc::PAGE_SIZE)
            .ok_or(SystemCallError::IllegalArgument)?;
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(core.backing.projection_capacity())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        core.backing
            .project(offset / super::proc::PAGE_SIZE, pages, &mut spans);
        let view_owner = super::proc::PreparedObjectView::new(Arc::clone(&core), sponsor)
            .map_err(SystemCallError::from)?;
        Ok((spans, view_owner))
    })();
    let (spans, view_owner) = match map_result {
        Ok(prepared) => prepared,
        Err(error) => {
            core.cancel_writes(permits);
            return Err(error);
        }
    };

    let plan_result = {
        let mut space = process.space.lock();
        space.plan_object_map(&intent, offset, &spans, authorization, permits, view_owner)
    };
    let plan = match plan_result {
        Ok(plan) => plan,
        Err(failure) => {
            core.cancel_writes(failure.permits);
            return Err(SystemCallError::from(failure.error));
        }
    };
    super::proc::finish_running_map(thread, process, plan)
}
