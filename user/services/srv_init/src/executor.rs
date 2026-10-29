//! 组态执行器：读取 initfs 根的 `formation`，按拓扑序启动实例、按名路由能力、
//! 持续监督，并在 `finish` 要求时按反序正常停止。init 不认识任何领域协议：
//! 实例只经出生帧收到命名输入，经 `instance.report` 交出命名导出。
use super::platform::{DESCRIPTION_RIGHTS, Platform, ROOT_RIGHTS};
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
use erhino_shared::{
    call::SystemCallError,
    memory_pool::{METADATA_SLOT_BYTES, MemoryPoolBudget},
    object::{Handle, ObjectSignals, Rights},
    proc::{
        HandleGrant, PROCESS_MAIN_STACK_SIZE, PROCESS_PAGE_SIZE, PROCESS_USER_TOP, ProcessState,
        TerminationCause, TerminationDetail,
    },
    time::Deadline,
    wait_set::MemberRequest,
};
use formation::{Finish, Formation, Input, Kind, Source};
use libkernel::{
    debug,
    ipc::{
        capability::Capability,
        message::{Mailbox, MailboxSender},
        notification::{self, NotificationSignaler},
        object,
        wait_set::WaitSet,
    },
    process,
};
use libprocess::{
    GrantOutcome, Preparation, SpawnRequest,
    context::CurrentProcessInfo,
    image::{IMAGE_RIGHTS, Image, ImageCopy},
};
use libservice::instance::{self, Report, revoke};

/// init 保留的实例 Control 权利：查询、等待、终止与受保护收束。
const SUPERVISOR_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::GRANT.raw(),
);
pub(super) const JOB_RIGHTS: Rights = Rights::from_raw(
    Rights::CREATE.raw()
        | Rights::MANAGE.raw()
        | Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
const POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
/// 撤销调用经导出管理端的临时别名发出。
pub(super) const CALL_RIGHTS: Rights =
    Rights::from_raw(Rights::WRITE.raw() | Rights::WAIT.raw() | Rights::TRANSIT.raw());
/// 撤销轮询的重查间隔与终局撤销的总期限。
const REVOKE_POLL_MS: u64 = 1;
pub(super) const TERMINAL_REVOKE_MS: u64 = 5_000;
const FORMATION_ENTRY: &str = "formation";
const IMAGE_DIR: &str = "image/";

/// initfs 中的一个文件；归档为 init 出生 payload，进程生命期内有效。
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
    let mut path = String::new();
    let _ = write!(path, "{IMAGE_DIR}{component}");
    path
}

/// 读取并静态校验组态；任何副作用之前完成。
pub(super) fn load(archive: &'static [u8]) -> Result<Formation, &'static str> {
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
    Ok(formation)
}

/// 一条路由解析出的（输入名、源 handle）列表与授予权利。
type Resolved = (Vec<(String, Handle)>, Rights);

pub(super) fn owned(handle: Handle) -> Capability {
    // SAFETY: 调用者刚由 create/derive/duplicate 取得该表项，关闭责任由此唯一移交。
    unsafe { Capability::from_raw(handle) }
}

fn timeout(ms: u64) -> Result<Deadline, &'static str> {
    if ms == 0 {
        Ok(Deadline::INFINITE)
    } else {
        libkernel::time::timeout_millis(ms).map_err(|_| "formation clock failed")
    }
}

fn expired(deadline: Deadline) -> Result<bool, &'static str> {
    libkernel::time::expired(deadline).map_err(|_| "formation clock failed")
}

