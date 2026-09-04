//! 公共 MemoryObject 与 Tunnel 的统一对象 core。
//!
//! `MemoryObjectCore` 同时持：ObjectBacking（固定长度、堆化多 extent 的资金化
//! backing）、MemoryObjectState（Mutable → Sealing → Executable 状态机，内含对象身份
//! 与固定长度）与 metadata owner（sponsor 强引用 + backing permit）。
//!
//! 对象身份从内核对象身份序列（`object::try_mint_koid`）铸造后交给状态机保管，不在
//! core 上另存一份——身份、长度与可执行状态同属对象的逻辑状态，单一真值点避免二者
//! 失步。
//!
//! 等待面不属于 core：Tunnel 的等待面在 Endpoint 上，公共 MemoryObject 的等待面在其
//! 公共 shell 上，二者各自拥有独立的 ObjectWaitState。

use alloc::sync::Arc;

use memory_space::{MemoryObjectState, ObjectId};

use crate::{
    frame::{self, ObjectBacking},
    sync::Spinlock,
    task::{
        memory_pool::MemoryPool,
        object,
        resources::{ConnectionPermit, MetadataSponsor, ObjectBackingPermit},
    },
};

use erhino_shared::call::SystemCallError;

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

/// 公共 MemoryObject 与 Tunnel Connection 的统一 core。
///
/// 持有对象身份、backing、状态机与 metadata 生命周期 owner。状态机经 `MEMORY_OBJECT`
/// 锁秩包裹。
pub(crate) struct MemoryObjectCore {
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
        let identity = mint_object_id()?;

        let backing = frame::fund_object_backing(
            pool,
            1,
            funded_frame::Limits {
                max_pages: 1,
                max_extents: 1,
            },
        )
        .map_err(map_fund_error)?;

        let object_bytes = backing.pages() * super::proc::PAGE_SIZE;
        let state = Spinlock::new(
            crate::sync::ranks::MEMORY_OBJECT,
            MemoryObjectState::new(identity, object_bytes, 2),
        );

        let core = Self {
            backing,
            state,
            _sponsor: Arc::clone(sponsor),
            _backing_permit: backing_permit,
        };

        Ok((core, connection_permit))
    }
}

