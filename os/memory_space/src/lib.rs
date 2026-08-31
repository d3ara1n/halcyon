#![no_std]
#![forbid(unsafe_code)]

//! 用户地址空间的纯逻辑规划器。
//!
//! 本 crate 只拥有区域几何、身份、权限、事务阶段和 MemoryObject 写许可状态；
//! 不访问页表、物理帧、用户指针、hart 或内核对象。内核 AddressSpace adapter
//! 负责把这里产出的 translation/retire intent 与真实资源组合。

extern crate alloc;

mod object;
mod range;
mod space;

pub use object::{
    ExecutableState, MemoryObjectState, ObjectError, ObjectId, ObjectViewAuthorization,
    SealOutcome, WritePermit,
};
pub use range::{AddressRange, PAGE_SIZE, PageRange, RangeError};
pub use space::{
    AllocationKey, AnonymousClass, BackingId, BackingRetire, BackingView, ChangeError,
    CommittedChange, FaultClass, LeaseKey, Limits, MapBacking, MapPlacement, MapRequest,
    MapResultLayout, MemorySpace, PermitRequirement, PreparedChange, ProtectRequest, Protection,
    PublishedChange, RegionKey, RegionKindView, RegionOwner, RegionView, ReserveFailure,
    RetireBatch, RetiredChange, RetiringChange, RetiringFragment, SynchronizedChange,
    TranslationIntent, UnmapRequest, UserWriteLease, UserWriteLeaseRequest, UserWriteProjection,
    UserWriteSegment, ValidatedChange,
};