/// 映像页：ELF load runs、主栈与出生页；工作集由组态另加。
fn image_pages(bytes: &[u8]) -> Result<u64, &'static str> {
    let page = PROCESS_PAGE_SIZE as u64;
    let mut validation = elf::Validation::new(
        bytes,
        elf::LoadLimits {
            page_size: page,
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
    plan.runs()
        .iter()
        .try_fold(0u64, |sum, run| sum.checked_add(run.memsz.div_ceil(page)))
        .and_then(|pages| {
            pages.checked_add((PROCESS_MAIN_STACK_SIZE / PROCESS_PAGE_SIZE) as u64 + 1)
        })
        .ok_or("instance page budget overflow")
}

#[derive(Default)]
struct Record {
    pid: u64,
    /// 实例在 services 下自有的 Job；使用具名 job 时为空。
    job: Option<Capability>,
    pool: Option<Capability>,
    control: Option<Capability>,
    report: Option<Mailbox>,
    stop: Option<NotificationSignaler>,
    exports: Vec<(String, Capability)>,
    tokens: [Option<u64>; 2],
    ready: bool,
    stopping: bool,
    stopped: bool,
    done: bool,
}

/// 消费者停止后须撤销的路由：提供方的管理端导出名。
struct Route {
    consumer: usize,
    provider: usize,
    admin: String,
}

pub(super) struct Executor<'a> {
    formation: Formation,
    archive: &'static [u8],
    parent: CurrentProcessInfo,
    services: Handle,
    root_pool: Handle,
    platform: &'a Platform,
    jobs: Vec<(String, Capability)>,
    pools: Vec<(String, Capability)>,
    images: Vec<(String, Image)>,
    records: Vec<Record>,
    set: WaitSet,
    started: Vec<usize>,
    routes: Vec<Route>,
}

