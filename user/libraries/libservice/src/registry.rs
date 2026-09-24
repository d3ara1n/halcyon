//! 服务注册状态与只读 FAL 投影的单一 owner。

use crate::{
    authority::{AuthorityError, AuthorityTable, InstallFailure, PreparedAuthority},
    protocol::{InstanceInfo, State, TerminalReason, valid_name},
    record::ServiceRecord,
    resource::ServiceResource,
};
use alloc::{string::String, sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::{HandleRole, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Charge};
use libfal::watch::{Effect, Effects};
use libfal::{
    authority::{AccessSnapshot, FalRights},
    backend::{Backend, BackendError, LookupResult, NodeMetadata, ReadError},
    node::{NodeKind, validate_path},
    protocol::{WatchMask, WatchReason},
    resource::FalResource,
    store::{NodeId, NodeRef, NodeStore, Payload, PreparedNode, RetireContext, RetireProgress},
    value::{Capability, ExportPolicy, ReadSnapshot, SnapshotHandle, ValueError},
};
use metadata_admission::{Counter, Permit};
use ordered_table::{InsertError, OrderedTable, PreparedEntry};

const ROOT_RIGHTS: FalRights = FalRights::TRAVERSE
    .union(FalRights::ENUMERATE)
    .union(FalRights::READ_PROPERTY)
    .union(FalRights::WATCH)
    .union(FalRights::ACQUIRE_CAPABILITY);
const RECORD_RIGHTS: FalRights = FalRights::READ_PROPERTY
    .union(FalRights::WATCH)
    .union(FalRights::ACQUIRE_CAPABILITY);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Closed,
    Permission,
    Invalid,
    Exists,
    NotFound,
    Conflict,
    Expired,
    Resource(SystemCallError),
}

pub struct Transition {
    pub info: InstanceInfo,
    pub effects: Effects,
}

impl From<SystemCallError> for RegistryError {
    fn from(error: SystemCallError) -> Self {
        Self::Resource(error)
    }
}

fn registry_authority_error(error: AuthorityError) -> RegistryError {
    match error {
        AuthorityError::Closed => RegistryError::Closed,
        AuthorityError::Invalid => RegistryError::Invalid,
        AuthorityError::NotFound => RegistryError::NotFound,
        AuthorityError::Permission => RegistryError::Permission,
        AuthorityError::Exists => RegistryError::Exists,
        AuthorityError::Resource(error) => RegistryError::Resource(error),
    }
}

type PreparedRegistration<C> = (
    PreparedEntry<NameBinding, String>,
    PreparedEntry<Registration<C>>,
);

#[derive(Debug)]
pub struct RegisterFailure<C> {
    pub error: RegistryError,
    pub endpoint: C,
}

struct NameBinding {
    instance: u64,
    node: Option<NodeId>,
    _charge: Charge,
}

struct Registration<C> {
    name: String,
    instance: u64,
    generation: u64,
    state: State,
    reason: TerminalReason,
    protocol: u64,
    version: u32,
    policy: ExportPolicy,
    establish_deadline: Deadline,
    record: Vec<u8>,
    endpoint: Option<C>,
    prepared_node: Option<PreparedNode<Projection>>,
    node: Option<NodeRef>,
    retire_entry: Option<PreparedEntry<()>>,
    control_alive: bool,
    endpoint_observed: bool,
    _slot: Permit,
    _registration_charge: Charge,
    _bytes_charge: Charge,
}

impl<C> Registration<C> {
    fn info(&self) -> InstanceInfo {
        InstanceInfo {
            instance: self.instance,
            generation: self.generation,
            state: self.state,
            reason: self.reason,
            protocol: self.protocol,
            version: self.version,
            establish_deadline: self.establish_deadline,
        }
    }
}

struct Projection {
    kind: ProjectionKind,
    version: u64,
}

enum ProjectionKind {
    Root,
    Record { instance: u64 },
}

impl Payload for Projection {
    fn retire(
        &mut self,
        _context: &mut RetireContext<'_, Self>,
        _budget: usize,
    ) -> Result<RetireProgress, SystemCallError> {
        Ok(RetireProgress {
            work_done: 0,
            done: true,
        })
    }
}

pub struct Registry<C> {
    nodes: NodeStore<Projection>,
    root: Option<NodeRef>,
    names: OrderedTable<NameBinding, String>,
    registrations: OrderedTable<Registration<C>>,
    draining: OrderedTable<()>,
    authorities: AuthorityTable,
    observed_authorities: usize,
    registration_slots: Arc<Counter>,
    fal_account: AccountView<FalResource>,
    service_account: AccountView<ServiceResource>,
    wake: alloc::rc::Rc<dyn libexecution::wake::Wake>,
    root_version: u64,
    cursor_epoch: u64,
    reserved_root_versions: u64,
    sealed: bool,
}

impl<C: Capability> Registry<C> {
    pub fn new(
        fal_account: AccountView<FalResource>,
        service_account: AccountView<ServiceResource>,
        limit: usize,
        wake: alloc::rc::Rc<dyn libexecution::wake::Wake>,
    ) -> Result<Self, RegistryError> {
        if limit == 0 {
            return Err(RegistryError::Invalid);
        }
        let capacity = limit.checked_add(1).ok_or(RegistryError::Invalid)?;
        let authorities = AuthorityTable::new(limit).map_err(registry_authority_error)?;
        let registration_slots = Arc::try_new(Counter::new(limit))
            .map_err(|_| RegistryError::Resource(SystemCallError::OutOfMemory))?;
        let root = Projection {
            kind: ProjectionKind::Root,
            version: 1,
        };
        let (nodes, root) = NodeStore::new(root, &fal_account, capacity, wake.clone())
            .map_err(|failure| RegistryError::Resource(failure.error))?;
        Ok(Self {
            nodes,
            root: Some(root),
            names: OrderedTable::new(limit),
            registrations: OrderedTable::new(limit),
            draining: OrderedTable::new(limit),
            authorities,
            observed_authorities: 0,
            registration_slots,
            fal_account,
            service_account,
            wake,
            root_version: 1,
            cursor_epoch: 1,
            reserved_root_versions: 0,
            sealed: false,
        })
    }

