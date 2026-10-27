//! 单 actor 设备登记 backing；上下文、范围、父关系和付款在这里共同复核。
//!
//! 调用者必须使用内核认证的 sender context，并在发行前安装实际 Lifetime/任务。
//! 本模块不接收 payload 中自报的 authority，也不替运行体完成硬件清理。

use alloc::{string::String, vec::Vec};
use core::num::NonZeroU64;
use libbudget::{AccountView, Charge};
use ordered_table::{OrderedTable, PreparedEntry};

use crate::{
    Error,
    binding::{self, Seat, Subject},
    funding::Funding,
    identity::{BindingRef, DeviceId, Ids},
    publication::{PreparedPublication, Presence, Publications},
    resource::{DeviceResource, Limits},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access(u32);
impl Access {
    pub const OBSERVE: Self = Self(1);
    pub const FUNCTIONS: Self = Self(2);
    pub const REGISTER: Self = Self(4);
    pub const CLAIM: Self = Self(8);
    pub const ADMIN: Self = Self(16);
    pub const REPORT_STOPPED: Self = Self(32);
    pub const CHILD_REGISTER: Self = Self(64);
    pub const ALL: Self = Self(127);
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    pub fn from_raw(raw: u32) -> Result<Self, Error> {
        if raw == 0 || raw & !Self::ALL.0 != 0 {
            Err(Error::Invalid)
        } else {
            Ok(Self(raw))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Root,
    Subtree(DeviceId),
}

/// 同一登记/来源关系的只读停止投影，不保存另一份来源阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceStop {
    Open,
    Stopping,
    Closed,
}

/// 来源控制与申请角色是不同的认证 context；复制一个 sender 不创建新角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceIds {
    pub use_context: u64,
    pub admin_context: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointIdentity {
    pub object: NonZeroU64,
    pub mailbox: NonZeroU64,
}
impl EndpointIdentity {
    pub fn from_description(d: &erhino_shared::object::HandleDescription) -> Result<Self, Error> {
        use erhino_shared::object::{ObjectKind, Rights};
        if d.kind != ObjectKind::MailboxSender as u32
            || !d
                .rights
                .contains(Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT)
        {
            return Err(Error::Permission);
        }
        Ok(Self {
            object: NonZeroU64::new(d.object_id).ok_or(Error::Invalid)?,
            mailbox: NonZeroU64::new(d.related_object_id).ok_or(Error::Invalid)?,
        })
    }
}

#[derive(Clone, Copy)]
pub struct Config {
    pub sources: usize,
    pub devices: usize,
    pub watches: usize,
    pub functions: usize,
}

struct Paid<T> {
    value: T,
    _bytes: Charge,
}
fn prepare<T>(
    table: &OrderedTable<Paid<T>, ()>,
    key: u64,
    value: T,
    account: &AccountView<DeviceResource>,
) -> Result<PreparedEntry<Paid<T>, ()>, Error> {
    let bytes = PreparedEntry::<Paid<T>, ()>::allocation_bytes();
    let charge = account.acquire(DeviceResource::Bytes, bytes)?;
    table
        .prepare_insert_candidate(
            key,
            Paid {
                value,
                _bytes: charge,
            },
            &libbudget::AllocatorBacked,
        )
        .map_err(|e| match e {
            ordered_table::InsertError::Limit(_) => Error::Quota,
            ordered_table::InsertError::Storage(e, _) => libbudget::allocator_error(e).into(),
        })
}

struct Source {
    ids: SourceIds,
    parent: Option<u64>,
    binding: Option<BindingRef>,
    scope: Scope,
    access: Access,
    funding: Funding,
    publications: Publications<DeviceId>,
    task: NonZeroU64,
    sealed: bool,
    use_closed: bool,
    admin_closed: bool,
    children: usize,
    // 本主体的 Binding/在途任务在生命周期结束前持一份引用。
    retained: usize,
    _unit: Charge,
}

struct Device<C> {
    id: DeviceId,
    source: u64,
    publication: u64,
    parent: Option<DeviceId>,
    name: String,
    identity: Vec<u8>,
    limits: Limits,
    stop_requirement: binding::StopRequirement,
    endpoint: EndpointIdentity,
    provision: C,
    seat: Option<Seat>,
    task: NonZeroU64,
    binding_task: Option<NonZeroU64>,
    activation: Option<functions::Activation<C>>,
    control_closed: bool,
    children: usize,
    retained: usize,
    _text_bytes: Charge,
    _extra_text_bytes: Option<Charge>,
}

pub struct Registration<'a> {
    pub seq: u64,
    pub parent: Option<DeviceId>,
    pub name: &'a str,
    pub identity: &'a [u8],
    pub limits: Limits,
    pub claimable: bool,
    pub stop_requirement: binding::StopRequirement,
    pub endpoint: EndpointIdentity,
}

pub struct PreparedDevice<C> {
    source: u64,
    publication: PreparedPublication<DeviceId>,
    node: PreparedEntry<Paid<Device<C>>, ()>,
}
impl<C> PreparedDevice<C> {
    pub fn id(&self) -> DeviceId {
        self.node.value().value.id
    }
}
pub struct RegisterFailure<C> {
    pub error: Error,
    pub prepared: PreparedDevice<C>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterCommit {
    Created(DeviceId),
    Existing(DeviceId),
}

pub struct DeviceInfo<'a> {
    pub id: DeviceId,
    pub parent: Option<DeviceId>,
    pub name: &'a str,
    pub identity: &'a [u8],
    pub limits: Limits,
    pub present: bool,
    pub epoch: Option<u64>,
    pub binding: Option<binding::Snapshot>,
}

/// 真实来源和设备表。所有祖先遍历均迭代执行，深度受已准入记录数界定；
/// 删除须等待引用计数归零，旧权限不会因父节点先删而丢失其不可变身份。
pub struct Registry<C> {
    ids: Ids,
    incarnation: u64,
    config: Config,
    root: Option<u64>,
    sources: OrderedTable<Paid<Source>, ()>,
    admins: OrderedTable<Paid<u64>, ()>,
    devices: OrderedTable<Paid<Device<C>>, ()>,
    watches: OrderedTable<Paid<discovery::Watch>, ()>,
    discovery_dirty: bool,
    functions: OrderedTable<Paid<functions::Function<C>>, ()>,
    next_function: u64,
}

mod discovery;
pub use discovery::{Scan, WatchTarget};
mod functions;
pub use functions::{FunctionInfo, FunctionRegistration};
pub mod oal;

impl<C> Registry<C> {
    pub fn new(mailbox_koid: u64, config: Config) -> Result<Self, Error> {
        if config.sources == 0 || config.devices == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            ids: Ids::new(mailbox_koid)?,
            incarnation: mailbox_koid,
            config,
            root: None,
            sources: OrderedTable::new(config.sources),
            admins: OrderedTable::new(config.sources),
            devices: OrderedTable::new(config.devices),
            watches: OrderedTable::new(config.watches),
            discovery_dirty: false,
            functions: OrderedTable::new(config.functions),
            next_function: 0,
        })
    }

    /// 仅进程装配使用一次；两个 context 均来自本 mailbox 的实际 MintedSender。
    /// task 是发行前已准入、持这两个 Lifetime 的运行体任务，不是客户端给的数字。
    pub fn bootstrap(&mut self, ids: SourceIds, task: u64, funding: Funding) -> Result<(), Error> {
        if self.root.is_some() {
            return Err(Error::Conflict);
        }
        self.install_source(None, ids, task, Scope::Root, Access::ALL, funding)?;
        self.root = Some(ids.use_context);
        Ok(())
    }

    pub fn derive_source(
        &mut self,
        issuer: u64,
        ids: SourceIds,
        task: u64,
        scope: Scope,
        access: Access,
    ) -> Result<(), Error> {
        self.check_derive_source(issuer, scope, access)?;
        let funding = self.source(issuer)?.funding.clone();
        self.install_source(Some((issuer, None)), ids, task, scope, access, funding)
    }
    /// 发行前的廉价权限检查；最终 derive_source 仍会在提交门重复复核。
    pub fn check_derive_source(
        &self,
        issuer: u64,
        scope: Scope,
        access: Access,
    ) -> Result<(), Error> {
        let source = self.source(issuer)?;
        if !source.access.contains(Access::ADMIN) || !source.access.contains(access) {
            return Err(Error::Permission);
        }
        self.source_open(issuer)?;
        if !self.scope_contains(source.scope, scope)? {
            return Err(Error::Permission);
        }
        if let Scope::Subtree(id) = scope {
            self.device_open(id)?;
        }
        Ok(())
    }

    fn install_source(
        &mut self,
        origin: Option<(u64, Option<BindingRef>)>,
        ids: SourceIds,
        task: u64,
        scope: Scope,
        access: Access,
        funding: Funding,
    ) -> Result<(), Error> {
        let parent = origin.map(|(parent, _)| parent);
        let binding = origin.and_then(|(_, binding)| binding);
        let task = NonZeroU64::new(task).ok_or(Error::Invalid)?;
        if ids.use_context == 0
            || ids.admin_context == 0
            || ids.use_context == ids.admin_context
            || self.context_exists(ids.use_context)
            || self.context_exists(ids.admin_context)
        {
            return Err(Error::Conflict);
        }
        if let Some(parent) = parent {
            self.source(parent)?
                .children
                .checked_add(1)
                .ok_or(Error::Quota)?;
        }
        if let Scope::Subtree(id) = scope {
            self.device(id)?
                .retained
                .checked_add(1)
                .ok_or(Error::Quota)?;
        }
        let account = funding.domain().clone();
        let unit = account.acquire(DeviceResource::Source, 1)?;
        let source = Source {
            ids,
            parent,
            binding,
            scope,
            access,
            funding,
            publications: Publications::new(ids.use_context, self.config.devices)?,
            task,
            sealed: false,
            use_closed: false,
            admin_closed: false,
            children: 0,
            retained: 0,
            _unit: unit,
        };
        let source = prepare(&self.sources, ids.use_context, source, &account)?;
        let admin = prepare(&self.admins, ids.admin_context, ids.use_context, &account)?;
        self.sources.insert_prepared(source);
        self.admins.insert_prepared(admin);
        if let Some(parent) = parent {
            self.source_mut(parent)?.children += 1;
        }
        if let Scope::Subtree(id) = scope {
            self.device_mut(id)?.retained += 1;
        }
        Ok(())
    }

    fn context_exists(&self, context: u64) -> bool {
        self.sources.contains_key(context) || self.admins.contains_key(context)
    }
    fn source(&self, context: u64) -> Result<&Source, Error> {
        self.sources
            .get(context)
            .map(|s| &s.value)
            .ok_or(Error::Permission)
    }
    fn source_mut(&mut self, context: u64) -> Result<&mut Source, Error> {
        self.sources
            .get_mut(context)
            .map(|s| &mut s.value)
            .ok_or(Error::Permission)
    }
    fn device(&self, id: DeviceId) -> Result<&Device<C>, Error> {
        if id.incarnation() != self.incarnation {
            return Err(Error::StaleInstance);
        }
        self.devices
            .get(id.serial())
            .map(|d| &d.value)
            .ok_or(Error::StaleInstance)
    }
    fn device_mut(&mut self, id: DeviceId) -> Result<&mut Device<C>, Error> {
        if id.incarnation() != self.incarnation {
            return Err(Error::StaleInstance);
        }
        self.devices
            .get_mut(id.serial())
            .map(|d| &mut d.value)
            .ok_or(Error::StaleInstance)
    }
    fn source_open(&self, context: u64) -> Result<(), Error> {
        let mut current = Some(context);
        while let Some(context) = current {
            let source = self.source(context)?;
            if source.sealed {
                return Err(Error::Closed);
            }
            if let Some(reference) = source.binding
                && !self
                    .binding_snapshot(reference)
                    .is_ok_and(|s| s.phase == binding::Phase::ResourcesReady)
            {
                return Err(Error::Closed);
            }
            // scope 存在性由 device_open 的使用点检查，避免 source→device→source 递归。
            current = source.parent;
        }
        Ok(())
    }
    fn device_open(&self, id: DeviceId) -> Result<(), Error> {
        let mut current = Some(id);
        while let Some(id) = current {
            let device = self.device(id)?;
            let source = self.source(device.source)?;
            self.source_open(device.source)?;
            if source.publications.get(device.publication)?.presence() != Presence::Present {
                return Err(Error::Closed);
            }
            current = device.parent;
        }
        Ok(())
    }
    /// 有效撤出不可逆。会推进 epoch 的操作必须先同步，不能依赖维护扫描时序。
    fn synchronize_withdrawal(&mut self, id: DeviceId) -> Result<bool, Error> {
        match self.device_open(id) {
            Ok(()) => Ok(false),
            Err(Error::Closed) => {
                let device = self.device(id)?;
                let source = device.source;
                let publication = device.publication;
                let changed = self
                    .source(source)?
                    .publications
                    .get(publication)?
                    .presence()
                    == Presence::Present;
                self.source_mut(source)?
                    .publications
                    .withdraw(publication)?;
                if let Some(seat) = self.device_mut(id)?.seat.as_mut() {
                    seat.withdraw();
                }
                if changed {
                    self.changed(id);
                }
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }
    pub fn is_descendant(&self, mut child: DeviceId, ancestor: DeviceId) -> Result<bool, Error> {
        self.device(ancestor)?;
        loop {
            let node = self.device(child)?;
            if child == ancestor {
                return Ok(true);
            }
            match node.parent {
                Some(parent) => child = parent,
                None => return Ok(false),
            }
        }
    }
    fn scope_contains(&self, outer: Scope, inner: Scope) -> Result<bool, Error> {
        match (outer, inner) {
            (Scope::Root, Scope::Root) => Ok(true),
            (Scope::Root, Scope::Subtree(id)) => {
                self.device(id)?;
                Ok(true)
            }
            (Scope::Subtree(_), Scope::Root) => Ok(false),
            (Scope::Subtree(parent), Scope::Subtree(child)) => self.is_descendant(child, parent),
        }
    }
    fn authorize(
        &self,
        context: u64,
        required: Access,
        target: Scope,
        opening: bool,
    ) -> Result<(), Error> {
        let source = self.source(context)?;
        if !source.access.contains(required) || !self.scope_contains(source.scope, target)? {
            return Err(Error::Permission);
        }
        if opening {
            self.source_open(context)?;
            if let Scope::Subtree(id) = source.scope {
                self.device_open(id)?;
            }
            if let Scope::Subtree(id) = target {
                self.device_open(id)?;
            }
        }
        Ok(())
    }

    pub fn funding(&self, context: u64) -> Result<&Funding, Error> {
        let context = self.admins.get(context).map_or(context, |a| a.value);
        Ok(&self.source(context)?.funding)
    }
    pub fn source_access(&self, context: u64) -> Result<Access, Error> {
        Ok(self.source(context)?.access)
    }
    pub fn is_source_admin(&self, context: u64) -> bool {
        self.admins.contains_key(context)
    }

    fn source_descends_from(&self, mut context: u64, ancestor: u64) -> Result<bool, Error> {
        loop {
            let source = self.source(context)?;
            if context == ancestor {
                return Ok(true);
            }
            match source.parent {
                Some(parent) => context = parent,
                None => return Ok(false),
            }
        }
    }
    fn source_depends_on_binding(
        &self,
        mut context: u64,
        reference: BindingRef,
    ) -> Result<bool, Error> {
        loop {
            let source = self.source(context)?;
            if source.binding == Some(reference) {
                return Ok(true);
            }
            match source.parent {
                Some(parent) => context = parent,
                None => return Ok(false),
            }
        }
    }
    // 两个停止投影都消费同一来源集合：申请来源，以及设备和设备祖先的登记来源。
    fn binding_sources_any(
        &self,
        binding: binding::Snapshot,
        predicate: impl Fn(u64) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        if predicate(binding.subject.context())? {
            return Ok(true);
        }
        let mut current = Some(binding.reference.device());
        while let Some(id) = current {
            let device = self.device(id)?;
            if predicate(device.source)? {
                return Ok(true);
            }
            current = device.parent;
        }
        Ok(false)
    }
    /// 只依据原 Source.binding 的精确轮次关系；拓扑下的独立静态绑定不构成依赖。
    pub fn binding_dependents_released(&self, reference: BindingRef) -> Result<bool, Error> {
        let mut cursor = None;
        while let Some((serial, node)) = self.devices.next_after(cursor.as_ref()) {
            cursor = Some(*serial);
            let Some(binding) = node.value.seat.as_ref().and_then(Seat::active) else {
                continue;
            };
            if binding.reference != reference
                && self.binding_sources_any(binding, |context| {
                    self.source_depends_on_binding(context, reference)
                })?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// 与 seal/maintenance 相同的影响集合；只观察实际 Seat，不 pump 清理或等待角色退款。
    pub fn query_source_stop(&self, admin_context: u64) -> Result<SourceStop, Error> {
        let target = self
            .admins
            .get(admin_context)
            .ok_or(Error::Permission)?
            .value;
        let mut current = Some(target);
        let mut sealed = false;
        while let Some(context) = current {
            let source = self.source(context)?;
            sealed |= source.sealed;
            current = source.parent;
        }
        if !sealed {
            return Ok(SourceStop::Open);
        }
        let mut cursor = None;
        while let Some((serial, node)) = self.devices.next_after(cursor.as_ref()) {
            cursor = Some(*serial);
            let Some(binding) = node.value.seat.as_ref().and_then(Seat::active) else {
                continue;
            };
            if self.binding_sources_any(binding, |context| {
                self.source_descends_from(context, target)
            })? {
                return Ok(SourceStop::Stopping);
            }
        }
        Ok(SourceStop::Closed)
    }

    /// 必须经实际认证的来源管理入口；封口后仍允许重复停止和观察。
    pub fn seal_source(&mut self, admin_context: u64) -> Result<u64, Error> {
        let target = self
            .admins
            .get(admin_context)
            .ok_or(Error::Permission)?
            .value;
        let source = self.source_mut(target)?;
        let changed = !source.sealed;
        source.sealed = true;
        source.publications.seal();
        let task = source.task.get();
        if changed {
            self.sources_changed();
        }
        Ok(task)
    }

    /// 只有实际 Lifetime CLOSED 才调用；不是每次 Close(sender 副本)。
    pub fn role_closed(&mut self, context: u64) -> Result<u64, Error> {
        let (target, admin) = match self.admins.get(context) {
            Some(a) => (a.value, true),
            None => (context, false),
        };
        let source = self.source_mut(target)?;
        let changed = !source.sealed && !admin;
        if admin {
            source.admin_closed = true;
        } else {
            source.use_closed = true;
        }
        // 最后申请角色消散已无法恢复申请者，仍由原任务接管已提交责任。
        if !admin {
            source.sealed = true;
            source.publications.seal();
        }
        let task = source.task.get();
        if changed {
            self.sources_changed();
        }
        Ok(task)
    }

    fn name_available(&self, parent: Option<DeviceId>, name: &str) -> Result<(), Error> {
        let mut cursor = None;
        while let Some((key, node)) = self.devices.next_after(cursor.as_ref()) {
            let node = &node.value;
            if node.parent == parent && node.name == name && self.device_open(node.id).is_ok() {
                return Err(Error::Conflict);
            }
            cursor = Some(*key);
        }
        Ok(())
    }
    fn check_registration(&self, context: u64, request: &Registration<'_>) -> Result<(), Error> {
        self.authorize(
            context,
            Access::REGISTER,
            request.parent.map_or(Scope::Root, Scope::Subtree),
            true,
        )?;
        liboal::path::validate_name(request.name).map_err(|_| Error::Invalid)?;
        if request.name.len() > oal::MAX_DISCOVERY_NAME {
            return Err(Error::Invalid);
        }
        self.name_available(request.parent, request.name)
    }
    pub fn existing_registration(
        &self,
        context: u64,
        request: &Registration<'_>,
    ) -> Result<Option<DeviceId>, Error> {
        let source = self.source(context)?;
        if !source.access.contains(Access::REGISTER) {
            return Err(Error::Permission);
        }
        let Some(id) = source.publications.existing(request.seq)? else {
            return Ok(None);
        };
        self.device_open(*id)?;
        let d = self.device(*id)?;
        if d.parent != request.parent
            || d.name != request.name
            || d.identity != request.identity
            || d.limits != request.limits
            || d.seat.is_some() != request.claimable
            || d.stop_requirement != request.stop_requirement
            || d.endpoint != request.endpoint
        {
            return Err(Error::Conflict);
        }
        Ok(Some(*id))
    }

    pub fn prepare_registration(
        &mut self,
        context: u64,
        request: &Registration<'_>,
        provision: C,
        task: u64,
    ) -> Result<PreparedDevice<C>, Error> {
        let task = NonZeroU64::new(task).ok_or(Error::Invalid)?;
        if self.existing_registration(context, request)?.is_some() {
            return Err(Error::Conflict);
        }
        self.check_registration(context, request)?;
        if let Some(parent) = request.parent {
            self.device(parent)?
                .children
                .checked_add(1)
                .ok_or(Error::Quota)?;
        }
        let account = self.source(context)?.funding.domain().clone();
        let id = self.ids.allocate()?;
        let publication = self
            .source(context)?
            .publications
            .prepare(request.seq, id, &account)?;
        let text_size = request
            .name
            .len()
            .checked_add(request.identity.len())
            .ok_or(Error::Quota)?;
        let text_bytes = account.acquire(DeviceResource::Bytes, text_size)?;
        let mut name = String::new();
        name.try_reserve_exact(request.name.len())
            .map_err(|_| Error::Resource)?;
        name.push_str(request.name);
        let mut identity = Vec::new();
        identity
            .try_reserve_exact(request.identity.len())
            .map_err(|_| Error::Resource)?;
        identity.extend_from_slice(request.identity);
        let extra = name
            .capacity()
            .checked_add(identity.capacity())
            .ok_or(Error::Quota)?;
        let extra_text_bytes = if extra > text_size {
            Some(account.acquire(DeviceResource::Bytes, extra - text_size)?)
        } else {
            None
        };
        let device = Device {
            id,
            source: context,
            publication: request.seq,
            parent: request.parent,
            name,
            identity,
            limits: request.limits,
            stop_requirement: request.stop_requirement,
            endpoint: request.endpoint,
            provision,
            seat: request.claimable.then(|| Seat::new(id)),
            task,
            binding_task: None,
            activation: None,
            control_closed: false,
            children: 0,
            retained: 0,
            _text_bytes: text_bytes,
            _extra_text_bytes: extra_text_bytes,
        };
        let node = prepare(&self.devices, id.serial(), device, &account)?;
        Ok(PreparedDevice {
            source: context,
            publication,
            node,
        })
    }

    pub fn commit_registration(
        &mut self,
        prepared: PreparedDevice<C>,
    ) -> Result<RegisterCommit, RegisterFailure<C>> {
        let d = &prepared.node.value().value;
        let request = Registration {
            seq: d.publication,
            parent: d.parent,
            name: &d.name,
            identity: &d.identity,
            limits: d.limits,
            claimable: d.seat.is_some(),
            stop_requirement: d.stop_requirement,
            endpoint: d.endpoint,
        };
        if d.id.incarnation() != self.incarnation {
            return Err(RegisterFailure {
                error: Error::StaleInstance,
                prepared,
            });
        }
        match self.existing_registration(prepared.source, &request) {
            Ok(Some(id)) => return Ok(RegisterCommit::Existing(id)),
            Ok(None) => {}
            Err(error) => return Err(RegisterFailure { error, prepared }),
        }
        let result = self
            .check_registration(prepared.source, &request)
            .and_then(|()| {
                if self.devices.has_room() {
                    Ok(())
                } else {
                    Err(Error::Quota)
                }
            });
        if let Err(error) = result {
            return Err(RegisterFailure { error, prepared });
        }
        let id = d.id;
        let parent = d.parent;
        let PreparedDevice {
            source,
            publication,
            node,
        } = prepared;
        // 来源在上面的同一次独占访问中已验证，尚未发生任何 await/远端调用。
        let source_record = self
            .sources
            .get_mut(source)
            .expect("registration source disappeared");
        if let Err(failure) = source_record.value.publications.commit(publication) {
            return Err(RegisterFailure {
                error: failure.error,
                prepared: PreparedDevice {
                    source,
                    publication: failure.prepared,
                    node,
                },
            });
        }
        self.devices.insert_prepared(node);
        if let Some(parent) = parent {
            self.device_mut(parent)
                .expect("registration parent disappeared")
                .children += 1;
        }
        self.changed(id);
        Ok(RegisterCommit::Created(id))
    }

    pub fn withdraw_publication(&mut self, context: u64, seq: u64) -> Result<Option<u64>, Error> {
        let source = self.source_mut(context)?;
        if !source.access.contains(Access::REGISTER) {
            return Err(Error::Permission);
        }
        let changed = source
            .publications
            .get(seq)
            .is_ok_and(|p| p.presence() == Presence::Present);
        let id = source.publications.withdraw(seq)?.copied();
        id.map(|id| {
            let device = self.device_mut(id)?;
            if let Some(seat) = device.seat.as_mut() {
                seat.withdraw();
            }
            let task = device.binding_task.unwrap_or(device.task).get();
            if changed {
                self.changed(id);
            }
            Ok(task)
        })
        .transpose()
    }

    pub fn query(&self, context: u64, id: DeviceId) -> Result<DeviceInfo<'_>, Error> {
        self.authorize(context, Access::OBSERVE, Scope::Subtree(id), false)?;
        self.control_snapshot(id)
    }
    /// 宿主已通过实际登记控制 context 验证本实例后使用。
    pub fn control_snapshot(&self, id: DeviceId) -> Result<DeviceInfo<'_>, Error> {
        let d = self.device(id)?;
        Ok(DeviceInfo {
            id,
            parent: d.parent,
            name: &d.name,
            identity: &d.identity,
            limits: d.limits,
            present: self.device_open(id).is_ok(),
            epoch: d.seat.as_ref().map(Seat::current_epoch),
            binding: d.seat.as_ref().and_then(Seat::active),
        })
    }

    /// Provision 实物仅由已获登记/控制权限的内部准备任务借用，不能经观察 RPC 导出。
    pub fn provision(&self, id: DeviceId) -> Result<&C, Error> {
        Ok(&self.device(id)?.provision)
    }
    pub fn limits(&self, id: DeviceId) -> Result<Limits, Error> {
        Ok(self.device(id)?.limits)
    }
    pub fn stop_requirement(&self, id: DeviceId) -> Result<binding::StopRequirement, Error> {
        Ok(self.device(id)?.stop_requirement)
    }
    /// 已认证的本轮控制/准备任务使用；公开观察仍走 query 的范围门。
    pub fn binding_snapshot(&self, reference: BindingRef) -> Result<binding::Snapshot, Error> {
        let active = self
            .device(reference.device())?
            .seat
            .as_ref()
            .and_then(Seat::active)
            .ok_or(Error::StaleEpoch)?;
        if active.reference != reference {
            return Err(Error::StaleEpoch);
        }
        Ok(active)
    }
    pub fn binding_usable(&self, reference: BindingRef) -> Result<(), Error> {
        let active = self.binding_snapshot(reference)?;
        self.device_open(reference.device())?;
        self.source_open(active.subject.context())?;
        if active.phase != binding::Phase::ResourcesReady {
            return Err(Error::Closed);
        }
        Ok(())
    }
    pub fn report_authorized(&self, context: u64, reference: BindingRef) -> Result<(), Error> {
        self.authorize(
            context,
            Access::REPORT_STOPPED,
            Scope::Subtree(reference.device()),
            false,
        )
    }

    pub fn claim(&self, context: u64, reference: BindingRef) -> Result<binding::Claim, Error> {
        self.authorize(
            context,
            Access::CLAIM,
            Scope::Subtree(reference.device()),
            false,
        )?;
        let open = self
            .authorize(
                context,
                Access::CLAIM,
                Scope::Subtree(reference.device()),
                true,
            )
            .is_ok();
        let seat = self
            .device(reference.device())?
            .seat
            .as_ref()
            .ok_or(Error::Unsupported)?;
        seat.claim(
            reference,
            Subject::from_authenticated_context(context)?,
            open,
        )
    }

    /// Binding 的真实控制、表、准备/停止任务已预付并归宿主稳定持有后调用。
    pub fn commit_claim(
        &mut self,
        context: u64,
        reference: BindingRef,
        task: u64,
    ) -> Result<binding::Commit, Error> {
        let task = NonZeroU64::new(task).ok_or(Error::Invalid)?;
        if let binding::Claim::Existing(active) = self.claim(context, reference)? {
            return Ok(binding::Commit::Existing(active));
        }
        self.source(context)?
            .retained
            .checked_add(1)
            .ok_or(Error::Quota)?;
        self.device(reference.device())?
            .retained
            .checked_add(1)
            .ok_or(Error::Quota)?;
        let open = self
            .authorize(
                context,
                Access::CLAIM,
                Scope::Subtree(reference.device()),
                true,
            )
            .is_ok();
        let result = self
            .device_mut(reference.device())?
            .seat
            .as_mut()
            .ok_or(Error::Unsupported)?
            .commit_claim(
                reference,
                Subject::from_authenticated_context(context)?,
                open,
            )?;
        if matches!(result, binding::Commit::Created(_)) {
            self.source_mut(context)?.retained += 1;
            let device = self.device_mut(reference.device())?;
            device.retained += 1;
            device.binding_task = Some(task);
            self.changed(reference.device());
        }
        Ok(result)
    }
    pub fn stop_claim(
        &mut self,
        context: u64,
        reference: BindingRef,
    ) -> Result<binding::Stop, Error> {
        let source = self.source(context)?;
        let manager = source.access.contains(Access::ADMIN);
        self.authorize(
            context,
            if manager {
                Access::ADMIN
            } else {
                Access::CLAIM
            },
            Scope::Subtree(reference.device()),
            false,
        )?;
        self.synchronize_withdrawal(reference.device())?;
        let before = self.control_snapshot(reference.device())?.binding;
        let epoch = self.control_snapshot(reference.device())?.epoch;
        let result = self
            .device_mut(reference.device())?
            .seat
            .as_mut()
            .ok_or(Error::Unsupported)?
            .stop(
                reference,
                Subject::from_authenticated_context(context)?,
                manager,
            )?;
        if before != self.control_snapshot(reference.device())?.binding
            || epoch != self.control_snapshot(reference.device())?.epoch
        {
            self.changed(reference.device());
        }
        Ok(result)
    }
    pub fn resources_ready(&mut self, reference: BindingRef) -> Result<(), Error> {
        self.device_open(reference.device())?;
        let subject = self
            .device(reference.device())?
            .seat
            .as_ref()
            .and_then(Seat::active)
            .ok_or(Error::NotReady)?
            .subject;
        self.source_open(subject.context())?;
        self.device_mut(reference.device())?
            .seat
            .as_mut()
            .ok_or(Error::Unsupported)?
            .resources_ready(reference)?;
        self.changed(reference.device());
        Ok(())
    }
    pub fn release_binding(&mut self, reference: BindingRef) -> Result<binding::Snapshot, Error> {
        self.synchronize_withdrawal(reference.device())?;
        if self.binding_snapshot(reference)?.phase != binding::Phase::Stopping {
            return Err(Error::NotReady);
        }
        if !self.binding_dependents_released(reference)? {
            return Err(Error::Busy);
        }
        let snapshot = self
            .device_mut(reference.device())?
            .seat
            .as_mut()
            .ok_or(Error::Unsupported)?
            .release(reference)?;
        // Created 同步持有这两份身份引用，retire_* 在归还前均被挡住。
        let source = self
            .source_mut(snapshot.subject.context())
            .expect("binding source disappeared");
        source.retained = source
            .retained
            .checked_sub(1)
            .expect("binding source reference missing");
        let device = self
            .device_mut(reference.device())
            .expect("binding device disappeared");
        device.retained = device
            .retained
            .checked_sub(1)
            .expect("binding device reference missing");
        device.binding_task = None;
        self.changed(reference.device());
        Ok(snapshot)
    }

    /// 扫描一个已存在节点；祖先/来源封口已即时挡新准入，此处只通知原清理任务。
    pub fn maintenance(
        &mut self,
        after: u64,
        end: u64,
    ) -> Result<Option<(u64, Option<u64>)>, Error> {
        let Some((key, d)) = self.devices.next_after(Some(&after)) else {
            return Ok(None);
        };
        if *key > end {
            return Ok(None);
        }
        let id = d.value.id;
        let key = *key;
        let claimant_closed = d
            .value
            .seat
            .as_ref()
            .and_then(Seat::active)
            .is_some_and(|b| self.source_open(b.subject.context()).is_err());
        let stopping = d
            .value
            .seat
            .as_ref()
            .and_then(Seat::active)
            .is_some_and(|binding| binding.phase == binding::Phase::Stopping);
        let closed = self.synchronize_withdrawal(id)?;
        let task = if closed || claimant_closed || stopping {
            let device = self.device_mut(id)?;
            if !closed
                && let Some(seat) = device.seat.as_mut()
                && let Some(b) = seat.active()
            {
                let before = seat.active();
                seat.stop(b.reference, b.subject, false)?;
                if before != seat.active() {
                    self.changed(id);
                }
            }
            let device = self.device(id)?;
            Some(device.binding_task.unwrap_or(device.task).get())
        } else {
            None
        };
        Ok(Some((key, task)))
    }
    pub fn scan_end(&self) -> u64 {
        self.ids.scan_end()
    }
    pub fn source_tasks(&self, after: u64) -> Option<(u64, u64)> {
        self.sources
            .next_after(Some(&after))
            .map(|(key, source)| (*key, source.value.task.get()))
    }
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
            && self.admins.is_empty()
            && self.devices.is_empty()
            && self.watches.is_empty()
    }
    /// 实际登记控制入口封本实例，仍由原任务推进祖先影响和迟到责任。
    pub fn control_withdraw(&mut self, id: DeviceId) -> Result<Option<u64>, Error> {
        let d = self.device(id)?;
        let source = d.source;
        let seq = d.publication;
        self.withdraw_publication(source, seq)
    }

    /// 宿主在发行控制/建立在途责任前持有引用，退休最后一份后才放回。
    pub fn retain_device(&mut self, id: DeviceId) -> Result<(), Error> {
        let d = self.device_mut(id)?;
        d.retained = d.retained.checked_add(1).ok_or(Error::Quota)?;
        Ok(())
    }
    pub fn release_device(&mut self, id: DeviceId) -> Result<(), Error> {
        let d = self.device_mut(id)?;
        d.retained = d.retained.checked_sub(1).ok_or(Error::Invalid)?;
        Ok(())
    }
    pub fn retain_source(&mut self, context: u64) -> Result<(), Error> {
        let s = self.source_mut(context)?;
        s.retained = s.retained.checked_add(1).ok_or(Error::Quota)?;
        Ok(())
    }
    pub fn release_source(&mut self, context: u64) -> Result<(), Error> {
        let s = self.source_mut(context)?;
        s.retained = s.retained.checked_sub(1).ok_or(Error::Invalid)?;
        Ok(())
    }
    pub fn control_closed(&mut self, id: DeviceId) -> Result<(), Error> {
        self.device_mut(id)?.control_closed = true;
        Ok(())
    }

    /// 原登记任务确认没有迟到 Prepare/在途清理后才调用；返回真实 Provision owner。
    pub fn retire_device(&mut self, id: DeviceId) -> Result<C, Error> {
        let d = self.device(id)?;
        let s = self.source(d.source)?;
        if s.publications.get(d.publication)?.presence() != Presence::Withdrawn
            || !d.control_closed
            || d.children != 0
            || d.retained != 0
            || d.seat.as_ref().and_then(Seat::active).is_some()
        {
            return Err(Error::Busy);
        }
        let source = d.source;
        let seq = d.publication;
        let parent = d.parent;
        self.changed(id);
        let publication = self.source_mut(source)?.publications.remove_retired(seq)?;
        // 无 await，引用和表项由本 actor 独占；以上条件排除了全部持久引用。
        let node = self
            .devices
            .remove(id.serial())
            .expect("retired device disappeared");
        if let Some(parent) = parent {
            self.device_mut(parent)
                .expect("retired device parent disappeared")
                .children -= 1;
        }
        let value = node.into_value();
        drop(publication);
        Ok(value.value.provision)
    }
    pub fn retire_source(&mut self, context: u64) -> Result<(), Error> {
        let s = self.source(context)?;
        if !s.sealed
            || !s.use_closed
            || !s.admin_closed
            || s.children != 0
            || s.retained != 0
            || s.publications.next(0).is_some()
        {
            return Err(Error::Busy);
        }
        let parent = s.parent;
        let scope = s.scope;
        let admin = s.ids.admin_context;
        self.admins.remove(admin);
        self.sources.remove(context);
        if let Some(parent) = parent {
            self.source_mut(parent)?.children -= 1;
        }
        if let Scope::Subtree(id) = scope {
            self.release_device(id)?;
        }
        Ok(())
    }
}