impl<'a> Executor<'a> {
    pub(super) fn new(
        formation: Formation,
        archive: &'static [u8],
        parent: CurrentProcessInfo,
        services: Handle,
        root_pool: Handle,
        platform: &'a Platform,
    ) -> Result<Self, &'static str> {
        let count = formation.instances.len();
        let set = WaitSet::create(2 * count.max(1)).map_err(|_| "formation WaitSet failed")?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(count)
            .map_err(|_| "formation record storage failed")?;
        records.resize_with(count, Record::default);
        Ok(Self {
            formation,
            archive,
            parent,
            services,
            root_pool,
            platform,
            jobs: Vec::new(),
            pools: Vec::new(),
            images: Vec::new(),
            records,
            set,
            started: Vec::new(),
            routes: Vec::new(),
        })
    }

    pub(super) fn finish(&self) -> Finish {
        self.formation.finish
    }

    /// 系统终局须撤销的全部管理端导出。
    pub(super) fn admins(&self) -> Vec<&Capability> {
        self.records
            .iter()
            .flat_map(|record| record.exports.iter())
            .filter(|(name, _)| name.ends_with(".admin"))
            .map(|(_, capability)| capability)
            .collect()
    }

    fn name(&self, index: usize) -> &str {
        &self.formation.instances[index].name
    }

    fn derive_pool(&self, budget: MemoryPoolBudget) -> Result<Capability, &'static str> {
        libkernel::memory_pool::derive(self.root_pool, budget, POOL_RIGHTS)
            .map(|pool| owned(pool.into_handle()))
            .map_err(|error| {
                debug!("formation Pool admission rejected: {:?}", error);
                "formation Pool admission failed"
            })
    }

    fn named_job(&mut self, name: &str) -> Result<Handle, &'static str> {
        if let Some((_, job)) = self.jobs.iter().find(|(job, _)| job == name) {
            return Ok(job.as_handle());
        }
        let job = self
            .formation
            .job(name)
            .ok_or("formation job undeclared")?
            .clone();
        let parent = if job.parent == formation::SERVICES_JOB {
            self.services
        } else {
            self.named_job(&job.parent)?
        };
        let payer = if let Some((_, pool)) = self.pools.iter().find(|(pool, _)| *pool == job.payer)
        {
            pool.as_handle()
        } else {
            let index = self
                .formation
                .instances
                .iter()
                .position(|instance| instance.name == job.payer)
                .ok_or("formation job payer undeclared")?;
            self.records[index]
                .pool
                .as_ref()
                .ok_or("formation job payer has not started")?
                .as_handle()
        };
        let handle = process::create_job(parent, payer, JOB_RIGHTS)
            .map_err(|_| "formation job creation failed")?;
        self.jobs.push((job.name, owned(handle)));
        Ok(handle)
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
        self.images.push((String::from(component), image));
        Ok(handle)
    }

    /// 一条路由解析为若干（输入名、源 handle）及授予权利；列表来源展开为 `<名>.N`。
    fn resolve(&mut self, index: usize, input: &Input) -> Result<Resolved, &'static str> {
        let single = |handle| alloc::vec![(input.name.clone(), handle)];
        let (items, default) = match &input.source {
            Source::PlatformDescription => {
                (single(self.platform.description()), DESCRIPTION_RIGHTS)
            }
            Source::PlatformRoot => {
                let mut items = Vec::new();
                for (n, root) in self.platform.roots().iter().enumerate() {
                    let mut name = input.name.clone();
                    let _ = write!(name, ".{n}");
                    items.push((name, root.capability.as_handle()));
                }
                (items, ROOT_RIGHTS)
            }
            Source::Image(component) => (single(self.image(component)?), IMAGE_RIGHTS),
            Source::Job(name) => (single(self.named_job(name)?), JOB_RIGHTS),
            Source::Pool(name) => {
                let (_, pool) = self
                    .pools
                    .iter()
                    .find(|(pool, _)| pool == name)
                    .ok_or("formation Pool undeclared")?;
                (single(pool.as_handle()), POOL_RIGHTS)
            }
            Source::SelfPool => (
                single(
                    self.records[index]
                        .pool
                        .as_ref()
                        .ok_or("instance Pool missing")?
                        .as_handle(),
                ),
                POOL_RIGHTS,
            ),
            Source::Export { instance, export } => {
                let provider = self
                    .formation
                    .instances
                    .iter()
                    .position(|candidate| candidate.name == *instance)
                    .ok_or("formation provider undeclared")?;
                let declared = self.formation.instances[provider]
                    .exports
                    .iter()
                    .find(|candidate| candidate.name == *export)
                    .ok_or("formation export undeclared")?
                    .rights;
                let (_, capability) = self.records[provider]
                    .exports
                    .iter()
                    .find(|(name, _)| name == export)
                    .ok_or("provider export not delivered")?;
                (single(capability.as_handle()), declared)
            }
        };
        if let Some(Source::Export { instance, export }) = &input.revoke {
            let provider = self
                .formation
                .instances
                .iter()
                .position(|candidate| candidate.name == *instance)
                .ok_or("formation revoke provider undeclared")?;
            self.routes.push(Route {
                consumer: index,
                provider,
                admin: export.clone(),
            });
        }
        Ok((items, input.rights.unwrap_or(default)))
    }

    fn start(&mut self, index: usize) -> Result<(), &'static str> {
        let instance = self.formation.instances[index].clone();
        let bytes =
            entry(self.archive, &image_path(&instance.image)).ok_or("formation image missing")?;
        let pages = image_pages(bytes)?
            .checked_add(instance.pool.work_pages)
            .ok_or("instance page budget overflow")?;
        let pool = self.derive_pool(MemoryPoolBudget {
            pages,
            metadata: instance.pool.metadata_bytes.div_ceil(METADATA_SLOT_BYTES),
        })?;
        let pool_handle = pool.as_handle();
        self.records[index].pool = Some(pool);
        let job = match &instance.job {
            Some(name) => self.named_job(name)?,
            None => {
                let handle = process::create_job(self.services, pool_handle, JOB_RIGHTS)
                    .map_err(|_| "instance Job creation failed")?;
                self.records[index].job = Some(owned(handle));
                handle
            }
        };
        let report = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
            .map_err(|_| "instance report mailbox failed")?;
        let sender = report
            .mint(0, instance::REPORT_RIGHTS)
            .map_err(|_| "instance report sender failed")?
            .sender;
        let stop = notification::create(instance::STOP_RIGHTS, Rights::SIGNAL)
            .map_err(|_| "instance stop notification failed")?;
        // 生命周期输入直接移交；其余输入以带 GRANT 的别名移交，出生时衰减为声明权利。
        let mut owners: Vec<(String, Capability, Rights)> = Vec::new();
        owners.push((
            instance::REPORT.into(),
            sender.into_capability(),
            instance::REPORT_RIGHTS,
        ));
        owners.push((
            instance::STOP.into(),
            stop.owner.into_capability(),
            instance::STOP_RIGHTS,
        ));
        for input in &instance.inputs {
            let (items, rights) = self.resolve(index, input)?;
            for (name, handle) in items {
                let alias = object::duplicate(handle, rights | Rights::GRANT)
                    .map_err(|_| "instance input alias failed")?;
                owners.push((name, owned(alias), rights));
            }
        }
        if owners.len() > erhino_shared::proc::PROCESS_MAX_GRANTS {
            return Err("instance inputs exceed startup grants");
        }
        let grants: Vec<HandleGrant> = owners
            .iter()
            .map(|(_, capability, rights)| HandleGrant {
                handle: capability.as_handle(),
                rights: *rights,
            })
            .collect();
        let table: Vec<(&str, u8)> = owners
            .iter()
            .enumerate()
            .map(|(slot, (name, _, _))| (name.as_str(), slot as u8))
            .collect();
        let payload = libprocess::birth::encode(&table, &instance.settings)
            .map_err(|_| "instance birth frame invalid")?;
        let request = SpawnRequest {
            job,
            parent: self.parent,
            memory_pool: pool_handle,
            image: bytes,
            payload: &payload,
            grants: &grants,
            control_rights: SUPERVISOR_RIGHTS,
        };
        let mut preparation =
            Preparation::new(request).map_err(|_| "instance preparation failed")?;
        let prepared = loop {
            self.pump(Deadline::at(0))?;
            match preparation.advance() {
                Ok(Some(prepared)) => break prepared,
                Ok(None) => {}
                Err(error) => {
                    debug!(
                        "instance preparation failed: {}, {:?}",
                        instance.name, error
                    );
                    if preparation.grants() == GrantOutcome::Consumed {
                        owners.into_iter().for_each(|(_, capability, _)| {
                            capability.into_raw();
                        });
                    }
                    return Err("instance preparation failed");
                }
            }
        };
        owners.into_iter().for_each(|(_, capability, _)| {
            capability.into_raw();
        });
        // Start 前登记 DONE 与报告观察，出生后的任何退出都不会被遗漏。
        let cookie = 2 * index as u64;
        let done = self
            .set
            .register(MemberRequest::new(
                prepared.control(),
                ObjectSignals::DONE,
                cookie,
            ))
            .map_err(|_| "instance DONE observation failed")?;
        let reports = report
            .register(&self.set, cookie + 1)
            .map_err(|_| "instance report observation failed")?;
        let mut prepared = prepared;
        let spawned = prepared.try_start().map_err(|_| "instance Start failed")?;
        let record = &mut self.records[index];
        record.pid = spawned.pid;
        record.control = Some(owned(spawned.control));
        record.report = Some(report);
        record.stop = Some(stop.peer);
        record.tokens = [Some(done), Some(reports)];
        debug!(
            "formation instance started: {}, pid={}",
            instance.name, spawned.pid
        );
        if instance.kind == Kind::Resident {
            let deadline = timeout(instance.start_timeout_ms)?;
            while !self.records[index].ready {
                if expired(deadline)? {
                    return Err("resident instance start deadline expired");
                }
                self.pump(deadline)?;
            }
            debug!("formation instance ready: {}", instance.name);
        }
        Ok(())
    }

    /// 等待并处理一批观察；`deadline` 为 at(0) 时只取已就绪的记录。
    fn pump(&mut self, deadline: Deadline) -> Result<(), &'static str> {
        self.set
            .wait(deadline)
            .map_err(|_| "formation wait failed")?;
        let ready = match self.set.receive(2 * self.records.len().max(1)) {
            Ok(ready) => ready,
            Err(SystemCallError::ObjectNotAvailable) => return Ok(()),
            Err(_) => return Err("formation receive failed"),
        };
        for record in ready {
            let index = (record.cookie / 2) as usize;
            if record.cookie % 2 == 0 {
                self.done(index)?;
            } else {
                self.reports(index, record.observed)?;
            }
        }
        Ok(())
    }

    fn forget(&mut self, index: usize, slot: usize) {
        if let Some(token) = self.records[index].tokens[slot].take() {
            let _ = self.set.remove(token);
        }
    }

    fn done(&mut self, index: usize) -> Result<(), &'static str> {
        let record = &self.records[index];
        if record.done {
            return Ok(());
        }
        let control = record.control.as_ref().ok_or("instance Control missing")?;
        let snapshot = process::query(control.as_handle()).map_err(|_| "instance query failed")?;
        if snapshot.state != ProcessState::Dead as u32 {
            return Ok(());
        }
        let stopping = record.stopping;
        self.records[index].done = true;
        self.forget(index, 0);
        let normal = snapshot.cause == TerminationCause::Completed as u32
            && snapshot.detail == TerminationDetail::NONE;
        debug!(
            "formation instance done: {}, cause={}, normal={}",
            self.name(index),
            snapshot.cause,
            normal
        );
        match self.formation.instances[index].kind {
            Kind::Resident if !(normal && stopping) => {
                Err("resident instance exited outside normal stop")
            }
            Kind::Task if !normal => Err("task instance did not complete normally"),
            _ => Ok(()),
        }
    }

    fn reports(&mut self, index: usize, observed: ObjectSignals) -> Result<(), &'static str> {
        loop {
            let record = &mut self.records[index];
            let Some(mailbox) = record.report.as_ref() else {
                return Ok(());
            };
            let mut message = match mailbox.receive() {
                Ok(message) => message,
                Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                    if observed.contains(ObjectSignals::CLOSED) {
                        self.forget(index, 1);
                    }
                    return Ok(());
                }
                Err(_) => return Err("instance report receive failed"),
            };
            if message.header.sender_pid != record.pid {
                return Err("instance report from another process");
            }
            let handles = message.handles.remaining();
            let report = instance::decode_report(message.header.kind, &message.payload, handles)
                .map_err(|_| "instance report invalid")?;
            let declared = &self.formation.instances[index].exports;
            match report {
                Report::Export { name } => {
                    let name = String::from(name);
                    let export = declared
                        .iter()
                        .find(|export| export.name == name)
                        .ok_or("instance exported an undeclared name")?;
                    if record.ready || record.exports.iter().any(|(seen, _)| *seen == name) {
                        return Err("instance export repeated or late");
                    }
                    let capability = message
                        .handles
                        .take(0)
                        .map_err(|_| "instance export missing")?;
                    let description = capability
                        .description()
                        .map_err(|_| "instance export query failed")?;
                    if description.kind != export.kind
                        || !description.rights.contains(export.rights)
                    {
                        return Err("instance export kind or rights below declaration");
                    }
                    record.exports.push((name, capability));
                }
                Report::Ready { exports, .. } => {
                    if record.ready
                        || exports as usize != record.exports.len()
                        || record.exports.len() != declared.len()
                    {
                        return Err("instance Ready inconsistent with its exports");
                    }
                    record.ready = true;
                }
                Report::Stopped => {
                    if !record.stopping {
                        return Err("instance reported Stopped without a stop request");
                    }
                    record.stopped = true;
                }
            }
        }
    }

    /// 按拓扑序启动全部实例；返回时 resident 均已 Ready。
    pub(super) fn start_all(&mut self) -> Result<(), &'static str> {
        for (name, budget) in self
            .formation
            .pools
            .iter()
            .map(|pool| (pool.name.clone(), pool.budget))
            .collect::<Vec<_>>()
        {
            let pool = self.derive_pool(MemoryPoolBudget {
                pages: budget.work_pages,
                metadata: budget.metadata_bytes.div_ceil(METADATA_SLOT_BYTES),
            })?;
            self.pools.push((name, pool));
        }
        let order = self
            .formation
            .order()
            .map_err(|_| "formation order invalid")?;
        for index in order {
            let after = self.formation.instances[index].after.clone();
            for name in after {
                let task = self
                    .formation
                    .instances
                    .iter()
                    .position(|instance| instance.name == name && instance.kind == Kind::Task);
                if let Some(task) = task {
                    while !self.records[task].done {
                        self.pump(Deadline::INFINITE)?;
                    }
                }
            }
            self.start(index)?;
            self.started.push(index);
        }
        debug!("formation started: instances={}", self.started.len());
        Ok(())
    }

    /// 常驻部署持续监督，不返回；有限部署等全部 task 完成后返回。
    pub(super) fn supervise(&mut self) -> Result<(), &'static str> {
        if self.formation.finish == Finish::Stay {
            loop {
                self.pump(Deadline::INFINITE)?;
            }
        }
        while self
            .formation
            .instances
            .iter()
            .zip(&self.records)
            .any(|(instance, record)| instance.kind == Kind::Task && !record.done)
        {
            self.pump(Deadline::INFINITE)?;
        }
        debug!("formation tasks complete");
        Ok(())
    }

    fn revoke(&mut self, route: usize) -> Result<(), &'static str> {
        let Route {
            provider,
            ref admin,
            ..
        } = self.routes[route];
        let deadline = timeout(self.formation.instances[provider].stop_timeout_ms)?;
        let (_, capability) = self.records[provider]
            .exports
            .iter()
            .find(|(name, _)| name == admin)
            .ok_or("revoke administration export missing")?;
        let alias = object::duplicate(capability.as_handle(), CALL_RIGHTS)
            .map_err(|_| "revoke administration alias failed")?;
        let sender = MailboxSender::from_capability(owned(alias))
            .map_err(|_| "revoke administration role invalid")?
            .0;
        let pid = self.records[provider].pid;
        let mut caller = librpc::Caller::new();
        let mut call = |op: u16| -> Result<revoke::Reply, &'static str> {
            let body = revoke::encode_request(op, deadline).ok_or("revoke request invalid")?;
            let reply = caller
                .call(
                    &sender,
                    deadline,
                    librpc::Request::new(revoke::PROTOCOL, &body)
                        .map_err(|_| "revoke request failed")?,
                )
                .map_err(|_| "revoke call failed")?;
            if reply.sender_pid != pid {
                return Err("revoke reply from another process");
            }
            revoke::decode_reply(op, &reply.payload).ok_or("revoke reply invalid")
        };
        if call(revoke::REVOKE)? != revoke::Reply::Revoked {
            return Err("revoke denied");
        }
        loop {
            match call(revoke::QUERY)? {
                revoke::Reply::State(revoke::State::Closed) => return Ok(()),
                revoke::Reply::State(_) => {}
                _ => return Err("revoke query denied"),
            }
            if expired(deadline)? {
                return Err("revoke did not reach Closed before the stop deadline");
            }
            libkernel::time::sleep_until(timeout(REVOKE_POLL_MS)?)
                .map_err(|_| "revoke poll sleep failed")?;
        }
    }

    /// 反启动序停止：每个 resident 先丢弃 init 持有的导出、再请求停止，
    /// Stopped 与正常 DONE 汇齐后撤销其收到的可撤销路由，提供方才继续停止。
    pub(super) fn stop_all(&mut self) -> Result<(), &'static str> {
        let started = core::mem::take(&mut self.started);
        for &index in started.iter().rev() {
            if self.formation.instances[index].kind == Kind::Resident && !self.records[index].done {
                self.records[index].exports.clear();
                self.records[index].stopping = true;
                self.records[index]
                    .stop
                    .as_ref()
                    .ok_or("instance stop signaler missing")?
                    .signal(1)
                    .map_err(|_| "instance stop signal failed")?;
                let deadline = timeout(self.formation.instances[index].stop_timeout_ms)?;
                while !(self.records[index].stopped && self.records[index].done) {
                    if expired(deadline)? {
                        return Err("resident instance stop deadline expired");
                    }
                    self.pump(deadline)?;
                }
                debug!("formation instance stopped: {}", self.name(index));
            }
            let routes: Vec<usize> = (0..self.routes.len())
                .filter(|&route| self.routes[route].consumer == index)
                .collect();
            for route in routes {
                self.revoke(route)?;
            }
            self.forget(index, 0);
            self.forget(index, 1);
            let record = &mut self.records[index];
            record.exports.clear();
            record.control = None;
            record.report = None;
            record.stop = None;
        }
        self.jobs.clear();
        for record in &mut self.records {
            record.job = None;
        }
        debug!("formation stopped: all instances collected");
        Ok(())
    }
}
