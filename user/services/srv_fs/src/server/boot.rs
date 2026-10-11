//! 启动装配：两个独立 mailbox/backing 在同一 Runtime 推进，母本直接移入启动 Packet。
use super::{FileTask, Ingress, World, lifecycle, services};
use alloc::{boxed::Box, rc::Rc};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    time::Deadline,
};
use libbudget::{Budget, Taxonomy};
use libexecution::{
    ExecutionResource,
    assembly::Assembly,
    runtime::{DriveState, Runtime},
};
use libfs::{backend::MemoryBackend, grant::GrantTable, resource::FsResource};
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender},
    notification,
    object::query,
    packet::Packet,
    wait_set::WaitSet,
};
use libservice::{
    authority::{AccessTable, AuthorityTable, Scope},
    maintenance::Maintenance,
    protocol::{
        self as service, AccessRights, Addressing, Descriptors, ProtocolDescriptor, PublishRights,
        RegisterMode, Request as Command, Response,
    },
    provider::State,
    registry::Registry,
    resource::ServiceResource,
    root::{Policy, Root},
};
use srv_fs::deployment::{
    BINDINGS, ENTRIES, ENTRY_WATCHES, GRANTS, NODES, REGISTRATIONS, REQUESTS, ROLES, RPC_CALLS,
    SOURCES, TASKS, WATCHES,
};
// Bootstrap peer 由 StartupBlock 独占移交，四种根报文加 Ready 最多五条。
// Stopped 发生在 init 已接受 Ready 之后。
const _: () = assert!(5 <= erhino_shared::message::MAILBOX_CAPACITY);
fn cap(handle: Handle) -> Capability {
    // SAFETY: 参数来自本进程 StartupBlock 的独占 grant，调用者已转出其原 owner。
    unsafe { Capability::from_raw(handle) }
}
fn publish<const N: usize>(
    bootstrap: &MailboxSender,
    kind: u64,
    senders: [Option<MailboxSender>; N],
) {
    let mut packet = Packet::new(kind, &[]).expect("bootstrap packet allocation failed");
    for sender in senders {
        let sender = sender.expect("bootstrap entry sender missing");
        let rights = sender
            .description()
            .expect("bootstrap sender description failed")
            .rights;
        packet
            .push(sender.into_capability(), rights)
            .map_err(|f| f.error)
            .expect("bootstrap sender packet failed");
    }
    packet
        .try_send(bootstrap, Deadline::INFINITE)
        .map_err(|f| f.error)
        .expect("bootstrap sender publication failed");
}
fn begin_registration(
    dispatcher: &mut librpc::dispatcher::Dispatcher,
    authority: MailboxSender,
    endpoint: MailboxSender,
    funding: &libbudget::AccountView<ExecutionResource>,
) -> Result<(u64, Deadline), SystemCallError> {
    let deadline = libkernel::time::timeout_millis(5000)?;
    let descriptor = [ProtocolDescriptor {
        id: libfs::protocol::ID,
        min_version: libfs::protocol::VERSION,
        max_version: libfs::protocol::VERSION,
        addressing: Addressing::Relative,
    }];
    let mut descriptor_bytes = [0; 20];
    let used = Descriptors::encode(&descriptor, &mut descriptor_bytes)
        .map_err(|_| SystemCallError::IllegalArgument)?;
    let transport = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
    let request = Command::Register {
        name: "fs.secondary",
        mode: RegisterMode::CreateOnly,
        descriptors: Descriptors::decode(&descriptor_bytes[..used])
            .map_err(|_| SystemCallError::IllegalArgument)?,
        export_transport: transport,
    };
    let mut bytes = [0; service::HEADER_LEN + service::BODY_MAX];
    let used = service::encode_request(&request, deadline, &mut bytes)
        .map_err(|_| SystemCallError::IllegalArgument)?;
    let mut rpc = librpc::Request::new(service::ID, &bytes[..used])?;
    rpc.push(endpoint.into_capability(), transport)
        .map_err(|f| f.error)?;
    let txid = dispatcher
        .begin(
            librpc::dispatcher::ServiceOwner::Owned(authority),
            deadline,
            rpc,
            funding,
        )
        .map_err(|f| match f.error.cause {
            librpc::exchange::CallCause::System(error) => error,
            _ => SystemCallError::ObjectClosed,
        })?;
    Ok((txid, deadline))
}
fn begin_withdraw(
    dispatcher: &mut librpc::dispatcher::Dispatcher,
    control: MailboxSender,
    funding: &libbudget::AccountView<ExecutionResource>,
) -> Result<(u64, Deadline), SystemCallError> {
    let deadline = libkernel::time::timeout_millis(5000)?;
    let request = Command::Withdraw;
    let mut bytes = [0; service::HEADER_LEN];
    let used = service::encode_request(&request, deadline, &mut bytes)
        .map_err(|_| SystemCallError::IllegalArgument)?;
    let rpc = librpc::Request::new(service::ID, &bytes[..used])?;
    let txid = dispatcher
        .begin(
            librpc::dispatcher::ServiceOwner::Owned(control),
            deadline,
            rpc,
            funding,
        )
        .map_err(|f| match f.error.cause {
            librpc::exchange::CallCause::System(error) => error,
            _ => SystemCallError::ObjectClosed,
        })?;
    Ok((txid, deadline))
}
pub(crate) enum Instance {
    Primary,
    Secondary { parent_pid: u64 },
}
pub(crate) fn run(
    mailbox: Mailbox,
    bootstrap: MailboxSender,
    route: Handle,
    release: Handle,
    registration: Handle,
    _pool: Capability,
    instance: Instance,
) {
    let primary = matches!(instance, Instance::Primary);
    let registration = cap(registration);
    let mut publisher = None;
    let service_mailbox = if primary {
        Some(
            Mailbox::from_capability(registration)
                .map_err(|f| f.error)
                .expect("registry mailbox role invalid")
                .0,
        )
    } else {
        let Instance::Secondary { parent_pid } = instance else {
            unreachable!("provider instance role changed")
        };
        let inbox = Mailbox::from_capability(registration)
            .map_err(|f| f.error)
            .expect("publication inbox role invalid")
            .0;
        let deadline =
            libkernel::time::timeout_millis(10_000).expect("publication handoff deadline failed");
        let mut message = libkernel::ipc::message::wait_message_until(inbox.as_handle(), deadline)
            .expect("publication authority receive failed");
        assert!(
            message.header.sender_pid == parent_pid
                && message.header.kind == service::AUTHORITY_GRANT_KIND
                && message.payload.is_empty()
                && message.handles.remaining() == 1,
            "publication authority message invalid"
        );
        let capability = message
            .handles
            .take(0)
            .expect("publication authority missing");
        let (sender, description) = MailboxSender::from_capability(capability)
            .map_err(|f| f.error)
            .expect("publication authority role invalid");
        assert!(
            description
                .rights
                .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
                && !description.rights.contains(Rights::GRANT),
            "publication authority transport invalid"
        );
        publisher = Some(sender);
        inbox
            .close()
            .map_err(|(_, e)| e)
            .expect("publication inbox close failed");
        None
    };
    // 管理 receiver 的运行与最终关闭责任进入 World，入口由独立管理 sender 出生。
    let route = Mailbox::from_capability(cap(route))
        .map_err(|f| f.error)
        .expect("management mailbox role invalid")
        .0;
    let input = Runtime::<FileTask, WaitSet>::fixed_input_budget(SOURCES)
        .and_then(|fixed| {
            fixed
                .checked_add(Runtime::<FileTask, WaitSet>::source_input_budget(SOURCES)?)
                .ok_or(SystemCallError::ReachLimit)
        })
        .and_then(|runtime| {
            runtime
                .checked_add(Assembly::<WaitSet>::input_budget()?)
                .ok_or(SystemCallError::ReachLimit)
        })
        .expect("runtime input budget overflow");
    let mut limits = [0; ExecutionResource::COUNT + FsResource::COUNT + ServiceResource::COUNT];
    let fs_base = ExecutionResource::COUNT;
    let service_base = fs_base + FsResource::COUNT;
    limits[ExecutionResource::Task.slot()] = TASKS;
    limits[ExecutionResource::InputBytes.slot()] = input;
    limits[fs_base..service_base].fill(REQUESTS + GRANTS + WATCHES);
    limits[fs_base + FsResource::Node.slot()] = NODES;
    limits[fs_base + FsResource::Binding.slot()] = BINDINGS;
    limits[fs_base + FsResource::Grant.slot()] = GRANTS;
    limits[fs_base + FsResource::Watch.slot()] = WATCHES;
    limits[fs_base + FsResource::Offer.slot()] = srv_fs::deployment::SESSIONS;
    limits[fs_base + FsResource::Stream.slot()] = srv_fs::deployment::SESSIONS;
    limits[fs_base + FsResource::Bytes.slot()] = 2 * 1024 * 1024;
    limits[fs_base + FsResource::WaitSource.slot()] =
        GRANTS + REQUESTS + WATCHES + 2 * srv_fs::deployment::SESSIONS + ENTRIES + 4;
    limits[service_base..].fill(REQUESTS + 2 * ROLES + 2 * REGISTRATIONS + WATCHES + 2);
    limits[service_base + ServiceResource::Authority.slot()] = ROLES;
    limits[service_base + ServiceResource::Access.slot()] = ROLES;
    limits[service_base + ServiceResource::Registration.slot()] = REGISTRATIONS;
    limits[service_base + ServiceResource::Watch.slot()] = WATCHES;
    limits[service_base + ServiceResource::Bytes.slot()] = 2 * 1024 * 1024;
    let budget = Budget::new(&limits, 1 + ENTRIES).expect("provider budget creation failed");
    let account = budget
        .account(&limits)
        .expect("provider account creation failed");
    let execution_slots: [_; ExecutionResource::COUNT] =
        core::array::from_fn(|i| budget.slot(i).expect("execution budget slot missing"));
    let fs_slots: [_; FsResource::COUNT] =
        core::array::from_fn(|i| budget.slot(fs_base + i).expect("file budget slot missing"));
    let service_slots: [_; ServiceResource::COUNT] = core::array::from_fn(|i| {
        budget
            .slot(service_base + i)
            .expect("service budget slot missing")
    });
    let execution = account
        .view::<ExecutionResource>(&execution_slots)
        .expect("execution view invalid");
    let fs = account
        .view::<FsResource>(&fs_slots)
        .expect("file view invalid");
    let service_account = account
        .view::<ServiceResource>(&service_slots)
        .expect("service view invalid");
    let tenants: [_; ENTRIES] = core::array::from_fn(|entry| {
        let mut local = limits;
        local[fs_base + FsResource::Watch.slot()] = ENTRY_WATCHES[entry];
        local[fs_base + FsResource::Stream.slot()] = srv_fs::deployment::ENTRY_SESSIONS[entry];
        local[fs_base + FsResource::Offer.slot()] = srv_fs::deployment::ENTRY_SESSIONS[entry];
        local[service_base + ServiceResource::Watch.slot()] = ENTRY_WATCHES[entry];
        let account = budget
            .account(&local)
            .expect("entry account creation failed");
        (
            account
                .view::<ExecutionResource>(&execution_slots)
                .expect("entry execution view invalid"),
            account
                .view::<FsResource>(&fs_slots)
                .expect("entry file view invalid"),
            account
                .view::<ServiceResource>(&service_slots)
                .expect("entry service view invalid"),
        )
    });
    let pair = notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
        .expect("retirement notification creation failed");
    let retire = pair.owner;
    let wake = libexecution::wake::NotificationWake::new(pair.peer.into_capability(), 1)
        .map_err(|(_, e)| e)
        .expect("retirement signaler role invalid");
    let memory = MemoryBackend::with_bindings(
        &fs,
        NODES,
        BINDINGS,
        WATCHES,
        Rc::try_new(wake).expect("retirement wake allocation failed"),
    )
    .expect("file backing creation failed");
    let grants = GrantTable::new(&mailbox, GRANTS).expect("file grant table creation failed");
    let service_state = service_mailbox.map(|mailbox| {
        let incarnation = query(mailbox.as_handle())
            .expect("registry mailbox query failed")
            .object_id;
        State {
            mailbox,
            registry: Registry::new(incarnation, REGISTRATIONS, WATCHES)
                .expect("registry creation failed"),
            accesses: AccessTable::new(ROLES).expect("directory access table creation failed"),
            authorities: AuthorityTable::new(ROLES).expect("authority table creation failed"),
            account: service_account.clone(),
            execution: execution.clone(),
            maintenance: 0,
            dirty: true,
        }
    });
    let mut world = World {
        pool: _pool,
        streams: libfs::stream::StreamTable::new(srv_fs::deployment::SESSIONS)
            .expect("stream table creation failed"),
        retirement_task: 0,
        transport_task: 0,
        management: libfs::management::ManagementTable::new(&route, ENTRIES)
            .expect("management table creation failed"),
        route,
        rpc: Some(
            Box::try_new(
                librpc::dispatcher::Dispatcher::new(RPC_CALLS, &execution)
                    .expect("provider Dispatcher creation failed"),
            )
            .expect("provider Dispatcher storage failed"),
        ),
        mailbox,
        memory,
        grants,
        account: fs.clone(),
        stop: false,
        service: service_state,
    };
    let set = WaitSet::create(SOURCES).expect("provider WaitSet creation failed");
    let mut assembly = Assembly::new(set, &execution)
        .map_err(|(_, e)| e)
        .expect("provider assembly creation failed");
    let mut runtime = Runtime::<FileTask, WaitSet>::new(
        assembly.bind().expect("assembly binding failed"),
        TASKS,
        SOURCES,
        &execution,
    )
    .expect("provider runtime creation failed");
    let mut file_roots = [None; ENTRIES];
    let mut management_roots = [None; ENTRIES];
    for entry in 0..if primary { ENTRIES } else { 1 } {
        let (execution, fs, _) = &tenants[entry];
        let prepared = world
            .prepare_root(primary, fs, execution)
            .map_err(|f| f.error)
            .expect("file root preparation failed");
        file_roots[entry] = Some(
            runtime
                .spawn(libexecution::TaskSubmission::new(
                    FileTask::FileRoot(
                        lifecycle::Root::new(prepared, fs)
                            .expect("file root owner preparation failed"),
                    ),
                    1,
                    execution.clone(),
                ))
                .map_err(|f| f.error)
                .expect("file root admission failed"),
        );
        let minted = world
            .route
            .mint(
                0,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT,
            )
            .expect("management sender mint failed");
        let prepared = world
            .management
            .prepare_minted(
                world
                    .memory
                    .root()
                    .expect("management root missing")
                    .clone(),
                libfs::FsRights::ALL,
                srv_fs::deployment::root_output_transport(),
                fs,
                execution,
                minted,
            )
            .map_err(|failure| failure.error)
            .expect("management role preparation failed");
        management_roots[entry] = Some(
            runtime
                .spawn(libexecution::TaskSubmission::new(
                    FileTask::FileRoot(
                        lifecycle::Root::management(prepared, fs)
                            .expect("management root owner preparation failed"),
                    ),
                    1,
                    execution.clone(),
                ))
                .map_err(|failure| failure.error)
                .expect("management root admission failed"),
        );
    }
    runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Ingress(Ingress::new(&fs).expect("file ingress preparation failed")),
            Ingress::MAX_SOURCES,
            execution.clone(),
        ))
        .map_err(|f| f.error)
        .expect("file ingress admission failed");
    runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Ingress(
                Ingress::management(&fs).expect("management ingress preparation failed"),
            ),
            Ingress::MAX_SOURCES,
            execution.clone(),
        ))
        .map_err(|failure| failure.error)
        .expect("management ingress admission failed");
    world.retirement_task = runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Retire(
                lifecycle::Retire::new(retire, &fs).expect("retirement owner preparation failed"),
            ),
            1,
            execution.clone(),
        ))
        .map_err(|f| f.error)
        .expect("retirement admission failed");
    runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Release(
                lifecycle::Release::new(
                    notification::NotificationOwner::from_capability(cap(release))
                        .map_err(|f| f.error)
                        .expect("provider release role invalid")
                        .0,
                    &fs,
                )
                .expect("release owner preparation failed"),
            ),
            1,
            execution.clone(),
        ))
        .map_err(|f| f.error)
        .expect("release admission failed");
    let dispatcher = runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Dispatcher(super::Transport::new()),
            librpc::dispatcher::Dispatcher::source_capacity(RPC_CALLS)
                .expect("publication Dispatcher source capacity invalid"),
            execution.clone(),
        ))
        .map_err(|f| f.error)
        .expect("publication Dispatcher admission failed");
    world.transport_task = dispatcher;
    let mut directory_roots = [None; ENTRIES];
    let mut authority_roots = [None; ENTRIES];
    if let Some(state) = world.service.as_mut() {
        let transport =
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT;
        for entry in 0..ENTRIES {
            let (execution, _, service_account) = &tenants[entry];
            let directory = Root::prepare(
                state,
                Policy::Directory {
                    scope: Scope::Root,
                    rights: AccessRights::ALL,
                    output: Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                },
                transport,
                service_account,
                execution,
            )
            .expect("directory root preparation failed");
            let authority = Root::prepare(
                state,
                Policy::Authority {
                    scope: Scope::Root,
                    rights: PublishRights::ALL,
                    output: transport,
                },
                transport,
                service_account,
                execution,
            )
            .expect("publication root preparation failed");
            directory_roots[entry] = Some(
                runtime
                    .spawn(libexecution::TaskSubmission::new(
                        FileTask::ServiceRoot(directory),
                        1,
                        execution.clone(),
                    ))
                    .map_err(|f| f.error)
                    .expect("directory root admission failed"),
            );
            authority_roots[entry] = Some(
                runtime
                    .spawn(libexecution::TaskSubmission::new(
                        FileTask::ServiceRoot(authority),
                        1,
                        execution.clone(),
                    ))
                    .map_err(|f| f.error)
                    .expect("publication root admission failed"),
            );
        }
        let maintenance = Maintenance::new(state).expect("registry maintenance preparation failed");
        state.maintenance = runtime
            .spawn(libexecution::TaskSubmission::new(
                FileTask::Maintenance(maintenance),
                0,
                execution.clone(),
            ))
            .map_err(|f| f.error)
            .expect("registry maintenance admission failed");
        let ingress = services::Ingress::new(state).expect("registry ingress preparation failed");
        runtime
            .spawn(libexecution::TaskSubmission::new(
                FileTask::ServiceIngress(ingress),
                2,
                execution.clone(),
            ))
            .map_err(|f| f.error)
            .expect("registry ingress admission failed");
    }
    let mut published_file = false;
    let mut published_management = false;
    let mut management_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut published_directory = !primary;
    let mut published_authority = !primary;
    let mut file_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut directory_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut authority_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut publication = None;
    let mut registration_control = None;
    let mut ready = false;
    let mut sealing = false;
    let mut stop_prepared = false;
    let mut withdrawal = None;
    let mut withdrawal_finished = primary;
    loop {
        for &record in assembly
            .receive(64)
            .expect("provider source receive failed")
        {
            runtime.feed(record);
        }
        if world.stop && !stop_prepared {
            world.memory.seal();
            world.streams.seal();
            world.grants.seal();
            world.management.seal();
            if let Some(state) = world.service.as_mut() {
                state.seal();
            }
            for file_root in file_roots.iter().flatten().copied() {
                if let Some(FileTask::FileRoot(task)) = runtime.get_task_mut(file_root) {
                    task.stop();
                }
                let _ = runtime.wake(file_root);
            }
            for management_root in management_roots.iter().flatten().copied() {
                if let Some(FileTask::FileRoot(task)) = runtime.get_task_mut(management_root) {
                    task.stop();
                }
                let _ = runtime.wake(management_root);
            }
            stop_prepared = true;
        }
        if world.stop && !withdrawal_finished && publication.is_none() && withdrawal.is_none() {
            if let Some(control) = registration_control.take() {
                let dispatch = world.rpc_mut();
                match begin_withdraw(dispatch, control, &tenants[0].0) {
                    Ok(call) => {
                        withdrawal = Some(call);
                        runtime.wake(dispatcher).expect("withdrawal wake failed");
                    }
                    Err(_) => {
                        withdrawal_finished = true;
                    }
                }
            } else {
                withdrawal_finished = true;
            }
        }
        if world.stop && !sealing && withdrawal_finished {
            runtime.seal();
            sealing = true;
        }
        if sealing {
            runtime.shutdown_turn(&mut world, 1)
        } else {
            runtime.turn(&mut world, 1)
        }
        .unwrap_or_else(|failure| panic!("provider Runtime failed: {:?}", failure.error));
        if !world.stop {
            for entry in 0..ENTRIES {
                if management_senders[entry].is_none()
                    && !published_management
                    && let Some(id) = management_roots[entry]
                    && let Some(FileTask::FileRoot(root)) = runtime.get_task_mut(id)
                {
                    management_senders[entry] = root.take_sender();
                }
                if file_senders[entry].is_none()
                    && !published_file
                    && let Some(id) = file_roots[entry]
                    && let Some(FileTask::FileRoot(root)) = runtime.get_task_mut(id)
                {
                    file_senders[entry] = root.take_sender();
                }
                if directory_senders[entry].is_none()
                    && !published_directory
                    && let Some(id) = directory_roots[entry]
                    && let Some(FileTask::ServiceRoot(root)) = runtime.get_task_mut(id)
                {
                    directory_senders[entry] = root.take_sender();
                }
                if authority_senders[entry].is_none()
                    && !published_authority
                    && let Some(id) = authority_roots[entry]
                    && let Some(FileTask::ServiceRoot(root)) = runtime.get_task_mut(id)
                {
                    authority_senders[entry] = root.take_sender();
                }
            }
        }
        if !world.stop
            && !published_file
            && (if primary {
                file_senders.iter().all(Option::is_some)
            } else {
                file_senders[0].is_some()
            })
        {
            if primary {
                publish(
                    &bootstrap,
                    libfs::protocol::ROOT_GRANT_KIND,
                    core::mem::replace(&mut file_senders, core::array::from_fn(|_| None)),
                );
            } else {
                let dispatch = world.rpc_mut();
                publication = Some(
                    begin_registration(
                        dispatch,
                        publisher.take().expect("publication authority missing"),
                        file_senders[0]
                            .take()
                            .expect("publication file root missing"),
                        &tenants[0].0,
                    )
                    .expect("publication request preparation failed"),
                );
                runtime.wake(dispatcher).expect("publication wake failed");
            }
            published_file = true;
        }
        if !world.stop && !published_directory && directory_senders.iter().all(Option::is_some) {
            publish(
                &bootstrap,
                service::DIRECTORY_GRANT_KIND,
                core::mem::replace(&mut directory_senders, core::array::from_fn(|_| None)),
            );
            published_directory = true;
        }
        if !world.stop
            && !published_management
            && management_senders[..if primary { ENTRIES } else { 1 }]
                .iter()
                .all(Option::is_some)
        {
            // srv_fs 创建并观察寿命；原 A/B 账户付款，发送后由 init 保管/移交/关闭。
            if primary {
                publish(
                    &bootstrap,
                    libfs::route::MANAGEMENT_GRANT_KIND,
                    core::mem::replace(&mut management_senders, core::array::from_fn(|_| None)),
                );
            } else {
                publish(
                    &bootstrap,
                    libfs::route::MANAGEMENT_GRANT_KIND,
                    [management_senders[0].take()],
                );
            }
            published_management = true;
        }
        if !world.stop && !published_authority && authority_senders.iter().all(Option::is_some) {
            publish(
                &bootstrap,
                service::AUTHORITY_GRANT_KIND,
                core::mem::replace(&mut authority_senders, core::array::from_fn(|_| None)),
            );
            published_authority = true;
        }
        if let Some((txid, deadline)) = publication
            && let Some(completion) = world.rpc_mut().take(txid)
        {
            let mut reply = completion.result.expect("registration RPC failed");
            let mut descriptors = [0; 20];
            let used = Descriptors::encode(
                &[ProtocolDescriptor {
                    id: libfs::protocol::ID,
                    min_version: 1,
                    max_version: 1,
                    addressing: Addressing::Relative,
                }],
                &mut descriptors,
            )
            .expect("registration descriptor invariant");
            let request = Command::Register {
                name: "fs.secondary",
                mode: RegisterMode::CreateOnly,
                descriptors: Descriptors::decode(&descriptors[..used])
                    .expect("registration descriptor invariant"),
                export_transport: Rights::WRITE
                    | Rights::WAIT
                    | Rights::TRANSIT
                    | Rights::DUPLICATE,
            };
            let response = service::decode_response(
                &reply.payload,
                &request,
                deadline,
                if reply.handles.remaining() == 1 { 1 } else { 0 },
            )
            .expect("registration reply invalid");
            assert!(
                matches!(response, Response::Registration(_)),
                "registration was rejected"
            );
            registration_control = Some(
                MailboxSender::from_capability(
                    reply.handles.take(0).expect("registration control missing"),
                )
                .map_err(|f| f.error)
                .expect("registration control role invalid")
                .0,
            );
            publication = None;
        }
        if let Some((txid, deadline)) = withdrawal
            && let Some(completion) = world.rpc_mut().take(txid)
        {
            // 失联仍关闭原控制别名；目录 owner 会自动撤出，不按名字补删或恢复旧注册。
            if let Ok(reply) = completion.result {
                let response =
                    service::decode_response(&reply.payload, &Command::Withdraw, deadline, 0)
                        .expect("withdrawal reply invalid");
                assert!(
                    matches!(response,Response::Registration(info)if info.state==service::State::Withdrawn),
                    "withdrawal state invalid"
                );
            }
            withdrawal = None;
            withdrawal_finished = true;
        }
        if !world.stop
            && !ready
            && published_file
            && published_directory
            && published_authority
            && published_management
            && (primary || registration_control.is_some())
        {
            bootstrap
                .send(libfs::protocol::PROVIDER_READY_KIND, &[])
                .expect("provider ready publication failed");
            ready = true;
        }
        if world.stop && withdrawal_finished && !sealing {
            continue;
        }
        if sealing
            && world.requests_empty()
            && world
                .rpc
                .as_ref()
                .is_some_and(|rpc| rpc.pending_count() == 0)
        {
            // 最后业务 task 释放付款后，停驻的运输 task 仍须得到一次封口推进。
            runtime
                .wake(dispatcher)
                .expect("transport drain wake failed");
        }
        match runtime.drive_state() {
            DriveState::Drained => break,
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => assembly.wait(deadline).expect("provider wait failed"),
        }
    }
    runtime
        .close()
        .map_err(|(_, e)| e)
        .expect("provider Runtime close failed");
    assembly
        .close()
        .map_err(|(_, e)| e)
        .expect("provider assembly close failed");
    assert!(
        world.memory.is_empty() && world.grants.is_empty() && world.management.is_empty(),
        "file retirement remained after Runtime shutdown"
    );
    if let Some(state) = world.service.as_ref() {
        assert!(
            state.registry.is_empty() && state.accesses.is_empty() && state.authorities.is_empty(),
            "registry retirement remained after Runtime shutdown"
        );
    }
    drop(registration_control);
    drop(world);
    assert!(
        budget.is_empty(),
        "provider budget remained after owner retirement"
    );
    libkernel::debug!("FS1 provider budget empty after owner retirement");
    bootstrap
        .send(libfs::protocol::PROVIDER_STOPPED_KIND, &[])
        .expect("provider stopped publication failed");
}