    pub fn prepare_root_authority(&self) -> Result<PreparedAuthority, RegistryError> {
        self.authorities
            .prepare_root(self.service_account.clone())
            .map_err(registry_authority_error)
    }

    pub fn prepare_name_authority(
        &self,
        parent_identity: u64,
        name: &str,
    ) -> Result<PreparedAuthority, RegistryError> {
        self.authorities
            .prepare_exact(parent_identity, name)
            .map_err(registry_authority_error)
    }

    pub fn install_authority(
        &mut self,
        prepared: PreparedAuthority,
        identity: u64,
    ) -> Result<crate::protocol::AuthorityInfo, InstallFailure> {
        let info = self.authorities.install(prepared, identity)?;
        self.observed_authorities += 1;
        Ok(info)
    }

    pub fn reserve_wait_source(&self) -> Result<Charge, RegistryError> {
        self.service_account
            .acquire(ServiceResource::WaitSource, 1)
            .map_err(RegistryError::Resource)
    }

    pub fn authorize_name(&self, identity: u64, name: &str) -> Result<(), RegistryError> {
        self.authorities
            .authorize_name(identity, name)
            .map(|_| ())
            .map_err(registry_authority_error)
    }

    pub fn remove_authority(&mut self, identity: u64) -> Result<(), RegistryError> {
        self.authorities
            .remove(identity)
            .map_err(registry_authority_error)?;
        self.observed_authorities -= 1;
        if self.sealed {
            self.wake.publish();
        }
        Ok(())
    }

    pub fn owns_node(&self, reference: &NodeRef) -> bool {
        self.nodes.get(reference).is_some()
    }

    pub fn root_watch_target(&self) -> Result<(NodeRef, u64), RegistryError> {
        Ok((
            self.root.as_ref().ok_or(RegistryError::Closed)?.clone(),
            self.root_version,
        ))
    }

