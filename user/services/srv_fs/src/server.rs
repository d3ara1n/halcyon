//! srv_fs 验收 provider 的长期 Runtime 装配。

use alloc::vec::Vec;
use alloc::{rc::Rc, string::String};
use erhino_shared::{
    call::SystemCallError,
    message::PAYLOAD_MAX,
    object::{Handle, HandleRole, ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Budget, Charge, Taxonomy};
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, DriveState, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step,
        Task,
    },
    wake::NotificationWake,
};
use libfal::{
    authority::{AccessSnapshot, FalRights},
    backend::{
        Backend as FalBackend, BackendError, CommitFailure, CommitResult, CreateError, CreateInput,
        LookupResult, MemoryBackend, MutationBackend as FalMutationBackend, NodeMetadata, Position,
        PreparedMutation, PreparedTake, PropertyFailure, ProviderBackend as FalProviderBackend,
        ReadError,
    },
    bytes::Writer,
    grant::{GrantTable, Issuance},
    node::{NodeKind, validate_path},
    protocol, provider,
    resource::FalResource,
    route,
    store::{NodeId, NodeRef, RetireProgress},
    value::{
        ExportMode, ExportPolicy, Protocol as ValueProtocol, ReadSnapshot, SnapshotHandle,
        StoredHandle, StoredValue, TakenValue,
    },
    watch::{self, ControlError, Effect, Effects, WakeBatch},
};
use librpc::dispatcher::{Completion, Dispatcher};
use librpc::{CallCause, CallError, Outbox, OutboxResult, Request as RpcRequest, RequestContext};
use libservice::{
    protocol as service_protocol,
    registry::{Registry, RegistryError},
    resource::ServiceResource,
};
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MessageStorage, ReceiveBuffer},
    notification,
    object::{duplicate, query},
    packet::Packet,
    wait::wait_until,
    wait_set::WaitSet,
};

mod authority;
use authority::AuthorityTask;
mod registration;
use registration::{IssuedRegistration, PendingRegistration, RegistrationReplyTask};
mod stream;
use stream::{OpenArgs, StreamControlTask, StreamTable, StreamTask};
const KIND_MAILBOX: SourceKind = 1;
const KIND_REPLY: SourceKind = 11;
const KIND_GRANT_LIFETIME: SourceKind = 2;
const KIND_RETIRE: SourceKind = 3;
const KIND_ROUTE: SourceKind = 4;
const KIND_RELEASE: SourceKind = 5;
const KIND_WATCH_OWNER: SourceKind = 6;
const KIND_REGISTRATION: SourceKind = 7;
const KIND_REGISTRATION_LIFETIME: SourceKind = 8;
const KIND_REGISTRATION_ENDPOINT: SourceKind = 9;
const KIND_AUTHORITY_LIFETIME: SourceKind = 10;
const KIND_STREAM_LIFETIME: SourceKind = 12;
const KIND_STREAM_DATA: SourceKind = 13;
const MEMORY_NODE_LIMIT: usize = 64;
const REGISTRATION_LIMIT: usize = 16;
const GRANT_LIMIT: usize = 16;
const AUTHORITY_LIMIT: usize = REGISTRATION_LIMIT;
const WATCH_LIMIT: usize = 24;
const REQUEST_HEADROOM: usize = 16;
const STREAM_LIMIT: usize = REQUEST_HEADROOM - 1;
const DISPATCH_LIMIT: usize = 8;
const RETIRE_BIT: u64 = 1;

// 固定任务：retire、dispatcher、FAL ingress、route、release，以及 A 的注册 ingress
// 或 B 的注册 client；其余按各域最多同时存活的任务计算。
const TASK_LIMIT: usize = 6
    + GRANT_LIMIT
    + AUTHORITY_LIMIT
    + REGISTRATION_LIMIT
    + WATCH_LIMIT
    + REQUEST_HEADROOM
    + 2 * STREAM_LIMIT
    + DISPATCH_LIMIT;
// 固定来源含 dispatcher 的 2D+1；请求余量按回复与下游两来源预留。
const SOURCE_LIMIT: usize = 7
    + (2 * DISPATCH_LIMIT + 1)
    + 2 * GRANT_LIMIT
    + 2 * AUTHORITY_LIMIT
    + 3 * REGISTRATION_LIMIT
    + 2 * WATCH_LIMIT
    + 2 * REQUEST_HEADROOM
    + 4 * STREAM_LIMIT
    + DISPATCH_LIMIT;

struct DispatchSubmission {
    waiter: u64,
    service: Capability,
    deadline: Deadline,
    request: RpcRequest,
}

enum DispatchIntent {
    Submit(DispatchSubmission),
    Cancel { txid: u64 },
}

enum RegistrationEndpoint {
    Authority(Mailbox),
    Client(MailboxSender),
}

struct ServiceBackend {
    memory: MemoryBackend<Capability>,
    registry: Option<Registry<Capability>>,
    route: Option<route::Binding<Capability>>,
}

impl ServiceBackend {
    fn new(
        fal_account: &AccountView<FalResource>,
        service_account: &AccountView<ServiceResource>,
        wake: Rc<dyn libexecution::wake::Wake>,
        registry: bool,
    ) -> Result<Self, BackendError> {
        let mut memory = MemoryBackend::new(fal_account, MEMORY_NODE_LIMIT, wake.clone())?;
        let registry = match registry
            .then(|| {
                Registry::new(
                    fal_account.clone(),
                    service_account.clone(),
                    REGISTRATION_LIMIT,
                    wake,
                )
            })
            .transpose()
        {
            Ok(registry) => registry,
            Err(error) => {
                // 尚未发布的 Memory 只有无能力的根，seal 后可有界退休。
                memory.seal();
                while !memory.is_empty() {
                    memory
                        .retire_step(1)
                        .expect("unpublished memory root retirement failed");
                }
                assert!(
                    memory.close().is_ok(),
                    "unpublished memory root retained an owner"
                );
                return Err(registry_backend_error(error));
            }
        };
        Ok(Self {
            memory,
            registry,
            route: None,
        })
    }

    fn registry_root(&self) -> Option<NodeRef> {
        self.registry.as_ref()?.root().cloned()
    }

    fn registry_for_access(&self, access: &AccessSnapshot) -> Option<&Registry<Capability>> {
        let registry = self.registry.as_ref()?;
        registry.owns_node(access.root()).then_some(registry)
    }

    fn registry_for_ref(&self, reference: &NodeRef) -> Option<&Registry<Capability>> {
        let registry = self.registry.as_ref()?;
        registry.owns_node(reference).then_some(registry)
    }

    fn bind_route(
        &mut self,
        name: &str,
        target: MailboxSender,
        rights: FalRights,
    ) -> Result<(), BackendError> {
        let root = self.memory.root().ok_or(BackendError::Closed)?;
        let name = copy_request_name(name).map_err(BackendError::Resource)?;
        self.route = Some(route::Binding::new(
            root,
            name,
            target.into_capability(),
            rights,
        ));
        Ok(())
    }
}

fn registry_backend_error(error: RegistryError) -> BackendError {
    match error {
        RegistryError::Closed => BackendError::Closed,
        RegistryError::Permission => BackendError::Permission,
        RegistryError::Invalid => BackendError::InvalidName,
        RegistryError::Exists => BackendError::Exists,
        RegistryError::NotFound => BackendError::NotFound,
        RegistryError::Conflict | RegistryError::Expired => BackendError::Conflict,
        RegistryError::Resource(error) => BackendError::Resource(error),
    }
}

impl FalBackend<Capability> for ServiceBackend {
    fn root(&self) -> Option<&NodeRef> {
        self.memory.root()
    }

    fn resolve(&self, access: &AccessSnapshot, path: &str) -> Result<NodeRef, BackendError> {
        if let Some(registry) = self.registry_for_access(access) {
            registry.resolve(access, path)
        } else {
            self.memory.resolve(access, path)
        }
    }

    fn lookup(
        &self,
        access: &AccessSnapshot,
        path: &str,
    ) -> Result<LookupResult<Capability>, BackendError> {
        if let Some(registry) = self.registry_for_access(access) {
            return registry.lookup(access, path);
        }
        if let Some(binding) = self.route.as_ref()
            && let Some(boundary) = binding.lookup(access, path)?
        {
            return Ok(boundary);
        }
        self.memory.lookup(access, path)
    }

    fn metadata(
        &self,
        reference: &NodeRef,
        ceiling: FalRights,
    ) -> Result<NodeMetadata, BackendError> {
        if let Some(registry) = self.registry_for_ref(reference) {
            registry.metadata(reference, ceiling)
        } else {
            self.memory.metadata(reference, ceiling)
        }
    }

    fn enumerate<F>(
        &self,
        parent: &NodeRef,
        access: &AccessSnapshot,
        cursor: u64,
        limit: usize,
        visit: F,
    ) -> Result<u64, BackendError>
    where
        F: FnMut(&str, &NodeRef, NodeMetadata),
    {
        if let Some(registry) = self.registry_for_access(access) {
            registry.enumerate(parent, access, cursor, limit, visit)
        } else {
            FalBackend::enumerate(&self.memory, parent, access, cursor, limit, visit)
        }
    }

    fn read_snapshot(
        &self,
        access: &AccessSnapshot,
        path: &str,
    ) -> Result<ReadSnapshot<Capability>, ReadError> {
        if let Some(registry) = self.registry_for_access(access) {
            registry.read_snapshot(access, path)
        } else {
            self.memory.read_snapshot(access, path)
        }
    }

    fn watch_snapshot(
        &self,
        access: &AccessSnapshot,
        path: &str,
    ) -> Result<(NodeRef, u64), BackendError> {
        if let Some(registry) = self.registry_for_access(access) {
            registry.watch_snapshot(access, path)
        } else {
            self.memory.watch_snapshot(access, path)
        }
    }

    fn seal(&mut self) {
        self.route = None;
        self.memory.seal();
        if let Some(registry) = self.registry.as_mut() {
            registry.seal();
        }
    }

    fn has_retire_work(&self) -> bool {
        self.memory.has_retire_work()
            || self
                .registry
                .as_ref()
                .is_some_and(FalBackend::has_retire_work)
    }

    fn retire_step(&mut self, budget: usize) -> Result<RetireProgress, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let memory = self.memory.retire_step(budget)?;
        if memory.work_done == budget {
            return Ok(RetireProgress {
                work_done: memory.work_done,
                done: !self.has_retire_work(),
            });
        }
        let registry = if let Some(registry) = self.registry.as_mut() {
            registry.retire_step(budget - memory.work_done)?
        } else {
            RetireProgress {
                work_done: 0,
                done: true,
            }
        };
        Ok(RetireProgress {
            work_done: memory.work_done + registry.work_done,
            done: memory.done && registry.done,
        })
    }

    fn is_empty(&self) -> bool {
        self.route.is_none()
            && self.memory.is_empty()
            && self.registry.as_ref().is_none_or(FalBackend::is_empty)
    }

    fn close(self) -> Result<(), Self> {
        if !self.is_empty() {
            return Err(self);
        }
        let Self {
            memory,
            registry,
            route: _,
        } = self;
        let Ok(()) = memory.close() else {
            unreachable!("empty memory backend refused close")
        };
        if let Some(registry) = registry {
            let Ok(()) = registry.close() else {
                unreachable!("empty Registry backend refused close")
            };
        }
        Ok(())
    }
}

impl FalMutationBackend<Capability> for ServiceBackend {
    type TakeReservation = PreparedTake<Capability>;
    type Position = Position;
    type Mutation = PreparedMutation<Capability>;
    fn lookup_child(
        &self,
        parent: &NodeRef,
        name: &str,
        access: &AccessSnapshot,
    ) -> Result<NodeRef, BackendError> {
        if let Some(registry) = self.registry_for_ref(parent) {
            if registry.root().ok_or(BackendError::Closed)?.id() != parent.id() {
                return Err(BackendError::NotDirectory);
            }
            registry.resolve(access, name)
        } else {
            self.memory.lookup_child(parent, name, access)
        }
    }

