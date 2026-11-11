//! 组态执行器：先建立付款、真实运输入口及完整出生视图，再逐个发布进程。
//! init 不解释领域政策、不等待业务就绪；发布后的内核 DONE 由根监督持续观察。
use super::platform::Platform;
use alloc::{string::String, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    memory_pool::{METADATA_SLOT_BYTES, MemoryPoolBudget},
    object::{Handle, ObjectSignals, Rights},
    proc::{
        HandleGrant, PROCESS_MAIN_STACK_SIZE, PROCESS_MAX_GRANTS, PROCESS_PAGE_SIZE,
        PROCESS_USER_TOP, ProcessState,
    },
    time::Deadline,
    wait_set::MemberRequest,
};
use formation::{Formation, ResourceKind, ResourceOutput, Source};
use libkernel::{
    debug,
    ipc::{
        capability::Capability,
        message::{LifetimeObserver, Mailbox, MailboxSender},
        object,
        wait_set::WaitSet,
    },
    process,
};
use libprocess::{
    GrantOutcome, Preparation, SpawnRequest,
    context::CurrentProcessInfo,
    image::{Image, ImageCopy},
};

const SUPERVISOR_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSFER.raw(),
);
pub(super) const JOB_RIGHTS: Rights = Rights::from_raw(
    Rights::CREATE.raw()
        | Rights::MANAGE.raw()
        | Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSFER.raw(),
);
const POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::CREATE.raw() | Rights::DUPLICATE.raw() | Rights::TRANSFER.raw(),
);
pub(super) const CALL_RIGHTS: Rights =
    Rights::from_raw(Rights::WRITE.raw() | Rights::WAIT.raw() | Rights::TRANSFER.raw());
pub(super) const TERMINAL_REVOKE_MS: u64 = 5_000;
const FORMATION_ENTRY: &str = "formation";

fn entry(archive: &'static [u8], name: &str) -> Option<&'static [u8]> {
    let mut found = None;
    let mut duplicate = false;
    tar::walk(archive, |entry| {
        if entry.name == name {
            duplicate |= found.replace(entry.data).is_some();
        }
    })
    .ok()?;
    if duplicate { None } else { found }
}

fn image_path(component: &str) -> String {
    alloc::format!("image/{component}")
}

pub(super) fn load(
    archive: &'static [u8],
    platform: &Platform,
    root_pool: Handle,
) -> Result<PreparedFormation, &'static str> {
    let bytes = entry(archive, FORMATION_ENTRY).ok_or("initfs formation missing or duplicated")?;
    let formation = Formation::decode(bytes).map_err(|error| {
        debug!("formation decode failed: {}", error);
        "formation encoding invalid"
    })?;
    formation
        .validate(|component| entry(archive, &image_path(component)).is_some())
        .map_err(|error| {
            debug!("formation rejected: {}", error);
            "formation invalid"
        })?;
    formation
        .validate_grants(platform.roots().len())
        .map_err(|_| "formation expanded grants invalid")?;
    let plans = preflight(&formation, archive, platform)?;
    let (pages, metadata) = plans
        .iter()
        .map(|plan| (plan.budget.pages, plan.budget.metadata))
        .chain(formation.pools.iter().map(|pool| {
            (
                pool.budget.work_pages,
                pool.budget.metadata_bytes.div_ceil(METADATA_SLOT_BYTES),
            )
        }))
        .try_fold((0u64, 0u64), |(pages, metadata), (p, m)| {
            Some((pages.checked_add(p)?, metadata.checked_add(m)?))
        })
        .ok_or("formation budget sum overflow")?;
    let root = libkernel::memory_pool::query(root_pool).map_err(|_| "root Pool query failed")?;
    if pages > root.available || metadata > root.metadata.available {
        return Err("formation exceeds root Pool available budget");
    }
    Ok(PreparedFormation { formation, plans })
}

pub(super) struct PreparedFormation {
    formation: Formation,
    plans: Vec<BirthPlan>,
}

pub(super) fn owned(handle: Handle) -> Capability {
    // SAFETY: 调用者刚从工厂或转移取得唯一表项，此处独占关闭责任。
    unsafe { Capability::from_raw(handle) }
}

