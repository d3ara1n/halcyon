//! 公共 MemoryObject 与 Tunnel 的统一对象 core。
//!
//! `MemoryObjectCore` 同时持：ObjectId（全局单调铸造）、ObjectBacking（固定长度多 extent
//! funded storage）、MemoryObjectState（Mutable → Sealing → Executable 状态机）与
//! metadata owner（sponsor 强引用 + backing permit）。
//!
//! 等待面不属于 core：Tunnel 的等待面在 Endpoint 上，公共 MemoryObject 的等待面在其
//! 公共 shell 上，二者各自拥有独立的 ObjectWaitState。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use memory_space::{MemoryObjectState, ObjectId};

use crate::{
    frame::{self, ObjectBacking},
    sync::Spinlock,
    task::{
        memory_pool::MemoryPool,
        resources::{ConnectionPermit, MetadataSponsor, ObjectBackingPermit},
    },
};

use erhino_shared::call::SystemCallError;

static NEXT_OBJECT_ID: AtomicU64 = AtomicU64::new(1);

fn mint_object_id() -> ObjectId {
    let identity = NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed);
    ObjectId::new(identity).expect("MemoryObject identity exhausted")
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

/// 公共 MemoryObject 与 Tunnel Connection 的统一 core。
///
/// 持有对象身份、backing、状态机与 metadata 生命周期 owner。状态机经 `MEMORY_OBJECT`
/// 锁秩包裹。
pub(crate) struct MemoryObjectCore {
    pub(crate) identity: ObjectId,
    pub(crate) backing: ObjectBacking,
    pub(crate) state: Spinlock<MemoryObjectState>,
    _sponsor: Arc<MetadataSponsor>,
    _backing_permit: ObjectBackingPermit,
}

impl MemoryObjectCore {
    /// 为 Tunnel Connection 创建内部对象 core（单页，可变）。
    ///
    /// 除了 backing 长度固定为单页外，其余与公共 MemoryObject 完全相同——同一套
    /// ObjectId 铸造、状态机与 metadata admission。Connection 两端 Endpoint 共享
    /// 同一对象，状态机 permit_limit 设为 2。
    pub(crate) fn new_tunnel_connection(
        pool: &Arc<MemoryPool>,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<(Self, ConnectionPermit), SystemCallError> {
        let backing_permit = MetadataSponsor::reserve_object_backing(sponsor)?;
        let connection_permit = MetadataSponsor::reserve_connection(sponsor)?;

        let backing = frame::fund_object_backing(
            pool,
            1,
            funded_frame::Limits {
                max_pages: 1,
                max_extents: 1,
            },
        )
        .map_err(map_fund_error)?;

        let identity = mint_object_id();
        let object_bytes = backing.pages() * super::proc::PAGE_SIZE;
        let state = Spinlock::new(
            crate::sync::ranks::MEMORY_OBJECT,
            MemoryObjectState::new(identity, object_bytes, 2),
        );

        let core = Self {
            identity,
            backing,
            state,
            _sponsor: Arc::clone(sponsor),
            _backing_permit: backing_permit,
        };

        Ok((core, connection_permit))
    }
}

