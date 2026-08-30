use alloc::vec::Vec;

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
struct ChangeKey(u64);

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

#[derive(Debug, PartialEq, Eq)]
pub enum MapBacking {
    Anonymous {
        identity: BackingId,
        class: AnonymousClass,
    },
    Object {
        authorization: ObjectViewAuthorization,
        offset: usize,
        object_bytes: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackingView {
    Anonymous {
        identity: BackingId,
        class: AnonymousClass,
        offset: usize,
    },
    Object {
        object: ObjectId,
        offset: usize,
    },
}

impl BackingView {
    pub const fn offset(self) -> usize {
        match self {
            Self::Anonymous { offset, .. } | Self::Object { offset, .. } => offset,
        }
    }

    pub const fn object(self) -> Option<ObjectId> {
        match self {
            Self::Anonymous { .. } => None,
            Self::Object { object, .. } => Some(object),
        }
    }

    fn shifted(self, bytes: usize) -> Option<Self> {
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
    pub backing: MapBacking,
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
    AllocationFailed,
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
    pub object: ObjectId,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationIntent {
    Install {
        range: PageRange,
        backing: BackingView,
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
        backing: BackingView,
        current: Protection,
        maximum: Protection,
        holds_write_permit: bool,
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

#[derive(Debug)]
enum RegionKind {
    Guard,
    Mapping {
        backing: BackingView,
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
                    holds_write_permit: self.permit.is_some(),
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserWriteSegment {
    pub region: RegionKey,
    pub user: AddressRange,
    pub backing: BackingView,
    pub backing_offset: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct UserWriteProjection {
    segments: Vec<UserWriteSegment>,
}

impl UserWriteProjection {
    pub fn segments(&self) -> &[UserWriteSegment] {
        &self.segments
    }
}

#[derive(Debug, PartialEq, Eq)]
#[must_use = "the user write lease must be committed or rolled back"]
pub struct UserWriteLease {
    range: AddressRange,
    projection: UserWriteProjection,
}

impl UserWriteLease {
    pub const fn range(&self) -> AddressRange {
        self.range
    }

    pub const fn projection(&self) -> &UserWriteProjection {
        &self.projection
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetiringFragment {
    pub key: RegionKey,
    pub allocation: AllocationKey,
    pub range: PageRange,
    pub owner: RegionOwner,
    pub kind: RegionKindView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    Free,
    Guard,
    Mapping { protection: Protection },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeStage {
    Reserved,
    Committed,
    Published,
    Synchronized,
    Retired,
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
        backing: BackingView,
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
}

impl RegionTemplate {
    fn writable_object(self) -> Option<ObjectId> {
        match self.kind {
            TemplateKind::Mapping {
                backing: BackingView::Object { object, .. },
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
}

#[derive(Debug)]
struct ValidatedPlan {
    remove: Vec<RegionKey>,
    replacements: Vec<RegionTemplate>,
    retiring: Vec<RetireTemplate>,
    translations: Vec<TranslationIntent>,
    footprint: PageRange,
    result_layout: Option<MapResultLayout>,
}

#[derive(Debug)]
#[must_use = "validated changes must be reserved or discarded"]
pub struct ValidatedChange {
    plan: ValidatedPlan,
    snapshot: Vec<RegionKey>,
    lease: Option<(AddressRange, Vec<UserWriteSegment>)>,
    permits: Vec<PermitRequirement>,
    growth: usize,
}

impl ValidatedChange {
    pub fn permit_requirements(&self) -> &[PermitRequirement] {
        &self.permits
    }

    pub const fn map_result(&self) -> Option<MapResultLayout> {
        self.plan.result_layout
    }
}

#[derive(Debug)]
struct PreparedPlan {
    remove: Vec<RegionKey>,
    replacements: Vec<Region>,
    retiring: Vec<RetiringFragment>,
    translations: Vec<TranslationIntent>,
    result_layout: Option<MapResultLayout>,
    retiring_permits: Vec<WritePermit>,
}

struct MaterializedReservation {
    replacements: Vec<Region>,
    retiring: Vec<RetiringFragment>,
    retiring_permits: Vec<WritePermit>,
    permits: Vec<WritePermit>,
}

#[derive(Debug)]
#[must_use = "prepared changes must be committed or rolled back"]
pub struct PreparedChange {
    key: ChangeKey,
    plan: PreparedPlan,
    permits: Vec<WritePermit>,
    lease: Option<UserWriteLease>,
}

impl PreparedChange {
    pub const fn user_write_lease(&self) -> Option<&UserWriteLease> {
        self.lease.as_ref()
    }

    pub const fn map_result(&self) -> Option<MapResultLayout> {
        self.plan.result_layout
    }

    pub fn translation_intents(&self) -> &[TranslationIntent] {
        &self.plan.translations
    }
}

#[derive(Debug)]
pub struct ReserveFailure {
    pub error: ChangeError,
    validated: ValidatedChange,
    permits: Vec<WritePermit>,
}

impl ReserveFailure {
    pub fn into_parts(self) -> (ChangeError, ValidatedChange, Vec<WritePermit>) {
        (self.error, self.validated, self.permits)
    }
}

#[derive(Debug)]
struct ChangePayload {
    key: ChangeKey,
    translations: Vec<TranslationIntent>,
    retiring: Vec<RetiringFragment>,
    retiring_permits: Vec<WritePermit>,
    result_layout: Option<MapResultLayout>,
}

macro_rules! change_token {
    ($name:ident) => {
        #[derive(Debug)]
        #[must_use = "memory changes must advance to Complete"]
        pub struct $name(ChangePayload);

        impl $name {
            pub fn translation_intents(&self) -> &[TranslationIntent] {
                &self.0.translations
            }

            pub const fn map_result(&self) -> Option<MapResultLayout> {
                self.0.result_layout
            }
        }
    };
}

change_token!(CommittedChange);
change_token!(PublishedChange);
change_token!(SynchronizedChange);
change_token!(RetiredChange);

#[derive(Debug)]
pub struct RetireBatch {
    fragments: Vec<RetiringFragment>,
    permits: Vec<WritePermit>,
}

impl RetireBatch {
    pub fn fragments(&self) -> &[RetiringFragment] {
        &self.fragments
    }

    pub fn into_parts(self) -> (Vec<RetiringFragment>, Vec<WritePermit>) {
        (self.fragments, self.permits)
    }
}

#[derive(Debug)]
struct TransactionRecord {
    key: ChangeKey,
    footprint: PageRange,
    lease: Option<AddressRange>,
    stage: ChangeStage,
    reserved_growth: usize,
}

#[derive(Debug)]
pub struct MemorySpace {
    bounds: PageRange,
    limits: Limits,
    regions: Vec<Region>,
    transactions: Vec<TransactionRecord>,
    next_allocation: u64,
    next_region: u64,
    next_change: u64,
    reserved_growth: usize,
}

impl MemorySpace {
    pub fn new(bounds: PageRange, limits: Limits) -> Result<Self, ChangeError> {
        if limits.max_regions == 0
            || limits.max_transactions == 0
            || limits.max_pages_per_change == 0
            || limits.max_lease_bytes == 0
            || limits.max_lease_segments == 0
        {
            return Err(ChangeError::BadLimits);
        }
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(limits.max_regions)
            .map_err(|_| ChangeError::AllocationFailed)?;
        let mut transactions = Vec::new();
        transactions
            .try_reserve_exact(limits.max_transactions)
            .map_err(|_| ChangeError::AllocationFailed)?;
        Ok(Self {
            bounds,
            limits,
            regions,
            transactions,
            next_allocation: 1,
            next_region: 1,
            next_change: 1,
            reserved_growth: 0,
        })
    }

    pub const fn bounds(&self) -> PageRange {
        self.bounds
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    pub fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    pub fn regions(&self) -> impl Iterator<Item = RegionView> + '_ {
        self.regions.iter().map(Region::view)
    }

    pub fn fault_class(&self, address: usize) -> FaultClass {
        let Some(region) = self
            .regions
            .iter()
            .find(|region| region.range.address_range().contains_address(address))
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

    pub fn validate_map(&self, request: MapRequest) -> Result<ValidatedChange, ChangeError> {
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
        if self
            .regions
            .iter()
            .any(|region| region.range.overlaps(reservation))
        {
            return Err(ChangeError::Conflict);
        }
        let usable = PageRange::new(reservation.start() + guard_before, usable_bytes)?;
        let layout = MapResultLayout {
            usable,
            reservation,
        };

        let backing = match request.backing {
            MapBacking::Anonymous { identity, class } => {
                let allowed = match class {
                    AnonymousClass::Data => request.maximum != Protection::ReadExecute,
                    AnonymousClass::InitialExecutable => request.maximum != Protection::ReadWrite,
                };
                if !allowed {
                    return Err(ChangeError::PermissionDenied);
                }
                BackingView::Anonymous {
                    identity,
                    class,
                    offset: 0,
                }
            }
            MapBacking::Object {
                authorization,
                offset,
                object_bytes,
            } => {
                if authorization.maximum() != request.maximum {
                    return Err(ChangeError::ObjectAuthorization);
                }
                let end = offset
                    .checked_add(usable_bytes)
                    .ok_or(ChangeError::BackingOutOfRange)?;
                if end > object_bytes {
                    return Err(ChangeError::BackingOutOfRange);
                }
                BackingView::Object {
                    object: authorization.object(),
                    offset,
                }
            }
        };

        let mut replacements = Vec::new();
        replacements
            .try_reserve_exact(1 + usize::from(guard_before > 0) + usize::from(guard_after > 0))
            .map_err(|_| ChangeError::AllocationFailed)?;
        if guard_before > 0 {
            replacements.push(RegionTemplate {
                allocation: AllocationSource::New,
                range: PageRange::new(reservation.start(), guard_before)?,
                owner: request.owner,
                kind: TemplateKind::Guard,
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
        });
        if guard_after > 0 {
            replacements.push(RegionTemplate {
                allocation: AllocationSource::New,
                range: PageRange::new(usable.end(), guard_after)?,
                owner: request.owner,
                kind: TemplateKind::Guard,
            });
        }

        let mut translations = Vec::new();
        translations
            .try_reserve_exact(1)
            .map_err(|_| ChangeError::AllocationFailed)?;
        translations.push(TranslationIntent::Install {
            range: usable,
            backing,
            protection: request.current,
        });

        let mut snapshot = Vec::new();
        let lease = match request.result {
            Some(lease) => {
                let (segments, keys) = self.validate_user_write_lease(lease.range, reservation)?;
                snapshot = keys;
                Some((lease.range, segments))
            }
            None => None,
        };
        let permits = permit_requirements(&replacements)?;
        let growth = replacements.len();
        self.check_projected_regions(growth)?;
        Ok(ValidatedChange {
            plan: ValidatedPlan {
                remove: Vec::new(),
                replacements,
                retiring: Vec::new(),
                translations,
                footprint: reservation,
                result_layout: Some(layout),
            },
            snapshot,
            lease,
            permits,
            growth,
        })
    }

    pub fn validate_unmap(&self, request: UnmapRequest) -> Result<ValidatedChange, ChangeError> {
        self.validate_change_range(request.range)?;
        self.ensure_no_transaction_conflict(request.range, None)?;
        let indices = self.covered_indices(request.range, request.authority, false)?;

        let mut remove = Vec::new();
        let mut replacements = Vec::new();
        let mut retiring = Vec::new();
        let mut translations = Vec::new();
        remove
            .try_reserve_exact(indices.len())
            .map_err(|_| ChangeError::AllocationFailed)?;
        let replacement_capacity = indices
            .len()
            .checked_mul(2)
            .ok_or(ChangeError::RegionLimit)?;
        replacements
            .try_reserve_exact(replacement_capacity)
            .map_err(|_| ChangeError::AllocationFailed)?;
        retiring
            .try_reserve_exact(indices.len())
            .map_err(|_| ChangeError::AllocationFailed)?;
        translations
            .try_reserve_exact(indices.len())
            .map_err(|_| ChangeError::AllocationFailed)?;

        for index in indices {
            let region = &self.regions[index];
            let cut = region
                .range
                .intersection(request.range)
                .expect("covered region intersects request");
            remove.push(region.key);
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
            retiring.push(retire_template(region, cut)?);
            if matches!(region.kind, RegionKind::Mapping { .. }) {
                translations.push(TranslationIntent::Remove { range: cut });
            }
        }

        replacements.sort_unstable_by_key(|region| region.range.start());
        let permits = permit_requirements(&replacements)?;
        let growth = replacements.len().saturating_sub(remove.len());
        self.check_projected_regions(growth)?;
        let snapshot = remove.clone();
        Ok(ValidatedChange {
            plan: ValidatedPlan {
                remove,
                replacements,
                retiring,
                translations,
                footprint: request.range,
                result_layout: None,
            },
            snapshot,
            lease: None,
            permits,
            growth,
        })
    }

    pub fn validate_protect(
        &self,
        request: ProtectRequest,
    ) -> Result<ValidatedChange, ChangeError> {
        self.validate_change_range(request.range)?;
        self.ensure_no_transaction_conflict(request.range, None)?;
        let indices = self.covered_indices(request.range, request.authority, true)?;
        for &index in &indices {
            let RegionKind::Mapping { maximum, .. } = self.regions[index].kind else {
                return Err(ChangeError::Guard);
            };
            if !maximum.permits(request.protection) {
                return Err(ChangeError::PermissionDenied);
            }
        }

        let all_same = indices.iter().all(|&index| {
            matches!(
                self.regions[index].kind,
                RegionKind::Mapping { current, .. } if current == request.protection
            )
        });
        if all_same {
            let snapshot = indices
                .iter()
                .map(|&index| self.regions[index].key)
                .collect();
            return Ok(ValidatedChange {
                plan: ValidatedPlan {
                    remove: Vec::new(),
                    replacements: Vec::new(),
                    retiring: Vec::new(),
                    translations: Vec::new(),
                    footprint: request.range,
                    result_layout: None,
                },
                snapshot,
                lease: None,
                permits: Vec::new(),
                growth: 0,
            });
        }

        let mut remove = Vec::new();
        let mut replacements = Vec::new();
        let mut retiring = Vec::new();
        let mut translations = Vec::new();
        let remove_capacity = indices
            .len()
            .checked_add(2)
            .ok_or(ChangeError::RegionLimit)?;
        let replacement_capacity = indices
            .len()
            .checked_mul(3)
            .and_then(|count| count.checked_add(2))
            .ok_or(ChangeError::RegionLimit)?;
        remove
            .try_reserve_exact(remove_capacity)
            .map_err(|_| ChangeError::AllocationFailed)?;
        replacements
            .try_reserve_exact(replacement_capacity)
            .map_err(|_| ChangeError::AllocationFailed)?;
        retiring
            .try_reserve_exact(indices.len())
            .map_err(|_| ChangeError::AllocationFailed)?;
        translations
            .try_reserve_exact(indices.len())
            .map_err(|_| ChangeError::AllocationFailed)?;

        let first_index = *indices.first().expect("coverage is nonempty");
        let last_index = *indices.last().expect("coverage is nonempty");
        for &index in &indices {
            let region = &self.regions[index];
            let cut = region
                .range
                .intersection(request.range)
                .expect("covered region intersects request");
            remove.push(region.key);
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
            retiring.push(retire_template(region, cut)?);
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

        if first_index > 0 {
            let neighbor = &self.regions[first_index - 1];
            if neighbor.permit.is_none()
                && templates_compatible_region(
                    neighbor,
                    replacements
                        .iter()
                        .min_by_key(|region| region.range.start())
                        .expect("protect produces replacements"),
                )
            {
                remove.push(neighbor.key);
                replacements.push(template_from_region(neighbor, neighbor.range, None)?);
            }
        }
        if last_index + 1 < self.regions.len() {
            let neighbor = &self.regions[last_index + 1];
            if neighbor.permit.is_none()
                && templates_compatible_region(
                    neighbor,
                    replacements
                        .iter()
                        .max_by_key(|region| region.range.end())
                        .expect("protect produces replacements"),
                )
            {
                remove.push(neighbor.key);
                replacements.push(template_from_region(neighbor, neighbor.range, None)?);
            }
        }

        normalize_templates(&mut replacements)?;
        let footprint = PageRange::from_bounds(
            replacements
                .first()
                .map_or(request.range.start(), |region| region.range.start()),
            replacements
                .last()
                .map_or(request.range.end(), |region| region.range.end()),
        )?;
        self.ensure_no_transaction_conflict(footprint, None)?;
        let permits = permit_requirements(&replacements)?;
        let growth = replacements.len().saturating_sub(remove.len());
        self.check_projected_regions(growth)?;
        let snapshot = remove.clone();
        Ok(ValidatedChange {
            plan: ValidatedPlan {
                remove,
                replacements,
                retiring,
                translations,
                footprint,
                result_layout: None,
            },
            snapshot,
            lease: None,
            permits,
            growth,
        })
    }

    /// Reserve 失败必须原样归还 validated plan 与 affine permits。
    #[allow(clippy::result_large_err)]
    pub fn reserve(
        &mut self,
        validated: ValidatedChange,
        permits: Vec<WritePermit>,
    ) -> Result<PreparedChange, ReserveFailure> {
        if let Err(error) = self.check_reservation(&validated, &permits) {
            return Err(ReserveFailure {
                error,
                validated,
                permits,
            });
        }
        let materialized = match self.materialize_reservation(&validated, permits) {
            Ok(materialized) => materialized,
            Err((error, permits)) => {
                return Err(ReserveFailure {
                    error,
                    validated,
                    permits,
                });
            }
        };
        let ValidatedChange {
            plan,
            lease,
            growth,
            ..
        } = validated;
        let ValidatedPlan {
            remove,
            translations,
            footprint,
            result_layout,
            ..
        } = plan;
        let key = self.mint_change_key();
        let lease = lease.map(|(range, segments)| UserWriteLease {
            range,
            projection: UserWriteProjection { segments },
        });
        self.transactions.push(TransactionRecord {
            key,
            footprint,
            lease: lease.as_ref().map(UserWriteLease::range),
            stage: ChangeStage::Reserved,
            reserved_growth: growth,
        });
        self.reserved_growth += growth;
        Ok(PreparedChange {
            key,
            plan: PreparedPlan {
                remove,
                replacements: materialized.replacements,
                retiring: materialized.retiring,
                translations,
                result_layout,
                retiring_permits: materialized.retiring_permits,
            },
            permits: materialized.permits,
            lease,
        })
    }

    fn check_reservation(
        &mut self,
        validated: &ValidatedChange,
        permits: &[WritePermit],
    ) -> Result<(), ChangeError> {
        if self.transactions.len() >= self.limits.max_transactions {
            return Err(ChangeError::TransactionLimit);
        }
        if !self.snapshot_valid(&validated.snapshot) || !self.plan_still_valid(validated) {
            return Err(ChangeError::Stale);
        }
        self.ensure_no_transaction_conflict(
            validated.plan.footprint,
            validated.lease.as_ref().map(|lease| lease.0),
        )?;
        if !permits_match(&validated.permits, permits) {
            return Err(ChangeError::PermitMismatch);
        }
        self.check_projected_regions(validated.growth)?;
        let required_region_keys = validated
            .plan
            .replacements
            .len()
            .checked_add(validated.plan.retiring.len())
            .and_then(|count| u64::try_from(count).ok())
            .ok_or(ChangeError::KeyExhausted)?;
        if self.next_region.checked_add(required_region_keys).is_none()
            || self.next_change.checked_add(1).is_none()
            || (validated.plan.result_layout.is_some()
                && self.next_allocation.checked_add(1).is_none())
        {
            return Err(ChangeError::KeyExhausted);
        }
        let reserved_growth = self
            .reserved_growth
            .checked_add(validated.growth)
            .ok_or(ChangeError::RegionLimit)?;
        self.regions
            .try_reserve(reserved_growth)
            .map_err(|_| ChangeError::AllocationFailed)?;
        self.transactions
            .try_reserve(1)
            .map_err(|_| ChangeError::AllocationFailed)?;
        Ok(())
    }

    fn materialize_reservation(
        &mut self,
        validated: &ValidatedChange,
        permits: Vec<WritePermit>,
    ) -> Result<MaterializedReservation, (ChangeError, Vec<WritePermit>)> {
        let mut replacements = Vec::new();
        if replacements
            .try_reserve_exact(validated.plan.replacements.len())
            .is_err()
        {
            return Err((ChangeError::AllocationFailed, permits));
        }
        let mut retiring = Vec::new();
        if retiring
            .try_reserve_exact(validated.plan.retiring.len())
            .is_err()
        {
            return Err((ChangeError::AllocationFailed, permits));
        }
        let mut retiring_permits = Vec::new();
        if retiring_permits
            .try_reserve_exact(validated.snapshot.len())
            .is_err()
        {
            return Err((ChangeError::AllocationFailed, permits));
        }

        let mut supplied = permits;
        let allocation = validated
            .plan
            .result_layout
            .map(|_| self.mint_allocation_key());
        for template in &validated.plan.replacements {
            let permit = template
                .writable_object()
                .map(|object| take_permit(&mut supplied, object))
                .transpose()
                .expect("permit multiset was validated");
            replacements.push(Region {
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
        debug_assert!(supplied.is_empty());
        for template in &validated.plan.retiring {
            retiring.push(RetiringFragment {
                key: self.mint_region_key(),
                allocation: template.allocation,
                range: template.range,
                owner: template.owner,
                kind: template.kind,
            });
        }
        Ok(MaterializedReservation {
            replacements,
            retiring,
            retiring_permits,
            permits: supplied,
        })
    }

    pub fn rollback(&mut self, prepared: PreparedChange) -> Vec<WritePermit> {
        let index = self.transaction_index(prepared.key, ChangeStage::Reserved);
        let record = self.transactions.remove(index);
        self.reserved_growth -= record.reserved_growth;
        let mut permits = prepared.permits;
        for region in prepared.plan.replacements {
            if let Some(permit) = region.permit {
                permits.push(permit);
            }
        }
        permits
    }

    /// 不可失败的账本 Commit。调用方必须先经 UserWriteLease 发布 payload/cookie。
    pub fn commit(&mut self, prepared: PreparedChange) -> CommittedChange {
        let index = self.transaction_index(prepared.key, ChangeStage::Reserved);
        let mut retiring_permits = prepared.plan.retiring_permits;

        for key in &prepared.plan.remove {
            let region_index = self
                .regions
                .iter()
                .position(|region| region.key == *key)
                .expect("validated region key changed before Commit");
            let region = self.regions.remove(region_index);
            if let Some(permit) = region.permit {
                retiring_permits.push(permit);
            }
        }
        self.regions.extend(prepared.plan.replacements);
        self.regions
            .sort_unstable_by_key(|region| region.range.start());

        let record = &mut self.transactions[index];
        self.reserved_growth -= record.reserved_growth;
        record.reserved_growth = 0;
        record.lease = None;
        record.stage = ChangeStage::Committed;
        CommittedChange(ChangePayload {
            key: prepared.key,
            translations: prepared.plan.translations,
            retiring: prepared.plan.retiring,
            retiring_permits,
            result_layout: prepared.plan.result_layout,
        })
    }

    pub fn publish(&mut self, committed: CommittedChange) -> PublishedChange {
        self.advance(
            committed.0.key,
            ChangeStage::Committed,
            ChangeStage::Published,
        );
        PublishedChange(committed.0)
    }

    pub fn synchronize(&mut self, published: PublishedChange) -> SynchronizedChange {
        self.advance(
            published.0.key,
            ChangeStage::Published,
            ChangeStage::Synchronized,
        );
        SynchronizedChange(published.0)
    }

    pub fn retire(&mut self, synchronized: SynchronizedChange) -> (RetiredChange, RetireBatch) {
        self.advance(
            synchronized.0.key,
            ChangeStage::Synchronized,
            ChangeStage::Retired,
        );
        let mut payload = synchronized.0;
        let batch = RetireBatch {
            fragments: core::mem::take(&mut payload.retiring),
            permits: core::mem::take(&mut payload.retiring_permits),
        };
        (RetiredChange(payload), batch)
    }

    pub fn complete(&mut self, retired: RetiredChange) {
        let index = self.transaction_index(retired.0.key, ChangeStage::Retired);
        self.transactions.remove(index);
    }

    /// REAPABLE 后的有界收束：无事务时每次从账本摘除一个 region，调用者据
    /// 返回的 backing/permit 推进真实资源 Retire。顺序无契约。
    pub fn drain_one(&mut self) -> Option<(RetiringFragment, Option<WritePermit>)> {
        assert!(
            self.transactions.is_empty(),
            "memory-space regions cannot drain with live transactions"
        );
        let region = self.regions.pop()?;
        let view = region.view();
        Some((
            RetiringFragment {
                key: view.key,
                allocation: view.allocation,
                range: view.range,
                owner: view.owner,
                kind: view.kind,
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

    fn find_hole(&self, bytes: usize) -> Result<PageRange, ChangeError> {
        let obstacle_capacity = self
            .regions
            .len()
            .checked_add(self.transactions.len())
            .ok_or(ChangeError::RegionLimit)?;
        let mut obstacles = Vec::new();
        obstacles
            .try_reserve_exact(obstacle_capacity)
            .map_err(|_| ChangeError::AllocationFailed)?;
        obstacles.extend(self.regions.iter().map(|region| region.range));
        obstacles.extend(self.transactions.iter().map(|record| record.footprint));
        obstacles.sort_unstable_by_key(|range| range.start());

        let mut cursor = self.bounds.start();
        for obstacle in obstacles {
            if obstacle.end() <= cursor {
                continue;
            }
            if obstacle.start() > cursor && obstacle.start() - cursor >= bytes {
                return PageRange::new(cursor, bytes).map_err(Into::into);
            }
            cursor = cursor.max(obstacle.end());
        }
        let end = cursor
            .checked_add(bytes)
            .ok_or(ChangeError::Range(RangeError::Overflow))?;
        if end <= self.bounds.end() {
            PageRange::new(cursor, bytes).map_err(Into::into)
        } else {
            Err(ChangeError::Conflict)
        }
    }

    fn range_empty(&self, range: PageRange) -> bool {
        !self
            .regions
            .iter()
            .any(|region| region.range.overlaps(range))
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
    ) -> Result<Vec<usize>, ChangeError> {
        let mut indices = Vec::new();
        let mut cursor = range.start();
        for (index, region) in self.regions.iter().enumerate() {
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
            if mappings_only && matches!(region.kind, RegionKind::Guard) {
                return Err(ChangeError::Guard);
            }
            indices
                .try_reserve(1)
                .map_err(|_| ChangeError::AllocationFailed)?;
            indices.push(index);
            cursor = region.range.end().min(range.end());
            if cursor == range.end() {
                return Ok(indices);
            }
        }
        Err(ChangeError::NotCovered)
    }

    fn validate_user_write_lease(
        &self,
        range: AddressRange,
        mutation: PageRange,
    ) -> Result<(Vec<UserWriteSegment>, Vec<RegionKey>), ChangeError> {
        if range.bytes() > self.limits.max_lease_bytes {
            return Err(ChangeError::LeaseTooLarge);
        }
        if !self.bounds.address_range().contains(range) {
            return Err(ChangeError::OutOfBounds);
        }
        self.ensure_no_transaction_conflict(mutation, Some(range))?;
        let mut segments = Vec::new();
        let mut keys = Vec::new();
        let mut cursor = range.start();
        for region in &self.regions {
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
            let segment_end = region.range.end().min(range.end());
            let user = AddressRange::from_bounds(cursor, segment_end)?;
            if segments.len() >= self.limits.max_lease_segments {
                return Err(ChangeError::LeaseTooLarge);
            }
            segments
                .try_reserve(1)
                .map_err(|_| ChangeError::AllocationFailed)?;
            keys.try_reserve(1)
                .map_err(|_| ChangeError::AllocationFailed)?;
            segments.push(UserWriteSegment {
                region: region.key,
                user,
                backing,
                backing_offset: backing
                    .offset()
                    .checked_add(cursor - region.range.start())
                    .ok_or(ChangeError::BackingOutOfRange)?,
            });
            keys.push(region.key);
            cursor = segment_end;
            if cursor == range.end() {
                return Ok((segments, keys));
            }
        }
        Err(ChangeError::LeaseInvalid)
    }

    fn ensure_no_transaction_conflict(
        &self,
        footprint: PageRange,
        lease: Option<AddressRange>,
    ) -> Result<(), ChangeError> {
        for record in &self.transactions {
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

    fn snapshot_valid(&self, snapshot: &[RegionKey]) -> bool {
        snapshot
            .iter()
            .all(|key| self.regions.iter().any(|region| region.key == *key))
    }

    fn plan_still_valid(&self, validated: &ValidatedChange) -> bool {
        if validated.plan.remove.is_empty()
            && validated.plan.result_layout.is_some()
            && !self.range_empty(validated.plan.footprint)
        {
            return false;
        }
        if let Some((_range, segments)) = &validated.lease {
            segments.iter().all(|segment| {
                self.regions.iter().any(|region| {
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
    })
}

fn retire_template(region: &Region, range: PageRange) -> Result<RetireTemplate, ChangeError> {
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
            holds_write_permit: region.permit.is_some(),
        },
    };
    Ok(RetireTemplate {
        allocation: region.allocation,
        range,
        owner: region.owner,
        kind,
    })
}

fn permit_requirements(
    replacements: &[RegionTemplate],
) -> Result<Vec<PermitRequirement>, ChangeError> {
    let mut requirements: Vec<PermitRequirement> = Vec::new();
    for template in replacements {
        let Some(object) = template.writable_object() else {
            continue;
        };
        if let Some(requirement) = requirements
            .iter_mut()
            .find(|requirement| requirement.object == object)
        {
            requirement.count = requirement
                .count
                .checked_add(1)
                .ok_or(ChangeError::PermitMismatch)?;
        } else {
            requirements
                .try_reserve(1)
                .map_err(|_| ChangeError::AllocationFailed)?;
            requirements.push(PermitRequirement { object, count: 1 });
        }
    }
    Ok(requirements)
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

fn take_permit(
    permits: &mut Vec<WritePermit>,
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

fn normalize_templates(templates: &mut Vec<RegionTemplate>) -> Result<(), ChangeError> {
    templates.sort_unstable_by_key(|template| template.range.start());
    let mut normalized: Vec<RegionTemplate> = Vec::new();
    normalized
        .try_reserve_exact(templates.len())
        .map_err(|_| ChangeError::AllocationFailed)?;
    for template in templates.drain(..) {
        if let Some(previous) = normalized.last_mut()
            && templates_compatible(*previous, template)
        {
            previous.range = PageRange::from_bounds(previous.range.start(), template.range.end())?;
            continue;
        }
        normalized.push(template);
    }
    *templates = normalized;
    Ok(())
}
