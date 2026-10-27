//! MMIO 的两个实际 capability 角色与系统调用；普通 Close 不认识设备类型。
use super::{MmioNode, PreparedMmioNode};
use crate::task::{
    Thread,
    cap::{self, Cap},
    handle::{self, Authorization},
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectRef},
    request::{Pending, Request, Step},
    resources::MetadataCharge,
    wait_source::ObservationSource,
};
use alloc::sync::Arc;
use core::any::Any;
use erhino_shared::{
    call::SystemCallError,
    mmio::MmioSnapshot,
    object::{Handle, ObjectKind, ObjectSignals, Rights},
};

const USE_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
const CONTROL_RIGHTS: Rights = Rights::from_raw(USE_RIGHTS.raw() | Rights::CREATE.raw());

struct Object {
    header: ObjectHeader,
    holders: capability::Holders,
    node: Arc<MmioNode>,
    _metadata: MetadataCharge,
}
pub(crate) struct MmioControl(Object);
pub(crate) struct MmioUse(Object);

macro_rules! object {
    ($ty:ident, $rights:ident) => {
        impl $ty {
            fn new(
                node: Arc<MmioNode>,
                pool: &Arc<MemoryPool>,
            ) -> Result<Arc<Self>, SystemCallError> {
                let metadata = MetadataCharge::arc::<Self>(pool)?;
                Arc::try_new(Self(Object {
                    header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
                    holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
                    node,
                    _metadata: metadata,
                }))
                .map_err(|_| SystemCallError::OutOfMemory)
            }
        }
        impl KernelObject for $ty {
            fn header(&self) -> &ObjectHeader {
                &self.0.header
            }
            fn holders(&self) -> &capability::Holders {
                &self.0.holders
            }
            fn kind(&self) -> ObjectKind {
                ObjectKind::$ty
            }
            fn on_last_capability(&self) {
                self.0.node.revoke();
            }
            fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
                Some(self.0.node.completion.clone())
            }
            fn allowed_signals(&self) -> Option<ObjectSignals> {
                Some(ObjectSignals::DONE)
            }
            fn allowed_rights(&self) -> Option<Rights> {
                Some(if self.0.node.writable {
                    $rights
                } else {
                    $rights & !Rights::WRITE
                })
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
    };
}
object!(MmioControl, CONTROL_RIGHTS);
object!(MmioUse, USE_RIGHTS);
impl MmioUse {
    pub(crate) fn node(&self) -> &Arc<MmioNode> {
        &self.0.node
    }
}
impl MmioControl {
    /// # Safety
    /// physical/bytes 已由真实平台确认是可委托 IO；不能包含 RAM、固件/内核私有页。
    /// 启动发行者已核根间不重叠，且每份平台区间只铸造一次根。
    pub(crate) unsafe fn root(
        pool: &Arc<MemoryPool>,
        physical: usize,
        bytes: usize,
        writable: bool,
    ) -> Result<Arc<Self>, SystemCallError> {
        // SAFETY: 同调用者的固定平台 IO admission 合同。
        let node = unsafe { MmioNode::root(pool, physical, bytes, writable) }?;
        Self::new(node, pool)
    }
}

fn node(object: &ObjectRef) -> Result<&Arc<MmioNode>, SystemCallError> {
    if let Some(control) = object.as_any().downcast_ref::<MmioControl>() {
        Ok(&control.0.node)
    } else if let Some(usage) = object.as_any().downcast_ref::<MmioUse>() {
        Ok(&usage.0.node)
    } else {
        Err(SystemCallError::WrongObjectType)
    }
}

pub(crate) fn create(
    thread: &Thread,
    parent: Handle,
    offset: usize,
    bytes: usize,
    rights: Rights,
    control: bool,
) -> Result<Pending, SystemCallError> {
    let (authority, (parent, admission)) =
        Authorization::accept_with(thread, parent, Rights::CREATE, |object, source_rights| {
            let parent = object
                .as_any()
                .downcast_ref::<MmioControl>()
                .ok_or(SystemCallError::WrongObjectType)?;
            if !rights.is_known() || !rights.is_subset_of(source_rights) {
                return Err(SystemCallError::RightsDenied);
            }
            Ok((parent.0.node.clone(), parent.0.node.begin()?))
        })?;
    let pool = thread.process.funding_pool();
    let child = parent.prepare_child(&pool, offset, bytes, rights.contains(Rights::WRITE))?;
    let object: ObjectRef = if control {
        MmioControl::new(child.node().clone(), &pool)?
    } else {
        MmioUse::new(child.node().clone(), &pool)?
    };
    let cap = handle::mint(object, rights)?;
    drop(authority);
    Pending::new(
        Create {
            parent,
            admission,
            child,
            cap: Some(cap),
        },
        &pool,
    )
}
struct Create {
    parent: Arc<MmioNode>,
    admission: resource_tree::Admission,
    child: PreparedMmioNode,
    cap: Option<Cap>,
}
impl Request for Create {
    fn advance(&mut self, thread: &Thread) -> Result<Step, SystemCallError> {
        if !self.parent.advance(&mut self.admission)? {
            return Ok(Step::Pending);
        }
        self.child.commit(&self.admission)?;
        // 实际节点先提交；安装失败归同一 capability 最后持有路径，不反向拆责任。
        let cap = self.cap.take().expect("MMIO child delivered twice");
        let handles = handle::install(thread, [cap], |_| Ok(()), || {})?;
        Ok(Step::Complete(handles[0].raw() as usize))
    }
}
impl Drop for Create {
    fn drop(&mut self) {
        if let Some(cap) = self.cap.take() {
            cap::deliver(cap.release());
        }
    }
}

pub(crate) fn seal(thread: &Thread, source: Handle) -> Result<(), SystemCallError> {
    let authority = Authorization::accept(thread, source, Rights::MANAGE)?;
    let control = authority
        .object()
        .as_any()
        .downcast_ref::<MmioControl>()
        .ok_or(SystemCallError::WrongObjectType)?;
    control.0.node.seal();
    Ok(())
}
pub(crate) fn revoke(thread: &Thread, source: Handle) -> Result<(), SystemCallError> {
    let authority = Authorization::accept(thread, source, Rights::MANAGE)?;
    node(authority.object())?.revoke();
    Ok(())
}
pub(crate) fn query(thread: &Thread, source: Handle, output: usize) -> Result<(), SystemCallError> {
    let authority = Authorization::accept(thread, source, Rights::READ)?;
    let node = node(authority.object())?;
    let status = node.status();
    let snapshot = MmioSnapshot {
        physical: node.physical as u64,
        bytes: node.bytes as u64,
        members: status.members as u64,
        flags: if status.sealed {
            MmioSnapshot::SEALED
        } else {
            0
        } | if status.revoking {
            MmioSnapshot::REVOKING
        } else {
            0
        } | if status.finished {
            MmioSnapshot::DONE
        } else {
            0
        },
        writable: u32::from(node.writable),
        reserved: [0; 2],
    };
    crate::uaccess::write_user(thread, output, &snapshot)
}