    fn read_stream(
        &self,
        reference: &NodeRef,
        access: &AccessSnapshot,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BackendError> {
        if self.registry_for_ref(reference).is_some() {
            Err(BackendError::WrongType)
        } else {
            self.memory.read_stream(reference, access, offset, buffer)
        }
    }

    fn property(&self, reference: &NodeRef) -> Result<&StoredValue<Capability>, BackendError> {
        if self.registry_for_ref(reference).is_some() {
            Err(BackendError::WrongType)
        } else {
            self.memory.property(reference)
        }
    }

    fn position(
        &self,
        parent: &NodeRef,
        final_name: &str,
        expected: Option<(NodeId, u64)>,
    ) -> Result<Self::Position, BackendError> {
        if self.registry_for_ref(parent).is_some() {
            Err(BackendError::Permission)
        } else {
            self.memory.position(parent, final_name, expected)
        }
    }

    fn prepare_create(
        &self,
        position: Self::Position,
        access: &AccessSnapshot,
        input: CreateInput<'_>,
        owners: &mut Vec<Capability>,
        rights: FalRights,
    ) -> Result<Self::Mutation, CreateError> {
        if self.registry_for_access(access).is_some() {
            Err(CreateError::Backend(BackendError::Permission))
        } else {
            self.memory
                .prepare_create_input(position, access, input, owners, rights)
        }
    }

    fn prepare_delete(
        &self,
        position: Self::Position,
        access: &AccessSnapshot,
    ) -> Result<Self::Mutation, BackendError> {
        self.memory.prepare_delete(position, access)
    }

    fn prepare_property(
        &self,
        target: NodeRef,
        access: &AccessSnapshot,
        value: StoredValue<Capability>,
    ) -> Result<Self::Mutation, PropertyFailure<Capability>> {
        if self.registry_for_ref(&target).is_some() {
            Err(PropertyFailure {
                error: BackendError::Permission,
                value,
            })
        } else {
            self.memory.prepare_property(target, access, value)
        }
    }

    fn prepare_write(
        &self,
        target: NodeRef,
        access: &AccessSnapshot,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Self::Mutation, BackendError> {
        if self.registry_for_ref(&target).is_some() {
            Err(BackendError::Permission)
        } else {
            self.memory.prepare_write(target, access, offset, bytes)
        }
    }

    fn prepare_move(
        &self,
        source: Self::Position,
        source_access: &AccessSnapshot,
        destination: &AccessSnapshot,
        final_name: &str,
    ) -> Result<Self::Mutation, BackendError> {
        let source_registry = self.registry_for_ref(source.parent()).is_some();
        let destination_registry = self.registry_for_access(destination).is_some();
        if source_registry != destination_registry {
            Err(BackendError::CrossDevice)
        } else if source_registry {
            Err(BackendError::Permission)
        } else {
            self.memory
                .prepare_move(source, source_access, destination, final_name)
        }
    }

    fn validate_move_step(
        &self,
        mutation: &mut Self::Mutation,
        budget: usize,
    ) -> Result<bool, BackendError> {
        self.memory.validate_move_step(mutation, budget)
    }

    fn commit(
        &mut self,
        mutation: Self::Mutation,
    ) -> Result<CommitResult, CommitFailure<Self::Mutation>> {
        self.memory.commit(mutation)
    }

    fn prepare_take(
        &mut self,
        target: NodeRef,
        access: &AccessSnapshot,
    ) -> Result<Self::TakeReservation, BackendError> {
        if self.registry_for_ref(&target).is_some() {
            Err(BackendError::Permission)
        } else {
            self.memory.prepare_take(target, access)
        }
    }

    fn take_value(prepared: &mut Self::TakeReservation) -> TakenValue<Capability> {
        MemoryBackend::take_value(prepared)
    }

    fn commit_take(&mut self, prepared: Self::TakeReservation) {
        self.memory.commit_take(prepared);
    }

    fn rollback_take(&mut self, prepared: Self::TakeReservation, value: TakenValue<Capability>) {
        self.memory.rollback_take(prepared, value);
    }
}

trait RuntimeBackend: FalProviderBackend<Capability> {
    fn registry(&self) -> Option<&Registry<Capability>>;
    fn registry_mut(&mut self) -> Option<&mut Registry<Capability>>;
    fn bind_route(
        &mut self,
        name: &str,
        target: MailboxSender,
        rights: FalRights,
    ) -> Result<(), BackendError>;
}

impl RuntimeBackend for ServiceBackend {
    fn registry(&self) -> Option<&Registry<Capability>> {
        self.registry.as_ref()
    }

    fn registry_mut(&mut self) -> Option<&mut Registry<Capability>> {
        self.registry.as_mut()
    }

    fn bind_route(
        &mut self,
        name: &str,
        target: MailboxSender,
        rights: FalRights,
    ) -> Result<(), BackendError> {
        ServiceBackend::bind_route(self, name, target, rights)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RegistrationControl {
    instance: u64,
    task_id: u64,
}

struct World<B = ServiceBackend> {
    provider: provider::State<B>,
    route_mailbox: Handle,
    dispatch_intent: Option<DispatchIntent>,
    dispatcher_task: u64,
    root_grant_task: u64,
    registry_grant_task: Option<u64>,
    registration_endpoint: Option<RegistrationEndpoint>,
    registration_ready: bool,
    root_sender: Option<MailboxSender>,
    registry_sender: Option<MailboxSender>,
    registration_root_sender: Option<MailboxSender>,
    registration_controls: Vec<RegistrationControl>,
    streams: StreamTable,
    downstream_abandoned: u64,
    stop: bool,
    failed: bool,
}

impl<B> World<B> {
    fn publish_initial_grant(&mut self, task_id: u64, sender: MailboxSender) {
        if task_id == self.root_grant_task {
            assert!(
                self.root_sender.replace(sender).is_none(),
                "root grant published more than once"
            );
        } else if Some(task_id) == self.registry_grant_task {
            assert!(
                self.registry_sender.replace(sender).is_none(),
                "Registry root grant published more than once"
            );
        } else {
            panic!("unexpected initial grant task");
        }
    }
}

impl<B: RuntimeBackend> provider::Host<B> for World<B> {
    fn provider(&self) -> &provider::State<B> {
        &self.provider
    }
    fn provider_mut(&mut self) -> &mut provider::State<B> {
        &mut self.provider
    }
    fn publish_initial_grant(&mut self, task_id: u64, sender: MailboxSender) {
        World::publish_initial_grant(self, task_id, sender);
    }
    fn stopping(&self) -> bool {
        self.stop
    }
    fn fail(&mut self) {
        self.failed = true;
    }
}

type WatchTask = provider::Watch;

impl<B: RuntimeBackend> Task<World<B>> for WatchTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        provider::Watch::advance(self, id, world, requests, input, budget)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        provider::Watch::refused(self, world, failure);
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        provider::Watch::registered(self, world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        provider::Watch::unregistered(self, world, kind, source);
    }

    fn stop(&mut self, _world: &mut World<B>) {
        provider::Watch::stop(self);
    }

    fn deadline(&self) -> Deadline {
        provider::Watch::deadline(self)
    }
}

type GrantTask = provider::Grant;

impl<B: RuntimeBackend> Task<World<B>> for GrantTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        provider::Grant::advance(self, id, world, requests, input, budget)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        provider::Grant::refused(self, world, failure);
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        provider::Grant::registered(self, world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        provider::Grant::unregistered(self, world, kind, source);
    }

    fn stop(&mut self, _world: &mut World<B>) {
        provider::Grant::stop(self);
    }

    fn deadline(&self) -> Deadline {
        provider::Grant::deadline(self)
    }
}

struct RetireTask(provider::Retirement);

impl RetireTask {
    fn new(owner: Capability) -> Self {
        Self(provider::Retirement::new(owner, KIND_RETIRE, RETIRE_BIT))
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RetireTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let (provider, failed) = (&mut world.provider, &mut world.failed);
        self.0
            .advance(provider, requests, input, budget, || *failed = true)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        if self.0.refused(failure) && !world.stop {
            world.failed = true;
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.0.registered(&mut world.provider, kind, source);
    }

    fn unregistered(&mut self, _world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.0.unregistered(kind, source);
    }

    fn stop(&mut self, _world: &mut World<B>) {
        self.0.stop();
    }
}

struct ReleaseTask {
    owner: Option<Capability>,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    released: bool,
}

impl ReleaseTask {
    fn new(owner: Capability) -> Self {
        Self {
            owner: Some(owner),
            source: None,
            requested: false,
            removing: false,
            released: false,
        }
    }
}

impl<B: RuntimeBackend> Task<World<B>> for ReleaseTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.error != 0 {
                world.failed = true;
            } else if event.observed.intersects(ObjectSignals::READABLE) {
                let owner = self.owner.as_ref().expect("release source lost its owner");
                notification::take(owner.as_handle(), 1)?;
                self.released = true;
                world.stop = true;
            } else if event.observed.intersects(ObjectSignals::CLOSED) {
                world.failed = true;
            }
        }
        if self.released {
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            if self.source.is_none() && !self.requested {
                let owner = self.owner.take().expect("release owner already closed");
                owner.close().map_err(|(_, error)| error)?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                self.owner
                    .as_ref()
                    .expect("release task lost its owner")
                    .as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_RELEASE,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World<B>, _failure: RequestFailure<ServiceTask<B>>) {
        world.failed = true;
    }

    fn registered(&mut self, _world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_RELEASE {
            self.requested = false;
            self.source = Some(source);
        }
    }

    fn unregistered(&mut self, _world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_RELEASE && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World<B>) {
        self.released = true;
    }
}

struct RouteIngress {
    buffer: ReceiveBuffer,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
}

impl RouteIngress {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new().expect("route receive buffer creation failed"),
            source: None,
            requested: false,
            removing: false,
            stopping: false,
        }
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RouteIngress {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_none() && !self.requested {
                    Step::Complete
                } else {
                    Step::Parked
                },
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                world.route_mailbox,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_ROUTE,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                self.stopping = true;
            } else {
                ready = true;
            }
        }
        if self.stopping {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        match self.buffer.receive(world.route_mailbox) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                requests.rearm(self.source.expect("route source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        let message = MessageStorage::new()?.take(&mut self.buffer)?;
        let context = match RequestContext::decode(message, route::ID) {
            Ok(context) => context,
            Err(_) => {
                requests.rearm(self.source.expect("route source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let outbox = Outbox::prepare(context, route::RESPONSE_LEN, Deadline::INFINITE, KIND_REPLY)
            .map_err(|_| SystemCallError::InternalError)?;
        requests
            .spawn(
                ServiceTask::RouteReply(RouteReplyTask {
                    outbox,
                    encoded: false,
                    recorded: false,
                }),
                1,
            )
            .map_err(|_| SystemCallError::ReachLimit)?;
        requests.rearm(self.source.expect("route source remains registered"))?;
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World<B>, _failure: RequestFailure<ServiceTask<B>>) {
        world.failed = true;
    }

    fn registered(&mut self, _world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_ROUTE {
            self.requested = false;
            self.source = Some(source);
        }
    }

    fn unregistered(&mut self, _world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_ROUTE && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World<B>) {
        self.stopping = true;
    }
}

struct RouteReplyTask {
    outbox: Outbox,
    encoded: bool,
    recorded: bool,
}

impl RouteReplyTask {
    fn bind<B: RuntimeBackend>(
        world: &mut World<B>,
        context: &mut RequestContext,
    ) -> route::Status {
        if context.handles.remaining() != 1 {
            return route::Status::Invalid;
        }
        let Ok(binding) = route::Bind::decode(&context.payload) else {
            return route::Status::Invalid;
        };
        if !binding.rights.contains(FalRights::TRAVERSE) {
            return route::Status::Invalid;
        }
        let Ok(capability) = context.handles.get(1) else {
            return route::Status::Invalid;
        };
        let rights = Rights::WRITE | Rights::WAIT | Rights::DUPLICATE;
        let Ok(description) = capability.description() else {
            return route::Status::Permission;
        };
        if !description.rights.contains(rights) {
            return route::Status::Permission;
        }
        let capability = context
            .handles
            .take(1)
            .expect("validated route target disappeared");
        let Ok((target, _)) = MailboxSender::from_capability(capability) else {
            return route::Status::Invalid;
        };
        match world.provider.backend.as_mut() {
            Some(backend) => backend
                .bind_route(binding.name, target, binding.rights)
                .map_or(route::Status::Internal, |_| route::Status::Ok),
            None => route::Status::Internal,
        }
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RouteReplyTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<Self::Family>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if !self.encoded && self.outbox.result().is_none() {
            if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            }
            let status = {
                let (context, _) = self.outbox.response_mut()?.parts()?;
                Self::bind(world, context)
            };
            let used = route::encode_status(status, self.outbox.response_mut()?.body_mut()?)
                .ok_or(SystemCallError::InternalError)?;
            self.outbox.response_mut()?.finish_body(used)?;
            self.encoded = true;
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete && self.encoded && !self.recorded {
            record_provider_response(world, &self.outbox, true)?;
            self.recorded = true;
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<Self::Family>) {
        self.outbox.refused(world, failure);
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

enum PendingFalTask<B: RuntimeBackend> {
    Request(RequestTask<B>),
    Read(ReadTask),
    Grant(GrantTask),
    Watch(WatchTask),
    Delegate(DelegateTask),
    Stream(StreamTask),
    StreamControl(StreamControlTask),
}

impl<B: RuntimeBackend> PendingFalTask<B> {
    fn into_service(self) -> ServiceTask<B> {
        match self {
            Self::Request(task) => ProviderTask::Request(task).into(),
            Self::Read(task) => ProviderTask::Read(task).into(),
            Self::Grant(task) => ProviderTask::Grant(task).into(),
            Self::Watch(task) => ProviderTask::Watch(task).into(),
            Self::Delegate(task) => ProviderTask::Delegate(task).into(),
            Self::Stream(task) => ServiceTask::Stream(task),
            Self::StreamControl(task) => ServiceTask::StreamControl(task),
        }
    }
}

struct Ingress<B: RuntimeBackend> {
    buffer: ReceiveBuffer,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
    gate_pending: bool,
    queued: Option<(PendingFalTask<B>, usize)>,
    refusal: Option<Outbox>,
    refusal_deadline: Deadline,
}

impl<B: RuntimeBackend> Ingress<B> {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new().expect("provider receive buffer creation failed"),
            source: None,
            requested: false,
            removing: false,
            stopping: false,
            gate_pending: false,
            queued: None,
            refusal: None,
            refusal_deadline: Deadline::INFINITE,
        }
    }
    fn reject_task(&mut self, task: ServiceTask<B>, error: SystemCallError, world: &mut World<B>) {
        let mut outbox = task.into_fal_refusal();
        self.gate_pending = false;
        let status = match error {
            SystemCallError::QuotaExceeded | SystemCallError::ReachLimit => protocol::Status::Quota,
            SystemCallError::OutOfMemory => protocol::Status::Resource,
            _ => protocol::Status::Cancelled,
        };
        if self.stopping {
            outbox.stop(world);
        } else {
            let cap =
                rinlib::time::timeout_millis(5_000).expect("FAL refusal deadline unavailable");
            let original = outbox
                .deadline()
                .instant()
                .expect("FAL refusal request deadline invalid");
            self.refusal_deadline =
                Deadline::at(original.map_or(cap.at_ns, |at| at.min(cap.at_ns)));
            let response = outbox.response_mut().expect("FAL refusal lost reply owner");
            let (context, body) = response.parts().expect("FAL refusal lost request body");
            let (header, _) = protocol::decode_request(&context.payload)
                .expect("FAL refusal lost validated request");
            let used = encode_v2(
                body,
                header.op,
                status,
                header.deadline,
                protocol::Response::Empty,
            )
            .expect("FAL refusal reply layout invalid");
            response
                .finish_body(used)
                .expect("FAL refusal reply length invalid");
        }
        self.refusal = Some(outbox);
    }
}

impl<B: RuntimeBackend> Task<World<B>> for Ingress<B> {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.kind == KIND_REPLY {
                if let Some(outbox) = self.refusal.as_mut() {
                    outbox.observe(event);
                }
            } else if event.kind == KIND_MAILBOX {
                if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                    world.failed = true;
                    self.stopping = true;
                } else {
                    ready = true;
                }
            } else {
                return Err(SystemCallError::InternalError);
            }
        }
        if input.take_timeout()
            && let Some(outbox) = self.refusal.as_mut()
        {
            outbox.timed_out();
        }
        if self.stopping {
            self.queued = None;
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            if let Some(outbox) = self.refusal.as_mut() {
                outbox.stop(world);
                let advance = outbox.drive(requests, budget)?;
                if !outbox.is_complete() {
                    return Ok(advance);
                }
                self.refusal = None;
                self.refusal_deadline = Deadline::INFINITE;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_none() && !self.requested && !self.gate_pending {
                    Step::Complete
                } else {
                    Step::Parked
                },
            });
        }
        if let Some(outbox) = self.refusal.as_mut() {
            let advance = if outbox.result().is_none() && !outbox.is_admitted() {
                outbox.admit(requests)
            } else {
                outbox.drive(requests, budget)?
            };
            if !outbox.is_complete() {
                return Ok(advance);
            }
            self.refusal = None;
            self.refusal_deadline = Deadline::INFINITE;
            requests.rearm(self.source.expect("provider source remains registered"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if let Some((task, max_sources)) = self.queued.take() {
            match requests.spawn(task.into_service(), max_sources) {
                Ok(()) => self.gate_pending = true,
                Err(task) => self.queued = Some((task.into_fal_pending(), max_sources)),
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.gate_pending {
            self.gate_pending = false;
            requests.rearm(self.source.expect("provider source remains registered"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                world.provider.mailbox.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_MAILBOX,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        match self.buffer.receive(world.provider.mailbox.as_handle()) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        let message = MessageStorage::new()?.take(&mut self.buffer)?;
        let mut context = match RequestContext::decode(message, protocol::ID) {
            Ok(context) => context,
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let parsed = protocol::decode_request(&context.payload);
        let (header, request) = match parsed {
            Ok((header, request)) => {
                let expected_handles = match request {
                    protocol::Request::Move { .. } | protocol::Request::Subscribe { .. } => Some(1),
                    protocol::Request::Create {
                        kind: NodeKind::Property,
                        ..
                    }
                    | protocol::Request::Write { .. } => None,
                    _ => Some(0),
                };
                if expected_handles.is_none()
                    || context.handles.remaining() == expected_handles.unwrap()
                {
                    (header, request)
                } else {
                    requests.rearm(self.source.expect("provider source remains registered"))?;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    });
                }
            }
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        if matches!(
            request,
            protocol::Request::Start
                | protocol::Request::QueryStream
                | protocol::Request::FinishStream
                | protocol::Request::CancelStream
        ) {
            let identity = context.envelope.sender_context_id;
            let session_end = world.streams.get(identity)
                .and_then(|entry| entry.session_deadline.instant().ok().flatten())
                .unwrap_or(input.now_ns().saturating_add(5_000_000_000));
            let reply_end = header.deadline.instant().ok().flatten()
                .unwrap_or(u64::MAX)
                .min(session_end);
            let reply_deadline = Deadline::at(reply_end);
            let outbox = match Outbox::prepare(
                context,
                protocol::HEADER_LEN + protocol::StreamInfo::ENCODED_LEN,
                reply_deadline,
                KIND_REPLY,
            ) {
                Ok(outbox) => outbox,
                Err(_) => {
                    requests.rearm(self.source.expect("provider source remains registered"))?;
                    return Ok(Advance { work_done: 1, step: Step::Parked });
                }
            };
            let task = ServiceTask::StreamControl(StreamControlTask::new(
                outbox,
                header,
                reply_deadline,
                identity,
            ));
            match requests.spawn(task, 1) {
                Ok(()) => self.gate_pending = true,
                Err(task) => self.queued = Some((task.into_fal_pending(), 1)),
            }
            return Ok(Advance { work_done: 1, step: Step::Runnable });
        }
        let Some(access) = world
            .provider
            .grants
            .as_ref()
            .expect("grant table missing during request admission")
            .snapshot(context.envelope.sender_context_id)
        else {
            requests.rearm(self.source.expect("provider source remains registered"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        };
        let sender_context = context.envelope.sender_context_id;
        if let protocol::Request::Open {
            path,
            expected_identity,
            direction,
            offset,
            length,
            session_deadline,
            stream_protocol,
            tunnel_bytes,
        } = request
        {
            let args = copy_request_name(path)
                .map_err(|_| protocol::Status::Resource)
                .map(|path| OpenArgs {
                    path,
                    expected_identity,
                    direction,
                    offset,
                    length,
                    session_deadline,
                    stream_protocol,
                    tunnel_bytes,
                });
            let reply_end = input.now_ns().saturating_add(5_000_000_000).min(
                header.deadline.instant().ok().flatten().unwrap_or(u64::MAX),
            );
            let reply_deadline = Deadline::at(reply_end);
            let outbox = match Outbox::prepare(
                context,
                protocol::HEADER_LEN + protocol::StreamOffer::ENCODED_LEN,
                reply_deadline,
                KIND_REPLY,
            ) {
                Ok(outbox) => outbox,
                Err(_) => {
                    requests.rearm(self.source.expect("provider source remains registered"))?;
                    return Ok(Advance { work_done: 1, step: Step::Parked });
                }
            };
            let task = ServiceTask::Stream(StreamTask::new(outbox, header, reply_deadline, access, args));
            match requests.spawn(task, 3) {
                Ok(()) => self.gate_pending = true,
                Err(task) => self.queued = Some((task.into_fal_pending(), 3)),
            }
            return Ok(Advance { work_done: 1, step: Step::Runnable });
        }
        let mut read_failure = None;
        let read_snapshot = if let protocol::Request::Read { path } = request {
            let backend = world
                .provider
                .backend
                .as_ref()
                .expect("provider backend missing during Read preparation");
            match FalBackend::read_snapshot(backend, &access, path) {
                Ok(snapshot) => Some(snapshot),
                Err(ReadError::Backend(error)) => {
                    read_failure = Some(backend_status(error));
                    None
                }
                Err(ReadError::Value(error)) => {
                    read_failure = Some(value_status(error));
                    None
                }
            }
        } else {
            None
        };
        let mut watch_failure = None;
        let watch_owner = if let protocol::Request::Subscribe { path, mask } = request {
            let prepared = (|| {
                let capability = context
                    .handles
                    .take(1)
                    .map_err(|_| protocol::Status::Invalid)?;
                let description = capability
                    .description()
                    .map_err(|_| protocol::Status::Internal)?;
                let required = Rights::SIGNAL | Rights::WAIT | Rights::TRANSIT;
                if description.role != HandleRole::NotificationSignaler as u32
                    || !description.rights.contains(required)
                {
                    return Err(protocol::Status::Invalid);
                }
                let backend = world
                    .provider
                    .backend
                    .as_ref()
                    .expect("provider backend missing during Watch preparation");
                let (node, _) =
                    FalBackend::watch_snapshot(backend, &access, path).map_err(backend_status)?;
                let mut watch_path = String::new();
                watch_path
                    .try_reserve_exact(path.len())
                    .map_err(|_| protocol::Status::Resource)?;
                watch_path.push_str(path);
                let watch_charge = access.account().acquire(FalResource::Watch, 1).map_err(
                    |error| match error {
                        SystemCallError::QuotaExceeded => protocol::Status::Quota,
                        SystemCallError::OutOfMemory => protocol::Status::Resource,
                        _ => protocol::Status::Internal,
                    },
                )?;
                let source_charge = access
                    .account()
                    .acquire(FalResource::WaitSource, 1)
                    .map_err(|error| match error {
                        SystemCallError::QuotaExceeded => protocol::Status::Quota,
                        SystemCallError::OutOfMemory => protocol::Status::Resource,
                        _ => protocol::Status::Internal,
                    })?;
                let id = world
                    .provider
                    .watches
                    .allocate_id()
                    .ok_or(protocol::Status::Resource)?;
                Ok(provider::WatchOwner::new(
                    id,
                    sender_context,
                    node,
                    access.clone(),
                    watch_path,
                    mask,
                    capability,
                    watch_charge,
                    source_charge,
                ))
            })();
            match prepared {
                Ok(owner) => Some(owner),
                Err(status) => {
                    watch_failure = Some(status);
                    None
                }
            }
        } else {
            None
        };
        let mut move_failure = None;
        let move_destination = if matches!(request, protocol::Request::Move { .. }) {
            let capability = context
                .handles
                .take(1)
                .map_err(|_| SystemCallError::IllegalArgument)?;
            match world
                .provider
                .grants
                .as_ref()
                .expect("grant table missing during request admission")
                .validate_received(&capability)
            {
                Ok(destination) => Some(destination),
                Err(error) => {
                    move_failure = Some(match error {
                        libfal::grant::GrantError::CrossDevice => protocol::Status::CrossDevice,
                        libfal::grant::GrantError::Revoked => protocol::Status::GrantRevoked,
                        libfal::grant::GrantError::WrongRole => protocol::Status::Invalid,
                        libfal::grant::GrantError::Transport(_) => protocol::Status::Internal,
                    });
                    None
                }
            }
        } else {
            None
        };
        let mut delegate_failure = None;
        let delegated = if let protocol::Request::Lookup { path } = request {
            let backend = world
                .provider
                .backend
                .as_ref()
                .expect("provider backend missing during lookup");
            match backend.lookup(&access, path) {
                Ok(LookupResult::DelegationBoundary {
                    target,
                    rights,
                    consumed,
                    remaining,
                }) => match prepare_delegate(target, rights, consumed, remaining, header) {
                    Ok(delegate) => Some(delegate),
                    Err(status) => {
                        delegate_failure = Some(status);
                        None
                    }
                },
                Ok(_) => None,
                Err(error) => {
                    delegate_failure = Some(backend_status(error));
                    None
                }
            }
        } else {
            None
        };
        let mut derive_failure = None;
        let derived = if let protocol::Request::Derive { path, rights } = request {
            let prepared = (|| {
                if !access.rights().contains(rights) {
                    return Err(protocol::Status::Permission);
                }
                let backend = world
                    .provider
                    .backend
                    .as_ref()
                    .expect("provider backend missing during grant derivation");
                let root = resolve_v2(backend, &access, path).map_err(backend_status)?;
                let info = node_info_v2(backend, &root, rights).map_err(backend_status)?;
                if info.kind != NodeKind::Directory {
                    return Err(protocol::Status::NotDirectory);
                }
                let prepared = world
                    .provider
                    .grants
                    .as_mut()
                    .expect("grant table missing during derivation")
                    .prepare_derive(
                        &world.provider.mailbox,
                        context.envelope.sender_context_id,
                        root,
                        rights,
                        context.envelope.sender_context_id,
                    )
                    .map_err(|failure| match failure.error {
                        SystemCallError::RightsDenied => protocol::Status::Permission,
                        SystemCallError::QuotaExceeded => protocol::Status::Quota,
                        SystemCallError::OutOfMemory => protocol::Status::Resource,
                        SystemCallError::ObjectClosed => protocol::Status::GrantRevoked,
                        _ => protocol::Status::Internal,
                    })?;
                Ok((prepared, info))
            })();
            match prepared {
                Ok(prepared) => Some(prepared),
                Err(status) => {
                    derive_failure = Some(status);
                    None
                }
            }
        } else {
            None
        };
        let move_operation = if let protocol::Request::Move {
            source_parent,
            source_name,
            destination_name,
            expected,
        } = request
        {
            let expected = if expected.identity == 0 {
                if expected.version != 0 {
                    move_failure = Some(protocol::Status::Invalid);
                    None
                } else {
                    None
                }
            } else {
                match NodeId::from_raw(expected.identity) {
                    Some(identity) => Some((identity, expected.version)),
                    None => {
                        move_failure = Some(protocol::Status::Invalid);
                        None
                    }
                }
            };
            match move_destination {
                Some(destination) if move_failure.is_none() => {
                    let prepared = (|| {
                        Ok::<_, SystemCallError>(MoveOperation {
                            access: access.clone(),
                            destination,
                            header,
                            source_parent: copy_request_name(source_parent)?,
                            source_name: copy_request_name(source_name)?,
                            destination_name: copy_request_name(destination_name)?,
                            expected,
                            prepared: None,
                            source_parent_ref: None,
                            target_ref: None,
                        })
                    })();
                    match prepared {
                        Ok(operation) => Some(operation),
                        Err(_) => {
                            move_failure = Some(protocol::Status::Resource);
                            None
                        }
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let mut take_failure = None;
        let take_operation = if let protocol::Request::Take { path } = request {
            copy_request_name(path).map_or_else(
                |_| {
                    take_failure = Some(protocol::Status::Resource);
                    None
                },
                |path| {
                    Some(TakeOperation {
                        access: access.clone(),
                        header,
                        path,
                        prepared: None,
                        reference: None,
                        bytes: None,
                        policies: Vec::new(),
                        restore_handles: Vec::new(),
                        encoded: false,
                        failed: false,
                    })
                },
            )
        } else {
            None
        };
        let watch_control = match request {
            protocol::Request::QuerySubscription { id } => Some(WatchControlOperation {
                context: sender_context,
                header,
                kind: WatchControlKind::Query(id),
                result: None,
                pending_wake: None,
                encoded: false,
            }),
            protocol::Request::Unsubscribe { id } => Some(WatchControlOperation {
                context: sender_context,
                header,
                kind: WatchControlKind::Unsubscribe(id),
                result: None,
                pending_wake: None,
                encoded: false,
            }),
            _ => None,
        };
        let mode = if let Some(status) = watch_failure
            .or(move_failure)
            .or(take_failure)
            .or(derive_failure)
            .or(delegate_failure)
            .or(read_failure)
        {
            RequestMode::V2Failure { header, status }
        } else if let Some(operation) = move_operation {
            RequestMode::Move(operation)
        } else if let Some(operation) = take_operation {
            RequestMode::Take(operation)
        } else if let Some(operation) = watch_control {
            RequestMode::WatchControl(operation)
        } else {
            RequestMode::V2 { access, header }
        };
        let (mode, deadline, derived, delegated) = (mode, header.deadline, derived, delegated);
        let outbox = match Outbox::prepare(
            context,
            PAYLOAD_MAX - librpc::PREFIX_LEN,
            deadline,
            KIND_REPLY,
        ) {
            Ok(outbox) => outbox,
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let (task, max_sources) = if let Some(delegate) = delegated {
            (
                ProviderTask::Delegate(delegate.with_outbox(outbox)).into(),
                1,
            )
        } else if let Some((prepared, info)) = derived {
            (
                ProviderTask::Grant(GrantTask::reply(
                    prepared,
                    outbox,
                    info,
                    KIND_REPLY,
                    KIND_GRANT_LIFETIME,
                ))
                .into(),
                2,
            )
        } else if let Some(snapshot) = read_snapshot {
            (
                ProviderTask::Read(ReadTask::new(outbox, header, snapshot)).into(),
                1,
            )
        } else if let Some(owner) = watch_owner {
            (
                ProviderTask::Watch(WatchTask::new(owner, outbox, KIND_REPLY, KIND_WATCH_OWNER))
                    .into(),
                2,
            )
        } else {
            (
                ServiceTask::Fal(ProviderTask::Request(RequestTask {
                    outbox,
                    mode,
                    committed: false,
                    recorded: false,
                    wakes: WakeBatch::new(WATCH_LIMIT).map_err(|_| SystemCallError::OutOfMemory)?,
                })),
                1,
            )
        };
        match requests.spawn(task, max_sources) {
            Ok(()) => self.gate_pending = true,
            Err(task) => self.queued = Some((task.into_fal_pending(), max_sources)),
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Runnable,
        })
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        match failure {
            RequestFailure::Spawn { task, error } => self.reject_task(task, error, world),
            failure @ RequestFailure::Source {
                kind: KIND_REPLY, ..
            } => {
                if let Some(outbox) = self.refusal.as_mut() {
                    outbox.refused(world, failure);
                }
            }
            RequestFailure::Source { .. } | RequestFailure::Wake { .. } => {
                world.failed = true;
            }
        }
    }
    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_MAILBOX {
            self.requested = false;
            self.source = Some(source);
        } else if kind == KIND_REPLY
            && let Some(outbox) = self.refusal.as_mut()
        {
            outbox.registered(world, kind, source);
        }
    }
    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_MAILBOX && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        } else if kind == KIND_REPLY
            && let Some(outbox) = self.refusal.as_mut()
        {
            outbox.unregistered(world, kind, source);
        }
    }
    fn stop(&mut self, world: &mut World<B>) {
        self.stopping = true;
        self.queued = None;
        if let Some(outbox) = self.refusal.as_mut() {
            outbox.stop(world);
        }
    }
    fn deadline(&self) -> Deadline {
        self.refusal_deadline
    }
}

fn publish_watch_events<B: RuntimeBackend>(
    world: &mut World<B>,
    effects: &Effects,
    wakes: &mut WakeBatch,
) {
    if !effects.is_empty() {
        world.provider.watches.publish(effects, wakes);
    }
}

fn backend_status(error: BackendError) -> protocol::Status {
    match error {
        BackendError::NotFound => protocol::Status::NotFound,
        BackendError::NotDirectory => protocol::Status::NotDirectory,
        BackendError::Permission => protocol::Status::Permission,
        BackendError::Exists => protocol::Status::Exists,
        BackendError::NotEmpty => protocol::Status::NotEmpty,
        BackendError::Conflict => protocol::Status::Conflict,
        BackendError::InvalidName | BackendError::Cycle => protocol::Status::Invalid,
        BackendError::WrongType => protocol::Status::Invalid,
        BackendError::Unsupported => protocol::Status::Unsupported,
        BackendError::CrossDevice => protocol::Status::CrossDevice,
        BackendError::Busy | BackendError::Closed => protocol::Status::Busy,
        BackendError::Resource(SystemCallError::QuotaExceeded) => protocol::Status::Quota,
        BackendError::Resource(SystemCallError::OutOfMemory) => protocol::Status::Resource,
        BackendError::Resource(_) => protocol::Status::Internal,
    }
}

fn value_status(error: libfal::value::ValueError) -> protocol::Status {
    match error {
        libfal::value::ValueError::Budget
        | libfal::value::ValueError::Encoding
        | libfal::value::ValueError::DuplicateField
        | libfal::value::ValueError::HandleSlots => protocol::Status::Invalid,
        libfal::value::ValueError::Allocation => protocol::Status::Resource,
        libfal::value::ValueError::Affine => protocol::Status::Busy,
        libfal::value::ValueError::Unsupported => protocol::Status::Unsupported,
        libfal::value::ValueError::Capability(SystemCallError::QuotaExceeded) => {
            protocol::Status::Quota
        }
        libfal::value::ValueError::Capability(SystemCallError::OutOfMemory) => {
            protocol::Status::Resource
        }
        libfal::value::ValueError::Capability(_) => protocol::Status::Internal,
    }
}

fn export_status(error: SystemCallError) -> protocol::Status {
    match error {
        SystemCallError::OutOfMemory | SystemCallError::ReachLimit => protocol::Status::Resource,
        SystemCallError::QuotaExceeded => protocol::Status::Quota,
        SystemCallError::ObjectClosed
        | SystemCallError::ObjectNotFound
        | SystemCallError::StaleHandle => protocol::Status::GrantRevoked,
        SystemCallError::DeadlineExpired => protocol::Status::Cancelled,
        _ => protocol::Status::Internal,
    }
}

fn resolve_v2(
    backend: &impl FalBackend<Capability>,
    access: &AccessSnapshot,
    path: &str,
) -> Result<NodeRef, BackendError> {
    FalBackend::resolve(backend, access, path)
}

fn lookup_v2(
    backend: &impl FalBackend<Capability>,
    access: &AccessSnapshot,
    path: &str,
) -> Result<LookupResult<Capability>, BackendError> {
    FalBackend::lookup(backend, access, path)
}

fn node_info_v2(
    backend: &impl FalBackend<Capability>,
    reference: &NodeRef,
    ceiling: FalRights,
) -> Result<protocol::NodeInfo, BackendError> {
    let metadata = FalBackend::metadata(backend, reference, ceiling)?;
    Ok(protocol::NodeInfo {
        identity: metadata.identity,
        version: metadata.version,
        kind: metadata.kind,
        rights: metadata.rights,
        size: metadata.size,
    })
}

fn encode_v2(
    out: &mut [u8],
    op: protocol::Op,
    status: protocol::Status,
    deadline: Deadline,
    response: protocol::Response<'_>,
) -> Result<usize, SystemCallError> {
    protocol::encode_response(op, status, deadline, &response, out)
        .ok_or(SystemCallError::InternalError)
}

fn push_watch_effect(
    backend: &impl FalBackend<Capability>,
    effects: &mut Effects,
    node: &NodeRef,
    events: protocol::WatchMask,
    terminal: Option<protocol::WatchReason>,
) -> Result<(), SystemCallError> {
    let generation = FalBackend::metadata(backend, node, FalRights::ALL)
        .map_err(|_| SystemCallError::InternalError)?
        .version;
    effects.push(Effect {
        node: node.id(),
        generation,
        events,
        terminal,
    });
    Ok(())
}

fn drive_move<B: RuntimeBackend>(
    backend: &mut B,
    operation: &mut MoveOperation<B>,
    out: &mut [u8],
    budget: usize,
    effects: &mut Effects,
) -> Result<Option<usize>, SystemCallError> {
    let mut failure = |status| {
        encode_v2(
            out,
            protocol::Op::Move,
            status,
            operation.header.deadline,
            protocol::Response::Empty,
        )
    };
    if operation.prepared.is_none() {
        let parent = match resolve_v2(backend, &operation.access, &operation.source_parent) {
            Ok(parent) => parent,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        let target = match FalMutationBackend::lookup_child(
            backend,
            &parent,
            &operation.source_name,
            &operation.access,
        ) {
            Ok(target) => target,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        let source = match backend.position(&parent, &operation.source_name, operation.expected) {
            Ok(source) => source,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        let mutation = match backend.prepare_move(
            source,
            &operation.access,
            &operation.destination,
            &operation.destination_name,
        ) {
            Ok(mutation) => mutation,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        operation.source_parent_ref = Some(parent);
        operation.target_ref = Some(target);
        operation.prepared = Some(mutation);
        return Ok(None);
    }

    let checked = match backend.validate_move_step(
        operation
            .prepared
            .as_mut()
            .expect("move mutation exists before validation"),
        budget.max(1),
    ) {
        Ok(checked) => checked,
        Err(error) => {
            operation.prepared.take();
            return failure(backend_status(error)).map(Some);
        }
    };
    if !checked {
        return Ok(None);
    }

    let mutation = operation
        .prepared
        .take()
        .expect("checked move mutation remains owned");
    match backend.commit(mutation) {
        Ok(CommitResult::Moved(_)) => {
            let source = operation
                .source_parent_ref
                .as_ref()
                .expect("committed Move lost its source parent");
            let target = operation
                .target_ref
                .as_ref()
                .expect("committed Move lost its target");
            let destination = operation.destination.root();
            effects.push(Effect {
                node: source.id(),
                generation: FalBackend::metadata(backend, source, FalRights::ALL)
                    .map_err(|_| SystemCallError::InternalError)?
                    .version,
                events: protocol::WatchMask::RENAME,
                terminal: None,
            });
            effects.push(Effect {
                node: destination.id(),
                generation: FalBackend::metadata(backend, destination, FalRights::ALL)
                    .map_err(|_| SystemCallError::InternalError)?
                    .version,
                events: protocol::WatchMask::RENAME,
                terminal: None,
            });
            effects.push(Effect {
                node: target.id(),
                generation: FalBackend::metadata(backend, target, FalRights::ALL)
                    .map_err(|_| SystemCallError::InternalError)?
                    .version,
                events: protocol::WatchMask::RENAME,
                terminal: None,
            });
            failure(protocol::Status::Ok).map(Some)
        }
        Ok(_) => failure(protocol::Status::Internal).map(Some),
        Err(commit) => failure(backend_status(commit.error)).map(Some),
    }
}

struct ServeTransfers<'a> {
    input: &'a mut Vec<Capability>,
    output: &'a mut Vec<(Capability, Rights)>,
    effects: &'a mut Effects,
}

fn serve_v2<B: RuntimeBackend>(
    backend: &mut B,
    access: &AccessSnapshot,
    expected: protocol::Header,
    payload: &[u8],
    transfers: &mut ServeTransfers<'_>,
    out: &mut [u8],
) -> Result<usize, SystemCallError> {
    let (header, request) =
        protocol::decode_request(payload).map_err(|_| SystemCallError::IllegalArgument)?;
    if header != expected {
        return Err(SystemCallError::IllegalArgument);
    }
    let failure = |out: &mut [u8], status| {
        encode_v2(
            out,
            header.op,
            status,
            header.deadline,
            protocol::Response::Empty,
        )
    };
    match request {
        protocol::Request::Lookup { path } => {
            let lookup = match lookup_v2(backend, access, path) {
                Ok(lookup) => lookup,
                Err(error) => return failure(out, backend_status(error)),
            };
            match lookup {
                LookupResult::Found(reference) => {
                    let info = match node_info_v2(backend, &reference, access.rights()) {
                        Ok(info) => info,
                        Err(error) => return failure(out, backend_status(error)),
                    };
                    encode_v2(
                        out,
                        header.op,
                        protocol::Status::Ok,
                        header.deadline,
                        protocol::Response::Node(info),
                    )
                }
                LookupResult::LinkBoundary {
                    reference,
                    consumed,
                    target,
                    remaining,
                } => {
                    let info = match node_info_v2(backend, &reference, access.rights()) {
                        Ok(info) => info,
                        Err(error) => return failure(out, backend_status(error)),
                    };
                    encode_v2(
                        out,
                        header.op,
                        protocol::Status::Ok,
                        header.deadline,
                        protocol::Response::LinkBoundary {
                            node: info,
                            consumed: &consumed,
                            target: &target,
                            remaining: &remaining,
                        },
                    )
                }
                LookupResult::DelegationBoundary { .. } => failure(out, protocol::Status::Conflict),
            }
        }
        protocol::Request::Create {
            name,
            kind,
            rights,
            value,
        } => {
            if name.contains('/') || name.is_empty() {
                return failure(out, protocol::Status::Invalid);
            }
            let position = match backend.position(access.root(), name, None) {
                Ok(position) => position,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_create(
                position,
                access,
                CreateInput::Node { kind, value },
                transfers.input,
                rights,
            ) {
                Ok(mutation) => mutation,
                Err(CreateError::Backend(error)) => return failure(out, backend_status(error)),
                Err(CreateError::Value(error)) => return failure(out, value_status(error)),
            };
            let created = match backend.commit(mutation) {
                Ok(CommitResult::Created(created)) => created,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            push_watch_effect(
                backend,
                transfers.effects,
                access.root(),
                protocol::WatchMask::CREATE,
                None,
            )?;
            let info = node_info_v2(backend, &created, access.rights())
                .map_err(|_| SystemCallError::InternalError)?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Node(info),
            )
        }
        protocol::Request::Link {
            name,
            target,
            rights,
        } => {
            if name.contains('/') || name.is_empty() || !validate_path(target.as_bytes()) {
                return failure(out, protocol::Status::Invalid);
            }
            let position = match backend.position(access.root(), name, None) {
                Ok(position) => position,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_create(
                position,
                access,
                CreateInput::Link(target),
                transfers.input,
                rights,
            ) {
                Ok(mutation) => mutation,
                Err(CreateError::Backend(error)) => return failure(out, backend_status(error)),
                Err(CreateError::Value(error)) => return failure(out, value_status(error)),
            };
            let created = match backend.commit(mutation) {
                Ok(CommitResult::Created(created)) => created,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            push_watch_effect(
                backend,
                transfers.effects,
                access.root(),
                protocol::WatchMask::CREATE,
                None,
            )?;
            let info = node_info_v2(backend, &created, access.rights())
                .map_err(|_| SystemCallError::InternalError)?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Node(info),
            )
        }
        protocol::Request::Enumerate {
            path,
            cursor,
            limit,
        } => {
            let parent = match resolve_v2(backend, access, path) {
                Ok(parent) => parent,
                Err(error) => return failure(out, backend_status(error)),
            };
            let page_limit = usize::from(limit).min(7);
            let body = &mut out[protocol::HEADER_LEN..];
            let mut writer = Writer::new(body);
            writer.u64(0);
            writer.u16(0);
            writer.u16(0);
            let mut count = 0u16;
            let next = match FalBackend::enumerate(
                backend,
                &parent,
                access,
                cursor,
                page_limit,
                |name, reference, metadata| {
                    writer.sized_bytes(name.as_bytes());
                    writer.u64(reference.id().raw());
                    writer.u64(metadata.version);
                    writer.u32(metadata.kind as u32);
                    writer.u32(metadata.rights.raw());
                    writer.u64(metadata.size);
                    count += 1;
                },
            ) {
                Ok(next) => next,
                Err(error) => return failure(out, backend_status(error)),
            };
            let used = writer.written();
            body[..8].copy_from_slice(&next.to_le_bytes());
            body[8..10].copy_from_slice(&count.to_le_bytes());
            body[10..12].copy_from_slice(&0u16.to_le_bytes());
            protocol::Header {
                op: header.op,
                status: protocol::Status::Ok,
                body_len: used,
                deadline: header.deadline,
            }
            .encode(&mut out[..protocol::HEADER_LEN]);
            Some(protocol::HEADER_LEN + used).ok_or(SystemCallError::InternalError)
        }
        protocol::Request::Read { path } => {
            let snapshot = match FalBackend::read_snapshot(backend, access, path) {
                Ok(snapshot) => snapshot,
                Err(ReadError::Backend(error)) => {
                    return failure(out, backend_status(error));
                }
                Err(ReadError::Value(error)) => {
                    return failure(out, value_status(error));
                }
            };
            let mut direct = match snapshot.into_direct() {
                Ok(exported) => exported,
                Err(_) => return failure(out, protocol::Status::Unsupported),
            };
            let used = encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Value(&direct.bytes),
            )?;
            transfers.output.append(&mut direct.handles);
            Ok(used)
        }
        protocol::Request::Write { path, value } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let owners = core::mem::take(transfers.input);
            let stored = match StoredValue::prepare(value, owners, 1, PAYLOAD_MAX, access.account())
            {
                Ok(stored) => stored,
                Err(store) => {
                    transfers.input.extend(store.handles);
                    return failure(out, value_status(store.error));
                }
            };
            let mutation = match backend.prepare_property(reference.clone(), access, stored) {
                Ok(mutation) => mutation,
                Err(property) => return failure(out, backend_status(property.error)),
            };
            match backend.commit(mutation) {
                Ok(CommitResult::PropertyReplaced) => (),
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            push_watch_effect(
                backend,
                transfers.effects,
                &reference,
                protocol::WatchMask::MODIFY,
                None,
            )?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Empty,
            )
        }
        protocol::Request::ReadAt {
            path,
            offset,
            count,
        } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let maximum = PAYLOAD_MAX - librpc::PREFIX_LEN - protocol::HEADER_LEN - 2;
            if count as usize > maximum {
                return failure(out, protocol::Status::Invalid);
            }
            let mut bytes = [0; PAYLOAD_MAX];
            let read = match FalMutationBackend::read_stream(
                backend,
                &reference,
                access,
                offset,
                &mut bytes[..count as usize],
            ) {
                Ok(read) => read,
                Err(error) => return failure(out, backend_status(error)),
            };
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Value(&bytes[..read]),
            )
        }
        protocol::Request::WriteAt {
            path,
            offset,
            value,
        } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_write(reference.clone(), access, offset, value) {
                Ok(mutation) => mutation,
                Err(error) => return failure(out, backend_status(error)),
            };
            match backend.commit(mutation) {
                Ok(CommitResult::Written) => {}
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            }
            push_watch_effect(
                backend,
                transfers.effects,
                &reference,
                protocol::WatchMask::MODIFY,
                None,
            )?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Written(value.len() as u32),
            )
        }
        protocol::Request::Delete { name, expected } => {
            if name.contains('/') || name.is_empty() {
                return failure(out, protocol::Status::Invalid);
            }
            let expected = if expected.identity == 0 {
                None
            } else {
                let Some(identity) = NodeId::from_raw(expected.identity) else {
                    return failure(out, protocol::Status::Invalid);
                };
                Some((identity, expected.version))
            };
            let parent = access.root().clone();
            let target = match FalMutationBackend::lookup_child(backend, &parent, name, access) {
                Ok(target) => target,
                Err(error) => return failure(out, backend_status(error)),
            };
            let position = match backend.position(access.root(), name, expected) {
                Ok(position) => position,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_delete(position, access) {
                Ok(mutation) => mutation,
                Err(error) => return failure(out, backend_status(error)),
            };
            let deleted = match backend.commit(mutation) {
                Ok(CommitResult::Deleted(reference)) => reference,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            assert_eq!(deleted.id(), target.id(), "Delete returned the wrong node");
            push_watch_effect(
                backend,
                transfers.effects,
                &parent,
                protocol::WatchMask::DELETE,
                None,
            )?;
            push_watch_effect(
                backend,
                transfers.effects,
                &deleted,
                protocol::WatchMask::DELETE,
                Some(protocol::WatchReason::NodeDeleted),
            )?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Empty,
            )
        }
        protocol::Request::Take { .. } => failure(out, protocol::Status::Unsupported),
        protocol::Request::Derive { .. }
        | protocol::Request::Move { .. }
        | protocol::Request::Subscribe { .. }
        | protocol::Request::QuerySubscription { .. }
        | protocol::Request::Unsubscribe { .. }
        | protocol::Request::Open { .. } => failure(out, protocol::Status::Internal),
        protocol::Request::Start
        | protocol::Request::QueryStream
        | protocol::Request::FinishStream
        | protocol::Request::CancelStream => failure(out, protocol::Status::Permission),
    }
}

fn record_provider_response<B: RuntimeBackend>(
    world: &mut World<B>,
    outbox: &Outbox,
    business_committed: bool,
) -> Result<(), SystemCallError> {
    world.provider.record_response(
        business_committed,
        matches!(outbox.result(), Some(OutboxResult::Abandoned(_))),
    )?;
    if let Some(OutboxResult::Abandoned(cause)) = outbox.result() {
        rinlib::debug!("fs: provider response abandoned after commit: {:?}", cause);
    }
    Ok(())
}

struct RequestTask<B: RuntimeBackend> {
    outbox: Outbox,
    mode: RequestMode<B>,
    committed: bool,
    recorded: bool,
    wakes: WakeBatch,
}

enum RequestMode<B: RuntimeBackend> {
    V2 {
        access: AccessSnapshot,
        header: protocol::Header,
    },
    Move(MoveOperation<B>),
    Take(TakeOperation<B>),
    WatchControl(WatchControlOperation),
    V2Failure {
        header: protocol::Header,
        status: protocol::Status,
    },
}

enum WatchControlKind {
    Query(u64),
    Unsubscribe(u64),
}

struct WatchControlOperation {
    context: u64,
    header: protocol::Header,
    kind: WatchControlKind,
    result: Option<(protocol::Status, Option<protocol::SubscriptionInfo>)>,
    pending_wake: Option<u64>,
    encoded: bool,
}

impl WatchControlOperation {
    fn drive<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        out: &mut [u8],
    ) -> Result<Option<usize>, SystemCallError> {
        if self.encoded {
            return Ok(Some(0));
        }
        if self.result.is_none() {
            let result = match self.kind {
                WatchControlKind::Query(id) => match world.provider.watches.query(id, self.context)
                {
                    Ok(info) => (protocol::Status::Ok, Some(info)),
                    Err(ControlError::NotFound) => (protocol::Status::NotFound, None),
                    Err(ControlError::Permission) => (protocol::Status::Permission, None),
                },
                WatchControlKind::Unsubscribe(id) => {
                    match world.provider.watches.cancel(id, self.context) {
                        Ok(task) => {
                            self.pending_wake = Some(task);
                            (protocol::Status::Ok, None)
                        }
                        Err(ControlError::NotFound) => (protocol::Status::NotFound, None),
                        Err(ControlError::Permission) => (protocol::Status::Permission, None),
                    }
                }
            };
            self.result = Some(result);
        }
        if let Some(task) = self.pending_wake {
            match requests.wake(task) {
                Ok(()) => self.pending_wake = None,
                Err(SystemCallError::ReachLimit) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        let (status, info) = self.result.expect("Watch control result missing");
        let response = info
            .map(protocol::Response::Subscription)
            .unwrap_or(protocol::Response::Empty);
        let used = encode_v2(out, self.header.op, status, self.header.deadline, response)?;
        self.encoded = true;
        Ok(Some(used))
    }
}

struct MoveOperation<B: RuntimeBackend> {
    access: AccessSnapshot,
    destination: AccessSnapshot,
    header: protocol::Header,
    source_parent: String,
    source_name: String,
    destination_name: String,
    expected: Option<(NodeId, u64)>,
    prepared: Option<B::Mutation>,
    source_parent_ref: Option<NodeRef>,
    target_ref: Option<NodeRef>,
}

struct TakeOperation<B: RuntimeBackend> {
    access: AccessSnapshot,
    header: protocol::Header,
    path: String,
    prepared: Option<B::TakeReservation>,
    reference: Option<NodeRef>,
    bytes: Option<Vec<u8>>,
    policies: Vec<ExportPolicy>,
    restore_handles: Vec<StoredHandle<Capability>>,
    encoded: bool,
    failed: bool,
}

impl<B: RuntimeBackend> TakeOperation<B> {
    fn drive(&mut self, backend: &mut B, outbox: &mut Outbox) -> Result<usize, SystemCallError> {
        if self.encoded {
            return Ok(0);
        }
        if self.prepared.is_none() {
            let reference = match resolve_v2(backend, &self.access, &self.path) {
                Ok(reference) => reference,
                Err(error) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        backend_status(error),
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
            };
            let value = match FalMutationBackend::property(backend, &reference) {
                Ok(value) => value,
                Err(BackendError::NotFound) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        protocol::Status::NotFound,
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
                Err(_) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        protocol::Status::Invalid,
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
            };
            if let Err(error) = value.validate_output_transport(self.access.output_transport()) {
                self.failed = true;
                self.encoded = true;
                return encode_v2(
                    outbox.response_mut()?.body_mut()?,
                    self.header.op,
                    value_status(error),
                    self.header.deadline,
                    protocol::Response::Empty,
                );
            }
            if value
                .handles
                .iter()
                .any(|handle| handle.policy.protocol == ValueProtocol::Directory)
            {
                self.failed = true;
                self.encoded = true;
                return encode_v2(
                    outbox.response_mut()?.body_mut()?,
                    self.header.op,
                    protocol::Status::Unsupported,
                    self.header.deadline,
                    protocol::Response::Empty,
                );
            }
            if self
                .policies
                .try_reserve_exact(value.handles.len())
                .is_err()
                || self
                    .restore_handles
                    .try_reserve_exact(value.handles.len())
                    .is_err()
            {
                self.failed = true;
                self.encoded = true;
                return encode_v2(
                    outbox.response_mut()?.body_mut()?,
                    self.header.op,
                    protocol::Status::Resource,
                    self.header.deadline,
                    protocol::Response::Empty,
                );
            }
            self.policies
                .extend(value.handles.iter().map(|handle| handle.policy));
            let watch_reference = reference.clone();
            let mut prepared = match backend.prepare_take(reference, &self.access) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        backend_status(error),
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
            };
            self.reference = Some(watch_reference);
            let taken = B::take_value(&mut prepared);
            self.bytes = Some(taken.bytes);
            self.prepared = Some(prepared);

            let bytes = self.bytes.as_ref().expect("take bytes prepared");
            let response = outbox.response_mut()?;
            let used = encode_v2(
                response.body_mut()?,
                self.header.op,
                protocol::Status::Ok,
                self.header.deadline,
                protocol::Response::Value(bytes),
            )?;
            let mut handles = taken.handles;
            let mut index = 0usize;
            while !handles.is_empty() {
                let handle = handles.remove(0);
                let policy = self.policies[index];
                if let Err(failure) = response.push(handle.owner, policy.transport) {
                    handles.insert(
                        0,
                        StoredHandle {
                            owner: failure.capability,
                            policy,
                        },
                    );
                    let mut prior = index;
                    response.drain_capabilities(|owner, _| {
                        prior -= 1;
                        handles.insert(
                            0,
                            StoredHandle {
                                owner,
                                policy: self.policies[prior],
                            },
                        );
                    });
                    assert_eq!(prior, 0, "take reply recovery omitted a capability");
                    let taken = TakenValue {
                        bytes: self.bytes.take().expect("take bytes on push failure"),
                        handles,
                    };
                    let prepared = self.prepared.take().expect("take reservation exists");
                    backend.rollback_take(prepared, taken);
                    return Err(failure.error);
                }
                index += 1;
            }
            self.restore_handles = handles;
            self.encoded = true;
            return Ok(used);
        }
        Ok(0)
    }

    fn finalize(
        &mut self,
        backend: &mut B,
        outbox: &mut Outbox,
        effects: &mut Effects,
    ) -> Result<bool, SystemCallError> {
        if self.failed {
            return Ok(false);
        }
        let prepared = self.prepared.take().expect("take reservation exists");
        if matches!(outbox.result(), Some(OutboxResult::Sent)) {
            backend.commit_take(prepared);
            push_watch_effect(
                backend,
                effects,
                self.reference
                    .as_ref()
                    .expect("committed Take lost its target"),
                protocol::WatchMask::MODIFY,
                None,
            )?;
            self.bytes = None;
            self.policies.clear();
            return Ok(true);
        }
        let mut index = self.policies.len();
        outbox.drain_capabilities(|owner, _| {
            index -= 1;
            self.restore_handles.insert(
                0,
                StoredHandle {
                    owner,
                    policy: self.policies[index],
                },
            );
        });
        assert_eq!(index, 0, "take rollback omitted a capability");
        assert_eq!(
            self.restore_handles.len(),
            self.policies.len(),
            "take rollback capability count mismatch"
        );
        let taken = TakenValue {
            bytes: self.bytes.take().expect("take bytes retained"),
            handles: core::mem::take(&mut self.restore_handles),
        };
        backend.rollback_take(prepared, taken);
        self.policies.clear();
        Ok(false)
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RequestTask<B> {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if !self.wakes.is_empty() && !self.wakes.drive(requests)? {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !self.committed && self.outbox.result().is_none() {
            if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            }
            let used = match &mut self.mode {
                RequestMode::V2 { access, header } => {
                    let mut effects = Effects::default();
                    let used = {
                        let response = self.outbox.response_mut()?;
                        let (context, body) = response.parts()?;
                        let mut input_handles = context.handles.take_all();
                        let mut output_handles = Vec::new();
                        let mut transfers = ServeTransfers {
                            input: &mut input_handles,
                            output: &mut output_handles,
                            effects: &mut effects,
                        };
                        let used = serve_v2(
                            world
                                .provider
                                .backend
                                .as_mut()
                                .expect("provider backend missing during request"),
                            access,
                            *header,
                            &context.payload,
                            &mut transfers,
                            body,
                        )?;
                        for (capability, rights) in output_handles {
                            response
                                .push(capability, rights)
                                .map_err(|failure| failure.error)?;
                        }
                        used
                    };
                    publish_watch_events(world, &effects, &mut self.wakes);
                    used
                }
                RequestMode::Move(operation) => {
                    let mut effects = Effects::default();
                    let Some(used) = drive_move(
                        world
                            .provider
                            .backend
                            .as_mut()
                            .expect("provider backend missing during move"),
                        operation,
                        self.outbox.response_mut()?.body_mut()?,
                        budget,
                        &mut effects,
                    )?
                    else {
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    };
                    publish_watch_events(world, &effects, &mut self.wakes);
                    used
                }
                RequestMode::Take(operation) => operation.drive(
                    world
                        .provider
                        .backend
                        .as_mut()
                        .expect("provider backend missing during take"),
                    &mut self.outbox,
                )?,
                RequestMode::WatchControl(operation) => {
                    let Some(used) = operation.drive(
                        world,
                        requests,
                        self.outbox.response_mut()?.body_mut()?,
                    )?
                    else {
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    };
                    used
                }
                RequestMode::V2Failure { header, status } => encode_v2(
                    self.outbox.response_mut()?.body_mut()?,
                    header.op,
                    *status,
                    header.deadline,
                    protocol::Response::Empty,
                )?,
            };
            self.outbox.response_mut()?.finish_body(used)?;
            self.committed = true;
            if !self.wakes.is_empty() && !self.wakes.drive(requests)? {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete && self.committed && !self.recorded {
            let business_committed = if let RequestMode::Take(operation) = &mut self.mode {
                let mut effects = Effects::default();
                let committed = operation.finalize(
                    world
                        .provider
                        .backend
                        .as_mut()
                        .expect("provider backend missing during take finalization"),
                    &mut self.outbox,
                    &mut effects,
                )?;
                publish_watch_events(world, &effects, &mut self.wakes);
                committed
            } else {
                true
            };
            record_provider_response(world, &self.outbox, business_committed)?;
            self.recorded = true;
            if !self.wakes.is_empty() && !self.wakes.drive(requests)? {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        match failure {
            RequestFailure::Wake {
                error: SystemCallError::ObjectNotFound,
                ..
            } => {
                // Watch 已取消/完成：该 stale wake 的债务已经自然解除。
            }
            RequestFailure::Wake { .. } => world.failed = true,
            other => self.outbox.refused(world, other),
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

struct PendingDirectoryExport {
    index: usize,
    policy: ExportPolicy,
    mailbox: u64,
    context: u64,
}

struct ReadTask {
    outbox: Outbox,
    header: protocol::Header,
    snapshot: Option<ReadSnapshot<Capability>>,
    pending: Option<PendingDirectoryExport>,
    dispatch: DownstreamState,
    status: Option<protocol::Status>,
    encoded: bool,
    recorded: bool,
}

impl ReadTask {
    fn new(outbox: Outbox, header: protocol::Header, snapshot: ReadSnapshot<Capability>) -> Self {
        Self {
            outbox,
            header,
            snapshot: Some(snapshot),
            pending: None,
            dispatch: DownstreamState::Queued,
            status: None,
            encoded: false,
            recorded: false,
        }
    }

    fn submitted(&mut self, result: Result<u64, Completion>) {
        assert!(
            matches!(self.dispatch, DownstreamState::Queued),
            "Directory export submission completed from an invalid state"
        );
        self.dispatch = match result {
            Ok(txid) => DownstreamState::Pending(txid),
            Err(completion) => DownstreamState::Complete(completion),
        };
    }

    fn completed(&mut self, txid: u64, completion: Completion) {
        assert!(
            matches!(self.dispatch, DownstreamState::Pending(pending) | DownstreamState::Cancelling(pending) if pending == txid),
            "Directory export completion txid mismatch"
        );
        self.dispatch = DownstreamState::Complete(completion);
    }

    fn begin_next_export(&mut self) -> bool {
        let prepared = (|| {
            let Some(snapshot) = self.snapshot.as_mut() else {
                return Ok(false);
            };
            let Some(index) = snapshot
                .handles
                .iter()
                .position(|handle| matches!(handle, SnapshotHandle::Directory { .. }))
            else {
                return Ok(false);
            };
            let SnapshotHandle::Directory { provider, policy } = snapshot.handles.remove(index)
            else {
                unreachable!()
            };
            let description = provider.description().map_err(export_status)?;
            let request = protocol::Request::Derive {
                path: "",
                rights: policy.fal_ceiling,
            };
            let capacity = protocol::HEADER_LEN
                .checked_add(request.encoded_len().ok_or(protocol::Status::Invalid)?)
                .ok_or(protocol::Status::Resource)?;
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(capacity)
                .map_err(|_| protocol::Status::Resource)?;
            payload.resize(capacity, 0);
            let used = protocol::encode_request(&request, self.header.deadline, &mut payload)
                .ok_or(protocol::Status::Internal)?;
            payload.truncate(used);
            let request = RpcRequest::new(protocol::ID, &payload).map_err(export_status)?;
            self.pending = Some(PendingDirectoryExport {
                index,
                policy,
                mailbox: description.related_object_id,
                context: description.object_id,
            });
            self.dispatch = DownstreamState::Ready {
                service: provider,
                request,
            };
            Ok(true)
        })();
        match prepared {
            Ok(started) => started,
            Err(status) => {
                self.status = Some(status);
                false
            }
        }
    }

    fn consume_completion(&mut self) -> Result<(), SystemCallError> {
        let completion = match core::mem::replace(&mut self.dispatch, DownstreamState::Queued) {
            DownstreamState::Complete(completion) => completion,
            other => {
                self.dispatch = other;
                return Ok(());
            }
        };
        drop(completion.service);
        let pending = self
            .pending
            .take()
            .expect("Directory export completion lost its pending field");
        let outcome = match completion.result {
            Ok(mut reply) => {
                let decoded = protocol::decode_response(&reply.payload);
                match decoded {
                    Ok((header, protocol::Response::Node(node)))
                        if header.op == protocol::Op::Derive
                            && header.status == protocol::Status::Ok
                            && header.deadline == self.header.deadline
                            && node.kind == NodeKind::Directory
                            && node.rights == pending.policy.fal_ceiling
                            && reply.handles.remaining() == 1 =>
                    {
                        let capability = reply
                            .handles
                            .take(0)
                            .map_err(|_| SystemCallError::InternalError)?;
                        match MailboxSender::from_capability(capability) {
                            Ok((sender, description))
                                if description.related_object_id == pending.mailbox
                                    && description.object_id != pending.context
                                    && pending
                                        .policy
                                        .transport
                                        .is_subset_of(description.rights) =>
                            {
                                let child = sender.into_capability();
                                child
                                    .duplicate(pending.policy.transport)
                                    .map_err(export_status)
                            }
                            Ok(_) | Err(_) => Err(protocol::Status::Internal),
                        }
                    }
                    Ok((header, _))
                        if header.op == protocol::Op::Derive
                            && header.status != protocol::Status::Ok
                            && header.deadline == self.header.deadline
                            && reply.handles.is_empty() =>
                    {
                        Err(header.status)
                    }
                    _ => Err(protocol::Status::Internal),
                }
            }
            Err(CallError { cause, .. }) => Err(match cause {
                CallCause::Timeout | CallCause::Cancelled | CallCause::Shutdown => protocol::Status::Cancelled,
                CallCause::ServiceClosed => protocol::Status::GrantRevoked,
                CallCause::System(error) => export_status(error),
                CallCause::Frame(_) => protocol::Status::Internal,
            }),
        };
        match outcome {
            Ok(owner) => {
                self.snapshot
                    .as_mut()
                    .expect("Directory export snapshot disappeared")
                    .handles
                    .insert(
                        pending.index,
                        SnapshotHandle::Direct {
                            owner,
                            policy: pending.policy,
                        },
                    );
            }
            Err(status) => self.status = Some(status),
        }
        Ok(())
    }

    fn encode(&mut self) -> Result<(), SystemCallError> {
        if self.encoded || self.outbox.result().is_some() {
            self.snapshot = None;
            self.encoded = true;
            return Ok(());
        }
        let response = self.outbox.response_mut()?;
        if let Some(status) = self.status {
            self.snapshot = None;
            let used = protocol::encode_response(
                protocol::Op::Read,
                status,
                self.header.deadline,
                &protocol::Response::Empty,
                response.body_mut()?,
            )
            .ok_or(SystemCallError::InternalError)?;
            response.finish_body(used)?;
            self.encoded = true;
            return Ok(());
        }
        let snapshot = self
            .snapshot
            .as_mut()
            .expect("Read snapshot disappeared before encoding");
        let used = protocol::encode_response(
            protocol::Op::Read,
            protocol::Status::Ok,
            self.header.deadline,
            &protocol::Response::Value(&snapshot.bytes),
            response.body_mut()?,
        )
        .ok_or(SystemCallError::InternalError)?;
        while !snapshot.handles.is_empty() {
            let handle = snapshot.handles.remove(0);
            let SnapshotHandle::Direct { owner, policy } = handle else {
                panic!("Read reply encoded before Directory export completed")
            };
            if let Err(failure) = response.push(owner, policy.transport) {
                snapshot.handles.insert(
                    0,
                    SnapshotHandle::Direct {
                        owner: failure.capability,
                        policy,
                    },
                );
                return Err(failure.error);
            }
        }
        response.finish_body(used)?;
        self.snapshot = None;
        self.encoded = true;
        Ok(())
    }
}

impl<B: RuntimeBackend> Task<World<B>> for ReadTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if world.stop {
            self.outbox.stop(world);
        }
        if self.outbox.result().is_some() {
            self.snapshot = None;
            self.pending = None;
            self.encoded = true;
            if let Some(advance) = abandon_downstream(&mut self.dispatch, id, world, requests)? {
                return Ok(advance);
            }
        }
        if self.outbox.result().is_none() && !self.outbox.is_admitted() {
            return Ok(self.outbox.admit(requests));
        }
        if matches!(self.dispatch, DownstreamState::Complete(_)) {
            self.consume_completion()?;
        }
        if !self.encoded
            && self.status.is_none()
            && matches!(self.dispatch, DownstreamState::Queued)
            && self.begin_next_export()
        {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if matches!(self.dispatch, DownstreamState::Ready { .. }) {
            if world.dispatch_intent.is_some() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            requests.wake(world.dispatcher_task)?;
            let ready = core::mem::replace(&mut self.dispatch, DownstreamState::Queued);
            let DownstreamState::Ready { service, request } = ready else {
                unreachable!()
            };
            world.dispatch_intent = Some(DispatchIntent::Submit(DispatchSubmission {
                waiter: id,
                service,
                deadline: self.header.deadline,
                request,
            }));
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if matches!(self.dispatch, DownstreamState::Pending(_)) {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if !self.encoded {
            self.encode()?;
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete && !self.recorded {
            world.provider.record_response(
                true,
                matches!(self.outbox.result(), Some(OutboxResult::Abandoned(_))),
            )?;
            if let Some(OutboxResult::Abandoned(cause)) = self.outbox.result() {
                rinlib::debug!("fs: Read response abandoned after snapshot: {:?}", cause);
            }
            self.recorded = true;
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        self.outbox.refused(world, failure);
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

struct DelegateSetup {
    header: protocol::Header,
    consumed: String,
    remaining: String,
    service: Capability,
    request: RpcRequest,
}

impl DelegateSetup {
    fn with_outbox(self, outbox: Outbox) -> DelegateTask {
        DelegateTask {
            outbox,
            header: self.header,
            consumed: self.consumed,
            remaining: self.remaining,
            dispatch: DownstreamState::Ready {
                service: self.service,
                request: self.request,
            },
            committed: false,
            recorded: false,
        }
    }
}

fn prepare_delegate(
    target: Capability,
    rights: FalRights,
    consumed: String,
    remaining: String,
    header: protocol::Header,
) -> Result<DelegateSetup, protocol::Status> {
    let request = protocol::Request::Derive { path: "", rights };
    let capacity = protocol::HEADER_LEN
        .checked_add(request.encoded_len().ok_or(protocol::Status::Invalid)?)
        .ok_or(protocol::Status::Invalid)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(capacity)
        .map_err(|_| protocol::Status::Resource)?;
    payload.resize(capacity, 0);
    let used = protocol::encode_request(&request, header.deadline, &mut payload)
        .ok_or(protocol::Status::Internal)?;
    payload.truncate(used);
    let rpc = RpcRequest::new(protocol::ID, &payload).map_err(|_| protocol::Status::Resource)?;
    Ok(DelegateSetup {
        header,
        consumed,
        remaining,
        service: target,
        request: rpc,
    })
}

enum DownstreamState {
    Ready {
        service: Capability,
        request: RpcRequest,
    },
    Queued,
    Pending(u64),
    Cancelling(u64),
    Complete(Completion),
}

fn abandon_downstream<B: RuntimeBackend>(
    dispatch: &mut DownstreamState,
    id: u64,
    world: &mut World<B>,
    requests: &mut Requests<ServiceTask<B>>,
) -> Result<Option<Advance>, SystemCallError> {
    match dispatch {
        DownstreamState::Ready { .. } | DownstreamState::Complete(_) => {
            *dispatch = DownstreamState::Queued;
        }
        DownstreamState::Queued => {
            if matches!(
                world.dispatch_intent.as_ref(),
                Some(DispatchIntent::Submit(submission)) if submission.waiter == id
            ) {
                world.dispatch_intent = None;
            }
        }
        DownstreamState::Pending(txid) => {
            if world.dispatch_intent.is_some() {
                return Ok(Some(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                }));
            }
            let txid = *txid;
            requests.wake(world.dispatcher_task)?;
            world.dispatch_intent = Some(DispatchIntent::Cancel { txid });
            *dispatch = DownstreamState::Cancelling(txid);
            return Ok(Some(Advance {
                work_done: 1,
                step: Step::Parked,
            }));
        }
        DownstreamState::Cancelling(_) => {
            return Ok(Some(Advance {
                work_done: 1,
                step: Step::Parked,
            }));
        }
    }
    Ok(None)
}

struct DelegateTask {
    outbox: Outbox,
    header: protocol::Header,
    consumed: String,
    remaining: String,
    dispatch: DownstreamState,
    committed: bool,
    recorded: bool,
}

impl DelegateTask {
    fn submitted(&mut self, result: Result<u64, Completion>) {
        assert!(
            matches!(self.dispatch, DownstreamState::Queued),
            "Delegate submission completed from an invalid state"
        );
        self.dispatch = match result {
            Ok(txid) => DownstreamState::Pending(txid),
            Err(completion) => DownstreamState::Complete(completion),
        };
    }

    fn completed(&mut self, txid: u64, completion: Completion) {
        assert!(
            matches!(self.dispatch, DownstreamState::Pending(pending) | DownstreamState::Cancelling(pending) if pending == txid),
            "Delegate completion txid mismatch"
        );
        self.dispatch = DownstreamState::Complete(completion);
    }

    fn encode_completion(&mut self) -> Result<(), SystemCallError> {
        let completion = match core::mem::replace(&mut self.dispatch, DownstreamState::Queued) {
            DownstreamState::Complete(completion) => completion,
            other => {
                self.dispatch = other;
                return Ok(());
            }
        };
        drop(completion.service);
        let mut derived = None;
        let mut info = None;
        let status = match completion.result {
            Ok(mut reply) => {
                let decoded = protocol::decode_response(&reply.payload);
                match decoded {
                    Ok((header, response))
                        if header.op == protocol::Op::Derive
                            && header.status == protocol::Status::Ok
                            && matches!(response, protocol::Response::Node(_))
                            && reply.handles.remaining() == 1 =>
                    {
                        let protocol::Response::Node(node) = response else {
                            unreachable!()
                        };
                        let capability = reply
                            .handles
                            .take(0)
                            .map_err(|_| SystemCallError::InternalError)?;
                        match MailboxSender::from_capability(capability) {
                            Ok((sender, _)) => {
                                derived = Some(sender);
                                info = Some(node);
                                protocol::Status::Ok
                            }
                            Err(_) => protocol::Status::Internal,
                        }
                    }
                    Ok((header, _))
                        if header.op == protocol::Op::Derive
                            && header.status != protocol::Status::Ok
                            && reply.handles.is_empty() =>
                    {
                        header.status
                    }
                    _ => protocol::Status::Internal,
                }
            }
            Err(CallError { cause, .. }) => match cause {
                CallCause::Timeout | CallCause::Cancelled | CallCause::Shutdown => protocol::Status::Cancelled,
                CallCause::ServiceClosed => protocol::Status::GrantRevoked,
                CallCause::Frame(_) | CallCause::System(_) => protocol::Status::Internal,
            },
        };
        let response = self.outbox.response_mut()?;
        let used = if status == protocol::Status::Ok {
            let sender = derived
                .take()
                .expect("successful Delegate lost its derived grant");
            response
                .push(
                    sender.into_capability(),
                    Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                )
                .map_err(|failure| failure.error)?;
            protocol::encode_response(
                protocol::Op::Lookup,
                status,
                self.header.deadline,
                &protocol::Response::Delegate {
                    node: info.expect("successful Delegate lost its node information"),
                    consumed: &self.consumed,
                    remaining: &self.remaining,
                },
                response.body_mut()?,
            )
        } else {
            protocol::encode_response(
                protocol::Op::Lookup,
                status,
                self.header.deadline,
                &protocol::Response::Empty,
                response.body_mut()?,
            )
        }
        .ok_or(SystemCallError::InternalError)?;
        response.finish_body(used)?;
        self.committed = true;
        Ok(())
    }
}

impl<B: RuntimeBackend> Task<World<B>> for DelegateTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if world.stop {
            self.outbox.stop(world);
        }
        if !self.committed && self.outbox.result().is_some() {
            if let Some(advance) = abandon_downstream(&mut self.dispatch, id, world, requests)? {
                return Ok(advance);
            }
            return self.outbox.drive(requests, budget);
        }
        if self.outbox.result().is_none() && !self.outbox.is_admitted() {
            return Ok(self.outbox.admit(requests));
        }
        if matches!(self.dispatch, DownstreamState::Ready { .. }) {
            if world.dispatch_intent.is_some() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            requests.wake(world.dispatcher_task)?;
            let ready = core::mem::replace(&mut self.dispatch, DownstreamState::Queued);
            let DownstreamState::Ready { service, request } = ready else {
                unreachable!()
            };
            world.dispatch_intent = Some(DispatchIntent::Submit(DispatchSubmission {
                waiter: id,
                service,
                deadline: self.header.deadline,
                request,
            }));
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if !self.committed && matches!(self.dispatch, DownstreamState::Complete(_)) {
            self.encode_completion()?;
        }
        if !self.committed {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete && self.committed && !self.recorded {
            world.provider.record_response(
                true,
                matches!(self.outbox.result(), Some(OutboxResult::Abandoned(_))),
            )?;
            if let Some(OutboxResult::Abandoned(cause)) = self.outbox.result() {
                rinlib::debug!("fs: Delegate response abandoned after commit: {:?}", cause);
            }
            self.recorded = true;
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        self.outbox.refused(world, failure);
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

fn service_status(error: RegistryError) -> service_protocol::Status {
    match error {
        RegistryError::Closed => service_protocol::Status::Cancelled,
        RegistryError::Permission => service_protocol::Status::Permission,
        RegistryError::Invalid => service_protocol::Status::Invalid,
        RegistryError::Exists => service_protocol::Status::Exists,
        RegistryError::NotFound => service_protocol::Status::NotFound,
        RegistryError::Conflict => service_protocol::Status::Conflict,
        RegistryError::Expired => service_protocol::Status::Expired,
        RegistryError::Resource(SystemCallError::QuotaExceeded) => service_protocol::Status::Quota,
        RegistryError::Resource(SystemCallError::ObjectBusy) => service_protocol::Status::Busy,
        RegistryError::Resource(_) => service_protocol::Status::Resource,
    }
}

fn check_registration_authority<B: RuntimeBackend>(
    world: &World<B>,
    identity: u64,
    name: &str,
) -> Result<(), RegistryError> {
    world
        .provider
        .backend
        .as_ref()
        .and_then(RuntimeBackend::registry)
        .ok_or(RegistryError::Closed)?
        .authorize_name(identity, name)
}

fn drain_registration<B: RuntimeBackend>(
    world: &mut World<B>,
    instance: u64,
    reason: service_protocol::TerminalReason,
) -> Effects {
    world
        .provider
        .backend
        .as_mut()
        .and_then(RuntimeBackend::registry_mut)
        .and_then(|registry| registry.begin_drain(instance, reason).ok())
        .map_or_else(Effects::default, |transition| transition.effects)
}

fn remove_registration_control<B: RuntimeBackend>(
    world: &mut World<B>,
    control: RegistrationControl,
    reason: service_protocol::TerminalReason,
) -> Effects {
    if let Some(index) = world
        .registration_controls
        .iter()
        .position(|candidate| candidate.instance == control.instance)
    {
        world.registration_controls.swap_remove(index);
    }
    let effects = drain_registration(world, control.instance, reason);
    if let Some(registry) = world
        .provider
        .backend
        .as_mut()
        .and_then(RuntimeBackend::registry_mut)
    {
        registry.release_control(control.instance);
    }
    effects
}

enum RegistrationCommand {
    Register {
        name: String,
        protocol: u64,
        version: u32,
        policy: ExportPolicy,
        establish_deadline: Deadline,
        endpoint: Capability,
    },
    QueryName(String),
    Withdraw(String, u64, u64),
    PublishReady,
    BeginDrain,
    Query,
    DelegateName(String),
}

fn copy_request_name(name: &str) -> Result<String, SystemCallError> {
    let mut copy = String::new();
    copy.try_reserve_exact(name.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    copy.push_str(name);
    Ok(copy)
}

#[allow(
    clippy::large_enum_variant,
    reason = "额度拒绝时原样保留已预付任务 owner，避免在无额度路径额外堆分配"
)]
enum PendingRegistrationTask {
    Authority(AuthorityTask),
    Reply(RegistrationReplyTask),
}

impl PendingRegistrationTask {
    fn into_service<B: RuntimeBackend>(self) -> ServiceTask<B> {
        match self {
            Self::Authority(task) => ServiceTask::Authority(task),
            Self::Reply(task) => ServiceTask::RegistrationReply(task),
        }
    }
}

struct RegistrationIngress {
    buffer: ReceiveBuffer,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
    gate_pending: bool,
    queued: Option<(PendingRegistrationTask, usize)>,
    refusal: Option<Outbox>,
}

impl RegistrationIngress {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new()
                .expect("RegistrationControl receive buffer creation failed"),
            source: None,
            requested: false,
            gate_pending: false,
            queued: None,
            refusal: None,
            removing: false,
            stopping: false,
        }
    }
    fn reject_task<B: RuntimeBackend>(
        &mut self,
        task: ServiceTask<B>,
        error: SystemCallError,
        world: &mut World<B>,
    ) {
        let (mut outbox, op) = task.into_registration_refusal();
        self.gate_pending = false;
        let status = match error {
            SystemCallError::QuotaExceeded | SystemCallError::ReachLimit => {
                service_protocol::Status::Quota
            }
            SystemCallError::OutOfMemory => service_protocol::Status::Resource,
            _ => service_protocol::Status::Cancelled,
        };
        if self.stopping {
            outbox.stop(world);
        } else {
            let deadline = outbox.deadline();
            let response = outbox
                .response_mut()
                .expect("registration refusal lost prepared reply");
            let used = service_protocol::encode_response(
                op,
                status,
                deadline,
                service_protocol::Response::Empty,
                response
                    .body_mut()
                    .expect("registration refusal lost reply body"),
            )
            .expect("registration refusal reply layout invalid");
            response
                .finish_body(used)
                .expect("registration refusal reply length invalid");
        }
        self.refusal = Some(outbox);
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RegistrationIngress {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.kind == KIND_REPLY {
                if let Some(outbox) = self.refusal.as_mut() {
                    outbox.observe(event);
                }
            } else if event.kind == KIND_REGISTRATION {
                if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                    rinlib::debug!(
                        "RegistrationControl terminal event: error={}, signals={:#x}",
                        event.error,
                        event.observed.raw()
                    );
                    world.failed = true;
                    self.stopping = true;
                } else {
                    ready = true;
                }
            } else {
                return Err(SystemCallError::InternalError);
            }
        }
        if input.take_timeout()
            && let Some(outbox) = self.refusal.as_mut()
        {
            outbox.timed_out();
        }
        if self.stopping {
            self.queued = None;
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            if let Some(outbox) = self.refusal.as_mut() {
                outbox.stop(world);
                let advance = outbox.drive(requests, budget)?;
                if !outbox.is_complete() {
                    return Ok(advance);
                }
                self.refusal = None;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_none() && !self.requested && !self.gate_pending {
                    Step::Complete
                } else {
                    Step::Parked
                },
            });
        }
        if let Some(outbox) = self.refusal.as_mut() {
            let advance = if outbox.result().is_none() && !outbox.is_admitted() {
                outbox.admit(requests)
            } else {
                outbox.drive(requests, budget)?
            };
            if !outbox.is_complete() {
                return Ok(advance);
            }
            self.refusal = None;
            requests.rearm(self.source.expect("RegistrationControl source missing"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if let Some((task, max_sources)) = self.queued.take() {
            match requests.spawn(task.into_service(), max_sources) {
                Ok(()) => self.gate_pending = true,
                Err(task) => {
                    self.queued = Some((task.into_registration_pending(), max_sources));
                }
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.gate_pending {
            self.gate_pending = false;
            requests.rearm(self.source.expect("RegistrationControl source missing"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let Some(RegistrationEndpoint::Authority(mailbox)) = world.registration_endpoint.as_ref()
        else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        };
        if self.source.is_none() && !self.requested {
            requests.add_source(
                mailbox.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_REGISTRATION,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let handle = match world.registration_endpoint.as_ref() {
            Some(RegistrationEndpoint::Authority(mailbox)) => mailbox.as_handle(),
            _ => return Err(SystemCallError::InternalError),
        };
        match self.buffer.receive(handle) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                requests.rearm(self.source.expect("RegistrationControl source missing"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        let message = MessageStorage::new()?.take(&mut self.buffer)?;
        let mut context = match RequestContext::decode(message, service_protocol::ID) {
            Ok(context) => context,
            Err(_) => {
                requests.rearm(self.source.expect("RegistrationControl source missing"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let (header, decoded) = match service_protocol::decode_request(&context.payload) {
            Ok(parsed) => parsed,
            Err(_) => {
                requests.rearm(self.source.expect("RegistrationControl source missing"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        if context.handles.remaining() != service_protocol::request_capability_count(header.op) {
            requests.rearm(self.source.expect("RegistrationControl source missing"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let command = (|| {
            Ok::<_, SystemCallError>(match decoded {
                service_protocol::Request::Register {
                    name,
                    protocol,
                    version,
                    policy,
                    establish_deadline,
                } => RegistrationCommand::Register {
                    name: copy_request_name(name)?,
                    protocol,
                    version,
                    policy,
                    establish_deadline,
                    endpoint: context
                        .handles
                        .take(1)
                        .map_err(|_| SystemCallError::IllegalArgument)?,
                },
                service_protocol::Request::QueryName { name } => {
                    RegistrationCommand::QueryName(copy_request_name(name)?)
                }
                service_protocol::Request::Withdraw {
                    name,
                    expected_instance,
                    expected_generation,
                } => RegistrationCommand::Withdraw(
                    copy_request_name(name)?,
                    expected_instance,
                    expected_generation,
                ),
                service_protocol::Request::PublishReady => RegistrationCommand::PublishReady,
                service_protocol::Request::BeginDrain => RegistrationCommand::BeginDrain,
                service_protocol::Request::Query => RegistrationCommand::Query,
                service_protocol::Request::DelegateName { name } => {
                    RegistrationCommand::DelegateName(copy_request_name(name)?)
                }
            })
        })();
        let sender_context = context.envelope.sender_context_id;
        let now_ns = input.now_ns();
        let mut reply_until = now_ns.saturating_add(5_000_000_000);
        if let Some(caller_until) = header
            .deadline
            .instant()
            .map_err(|_| SystemCallError::IllegalArgument)?
        {
            reply_until = reply_until.min(caller_until);
        }
        if let Ok(RegistrationCommand::Register {
            establish_deadline, ..
        }) = &command
        {
            let establish_until = establish_deadline
                .instant()
                .map_err(|_| SystemCallError::IllegalArgument)?
                .ok_or(SystemCallError::IllegalArgument)?;
            reply_until = reply_until.min(establish_until);
        }
        let outbox = match Outbox::prepare(
            context,
            PAYLOAD_MAX - librpc::PREFIX_LEN,
            Deadline::at(reply_until),
            KIND_REPLY,
        ) {
            Ok(outbox) => outbox,
            Err(_) => {
                requests.rearm(self.source.expect("RegistrationControl source missing"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let mut status = service_protocol::Status::Ok;
        let mut response = service_protocol::Response::Empty;
        let mut issued = None;
        let mut authority_issue = None;
        let mut action = None;
        let wakes = if matches!(
            command,
            Ok(RegistrationCommand::Register { .. }
                | RegistrationCommand::Withdraw(..)
                | RegistrationCommand::PublishReady
                | RegistrationCommand::BeginDrain)
        ) {
            WakeBatch::new(WATCH_LIMIT + 1)
        } else {
            WakeBatch::new(0)
        };
        let wakes = match wakes {
            Ok(wakes) => wakes,
            Err(_) => {
                status = service_protocol::Status::Resource;
                WakeBatch::new(0).expect("zero-capacity WakeBatch does not allocate")
            }
        };
        if let Err(error) = &command {
            status = service_status(RegistryError::Resource(*error));
        }
        if status == service_protocol::Status::Ok {
            match command.expect("validated registration command lost") {
                RegistrationCommand::Register {
                    name,
                    protocol,
                    version,
                    policy,
                    establish_deadline,
                    endpoint,
                } => {
                    if let Err(error) = check_registration_authority(world, sender_context, &name) {
                        status = service_status(error);
                        drop(endpoint);
                    } else {
                        let charges = (|| {
                            let registry = world
                                .provider
                                .backend
                                .as_ref()
                                .and_then(RuntimeBackend::registry)
                                .ok_or(RegistryError::Closed)?;
                            Ok::<_, RegistryError>((
                                registry.reserve_wait_source()?,
                                registry.reserve_wait_source()?,
                            ))
                        })();
                        match charges {
                            Ok((control_charge, observation_charge)) => {
                                match endpoint.duplicate(Rights::WAIT) {
                                    Ok(observation) => {
                                        let minted = match world.registration_endpoint.as_ref() {
                                            Some(RegistrationEndpoint::Authority(mailbox)) => {
                                                mailbox.mint(
                                                    0,
                                                    Rights::WRITE
                                                        | Rights::WAIT
                                                        | Rights::TRANSIT
                                                        | Rights::DUPLICATE,
                                                )
                                            }
                                            _ => Err(SystemCallError::ObjectClosed),
                                        };
                                        match minted {
                                            Ok(minted) => {
                                                let identity = minted
                                                    .sender
                                                    .description()
                                                    .expect("minted control description failed")
                                                    .object_id;
                                                issued = Some(IssuedRegistration {
                                                    pending: PendingRegistration {
                                                        authority_identity: sender_context,
                                                        name,
                                                        protocol,
                                                        version,
                                                        policy,
                                                        establish_deadline,
                                                        endpoint,
                                                        instance: identity,
                                                    },
                                                    sender: minted.sender,
                                                    lifetime: minted.lifetime,
                                                    endpoint_observer: observation,
                                                    lifetime_charge: control_charge,
                                                    endpoint_charge: observation_charge,
                                                });
                                            }
                                            Err(error) => {
                                                status =
                                                    service_status(RegistryError::Resource(error));
                                                drop(endpoint);
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        status = service_status(RegistryError::Resource(error));
                                        drop(endpoint);
                                    }
                                }
                            }
                            Err(error) => {
                                status = service_status(error);
                                drop(endpoint);
                            }
                        }
                    }
                }
                RegistrationCommand::QueryName(name) => {
                    let result = check_registration_authority(world, sender_context, &name)
                        .and_then(|()| {
                            world
                                .provider
                                .backend
                                .as_mut()
                                .and_then(RuntimeBackend::registry_mut)
                                .ok_or(RegistryError::Closed)?
                                .query_name(sender_context, &name)
                        });
                    match result {
                        Ok(info) => response = service_protocol::Response::Instance(info),
                        Err(error) => status = service_status(error),
                    }
                }
                RegistrationCommand::Withdraw(name, instance, generation) => {
                    action = Some(RegistrationCommand::Withdraw(name, instance, generation));
                }
                RegistrationCommand::PublishReady => {
                    action = Some(RegistrationCommand::PublishReady);
                }
                RegistrationCommand::BeginDrain => {
                    action = Some(RegistrationCommand::BeginDrain);
                }
                RegistrationCommand::Query => {
                    let control = world
                        .registration_controls
                        .iter()
                        .copied()
                        .find(|control| control.instance == sender_context);
                    match control.and_then(|control| {
                        world
                            .provider
                            .backend
                            .as_mut()
                            .and_then(RuntimeBackend::registry_mut)
                            .and_then(|registry| registry.query(control.instance).ok())
                    }) {
                        Some(info) => response = service_protocol::Response::Instance(info),
                        None => status = service_protocol::Status::Permission,
                    }
                }
                RegistrationCommand::DelegateName(name) => {
                    let result = (|| {
                        let registry = world
                            .provider
                            .backend
                            .as_ref()
                            .and_then(RuntimeBackend::registry)
                            .ok_or(RegistryError::Closed)?;
                        let prepared = registry.prepare_name_authority(sender_context, &name)?;
                        let charge = registry.reserve_wait_source()?;
                        let minted = match world.registration_endpoint.as_ref() {
                            Some(RegistrationEndpoint::Authority(mailbox)) => mailbox
                                .mint(
                                    0,
                                    Rights::WRITE
                                        | Rights::WAIT
                                        | Rights::TRANSIT
                                        | Rights::DUPLICATE
                                        | Rights::GRANT,
                                )
                                .map_err(RegistryError::Resource)?,
                            _ => return Err(RegistryError::Closed),
                        };
                        Ok::<_, RegistryError>((prepared, minted, charge))
                    })();
                    match result {
                        Ok(issue) => authority_issue = Some(issue),
                        Err(error) => status = service_status(error),
                    }
                }
            }
        }
        let (task, max_sources) = if let Some((prepared, minted, charge)) = authority_issue {
            (
                ServiceTask::Authority(AuthorityTask::reply(prepared, minted, charge, outbox)),
                2,
            )
        } else {
            (
                ServiceTask::RegistrationReply(RegistrationReplyTask::new(
                    outbox,
                    header.op,
                    status,
                    response,
                    issued,
                    action.map(|command| (sender_context, command)),
                    wakes,
                )),
                3,
            )
        };
        match requests.spawn(task, max_sources) {
            Ok(()) => self.gate_pending = true,
            Err(task) => self.queued = Some((task.into_registration_pending(), max_sources)),
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Runnable,
        })
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        match failure {
            RequestFailure::Spawn { task, error } => self.reject_task(task, error, world),
            failure @ RequestFailure::Source {
                kind: KIND_REPLY, ..
            } => {
                if let Some(outbox) = self.refusal.as_mut() {
                    outbox.refused(world, failure);
                }
            }
            RequestFailure::Source { kind, error } => {
                rinlib::debug!(
                    "RegistrationControl source refused: kind={}, error={:?}",
                    kind,
                    error
                );
                world.failed = true;
            }
            RequestFailure::Wake { task, error } => {
                rinlib::debug!(
                    "RegistrationControl wake refused: task={}, error={:?}",
                    task,
                    error
                );
                world.failed = true;
            }
        }
    }
    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_REGISTRATION {
            self.source = Some(source);
            self.requested = false;
        } else if kind == KIND_REPLY
            && let Some(outbox) = self.refusal.as_mut()
        {
            outbox.registered(world, kind, source);
        }
    }
    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_REGISTRATION && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        } else if kind == KIND_REPLY
            && let Some(outbox) = self.refusal.as_mut()
        {
            outbox.unregistered(world, kind, source);
        }
    }
    fn stop(&mut self, world: &mut World<B>) {
        self.stopping = true;
        self.queued = None;
        if let Some(outbox) = self.refusal.as_mut() {
            outbox.stop(world);
        }
    }
    fn deadline(&self) -> Deadline {
        self.refusal
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline)
    }
}

enum RegistrationClientPhase {
    Register,
    PublishReady,
    Published { control: Capability },
}

struct RegistrationClientTask {
    phase: RegistrationClientPhase,
    dispatch: DownstreamState,
    deadline: Deadline,
    stopping: bool,
}

impl RegistrationClientTask {
    fn new(
        service: Capability,
        endpoint: Capability,
        deadline: Deadline,
    ) -> Result<Self, SystemCallError> {
        let policy = ExportPolicy {
            protocol: ValueProtocol::Directory,
            mode: ExportMode::Repeatable,
            transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
            fal_ceiling: FalRights::ALL,
        };
        let request = service_protocol::Request::Register {
            name: "fs.secondary",
            protocol: protocol::ID,
            version: protocol::VERSION as u32,
            policy,
            establish_deadline: deadline,
        };
        let mut payload = [0; PAYLOAD_MAX - librpc::PREFIX_LEN];
        let used = service_protocol::encode_request(&request, deadline, &mut payload)
            .ok_or(SystemCallError::InternalError)?;
        let mut request = RpcRequest::new(service_protocol::ID, &payload[..used])?;
        request
            .push(endpoint, policy.transport)
            .map_err(|failure| failure.error)?;
        Ok(Self {
            phase: RegistrationClientPhase::Register,
            dispatch: DownstreamState::Ready { service, request },
            deadline,
            stopping: false,
        })
    }

    fn submitted(&mut self, result: Result<u64, Completion>) {
        assert!(matches!(self.dispatch, DownstreamState::Queued));
        self.dispatch = match result {
            Ok(txid) => DownstreamState::Pending(txid),
            Err(completion) => DownstreamState::Complete(completion),
        };
    }

    fn completed(&mut self, txid: u64, completion: Completion) {
        assert!(matches!(self.dispatch, DownstreamState::Pending(pending) if pending == txid));
        self.dispatch = DownstreamState::Complete(completion);
    }

    fn consume_completion<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
    ) -> Result<(), SystemCallError> {
        let completion = match core::mem::replace(&mut self.dispatch, DownstreamState::Queued) {
            DownstreamState::Complete(completion) => completion,
            state => {
                self.dispatch = state;
                return Ok(());
            }
        };
        let Completion { service, result } = completion;
        let mut reply = result.map_err(|_| SystemCallError::ObjectClosed)?;
        let (header, response) = service_protocol::decode_response(&reply.payload)
            .map_err(|_| SystemCallError::InternalError)?;
        if header.status != service_protocol::Status::Ok {
            return Err(SystemCallError::InternalError);
        }
        match self.phase {
            RegistrationClientPhase::Register => {
                let service_protocol::Response::Instance(info) = response else {
                    return Err(SystemCallError::InternalError);
                };
                if info.state != service_protocol::State::Starting || reply.handles.remaining() != 1
                {
                    return Err(SystemCallError::InternalError);
                }
                drop(service);
                let control = reply
                    .handles
                    .take(0)
                    .map_err(|_| SystemCallError::InternalError)?;
                let request_body = service_protocol::Request::PublishReady;
                let mut payload = [0; service_protocol::HEADER_LEN];
                let used =
                    service_protocol::encode_request(&request_body, header.deadline, &mut payload)
                        .ok_or(SystemCallError::InternalError)?;
                let request = RpcRequest::new(service_protocol::ID, &payload[..used])?;
                self.phase = RegistrationClientPhase::PublishReady;
                self.dispatch = DownstreamState::Ready {
                    service: control,
                    request,
                };
            }
            RegistrationClientPhase::PublishReady => {
                let service_protocol::Response::Instance(info) = response else {
                    return Err(SystemCallError::InternalError);
                };
                if info.state != service_protocol::State::Ready || !reply.handles.is_empty() {
                    return Err(SystemCallError::InternalError);
                }
                self.phase = RegistrationClientPhase::Published { control: service };
                world.registration_ready = true;
            }
            RegistrationClientPhase::Published { .. } => {
                return Err(SystemCallError::InternalError);
            }
        }
        Ok(())
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RegistrationClientTask {
    type Family = ServiceTask<B>;
    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        _input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if !matches!(self.phase, RegistrationClientPhase::Published { .. })
                && matches!(
                    self.dispatch,
                    DownstreamState::Queued | DownstreamState::Pending(_)
                )
            {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if let RegistrationClientPhase::Published { control } =
                core::mem::replace(&mut self.phase, RegistrationClientPhase::PublishReady)
            {
                drop(control);
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if matches!(self.dispatch, DownstreamState::Ready { .. }) {
            if world.dispatch_intent.is_some() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            requests.wake(world.dispatcher_task)?;
            let state = core::mem::replace(&mut self.dispatch, DownstreamState::Queued);
            let DownstreamState::Ready { service, request } = state else {
                unreachable!()
            };
            world.dispatch_intent = Some(DispatchIntent::Submit(DispatchSubmission {
                waiter: id,
                service,
                deadline: self.deadline,
                request,
            }));
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if matches!(self.dispatch, DownstreamState::Complete(_)) {
            self.consume_completion(world)?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }
    fn refused(&mut self, world: &mut World<B>, _failure: RequestFailure<ServiceTask<B>>) {
        world.failed = true;
    }
    fn registered(&mut self, _world: &mut World<B>, _kind: SourceKind, _source: SourceId) {}
    fn unregistered(&mut self, _world: &mut World<B>, _kind: SourceKind, _source: SourceId) {}
    fn stop(&mut self, _world: &mut World<B>) {
        self.stopping = true;
    }
    fn deadline(&self) -> Deadline {
        match self.phase {
            RegistrationClientPhase::Published { .. } => Deadline::INFINITE,
            _ => self.deadline,
        }
    }
}

#[allow(
    clippy::large_enum_variant,
    reason = "Runtime 按 Task 上限预分配整族 owner，内联避免绕过额度的额外堆分配"
)]
enum ProviderTask<B: RuntimeBackend> {
    Delegate(DelegateTask),
    Grant(GrantTask),
    Ingress(Ingress<B>),
    Read(ReadTask),
    Request(RequestTask<B>),
    Retire(RetireTask),
    Watch(WatchTask),
}

#[allow(
    clippy::large_enum_variant,
    reason = "Runtime 按 Task 上限预分配整族 owner，内联避免绕过额度的额外堆分配"
)]
enum ServiceTask<B: RuntimeBackend> {
    Fal(ProviderTask<B>),
    Authority(AuthorityTask),
    Dispatcher(Dispatcher),
    Release(ReleaseTask),
    RegistrationClient(RegistrationClientTask),
    Route(RouteIngress),
    RouteReply(RouteReplyTask),
    RegistrationIngress(RegistrationIngress),
    RegistrationReply(RegistrationReplyTask),
    Stream(StreamTask),
    StreamControl(StreamControlTask),
}

impl<B: RuntimeBackend> From<ProviderTask<B>> for ServiceTask<B> {
    fn from(task: ProviderTask<B>) -> Self {
        Self::Fal(task)
    }
}

impl<B: RuntimeBackend> ServiceTask<B> {
    fn into_registration_refusal(mut self) -> (Outbox, service_protocol::Op) {
        match &mut self {
            Self::Authority(task) => (
                task.take_refused_outbox()
                    .expect("authority refusal lost reply owner"),
                service_protocol::Op::DelegateName,
            ),
            Self::RegistrationReply(task) => task
                .take_refused_outbox()
                .expect("registration refusal lost reply owner"),
            _ => panic!("registration admission returned unrelated task"),
        }
    }
    fn into_registration_pending(self) -> PendingRegistrationTask {
        match self {
            Self::Authority(task) => PendingRegistrationTask::Authority(task),
            Self::RegistrationReply(task) => PendingRegistrationTask::Reply(task),
            _ => panic!("registration admission returned unrelated task"),
        }
    }
    fn into_fal_pending(self) -> PendingFalTask<B> {
        if let Self::Stream(task) = self {
            return PendingFalTask::Stream(task);
        }
        if let Self::StreamControl(task) = self {
            return PendingFalTask::StreamControl(task);
        }
        let Self::Fal(task) = self else {
            panic!("FAL admission returned unrelated task");
        };
        task.into_pending()
    }
    fn into_fal_refusal(self) -> Outbox {
        if let Self::Stream(task) = self {
            return task.into_refused_outbox();
        }
        if let Self::StreamControl(task) = self {
            return task.into_refused_outbox();
        }
        let Self::Fal(task) = self else {
            panic!("FAL admission returned unrelated task");
        };
        task.into_refused_outbox()
    }
}

impl<B: RuntimeBackend> ProviderTask<B> {
    fn into_pending(self) -> PendingFalTask<B> {
        match self {
            Self::Request(task) => PendingFalTask::Request(task),
            Self::Read(task) => PendingFalTask::Read(task),
            Self::Grant(task) => PendingFalTask::Grant(task),
            Self::Watch(task) => PendingFalTask::Watch(task),
            Self::Delegate(task) => PendingFalTask::Delegate(task),
            _ => panic!("FAL admission returned unrelated task"),
        }
    }
    fn into_refused_outbox(self) -> Outbox {
        match self {
            Self::Request(task) => task.outbox,
            Self::Read(task) => task.outbox,
            Self::Delegate(task) => task.outbox,
            Self::Grant(mut task) => task.take_refused_outbox(),
            Self::Watch(mut task) => task.take_refused_outbox(),
            _ => panic!("FAL admission returned unrelated task"),
        }
    }
}

impl<B: RuntimeBackend> Task<World<B>> for ProviderTask<B> {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<Self::Family>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Delegate(task) => task.advance(id, world, requests, input, budget),
            Self::Grant(task) => task.advance(id, world, requests, input, budget),
            Self::Ingress(task) => task.advance(id, world, requests, input, budget),
            Self::Read(task) => task.advance(id, world, requests, input, budget),
            Self::Request(task) => task.advance(id, world, requests, input, budget),
            Self::Retire(task) => task.advance(id, world, requests, input, budget),
            Self::Watch(task) => task.advance(id, world, requests, input, budget),
        }
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<Self::Family>) {
        match self {
            Self::Delegate(task) => task.refused(world, failure),
            Self::Grant(task) => task.refused(world, failure),
            Self::Ingress(task) => task.refused(world, failure),
            Self::Read(task) => task.refused(world, failure),
            Self::Request(task) => task.refused(world, failure),
            Self::Retire(task) => task.refused(world, failure),
            Self::Watch(task) => task.refused(world, failure),
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match self {
            Self::Delegate(task) => task.registered(world, kind, source),
            Self::Grant(task) => task.registered(world, kind, source),
            Self::Ingress(task) => task.registered(world, kind, source),
            Self::Read(task) => task.registered(world, kind, source),
            Self::Request(task) => task.registered(world, kind, source),
            Self::Retire(task) => task.registered(world, kind, source),
            Self::Watch(task) => task.registered(world, kind, source),
        }
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match self {
            Self::Delegate(task) => task.unregistered(world, kind, source),
            Self::Grant(task) => task.unregistered(world, kind, source),
            Self::Ingress(task) => task.unregistered(world, kind, source),
            Self::Read(task) => task.unregistered(world, kind, source),
            Self::Request(task) => task.unregistered(world, kind, source),
            Self::Retire(task) => task.unregistered(world, kind, source),
            Self::Watch(task) => task.unregistered(world, kind, source),
        }
    }

    fn stop(&mut self, world: &mut World<B>) {
        match self {
            Self::Delegate(task) => task.stop(world),
            Self::Grant(task) => task.stop(),
            Self::Ingress(task) => task.stop(world),
            Self::Read(task) => task.stop(world),
            Self::Request(task) => task.stop(world),
            Self::Retire(task) => task.stop(world),
            Self::Watch(task) => task.stop(),
        }
    }

    fn deadline(&self) -> Deadline {
        match self {
            Self::Delegate(task) => <DelegateTask as Task<World<B>>>::deadline(task),
            Self::Grant(task) => <GrantTask as Task<World<B>>>::deadline(task),
            Self::Read(task) => <ReadTask as Task<World<B>>>::deadline(task),
            Self::Request(task) => <RequestTask<B> as Task<World<B>>>::deadline(task),
            Self::Watch(task) => <WatchTask as Task<World<B>>>::deadline(task),
            Self::Ingress(task) => <Ingress<B> as Task<World<B>>>::deadline(task),
            Self::Retire(_) => Deadline::INFINITE,
        }
    }
}

impl<B: RuntimeBackend> Task<World<B>> for ServiceTask<B> {
    type Family = Self;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Fal(task) => task.advance(id, world, requests, input, budget),
            Self::Dispatcher(task) => task.advance(requests, input, budget),
            Self::Authority(task) => task.advance(id, world, requests, input, budget),
            Self::Release(task) => task.advance(id, world, requests, input, budget),
            Self::Route(task) => task.advance(id, world, requests, input, budget),
            Self::RouteReply(task) => task.advance(id, world, requests, input, budget),
            Self::RegistrationClient(task) => task.advance(id, world, requests, input, budget),
            Self::RegistrationIngress(task) => task.advance(id, world, requests, input, budget),
            Self::RegistrationReply(task) => task.advance(id, world, requests, input, budget),
            Self::Stream(task) => task.advance(id, world, requests, input, budget),
            Self::StreamControl(task) => task.advance(id, world, requests, input, budget),
        }
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<Self>) {
        match self {
            Self::Fal(task) => task.refused(world, failure),
            Self::Dispatcher(task) => task.refused(world, failure),
            Self::Authority(task) => task.refused(world, failure),
            Self::Release(task) => task.refused(world, failure),
            Self::Route(task) => task.refused(world, failure),
            Self::RouteReply(task) => task.refused(world, failure),
            Self::RegistrationClient(task) => task.refused(world, failure),
            Self::RegistrationIngress(task) => task.refused(world, failure),
            Self::RegistrationReply(task) => task.refused(world, failure),
            Self::Stream(task) => task.refused(world, failure),
            Self::StreamControl(task) => task.refused(world, failure),
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match self {
            Self::Fal(task) => task.registered(world, kind, source),
            Self::Dispatcher(task) => task.registered(world, kind, source),
            Self::Authority(task) => task.registered(world, kind, source),
            Self::Release(task) => task.registered(world, kind, source),
            Self::Route(task) => task.registered(world, kind, source),
            Self::RouteReply(task) => task.registered(world, kind, source),
            Self::RegistrationClient(task) => task.registered(world, kind, source),
            Self::RegistrationIngress(task) => task.registered(world, kind, source),
            Self::RegistrationReply(task) => task.registered(world, kind, source),
            Self::Stream(task) => task.registered(world, kind, source),
            Self::StreamControl(task) => task.registered(world, kind, source),
        }
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match self {
            Self::Fal(task) => task.unregistered(world, kind, source),
            Self::Dispatcher(task) => task.unregistered(world, kind, source),
            Self::Authority(task) => task.unregistered(world, kind, source),
            Self::Release(task) => task.unregistered(world, kind, source),
            Self::Route(task) => task.unregistered(world, kind, source),
            Self::RouteReply(task) => task.unregistered(world, kind, source),
            Self::RegistrationClient(task) => task.unregistered(world, kind, source),
            Self::RegistrationIngress(task) => task.unregistered(world, kind, source),
            Self::RegistrationReply(task) => task.unregistered(world, kind, source),
            Self::Stream(task) => task.unregistered(world, kind, source),
            Self::StreamControl(task) => task.unregistered(world, kind, source),
        }
    }

    fn stop(&mut self, world: &mut World<B>) {
        match self {
            Self::Fal(task) => task.stop(world),
            Self::Dispatcher(task) => task.stop(world),
            Self::Authority(task) => task.stop(world),
            Self::Release(task) => task.stop(world),
            Self::Route(task) => task.stop(world),
            Self::RouteReply(task) => task.stop(world),
            Self::RegistrationClient(task) => task.stop(world),
            Self::RegistrationIngress(task) => task.stop(world),
            Self::RegistrationReply(task) => task.stop(world),
            Self::Stream(task) => task.stop(world),
            Self::StreamControl(task) => task.stop(world),
        }
    }

    fn deadline(&self) -> Deadline {
        match self {
            Self::Fal(task) => <ProviderTask<B> as Task<World<B>>>::deadline(task),
            Self::Dispatcher(task) => task.deadline(),
            Self::RegistrationIngress(task) => {
                <RegistrationIngress as Task<World<B>>>::deadline(task)
            }
            Self::Release(_) | Self::Route(_) => Deadline::INFINITE,
            Self::RouteReply(task) => <RouteReplyTask as Task<World<B>>>::deadline(task),
            Self::Authority(task) => <AuthorityTask as Task<World<B>>>::deadline(task),
            Self::RegistrationClient(task) => {
                <RegistrationClientTask as Task<World<B>>>::deadline(task)
            }
            Self::RegistrationReply(task) => {
                <RegistrationReplyTask as Task<World<B>>>::deadline(task)
            }
            Self::Stream(task) => <StreamTask as Task<World<B>>>::deadline(task),
            Self::StreamControl(task) => <StreamControlTask as Task<World<B>>>::deadline(task),
        }
    }
}

impl<B: RuntimeBackend> ProviderTask<B> {
    fn downstream_submitted(&mut self, result: Result<u64, Completion>) -> bool {
        match self {
            Self::Delegate(task) => task.submitted(result),
            Self::Read(task) => task.submitted(result),
            _ => return false,
        }
        true
    }

    fn downstream_completed(&mut self, txid: u64, completion: Completion) -> bool {
        match self {
            Self::Delegate(task) => task.completed(txid, completion),
            Self::Read(task) => task.completed(txid, completion),
            _ => return false,
        }
        true
    }
}

impl<B: RuntimeBackend> ServiceTask<B> {
    fn downstream_submitted(&mut self, result: Result<u64, Completion>) -> bool {
        match self {
            Self::Fal(task) => task.downstream_submitted(result),
            Self::RegistrationClient(task) => {
                task.submitted(result);
                true
            }
            _ => false,
        }
    }

    fn downstream_completed(&mut self, txid: u64, completion: Completion) -> bool {
        match self {
            Self::Fal(task) => task.downstream_completed(txid, completion),
            Self::RegistrationClient(task) => {
                task.completed(txid, completion);
                true
            }
            _ => false,
        }
    }
}

fn progress_dispatch<B: RuntimeBackend>(
    runtime: &mut Runtime<ServiceTask<B>, WaitSet>,
    world: &mut World<B>,
    bindings: &mut Vec<(u64, u64)>,
) -> Result<(), SystemCallError> {
    if let Some(intent) = world.dispatch_intent.take() {
        match intent {
            DispatchIntent::Submit(submission) => {
                let waiter = submission.waiter;
                let started = {
                    let Some(ServiceTask::Dispatcher(dispatcher)) =
                        runtime.get_task_mut(world.dispatcher_task)
                    else {
                        return Err(SystemCallError::InternalError);
                    };
                    dispatcher.begin_for(
                        submission.service,
                        submission.deadline,
                        submission.request,
                        Some(waiter),
                    )
                };
                match started {
                    Ok(txid) => {
                        if bindings.len() == bindings.capacity() {
                            return Err(SystemCallError::ReachLimit);
                        }
                        bindings.push((txid, waiter));
                        let Some(task) = runtime.get_task_mut(waiter) else {
                            return Err(SystemCallError::InternalError);
                        };
                        if !task.downstream_submitted(Ok(txid)) {
                            return Err(SystemCallError::InternalError);
                        }
                        runtime.wake(world.dispatcher_task)?;
                    }
                    Err(failure) => {
                        let completion = Completion {
                            service: failure.service,
                            result: Err(failure.error),
                        };
                        let Some(task) = runtime.get_task_mut(waiter) else {
                            return Err(SystemCallError::InternalError);
                        };
                        if !task.downstream_submitted(Err(completion)) {
                            return Err(SystemCallError::InternalError);
                        }
                        runtime.wake(waiter)?;
                    }
                }
            }
            DispatchIntent::Cancel { txid } => {
                let Some(ServiceTask::Dispatcher(dispatcher)) =
                    runtime.get_task_mut(world.dispatcher_task)
                else {
                    return Err(SystemCallError::InternalError);
                };
                dispatcher.cancel(txid);
                runtime.wake(world.dispatcher_task)?;
            }
        }
    }
    if bindings.is_empty() {
        return Ok(());
    }
    loop {
        let completed = {
            let Some(ServiceTask::Dispatcher(dispatcher)) =
                runtime.get_task_mut(world.dispatcher_task)
            else {
                return Err(SystemCallError::InternalError);
            };
            dispatcher.pop_completed()
        };
        let Some((txid, completion)) = completed else {
            break;
        };
        let Some(index) = bindings.iter().position(|(pending, _)| *pending == txid) else {
            return Err(SystemCallError::InternalError);
        };
        let (_, waiter) = bindings.swap_remove(index);
        if matches!(
            &completion.result,
            Err(CallError {
                phase: librpc::CallPhase::Sent,
                ..
            })
        ) {
            world.downstream_abandoned = world
                .downstream_abandoned
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
        }
        let Some(task) = runtime.get_task_mut(waiter) else {
            return Err(SystemCallError::InternalError);
        };
        if !task.downstream_completed(txid, completion) {
            return Err(SystemCallError::InternalError);
        }
        runtime.wake(waiter)?;
    }
    Ok(())
}

pub(super) fn run(
    mailbox: Mailbox,
    bootstrap: MailboxSender,
    route_mailbox: Handle,
    release: Handle,
    registration: Handle,
) {
    let registration_role = query(registration)
        .expect("provider registration endpoint query failed")
        .role;
    let registration_endpoint = match registration_role {
        role if role == HandleRole::MailboxOwner as u32 => {
            // SAFETY: StartupBlock transfers the unique RegistrationControl Mailbox owner to provider A.
            let capability = unsafe { Capability::from_raw(registration) };
            let (mailbox, _) = Mailbox::from_capability(capability)
                .map_err(|failure| failure.error)
                .expect("provider registration Mailbox has an invalid role");
            RegistrationEndpoint::Authority(mailbox)
        }
        role if role == HandleRole::MailboxSender as u32 => {
            // SAFETY: StartupBlock transfers the unique RegistrationControl sender to provider B.
            let capability = unsafe { Capability::from_raw(registration) };
            let (sender, _) = MailboxSender::from_capability(capability)
                .map_err(|failure| failure.error)
                .expect("provider registration sender has an invalid role");
            RegistrationEndpoint::Client(sender)
        }
        _ => panic!("provider registration endpoint has an invalid role"),
    };
    let has_registry = matches!(registration_endpoint, RegistrationEndpoint::Authority(_));
    run_with_backend(
        mailbox,
        bootstrap,
        route_mailbox,
        release,
        registration_endpoint,
        move |account, service_account, wake| {
            let backend = ServiceBackend::new(account, service_account, wake, has_registry)?;
            let registry_root = backend.registry_root();
            Ok((backend, registry_root))
        },
    );
}

fn run_with_backend<B, F>(
    mailbox: Mailbox,
    bootstrap: MailboxSender,
    route_mailbox: Handle,
    release: Handle,
    registration_endpoint: RegistrationEndpoint,
    backend_factory: F,
) where
    B: RuntimeBackend,
    F: FnOnce(
        &AccountView<FalResource>,
        &AccountView<ServiceResource>,
        Rc<dyn libexecution::wake::Wake>,
    ) -> Result<(B, Option<NodeRef>), BackendError>,
{
    let registration_authority =
        matches!(&registration_endpoint, RegistrationEndpoint::Authority(_));
    let registration_client_expected =
        matches!(&registration_endpoint, RegistrationEndpoint::Client(_));
    let task_limit = TASK_LIMIT;
    let source_limit = SOURCE_LIMIT;
    let input_bytes = Runtime::<ServiceTask<B>, WaitSet>::input_budget(source_limit)
        .expect("provider Runtime input budget calculation failed");
    let execution_layout = [0, 1];
    let fal_layout = [2, 3, 4, 5, 6, 7, 8, 9, 10];
    let service_layout = [11, 12, 13, 14];
    let mut limits = [0; ExecutionResource::COUNT + FalResource::COUNT + ServiceResource::COUNT];
    limits[execution_layout[ExecutionResource::Task.slot()]] = task_limit;
    limits[execution_layout[ExecutionResource::InputBytes.slot()]] = input_bytes;
    limits[fal_layout[FalResource::Node.slot()]] = MEMORY_NODE_LIMIT + REGISTRATION_LIMIT + 1;
    limits[fal_layout[FalResource::Bytes.slot()]] = 2 * 1024 * 1024;
    limits[fal_layout[FalResource::Grant.slot()]] = GRANT_LIMIT;
    limits[fal_layout[FalResource::Watch.slot()]] = WATCH_LIMIT;
    limits[fal_layout[FalResource::WaitSource.slot()]] =
        GRANT_LIMIT + WATCH_LIMIT + 2 * STREAM_LIMIT;
    limits[service_layout[ServiceResource::Authority.slot()]] = AUTHORITY_LIMIT;
    limits[service_layout[ServiceResource::Registration.slot()]] = REGISTRATION_LIMIT;
    limits[service_layout[ServiceResource::Bytes.slot()]] = 64 * 1024;
    limits[service_layout[ServiceResource::WaitSource.slot()]] =
        AUTHORITY_LIMIT + 2 * REGISTRATION_LIMIT;
    let budget = Budget::new(&limits, 1).expect("provider Runtime budget creation failed");
    let execution_binding = execution_layout.map(|index| budget.slot(index).unwrap());
    let fal_binding = fal_layout.map(|index| budget.slot(index).unwrap());
    let service_binding = service_layout.map(|index| budget.slot(index).unwrap());
    let account = budget
        .account(&limits)
        .expect("provider Runtime account creation failed");
    let execution_account = account
        .view::<ExecutionResource>(&execution_binding)
        .expect("provider execution budget binding failed");
    let fal_account = account
        .view::<FalResource>(&fal_binding)
        .expect("provider FAL budget binding failed");
    let service_account = account
        .view::<ServiceResource>(&service_binding)
        .expect("provider service budget binding failed");
    let event = notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
        .expect("provider retirement notification creation failed");
    // SAFETY: NotificationCreate returned two fresh affine entries; this worker owns both.
    let retire_owner = unsafe { Capability::from_raw(event.owner) };
    // SAFETY: the peer entry is the unique signaler owner and is transferred into NotificationWake.
    let retire_signaler = unsafe { Capability::from_raw(event.peer) };
    let wake = NotificationWake::new(retire_signaler, RETIRE_BIT)
        .map_err(|(_, error)| error)
        .expect("provider retirement wake validation failed");
    let (mut backend, registry_root) =
        backend_factory(&fal_account, &service_account, Rc::new(wake))
            .expect("provider backend creation failed");
    let root_authority_task = if let RegistrationEndpoint::Authority(registration_mailbox) =
        &registration_endpoint
    {
        let registry = backend
            .registry_mut()
            .expect("registration owner requires Registry");
        let prepared = registry
            .prepare_root_authority()
            .expect("root registration authority preparation failed");
        let charge = registry
            .reserve_wait_source()
            .expect("root authority observation charge failed");
        let minted = registration_mailbox
            .mint(
                0,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT,
            )
            .expect("root registration authority mint failed");
        Some(AuthorityTask::root(prepared, minted, charge))
    } else {
        None
    };
    let mut grants =
        GrantTable::new(&mailbox, GRANT_LIMIT).expect("provider grant table creation failed");
    let root = backend
        .root()
        .expect("provider backend root missing")
        .clone();
    let prepared_root = grants
        .prepare_issue(
            &mailbox,
            root,
            Issuance {
                rights: FalRights::ALL,
                sender_transport: Rights::WRITE
                    | Rights::WAIT
                    | Rights::TRANSIT
                    | Rights::DUPLICATE
                    | Rights::GRANT,
                output_transport: Rights::TRANSIT,
            },
            fal_account.clone(),
            0,
        )
        .map_err(|failure| failure.error)
        .expect("provider root grant preparation failed");
    let registry_rights = FalRights::TRAVERSE
        .union(FalRights::ENUMERATE)
        .union(FalRights::READ_PROPERTY)
        .union(FalRights::WATCH)
        .union(FalRights::ACQUIRE_CAPABILITY);
    let prepared_registry_root = registry_root
        .map(|root| {
            grants.prepare_issue(
                &mailbox,
                root,
                Issuance {
                    rights: registry_rights,
                    sender_transport: Rights::WRITE
                        | Rights::WAIT
                        | Rights::TRANSIT
                        | Rights::DUPLICATE
                        | Rights::GRANT,
                    output_transport: Rights::TRANSIT,
                },
                fal_account.clone(),
                0,
            )
        })
        .transpose()
        .map_err(|failure| failure.error)
        .expect("Registry root grant preparation failed");
    let set = WaitSet::create(source_limit).expect("provider Runtime WaitSet creation failed");
    let mut runtime =
        Runtime::<ServiceTask<B>, WaitSet>::new(set, task_limit, source_limit, &execution_account)
            .expect("provider Runtime creation failed");
    let retire_task = runtime
        .spawn(
            ProviderTask::Retire(RetireTask::new(retire_owner)).into(),
            1,
        )
        .map_err(|failure| failure.error)
        .expect("provider retirement task admission failed");
    let dispatcher = Dispatcher::new(DISPATCH_LIMIT).expect("provider Dispatcher creation failed");
    let dispatcher_task = runtime
        .spawn(ServiceTask::Dispatcher(dispatcher), DISPATCH_LIMIT * 2 + 1)
        .map_err(|failure| failure.error)
        .expect("provider Dispatcher admission failed");
    let root_grant_task = runtime
        .spawn(
            ProviderTask::Grant(GrantTask::root(prepared_root, KIND_GRANT_LIFETIME)).into(),
            1,
        )
        .map_err(|failure| failure.error)
        .expect("provider root grant task admission failed");
    let registry_expected = prepared_registry_root.is_some();
    let registry_grant_task = prepared_registry_root.map(|prepared_registry_root| {
        runtime
            .spawn(
                ProviderTask::Grant(GrantTask::root(prepared_registry_root, KIND_GRANT_LIFETIME))
                    .into(),
                1,
            )
            .map_err(|failure| failure.error)
            .expect("Registry root grant task admission failed")
    });
    runtime
        .spawn(ProviderTask::Ingress(Ingress::new()).into(), 2)
        .map_err(|failure| failure.error)
        .expect("provider ingress admission failed");
    if let Some(root_authority_task) = root_authority_task {
        runtime
            .spawn(ServiceTask::Authority(root_authority_task), 1)
            .map_err(|failure| failure.error)
            .expect("root registration authority admission failed");
    }
    if registration_authority {
        runtime
            .spawn(
                ServiceTask::RegistrationIngress(RegistrationIngress::new()),
                2,
            )
            .map_err(|failure| failure.error)
            .expect("RegistrationControl ingress admission failed");
    }
    runtime
        .spawn(ServiceTask::Route(RouteIngress::new()), 1)
        .map_err(|failure| failure.error)
        .expect("provider route ingress admission failed");
    // SAFETY: StartupBlock transferred the unique release notification owner to this process.
    let release = unsafe { Capability::from_raw(release) };
    runtime
        .spawn(ServiceTask::Release(ReleaseTask::new(release)), 1)
        .map_err(|failure| failure.error)
        .expect("provider release task admission failed");
    let mut registration_controls = Vec::new();
    registration_controls
        .try_reserve_exact(REGISTRATION_LIMIT)
        .expect("registration control table allocation failed");
    let mut world = World {
        provider: provider::State {
            mailbox,
            backend: Some(backend),
            grants: Some(grants),
            watches: watch::Table::new(WATCH_LIMIT)
                .expect("provider Watch table allocation failed"),
            retire_task,
            retire_ready: false,
            backend_sealed: false,
            committed: 0,
            abandoned: 0,
        },
        route_mailbox,
        dispatch_intent: None,
        dispatcher_task,
        root_grant_task,
        registry_grant_task,
        registration_endpoint: Some(registration_endpoint),
        registration_ready: !registration_client_expected,
        root_sender: None,
        registry_sender: None,
        registration_root_sender: None,
        registration_controls,
        streams: StreamTable::new(STREAM_LIMIT),
        downstream_abandoned: 0,
        stop: false,
        failed: false,
    };
    let mut dispatch_bindings = Vec::new();
    dispatch_bindings
        .try_reserve_exact(DISPATCH_LIMIT)
        .expect("provider dispatch binding allocation failed");
    let mut sealing = false;
    let mut root_published = false;
    let mut registry_published = false;
    let mut registration_root_published = !registration_authority;
    let mut ready_published = false;
    loop {
        assert!(!world.failed, "provider Runtime entered a fatal state");
        if world.stop && !sealing {
            world
                .provider
                .grants
                .as_mut()
                .expect("provider grant table missing during seal")
                .seal();
            runtime.seal();
            sealing = true;
        }
        let result = if sealing {
            runtime.shutdown_turn(&mut world, 1)
        } else {
            runtime.turn(&mut world, 1)
        };
        result.expect("provider Runtime failed");
        progress_dispatch(&mut runtime, &mut world, &mut dispatch_bindings)
            .expect("provider dispatch integration failed");
        if let Some(sender) = world.root_sender.take() {
            if matches!(
                world.registration_endpoint,
                Some(RegistrationEndpoint::Client(_))
            ) {
                let endpoint = duplicate(
                    sender.as_handle(),
                    Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                )
                .map(|handle| {
                    // SAFETY: duplicate returned a fresh endpoint owner for Registry storage.
                    unsafe { Capability::from_raw(handle) }
                })
                .expect("business endpoint duplication for registration failed");
                let Some(RegistrationEndpoint::Client(target)) = world.registration_endpoint.take()
                else {
                    unreachable!()
                };
                let deadline = rinlib::time::timeout_millis(5_000)
                    .expect("registration deadline construction failed");
                let task =
                    RegistrationClientTask::new(target.into_capability(), endpoint, deadline)
                        .expect("registration client request preparation failed");
                runtime
                    .spawn(ServiceTask::RegistrationClient(task), 0)
                    .map_err(|failure| {
                        drop(failure.task);
                        failure.error
                    })
                    .expect("registration client task admission failed");
            }
            if registration_client_expected {
                drop(sender);
            } else {
                let mut packet = Packet::new(protocol::ROOT_GRANT_KIND, &[])
                    .expect("provider root grant packet creation failed");
                packet
                    .push(
                        sender.into_capability(),
                        Rights::WRITE
                            | Rights::WAIT
                            | Rights::TRANSIT
                            | Rights::DUPLICATE
                            | Rights::GRANT,
                    )
                    .map_err(|failure| failure.error)
                    .expect("provider root grant packet preparation failed");
                packet
                    .try_send(&bootstrap, Deadline::INFINITE)
                    .map_err(|failure| failure.error)
                    .expect("provider root grant publication failed");
            }
            root_published = true;
        }
        if let Some(sender) = world.registry_sender.take() {
            let mut packet = Packet::new(service_protocol::DIRECTORY_GRANT_KIND, &[])
                .expect("Registry root grant packet creation failed");
            packet
                .push(
                    sender.into_capability(),
                    Rights::WRITE
                        | Rights::WAIT
                        | Rights::TRANSIT
                        | Rights::DUPLICATE
                        | Rights::GRANT,
                )
                .map_err(|failure| failure.error)
                .expect("Registry root grant packet preparation failed");
            packet
                .try_send(&bootstrap, Deadline::INFINITE)
                .map_err(|failure| failure.error)
                .expect("Registry root grant publication failed");
            registry_published = true;
        }
        if root_published
            && registry_published
            && let Some(sender) = world.registration_root_sender.take()
        {
            let mut packet = Packet::new(service_protocol::AUTHORITY_GRANT_KIND, &[])
                .expect("root registration authority packet creation failed");
            packet
                .push(
                    sender.into_capability(),
                    Rights::WRITE
                        | Rights::WAIT
                        | Rights::TRANSIT
                        | Rights::DUPLICATE
                        | Rights::GRANT,
                )
                .map_err(|failure| failure.error)
                .expect("root registration authority packet preparation failed");
            packet
                .try_send(&bootstrap, Deadline::INFINITE)
                .map_err(|failure| failure.error)
                .expect("root registration authority publication failed");
            registration_root_published = true;
        }
        if root_published
            && (!registry_expected || registry_published)
            && registration_root_published
            && world.registration_ready
            && !ready_published
        {
            bootstrap
                .send(protocol::PROVIDER_READY_KIND, &[])
                .expect("provider Ready publication failed");
            ready_published = true;
        }
        match runtime.drive_state() {
            DriveState::Drained => break,
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => {
                wait_until(&[runtime.wait_item(0)], deadline)
                    .expect("provider Runtime wait failed");
                runtime.notified();
            }
        }
    }
    runtime
        .close()
        .map_err(|(_, error)| error)
        .expect("provider Runtime close failed");
    assert!(
        dispatch_bindings.is_empty() && world.dispatch_intent.is_none(),
        "provider dispatch owners remained after shutdown"
    );
    assert!(
        world.provider.watches.is_empty(),
        "provider Watch records remained after Runtime shutdown"
    );
    assert!(world.streams.is_empty(), "provider Stream records remained after Runtime shutdown");
    rinlib::debug!("fs provider shutdown: Runtime closed");
    world
        .provider
        .grants
        .take()
        .expect("provider grant table missing at close")
        .close()
        .map_err(|_| SystemCallError::ObjectBusy)
        .expect("provider grant table close failed");
    rinlib::debug!("fs provider shutdown: GrantTable closed");
    assert!(
        world.provider.backend.is_none(),
        "provider backend remained after shutdown"
    );
    // SAFETY: StartupBlock transferred the unique route mailbox owner to this process,
    // and Runtime has removed its final WaitSet source before reaching this point.
    unsafe { rinlib::ipc::object::close(world.route_mailbox) }
        .expect("provider route mailbox close failed");
    rinlib::debug!("fs provider shutdown: route mailbox closed");
    for kind in [
        FalResource::Node,
        FalResource::Bytes,
        FalResource::Grant,
        FalResource::Watch,
        FalResource::WaitSource,
    ] {
        assert_eq!(
            fal_account.usage(kind).0,
            0,
            "provider FAL account did not refund {kind:?}"
        );
    }
    for kind in [ExecutionResource::Task, ExecutionResource::InputBytes] {
        assert_eq!(
            execution_account.usage(kind).0,
            0,
            "provider execution account did not refund {kind:?}"
        );
    }
    for kind in [
        ServiceResource::Authority,
        ServiceResource::Registration,
        ServiceResource::Bytes,
        ServiceResource::WaitSource,
    ] {
        assert_eq!(
            service_account.usage(kind).0,
            0,
            "provider service account did not refund {kind:?}"
        );
    }
    rinlib::debug!("fs provider shutdown: account refunded");
    let report = protocol::ProviderReport {
        committed: world.provider.committed,
        abandoned: world.provider.abandoned,
        downstream_abandoned: world.downstream_abandoned,
    };
    let mut payload = [0; protocol::PROVIDER_REPORT_LEN];
    let used = report
        .encode(&mut payload)
        .expect("provider report encoding failed");
    bootstrap
        .send(protocol::PROVIDER_STOPPED_KIND, &payload[..used])
        .expect("provider shutdown report failed");
    rinlib::debug!(
        "fs provider shutdown report: committed={}, abandoned={}, downstream_abandoned={}",
        report.committed,
        report.abandoned,
        report.downstream_abandoned
    );
}