/// 一个展开后的真实输入；平台根列表只在此保留其确定序号。
struct PlannedInput {
    name: String,
    source: Source,
    rights: Rights,
    platform_index: usize,
}
struct BirthPlan {
    inputs: Vec<PlannedInput>,
    payload: Vec<u8>,
    budget: MemoryPoolBudget,
}
#[derive(Default)]
struct Record {
    pid: u64,
    pool: Option<Capability>,
    job: Option<Capability>,
    control: Option<Capability>,
}
enum ResourceState {
    Mailbox(Option<Mailbox>),
    Sender {
        sender: Option<MailboxSender>,
        lifetime: Option<LifetimeObserver>,
    },
}

pub(super) struct Executor<'a> {
    formation: Formation,
    archive: &'static [u8],
    parent: CurrentProcessInfo,
    services: Handle,
    root_pool: Handle,
    platform: &'a Platform,
    plans: Vec<BirthPlan>,
    records: Vec<Record>,
    pools: Vec<(String, Capability)>,
    jobs: Vec<(String, Capability)>,
    images: Vec<(String, Image)>,
    /// 下标始终与 formation.resources 对应；receiver/Lifetime 转授后槽位清空。
    resources: Vec<Option<ResourceState>>,
    set: WaitSet,
}

/// 纯 preflight：不构造任何对象。实际出生块页数与 loader 使用同一几何。
fn preflight(
    formation: &Formation,
    archive: &'static [u8],
    platform: &Platform,
) -> Result<Vec<BirthPlan>, &'static str> {
    let mut plans = Vec::new();
    for instance in &formation.instances {
        let mut inputs = Vec::new();
        for input in &instance.inputs {
            let rights = input.rights.unwrap_or(
                formation
                    .source_rights(&input.source)
                    .map_err(|_| "formation source rights invalid")?,
            );
            if matches!(input.source, Source::PlatformRoot) {
                for index in 0..platform.roots().len() {
                    inputs.push(PlannedInput {
                        name: alloc::format!("{}.{index}", input.name),
                        source: input.source.clone(),
                        rights,
                        platform_index: index,
                    });
                }
            } else {
                inputs.push(PlannedInput {
                    name: input.name.clone(),
                    source: input.source.clone(),
                    rights,
                    platform_index: 0,
                });
            }
        }
        let mut description = oal::namespace::Description {
            bindings: Vec::new(),
        };
        // 编译器与执行器都按 prefix 顺序建立隐藏输入，描述不保存裸 handle。
        let mut bindings: Vec<_> = instance.namespace.iter().collect();
        bindings.sort_unstable_by(|a, b| a.prefix.as_bytes().cmp(b.prefix.as_bytes()));
        for (index, binding) in bindings.into_iter().enumerate() {
            let name = oal::namespace::input_name(index);
            description.bindings.push(oal::namespace::Binding {
                prefix: binding.prefix.clone(),
                input: name.clone(),
            });
            inputs.push(PlannedInput {
                name,
                source: binding.source.clone(),
                rights: binding.rights,
                platform_index: 0,
            });
        }
        if inputs.len() > PROCESS_MAX_GRANTS {
            return Err("instance inputs exceed startup grants");
        }
        let namespace = description
            .encode()
            .map_err(|_| "instance namespace invalid")?;
        let table: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(slot, input)| (input.name.as_str(), slot as u8))
            .collect();
        let payload = libprocess::birth::encode(
            &table,
            &instance.settings,
            &[(oal::namespace::SECTION_NAME, namespace.as_slice())],
        )
        .map_err(|_| "instance birth frame invalid")?;
        let block_len = erhino_shared::startup::compact_block_len(inputs.len() + 1, payload.len())
            .ok_or("instance startup geometry overflow")?;
        let bytes =
            entry(archive, &image_path(&instance.image)).ok_or("formation image missing")?;
        let mut validation = elf::Validation::new(
            bytes,
            elf::LoadLimits {
                page_size: PROCESS_PAGE_SIZE as u64,
                image_limit: (PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE) as u64,
            },
        )
        .map_err(|_| "instance image is not a loadable ELF")?;
        let plan = loop {
            if let Some(plan) = validation
                .advance()
                .map_err(|_| "instance image is not a loadable ELF")?
            {
                break plan;
            }
        };
        let pages = plan
            .runs()
            .iter()
            .try_fold(0u64, |sum, run| {
                sum.checked_add(run.memsz.div_ceil(PROCESS_PAGE_SIZE as u64))
            })
            .and_then(|pages| {
                pages.checked_add((PROCESS_MAIN_STACK_SIZE / PROCESS_PAGE_SIZE) as u64)
            })
            .and_then(|pages| pages.checked_add(block_len.div_ceil(PROCESS_PAGE_SIZE) as u64))
            .and_then(|pages| pages.checked_add(instance.pool.work_pages))
            .ok_or("instance page budget overflow")?;
        plans.push(BirthPlan {
            inputs,
            payload,
            budget: MemoryPoolBudget {
                pages,
                metadata: instance.pool.metadata_bytes.div_ceil(METADATA_SLOT_BYTES),
            },
        });
    }
    Ok(plans)
}