    pub fn record_watch_target(&self, instance: u64) -> Result<(NodeRef, u64), RegistryError> {
        let registration = self
            .registrations
            .get(instance)
            .ok_or(RegistryError::NotFound)?;
        let node = registration.node.as_ref().ok_or(RegistryError::NotFound)?;
        let version = self
            .projection(node)
            .map_err(|_| RegistryError::NotFound)?
            .version;
        Ok((node.clone(), version))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "注册参数共同冻结一个实例，拆分会产生可变的第二真值"
    )]
    pub fn register(
        &mut self,
        authority_identity: u64,
        name: &str,
        instance: u64,
        protocol: u64,
        version: u32,
        policy: ExportPolicy,
        establish_deadline: Deadline,
        endpoint: C,
    ) -> Result<InstanceInfo, RegisterFailure<C>> {
        let service_account = match self.authorities.authorize_name(authority_identity, name) {
            Ok(account) => account,
            Err(error) => {
                return Err(RegisterFailure {
                    error: registry_authority_error(error),
                    endpoint,
                });
            }
        };
        match endpoint.description() {
            Ok(description)
                if description.role == HandleRole::MailboxSender as u32
                    && policy.transport.is_subset_of(description.rights) =>
            {
                // 真实 endpoint role/rights 已在任何资源预留前冻结验证。
            }
            Ok(_) => {
                return Err(RegisterFailure {
                    error: RegistryError::Invalid,
                    endpoint,
                });
            }
            Err(error) => {
                return Err(RegisterFailure {
                    error: RegistryError::Resource(error),
                    endpoint,
                });
            }
        }
        let validate = (|| {
            if self.sealed {
                return Err(RegistryError::Closed);
            }
            if !valid_name(name)
                || instance == 0
                || protocol == 0
                || version == 0
                || establish_deadline.instant().ok().flatten().is_none()
            {
                return Err(RegistryError::Invalid);
            }
            if self.names.get_by(name).is_some() || self.registrations.get(instance).is_some() {
                return Err(RegistryError::Exists);
            }
            self.root_version
                .checked_add(self.reserved_root_versions)
                .and_then(|version| version.checked_add(2))
                .ok_or(RegistryError::Resource(SystemCallError::ReachLimit))?;
            Ok(())
        })();
        if let Err(error) = validate {
            return Err(RegisterFailure { error, endpoint });
        }
        let mut endpoint = Some(endpoint);
        let prepared: Result<PreparedRegistration<C>, RegistryError> = (|| {
            let stored_name = copy_name(name)?;
            let index_name = copy_name(name)?;
            let record = ServiceRecord {
                instance,
                protocol,
                version,
                generation: 2,
                endpoint_policy: policy,
            }
            .encode()
            .map_err(|error| match error {
                ValueError::Allocation => RegistryError::Resource(SystemCallError::OutOfMemory),
                ValueError::Capability(error) => RegistryError::Resource(error),
                _ => RegistryError::Invalid,
            })?;
            let slot = Counter::try_acquire(&self.registration_slots)
                .map_err(|_| RegistryError::Resource(SystemCallError::QuotaExceeded))?;
            let registration_charge = service_account.acquire(ServiceResource::Registration, 1)?;
            let bytes = stored_name
                .len()
                .checked_add(record.len())
                .and_then(|bytes| {
                    bytes.checked_add(PreparedEntry::<Registration<C>>::allocation_bytes())
                })
                .and_then(|bytes| bytes.checked_add(PreparedEntry::<()>::allocation_bytes()))
                .ok_or(RegistryError::Resource(SystemCallError::ReachLimit))?;
            let bytes_charge = service_account.acquire(ServiceResource::Bytes, bytes)?;
            let name_charge = service_account.acquire(
                ServiceResource::Bytes,
                index_name.len() + PreparedEntry::<NameBinding, String>::allocation_bytes(),
            )?;
            let prepared_node = self
                .nodes
                .prepare(
                    Projection {
                        kind: ProjectionKind::Record { instance },
                        version: 2,
                    },
                    &self.fal_account,
                )
                .map_err(|failure| RegistryError::Resource(failure.error))?;
            let retire_entry = self
                .draining
                .prepare_insert(instance, ())
                .map_err(|error| match error {
                    InsertError::Limit(()) => {
                        RegistryError::Resource(SystemCallError::QuotaExceeded)
                    }
                    InsertError::Allocation(()) => {
                        RegistryError::Resource(SystemCallError::OutOfMemory)
                    }
                })?;
            let binding = NameBinding {
                instance,
                node: None,
                _charge: name_charge,
            };
            let name_entry =
                prepare_entry(&self.names, index_name, binding).map_err(|(error, _)| error)?;
            let registration = Registration {
                name: stored_name,
                instance,
                generation: 1,
                state: State::Starting,
                reason: TerminalReason::None,
                protocol,
                version,
                policy,
                establish_deadline,
                record,
                endpoint: endpoint.take(),
                prepared_node: Some(prepared_node),
                node: None,
                retire_entry: Some(retire_entry),
                control_alive: true,
                endpoint_observed: true,
                _slot: slot,
                _registration_charge: registration_charge,
                _bytes_charge: bytes_charge,
            };
            let registration_entry =
                match prepare_entry(&self.registrations, instance, registration) {
                    Ok(entry) => entry,
                    Err((error, mut registration)) => {
                        endpoint = registration.endpoint.take();
                        return Err(error);
                    }
                };
            Ok((name_entry, registration_entry))
        })();
        let (name_entry, registration_entry) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(RegisterFailure {
                    error,
                    endpoint: endpoint
                        .take()
                        .expect("registration endpoint lost before commit"),
                });
            }
        };
        self.names.insert_prepared(name_entry);
        self.registrations.insert_prepared(registration_entry);
        self.reserved_root_versions += 2;
        Ok(self
            .registrations
            .get(instance)
            .expect("installed registration missing")
            .info())
    }

    pub fn query_name(
        &self,
        authority_identity: u64,
        name: &str,
    ) -> Result<InstanceInfo, RegistryError> {
        self.authorities
            .authorize_name(authority_identity, name)
            .map_err(registry_authority_error)?;
        let binding = self.names.get_by(name).ok_or(RegistryError::NotFound)?;
        self.registrations
            .get(binding.instance)
            .map(Registration::info)
            .ok_or(RegistryError::NotFound)
    }

    pub fn query(&self, instance: u64) -> Result<InstanceInfo, RegistryError> {
        self.registrations
            .get(instance)
            .map(Registration::info)
            .ok_or(RegistryError::NotFound)
    }

    pub fn publish_ready(
        &mut self,
        instance: u64,
        now_ns: u64,
    ) -> Result<Transition, RegistryError> {
        let prepared = {
            let registration = self
                .registrations
                .get_mut(instance)
                .ok_or(RegistryError::NotFound)?;
            match registration.state {
                State::Ready => {
                    return Ok(Transition {
                        info: registration.info(),
                        effects: Effects::default(),
                    });
                }
                State::Draining | State::Terminal => return Err(RegistryError::Conflict),
                State::Starting => {}
            }
            if registration
                .establish_deadline
                .instant()
                .ok()
                .flatten()
                .is_none_or(|deadline| now_ns >= deadline)
            {
                return Err(RegistryError::Expired);
            }
            registration
                .prepared_node
                .take()
                .expect("Starting registration lost prepared projection")
        };
        let node = self.nodes.commit(prepared);
        self.nodes.link(&node);
        {
            let registration = self
                .registrations
                .get(instance)
                .expect("published registration disappeared");
            let binding = self
                .names
                .get_mut_by(registration.name.as_str())
                .expect("Starting registration lost name binding");
            assert_eq!(binding.instance, instance, "name binding instance changed");
            binding.node = Some(node.id());
        }
        let record_id = node.id();
        let info = {
            let registration = self
                .registrations
                .get_mut(instance)
                .expect("published registration disappeared");
            registration.node = Some(node);
            registration.state = State::Ready;
            registration.generation = 2;
            registration.info()
        };
        self.commit_visible_change();
        self.reserved_root_versions -= 1;
        let mut effects = Effects::default();
        effects.push(Effect {
            node: self
                .root
                .as_ref()
                .expect("published Registry lost root")
                .id(),
            generation: self.root_version,
            events: WatchMask::CREATE,
            terminal: None,
        });
        effects.push(Effect {
            node: record_id,
            generation: info.generation,
            events: WatchMask::MODIFY,
            terminal: None,
        });
        Ok(Transition { info, effects })
    }

    pub fn begin_drain(
        &mut self,
        instance: u64,
        reason: TerminalReason,
    ) -> Result<Transition, RegistryError> {
        let registration = self
            .registrations
            .get(instance)
            .ok_or(RegistryError::NotFound)?;
        if matches!(registration.state, State::Draining | State::Terminal) {
            return Ok(Transition {
                info: registration.info(),
                effects: Effects::default(),
            });
        }
        let effects = self.drain_named(instance, None, reason)?;
        let info = self
            .registrations
            .get(instance)
            .expect("drained registration disappeared")
            .info();
        Ok(Transition { info, effects })
    }

    pub fn expire_if_starting(
        &mut self,
        instance: u64,
        now_ns: u64,
    ) -> Result<bool, RegistryError> {
        let registration = self
            .registrations
            .get(instance)
            .ok_or(RegistryError::NotFound)?;
        if registration.state != State::Starting {
            return Ok(false);
        }
        let deadline = registration
            .establish_deadline
            .instant()
            .expect("validated establish deadline changed representation")
            .expect("Starting registration lost establish deadline");
        if now_ns < deadline {
            return Ok(false);
        }
        self.drain_named(instance, None, TerminalReason::EstablishExpired)?;
        Ok(true)
    }

    pub fn withdraw(
        &mut self,
        authority_identity: u64,
        name: &str,
        expected_instance: u64,
        expected_generation: u64,
    ) -> Result<Transition, RegistryError> {
        self.authorities
            .authorize_name(authority_identity, name)
            .map_err(registry_authority_error)?;
        let binding = self.names.get_by(name).ok_or(RegistryError::NotFound)?;
        let registration = self
            .registrations
            .get(binding.instance)
            .ok_or(RegistryError::NotFound)?;
        if binding.instance != expected_instance || registration.generation != expected_generation {
            return Err(RegistryError::Conflict);
        }
        let effects = self.drain_named(expected_instance, Some(name), TerminalReason::Withdrawn)?;
        let info = self
            .registrations
            .get(expected_instance)
            .expect("withdrawn registration disappeared")
            .info();
        Ok(Transition { info, effects })
    }

    fn drain_named(
        &mut self,
        instance: u64,
        expected_name: Option<&str>,
        reason: TerminalReason,
    ) -> Result<Effects, RegistryError> {
        let actual_name = self
            .registrations
            .get(instance)
            .ok_or(RegistryError::NotFound)?
            .name
            .as_str();
        if expected_name.is_some_and(|expected| expected != actual_name) {
            return Err(RegistryError::Conflict);
        }
        let binding = self
            .names
            .remove_by(actual_name)
            .ok_or(RegistryError::NotFound)?;
        assert_eq!(
            binding.instance, instance,
            "Registry name binding changed instance"
        );
        let mut effects = Effects::default();
        let mut record_change = None;
        let retire_entry;
        {
            let registration = self
                .registrations
                .get_mut(instance)
                .ok_or(RegistryError::NotFound)?;
            match registration.state {
                State::Starting => {
                    registration.prepared_node = None;
                    registration.generation = registration
                        .generation
                        .checked_add(1)
                        .ok_or(RegistryError::Resource(SystemCallError::ReachLimit))?;
                    self.reserved_root_versions -= 2;
                }
                State::Ready => {
                    let node = registration
                        .node
                        .take()
                        .expect("Ready registration lost projection node");
                    assert_eq!(binding.node, Some(node.id()), "name projection changed");
                    registration.generation = registration
                        .generation
                        .checked_add(1)
                        .ok_or(RegistryError::Resource(SystemCallError::ReachLimit))?;
                    self.nodes
                        .get_mut(&node)
                        .expect("draining record projection disappeared")
                        .version = registration.generation;
                    self.nodes.unlink(&node);
                    record_change = Some((node.id(), registration.generation));
                    self.reserved_root_versions -= 1;
                }
                State::Draining | State::Terminal => return Ok(effects),
            }
            registration.state = State::Draining;
            registration.reason = reason;
            if !registration.endpoint_observed {
                retire_entry = registration.retire_entry.take();
            } else {
                retire_entry = None;
            }
        }
        if let Some(retire_entry) = retire_entry {
            self.draining.insert_prepared(retire_entry);
        }
        if let Some((record, generation)) = record_change {
            self.commit_visible_change();
            effects.push(Effect {
                node: self
                    .root
                    .as_ref()
                    .expect("draining Registry lost root")
                    .id(),
                generation: self.root_version,
                events: WatchMask::DELETE,
                terminal: None,
            });
            effects.push(Effect {
                node: record,
                generation,
                events: WatchMask::DELETE | WatchMask::TERMINATED,
                terminal: Some(WatchReason::NodeDeleted),
            });
        }
        Ok(effects)
    }

    /// Runtime 已确认撤销该实例的 endpoint CLOSED source 后才开放母本退休。
    pub fn release_endpoint_observation(&mut self, instance: u64) {
        let Some(registration) = self.registrations.get_mut(instance) else {
            return;
        };
        if !registration.endpoint_observed {
            return;
        }
        registration.endpoint_observed = false;
        if registration.state == State::Draining {
            let retire_entry = registration
                .retire_entry
                .take()
                .expect("draining registration lost prepared retirement entry");
            self.draining.insert_prepared(retire_entry);
        }
    }

    pub fn release_control(&mut self, instance: u64) {
        let Some(registration) = self.registrations.get_mut(instance) else {
            return;
        };
        registration.control_alive = false;
        if registration.state == State::Terminal {
            self.registrations.remove(instance);
        }
    }

    pub fn retire_registrations(&mut self, budget: usize) -> Result<usize, SystemCallError> {
        let mut work_done = 0;
        while work_done < budget {
            let Some((instance, ())) = self.draining.next_after(None) else {
                break;
            };
            let instance = *instance;
            {
                let registration = self
                    .registrations
                    .get_mut(instance)
                    .expect("retiring registration disappeared");
                assert_eq!(
                    registration.state,
                    State::Draining,
                    "retirement index contains a non-draining registration"
                );
                if let Some(endpoint) = registration.endpoint.take() {
                    match endpoint.close() {
                        Ok(()) => {}
                        Err((endpoint, error)) => {
                            registration.endpoint = Some(endpoint);
                            return Err(error);
                        }
                    }
                }
                registration.state = State::Terminal;
                registration.generation = registration
                    .generation
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?;
            }
            self.draining
                .remove(instance)
                .expect("retired registration missing retirement index");
            work_done += 1;
            if !self
                .registrations
                .get(instance)
                .expect("retired registration disappeared")
                .control_alive
            {
                self.registrations.remove(instance);
            }
        }
        Ok(work_done)
    }

    fn commit_visible_change(&mut self) {
        self.root_version = self
            .root_version
            .checked_add(1)
            .expect("reserved Registry root version exhausted");
        self.cursor_epoch = self
            .cursor_epoch
            .checked_add(1)
            .expect("Registry cursor epoch exhausted");
        if let Some(root) = self.root.as_ref() {
            self.nodes
                .get_mut(root)
                .expect("Registry root disappeared")
                .version = self.root_version;
        }
    }

    fn projection(&self, reference: &NodeRef) -> Result<&Projection, BackendError> {
        self.nodes.get(reference).ok_or(BackendError::NotFound)
    }

    fn record_registration(&self, reference: &NodeRef) -> Result<&Registration<C>, BackendError> {
        let ProjectionKind::Record { instance } = self.projection(reference)?.kind else {
            return Err(BackendError::WrongType);
        };
        let registration = self
            .registrations
            .get(instance)
            .ok_or(BackendError::NotFound)?;
        if registration.state != State::Ready
            || registration.node.as_ref().map(NodeRef::id) != Some(reference.id())
        {
            return Err(BackendError::NotFound);
        }
        Ok(registration)
    }
}

