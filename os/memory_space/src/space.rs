use crate::ordmap::{OrdMap, OrdMapStorage};

#[derive(Debug)]
pub enum SpaceCreateError<E> {
    BadLimits,
    Storage(funded_storage::StorageError<E>),
}

pub struct RegionStorage<C> {
    index: OrdMapStorage<usize, Region, C>,
}
impl<C> RegionStorage<C> {
    pub const fn new() -> Self {
        Self {
            index: OrdMapStorage::new(),
        }
    }
    pub fn try_empty<F: funded_storage::Funding<Charge = C>>(
        capacity: usize,
        payer: &F,
    ) -> Result<Self, funded_storage::StorageError<F::Error>> {
        Ok(Self {
            index: OrdMapStorage::try_empty(capacity, payer)?,
        })
    }
}
impl<C> Default for RegionStorage<C> {
    fn default() -> Self {
        Self::new()
    }
}

use crate::{
    AddressRange, ObjectId, ObjectViewAuthorization, PAGE_SIZE, PageRange, RangeError, WritePermit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BackingId(u64);

impl BackingId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeaseKey(u64);

impl LeaseKey {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllocationKey(u64);

impl AllocationKey {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegionKey(u64);

impl RegionKey {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 只由预约 token 取得的不透明身份，用于同一空间中“忽略自身”的末项收束。
pub struct ChangeKey(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    ReadOnly,
    ReadWrite,
    ReadExecute,
}

impl Protection {
    pub const fn permits(self, requested: Self) -> bool {
        matches!(
            (self, requested),
            (Self::ReadOnly, Self::ReadOnly)
                | (Self::ReadWrite, Self::ReadOnly | Self::ReadWrite)
                | (Self::ReadExecute, Self::ReadOnly | Self::ReadExecute)
        )
    }

    pub const fn writable(self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnonymousClass {
    Data,
    InitialExecutable,
}

/// 一次 MMIO Map 的稳定身份。由资源 owner 单调签发，切分不换号，VA 重用不复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MappingId(u64);

impl MappingId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MappingSource {
    Anonymous {
        identity: BackingId,
        class: AnonymousClass,
    },
    Object {
        authorization: ObjectViewAuthorization,
        offset: usize,
    },
    /// 本次 Map 已由资源 owner 准入的 IO 长度；不表示 RAM 或 MemoryObject 授权。
    Mmio { mapping: MappingId, bytes: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingOrigin {
    Anonymous {
        identity: BackingId,
        class: AnonymousClass,
        offset: usize,
    },
    Object {
        object: ObjectId,
        offset: usize,
    },
    Mmio {
        mapping: MappingId,
        offset: usize,
    },
}

impl MappingOrigin {
    pub const fn offset(self) -> usize {
        match self {
            Self::Anonymous { offset, .. }
            | Self::Object { offset, .. }
            | Self::Mmio { offset, .. } => offset,
        }
    }

    pub const fn object(self) -> Option<ObjectId> {
        match self {
            Self::Anonymous { .. } | Self::Mmio { .. } => None,
            Self::Object { object, .. } => Some(object),
        }
    }

    pub const fn is_ram(self) -> bool {
        matches!(self, Self::Anonymous { .. } | Self::Object { .. })
    }

    pub const fn mapping(self) -> Option<MappingId> {
        match self {
            Self::Mmio { mapping, .. } => Some(mapping),
            _ => None,
        }
    }

    /// 同一 backing 的后续区间，供账本与实际 View owner 同步物化切界。
    pub fn shifted(self, bytes: usize) -> Option<Self> {
        let offset = self.offset().checked_add(bytes)?;
        Some(match self {
            Self::Anonymous {
                identity, class, ..
            } => Self::Anonymous {
                identity,
                class,
                offset,
            },
            Self::Object { object, .. } => Self::Object { object, offset },
            Self::Mmio { mapping, .. } => Self::Mmio { mapping, offset },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOwner {
    AddressSpace,
    Lease(LeaseKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPlacement {
    Anywhere,
    /// 指定 usable mapping 的起始地址；前 guard 位于其下方。
    FixedEmpty {
        usable_start: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserWriteLeaseRequest {
    pub range: AddressRange,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MapRequest {
    pub bytes: usize,
    pub guard_before: usize,
    pub guard_after: usize,
    pub placement: MapPlacement,
    pub current: Protection,
    pub maximum: Protection,
    pub owner: RegionOwner,
    pub backing: MappingSource,
    pub result: Option<UserWriteLeaseRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmapRequest {
    pub range: PageRange,
    pub authority: RegionOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtectRequest {
    pub range: PageRange,
    pub protection: Protection,
    pub authority: RegionOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_regions: usize,
    pub max_transactions: usize,
    pub max_pages_per_change: usize,
    pub max_lease_bytes: usize,
    pub max_lease_segments: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeError {
    BadLimits,
    Range(RangeError),
    OutOfBounds,
    Conflict,
    Busy,
    NotCovered,
    Guard,
    OwnerDenied,
    PermissionDenied,
    BackingOutOfRange,
    ObjectAuthorization,
    PageLimit,
    RegionLimit,
    TransactionLimit,
    LeaseInvalid,
    LeaseTooLarge,
    PermitMismatch,
    Stale,
    KeyExhausted,
    CapacityChanged,
}

impl From<RangeError> for ChangeError {
    fn from(value: RangeError) -> Self {
        Self::Range(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapResultLayout {
    pub usable: PageRange,
    pub reservation: PageRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermitRequirement {
    object: ObjectId,
    /// 本次替换需要交给新区域的 permit 数量。
    count: usize,
    /// 其中不是从既有 writable 区域继承而来的数量；Sealing 必须为零。
    new_writes: usize,
}

impl PermitRequirement {
    pub const fn object(self) -> ObjectId {
        self.object
    }

    pub const fn count(self) -> usize {
        self.count
    }

    pub const fn new_writes(self) -> usize {
        self.new_writes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationIntent {
    Install {
        range: PageRange,
        backing: MappingOrigin,
        protection: Protection,
    },
    Remove {
        range: PageRange,
    },
    Protect {
        range: PageRange,
        from: Protection,
        to: Protection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKindView {
    Guard,
    Mapping {
        backing: MappingOrigin,
        current: Protection,
        maximum: Protection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionView {
    pub key: RegionKey,
    pub allocation: AllocationKey,
    pub range: PageRange,
    pub owner: RegionOwner,
    pub kind: RegionKindView,
}

#[derive(Debug, Clone, Copy)]
enum RegionKind {
    Guard,
    Mapping {
        backing: MappingOrigin,
        current: Protection,
        maximum: Protection,
    },
}

#[derive(Debug)]
struct Region {
    key: RegionKey,
    allocation: AllocationKey,
    range: PageRange,
    owner: RegionOwner,
    kind: RegionKind,
    permit: Option<WritePermit>,
}

impl Region {
    fn view(&self) -> RegionView {
        RegionView {
            key: self.key,
            allocation: self.allocation,
            range: self.range,
            owner: self.owner,
            kind: match self.kind {
                RegionKind::Guard => RegionKindView::Guard,
                RegionKind::Mapping {
                    backing,
                    current,
                    maximum,
                } => RegionKindView::Mapping {
                    backing,
                    current,
                    maximum,
                },
            },
        }
    }
}

/// 覆盖检查后借同一不可变账本；key 是 region 起点，不是稠密槽号。
struct Coverage<'a, C> {
    regions: &'a OrdMap<usize, Region, C>,
    first: usize,
    last: usize,
    count: usize,
}

impl<C> Coverage<'_, C> {
    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.regions
            .iter_from(self.first)
            .take(self.count)
            .map(|(key, _)| key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserWriteSegment {
    pub region: RegionKey,
    pub user: AddressRange,
    pub backing: MappingOrigin,
    pub backing_offset: usize,
}

/// 预约 owner 内投影的借用；付款与段数组始终由 PreparedChange 持有。
#[derive(Debug, Clone, Copy)]
pub struct UserWriteLease<'a> {
    range: AddressRange,
    segments: &'a [UserWriteSegment],
}

impl<'a> UserWriteLease<'a> {
    pub const fn range(&self) -> AddressRange {
        self.range
    }
    pub const fn segments(&self) -> &'a [UserWriteSegment] {
        self.segments
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackingRetire {
    Retain,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetiringFragment {
    pub key: RegionKey,
    pub allocation: AllocationKey,
    pub range: PageRange,
    pub owner: RegionOwner,
    pub kind: RegionKindView,
    pub backing_retire: BackingRetire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    Free,
    Guard,
    Mapping {
        protection: Protection,
    },
    /// 地址落在已提交、账本仍在分步推进的事务区间：调用者不得据此投递
    /// fault，应按 `target`（该事务承诺的终态权限，Unmap 为 None）判定：
    /// 终态允许则锁外让出后重试，否则正常投递。
    Publishing {
        change: ChangeKey,
        target: Option<Protection>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeStage {
    /// 可取消的准备 owner。
    Reserved,
    /// 不可撤销的已提交责任（账本与 PTE 发布绑定于同一协议）。
    Committed,
    /// 同步完成证明已发放，退休游标推进中。
    Retiring,
}

#[derive(Debug, Clone, Copy)]
enum AllocationSource {
    New,
    Existing(AllocationKey),
}

#[derive(Debug, Clone, Copy)]
enum TemplateKind {
    Guard,
    Mapping {
        backing: MappingOrigin,
        current: Protection,
        maximum: Protection,
    },
}

#[derive(Debug, Clone, Copy)]
struct RegionTemplate {
    allocation: AllocationSource,
    range: PageRange,
    owner: RegionOwner,
    kind: TemplateKind,
    /// writable replacement 是否继承自本次将被替换的 writable 区域。
    write_successor: bool,
}

impl RegionTemplate {
    fn writable_object(self) -> Option<ObjectId> {
        match self.kind {
            TemplateKind::Mapping {
                backing: MappingOrigin::Object { object, .. },
                current: Protection::ReadWrite,
                ..
            } => Some(object),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RetireTemplate {
    allocation: AllocationKey,
    range: PageRange,
    owner: RegionOwner,
    kind: RegionKindView,
    backing_retire: BackingRetire,
}

/// 阶段事实不拥有容器；实际存储沿 token 移动，门内转换不会析构付款。
#[derive(Debug)]
struct ChangePlan {
    footprint: PageRange,
    result_layout: Option<MapResultLayout>,
    lease: Option<AddressRange>,
    growth: usize,
    retiring_permits: usize,
}

/// 一次验证所需的容量，不构成预约；准备后仍须在同一账本门内复检。
#[derive(Debug, Clone, Copy, Default)]
pub struct ChangeCapacity {
    remove: usize,
    snapshot: usize,
    templates: usize,
    retiring: usize,
    translations: usize,
    segments: usize,
    requirements: usize,
}

/// 请求本身由调用者稳定持有，验证重试只借用，不复制对象授权。
#[derive(Debug)]
pub enum ChangeRequest {
    Map(MapRequest),
    Unmap(UnmapRequest),
    Protect(ProtectRequest),
}

/// 所有计划存储的唯一 owner。只在门外给出 Funding 准备实际容量；
/// 验证、预约、提交和退休在门内只填槽及移动，最终锁外释放整份 owner。
#[derive(Debug)]
struct StorageData<C> {
    remove: funded_storage::Buffer<(usize, RegionKey), C>,
    snapshot: funded_storage::Buffer<(usize, RegionKey), C>,
    templates: funded_storage::Buffer<RegionTemplate, C>,
    retire_templates: funded_storage::Buffer<RetireTemplate, C>,
    translations: funded_storage::Buffer<TranslationIntent, C>,
    segments: funded_storage::Buffer<UserWriteSegment, C>,
    requirements: funded_storage::Buffer<PermitRequirement, C>,
    regions: funded_storage::Buffer<Region, C>,
    retired: funded_storage::Buffer<RetiringFragment, C>,
    retiring_permits: funded_storage::Buffer<WritePermit, C>,
}

impl<C> Default for StorageData<C> {
    fn default() -> Self {
        Self {
            remove: funded_storage::Buffer::new(),
            snapshot: funded_storage::Buffer::new(),
            templates: funded_storage::Buffer::new(),
            retire_templates: funded_storage::Buffer::new(),
            translations: funded_storage::Buffer::new(),
            segments: funded_storage::Buffer::new(),
            requirements: funded_storage::Buffer::new(),
            regions: funded_storage::Buffer::new(),
            retired: funded_storage::Buffer::new(),
            retiring_permits: funded_storage::Buffer::new(),
        }
    }
}

/// 空候选不分配；准备时给单一存储头付款，阶段之间只移动小 owner。
#[must_use = "change storage must settle affine owners and be released outside the ledger gate"]
pub struct ChangeStorage<C> {
    data: Option<funded_storage::OwnedBox<StorageData<C>, C>>,
}
impl<C> Default for ChangeStorage<C> {
    fn default() -> Self {
        Self { data: None }
    }
}
impl<C> core::fmt::Debug for ChangeStorage<C> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ChangeStorage")
            .field("allocated", &self.data.is_some())
            .finish()
    }
}
impl<C> ChangeStorage<C> {
    /// 空间出生时预付一次完整 MMIO 片段 Remove 所需的固定承载。
    /// 不依赖片段页数，不保存新的 Region，也不取得 MemoryObject 写许可。
    pub fn prepare_forced_remove<F: funded_storage::Funding<Charge = C>>(
        &mut self,
        payer: &F,
    ) -> Result<(), funded_storage::StorageError<F::Error>> {
        self.prepare_validation(
            ChangeCapacity {
                remove: 1,
                retiring: 1,
                translations: 1,
                ..ChangeCapacity::default()
            },
            payer,
        )?;
        self.data_mut().retired.try_reserve_exact(1, payer)?;
        Ok(())
    }

    fn data(&self) -> &StorageData<C> {
        self.data
            .as_deref()
            .expect("validated change lost storage header")
    }
    fn data_mut(&mut self) -> &mut StorageData<C> {
        self.data
            .as_deref_mut()
            .expect("validated change lost storage header")
    }
    fn reset_validation(&mut self) {
        let data = self.data_mut();
        assert!(
            data.regions.is_empty() && data.retiring_permits.is_empty(),
            "validation storage contains affine owners"
        );
        data.remove.clear();
        data.snapshot.clear();
        data.templates.clear();
        data.retire_templates.clear();
        data.translations.clear();
        data.segments.clear();
        data.requirements.clear();
        // rollback 带出的普通退休描述尚未提交，复用时不可再次发布。
        data.retired.clear();
    }
    pub fn prepare_validation<F: funded_storage::Funding<Charge = C>>(
        &mut self,
        capacity: ChangeCapacity,
        payer: &F,
    ) -> Result<(), funded_storage::StorageError<F::Error>> {
        if self.data.is_none() {
            self.data = Some(
                funded_storage::OwnedBox::try_new(StorageData::default(), payer).map_err(
                    |(error, value)| {
                        drop(value);
                        error
                    },
                )?,
            );
        }
        self.reset_validation();
        let data = self.data_mut();
        data.remove.try_reserve_exact(capacity.remove, payer)?;
        data.snapshot.try_reserve_exact(capacity.snapshot, payer)?;
        data.templates
            .try_reserve_exact(capacity.templates, payer)?;
        data.retire_templates
            .try_reserve_exact(capacity.retiring, payer)?;
        data.translations
            .try_reserve_exact(capacity.translations, payer)?;
        data.segments.try_reserve_exact(capacity.segments, payer)?;
        data.requirements
            .try_reserve_exact(capacity.requirements, payer)?;
        Ok(())
    }
    fn validation_ready(&self, capacity: ChangeCapacity) -> bool {
        let Some(data) = self.data.as_deref() else {
            return false;
        };
        data.remove.capacity() >= capacity.remove
            && data.snapshot.capacity() >= capacity.snapshot
            && data.templates.capacity() >= capacity.templates
            && data.retire_templates.capacity() >= capacity.retiring
            && data.translations.capacity() >= capacity.translations
            && data.segments.capacity() >= capacity.segments
            && data.requirements.capacity() >= capacity.requirements
    }
    /// 每次只移出一个 Region 及其可选许可；空候选无收束工作。
    pub fn rollback_step(&mut self) -> RollbackStep {
        let Some(data) = self.data.as_deref_mut() else {
            return RollbackStep::Done;
        };
        match data.regions.pop() {
            Some(region) => match region.permit {
                Some(permit) => RollbackStep::Permit(permit),
                None => RollbackStep::Progress,
            },
            None => RollbackStep::Done,
        }
    }
}

#[derive(Debug)]
pub enum RollbackStep {
    Progress,
    Permit(WritePermit),
    Done,
}

#[derive(Debug)]
#[must_use = "validated changes must be reserved or discarded"]
pub struct ValidatedChange<C> {
    plan: ChangePlan,
    storage: ChangeStorage<C>,
}

impl<C> ValidatedChange<C> {
    pub const fn footprint(&self) -> PageRange {
        self.plan.footprint
    }
    pub fn permit_requirements(&self) -> &[PermitRequirement] {
        &self.storage.data().requirements
    }
    pub fn translation_intents(&self) -> &[TranslationIntent] {
        &self.storage.data().translations
    }
    pub const fn map_result(&self) -> Option<MapResultLayout> {
        self.plan.result_layout
    }
    pub fn into_storage(self) -> ChangeStorage<C> {
        self.storage
    }
    /// 物化形状已由验证冻结；部分准入失败仍由本 token 持全部已分配块。
    pub fn prepare_reservation<F: funded_storage::Funding<Charge = C>>(
        &mut self,
        payer: &F,
    ) -> Result<(), funded_storage::StorageError<F::Error>> {
        let storage = self.storage.data_mut();
        storage
            .regions
            .try_reserve_exact(storage.templates.len(), payer)?;
        storage
            .retired
            .try_reserve_exact(storage.retire_templates.len(), payer)?;
        storage
            .retiring_permits
            .try_reserve_exact(self.plan.retiring_permits, payer)?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "prepared changes must be committed or rolled back"]
pub struct PreparedChange<C> {
    key: ChangeKey,
    plan: ChangePlan,
    storage: ChangeStorage<C>,
}

impl<C> PreparedChange<C> {
    pub const fn key(&self) -> ChangeKey {
        self.key
    }
    pub fn user_write_lease(&self) -> Option<UserWriteLease<'_>> {
        self.plan.lease.map(|range| UserWriteLease {
            range,
            segments: &self.storage.data().segments,
        })
    }
    pub const fn map_result(&self) -> Option<MapResultLayout> {
        self.plan.result_layout
    }
    pub fn mapped_region_key(&self) -> Option<RegionKey> {
        let usable = self.plan.result_layout?.usable;
        self.storage.data().regions.iter().find_map(|region| {
            (region.range == usable && matches!(region.kind, RegionKind::Mapping { .. }))
                .then_some(region.key)
        })
    }
    pub fn translation_intents(&self) -> &[TranslationIntent] {
        &self.storage.data().translations
    }
    pub fn retiring_fragments(&self) -> &[RetiringFragment] {
        &self.storage.data().retired
    }
}

#[derive(Debug)]
pub struct ReserveFailure<C> {
    pub error: ChangeError,
    validated: ValidatedChange<C>,
}

impl<C> ReserveFailure<C> {
    pub fn into_parts(self) -> (ChangeError, ValidatedChange<C>) {
        (self.error, self.validated)
    }
}

#[derive(Debug)]
struct ChangePayload<C> {
    key: ChangeKey,
    plan: ChangePlan,
    storage: ChangeStorage<C>,
}

macro_rules! change_token {
    ($name:ident) => {
        #[derive(Debug)]
        #[must_use = "memory changes must advance to Complete"]
        pub struct $name<C>(ChangePayload<C>);

        impl<C> $name<C> {
            pub const fn key(&self) -> ChangeKey {
                self.0.key
            }
            pub fn translation_intents(&self) -> &[TranslationIntent] {
                &self.0.storage.data().translations
            }
            pub fn retiring_fragments(&self) -> &[RetiringFragment] {
                &self.0.storage.data().retired
            }
            pub const fn map_result(&self) -> Option<MapResultLayout> {
                self.0.plan.result_layout
            }
        }
    };
}

change_token!(CommittedChange);
change_token!(RetiringChange);

impl<C> RetiringChange<C> {
    pub fn fragments(&self) -> &[RetiringFragment] {
        &self.0.storage.data().retired
    }
    pub fn permits(&self) -> &[WritePermit] {
        &self.0.storage.data().retiring_permits
    }
    pub fn pop_fragment(&mut self) -> Option<RetiringFragment> {
        self.0.storage.data_mut().retired.pop()
    }
    pub fn pop_permit(&mut self) -> Option<WritePermit> {
        self.0.storage.data_mut().retiring_permits.pop()
    }
    pub fn is_empty(&self) -> bool {
        self.0.storage.data().retired.is_empty()
            && self.0.storage.data().retiring_permits.is_empty()
    }
}

#[derive(Debug)]
struct TransactionRecord {
    key: ChangeKey,
    footprint: PageRange,
    lease: Option<AddressRange>,
    stage: ChangeStage,
    reserved_growth: usize,
    /// Committed 且 ledger 游标未耗尽。
    ledger_pending: bool,
    /// 事务承诺的终态权限（fault 在过渡期按此判定）。
    target: Option<(PageRange, Protection)>,
}

#[derive(Debug)]
pub struct MemorySpace<C> {
    bounds: PageRange,
    limits: Limits,
    regions: OrdMap<usize, Region, C>,
    transactions: funded_storage::Buffer<TransactionRecord, C>,
    next_allocation: u64,
    next_region: u64,
    next_change: u64,
    reserved_growth: usize,
    /// 每空间一个强制退休 owner，冻结其来源的新修改；既有预约继续原责任。
    frozen_mapping: Option<MappingId>,
}

impl<C> MemorySpace<C> {
    pub fn storage_ready(
        &self,
        nodes: usize,
        storage: &RegionStorage<C>,
    ) -> Result<bool, ChangeError> {
        self.regions
            .storage_ready(nodes, &storage.index)
            .map_err(|_| ChangeError::RegionLimit)
    }
    pub fn storage_capacity(&self, nodes: usize) -> Result<usize, ChangeError> {
        self.regions
            .storage_capacity(nodes)
            .map_err(|_| ChangeError::RegionLimit)
    }
    pub fn growth_capacity(&self, nodes: usize) -> Result<usize, ChangeError> {
        self.regions
            .growth_capacity(nodes)
            .map_err(|_| ChangeError::RegionLimit)
    }
    pub fn change_growth_capacity(
        &self,
        change: &ValidatedChange<C>,
    ) -> Result<usize, ChangeError> {
        self.growth_capacity(change.storage.data().templates.len())
    }
    pub fn change_storage_capacity(
        &self,
        change: &ValidatedChange<C>,
    ) -> Result<usize, ChangeError> {
        self.storage_capacity(change.storage.data().templates.len())
    }
    /// 目标架构上的 ledger 条目布局；容量预算还需计入数组与 allocator 开销。
    pub const fn entry_sizes() -> (usize, usize) {
        (
            core::mem::size_of::<Region>(),
            core::mem::size_of::<TransactionRecord>(),
        )
    }

    pub fn new<F: funded_storage::Funding<Charge = C>>(
        bounds: PageRange,
        limits: Limits,
        payer: &F,
    ) -> Result<Self, SpaceCreateError<F::Error>> {
        if limits.max_regions == 0
            || limits.max_transactions == 0
            || limits.max_pages_per_change == 0
            || limits.max_lease_bytes == 0
            || limits.max_lease_segments == 0
            || limits.max_transactions.checked_add(1).is_none()
        {
            return Err(SpaceCreateError::BadLimits);
        }
        let mut regions = OrdMap::new();
        let mut storage = OrdMapStorage::try_empty(limits.max_regions, payer)
            .map_err(SpaceCreateError::Storage)?;
        regions
            .install_storage(&mut storage)
            .expect("fresh arena fits empty index");
        // 额外一槽只由强制退休入口使用，业务预约不能消耗。
        let transactions = funded_storage::Buffer::try_empty(limits.max_transactions + 1, payer)
            .map_err(SpaceCreateError::Storage)?;
        Ok(Self {
            bounds,
            limits,
            regions,
            transactions,
            next_allocation: 1,
            next_region: 1,
            next_change: 1,
            reserved_growth: 0,
            frozen_mapping: None,
        })
    }

    pub const fn bounds(&self) -> PageRange {
        self.bounds
    }
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// 最后离开者判定：是否仍有在途事务（任一阶段）的 footprint 与 `range`
    /// 相交。调用者在同一空间锁内先 `complete`/`rollback` 自己的记录再查询，
    /// 检查与摘表之间不放锁。
    pub fn span_reserved(&self, range: AddressRange) -> bool {
        self.transactions
            .iter()
            .any(|record| record.footprint.address_range().overlaps(range))
    }
    /// 退休事务摘空表时忽略自己，仍保护所有其他预约/结果 lease。
    /// 调用者保留 RetiringChange 到最后边界扫表和 complete 的同一临界区。
    pub fn span_reserved_by_other(&self, owner: ChangeKey, range: AddressRange) -> bool {
        assert!(
            self.transactions
                .iter()
                .any(|transaction| transaction.key == owner),
            "table cleanup lost reservation"
        );
        self.transactions.iter().any(|transaction| {
            transaction.key != owner
                && (transaction.footprint.address_range().overlaps(range)
                    || transaction.lease.is_some_and(|lease| lease.overlaps(range)))
        })
    }
    pub fn prepared_footprint(&self, prepared: &PreparedChange<C>) -> PageRange {
        self.transactions[self.transaction_index(prepared.key, ChangeStage::Reserved)].footprint
    }

    pub fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    /// 强制退休等待已有预约的真实完成来源，包含尚未提交与退休中的责任。
    pub fn change_at(&self, address: usize) -> Option<ChangeKey> {
        self.transactions
            .iter()
            .find(|record| {
                record.footprint.address_range().contains_address(address)
                    || record
                        .lease
                        .is_some_and(|lease| lease.contains_address(address))
            })
            .map(|record| record.key)
    }

    /// 运行适配器的 dependency 存储与同一实际预约容量一致。
    pub fn transaction_capacity(&self) -> usize {
        self.limits.max_transactions + 1
    }

    pub fn freeze_mapping(&mut self, mapping: MappingId) -> Result<(), ChangeError> {
        if self
            .frozen_mapping
            .is_some_and(|current| current != mapping)
        {
            return Err(ChangeError::Busy);
        }
        self.frozen_mapping = Some(mapping);
        Ok(())
    }

    /// 调用者已封闭原来源的发布、结束全部准备/回滚并收到全部片段退休回执；
    /// 清空门供 FIFO 下一来源使用。
    /// 本接口不签发新的 Map 身份或重新开放原资源。
    pub fn finish_mapping_retirement(&mut self, mapping: MappingId) -> Result<(), ChangeError> {
        if self.frozen_mapping != Some(mapping) {
            return Err(ChangeError::Stale);
        }
        if self
            .transactions
            .iter()
            .any(|record| record.key == ChangeKey(0))
        {
            return Err(ChangeError::Busy);
        }
        self.frozen_mapping = None;
        Ok(())
    }

    /// 使用出生预付槽预约包含 `address` 的完整 MMIO 几何片段。
    /// 实物切界可以比几何更细，不要求来访 View 的起点就是几何片段起点。
    /// MappingId 必须仍匹配，所以旧空洞上后来建立的映射不会被撤掉。
    pub fn reserve_forced_remove(
        &mut self,
        mapping: MappingId,
        address: usize,
        storage: &mut ChangeStorage<C>,
    ) -> Result<PreparedChange<C>, ChangeError> {
        if self.frozen_mapping != Some(mapping) {
            return Err(ChangeError::OwnerDenied);
        }
        if self
            .transactions
            .iter()
            .any(|record| record.key == ChangeKey(0))
        {
            return Err(ChangeError::Busy);
        }
        let (start, region) = self
            .regions
            .floor(&address)
            .filter(|(_, region)| region.range.address_range().contains_address(address))
            .ok_or(ChangeError::Stale)?;
        if !matches!(region.kind, RegionKind::Mapping {
            backing: MappingOrigin::Mmio { mapping: current, .. }, ..
        } if current == mapping)
        {
            return Err(ChangeError::Stale);
        }
        self.ensure_no_transaction_conflict(region.range, None)?;
        let capacity = ChangeCapacity {
            remove: 1,
            retiring: 1,
            translations: 1,
            ..ChangeCapacity::default()
        };
        if !storage.validation_ready(capacity) || storage.data().retired.capacity() < 1 {
            return Err(ChangeError::CapacityChanged);
        }
        let view = region.view();
        storage.reset_validation();
        let data = storage.data_mut();
        data.remove.push((start, view.key));
        data.translations
            .push(TranslationIntent::Remove { range: view.range });
        data.retired.push(RetiringFragment {
            key: view.key,
            allocation: view.allocation,
            range: view.range,
            owner: view.owner,
            kind: view.kind,
            backing_retire: BackingRetire::Release,
        });
        // 0 专属于唯一强制槽，普通 key 从 1 单调发行。槽在完整退款/complete
        // 前不复用，旧 fault 等待持独立完成来源，不依赖再次查找此数值。
        let key = ChangeKey(0);
        self.transactions.push(TransactionRecord {
            key,
            footprint: view.range,
            lease: None,
            stage: ChangeStage::Reserved,
            reserved_growth: 0,
            ledger_pending: false,
            target: None,
        });
        Ok(PreparedChange {
            key,
            plan: ChangePlan {
                footprint: view.range,
                result_layout: None,
                lease: None,
                growth: 0,
                retiring_permits: 0,
            },
            storage: core::mem::take(storage),
        })
    }

    pub fn regions(&self) -> impl Iterator<Item = RegionView> + '_ {
        self.regions.iter().map(|(_, region)| Region::view(region))
    }

    /// 当前账本中的来源。调用者仍须核验真实 PTE，不能仅凭此结果访问物理内存。
    pub fn origin_at(&self, address: usize) -> Option<MappingOrigin> {
        let (_, region) = self.regions.floor(&address)?;
        if !region.range.address_range().contains_address(address) {
            return None;
        }
        match region.kind {
            RegionKind::Mapping { backing, .. } => backing.shifted(address - region.range.start()),
            RegionKind::Guard => None,
        }
    }

    pub fn fault_class(&self, address: usize) -> FaultClass {
        if let Some(transaction) = self.transactions.iter().find(|t| {
            t.stage == ChangeStage::Committed
                && t.footprint.address_range().contains_address(address)
        }) {
            let target = transaction.target.and_then(|(range, protection)| {
                range
                    .address_range()
                    .contains_address(address)
                    .then_some(protection)
            });
            return FaultClass::Publishing {
                change: transaction.key,
                target,
            };
        }
        let Some(region) = self
            .regions
            .floor(&address)
            .map(|(_, region)| region)
            .filter(|region| region.range.address_range().contains_address(address))
        else {
            return FaultClass::Free;
        };
        match region.kind {
            RegionKind::Guard => FaultClass::Guard,
            RegionKind::Mapping { current, .. } => FaultClass::Mapping {
                protection: current,
            },
        }
    }

    /// 准备所需几何容量；只读采样，不 mint 身份或预约。锁外准备后须再验证。
    pub fn validation_capacity(
        &self,
        request: &ChangeRequest,
    ) -> Result<ChangeCapacity, ChangeError> {
        let mut capacity = ChangeCapacity::default();
        match request {
            ChangeRequest::Map(request) => {
                capacity.templates = 1
                    + usize::from(request.guard_before != 0)
                    + usize::from(request.guard_after != 0);
                capacity.translations = 1;
                capacity.requirements = usize::from(
                    request.current.writable()
                        && matches!(&request.backing, MappingSource::Object { .. }),
                );
                if let Some(lease) = request.result {
                    if lease.range.bytes() > self.limits.max_lease_bytes {
                        return Err(ChangeError::LeaseTooLarge);
                    }
                    let first = self
                        .regions
                        .floor(&lease.range.start())
                        .map(|(key, _)| key)
                        .unwrap_or(lease.range.start());
                    capacity.segments = self
                        .regions
                        .iter_from(first)
                        .take_while(|(key, _)| *key < lease.range.end())
                        .filter(|(_, region)| region.range.address_range().overlaps(lease.range))
                        .count();
                    if capacity.segments > self.limits.max_lease_segments {
                        return Err(ChangeError::LeaseTooLarge);
                    }
                }
            }
            ChangeRequest::Unmap(request) => {
                self.validate_change_range(request.range)?;
                let covered = self.covered_indices(request.range, request.authority, false)?;
                capacity.remove = covered.count;
                capacity.retiring = covered.count;
                capacity.templates =
                    usize::from(
                        self.region_at(covered.first).range.start() < request.range.start(),
                    ) + usize::from(self.region_at(covered.last).range.end() > request.range.end());
                let first = self.region_at(covered.first);
                let last = self.region_at(covered.last);
                capacity.requirements = usize::from(
                    first.range.start() < request.range.start()
                        && matches!(first.kind, RegionKind::Mapping { backing: MappingOrigin::Object { .. }, current, .. } if current.writable()),
                ) + usize::from(
                    last.range.end() > request.range.end()
                        && matches!(last.kind, RegionKind::Mapping { backing: MappingOrigin::Object { .. }, current, .. } if current.writable()),
                );
                capacity.translations = usize::from(
                    covered
                        .iter()
                        .any(|key| matches!(self.region_at(key).kind, RegionKind::Mapping { .. })),
                );
            }
            ChangeRequest::Protect(request) => {
                self.validate_change_range(request.range)?;
                let covered = self.covered_indices(request.range, request.authority, true)?;
                let changed = covered
                    .iter()
                    .filter(|&key| {
                        matches!(self.region_at(key).kind,
                    RegionKind::Mapping { current, .. } if current != request.protection)
                    })
                    .count();
                if changed == 0 {
                    capacity.snapshot = covered.count;
                } else {
                    let outside = usize::from(
                        self.region_at(covered.first).range.start() < request.range.start(),
                    ) + usize::from(
                        self.region_at(covered.last).range.end() > request.range.end(),
                    );
                    capacity.remove = covered
                        .count
                        .checked_add(2)
                        .ok_or(ChangeError::RegionLimit)?;
                    capacity.templates = covered
                        .count
                        .checked_add(outside)
                        .and_then(|count| count.checked_add(2))
                        .ok_or(ChangeError::RegionLimit)?;
                    capacity.retiring = covered.count;
                    let first = self.region_at(covered.first);
                    let last = self.region_at(covered.last);
                    capacity.requirements = if request.protection.writable() {
                        covered
                            .iter()
                            .filter(|&key| {
                                matches!(
                                    self.region_at(key).kind,
                                    RegionKind::Mapping {
                                        backing: MappingOrigin::Object { .. },
                                        ..
                                    }
                                )
                            })
                            .count()
                    } else {
                        0
                    };
                    capacity.requirements += usize::from(
                        first.range.start() < request.range.start()
                            && matches!(first.kind, RegionKind::Mapping { backing: MappingOrigin::Object { .. }, current, .. } if current.writable()),
                    ) + usize::from(
                        last.range.end() > request.range.end()
                            && matches!(last.kind, RegionKind::Mapping { backing: MappingOrigin::Object { .. }, current, .. } if current.writable()),
                    );
                    capacity.translations = changed;
                }
            }
        }
        Ok(capacity)
    }

    /// 失败不取走调用者的 paid storage；足容量证明与最终读取在同一借用下成立。
    pub fn validate(
        &self,
        request: &ChangeRequest,
        storage: &mut ChangeStorage<C>,
    ) -> Result<ValidatedChange<C>, ChangeError> {
        let capacity = self.validation_capacity(request)?;
        if !storage.validation_ready(capacity) {
            return Err(ChangeError::CapacityChanged);
        }
        storage.reset_validation();
        let plan = match request {
            ChangeRequest::Map(request) => self.validate_map(request, storage)?,
            ChangeRequest::Unmap(request) => self.validate_unmap(request, storage)?,
            ChangeRequest::Protect(request) => self.validate_protect(request, storage)?,
        };
        Ok(ValidatedChange {
            plan,
            storage: core::mem::take(storage),
        })
    }

    fn validate_map(
        &self,
        request: &MapRequest,
        storage: &mut ChangeStorage<C>,
    ) -> Result<ChangePlan, ChangeError> {
        let storage = storage.data_mut();
        if !request.maximum.permits(request.current) {
            return Err(ChangeError::PermissionDenied);
        }
        let usable_bytes = round_page_bytes(request.bytes)?;
        let guard_before = round_guard_bytes(request.guard_before)?;
        let guard_after = round_guard_bytes(request.guard_after)?;
        let reservation_bytes = guard_before
            .checked_add(usable_bytes)
            .and_then(|bytes| bytes.checked_add(guard_after))
            .ok_or(ChangeError::Range(RangeError::Overflow))?;
        if reservation_bytes / PAGE_SIZE > self.limits.max_pages_per_change {
            return Err(ChangeError::PageLimit);
        }

        let reservation = match request.placement {
            MapPlacement::Anywhere => self.find_hole(reservation_bytes)?,
            MapPlacement::FixedEmpty { usable_start } => {
                if !usable_start.is_multiple_of(PAGE_SIZE) {
                    return Err(ChangeError::Range(RangeError::Unaligned));
                }
                let start = usable_start
                    .checked_sub(guard_before)
                    .ok_or(ChangeError::OutOfBounds)?;
                PageRange::new(start, reservation_bytes)?
            }
        };
        if !self.bounds.contains(reservation) {
            return Err(ChangeError::OutOfBounds);
        }
        if self
            .transactions
            .iter()
            .any(|record| record.footprint.overlaps(reservation))
        {
            return Err(ChangeError::Busy);
        }
        if self.regions_overlap(reservation) {
            return Err(ChangeError::Conflict);
        }
        let usable = PageRange::new(reservation.start() + guard_before, usable_bytes)?;
        let layout = MapResultLayout {
            usable,
            reservation,
        };

        let backing = match &request.backing {
            MappingSource::Anonymous { identity, class } => {
                let allowed = match class {
                    AnonymousClass::Data => request.maximum != Protection::ReadExecute,
                    AnonymousClass::InitialExecutable => request.maximum != Protection::ReadWrite,
                };
                if !allowed {
                    return Err(ChangeError::PermissionDenied);
                }
                MappingOrigin::Anonymous {
                    identity: *identity,
                    class: *class,
                    offset: 0,
                }
            }
            MappingSource::Object {
                authorization,
                offset,
            } => {
                if authorization.maximum() != request.maximum {
                    return Err(ChangeError::ObjectAuthorization);
                }
                let end = offset
                    .checked_add(usable_bytes)
                    .ok_or(ChangeError::BackingOutOfRange)?;
                if end > authorization.object_bytes() {
                    return Err(ChangeError::BackingOutOfRange);
                }
                MappingOrigin::Object {
                    object: authorization.object(),
                    offset: *offset,
                }
            }
            MappingSource::Mmio { mapping, bytes } => {
                if self.frozen_mapping == Some(*mapping) {
                    return Err(ChangeError::Busy);
                }
                if request.maximum == Protection::ReadExecute {
                    return Err(ChangeError::PermissionDenied);
                }
                if !bytes.is_multiple_of(PAGE_SIZE) || usable_bytes > *bytes {
                    return Err(ChangeError::BackingOutOfRange);
                }
                MappingOrigin::Mmio {
                    mapping: *mapping,
                    offset: 0,
                }
            }
        };

        let replacements = &mut storage.templates;
        if guard_before > 0 {
            replacements.push(RegionTemplate {
                allocation: AllocationSource::New,
                range: PageRange::new(reservation.start(), guard_before)?,
                owner: request.owner,
                kind: TemplateKind::Guard,
                write_successor: false,
            });
        }
        replacements.push(RegionTemplate {
            allocation: AllocationSource::New,
            range: usable,
            owner: request.owner,
            kind: TemplateKind::Mapping {
                backing,
                current: request.current,
                maximum: request.maximum,
            },
            write_successor: false,
        });
        if guard_after > 0 {
            replacements.push(RegionTemplate {
                allocation: AllocationSource::New,
                range: PageRange::new(usable.end(), guard_after)?,
                owner: request.owner,
                kind: TemplateKind::Guard,
                write_successor: false,
            });
        }

        let translations = &mut storage.translations;
        translations.push(TranslationIntent::Install {
            range: usable,
            backing,
            protection: request.current,
        });

        let lease = match request.result {
            Some(lease) => {
                self.validate_user_write_lease(lease.range, reservation, &mut storage.segments)?;
                Some(lease.range)
            }
            None => None,
        };
        permit_requirements(replacements, &mut storage.requirements)?;
        let growth = replacements.len();
        self.check_projected_regions(growth)?;
        Ok(ChangePlan {
            footprint: reservation,
            result_layout: Some(layout),
            lease,
            growth,
            retiring_permits: 0,
        })
    }

    fn validate_unmap(
        &self,
        request: &UnmapRequest,
        storage: &mut ChangeStorage<C>,
    ) -> Result<ChangePlan, ChangeError> {
        let storage = storage.data_mut();
        self.validate_change_range(request.range)?;
        self.ensure_no_transaction_conflict(request.range, None)?;
        let indices = self.covered_indices(request.range, request.authority, false)?;

        let remove = &mut storage.remove;
        let replacements = &mut storage.templates;
        let retiring = &mut storage.retire_templates;
        let translations = &mut storage.translations;
        let mut removes_translation = false;

        for index in indices.iter() {
            let region = self.region_at(index);
            let cut = region
                .range
                .intersection(request.range)
                .expect("covered region intersects request");
            remove.push((region.range.start(), region.key));
            if region.range.start() < cut.start() {
                replacements.push(template_from_region(
                    region,
                    PageRange::from_bounds(region.range.start(), cut.start())?,
                    None,
                )?);
            }
            if cut.end() < region.range.end() {
                replacements.push(template_from_region(
                    region,
                    PageRange::from_bounds(cut.end(), region.range.end())?,
                    None,
                )?);
            }
            retiring.push(retire_template(region, cut, BackingRetire::Release)?);
            if matches!(region.kind, RegionKind::Mapping { .. }) {
                removes_translation = true;
            }
        }
        if removes_translation {
            // Unmap authority 已保证 region 连续覆盖；Guard 本就没有 PTE，因此
            // 请求级 Remove 与逐 mapping fragment 等价，并让页表精确计算整体退役。
            translations.push(TranslationIntent::Remove {
                range: request.range,
            });
        }

        replacements.sort_unstable_by_key(|region| region.range.start());
        permit_requirements(replacements, &mut storage.requirements)?;
        let growth = replacements.len().saturating_sub(remove.len());
        self.check_projected_regions(growth)?;
        // footprint = 实际被替换节点的跨度：两个写者不会在提交后争同一节点。
        // 端点已物化（见 split_at_boundaries）时恰好等于请求区间。
        let footprint = PageRange::from_bounds(
            remove
                .iter()
                .map(|&(start, _)| start)
                .min()
                .unwrap_or(request.range.start()),
            remove
                .iter()
                .map(|&(start, _)| self.region_at(start).range.end())
                .max()
                .unwrap_or(request.range.end()),
        )?;
        self.ensure_no_transaction_conflict(footprint, None)?;
        let retiring_permits = remove
            .iter()
            .filter(|(start, _)| self.region_at(*start).permit.is_some())
            .count();
        Ok(ChangePlan {
            footprint,
            result_layout: None,
            lease: None,
            growth,
            retiring_permits,
        })
    }

    fn validate_protect(
        &self,
        request: &ProtectRequest,
        storage: &mut ChangeStorage<C>,
    ) -> Result<ChangePlan, ChangeError> {
        let storage = storage.data_mut();
        self.validate_change_range(request.range)?;
        self.ensure_no_transaction_conflict(request.range, None)?;
        let indices = self.covered_indices(request.range, request.authority, true)?;
        for index in indices.iter() {
            let RegionKind::Mapping { maximum, .. } = self.region_at(index).kind else {
                return Err(ChangeError::Guard);
            };
            if !maximum.permits(request.protection) {
                return Err(ChangeError::PermissionDenied);
            }
        }

        let all_same = indices.iter().all(|index| {
            matches!(
                self.region_at(index).kind,
                RegionKind::Mapping { current, .. } if current == request.protection
            )
        });
        if all_same {
            for index in indices.iter() {
                storage.snapshot.push((index, self.region_at(index).key));
            }
            return Ok(ChangePlan {
                footprint: request.range,
                result_layout: None,
                lease: None,
                growth: 0,
                retiring_permits: 0,
            });
        }

        let remove = &mut storage.remove;
        let replacements = &mut storage.templates;
        let retiring = &mut storage.retire_templates;
        let translations = &mut storage.translations;
        let first_index = indices.first;
        let last_index = indices.last;
        for index in indices.iter() {
            let region = self.region_at(index);
            let cut = region
                .range
                .intersection(request.range)
                .expect("covered region intersects request");
            remove.push((region.range.start(), region.key));
            if region.range.start() < cut.start() {
                replacements.push(template_from_region(
                    region,
                    PageRange::from_bounds(region.range.start(), cut.start())?,
                    None,
                )?);
            }
            replacements.push(template_from_region(region, cut, Some(request.protection))?);
            if cut.end() < region.range.end() {
                replacements.push(template_from_region(
                    region,
                    PageRange::from_bounds(cut.end(), region.range.end())?,
                    None,
                )?);
            }
            retiring.push(retire_template(region, cut, BackingRetire::Retain)?);
            let RegionKind::Mapping { current, .. } = region.kind else {
                unreachable!()
            };
            if current != request.protection {
                translations.push(TranslationIntent::Protect {
                    range: cut,
                    from: current,
                    to: request.protection,
                });
            }
        }

        if let Some((_, neighbor)) = first_index
            .checked_sub(1)
            .and_then(|probe| self.regions.floor(&probe))
            && neighbor.permit.is_none()
            && templates_compatible_region(
                neighbor,
                replacements
                    .iter()
                    .min_by_key(|region| region.range.start())
                    .expect("protect produces replacements"),
            )
        {
            remove.push((neighbor.range.start(), neighbor.key));
            replacements.push(template_from_region(neighbor, neighbor.range, None)?);
        }
        if let Some((_, neighbor)) = self.regions.after(&last_index)
            && neighbor.permit.is_none()
            && templates_compatible_region(
                neighbor,
                replacements
                    .iter()
                    .max_by_key(|region| region.range.end())
                    .expect("protect produces replacements"),
            )
        {
            remove.push((neighbor.range.start(), neighbor.key));
            replacements.push(template_from_region(neighbor, neighbor.range, None)?);
        }

        normalize_templates(replacements)?;
        let footprint = PageRange::from_bounds(
            replacements
                .first()
                .map_or(request.range.start(), |region| region.range.start()),
            replacements
                .last()
                .map_or(request.range.end(), |region| region.range.end()),
        )?;
        self.ensure_no_transaction_conflict(footprint, None)?;
        permit_requirements(replacements, &mut storage.requirements)?;
        let growth = replacements.len().saturating_sub(remove.len());
        self.check_projected_regions(growth)?;
        let retiring_permits = remove
            .iter()
            .filter(|(start, _)| self.region_at(*start).permit.is_some())
            .count();
        Ok(ChangePlan {
            footprint,
            result_layout: None,
            lease: None,
            growth,
            retiring_permits,
        })
    }

    /// Reserve 失败原样归还 validated plan、affine permits 与付款 owner；
    /// 门内不分配或析构 paid 块。
    #[allow(clippy::result_large_err)]
    pub fn reserve(
        &mut self,
        mut validated: ValidatedChange<C>,
        permits: &mut funded_storage::Buffer<WritePermit, C>,
        storage: &mut RegionStorage<C>,
    ) -> Result<PreparedChange<C>, ReserveFailure<C>> {
        if let Err(error) = self.check_reservation(&validated, permits, storage) {
            return Err(ReserveFailure { error, validated });
        }
        self.materialize_reservation(&mut validated, permits);
        let target =
            validated
                .storage
                .data()
                .translations
                .iter()
                .find_map(|intent| match *intent {
                    TranslationIntent::Install {
                        range, protection, ..
                    } => Some((range, protection)),
                    TranslationIntent::Protect { to, .. } => Some((validated.plan.footprint, to)),
                    TranslationIntent::Remove { .. } => None,
                });
        let key = self.mint_change_key();
        self.transactions.push(TransactionRecord {
            key,
            footprint: validated.plan.footprint,
            lease: validated.plan.lease,
            stage: ChangeStage::Reserved,
            reserved_growth: validated.plan.growth,
            ledger_pending: false,
            target,
        });
        self.reserved_growth += validated.plan.growth;
        Ok(PreparedChange {
            key,
            plan: validated.plan,
            storage: validated.storage,
        })
    }

    fn check_reservation(
        &mut self,
        validated: &ValidatedChange<C>,
        permits: &[WritePermit],
        storage: &mut RegionStorage<C>,
    ) -> Result<(), ChangeError> {
        let ordinary_count = self
            .transactions
            .iter()
            .filter(|record| record.key != ChangeKey(0))
            .count();
        if ordinary_count >= self.limits.max_transactions {
            return Err(ChangeError::TransactionLimit);
        }
        if let Some(mapping) = self.frozen_mapping {
            let data = validated.storage.data();
            let touches_frozen = data.templates.iter().any(|template| {
                matches!(template.kind, TemplateKind::Mapping { backing, .. }
                    if backing.mapping() == Some(mapping))
            }) || data.remove.iter().chain(data.snapshot.iter()).any(
                |(start, _)| {
                    self.regions.get(start).is_some_and(|region| {
                        matches!(region.kind, RegionKind::Mapping { backing, .. }
                        if backing.mapping() == Some(mapping))
                    })
                },
            );
            if touches_frozen {
                return Err(ChangeError::Busy);
            }
        }
        if !self.snapshot_valid(&validated.storage.data().remove)
            || !self.snapshot_valid(&validated.storage.data().snapshot)
            || !self.plan_still_valid(validated)
        {
            return Err(ChangeError::Stale);
        }
        self.ensure_no_transaction_conflict(validated.plan.footprint, validated.plan.lease)?;
        if !permits_match(&validated.storage.data().requirements, permits) {
            return Err(ChangeError::PermitMismatch);
        }
        self.check_projected_regions(validated.plan.growth)?;
        let required_region_keys = validated
            .storage
            .data()
            .templates
            .len()
            .checked_add(validated.storage.data().retire_templates.len())
            .and_then(|count| u64::try_from(count).ok())
            .ok_or(ChangeError::KeyExhausted)?;
        if self.next_region.checked_add(required_region_keys).is_none()
            || self.next_change.checked_add(1).is_none()
            || (validated.plan.result_layout.is_some()
                && self.next_allocation.checked_add(1).is_none())
        {
            return Err(ChangeError::KeyExhausted);
        }
        self.reserved_growth
            .checked_add(validated.plan.growth)
            .ok_or(ChangeError::RegionLimit)?;
        if validated.storage.data().regions.capacity() < validated.storage.data().templates.len()
            || validated.storage.data().retired.capacity()
                < validated.storage.data().retire_templates.len()
            || validated.storage.data().retiring_permits.capacity()
                < validated.plan.retiring_permits
        {
            return Err(ChangeError::CapacityChanged);
        }
        // 最后一个可失败步骤：所有物化槽已付费，成功后只 mint 身份/填槽。
        self.regions
            .reserve(validated.storage.data().templates.len(), &mut storage.index)
            .map_err(|error| match error {
                crate::ReserveFailed::Overflow => ChangeError::RegionLimit,
                crate::ReserveFailed::CapacityChanged => ChangeError::CapacityChanged,
            })?;
        Ok(())
    }

    fn materialize_reservation(
        &mut self,
        validated: &mut ValidatedChange<C>,
        permits: &mut funded_storage::Buffer<WritePermit, C>,
    ) {
        let storage = validated.storage.data_mut();
        let allocation = validated
            .plan
            .result_layout
            .map(|_| self.mint_allocation_key());
        for template in storage.templates.iter() {
            let permit = template
                .writable_object()
                .map(|object| take_permit(permits, object))
                .transpose()
                .expect("permit multiset was validated");
            storage.regions.push(Region {
                key: self.mint_region_key(),
                allocation: match template.allocation {
                    AllocationSource::New => allocation.expect("map has allocation key"),
                    AllocationSource::Existing(key) => key,
                },
                range: template.range,
                owner: template.owner,
                kind: match template.kind {
                    TemplateKind::Guard => RegionKind::Guard,
                    TemplateKind::Mapping {
                        backing,
                        current,
                        maximum,
                    } => RegionKind::Mapping {
                        backing,
                        current,
                        maximum,
                    },
                },
                permit,
            });
        }
        assert!(
            permits.is_empty(),
            "checked permits left after materialization"
        );
        for template in storage.retire_templates.iter() {
            storage.retired.push(RetiringFragment {
                key: self.mint_region_key(),
                allocation: template.allocation,
                range: template.range,
                owner: template.owner,
                kind: template.kind,
                backing_retire: template.backing_retire,
            });
        }
    }

    /// 准备期端点物化：把跨越 `range` 端点的 Region 按同权限切开（语义中性），
    /// 使随后事务的 footprint 恰好等于请求区间，不同 VA 的写者不再共享节点。
    /// 被其他事务 footprint 覆盖的节点不切（返回 Busy）；带写许可的节点不切
    /// （切分需要额外许可），其事务 footprint 扩展为整节点。切分形状在失败时
    /// 保留，不合并。
    pub fn split_at_boundaries(
        &mut self,
        range: PageRange,
        storage: &mut RegionStorage<C>,
    ) -> Result<(), ChangeError> {
        for edge in [range.start(), range.end()] {
            let Some((start, region)) = self.regions.floor(&edge) else {
                continue;
            };
            if !(start < edge && edge < region.range.end()) || region.permit.is_some() {
                continue;
            }
            if matches!(region.kind, RegionKind::Mapping { backing, .. }
                if backing.mapping().is_some() && backing.mapping() == self.frozen_mapping)
            {
                return Err(ChangeError::Busy);
            }
            if self
                .transactions
                .iter()
                .any(|t| t.footprint.overlaps(region.range))
            {
                return Err(ChangeError::Busy);
            }
            self.check_projected_regions(1)?;
            self.regions
                .reserve(2, &mut storage.index)
                .map_err(|error| match error {
                    crate::ReserveFailed::Overflow => ChangeError::RegionLimit,
                    crate::ReserveFailed::CapacityChanged => ChangeError::CapacityChanged,
                })?;
            let old = self.regions.remove(&start).expect("floor region exists");
            let old_kind = old.kind;
            let left = PageRange::from_bounds(old.range.start(), edge)?;
            let right = PageRange::from_bounds(edge, old.range.end())?;
            let shift = |kind: RegionKind, bytes: usize| match kind {
                RegionKind::Guard => RegionKind::Guard,
                RegionKind::Mapping {
                    backing,
                    current,
                    maximum,
                } => RegionKind::Mapping {
                    backing: backing.shifted(bytes).expect("split inside backing range"),
                    current,
                    maximum,
                },
            };
            let (lk, rk) = (self.mint_region_key(), self.mint_region_key());
            self.regions.insert(
                left.start(),
                Region {
                    key: lk,
                    allocation: old.allocation,
                    range: left,
                    owner: old.owner,
                    kind: old_kind,
                    permit: None,
                },
            );
            self.regions.insert(
                right.start(),
                Region {
                    key: rk,
                    allocation: old.allocation,
                    range: right,
                    owner: old.owner,
                    kind: shift(old_kind, edge - old.range.start()),
                    permit: None,
                },
            );
        }
        Ok(())
    }

    /// 撤销预约只返回完整 owner；调用者门外逐许可取消并释放数组。
    pub fn rollback(&mut self, prepared: PreparedChange<C>) -> ChangeStorage<C> {
        let index = self.transaction_index(prepared.key, ChangeStage::Reserved);
        let record = self.transactions.remove(index);
        self.reserved_growth -= record.reserved_growth;
        self.regions
            .unreserve(prepared.storage.data().regions.len());
        prepared.storage
    }

    /// 不可失败的责任 Commit：只移交责任，不改账本。调用方必须先经
    /// UserWriteLease 发布 payload/cookie。账本随后经 [`Self::commit_step`]
    /// 分步推进；结果租约保留到 [`Self::complete`]（最后写回之后）。
    /// 不可失败的责任 Commit：只移交责任，不改账本。cookie/输出由调用者
    /// 先发布，账本随后经 commit_step 分步推进；结果租约保留至 complete。
    pub fn commit(&mut self, prepared: PreparedChange<C>) -> CommittedChange<C> {
        let index = self.transaction_index(prepared.key, ChangeStage::Reserved);
        let record = &mut self.transactions[index];
        record.stage = ChangeStage::Committed;
        record.ledger_pending = !(prepared.storage.data().remove.is_empty()
            && prepared.storage.data().regions.is_empty());
        CommittedChange(ChangePayload {
            key: prepared.key,
            plan: prepared.plan,
            storage: prepared.storage,
        })
    }

    /// 有界推进账本：至多 `budget` 个节点操作（摘一个旧节点或插一个替换
    /// 节点各计 1，节点归还与移出的写许可随该操作计入）。返回账本是否已到
    /// 终态。不分配：替换节点槽已在 reserve 预取，旧节点槽归还 free。
    pub fn commit_step(&mut self, committed: &mut CommittedChange<C>, budget: usize) -> bool {
        assert!(budget > 0, "ledger step budget must be nonzero");
        let index = self.transaction_index(committed.0.key, ChangeStage::Committed);
        let payload = committed.0.storage.data_mut();
        let mut spent = 0;
        while spent < budget {
            if let Some((start, key)) = payload.remove.pop() {
                let region = self
                    .regions
                    .remove(&start)
                    .expect("committed region removed by another writer");
                assert_eq!(
                    region.key, key,
                    "committed region replaced by another writer"
                );
                if let Some(permit) = region.permit {
                    payload.retiring_permits.push(permit);
                }
            } else if let Some(region) = payload.regions.pop() {
                self.regions.insert(region.range.start(), region);
                self.reserved_growth -= usize::from(self.transactions[index].reserved_growth > 0);
                let record = &mut self.transactions[index];
                record.reserved_growth = record.reserved_growth.saturating_sub(1);
            } else {
                break;
            }
            spent += 1;
        }
        let done = payload.remove.is_empty() && payload.regions.is_empty();
        if done {
            let record = &mut self.transactions[index];
            self.reserved_growth -= record.reserved_growth;
            record.reserved_growth = 0;
            record.ledger_pending = false;
        }
        done
    }

    /// 全部所需确认到齐后调用；RetiringChange 同时持有同步证明与实际退休 owner。
    /// 同步完成证明与退休数组由同一个 token 持有，不另分叉一个批次 owner。
    pub fn synchronize(&mut self, committed: CommittedChange<C>) -> RetiringChange<C> {
        assert!(
            committed.0.storage.data().remove.is_empty()
                && committed.0.storage.data().regions.is_empty(),
            "synchronize before ledger steps finished"
        );
        self.advance(
            committed.0.key,
            ChangeStage::Committed,
            ChangeStage::Retiring,
        );
        RetiringChange(committed.0)
    }

    /// 最后一项退休资源已真实归还后收口；batch 非空为结构性错误。
    /// 注销事务并带出所有存储；退休元素必须已真实归还，门内不析构付款。
    pub fn complete(&mut self, retiring: RetiringChange<C>) -> ChangeStorage<C> {
        assert!(
            retiring.is_empty(),
            "memory change cannot complete with live retire owners"
        );
        let index = self.transaction_index(retiring.0.key, ChangeStage::Retiring);
        self.transactions.remove(index);
        retiring.0.storage
    }

    /// 空间封口后的有界收束：无事务时每次从账本摘除一个 region，调用者据
    /// 返回的 backing/permit 推进真实资源 Retire。顺序无契约。
    pub fn drain_one(&mut self) -> Option<(RetiringFragment, Option<WritePermit>)> {
        assert!(
            self.transactions.is_empty(),
            "memory-space regions cannot drain with live transactions"
        );
        let start = self.regions.first()?.0;
        let region = self.regions.remove(&start).expect("first region exists");
        let view = region.view();
        Some((
            RetiringFragment {
                key: view.key,
                allocation: view.allocation,
                range: view.range,
                owner: view.owner,
                kind: view.kind,
                backing_retire: BackingRetire::Release,
            },
            region.permit,
        ))
    }

    /// 完整 owner 释放：每次摘除 `allocation` 的一个 region，只释放不分配，
    /// 不受请求上限约束；调用者持游标反复调用直至 None。
    pub fn release_allocation_step(
        &mut self,
        allocation: AllocationKey,
        cursor: &mut usize,
    ) -> Option<(RetiringFragment, Option<WritePermit>)> {
        let (start, _) = self
            .regions
            .iter_from(*cursor)
            .find(|(_, region)| region.allocation == allocation)?;
        *cursor = start;
        let region = self.regions.remove(&start).expect("found region exists");
        let view = region.view();
        Some((
            RetiringFragment {
                key: view.key,
                allocation: view.allocation,
                range: view.range,
                owner: view.owner,
                kind: view.kind,
                backing_retire: BackingRetire::Release,
            },
            region.permit,
        ))
    }

    fn validate_change_range(&self, range: PageRange) -> Result<(), ChangeError> {
        if !self.bounds.contains(range) {
            return Err(ChangeError::OutOfBounds);
        }
        if range.pages() > self.limits.max_pages_per_change {
            return Err(ChangeError::PageLimit);
        }
        Ok(())
    }

    /// 首次适配：按起点有序遍历 region（每步 O(log n)），事务足迹数受
    /// `max_transactions` 约束，逐候选复检。不再拷贝并排序全表。
    fn find_hole(&self, bytes: usize) -> Result<PageRange, ChangeError> {
        let mut cursor = self.bounds.start();
        let mut regions = self
            .regions
            .iter()
            .map(|(_, region)| region.range)
            .peekable();
        loop {
            let end = cursor
                .checked_add(bytes)
                .ok_or(ChangeError::Range(RangeError::Overflow))?;
            if end > self.bounds.end() {
                return Err(ChangeError::Conflict);
            }
            while regions.peek().is_some_and(|range| range.end() <= cursor) {
                regions.next();
            }
            let candidate = PageRange::new(cursor, bytes)?;
            if let Some(range) = regions.peek().filter(|range| range.overlaps(candidate)) {
                cursor = range.end();
                continue;
            }
            if let Some(blocker) = self
                .transactions
                .iter()
                .filter(|record| record.footprint.overlaps(candidate))
                .map(|record| record.footprint.end())
                .max()
            {
                cursor = blocker;
                continue;
            }
            return Ok(candidate);
        }
    }

    fn range_empty(&self, range: PageRange) -> bool {
        !self.regions_overlap(range)
            && !self
                .transactions
                .iter()
                .any(|record| record.footprint.overlaps(range))
    }

    fn covered_indices(
        &self,
        range: PageRange,
        authority: RegionOwner,
        mappings_only: bool,
    ) -> Result<Coverage<'_, C>, ChangeError> {
        let mut first = None;
        let mut count = 0;
        let mut cursor = range.start();
        for (index, region) in self.regions.iter_from(
            self.regions
                .floor(&range.start())
                .map_or(range.start(), |(k, _)| k),
        ) {
            if region.range.end() <= cursor {
                continue;
            }
            if region.range.start() > cursor {
                return Err(ChangeError::NotCovered);
            }
            if !region.range.overlaps(range) {
                if cursor >= range.end() {
                    break;
                }
                continue;
            }
            if region.owner != authority {
                return Err(ChangeError::OwnerDenied);
            }
            if matches!(region.kind, RegionKind::Mapping { backing, .. }
                if backing.mapping().is_some() && backing.mapping() == self.frozen_mapping)
            {
                return Err(ChangeError::Busy);
            }
            if mappings_only && matches!(region.kind, RegionKind::Guard) {
                return Err(ChangeError::Guard);
            }
            first.get_or_insert(index);
            count += 1;
            cursor = region.range.end().min(range.end());
            if cursor == range.end() {
                return Ok(Coverage {
                    regions: &self.regions,
                    // 非空 PageRange 已由至少一个 region 覆盖，故首 key 必存在。
                    first: first.expect("nonempty coverage lost first region"),
                    last: index,
                    count,
                });
            }
        }
        Err(ChangeError::NotCovered)
    }

    fn validate_user_write_lease(
        &self,
        range: AddressRange,
        mutation: PageRange,
        segments: &mut funded_storage::Buffer<UserWriteSegment, C>,
    ) -> Result<(), ChangeError> {
        if range.bytes() > self.limits.max_lease_bytes {
            return Err(ChangeError::LeaseTooLarge);
        }
        if !self.bounds.address_range().contains(range) {
            return Err(ChangeError::OutOfBounds);
        }
        self.ensure_no_transaction_conflict(mutation, Some(range))?;
        let mut cursor = range.start();
        for (_, region) in self.regions.iter_from(
            self.regions
                .floor(&range.start())
                .map_or(range.start(), |(k, _)| k),
        ) {
            if region.range.end() <= cursor {
                continue;
            }
            if region.range.start() > cursor {
                return Err(ChangeError::LeaseInvalid);
            }
            let RegionKind::Mapping {
                backing,
                current: Protection::ReadWrite,
                ..
            } = region.kind
            else {
                return Err(ChangeError::LeaseInvalid);
            };
            if !backing.is_ram() {
                return Err(ChangeError::LeaseInvalid);
            }
            let segment_end = region.range.end().min(range.end());
            let user = AddressRange::from_bounds(cursor, segment_end)?;
            if segments.len() >= self.limits.max_lease_segments {
                return Err(ChangeError::LeaseTooLarge);
            }
            segments.push(UserWriteSegment {
                region: region.key,
                user,
                backing,
                backing_offset: backing
                    .offset()
                    .checked_add(cursor - region.range.start())
                    .ok_or(ChangeError::BackingOutOfRange)?,
            });
            cursor = segment_end;
            if cursor == range.end() {
                return Ok(());
            }
        }
        Err(ChangeError::LeaseInvalid)
    }

    fn ensure_no_transaction_conflict(
        &self,
        footprint: PageRange,
        lease: Option<AddressRange>,
    ) -> Result<(), ChangeError> {
        for record in self.transactions.iter() {
            if record.footprint.overlaps(footprint)
                || record
                    .lease
                    .is_some_and(|pinned| pinned.overlaps(footprint.address_range()))
                || lease.is_some_and(|range| {
                    record.footprint.address_range().overlaps(range)
                        || record.lease.is_some_and(|pinned| pinned.overlaps(range))
                })
            {
                return Err(ChangeError::Busy);
            }
        }
        Ok(())
    }

    fn check_projected_regions(&self, growth: usize) -> Result<(), ChangeError> {
        let projected = self
            .regions
            .len()
            .checked_add(self.reserved_growth)
            .and_then(|count| count.checked_add(growth))
            .ok_or(ChangeError::RegionLimit)?;
        if projected > self.limits.max_regions {
            Err(ChangeError::RegionLimit)
        } else {
            Ok(())
        }
    }

    fn snapshot_valid(&self, snapshot: &[(usize, RegionKey)]) -> bool {
        snapshot.iter().all(|&(start, key)| {
            self.regions
                .get(&start)
                .is_some_and(|region| region.key == key)
        })
    }

    fn plan_still_valid(&self, validated: &ValidatedChange<C>) -> bool {
        if validated.storage.data().remove.is_empty()
            && validated.plan.result_layout.is_some()
            && !self.range_empty(validated.plan.footprint)
        {
            return false;
        }
        if validated.plan.lease.is_some() {
            validated.storage.data().segments.iter().all(|segment| {
                self.regions
                    .floor(&segment.user.start())
                    .is_some_and(|(_, region)| {
                        region.key == segment.region
                            && matches!(
                                region.kind,
                                RegionKind::Mapping {
                                    current: Protection::ReadWrite,
                                    ..
                                }
                            )
                            && region.range.address_range().contains(segment.user)
                    })
            })
        } else {
            true
        }
    }

    fn region_at(&self, start: usize) -> &Region {
        self.regions
            .get(&start)
            .expect("covered region start is live")
    }

    /// O(log n)：有序且不重叠，只需检查 floor(start) 与 ceil(start) 两个候选。
    fn regions_overlap(&self, range: PageRange) -> bool {
        self.regions
            .floor(&range.start())
            .is_some_and(|(_, region)| region.range.overlaps(range))
            || self
                .regions
                .ceil(&range.start())
                .is_some_and(|(_, region)| region.range.overlaps(range))
    }

    fn transaction_index(&self, key: ChangeKey, stage: ChangeStage) -> usize {
        self.transactions
            .iter()
            .position(|record| record.key == key && record.stage == stage)
            .expect("change token belongs to another memory space or stage")
    }

    fn advance(&mut self, key: ChangeKey, from: ChangeStage, to: ChangeStage) {
        let index = self.transaction_index(key, from);
        self.transactions[index].stage = to;
    }

    fn mint_allocation_key(&mut self) -> AllocationKey {
        let key = AllocationKey(self.next_allocation);
        self.next_allocation += 1;
        key
    }

    fn mint_region_key(&mut self) -> RegionKey {
        let key = RegionKey(self.next_region);
        self.next_region += 1;
        key
    }

    fn mint_change_key(&mut self) -> ChangeKey {
        let key = ChangeKey(self.next_change);
        self.next_change += 1;
        key
    }
}

fn round_page_bytes(bytes: usize) -> Result<usize, ChangeError> {
    if bytes == 0 {
        return Err(ChangeError::Range(RangeError::Empty));
    }
    bytes
        .checked_add(PAGE_SIZE - 1)
        .ok_or(ChangeError::Range(RangeError::Overflow))
        .map(|value| value / PAGE_SIZE * PAGE_SIZE)
}

fn round_guard_bytes(bytes: usize) -> Result<usize, ChangeError> {
    if bytes == 0 {
        Ok(0)
    } else {
        round_page_bytes(bytes)
    }
}

fn template_from_region(
    region: &Region,
    range: PageRange,
    protection: Option<Protection>,
) -> Result<RegionTemplate, ChangeError> {
    let kind = match region.kind {
        RegionKind::Guard => TemplateKind::Guard,
        RegionKind::Mapping {
            backing,
            current,
            maximum,
        } => TemplateKind::Mapping {
            backing: backing
                .shifted(range.start() - region.range.start())
                .ok_or(ChangeError::BackingOutOfRange)?,
            current: protection.unwrap_or(current),
            maximum,
        },
    };
    Ok(RegionTemplate {
        allocation: AllocationSource::Existing(region.allocation),
        range,
        owner: region.owner,
        kind,
        write_successor: region.permit.is_some()
            && matches!(
                kind,
                TemplateKind::Mapping {
                    current: Protection::ReadWrite,
                    ..
                }
            ),
    })
}

fn retire_template(
    region: &Region,
    range: PageRange,
    backing_retire: BackingRetire,
) -> Result<RetireTemplate, ChangeError> {
    let kind = match region.kind {
        RegionKind::Guard => RegionKindView::Guard,
        RegionKind::Mapping {
            backing,
            current,
            maximum,
        } => RegionKindView::Mapping {
            backing: backing
                .shifted(range.start() - region.range.start())
                .ok_or(ChangeError::BackingOutOfRange)?,
            current,
            maximum,
        },
    };
    Ok(RetireTemplate {
        allocation: region.allocation,
        range,
        owner: region.owner,
        kind,
        backing_retire,
    })
}

fn permit_requirements<C>(
    replacements: &[RegionTemplate],
    requirements: &mut funded_storage::Buffer<PermitRequirement, C>,
) -> Result<(), ChangeError> {
    for template in replacements {
        let Some(object) = template.writable_object() else {
            continue;
        };
        let is_new = usize::from(!template.write_successor);
        if let Some(requirement) = requirements
            .iter_mut()
            .find(|requirement| requirement.object == object)
        {
            requirement.count = requirement
                .count
                .checked_add(1)
                .ok_or(ChangeError::PermitMismatch)?;
            requirement.new_writes = requirement
                .new_writes
                .checked_add(is_new)
                .ok_or(ChangeError::PermitMismatch)?;
        } else {
            requirements.push(PermitRequirement {
                object,
                count: 1,
                new_writes: is_new,
            });
        }
    }
    Ok(())
}

fn permits_match(requirements: &[PermitRequirement], permits: &[WritePermit]) -> bool {
    requirements.iter().all(|requirement| {
        permits
            .iter()
            .filter(|permit| permit.object() == requirement.object)
            .count()
            == requirement.count
    }) && permits.len()
        == requirements
            .iter()
            .map(|requirement| requirement.count)
            .sum()
}

fn take_permit<C>(
    permits: &mut funded_storage::Buffer<WritePermit, C>,
    object: ObjectId,
) -> Result<WritePermit, ChangeError> {
    let index = permits
        .iter()
        .position(|permit| permit.object() == object)
        .ok_or(ChangeError::PermitMismatch)?;
    Ok(permits.remove(index))
}

fn templates_compatible(left: RegionTemplate, right: RegionTemplate) -> bool {
    if !left.range.adjacent(right.range)
        || left.range.end() != right.range.start()
        || left.owner != right.owner
    {
        return false;
    }
    let (AllocationSource::Existing(left_allocation), AllocationSource::Existing(right_allocation)) =
        (left.allocation, right.allocation)
    else {
        return false;
    };
    if left_allocation != right_allocation {
        return false;
    }
    match (left.kind, right.kind) {
        (TemplateKind::Guard, TemplateKind::Guard) => true,
        (
            TemplateKind::Mapping {
                backing: left_backing,
                current: left_current,
                maximum: left_maximum,
            },
            TemplateKind::Mapping {
                backing: right_backing,
                current: right_current,
                maximum: right_maximum,
            },
        ) => {
            left_current == right_current
                && left_maximum == right_maximum
                && left.writable_object().is_none()
                && left_backing.shifted(left.range.bytes()) == Some(right_backing)
        }
        _ => false,
    }
}

fn templates_compatible_region(region: &Region, template: &RegionTemplate) -> bool {
    let Ok(region_template) = template_from_region(region, region.range, None) else {
        return false;
    };
    if region.range.end() == template.range.start() {
        templates_compatible(region_template, *template)
    } else if template.range.end() == region.range.start() {
        templates_compatible(*template, region_template)
    } else {
        false
    }
}

fn normalize_templates<C>(
    templates: &mut funded_storage::Buffer<RegionTemplate, C>,
) -> Result<(), ChangeError> {
    templates.sort_unstable_by_key(|template| template.range.start());
    let mut output = 0;
    for input in 0..templates.len() {
        let template = templates[input];
        if output > 0 && templates_compatible(templates[output - 1], template) {
            let previous = &mut templates[output - 1];
            previous.range = PageRange::from_bounds(previous.range.start(), template.range.end())?;
            continue;
        }
        templates[output] = template;
        output += 1;
    }
    templates.truncate(output);
    Ok(())
}
