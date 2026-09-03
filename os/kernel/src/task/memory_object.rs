//! 公共 MemoryObject 与 Tunnel 的统一对象 core。
//!
//! `MemoryObjectCore` 同时持：ObjectId（全局单调铸造）、ObjectBacking（固定长度多 extent
//! funded storage）、MemoryObjectState（Mutable → Sealing → Executable 状态机）、
//! ObjectWaitState（EXECUTABLE 电平位 + 订阅队列）、metadata permits + sponsor 强引用。
//!
//! 公共 MemoryObject shell 与 Tunnel Connection 都复用同一 core，不存在双来源 ObjectId
//! 或重复状态机。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use memory_space::{MemoryObjectState, ObjectId};

use crate::{
    frame::{self, ObjectBacking},
    sync::Spinlock,
    task::{
        memory_pool::MemoryPool,
        object::ObjectWaitState,
        resources::{ConnectionPermit, EndpointPermit, InvitationPermit, MetadataSponsor, ObjectBackingPermit},
    },
};

use erhino_shared::{call::SystemCallError, object::ObjectSignals};

static NEXT_OBJECT_ID: AtomicU64 = AtomicU64::new(1);

fn mint_object_id() -> ObjectId {
    let identity = NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed);
    ObjectId::new(identity).expect("MemoryObject identity exhausted")
}

/// 公共 MemoryObject 与 Tunnel Connection 的统一 core。
///
/// 持有对象身份、backing、状态机、等待面与 metadata 生命周期 owner。Connection 的
/// MemoryObjectState 与 ObjectWaitState 分别经 `CONNECTION` 与 `OBJECT_WAIT` 锁秩包裹。
pub(crate) struct MemoryObjectCore {
    pub(crate) identity: ObjectId,
    pub(crate) backing: ObjectBacking,
    pub(crate) state: Spinlock<MemoryObjectState>,
    pub(crate) wait: Spinlock<ObjectWaitState>,
    _sponsor: Arc<MetadataSponsor>,
    _backing_permit: ObjectBackingPermit,
}

impl MemoryObjectCore {
    /// 为 Tunnel Connection 创建内部对象 core（单页，可变，无初始等待）。
    ///
    /// 除了 backing 长度固定为单页外，其余与公共 MemoryObject 完全相同——同一套
    /// ObjectId 铸造、状态机与 metadata admission。Connection 两端 Endpoint 共享
    /// 同一对象，状态机 permit_limit 设为 2。
    pub(crate) fn new_tunnel_connection(
        pool: &Arc<MemoryPool>,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<
        (
            Self,
            ConnectionPermit,
            EndpointPermit,
            EndpointPermit,
            InvitationPermit,
        ),
        SystemCallError,
    > {
        let backing_permit = MetadataSponsor::reserve_object_backing(sponsor)?;
        let connection_permit = MetadataSponsor::reserve_connection(sponsor)?;
        let endpoint_0_permit = MetadataSponsor::reserve_endpoint(sponsor)?;
        let endpoint_1_permit = MetadataSponsor::reserve_endpoint(sponsor)?;
        let invitation_permit = MetadataSponsor::reserve_invitation(sponsor)?;

        let backing = frame::fund_object_backing(
            pool,
            1,
            funded_frame::Limits {
                max_pages: 1,
                max_extents: 1,
            },
        )
        .map_err(|_| SystemCallError::OutOfMemory)?;

        let identity = mint_object_id();
        let state = Spinlock::new(
            crate::sync::ranks::MEMORY_OBJECT,
            MemoryObjectState::new(identity, 2),
        );
        let wait = Spinlock::new(
            crate::sync::ranks::OBJECT_WAIT,
            ObjectWaitState::new(ObjectSignals::NONE),
        );

        let core = Self {
            identity,
            backing,
            state,
            wait,
            _sponsor: Arc::clone(sponsor),
            _backing_permit: backing_permit,
        };

        Ok((
            core,
            connection_permit,
            endpoint_0_permit,
            endpoint_1_permit,
            invitation_permit,
        ))
    }
}