impl<C: Capability> Backend<C> for Registry<C> {
    fn root(&self) -> Option<&NodeRef> {
        self.root.as_ref()
    }

    fn resolve(&self, access: &AccessSnapshot, path: &str) -> Result<NodeRef, BackendError> {
        if self.sealed || !validate_path(path.as_bytes()) {
            return Err(if self.sealed {
                BackendError::Closed
            } else {
                BackendError::InvalidName
            });
        }
        if access.root().id() != self.root.as_ref().ok_or(BackendError::Closed)?.id() {
            return Err(BackendError::NotFound);
        }
        if !access.rights().contains(FalRights::TRAVERSE) {
            return Err(BackendError::Permission);
        }
        if path.is_empty() {
            return Ok(access.root().clone());
        }
        if path.contains('/') {
            return Err(BackendError::NotFound);
        }
        let binding = self.names.get_by(path).ok_or(BackendError::NotFound)?;
        let node = binding.node.ok_or(BackendError::NotFound)?;
        self.nodes.pin_linked(node).ok_or(BackendError::NotFound)
    }

    fn lookup(&self, access: &AccessSnapshot, path: &str) -> Result<LookupResult<C>, BackendError> {
        self.resolve(access, path).map(LookupResult::Found)
    }

    fn metadata(
        &self,
        reference: &NodeRef,
        ceiling: FalRights,
    ) -> Result<NodeMetadata, BackendError> {
        let projection = self.projection(reference)?;
        let (kind, rights, size) = match projection.kind {
            ProjectionKind::Root => (NodeKind::Directory, ROOT_RIGHTS, 0),
            ProjectionKind::Record { .. } => {
                let registration = self.record_registration(reference)?;
                (
                    NodeKind::Property,
                    RECORD_RIGHTS,
                    registration.record.len() as u64,
                )
            }
        };
        Ok(NodeMetadata {
            identity: reference.id().raw(),
            version: projection.version,
            kind,
            rights: rights.intersect(ceiling),
            size,
        })
    }