impl<'a> Executor<'a> {
    pub(super) fn new(
        prepared: PreparedFormation,
        archive: &'static [u8],
        parent: CurrentProcessInfo,
        services: Handle,
        root_pool: Handle,
        platform: &'a Platform,
    ) -> Result<Self, &'static str> {
        let PreparedFormation { formation, plans } = prepared;
        let count = formation.instances.len();
        let set = WaitSet::create(count.max(1)).map_err(|_| "formation WaitSet failed")?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(count)
            .map_err(|_| "formation record storage failed")?;
        records.resize_with(count, Record::default);
        let mut resources = Vec::new();
        resources
            .try_reserve_exact(formation.resources.len())
            .map_err(|_| "formation resource storage failed")?;
        resources.resize_with(formation.resources.len(), || None);
        Ok(Self {
            formation,
            archive,
            parent,
            services,
            root_pool,
            platform,
            plans,
            records,
            pools: Vec::new(),
            jobs: Vec::new(),
            images: Vec::new(),
            resources,
            set,
        })
    }

    /// 显式委托给既有故障终局的能力；资源名字不参与协议判断。
    pub(super) fn admins(&self) -> Vec<Handle> {
        self.formation
            .resources
            .iter()
            .zip(&self.resources)
            .filter_map(|(definition, state)| {
                if !definition.terminal_revoke {
                    return None;
                }
                match state {
                    Some(ResourceState::Sender {
                        sender: Some(sender),
                        ..
                    }) => Some(sender.as_handle()),
                    _ => None,
                }
            })
            .collect()
    }

    fn derive_pool(&self, budget: MemoryPoolBudget) -> Result<Capability, &'static str> {
        libkernel::memory_pool::derive(self.root_pool, budget, POOL_RIGHTS)
            .map(|pool| owned(pool.into_handle()))
            .map_err(|_| "formation Pool admission failed")
    }
    fn payer(&self, name: &str) -> Result<Handle, &'static str> {
        if name == "init" {
            return Ok(self.root_pool);
        }
        if let Some((_, pool)) = self.pools.iter().find(|(candidate, _)| candidate == name) {
            return Ok(pool.as_handle());
        }
        self.formation
            .instances
            .iter()
            .position(|instance| instance.name == name)
            .and_then(|index| self.records[index].pool.as_ref())
            .map(Capability::as_handle)
            .ok_or("formation resource payer unavailable")
    }
    fn named_job(&mut self, name: &str) -> Result<Handle, &'static str> {
        if name == formation::SERVICES_JOB {
            return Ok(self.services);
        }
        if let Some((_, job)) = self.jobs.iter().find(|(candidate, _)| candidate == name) {
            return Ok(job.as_handle());
        }
        let job = self
            .formation
            .job(name)
            .ok_or("formation Job undeclared")?
            .clone();
        let parent = self.named_job(&job.parent)?;
        let payer = self.payer(&job.payer)?;
        let handle = process::create_job(parent, payer, JOB_RIGHTS)
            .map_err(|_| "formation Job creation failed")?;
        self.jobs.push((job.name, owned(handle)));
        Ok(handle)
    }
    fn resource_index(&self, name: &str) -> Result<usize, &'static str> {
        self.formation
            .resources
            .iter()
            .position(|resource| resource.name == name)
            .ok_or("formation resource undeclared")
    }
    fn create_resources(&mut self) -> Result<(), &'static str> {
        // receiver 在全部 sender 铸造前不移交；这里只排序构造依赖，不排序服务业务。
        for index in 0..self.formation.resources.len() {
            let definition = &self.formation.resources[index];
            if let ResourceKind::Mailbox { rights } = definition.kind {
                let receiver = Mailbox::create_in(
                    self.payer(&definition.payer)?,
                    rights | Rights::MANAGE | Rights::TRANSFER,
                )
                .map_err(|_| "formation mailbox creation failed")?;
                self.resources[index] = Some(ResourceState::Mailbox(Some(receiver)));
            }
        }
        for index in 0..self.formation.resources.len() {
            let definition = &self.formation.resources[index];
            if let ResourceKind::Sender {
                mailbox,
                badge,
                rights,
            } = &definition.kind
            {
                let payer = self.payer(&definition.payer)?;
                let receiver = match self.resources[self.resource_index(mailbox)?].as_ref() {
                    Some(ResourceState::Mailbox(Some(receiver))) => receiver,
                    _ => return Err("formation sender receiver unavailable"),
                };
                let minted = receiver
                    .mint_in(
                        payer,
                        *badge,
                        *rights | Rights::DUPLICATE | Rights::TRANSFER,
                    )
                    .map_err(|_| "formation sender creation failed")?;
                self.resources[index] = Some(ResourceState::Sender {
                    sender: Some(minted.sender),
                    lifetime: Some(minted.lifetime),
                });
            }
        }
        Ok(())
    }
    fn image(&mut self, component: &str) -> Result<Handle, &'static str> {
        if let Some((_, image)) = self.images.iter().find(|(name, _)| name == component) {
            return Ok(image.handle());
        }
        let bytes = entry(self.archive, &image_path(component)).ok_or("formation image missing")?;
        let mut copy = ImageCopy::new(bytes).map_err(|_| "formation image copy failed")?;
        let image = loop {
            self.pump(Deadline::at(0))?;
            if let Some(image) = copy.advance().map_err(|_| "formation image copy failed")? {
                break image;
            }
            if let Some(handle) = copy.waiting() {
                libkernel::ipc::wait::wait_until(
                    handle,
                    ObjectSignals::EXECUTABLE,
                    Deadline::INFINITE,
                )
                .map_err(|_| "formation image seal wait failed")?;
            }
        };
        let handle = image.handle();
        self.images.push((component.into(), image));
        Ok(handle)
    }
    fn input_owner(
        &mut self,
        index: usize,
        input: &PlannedInput,
    ) -> Result<Capability, &'static str> {
        if let Source::Resource { name, output } = &input.source {
            let slot = self.resource_index(name)?;
            return match (self.resources[slot].as_mut(), output) {
                (Some(ResourceState::Mailbox(receiver)), ResourceOutput::Main) => receiver
                    .take()
                    .map(|receiver| owned(receiver.into_raw()))
                    .ok_or("receiver already transferred"),
                (Some(ResourceState::Sender { lifetime, .. }), ResourceOutput::Lifetime) => {
                    lifetime
                        .take()
                        .map(LifetimeObserver::into_capability)
                        .ok_or("Lifetime already transferred")
                }
                (
                    Some(ResourceState::Sender {
                        sender: Some(sender),
                        ..
                    }),
                    ResourceOutput::Main,
                ) => object::duplicate(sender.as_handle(), input.rights | Rights::TRANSFER)
                    .map(owned)
                    .map_err(|_| "sender input alias failed"),
                _ => Err("formation resource output unavailable"),
            };
        }
        let handle = match &input.source {
            Source::PlatformDescription => self.platform.description(),
            Source::PlatformRoot => self
                .platform
                .roots()
                .get(input.platform_index)
                .ok_or("platform input index invalid")?
                .capability
                .as_handle(),
            Source::Image(component) => self.image(component)?,
            Source::Job(name) => self.named_job(name)?,
            Source::Pool(name) => self.payer(name)?,
            Source::SelfPool => self.records[index]
                .pool
                .as_ref()
                .ok_or("instance Pool missing")?
                .as_handle(),
            Source::Resource { .. } => return Err("resource resolution escaped ownership branch"),
        };
        object::duplicate(handle, input.rights | Rights::TRANSFER)
            .map(owned)
            .map_err(|_| "instance input alias failed")
    }
    fn start(&mut self, index: usize, plan: BirthPlan) -> Result<(), &'static str> {
        let instance = self.formation.instances[index].clone();
        let job = if let Some(name) = &instance.job {
            self.named_job(name)?
        } else {
            let pool = self.records[index]
                .pool
                .as_ref()
                .ok_or("instance Pool missing")?
                .as_handle();
            let handle = process::create_job(self.services, pool, JOB_RIGHTS)
                .map_err(|_| "instance Job creation failed")?;
            self.records[index].job = Some(owned(handle));
            handle
        };
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(plan.inputs.len())
            .map_err(|_| "instance input storage failed")?;
        for input in &plan.inputs {
            owners.push(self.input_owner(index, input)?);
        }
        let grants: Vec<_> = owners
            .iter()
            .zip(&plan.inputs)
            .map(|(owner, input)| HandleGrant {
                handle: owner.as_handle(),
                rights: input.rights,
            })
            .collect();
        let bytes =
            entry(self.archive, &image_path(&instance.image)).ok_or("formation image missing")?;
        let mut preparation = Preparation::new(SpawnRequest {
            job,
            parent: self.parent,
            memory_pool: self.records[index]
                .pool
                .as_ref()
                .ok_or("instance Pool missing")?
                .as_handle(),
            image: bytes,
            payload: &plan.payload,
            grants: &grants,
            control_rights: SUPERVISOR_RIGHTS,
        })
        .map_err(|_| "instance preparation failed")?;
        let result = (|| {
            loop {
                self.pump(Deadline::at(0))?;
                match preparation.advance() {
                    Ok(Some(prepared)) => break Ok(prepared),
                    Ok(None) => {}
                    Err(error) => {
                        debug!(
                            "instance preparation failed: {}, {:?}",
                            instance.name, error
                        );
                        break Err("instance preparation failed");
                    }
                }
            }
        })();
        // DONE 监督本身也可能在 Grant 提交后中断装配；所有退出边统一核实际消费事实。
        if preparation.grants() == GrantOutcome::Consumed {
            for owner in owners {
                owner.into_raw();
            }
        }
        let prepared = result?;
        // Start 前建立持续 DONE；目标还未接收任何业务也已处于根监督责任下。
        self.set
            .register(MemberRequest::new(
                prepared.control(),
                ObjectSignals::DONE,
                index as u64,
            ))
            .map_err(|_| "instance DONE observation failed")?;
        let mut prepared = prepared;
        let spawned = prepared.try_start().map_err(|_| "instance Start failed")?;
        self.records[index].pid = spawned.pid;
        self.records[index].control = Some(owned(spawned.control));
        debug!(
            "formation instance published: {}, pid={}",
            instance.name, spawned.pid
        );
        Ok(())
    }
    pub(super) fn start_all(&mut self) -> Result<(), &'static str> {
        // Pool 不依赖进程执行；所有资源及环形接线的来源在首次 Start 前已存在。
        for pool in self.formation.pools.clone() {
            let owner = self.derive_pool(MemoryPoolBudget {
                pages: pool.budget.work_pages,
                metadata: pool.budget.metadata_bytes.div_ceil(METADATA_SLOT_BYTES),
            })?;
            self.pools.push((pool.name, owner));
        }
        for index in 0..self.records.len() {
            self.records[index].pool = Some(self.derive_pool(self.plans[index].budget)?);
        }
        let jobs: Vec<_> = self
            .formation
            .jobs
            .iter()
            .map(|job| job.name.clone())
            .collect();
        for name in jobs {
            self.named_job(&name)?;
        }
        self.create_resources()?;
        let plans = core::mem::take(&mut self.plans);
        for (index, plan) in plans.into_iter().enumerate() {
            self.start(index, plan)?;
        }
        // 施工母本只保留显式终局角色，不能意外维持业务 Lifetime authority。
        for (definition, state) in self.formation.resources.iter().zip(&mut self.resources) {
            match state {
                Some(ResourceState::Mailbox(receiver)) if receiver.is_some() => {
                    return Err("formation receiver left without owner");
                }
                Some(ResourceState::Sender { sender, lifetime }) => {
                    lifetime.take();
                    if !definition.terminal_revoke {
                        sender.take();
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn pump(&mut self, deadline: Deadline) -> Result<(), &'static str> {
        self.set
            .wait(deadline)
            .map_err(|_| "formation wait failed")?;
        let ready = match self.set.receive(self.records.len().max(1)) {
            Ok(ready) => ready,
            Err(SystemCallError::ObjectNotAvailable) => return Ok(()),
            Err(_) => return Err("formation receive failed"),
        };
        for event in ready {
            let index = usize::try_from(event.cookie)
                .map_err(|_| "formation observation cookie invalid")?;
            let record = self
                .records
                .get(index)
                .ok_or("formation observation index invalid")?;
            let control = record.control.as_ref().ok_or("instance Control missing")?;
            let snapshot =
                process::query(control.as_handle()).map_err(|_| "instance query failed")?;
            if snapshot.state == ProcessState::Dead as u32 {
                debug!(
                    "formation foundation exited: {}, pid={}, cause={}, detail={:?}",
                    self.formation.instances[index].name,
                    record.pid,
                    snapshot.cause,
                    snapshot.detail
                );
                return Err("foundation instance exited");
            }
        }
        Ok(())
    }
    pub(super) fn supervise(&mut self) -> Result<(), &'static str> {
        loop {
            self.pump(Deadline::INFINITE)?;
        }
    }
}
