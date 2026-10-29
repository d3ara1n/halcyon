//! 组态：一份完整初始系统组成的数据模型、二进制编码、静态校验与启动顺序。
//!
//! 组态由 host 侧编译器从组态源生成，init 读取 initfs 根的 `formation` 执行；二者调用同一
//! [`Formation::validate`]，使组态错误在任何副作用之前被拒绝。本 crate 只描述组成，不认识任何
//! 服务的协议、导出业务语义或设定键。名字语法与保留名见组态计划「命名」一节。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::fmt;
use erhino_shared::object::{ObjectKind, Rights};

/// 组态文件魔数与版本。
pub const MAGIC: [u8; 8] = *b"ERFORMTN";
pub const VERSION: u32 = 1;
/// 名字总长上限（字节）。
pub const MAX_NAME: usize = 32;
pub const MAX_INSTANCES: usize = 64;
pub const MAX_JOBS: usize = 32;
pub const MAX_POOLS: usize = 32;
pub const MAX_EXPORTS: usize = 32;
/// 每实例的组态输入上限：出生 grant 上限扣除两个生命周期保留输入。
pub const MAX_INPUTS: usize = erhino_shared::proc::PROCESS_MAX_GRANTS - LIFECYCLE_INPUTS.len();
/// init 为每个实例提供的生命周期保留输入；组态不得声明同名输入或导出。
pub const LIFECYCLE_INPUTS: [&str; 2] = ["instance.report", "instance.stop"];
/// 保留实例名：`init` 指 init 内建来源，`self` 指引用方实例自身。
pub const RESERVED_INSTANCES: [&str; 2] = ["init", "self"];
/// init 内建、所有实例 Job 默认所在的 Job。
pub const SERVICES_JOB: &str = "services";

/// 全部 task 正常完成后的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Finish {
    Stay = 0,
    Shutdown = 1,
    Reboot = 2,
}

/// 实例种类：常驻须 Ready 并经正常停止；任务以 Completed 结束即成功。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Resident = 0,
    Task = 1,
}

/// 双维额度：页与 metadata 字节。实例映像页由 init 按 ELF load runs 另加。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    pub work_pages: u64,
    pub metadata_bytes: u64,
}

/// init 从根 Pool 派生并持有至系统结束的具名 Pool。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub name: String,
    pub budget: Budget,
}

/// init 按组态建立的具名 Job。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    /// 父 Job：`services` 或另一具名 job。
    pub parent: String,
    /// 付款者：具名 Pool 名或实例名（取该实例的 Pool）。
    pub payer: String,
}

/// 路由来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    PlatformDescription,
    /// 平台根列表，整表路由。
    PlatformRoot,
    /// initfs `image/<组件名>`，封存为只读 MemoryObject。
    Image(String),
    Job(String),
    /// 具名 Pool 的 CREATE 别名。
    Pool(String),
    /// 本实例 Pool 的 CREATE 别名。
    SelfPool,
    /// 另一实例的导出。
    Export {
        instance: String,
        export: String,
    },
}

impl Source {
    /// 解析组态源中的来源字符串：`init/…`、`self/memory.pool` 或 `<实例>/<导出>`。
    pub fn parse(text: &str) -> Option<Self> {
        let (instance, name) = text.split_once('/')?;
        Some(match instance {
            "init" => match name {
                "platform.description" => Self::PlatformDescription,
                "platform.root" => Self::PlatformRoot,
                _ => {
                    let (kind, item) = name.split_once('.')?;
                    if !segment(item.as_bytes()) {
                        return None;
                    }
                    match kind {
                        "image" if component(item) => Self::Image(item.into()),
                        "job" => Self::Job(item.into()),
                        "pool" => Self::Pool(item.into()),
                        _ => return None,
                    }
                }
            },
            "self" if name == "memory.pool" => Self::SelfPool,
            _ if !RESERVED_INSTANCES.contains(&instance)
                && valid_name(instance)
                && valid_name(name) =>
            {
                Self::Export {
                    instance: instance.into(),
                    export: name.into(),
                }
            }
            _ => return None,
        })
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformDescription => f.write_str("init/platform.description"),
            Self::PlatformRoot => f.write_str("init/platform.root"),
            Self::Image(name) => write!(f, "init/image.{name}"),
            Self::Job(name) => write!(f, "init/job.{name}"),
            Self::Pool(name) => write!(f, "init/pool.{name}"),
            Self::SelfPool => f.write_str("self/memory.pool"),
            Self::Export { instance, export } => write!(f, "{instance}/{export}"),
        }
    }
}