    fn enumerate<F>(
        &self,
        parent: &NodeRef,
        access: &AccessSnapshot,
        cursor: u64,
        limit: usize,
        mut visit: F,
    ) -> Result<u64, BackendError>
    where
        F: FnMut(&str, &NodeRef, NodeMetadata),
    {
        if limit == 0 || parent.id() != access.root().id() {
            return Err(BackendError::InvalidName);
        }
        if !access.rights().contains(FalRights::ENUMERATE) {
            return Err(BackendError::Permission);
        }
        let epoch = u32::try_from(self.cursor_epoch)
            .map_err(|_| BackendError::Resource(SystemCallError::ReachLimit))?;
        let ordinal = if cursor == 0 {
            0
        } else {
            if (cursor >> 32) as u32 != epoch {
                return Err(BackendError::Conflict);
            }
            cursor as u32 as usize
        };
        let mut after = None;
        let mut visible = 0usize;
        let mut emitted = 0usize;
        while let Some((name, binding)) = self.names.next_after(after) {
            after = Some(name.as_str());
            let Some(node_id) = binding.node else {
                continue;
            };
            if visible < ordinal {
                visible += 1;
                continue;
            }
            let reference = self
                .nodes
                .pin_linked(node_id)
                .ok_or(BackendError::NotFound)?;
            let metadata = self.metadata(&reference, access.rights())?;
            visit(name, &reference, metadata);
            emitted += 1;
            visible += 1;
            if emitted == limit {
                let mut rest = after;
                let mut more = false;
                while let Some((name, binding)) = self.names.next_after(rest) {
                    rest = Some(name.as_str());
                    if binding.node.is_some() {
                        more = true;
                        break;
                    }
                }
                return Ok(if more {
                    ((epoch as u64) << 32) | visible as u64
                } else {
                    0
                });
            }
        }
        Ok(0)
    }

    fn read_snapshot(
        &self,
        access: &AccessSnapshot,
        path: &str,
    ) -> Result<ReadSnapshot<C>, ReadError> {
        let reference = self.resolve(access, path)?;
        if !access
            .rights()
            .contains(FalRights::READ_PROPERTY | FalRights::ACQUIRE_CAPABILITY)
        {
            return Err(BackendError::Permission.into());
        }
        let registration = self.record_registration(&reference)?;
        if !(registration.policy.transport & (Rights::TRANSIT | Rights::GRANT))
            .is_subset_of(access.output_transport())
        {
            return Err(BackendError::Permission.into());
        }
        let endpoint = registration
            .endpoint
            .as_ref()
            .ok_or(BackendError::NotFound)?;
        let snapshot = if registration.policy.protocol == libfal::value::Protocol::Directory {
            SnapshotHandle::Directory {
                provider: endpoint
                    .duplicate(Rights::WRITE | Rights::WAIT)
                    .map_err(ValueError::Capability)?,
                policy: registration.policy,
            }
        } else {
            SnapshotHandle::Direct {
                owner: endpoint
                    .duplicate(registration.policy.transport)
                    .map_err(ValueError::Capability)?,
                policy: registration.policy,
            }
        };
        let mut snapshots = Vec::new();
        snapshots
            .try_reserve_exact(1)
            .map_err(|_| ReadError::Value(ValueError::Allocation))?;
        snapshots.push(snapshot);
        ReadSnapshot::prepare(&registration.record, snapshots, access.account())
            .map_err(|failure| ReadError::Value(failure.error))
    }

