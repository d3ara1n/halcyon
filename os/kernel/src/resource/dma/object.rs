//! DMA 的实际 control/use/mapping 角色及原线程分步请求。
use super::{
    mapping::{Mapping, PreparedMapping},
    *,
};
use crate::task::{
    Thread,
    cap::{self, Cap},
    handle::{self, Authorization},
    memory_object::{MemoryObject, MemoryObjectCore, map_object_error},
    object::{KernelObject, ObjectHeader, ObjectRef},
    request::{Pending, Request, Step},
    wait_source::ObservationSource,
};
use crate::vm::backing::BackingStorage;
use core::any::Any;
use erhino_shared::{
    dma::*,
    object::{Handle, ObjectKind, Rights},
};
use funded_storage::Buffer;

pub(crate) const DOMAIN_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
const MAPPING_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
struct Domain {
    header: ObjectHeader,
    holders: capability::Holders,
    node: Arc<Node>,
    _metadata: MetadataCharge,
}
impl Drop for Domain {
    fn drop(&mut self) {
        self.node.seal();
    }
}
pub(crate) struct DmaDomainControl(Domain);
pub(crate) struct DmaUse(Domain);
macro_rules! domain {
    ($ty:ident) => {
        impl $ty {
            fn new(node: Arc<Node>, pool: &Arc<MemoryPool>) -> Result<Arc<Self>, Error> {
                let metadata = MetadataCharge::arc::<Self>(pool)?;
                Arc::try_new(Self(Domain {
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
            fn allowed_rights(&self) -> Option<Rights> {
                Some(DOMAIN_RIGHTS)
            }
            fn allowed_signals(&self) -> Option<ObjectSignals> {
                Some(ObjectSignals::DONE)
            }
            fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
                Some(self.0.node.completion.clone())
            }
            fn on_last_capability(&self) {
                self.0.node.seal();
            }
        }
    };
}
domain!(DmaDomainControl);
domain!(DmaUse);
impl DmaDomainControl {
    /// # Safety
    /// 平台已核恒等物理 DMA、coherent、无 IOMMU；发行者只交给受信主控角色。
    pub(crate) unsafe fn root(pool: &Arc<MemoryPool>) -> Result<Arc<Self>, Error> {
        let node = Node::root(pool)?;
        let result = Self::new(node.clone(), pool);
        if result.is_err() {
            node.seal();
        }
        result
    }
}
struct DmaMapping {
    header: ObjectHeader,
    holders: capability::Holders,
    mapping: Arc<Mapping>,
    _metadata: MetadataCharge,
}
impl DmaMapping {
    fn new(mapping: Arc<Mapping>, pool: &Arc<MemoryPool>) -> Result<Arc<Self>, Error> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(Error::ReachLimit)?,
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            mapping,
            _metadata: metadata,
        })
        .map_err(|_| Error::OutOfMemory)
    }
}
impl KernelObject for DmaMapping {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::DmaMapping
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(MAPPING_RIGHTS)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::DONE)
    }
    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.mapping.completion.clone())
    }
    fn on_last_capability(&self) {
        self.mapping.stop();
    }
}
fn node(object: &ObjectRef) -> Result<(&Arc<Node>, bool), Error> {
    if let Some(control) = object.as_any().downcast_ref::<DmaDomainControl>() {
        Ok((&control.0.node, true))
    } else if let Some(usage) = object.as_any().downcast_ref::<DmaUse>() {
        Ok((&usage.0.node, false))
    } else {
        Err(Error::WrongObjectType)
    }
}
fn mapping(object: &ObjectRef) -> Result<&Arc<Mapping>, Error> {
    object
        .as_any()
        .downcast_ref::<DmaMapping>()
        .map(|object| &object.mapping)
        .ok_or(Error::WrongObjectType)
}
/// # Safety
/// T 是全部位型有效的定宽整数 ABI；读入之后逐字段校验。
unsafe fn input<T: Copy>(thread: &Thread, address: usize) -> Result<T, Error> {
    let body = thread.process.lease_body().ok_or(Error::ObjectClosed)?;
    let mut vm = body.vm.space.lock();
    let space = vm.space.as_mut().ok_or(Error::ObjectClosed)?;
    // SAFETY: 同 caller 合同；当前执行 owner 持空间锁并使用 translate+SUM。
    Ok(unsafe { crate::uaccess::read_user_value(space, address) }?)
}
pub(crate) fn create(
    thread: &Thread,
    source: Handle,
    rights: Rights,
    control: bool,
) -> Result<Pending, Error> {
    let (authority, (parent, admission)) =
        Authorization::accept_with(thread, source, Rights::CREATE, |object, held| {
            let parent = object
                .as_any()
                .downcast_ref::<DmaDomainControl>()
                .ok_or(Error::WrongObjectType)?;
            if !rights.is_known() || !rights.is_subset_of(held) {
                return Err(Error::RightsDenied);
            }
            Ok((parent.0.node.clone(), parent.0.node.begin()?))
        })?;
    let pool = thread.process.funding_pool();
    let child = parent.prepare(&pool)?;
    let object: ObjectRef = if control {
        DmaDomainControl::new(child.node.clone(), &pool)?
    } else {
        DmaUse::new(child.node.clone(), &pool)?
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
    admission: Admission,
    child: Prepared,
    cap: Option<Cap>,
}
impl Request for Create {
    fn advance(&mut self, thread: &Thread) -> Result<Step, Error> {
        if !self.parent.advance(&mut self.admission)? {
            return Ok(Step::Pending);
        }
        self.child.commit(&self.admission)?;
        let handles = handle::install(
            thread,
            [self.cap.take().expect("DMA child delivered twice")],
            |_| Ok(()),
            || {},
        )?;
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
pub(crate) fn seal(thread: &Thread, source: Handle) -> Result<(), Error> {
    let authority = Authorization::accept(thread, source, Rights::MANAGE)?;
    node(authority.object())?.0.seal();
    Ok(())
}
pub(crate) fn query_domain(thread: &Thread, source: Handle, output: usize) -> Result<(), Error> {
    let authority = Authorization::accept(thread, source, Rights::READ)?;
    let status = node(authority.object())?.0.status();
    let snapshot = DomainSnapshot {
        mode: TRUSTED_PHYSICAL,
        coherency: COHERENT,
        flags: if status.sealed { SEALED } else { 0 } | if status.finished { DONE } else { 0 },
        members: status.members as u64,
        reserved: 0,
    };
    crate::uaccess::write_user(thread, output, &snapshot)
}
pub(crate) fn query_mapping(thread: &Thread, source: Handle, output: usize) -> Result<(), Error> {
    let authority = Authorization::accept(thread, source, Rights::READ)?;
    crate::uaccess::write_user(thread, output, &mapping(authority.object())?.snapshot())
}
fn direction_rights(direction: u32) -> Result<Rights, Error> {
    match direction {
        DEVICE_READS => Ok(Rights::READ),
        DEVICE_WRITES => Ok(Rights::WRITE),
        BIDIRECTIONAL => Ok(Rights::READ | Rights::WRITE),
        _ => Err(Error::IllegalArgument),
    }
}
pub(crate) fn pin(thread: &Thread, source: Handle, address: usize) -> Result<Pending, Error> {
    // SAFETY: PinRequest 仅整数，没有 padding/无效位型。
    let request: PinRequest = unsafe { input(thread, address) }?;
    let required = direction_rights(request.direction)?;
    if request.reserved != 0
        || request.bytes == 0
        || !request.bytes.is_multiple_of(memory_space::PAGE_SIZE as u64)
        || !request
            .offset
            .is_multiple_of(memory_space::PAGE_SIZE as u64)
        || request.offset.checked_add(request.bytes).is_none()
    {
        return Err(Error::IllegalArgument);
    }
    let (authority, (owner, admission)) =
        Authorization::accept_with(thread, source, Rights::CREATE | required, |object, held| {
            let usage = object
                .as_any()
                .downcast_ref::<DmaUse>()
                .ok_or(Error::WrongObjectType)?;
            if !request.rights.is_known() || !request.rights.is_subset_of(held) {
                return Err(Error::RightsDenied);
            }
            Ok((usage.0.node.clone(), usage.0.node.begin()?))
        })?;
    let memory = Authorization::accept(thread, request.memory, required)?;
    let core = memory
        .object()
        .as_any()
        .downcast_ref::<MemoryObject>()
        .ok_or(Error::WrongObjectType)?
        .core()
        .clone();
    let pool = thread.process.funding_pool();
    let storage = core.prepare_use(&pool)?;
    let permits = Buffer::try_empty(
        usize::from(required.contains(Rights::WRITE)),
        &MetadataFunding(&pool),
    )
    .map_err(map_storage_error)?;
    let prepared = PreparedMapping::new(
        owner,
        &pool,
        request.offset as usize,
        request.bytes as usize,
        request.direction,
    )?;
    let cap = handle::mint(
        DmaMapping::new(prepared.mapping.clone(), &pool)?,
        request.rights,
    )?;
    drop(memory);
    drop(authority);
    Pending::new(
        Pin {
            request,
            core,
            storage,
            permits,
            prepared,
            admission,
            registered: false,
            cap: Some(cap),
        },
        &pool,
    )
}
struct Pin {
    request: PinRequest,
    core: Arc<MemoryObjectCore>,
    storage: BackingStorage,
    permits: Buffer<memory_space::WritePermit, MetadataCharge>,
    prepared: PreparedMapping,
    admission: Admission,
    registered: bool,
    cap: Option<Cap>,
}
impl Request for Pin {
    fn advance(&mut self, thread: &Thread) -> Result<Step, Error> {
        if !self.registered {
            if !self.prepared.mapping.owner.advance(&mut self.admission)? {
                return Ok(Step::Pending);
            }
            self.prepared.commit(&self.admission)?;
            self.registered = true;
            self.acquire(thread)?;
            return Ok(Step::Pending);
        }
        let mapping = &self.prepared.mapping;
        if mapping.stopped() {
            return Err(Error::ObjectClosed);
        }
        let mut owned = mapping
            .owned
            .lock()
            .take()
            .expect("DMA preparation lost owner");
        let result = owned
            .acquisition
            .as_mut()
            .expect("DMA preparation lost cursor")
            .step();
        if result == Ok(true) {
            owned.usage = owned
                .acquisition
                .take()
                .expect("DMA cursor missing")
                .finish();
        }
        *mapping.owned.lock() = Some(owned);
        if !result? {
            return Ok(Step::Pending);
        }
        if !mapping.prepared(true) {
            return Err(Error::ObjectClosed);
        }
        let handles = handle::install(
            thread,
            [self.cap.take().expect("DMA mapping delivered twice")],
            |_| Ok(()),
            || {},
        )?;
        Ok(Step::Complete(handles[0].raw() as usize))
    }
}
impl Pin {
    fn acquire(&mut self, thread: &Thread) -> Result<(), Error> {
        let required = direction_rights(self.request.direction)?;
        let (authority, acquisition) =
            Authorization::accept_with(thread, self.request.memory, required, |object, _| {
                let memory = object
                    .as_any()
                    .downcast_ref::<MemoryObject>()
                    .ok_or(Error::WrongObjectType)?;
                if !Arc::ptr_eq(memory.core(), &self.core) {
                    return Err(Error::WrongObjectType);
                }
                self.core
                    .begin_pin(
                        self.request.offset as usize,
                        self.request.bytes as usize,
                        &mut self.storage,
                    )?
                    .ok_or(Error::ObjectBusy)
            })?;
        drop(authority);
        self.prepared
            .mapping
            .owned
            .lock()
            .as_mut()
            .expect("DMA preparation lost owner")
            .acquisition = Some(acquisition);
        if required.contains(Rights::WRITE) {
            self.core
                .reserve_dma_write(&mut self.permits)
                .map_err(map_object_error)?;
            self.prepared
                .mapping
                .owned
                .lock()
                .as_mut()
                .expect("DMA preparation lost owner")
                .write = Some((
                self.core.clone(),
                self.permits.pop().expect("DMA write permit missing"),
            ));
        }
        Ok(())
    }
}
impl Drop for Pin {
    fn drop(&mut self) {
        self.prepared.mapping.abandon_preparation();
        if let Some(cap) = self.cap.take() {
            cap::deliver(cap.release());
        }
    }
}
pub(crate) fn read_segments(
    thread: &Thread,
    source: Handle,
    address: usize,
) -> Result<Pending, Error> {
    // SAFETY: ReadRequest 只有定宽整数。
    let request: ReadRequest = unsafe { input(thread, address) }?;
    if request.reserved != 0 || request.capacity == 0 {
        return Err(Error::IllegalArgument);
    }
    let authority = Authorization::accept(thread, source, Rights::READ)?;
    let mapping = mapping(authority.object())?.clone();
    if request.offset >= mapping.bytes as u64 {
        return Err(Error::IllegalArgument);
    }
    let admission = mapping.owner.begin()?;
    let pool = thread.process.funding_pool();
    let count = (request.capacity as usize).min(SEGMENTS_PER_READ);
    let mut output =
        Buffer::try_empty(count, &MetadataFunding(&pool)).map_err(map_storage_error)?;
    for _ in 0..count {
        output.push(Segment::default());
    }
    drop(authority);
    Pending::new(
        Read {
            mapping,
            admission,
            request,
            output,
        },
        &pool,
    )
}
struct Read {
    mapping: Arc<Mapping>,
    admission: Admission,
    request: ReadRequest,
    output: Buffer<Segment, MetadataCharge>,
}
impl Request for Read {
    fn advance(&mut self, thread: &Thread) -> Result<Step, Error> {
        if !self.mapping.owner.advance(&mut self.admission)? {
            return Ok(Step::Pending);
        }
        let _exposure = self.mapping.expose(&self.admission)?;
        let next = self
            .mapping
            .segments(self.request.offset as usize, &mut self.output);
        let count = self
            .output
            .iter()
            .take_while(|segment| segment.bytes != 0)
            .count();
        let body = thread.process.lease_body().ok_or(Error::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(Error::ObjectClosed)?;
        // SAFETY: Segment 是无 padding 的两整数，已初始化前缀；输出失败仍保留 MayBeExposed。
        let bytes = unsafe {
            core::slice::from_raw_parts(
                self.output.as_ptr().cast::<u8>(),
                count * core::mem::size_of::<Segment>(),
            )
        };
        crate::uaccess::deliver_bytes(thread, space, self.request.segments as usize, bytes)?;
        let result = ReadResult {
            count: count as u64,
            next_offset: next as u64,
        };
        // SAFETY: ReadResult 无 padding；同一 VM 锁与原执行 owner 写回。
        unsafe {
            crate::uaccess::deliver_output(thread, space, self.request.result as usize, &result)
        }?;
        Ok(Step::Complete(0))
    }
}
pub(crate) fn retire(thread: &Thread, source: Handle, target: Handle) -> Result<Pending, Error> {
    let authority = Authorization::accept(thread, source, Rights::MANAGE)?;
    let (owner, control) = node(authority.object())?;
    let owner = owner.clone();
    let target_authority = Authorization::accept(thread, target, Rights::READ)?;
    let mapping = mapping(target_authority.object())?.clone();
    if !Arc::ptr_eq(&owner.root, &mapping.owner.root) || (!control && owner.id != mapping.owner.id)
    {
        return Err(Error::RightsDenied);
    }
    let cursor = Some(mapping.owner.id);
    drop(target_authority);
    drop(authority);
    Pending::new(
        Retire {
            owner,
            mapping,
            cursor,
        },
        &thread.process.funding_pool(),
    )
}
struct Retire {
    owner: Arc<Node>,
    mapping: Arc<Mapping>,
    cursor: Option<NodeId>,
}
impl Request for Retire {
    fn advance(&mut self, _: &Thread) -> Result<Step, Error> {
        let current = self.cursor.ok_or(Error::RightsDenied)?;
        if current == self.owner.id {
            self.mapping.retire()?;
            return Ok(Step::Complete(0));
        }
        self.cursor = self
            .owner
            .root
            .state
            .lock()
            .tree
            .status(current)
            .map_err(tree_error)?
            .parent;
        Ok(Step::Pending)
    }
}