/// 一条路由：把来源接到本实例的某输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub source: Source,
    /// 衰减后的权利；缺省时沿用来源权利。
    pub rights: Option<Rights>,
    /// 撤销管理端，必须是同一提供方 `<导出>.admin`。
    pub revoke: Option<Source>,
}

/// 实例交出的命名能力及其最小要求。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    /// `ObjectKind` 的 ABI 判别值。
    pub kind: u32,
    pub rights: Rights,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub name: String,
    /// initfs `image/<组件名>` 的组件名。
    pub image: String,
    pub kind: Kind,
    /// 具名 job；缺省时 init 在 `services` 下为该实例新建 Job。
    pub job: Option<String>,
    pub pool: Budget,
    pub inputs: Vec<Input>,
    pub exports: Vec<Export>,
    /// `settings` 编码的实例设定。
    pub settings: Vec<u8>,
    /// 0 表示无期限；resident 两者均须为正。
    pub start_timeout_ms: u64,
    pub stop_timeout_ms: u64,
    /// 无数据依赖时的顺序约束。
    pub after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formation {
    pub finish: Finish,
    pub pools: Vec<Pool>,
    pub jobs: Vec<Job>,
    pub instances: Vec<Instance>,
}

/// 组态错误：格式错误或静态校验失败；`subject` 指出出错的名字或来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Format(&'static str),
    Invalid {
        reason: &'static str,
        subject: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(reason) => write!(f, "formation format error: {reason}"),
            Self::Invalid { reason, subject } => write!(f, "{reason}: {subject}"),
        }
    }
}

fn invalid(reason: &'static str, subject: impl fmt::Display) -> Error {
    Error::Invalid {
        reason,
        subject: alloc::format!("{subject}"),
    }
}

// —— 名字语法 ——

fn segment(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn index(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.iter().all(u8::is_ascii_digit)
        && (bytes.len() == 1 || bytes[0] != b'0')
}

/// 实例、输入、导出与 job 名：`.` 连接的小写段，末段可为十进制序号，总长不超过 32 字节。
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_NAME {
        return false;
    }
    let mut segments = name.split('.').peekable();
    let mut first = true;
    while let Some(part) = segments.next() {
        let last = segments.peek().is_none();
        let ok = segment(part.as_bytes()) || (!first && last && index(part.as_bytes()));
        if !ok {
            return false;
        }
        first = false;
    }
    true
}

/// 组件名：单段，与 Rust 组件身份一致。
fn component(name: &str) -> bool {
    name.len() <= MAX_NAME && segment(name.as_bytes())
}

// —— 对象类型名 ——

