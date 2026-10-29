//! 启动装配：两个独立 mailbox/backing 在同一 Runtime 推进，母本直接移入启动 Packet。
use super::{FileTask, Ingress, World, lifecycle, services};
use alloc::{boxed::Box, rc::Rc};
use erhino_shared::{call::SystemCallError, object::Rights, time::Deadline};
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
    wait_set::WaitSet,
};
use libservice::instance::child::Reporter;
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
use srv_fs::{
    deployment::{
        BINDINGS, Capacity, GRANTS, MAX_ENTRIES as ENTRIES, NODES, REGISTRATIONS, REQUESTS, ROLES,
        RPC_CALLS, WATCHES,
    },
    startup::{AUTHORITY_EXPORT, Config, DIRECTORY_EXPORT, FILE_ROOT_EXPORT, MANAGEMENT_EXPORT},
};
/// 每个入口一条命名导出 `<prefix>.<序号>`，权利取 sender 的实际权利。
fn export_all(reporter: &mut Reporter, prefix: &str, senders: &mut [Option<MailboxSender>]) {
    let mut name = alloc::string::String::new();
    for (entry, sender) in senders.iter_mut().enumerate() {
        let sender = sender.take().expect("export entry sender missing");
        let rights = sender
            .description()
            .expect("export sender description failed")
            .rights;
        name.clear();
        core::fmt::write(&mut name, format_args!("{prefix}.{entry}"))
            .expect("String write cannot fail");
        reporter
            .export(&name, sender.into_capability(), rights, Deadline::INFINITE)
            .map_err(|(_, error)| error)
            .expect("export publication failed");
    }
}
fn begin_registration(
    dispatcher: &mut librpc::dispatcher::Dispatcher,
    authority: MailboxSender,
    endpoint: MailboxSender,
    funding: &libbudget::AccountView<ExecutionResource>,
    name: &str,
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
        name,
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
pub(crate) struct Startup {
    pub reporter: Reporter,
    pub stop: Capability,
    pub pool: Capability,
    pub config: Config<'static>,
    pub authority: Option<MailboxSender>,
}
pub(crate) fn run(mailbox: Mailbox, startup: Startup) {
    let Startup {
        mut reporter,
        stop,
        pool: _pool,
        config,
        authority,
    } = startup;
    let with_registry = config.registry;
    let entries = config.entries().len();
    let capacity = Capacity::new(&config).expect("provider capacity invalid");
    let sources = capacity.sources;
    let tasks = capacity.tasks;
    let mut publisher = authority;
    let publish_name = config.publish.unwrap_or_default();
    let owner_rights = Rights::READ | Rights::WAIT | Rights::MANAGE;
    let service_mailbox = with_registry
        .then(|| Mailbox::create(owner_rights).expect("registry mailbox creation failed"));
    // 管理 receiver 的运行与最终关闭责任进入 World，入口由独立管理 sender 出生。
    let route = Mailbox::create(owner_rights).expect("management mailbox creation failed");
    let input = Runtime::<FileTask, WaitSet>::fixed_input_budget(sources)
        .and_then(|fixed| {
            fixed
                .checked_add(Runtime::<FileTask, WaitSet>::source_input_budget(sources)?)
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
    limits[ExecutionResource::Task.slot()] = tasks;
    limits[ExecutionResource::InputBytes.slot()] = input;
    limits[fs_base..service_base].fill(REQUESTS + GRANTS + WATCHES);
    limits[fs_base + FsResource::Node.slot()] = NODES;
    limits[fs_base + FsResource::Binding.slot()] = BINDINGS;
    limits[fs_base + FsResource::Grant.slot()] = GRANTS;
    limits[fs_base + FsResource::Watch.slot()] = WATCHES;
    limits[fs_base + FsResource::Offer.slot()] = srv_fs::deployment::SESSIONS;
    limits[fs_base + FsResource::Stream.slot()] = srv_fs::deployment::SESSIONS;
    limits[fs_base + FsResource::Bytes.slot()] = srv_fs::deployment::DOMAIN_BYTES;
    limits[fs_base + FsResource::WaitSource.slot()] =
        GRANTS + REQUESTS + WATCHES + 2 * srv_fs::deployment::SESSIONS + entries + 4;
    limits[service_base..].fill(REQUESTS + 2 * ROLES + 2 * REGISTRATIONS + WATCHES + 2);
    limits[service_base + ServiceResource::Authority.slot()] = ROLES;
    limits[service_base + ServiceResource::Access.slot()] = ROLES;
    limits[service_base + ServiceResource::Registration.slot()] = REGISTRATIONS;
    limits[service_base + ServiceResource::Watch.slot()] = WATCHES;
    limits[service_base + ServiceResource::Bytes.slot()] = srv_fs::deployment::DOMAIN_BYTES;
    let budget = Budget::new(&limits, 1 + entries).expect("provider budget creation failed");
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
    let tenants: alloc::vec::Vec<_> = (0..entries)
        .map(|entry| {
            let mut local = limits;
            local[fs_base + FsResource::Watch.slot()] = config.entries()[entry].watches;
            local[fs_base + FsResource::Stream.slot()] = config.entries()[entry].sessions;
            local[fs_base + FsResource::Offer.slot()] = config.entries()[entry].sessions;
            local[service_base + ServiceResource::Watch.slot()] = config.entries()[entry].watches;
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
        })
        .collect();
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
        management: libfs::management::ManagementTable::new(&route, entries)
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
    let set = WaitSet::create(sources).expect("provider WaitSet creation failed");
    let mut assembly = Assembly::new(set, &execution)
        .map_err(|(_, e)| e)
        .expect("provider assembly creation failed");
    let mut runtime = Runtime::<FileTask, WaitSet>::new(
        assembly.bind().expect("assembly binding failed"),
        tasks,
        sources,
        &execution,
    )
    .expect("provider runtime creation failed");
    let mut file_roots = [None; ENTRIES];
    let mut management_roots = [None; ENTRIES];
    for entry in 0..entries {
        let (execution, fs, _) = &tenants[entry];
        let prepared = world
            .prepare_root(with_registry, fs, execution)
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
    let file_ingress = runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Ingress(Ingress::new(&fs).expect("file ingress preparation failed")),
            Ingress::MAX_SOURCES,
            execution.clone(),
        ))
        .map_err(|f| f.error)
        .expect("file ingress admission failed");
    let management_ingress = runtime
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
    let release_task = runtime
        .spawn(libexecution::TaskSubmission::new(
            FileTask::Release(
                lifecycle::Release::new(
                    notification::NotificationOwner::from_capability(stop)
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
    let mut service_ingress = None;
    if let Some(state) = world.service.as_mut() {
        let transport =
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT;
        for entry in 0..entries {
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
        service_ingress = Some(
            runtime
                .spawn(libexecution::TaskSubmission::new(
                    FileTask::ServiceIngress(ingress),
                    2,
                    execution.clone(),
                ))
                .map_err(|f| f.error)
                .expect("registry ingress admission failed"),
        );
    }
    let mut published_file = false;
    let mut published_management = false;
    let mut management_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut published_directory = !with_registry;
    let mut published_authority = !with_registry;
    let mut file_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut directory_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut authority_senders: [Option<MailboxSender>; ENTRIES] = core::array::from_fn(|_| None);
    let mut publication = None;
    let mut registration_control = None;
    let mut ready = false;
    let mut sealing = false;
    let mut stop_prepared = false;
    let mut withdrawal = None;
    let mut withdrawal_finished = with_registry;
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
            // 已领取但尚未交付的根也须退回原领域寿命，不能由局部数组保活至收空之后。
            for roots in [
                &mut file_senders,
                &mut management_senders,
                &mut directory_senders,
                &mut authority_senders,
            ] {
                for sender in roots {
                    drop(sender.take());
                }
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
        let infrastructure_ready = [
            file_ingress,
            management_ingress,
            release_task,
            world.retirement_task,
        ]
        .into_iter()
        .chain(service_ingress)
        .all(|id| {
            runtime.get_task_mut(id).is_some_and(|task| match task {
                FileTask::Ingress(task) => task.source.source().is_some(),
                FileTask::ServiceIngress(task) => task.admitted(),
                FileTask::Release(task) => task.admitted(),
                FileTask::Retire(task) => task.admitted(),
                _ => false,
            })
        });
        if !world.stop && infrastructure_ready {
            for entry in 0..entries {
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
            && infrastructure_ready
            && !published_file
            && file_senders[..entries].iter().all(Option::is_some)
        {
            if with_registry {
                export_all(
                    &mut reporter,
                    FILE_ROOT_EXPORT,
                    &mut file_senders[..entries],
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
                        publish_name,
                    )
                    .expect("publication request preparation failed"),
                );
                runtime.wake(dispatcher).expect("publication wake failed");
            }
            published_file = true;
        }
        if !world.stop
            && infrastructure_ready
            && !published_directory
            && directory_senders[..entries].iter().all(Option::is_some)
        {
            export_all(
                &mut reporter,
                DIRECTORY_EXPORT,
                &mut directory_senders[..entries],
            );
            published_directory = true;
        }
        if !world.stop
            && infrastructure_ready
            && !published_management
            && management_senders[..entries].iter().all(Option::is_some)
        {
            export_all(
                &mut reporter,
                MANAGEMENT_EXPORT,
                &mut management_senders[..entries],
            );
            published_management = true;
        }
        if !world.stop
            && infrastructure_ready
            && !published_authority
            && authority_senders[..entries].iter().all(Option::is_some)
        {
            export_all(
                &mut reporter,
                AUTHORITY_EXPORT,
                &mut authority_senders[..entries],
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
                name: publish_name,
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
            && infrastructure_ready
            && !ready
            && published_file
            && published_directory
            && published_authority
            && published_management
            && (with_registry || registration_control.is_some())
        {
            reporter
                .ready(&[], Deadline::INFINITE)
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
    reporter
        .stopped(Deadline::INFINITE)
        .expect("provider stopped publication failed");
}