    fn watch_snapshot(
        &self,
        access: &AccessSnapshot,
        path: &str,
    ) -> Result<(NodeRef, u64), BackendError> {
        if !access.rights().contains(FalRights::WATCH) {
            return Err(BackendError::Permission);
        }
        let reference = self.resolve(access, path)?;
        let metadata = self.metadata(&reference, access.rights())?;
        Ok((reference, metadata.version))
    }

    fn seal(&mut self) {
        if self.sealed {
            return;
        }
        self.sealed = true;
        while let Some((_name, binding)) = self.names.pop_first() {
            let retire_entry = {
                let registration = self
                    .registrations
                    .get_mut(binding.instance)
                    .expect("Registry name lost registration");
                match registration.state {
                    State::Starting => {
                        registration.prepared_node = None;
                        self.reserved_root_versions -= 2;
                    }
                    State::Ready => {
                        let node = registration
                            .node
                            .take()
                            .expect("Ready registration lost node during seal");
                        self.nodes.unlink(&node);
                        self.reserved_root_versions -= 1;
                    }
                    State::Draining | State::Terminal => {
                        panic!("visible Registry name points to retired registration")
                    }
                }
                registration.state = State::Draining;
                registration.reason = TerminalReason::ProviderStopping;
                registration.generation = registration
                    .generation
                    .checked_add(1)
                    .expect("Registry generation exhausted during seal");
                if registration.endpoint_observed {
                    None
                } else {
                    Some(
                        registration
                            .retire_entry
                            .take()
                            .expect("live registration lost prepared retirement entry"),
                    )
                }
            };
            if let Some(retire_entry) = retire_entry {
                self.draining.insert_prepared(retire_entry);
            }
        }
        self.authorities.seal();
        self.root = None;
        self.nodes.seal();
    }

    fn has_retire_work(&self) -> bool {
        (self.authorities.is_sealed()
            && self.observed_authorities == 0
            && !self.authorities.is_empty())
            || !self.draining.is_empty()
            || self.nodes.has_retire_work()
    }

    fn retire_step(&mut self, budget: usize) -> Result<RetireProgress, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let authority_work = if self.observed_authorities == 0 {
            self.authorities.retire_step(budget)
        } else {
            0
        };
        let registration_work = self.retire_registrations(budget - authority_work)?;
        let remaining = budget - authority_work - registration_work;
        let node_progress = if remaining == 0 {
            RetireProgress {
                work_done: 0,
                done: self.nodes.is_empty(),
            }
        } else {
            self.nodes.retire_step(remaining)?
        };
        Ok(RetireProgress {
            work_done: authority_work + registration_work + node_progress.work_done,
            done: (!self.authorities.is_sealed() || self.authorities.is_empty())
                && self.draining.is_empty()
                && node_progress.done,
        })
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.authorities.is_empty()
            && self.draining.is_empty()
            && self
                .registrations
                .count_matching(|registration| registration.state != State::Terminal)
                == 0
    }

    fn close(mut self) -> Result<(), Self> {
        if !self.sealed || !self.is_empty() {
            return Err(self);
        }
        while self.registrations.pop_first().is_some() {}
        while self.names.pop_first().is_some() {}
        while self.draining.pop_first().is_some() {}
        Ok(())
    }
}

fn copy_name(name: &str) -> Result<String, RegistryError> {
    let mut copy = String::new();
    copy.try_reserve_exact(name.len())
        .map_err(|_| RegistryError::Resource(SystemCallError::OutOfMemory))?;
    copy.push_str(name);
    Ok(copy)
}