const KINDS: [(&str, ObjectKind); 27] = [
    ("job", ObjectKind::Job),
    ("memory_pool", ObjectKind::MemoryPool),
    ("memory_object", ObjectKind::MemoryObject),
    ("process_builder", ObjectKind::ProcessBuilder),
    ("process_control", ObjectKind::ProcessControl),
    ("thread_control", ObjectKind::ThreadControl),
    ("mailbox_receiver", ObjectKind::MailboxReceiver),
    ("notification_owner", ObjectKind::NotificationOwner),
    ("tunnel_endpoint", ObjectKind::TunnelEndpoint),
    ("tunnel_invitation", ObjectKind::TunnelInvitation),
    ("system_reset", ObjectKind::SystemReset),
    ("mailbox_sender", ObjectKind::MailboxSender),
    ("lifetime_observer", ObjectKind::LifetimeObserver),
    ("delivery", ObjectKind::Delivery),
    ("wait_set", ObjectKind::WaitSet),
    ("mailbox_once_sender", ObjectKind::MailboxOnceSender),
    ("notification_signaler", ObjectKind::NotificationSignaler),
    ("reply_observer", ObjectKind::ReplyObserver),
    ("thread_start_ticket", ObjectKind::ThreadStartTicket),
    ("notification_observer", ObjectKind::NotificationObserver),
    ("mmio_control", ObjectKind::MmioControl),
    ("mmio_use", ObjectKind::MmioUse),
    ("irq_control", ObjectKind::IrqControl),
    ("irq_use", ObjectKind::IrqUse),
    ("dma_domain_control", ObjectKind::DmaDomainControl),
    ("dma_use", ObjectKind::DmaUse),
    ("dma_mapping", ObjectKind::DmaMapping),
];

/// 组态源中的对象类型名到 ABI 判别值。
pub fn kind_by_name(name: &str) -> Option<u32> {
    KINDS
        .iter()
        .find(|(text, _)| *text == name)
        .map(|(_, kind)| *kind as u32)
}

pub fn kind_name(kind: u32) -> Option<&'static str> {
    KINDS
        .iter()
        .find(|(_, value)| *value as u32 == kind)
        .map(|(text, _)| *text)
}

const RIGHTS: [(&str, Rights); 10] = [
    ("read", Rights::READ),
    ("write", Rights::WRITE),
    ("wait", Rights::WAIT),
    ("signal", Rights::SIGNAL),
    ("transit", Rights::TRANSIT),
    ("duplicate", Rights::DUPLICATE),
    ("manage", Rights::MANAGE),
    ("grant", Rights::GRANT),
    ("create", Rights::CREATE),
    ("execute", Rights::EXECUTE),
];

/// 组态源中的权利名到权利位。
pub fn right_by_name(name: &str) -> Option<Rights> {
    RIGHTS
        .iter()
        .find(|(text, _)| *text == name)
        .map(|(_, right)| *right)
}

// —— 编码 ——

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn len(&mut self, len: usize) -> Result<(), Error> {
        self.u32(u32::try_from(len).map_err(|_| Error::Format("length overflow"))?);
        Ok(())
    }
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.len(bytes.len())?;
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn str(&mut self, text: &str) -> Result<(), Error> {
        self.bytes(text.as_bytes())
    }
    fn budget(&mut self, budget: Budget) {
        self.u64(budget.work_pages);
        self.u64(budget.metadata_bytes);
    }
    fn source(&mut self, source: &Source) -> Result<(), Error> {
        match source {
            Source::PlatformDescription => self.u8(0),
            Source::PlatformRoot => self.u8(1),
            Source::Image(name) => {
                self.u8(2);
                self.str(name)?;
            }
            Source::Job(name) => {
                self.u8(3);
                self.str(name)?;
            }
            Source::Pool(name) => {
                self.u8(4);
                self.str(name)?;
            }
            Source::SelfPool => self.u8(5),
            Source::Export { instance, export } => {
                self.u8(6);
                self.str(instance)?;
                self.str(export)?;
            }
        }
        Ok(())
    }
    fn optional_rights(&mut self, rights: Option<Rights>) {
        match rights {
            Some(rights) => {
                self.u8(1);
                self.u64(rights.raw());
            }
            None => self.u8(0),
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(Error::Format("truncated"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error::Format("truncated"))?;
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("fixed width"),
        ))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("fixed width"),
        ))
    }
    /// 计数字段；以剩余字节数为上界，防止伪造计数驱动超量分配。
    fn count(&mut self, limit: usize) -> Result<usize, Error> {
        let count = self.u32()? as usize;
        if count > limit || count > self.bytes.len() - self.pos {
            return Err(Error::Format("count out of range"));
        }
        Ok(count)
    }
    fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = self.u32()? as usize;
        self.take(len)
    }
    fn string(&mut self) -> Result<String, Error> {
        let bytes = self.bytes()?;
        core::str::from_utf8(bytes)
            .map(String::from)
            .map_err(|_| Error::Format("name is not UTF-8"))
    }
    fn budget(&mut self) -> Result<Budget, Error> {
        Ok(Budget {
            work_pages: self.u64()?,
            metadata_bytes: self.u64()?,
        })
    }
    fn source(&mut self) -> Result<Source, Error> {
        Ok(match self.u8()? {
            0 => Source::PlatformDescription,
            1 => Source::PlatformRoot,
            2 => Source::Image(self.string()?),
            3 => Source::Job(self.string()?),
            4 => Source::Pool(self.string()?),
            5 => Source::SelfPool,
            6 => Source::Export {
                instance: self.string()?,
                export: self.string()?,
            },
            _ => return Err(Error::Format("unknown source tag")),
        })
    }
    fn optional_rights(&mut self) -> Result<Option<Rights>, Error> {
        Ok(match self.u8()? {
            0 => None,
            1 => Some(self.rights()?),
            _ => return Err(Error::Format("bad option tag")),
        })
    }
    fn rights(&mut self) -> Result<Rights, Error> {
        let rights = Rights::from_raw(self.u64()?);
        if !rights.is_known() {
            return Err(Error::Format("unknown rights"));
        }
        Ok(rights)
    }
}

