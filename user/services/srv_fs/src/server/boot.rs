//! 采用预建 receiver/pair；本地角色全部安装之后才让 Ingress 接收。
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
    message::{Mailbox, MailboxSender, MintedSender},
    notification,
    object::query,
    wait_set::WaitSet,
};
use libservice::{
    authority::{AccessTable, AuthorityTable, Scope},
    maintenance::Maintenance,
    protocol::{
        self as service, Addressing, Descriptors, ProtocolDescriptor, RegisterMode,
        Request as Command, Response,
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
    startup::Config,
};
fn begin_registration(
    dispatcher: &mut librpc::dispatcher::Dispatcher,
    authority: MailboxSender,
    endpoint: MailboxSender,
    funding: &libbudget::AccountView<ExecutionResource>,
    name: &str,
    transport: Rights,
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
pub(crate) struct EntryPairs {
    pub file: MintedSender,
    pub management: MintedSender,
    pub directory: Option<MintedSender>,
    pub authority: Option<MintedSender>,
}
pub(crate) struct Startup {
    pub pool: Capability,
    pub config: Config<'static>,
    pub authority: Option<MailboxSender>,
    pub namespace: liboal::Namespace,
    pub route: Mailbox,
    pub service: Option<Mailbox>,
    pub pairs: alloc::vec::Vec<EntryPairs>,
}
fn scope(name: &str) -> Result<Scope, SystemCallError> {
    if name == "/" {
        return Ok(Scope::Root);
    }
    let mut owned = alloc::string::String::new();
    owned
        .try_reserve_exact(name.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    owned.push_str(name);
    Ok(Scope::ExactName(owned))
}
pub(crate) fn run(mailbox: Mailbox, startup: Startup) {
    let Startup {
        pool,
        config,
        authority,
        namespace: _namespace,
        route,
        service: service_mailbox,
        pairs,
    } = startup;
    // Namespace 与基础设施 Pool 持有到整个 provider 生命期结束。
    let with_registry = config.registry;
    let entries = config.entries().len();
    let capacity = Capacity::new(&config).expect("provider capacity invalid");
    let sources = capacity.sources;
    let tasks = capacity.tasks;
    let mut publisher = authority;
    let publish_name = config.publish.unwrap_or_default();
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
    // 普通 grant 表与独立 management 表分别持有自己的真实角色槽。
    limits[fs_base + FsResource::Grant.slot()] = GRANTS + entries;
    limits[fs_base + FsResource::Watch.slot()] = WATCHES;
    limits[fs_base + FsResource::Offer.slot()] = srv_fs::deployment::SESSIONS;
    limits[fs_base + FsResource::Stream.slot()] = srv_fs::deployment::SESSIONS;
    limits[fs_base + FsResource::Bytes.slot()] = srv_fs::deployment::DOMAIN_BYTES;
    limits[fs_base + FsResource::WaitSource.slot()] =
        GRANTS + REQUESTS + WATCHES + 2 * srv_fs::deployment::SESSIONS + entries + 3;
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
            local[fs_base + FsResource::Watch.slot()] = config.entry(entry).watches;
            local[fs_base + FsResource::Stream.slot()] = config.entry(entry).sessions;
            local[fs_base + FsResource::Offer.slot()] = config.entry(entry).sessions;
            local[service_base + ServiceResource::Watch.slot()] = config.entry(entry).watches;
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
        pool,
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
    let mut directory_roots = [None; ENTRIES];
    let mut authority_roots = [None; ENTRIES];
    for (entry, pair) in pairs.into_iter().enumerate() {
        let (execution, fs, service_account) = &tenants[entry];
        let policy = config.entry(entry);
        let file_transport = pair
            .file
            .sender
            .description()
            .expect("file sender query failed")
            .rights;
        let prepared = world
            .grants
            .prepare_minted(
                &world.mailbox,
                world
                    .memory
                    .root()
                    .expect("file backing root missing")
                    .clone(),
                libfs::authority::Issuance {
                    rights: policy.file.rights,
                    sender_transport: file_transport,
                    output_transport: policy.file.output,
                },
                fs,
                execution,
                pair.file,
            )
            .map_err(|failure| failure.error)
            .expect("file root adoption failed");
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
                .map_err(|failure| failure.error)
                .expect("file root admission failed"),
        );
        let prepared = world
            .management
            .prepare_minted(
                &world.route,
                world
                    .memory
                    .root()
                    .expect("management root missing")
                    .clone(),
                policy.management.rights,
                policy.management.output,
                fs,
                execution,
                pair.management,
            )
            .map_err(|failure| failure.error)
            .expect("management root adoption failed");
        management_roots[entry] = Some(
            runtime
                .spawn(libexecution::TaskSubmission::new(
                    FileTask::FileRoot(
                        lifecycle::Root::management(prepared, fs)
                            .expect("management owner preparation failed"),
                    ),
                    1,
                    execution.clone(),
                ))
                .map_err(|failure| failure.error)
                .expect("management root admission failed"),
        );
        if let Some(state) = world.service.as_ref() {
            let directory = policy.directory.expect("directory policy missing");
            let root = Root::prepare_minted(
                state,
                Policy::Directory {
                    scope: scope(directory.scope).expect("directory scope allocation failed"),
                    rights: directory.rights,
                    output: directory.output,
                },
                pair.directory.expect("directory pair missing"),
                service_account,
                execution,
            )
            .map_err(|failure| failure.status)
            .expect("directory root adoption failed");
            directory_roots[entry] = Some(
                runtime
                    .spawn(libexecution::TaskSubmission::new(
                        FileTask::ServiceRoot(root),
                        1,
                        execution.clone(),
                    ))
                    .map_err(|failure| failure.error)
                    .expect("directory root admission failed"),
            );
            let authority = policy.authority.expect("authority policy missing");
            let root = Root::prepare_minted(
                state,
                Policy::Authority {
                    scope: scope(authority.scope).expect("authority scope allocation failed"),
                    rights: authority.rights,
                    output: authority.output,
                },
                pair.authority.expect("authority pair missing"),
                service_account,
                execution,
            )
            .map_err(|failure| failure.status)
            .expect("authority root adoption failed");
            authority_roots[entry] = Some(
                runtime
                    .spawn(libexecution::TaskSubmission::new(
                        FileTask::ServiceRoot(root),
                        1,
                        execution.clone(),
                    ))
                    .map_err(|failure| failure.error)
                    .expect("authority root admission failed"),
            );
        }
    }
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
    if let Some(state) = world.service.as_mut() {
        let maintenance = Maintenance::new(state).expect("registry maintenance preparation failed");
        state.maintenance = runtime
            .spawn(libexecution::TaskSubmission::new(
                FileTask::Maintenance(maintenance),
                0,
                execution.clone(),
            ))
            .map_err(|f| f.error)
            .expect("registry maintenance admission failed");
    }
    // 上游 sender 已能发送。只驱动来源/角色 owner；任何 Ingress 尚未进入 Runtime，
    // 因而早到消息仍留在真实内核队列中，不会得到临时 Permission 或额外缓存。
    loop {
        for &record in assembly
            .receive(64)
            .expect("provider adoption source receive failed")
        {
            runtime.feed(record);
        }
        runtime
            .turn(&mut world, 1)
            .unwrap_or_else(|failure| panic!("provider adoption failed: {:?}", failure.error));
        assert!(!world.stop, "provider role source admission failed");
        let files = file_roots[..entries]
            .iter()
            .chain(&management_roots[..entries])
            .all(|id| {
                id.and_then(|id| runtime.get_task_mut(id)).is_some_and(
                    |task| matches!(task, FileTask::FileRoot(root) if root.installed()),
                )
            });
        let roles = !with_registry
            || directory_roots[..entries]
                .iter()
                .chain(&authority_roots[..entries])
                .all(|id| {
                    id.and_then(|id| runtime.get_task_mut(id)).is_some_and(
                        |task| matches!(task, FileTask::ServiceRoot(root) if root.installed()),
                    )
                });
        if files && roles {
            break;
        }
        if let DriveState::Waiting(deadline) = runtime.drive_state() {
            assembly
                .wait(deadline)
                .expect("provider adoption wait failed");
        }
    }
    let mut publication_endpoint = None;
    for (entry, id) in file_roots[..entries].iter().enumerate() {
        let Some(FileTask::FileRoot(root)) =
            runtime.get_task_mut(id.expect("file root task missing"))
        else {
            unreachable!("file root task role changed")
        };
        let sender = root.take_sender().expect("installed file witness missing");
        if !with_registry && entry == 0 {
            publication_endpoint = Some(sender);
        } else {
            drop(sender);
        }
    }
    for id in management_roots[..entries].iter().flatten() {
        let Some(FileTask::FileRoot(root)) = runtime.get_task_mut(*id) else {
            unreachable!("management root task role changed")
        };
        drop(root.take_sender());
    }
    for id in directory_roots[..entries]
        .iter()
        .chain(&authority_roots[..entries])
        .flatten()
    {
        let Some(FileTask::ServiceRoot(root)) = runtime.get_task_mut(*id) else {
            unreachable!("service root task role changed")
        };
        drop(root.take_sender());
    }
    // 全部 Lifetime 来源实际登记、授权表项安装并释放 witness 后才创建收件任务。
    for ingress in [Ingress::new(&fs), Ingress::management(&fs)] {
        runtime
            .spawn(libexecution::TaskSubmission::new(
                FileTask::Ingress(ingress.expect("file ingress preparation failed")),
                Ingress::MAX_SOURCES,
                execution.clone(),
            ))
            .map_err(|failure| failure.error)
            .expect("file ingress admission failed");
    }
    if let Some(state) = world.service.as_ref() {
        runtime
            .spawn(libexecution::TaskSubmission::new(
                FileTask::ServiceIngress(
                    services::Ingress::new(state).expect("registry ingress preparation failed"),
                ),
                2,
                execution.clone(),
            ))
            .map_err(|failure| failure.error)
            .expect("registry ingress admission failed");
    }
    let publication_transport = publication_endpoint
        .as_ref()
        .map(|endpoint| {
            let actual = endpoint
                .description()
                .expect("publication endpoint query failed")
                .rights;
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | (actual & Rights::DUPLICATE)
        })
        .unwrap_or(Rights::NONE);
    let mut publication = publication_endpoint.map(|endpoint| {
        let call = begin_registration(
            world.rpc_mut(),
            publisher.take().expect("publication authority missing"),
            endpoint,
            &tenants[0].0,
            publish_name,
            publication_transport,
        )
        .expect("publication request preparation failed");
        runtime.wake(dispatcher).expect("publication wake failed");
        call
    });
    let mut registration_control = None;
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
                export_transport: publication_transport,
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
}