fn prepare_entry<V, K: Ord>(
    table: &OrderedTable<V, K>,
    key: K,
    value: V,
) -> Result<PreparedEntry<V, K>, (RegistryError, V)> {
    table
        .prepare_insert(key, value)
        .map_err(|error| match error {
            InsertError::Limit(value) => (
                RegistryError::Resource(SystemCallError::QuotaExceeded),
                value,
            ),
            InsertError::Allocation(value) => {
                (RegistryError::Resource(SystemCallError::OutOfMemory), value)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use erhino_shared::object::{HandleDescription, HandleRole};
    use libbudget::{Budget, Taxonomy};
    use libexecution::wake::Wake;

    #[derive(Debug, Clone)]
    struct TestCapability {
        id: u64,
        mailbox: u64,
        rights: Rights,
    }

    impl Capability for TestCapability {
        fn description(&self) -> Result<HandleDescription, SystemCallError> {
            Ok(HandleDescription {
                object_id: self.id,
                related_object_id: self.mailbox,
                kind: 0,
                role: HandleRole::MailboxSender as u32,
                rights: self.rights,
                badge: 0,
                reserved: 0,
            })
        }

        fn duplicate(&self, rights: Rights) -> Result<Self, SystemCallError> {
            if !rights.is_subset_of(self.rights) {
                return Err(SystemCallError::RightsDenied);
            }
            Ok(Self {
                id: self.id,
                mailbox: self.mailbox,
                rights,
            })
        }

        fn close(self) -> Result<(), (Self, SystemCallError)> {
            Ok(())
        }
    }

    struct TestWake;
    impl Wake for TestWake {
        fn publish(&self) {}
    }

    struct CountingWake(Rc<core::cell::Cell<usize>>);
    impl Wake for CountingWake {
        fn publish(&self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn accounts() -> (AccountView<FalResource>, AccountView<ServiceResource>) {
        let fal_layout = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        let service_layout = [9, 10, 11, 12];
        let mut limits = [0; FalResource::COUNT + ServiceResource::COUNT];
        for index in fal_layout {
            limits[index] = 128 * 1024;
        }
        for index in service_layout {
            limits[index] = 128 * 1024;
        }
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let fal_binding = fal_layout.map(|index| budget.slot(index).unwrap());
        let service_binding = service_layout.map(|index| budget.slot(index).unwrap());
        (
            account.view(&fal_binding).unwrap(),
            account.view(&service_binding).unwrap(),
        )
    }

    fn policy() -> ExportPolicy {
        ExportPolicy {
            protocol: libfal::value::Protocol::Directory,
            mode: libfal::value::ExportMode::Repeatable,
            transport: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
            fal_ceiling: FalRights::TRAVERSE | FalRights::ENUMERATE | FalRights::READ_PROPERTY,
        }
    }

    fn endpoint(id: u64) -> TestCapability {
        TestCapability {
            id,
            mailbox: 99,
            rights: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        }
    }

    fn access(root: NodeRef, account: AccountView<FalResource>) -> AccessSnapshot {
        AccessSnapshot::new_for_test(root, ROOT_RIGHTS, Rights::TRANSIT, account)
    }

    fn install_authority(registry: &mut Registry<TestCapability>, identity: u64) {
        let prepared = registry.prepare_root_authority().unwrap();
        if let Err(failure) = registry.install_authority(prepared, identity) {
            panic!("authority installation failed: {:?}", failure.error);
        }
    }

    #[test]
    fn seal_retains_authorities_until_each_lifetime_observation_is_removed() {
        let (fal, service) = accounts();
        let notifications = Rc::new(core::cell::Cell::new(0));
        let mut registry = Registry::new(
            fal,
            service,
            4,
            Rc::new(CountingWake(notifications.clone())),
        )
        .unwrap();
        install_authority(&mut registry, 700);
        let prepared = registry.prepare_name_authority(700, "fs").unwrap();
        if let Err(failure) = registry.install_authority(prepared, 701) {
            panic!("name authority installation failed: {:?}", failure.error);
        }
        registry.seal();

        assert_eq!(registry.observed_authorities, 2);
        assert!(!registry.retire_step(64).unwrap().done);
        assert!(registry.authorities.info(700).is_ok());
        assert!(registry.authorities.info(701).is_ok());
        let before = notifications.get();

        registry.remove_authority(700).unwrap();
        assert_eq!(notifications.get(), before + 1);
        assert_eq!(registry.observed_authorities, 1);
        assert!(!registry.retire_step(64).unwrap().done);
        assert!(registry.authorities.info(701).is_ok());

        registry.remove_authority(701).unwrap();
        assert_eq!(notifications.get(), before + 2);
        assert_eq!(registry.observed_authorities, 0);
        assert!(registry.authorities.is_empty());
    }

    #[test]
    fn invalid_registry_capacity_does_not_commit_a_root() {
        let (fal, service) = accounts();
        assert!(matches!(
            Registry::<TestCapability>::new(
                fal.clone(),
                service.clone(),
                usize::MAX,
                Rc::new(TestWake),
            ),
            Err(RegistryError::Invalid)
        ));
        assert_eq!(fal.usage(FalResource::Node).0, 0);
        assert_eq!(fal.usage(FalResource::Bytes).0, 0);
        assert_eq!(service.usage(ServiceResource::Authority).0, 0);
        assert_eq!(service.usage(ServiceResource::Registration).0, 0);
    }

    #[test]
    fn exhausted_root_version_rejects_registration_without_consuming_endpoint() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal, service.clone(), 1, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        registry.root_version = u64::MAX - 1;
        let registration_usage = service.usage(ServiceResource::Registration).0;
        let failure = registry
            .register(
                700,
                "fs",
                11,
                libfal::protocol::ID,
                u32::from(libfal::protocol::VERSION),
                policy(),
                Deadline::at(100),
                endpoint(21),
            )
            .unwrap_err();
        assert_eq!(
            failure.error,
            RegistryError::Resource(SystemCallError::ReachLimit)
        );
        assert_eq!(failure.endpoint.id, 21);
        assert_eq!(
            service.usage(ServiceResource::Registration).0,
            registration_usage
        );
        assert_eq!(registry.query(11), Err(RegistryError::NotFound));
    }

    #[test]
    fn ready_projection_is_atomic_and_drain_does_not_remove_replacement() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal.clone(), service, 4, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        let root = registry.root().unwrap().clone();
        let access = access(root, fal);
        let starting = registry
            .register(
                700,
                "fs.second",
                11,
                libfal::protocol::ID,
                u32::from(libfal::protocol::VERSION),
                policy(),
                Deadline::at(100),
                endpoint(21),
            )
            .unwrap();
        assert_eq!(starting.state, State::Starting);
        assert!(matches!(
            registry.resolve(&access, "fs.second"),
            Err(BackendError::NotFound)
        ));
        let ready = registry.publish_ready(11, 50).unwrap();
        assert_eq!(ready.info.state, State::Ready);
        assert_eq!(
            ready
                .effects
                .iter()
                .map(|effect| effect.events)
                .collect::<Vec<_>>(),
            vec![WatchMask::CREATE, WatchMask::MODIFY]
        );
        let visible_version = registry.root_version;
        let repeated = registry.publish_ready(11, 50).unwrap();
        assert_eq!(repeated.info, ready.info);
        assert!(repeated.effects.is_empty());
        assert_eq!(registry.root_version, visible_version);
        let old = registry.resolve(&access, "fs.second").unwrap();
        let snapshot = registry.read_snapshot(&access, "fs.second").unwrap();
        assert!(snapshot.has_directory());
        let drained = registry.begin_drain(11, TerminalReason::Withdrawn).unwrap();
        assert_eq!(drained.info.generation, 3);
        let events = drained.effects.iter().collect::<Vec<_>>();
        assert_eq!(events[0].events, WatchMask::DELETE);
        assert_eq!(events[0].generation, registry.root_version);
        assert_eq!(events[1].events, WatchMask::DELETE | WatchMask::TERMINATED);
        assert_eq!(events[1].generation, drained.info.generation);
        assert_eq!(events[1].terminal, Some(WatchReason::NodeDeleted));
        assert_eq!(registry.nodes.get(&old).unwrap().version, 3);
        assert!(matches!(
            registry.resolve(&access, "fs.second"),
            Err(BackendError::NotFound)
        ));
        registry
            .register(
                700,
                "fs.second",
                12,
                libfal::protocol::ID,
                u32::from(libfal::protocol::VERSION),
                policy(),
                Deadline::at(200),
                endpoint(22),
            )
            .unwrap();
        registry.publish_ready(12, 120).unwrap();
        assert_ne!(
            registry.resolve(&access, "fs.second").unwrap().id(),
            old.id()
        );
        assert!(matches!(
            registry.withdraw(700, "fs.second", 11, 3),
            Err(RegistryError::Conflict)
        ));
        let withdrawn = registry.withdraw(700, "fs.second", 12, 2).unwrap();
        assert_eq!(withdrawn.info.state, State::Draining);
        assert_eq!(
            withdrawn
                .effects
                .iter()
                .map(|effect| effect.events)
                .collect::<Vec<_>>(),
            vec![WatchMask::DELETE, WatchMask::DELETE | WatchMask::TERMINATED]
        );
        assert!(
            registry
                .begin_drain(12, TerminalReason::Withdrawn)
                .unwrap()
                .effects
                .is_empty()
        );
        assert!(snapshot.has_directory());
    }

    #[test]
    fn delayed_establish_timer_cannot_drain_a_ready_instance() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal, service, 2, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        for (instance, name) in [(11, "ready"), (12, "starting")] {
            registry
                .register(
                    700,
                    name,
                    instance,
                    libfal::protocol::ID,
                    u32::from(libfal::protocol::VERSION),
                    policy(),
                    Deadline::at(100),
                    endpoint(instance),
                )
                .unwrap();
        }
        let ready = registry.publish_ready(11, 99).unwrap();
        let root_version = registry.root_version;
        assert!(!registry.expire_if_starting(11, 101).unwrap());
        assert_eq!(registry.query(11).unwrap(), ready.info);
        assert!(!registry.expire_if_starting(12, 99).unwrap());
        assert!(registry.expire_if_starting(12, 100).unwrap());
        assert_eq!(registry.query(12).unwrap().state, State::Draining);
        assert!(!registry.expire_if_starting(12, 101).unwrap());
        assert_eq!(registry.root_version, root_version);
    }

    #[test]
    fn abandoned_controls_release_registration_slots_after_retirement() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal, service, 1, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        for instance in 1..=3 {
            registry
                .register(
                    700,
                    "service",
                    instance,
                    7,
                    1,
                    policy(),
                    Deadline::at(100),
                    endpoint(instance),
                )
                .unwrap();
            registry
                .begin_drain(instance, TerminalReason::ControlClosed)
                .unwrap();
            registry.release_control(instance);
            assert_eq!(registry.retire_registrations(1).unwrap(), 0);
            assert_eq!(registry.query(instance).unwrap().state, State::Draining);
            registry.release_endpoint_observation(instance);
            assert_eq!(registry.retire_registrations(1).unwrap(), 1);
            assert_eq!(registry.query(instance), Err(RegistryError::NotFound));
        }
    }

    #[test]
    fn mailbox_record_uses_direct_snapshot_export() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal.clone(), service, 2, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        let access = access(registry.root().unwrap().clone(), fal);
        let mut mailbox_policy = policy();
        mailbox_policy.protocol = libfal::value::Protocol::Mailbox;
        mailbox_policy.fal_ceiling = FalRights::NONE;
        registry
            .register(
                700,
                "mailbox",
                31,
                7,
                1,
                mailbox_policy,
                Deadline::at(100),
                endpoint(31),
            )
            .unwrap();
        registry.publish_ready(31, 50).unwrap();
        let snapshot = registry.read_snapshot(&access, "mailbox").unwrap();
        assert!(!snapshot.has_directory());
        let direct = snapshot.into_direct().ok().unwrap();
        assert_eq!(direct.handles.len(), 1);
        assert_eq!(direct.handles[0].1, mailbox_policy.transport);
    }

    #[test]
    fn seal_waits_for_endpoint_observation_release() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal, service, 1, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        registry
            .register(
                700,
                "service",
                41,
                7,
                1,
                policy(),
                Deadline::at(100),
                endpoint(41),
            )
            .unwrap();
        registry.seal();
        assert_eq!(registry.retire_registrations(1).unwrap(), 0);
        assert_eq!(registry.query(41).unwrap().state, State::Draining);
        registry.release_endpoint_observation(41);
        assert_eq!(registry.retire_registrations(1).unwrap(), 1);
        assert_eq!(registry.query(41).unwrap().state, State::Terminal);
    }

    #[test]
    fn starting_expiry_is_automatic_and_monotonic() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal, service, 2, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        registry
            .register(
                700,
                "service",
                1,
                7,
                1,
                policy(),
                Deadline::at(10),
                endpoint(31),
            )
            .unwrap();
        assert!(matches!(
            registry.publish_ready(1, 10),
            Err(RegistryError::Expired)
        ));
        assert!(registry.expire_if_starting(1, 10).unwrap());
        let drained = registry.query(1).unwrap();
        assert_eq!(drained.state, State::Draining);
        assert_eq!(drained.reason, TerminalReason::EstablishExpired);
        registry.release_endpoint_observation(1);
        registry.retire_registrations(1).unwrap();
        assert_eq!(registry.query(1).unwrap().state, State::Terminal);
    }

    #[test]
    fn register_rejects_endpoint_rights_before_installation() {
        let (fal, service) = accounts();
        let mut registry = Registry::new(fal, service, 2, Rc::new(TestWake)).unwrap();
        install_authority(&mut registry, 700);
        let weak = TestCapability {
            id: 41,
            mailbox: 99,
            rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        };
        let failure = registry
            .register(700, "service", 1, 7, 1, policy(), Deadline::at(10), weak)
            .unwrap_err();
        assert_eq!(failure.error, RegistryError::Invalid);
        assert!(matches!(
            registry.query_name(700, "service"),
            Err(RegistryError::NotFound)
        ));
    }
}
