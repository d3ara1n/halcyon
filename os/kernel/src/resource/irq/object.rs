//! IRQ 的控制/使用角色；Take 返回序号，代次由固定快照取得，Ack 验证完整 token。
use super::{Node, Prepared};
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
    call::SystemCallError as Error,
    irq::{IrqScope, IrqSnapshot, IrqToken},
    object::{Handle, ObjectKind, ObjectSignals, Rights},
};

pub(crate) const USE_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
pub(crate) const CONTROL_RIGHTS: Rights = Rights::from_raw(USE_RIGHTS.raw() | Rights::CREATE.raw());
struct Object {
    header: ObjectHeader,
    holders: capability::Holders,
    node: Arc<Node>,
    _metadata: MetadataCharge,
}
pub(crate) struct IrqControl(Object);
pub(crate) struct IrqUse(Object);
macro_rules! object {
    ($ty:ident, $rights:ident) => {
        impl $ty {
            fn new(node: Arc<Node>, pool: &Arc<MemoryPool>) -> Result<Arc<Self>, Error> {
                let metadata = MetadataCharge::arc::<Self>(pool)?;
                Arc::try_new(Self(Object {
                    header: ObjectHeader::try_new().ok_or(Error::ReachLimit)?,
                    holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
                    node,
                    _metadata: metadata,
                }))
                .map_err(|_| Error::OutOfMemory)
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
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn on_last_capability(&self) {
                self.0.node.revoke();
            }
            fn allowed_rights(&self) -> Option<Rights> {
                Some($rights)
            }
            fn allowed_signals(&self) -> Option<ObjectSignals> {
                Some(if self.0.node.line.is_some() {
                    ObjectSignals::READABLE | ObjectSignals::DONE | ObjectSignals::CLOSED
                } else {
                    ObjectSignals::DONE
                })
            }
            fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
                Some(match &self.0.node.line {
                    Some(line) => line.clone(),
                    None => self
                        .0
                        .node
                        .completion
                        .as_ref()
                        .expect("IRQ control lost observation source")
                        .clone(),
                })
            }
        }
    };
}
object!(IrqControl, CONTROL_RIGHTS);
object!(IrqUse, USE_RIGHTS);
impl IrqControl {
    /// # Safety
    /// scope 来自唯一平台准入，sources 不与其他控制根重叠；只铸造一次。
    pub(crate) unsafe fn root(pool: &Arc<MemoryPool>, scope: IrqScope) -> Result<Arc<Self>, Error> {
        Self::new(Node::root(pool, scope)?, pool)
    }
}
fn node(object: &ObjectRef) -> Result<&Arc<Node>, Error> {
    if let Some(control) = object.as_any().downcast_ref::<IrqControl>() {
        Ok(&control.0.node)
    } else if let Some(usage) = object.as_any().downcast_ref::<IrqUse>() {
        Ok(&usage.0.node)
    } else {
        Err(Error::WrongObjectType)
    }
}
pub(crate) fn create(
    thread: &Thread,
    handle: Handle,
    scope_ptr: usize,
    rights: Rights,
    control: bool,
) -> Result<Pending, Error> {
    let scope: IrqScope = {
        let body = thread.process.lease_body().ok_or(Error::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(Error::ObjectClosed)?;
        // SAFETY: 固定整数布局，全部位型有效，随后检查范围与目标。
        unsafe { crate::uaccess::read_user_value(space, scope_ptr) }?
    };
    let (authority, (parent, admission)) =
        Authorization::accept_with(thread, handle, Rights::CREATE, |object, held| {
            let parent = object
                .as_any()
                .downcast_ref::<IrqControl>()
                .ok_or(Error::WrongObjectType)?;
            if !rights.is_known() || !rights.is_subset_of(held) {
                return Err(Error::RightsDenied);
            }
            Ok((parent.0.node.clone(), parent.0.node.begin()?))
        })?;
    let pool = thread.process.funding_pool();
    let child = parent.prepare(&pool, scope, control)?;
    let object: ObjectRef = if control {
        IrqControl::new(child.node.clone(), &pool)?
    } else {
        IrqUse::new(child.node.clone(), &pool)?
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
    parent: Arc<Node>,
    admission: resource_tree::Admission,
    child: Prepared,
    cap: Option<Cap>,
}
impl Request for Create {
    fn advance(&mut self, thread: &Thread) -> Result<Step, Error> {
        if !self.parent.advance(&mut self.admission)? {
            return Ok(Step::Pending);
        }
        self.child.commit(&self.admission)?;
        // 链接先于硬件登记。先行撤销会关闭 Line，即使候选尚未 activate。
        if let Some(line) = &self.child.node.line {
            line.activate()?;
        }
        let cap = self.cap.take().expect("IRQ child delivered twice");
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
pub(crate) fn seal(thread: &Thread, handle: Handle) -> Result<(), Error> {
    let authority = Authorization::accept(thread, handle, Rights::MANAGE)?;
    let control = authority
        .object()
        .as_any()
        .downcast_ref::<IrqControl>()
        .ok_or(Error::WrongObjectType)?;
    control.0.node.seal();
    Ok(())
}
pub(crate) fn revoke(thread: &Thread, handle: Handle) -> Result<(), Error> {
    let authority = Authorization::accept(thread, handle, Rights::MANAGE)?;
    node(authority.object())?.revoke();
    Ok(())
}
pub(crate) fn query(thread: &Thread, handle: Handle, output: usize) -> Result<(), Error> {
    let authority = Authorization::accept(thread, handle, Rights::READ)?;
    let node = node(authority.object())?;
    let status = node.status();
    let snapshot = IrqSnapshot {
        scope: node.scope,
        flags: if status.sealed {
            IrqSnapshot::SEALED
        } else {
            0
        } | if status.revoking {
            IrqSnapshot::REVOKING
        } else {
            0
        } | if status.finished {
            IrqSnapshot::DONE
        } else {
            0
        },
        generation: node.line.as_ref().map_or(0, |line| line.generation()),
    };
    crate::uaccess::write_user(thread, output, &snapshot)
}
pub(crate) fn take(thread: &Thread, handle: Handle) -> Result<usize, Error> {
    let authority = Authorization::accept(thread, handle, Rights::READ | Rights::WRITE)?;
    let usage = authority
        .object()
        .as_any()
        .downcast_ref::<IrqUse>()
        .ok_or(Error::WrongObjectType)?;
    usage
        .0
        .node
        .line
        .as_ref()
        .expect("IRQ use lost line")
        .take()
        .map(|token| token.sequence as usize)
}
pub(crate) fn ack(thread: &Thread, handle: Handle, token: IrqToken) -> Result<(), Error> {
    let authority = Authorization::accept(thread, handle, Rights::WRITE)?;
    let usage = authority
        .object()
        .as_any()
        .downcast_ref::<IrqUse>()
        .ok_or(Error::WrongObjectType)?;
    usage
        .0
        .node
        .line
        .as_ref()
        .expect("IRQ use lost line")
        .ack(token)
}