impl Formation {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(&MAGIC);
        w.u32(VERSION);
        w.u8(self.finish as u8);
        w.len(self.pools.len())?;
        for pool in &self.pools {
            w.str(&pool.name)?;
            w.budget(pool.budget);
        }
        w.len(self.jobs.len())?;
        for job in &self.jobs {
            w.str(&job.name)?;
            w.str(&job.parent)?;
            w.str(&job.payer)?;
        }
        w.len(self.instances.len())?;
        for instance in &self.instances {
            w.str(&instance.name)?;
            w.str(&instance.image)?;
            w.u8(instance.kind as u8);
            match &instance.job {
                Some(job) => {
                    w.u8(1);
                    w.str(job)?;
                }
                None => w.u8(0),
            }
            w.budget(instance.pool);
            w.len(instance.inputs.len())?;
            for input in &instance.inputs {
                w.str(&input.name)?;
                w.source(&input.source)?;
                w.optional_rights(input.rights);
                match &input.revoke {
                    Some(revoke) => {
                        w.u8(1);
                        w.source(revoke)?;
                    }
                    None => w.u8(0),
                }
            }
            w.len(instance.exports.len())?;
            for export in &instance.exports {
                w.str(&export.name)?;
                w.u32(export.kind);
                w.u64(export.rights.raw());
            }
            w.bytes(&instance.settings)?;
            w.u64(instance.start_timeout_ms);
            w.u64(instance.stop_timeout_ms);
            w.len(instance.after.len())?;
            for after in &instance.after {
                w.str(after)?;
            }
        }
        Ok(w.0)
    }

    /// 解码组态文件；只核格式，组成的合法性由 [`Formation::validate`] 判定。
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(MAGIC.len())? != MAGIC {
            return Err(Error::Format("bad magic"));
        }
        if r.u32()? != VERSION {
            return Err(Error::Format("unsupported version"));
        }
        let finish = match r.u8()? {
            0 => Finish::Stay,
            1 => Finish::Shutdown,
            2 => Finish::Reboot,
            _ => return Err(Error::Format("unknown finish")),
        };
        let mut pools = Vec::new();
        for _ in 0..r.count(MAX_POOLS)? {
            pools.push(Pool {
                name: r.string()?,
                budget: r.budget()?,
            });
        }
        let mut jobs = Vec::new();
        for _ in 0..r.count(MAX_JOBS)? {
            jobs.push(Job {
                name: r.string()?,
                parent: r.string()?,
                payer: r.string()?,
            });
        }
        let mut instances = Vec::new();
        for _ in 0..r.count(MAX_INSTANCES)? {
            let name = r.string()?;
            let image = r.string()?;
            let kind = match r.u8()? {
                0 => Kind::Resident,
                1 => Kind::Task,
                _ => return Err(Error::Format("unknown instance kind")),
            };
            let job = match r.u8()? {
                0 => None,
                1 => Some(r.string()?),
                _ => return Err(Error::Format("bad option tag")),
            };
            let pool = r.budget()?;
            let mut inputs = Vec::new();
            for _ in 0..r.count(MAX_INPUTS)? {
                let name = r.string()?;
                let source = r.source()?;
                let rights = r.optional_rights()?;
                let revoke = match r.u8()? {
                    0 => None,
                    1 => Some(r.source()?),
                    _ => return Err(Error::Format("bad option tag")),
                };
                inputs.push(Input {
                    name,
                    source,
                    rights,
                    revoke,
                });
            }
            let mut exports = Vec::new();
            for _ in 0..r.count(MAX_EXPORTS)? {
                exports.push(Export {
                    name: r.string()?,
                    kind: r.u32()?,
                    rights: r.rights()?,
                });
            }
            let settings = r.bytes()?.to_vec();
            let start_timeout_ms = r.u64()?;
            let stop_timeout_ms = r.u64()?;
            let mut after = Vec::new();
            for _ in 0..r.count(MAX_INSTANCES)? {
                after.push(r.string()?);
            }
            instances.push(Instance {
                name,
                image,
                kind,
                job,
                pool,
                inputs,
                exports,
                settings,
                start_timeout_ms,
                stop_timeout_ms,
                after,
            });
        }
        if r.pos != bytes.len() {
            return Err(Error::Format("trailing bytes"));
        }
        Ok(Self {
            finish,
            pools,
            jobs,
            instances,
        })
    }
}

// —— 静态校验与顺序 ——

impl Formation {
    pub fn instance(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|instance| instance.name == name)
    }

    pub fn pool(&self, name: &str) -> Option<&Pool> {
        self.pools.iter().find(|pool| pool.name == name)
    }

    pub fn job(&self, name: &str) -> Option<&Job> {
        self.jobs.iter().find(|job| job.name == name)
    }

    /// 全部具名 Pool 与实例工作集的合计；init 另加实例映像页后与根 Pool 比较。
    pub fn total_budget(&self) -> Result<Budget, Error> {
        let mut total = Budget::default();
        for budget in self
            .pools
            .iter()
            .map(|pool| pool.budget)
            .chain(self.instances.iter().map(|instance| instance.pool))
        {
            total.work_pages = total
                .work_pages
                .checked_add(budget.work_pages)
                .ok_or_else(|| invalid("budget sum overflows", "work_pages"))?;
            total.metadata_bytes = total
                .metadata_bytes
                .checked_add(budget.metadata_bytes)
                .ok_or_else(|| invalid("budget sum overflows", "metadata_bytes"))?;
        }
        Ok(total)
    }

    /// 实例 `index` 必须在其后启动的前驱（按声明序下标）。
    fn predecessors(&self, index: usize) -> Vec<usize> {
        let instance = &self.instances[index];
        let mut names: Vec<&str> = Vec::new();
        for input in &instance.inputs {
            for source in core::iter::once(&input.source).chain(input.revoke.as_ref()) {
                if let Source::Export { instance, .. } = source {
                    names.push(instance);
                }
            }
        }
        names.extend(instance.after.iter().map(String::as_str));
        if let Some(job) = instance.job.as_deref().and_then(|job| self.job(job))
            && job.payer != instance.name
            && self.instance(&job.payer).is_some()
        {
            names.push(&job.payer);
        }
        let mut result: Vec<usize> = names
            .into_iter()
            .filter_map(|name| self.instances.iter().position(|other| other.name == name))
            .collect();
        result.sort_unstable();
        result.dedup();
        result
    }

    /// 启动顺序：依赖拓扑序，同层按声明序；停止取其反序。有环时报错。
    pub fn order(&self) -> Result<Vec<usize>, Error> {
        let count = self.instances.len();
        let predecessors: Vec<Vec<usize>> = (0..count).map(|i| self.predecessors(i)).collect();
        let mut placed = alloc::vec![false; count];
        let mut order = Vec::with_capacity(count);
        while order.len() < count {
            let next = (0..count)
                .find(|&i| !placed[i] && predecessors[i].iter().all(|&p| placed[p]))
                .ok_or_else(|| {
                    let stuck = (0..count).find(|&i| !placed[i]).expect("unplaced remains");
                    invalid("instance dependency cycle", &self.instances[stuck].name)
                })?;
            placed[next] = true;
            order.push(next);
        }
        Ok(order)
    }

    /// 静态校验；`image_exists` 由调用方按 initfs 或 host 映像目录回答组件是否存在。
    pub fn validate(&self, image_exists: impl Fn(&str) -> bool) -> Result<(), Error> {
        if self.instances.len() > MAX_INSTANCES {
            return Err(invalid("too many instances", self.instances.len()));
        }
        if self.pools.len() > MAX_POOLS {
            return Err(invalid("too many pools", self.pools.len()));
        }
        if self.jobs.len() > MAX_JOBS {
            return Err(invalid("too many jobs", self.jobs.len()));
        }
        self.validate_names()?;
        self.validate_jobs()?;
        for instance in &self.instances {
            self.validate_instance(instance, &image_exists)?;
        }
        self.total_budget()?;
        self.order()?;
        if self.finish != Finish::Stay && !self.instances.iter().any(|i| i.kind == Kind::Task) {
            return Err(invalid(
                "finish action requires at least one task",
                "finish",
            ));
        }
        Ok(())
    }

    fn validate_names(&self) -> Result<(), Error> {
        for (index, instance) in self.instances.iter().enumerate() {
            if !valid_name(&instance.name) || RESERVED_INSTANCES.contains(&instance.name.as_str()) {
                return Err(invalid("invalid instance name", &instance.name));
            }
            if self.instances[..index]
                .iter()
                .any(|other| other.name == instance.name)
            {
                return Err(invalid("duplicate instance", &instance.name));
            }
            // job 付款者按名字解析到 Pool 或实例，两个命名空间不得重叠。
            if self.pool(&instance.name).is_some() {
                return Err(invalid("instance name collides with pool", &instance.name));
            }
        }
        for (index, pool) in self.pools.iter().enumerate() {
            if !component(&pool.name) {
                return Err(invalid("invalid pool name", &pool.name));
            }
            if self.pools[..index]
                .iter()
                .any(|other| other.name == pool.name)
            {
                return Err(invalid("duplicate pool", &pool.name));
            }
        }
        for (index, job) in self.jobs.iter().enumerate() {
            if !component(&job.name) || job.name == SERVICES_JOB {
                return Err(invalid("invalid job name", &job.name));
            }
            if self.jobs[..index]
                .iter()
                .any(|other| other.name == job.name)
            {
                return Err(invalid("duplicate job", &job.name));
            }
        }
        Ok(())
    }

    fn validate_jobs(&self) -> Result<(), Error> {
        for job in &self.jobs {
            if self.pool(&job.payer).is_none() && self.instance(&job.payer).is_none() {
                return Err(invalid("job payer is neither pool nor instance", &job.name));
            }
            // 父链必须终止于 services，不得成环。
            let mut parent = job.parent.as_str();
            for _ in 0..=self.jobs.len() {
                if parent == SERVICES_JOB {
                    break;
                }
                parent = self
                    .job(parent)
                    .ok_or_else(|| invalid("unknown parent job", &job.name))?
                    .parent
                    .as_str();
            }
            if parent != SERVICES_JOB {
                return Err(invalid("job parent cycle", &job.name));
            }
        }
        Ok(())
    }

    fn validate_instance(
        &self,
        instance: &Instance,
        image_exists: &impl Fn(&str) -> bool,
    ) -> Result<(), Error> {
        let name = &instance.name;
        if !component(&instance.image) || !image_exists(&instance.image) {
            return Err(invalid("missing image", &instance.image));
        }
        if let Some(job) = &instance.job
            && self.job(job).is_none()
        {
            return Err(invalid("unknown job", job));
        }
        if instance.kind == Kind::Resident
            && (instance.start_timeout_ms == 0 || instance.stop_timeout_ms == 0)
        {
            return Err(invalid("resident instance requires both timeouts", name));
        }
        if instance.inputs.len() > MAX_INPUTS {
            return Err(invalid("too many inputs", name));
        }
        if instance.exports.len() > MAX_EXPORTS {
            return Err(invalid("too many exports", name));
        }
        for (index, export) in instance.exports.iter().enumerate() {
            if !valid_name(&export.name) || LIFECYCLE_INPUTS.contains(&export.name.as_str()) {
                return Err(invalid("invalid export name", &export.name));
            }
            if instance.exports[..index]
                .iter()
                .any(|other| other.name == export.name)
            {
                return Err(invalid("duplicate export", &export.name));
            }
            if kind_name(export.kind).is_none() || !export.rights.is_known() {
                return Err(invalid("invalid export kind or rights", &export.name));
            }
        }
        for (index, input) in instance.inputs.iter().enumerate() {
            if !valid_name(&input.name) || LIFECYCLE_INPUTS.contains(&input.name.as_str()) {
                return Err(invalid("invalid input name", &input.name));
            }
            if instance.inputs[..index]
                .iter()
                .any(|other| other.name == input.name)
            {
                return Err(invalid("duplicate input", &input.name));
            }
            self.validate_source(instance, &input.source, input.rights)?;
            if let Some(revoke) = &input.revoke {
                let (
                    Source::Export {
                        instance: provider,
                        export,
                    },
                    Source::Export {
                        instance: admin_provider,
                        export: admin,
                    },
                ) = (&input.source, revoke)
                else {
                    return Err(invalid("revoke requires an export route", &input.name));
                };
                if provider != admin_provider || admin.strip_suffix(".admin") != Some(export) {
                    return Err(invalid(
                        "revoke must name <export>.admin of the provider",
                        revoke,
                    ));
                }
                self.validate_source(instance, revoke, None)?;
            }
        }
        for after in &instance.after {
            if after == name || self.instance(after).is_none() {
                return Err(invalid("invalid after reference", after));
            }
        }
        settings::Settings::parse(&instance.settings)
            .map_err(|_| invalid("invalid settings encoding", name))?;
        Ok(())
    }

    fn validate_source(
        &self,
        consumer: &Instance,
        source: &Source,
        rights: Option<Rights>,
    ) -> Result<(), Error> {
        match source {
            Source::PlatformDescription | Source::PlatformRoot | Source::SelfPool => {}
            Source::Image(image) => {
                if !component(image) {
                    return Err(invalid("invalid image source", source));
                }
            }
            Source::Job(job) => {
                if job != SERVICES_JOB && self.job(job).is_none() {
                    return Err(invalid("unknown job source", source));
                }
            }
            Source::Pool(pool) => {
                if self.pool(pool).is_none() {
                    return Err(invalid("unknown pool source", source));
                }
            }
            Source::Export { instance, export } => {
                if instance == &consumer.name {
                    return Err(invalid("instance routes its own export", source));
                }
                let declared = self
                    .instance(instance)
                    .ok_or_else(|| invalid("unknown provider instance", source))?
                    .exports
                    .iter()
                    .find(|candidate| &candidate.name == export)
                    .ok_or_else(|| invalid("undeclared export", source))?;
                if let Some(rights) = rights
                    && !declared.rights.contains(rights)
                {
                    return Err(invalid("route rights exceed the export", source));
                }
            }
        }
        if let Some(rights) = rights
            && !rights.is_known()
        {
            return Err(invalid("unknown route rights", source));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
