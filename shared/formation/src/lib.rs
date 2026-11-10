//! 组态 v2：静态资源、显式付款与完整初始视图。
//! 不解释服务协议，不包含业务拓扑或就绪/停止政策。
#![cfg_attr(not(test), no_std)]
extern crate alloc;
use alloc::{string::String, vec::Vec};
use core::fmt;
use erhino_shared::object::Rights;
pub const MAGIC: [u8; 8] = *b"ERFORMTN";
pub const VERSION: u32 = 2;
pub const MAX_NAME: usize = 32;
pub const MAX_INSTANCES: usize = 64;
pub const MAX_JOBS: usize = 32;
pub const MAX_POOLS: usize = 32;
pub const MAX_INPUTS: usize = erhino_shared::proc::PROCESS_MAX_GRANTS;
pub const RESERVED_INSTANCES: [&str; 2] = ["init", "self"];
pub const SERVICES_JOB: &str = "services";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    pub work_pages: u64,
    pub metadata_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub name: String,
    pub budget: Budget,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    pub parent: String,
    pub payer: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceOutput {
    Main,
    Lifetime,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Mailbox {
        rights: Rights,
    },
    Sender {
        mailbox: String,
        badge: u64,
        rights: Rights,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    pub payer: String,
    pub kind: ResourceKind,
    /// 显式声明该 sender 实现终局撤销协议，名字不产生协议知识。
    pub terminal_revoke: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    PlatformDescription,
    PlatformRoot,
    Image(String),
    Job(String),
    Pool(String),
    SelfPool,
    Resource {
        name: String,
        output: ResourceOutput,
    },
}
impl Source {
    pub fn parse(text: &str) -> Option<Self> {
        let (owner, name) = text.split_once('/')?;
        if owner == "self" && name == "memory.pool" {
            return Some(Self::SelfPool);
        }
        if owner != "init" {
            return None;
        }
        Some(match name {
            "platform.description" => Self::PlatformDescription,
            "platform.root" => Self::PlatformRoot,
            _ => {
                let (kind, item) = name.split_once('.')?;
                if !valid_name(item) {
                    return None;
                }
                match kind {
                    "image" if component(item) => Self::Image(item.into()),
                    "job" => Self::Job(item.into()),
                    "pool" => Self::Pool(item.into()),
                    "object" => Self::Resource {
                        name: item.into(),
                        output: ResourceOutput::Main,
                    },
                    "lifetime" => Self::Resource {
                        name: item.into(),
                        output: ResourceOutput::Lifetime,
                    },
                    _ => return None,
                }
            }
        })
    }
}
impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformDescription => f.write_str("init/platform.description"),
            Self::PlatformRoot => f.write_str("init/platform.root"),
            Self::Image(n) => write!(f, "init/image.{n}"),
            Self::Job(n) => write!(f, "init/job.{n}"),
            Self::Pool(n) => write!(f, "init/pool.{n}"),
            Self::SelfPool => f.write_str("self/memory.pool"),
            Self::Resource { name, output } => write!(
                f,
                "init/{}.{name}",
                match output {
                    ResourceOutput::Main => "object",
                    ResourceOutput::Lifetime => "lifetime",
                }
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub source: Source,
    pub rights: Option<Rights>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceBinding {
    pub prefix: String,
    pub source: Source,
    pub rights: Rights,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub name: String,
    pub image: String,
    pub job: Option<String>,
    pub pool: Budget,
    pub inputs: Vec<Input>,
    pub namespace: Vec<NamespaceBinding>,
    pub settings: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formation {
    pub pools: Vec<Pool>,
    pub jobs: Vec<Job>,
    pub resources: Vec<Resource>,
    pub instances: Vec<Instance>,
}
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
            Self::Format(r) => write!(f, "formation format error: {r}"),
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

/// 实例、输入、资源与 job 名：`.` 连接的小写段，末段可为十进制序号，总长不超过 32 字节。
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
            Source::Resource { name, output } => {
                self.u8(6);
                self.str(name)?;
                self.u8(match output {
                    ResourceOutput::Main => 0,
                    ResourceOutput::Lifetime => 1,
                });
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
            6 => Source::Resource {
                name: self.string()?,
                output: match self.u8()? {
                    0 => ResourceOutput::Main,
                    1 => ResourceOutput::Lifetime,
                    _ => return Err(Error::Format("unknown resource output")),
                },
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
        w.len(self.pools.len())?;
        for p in &self.pools {
            w.str(&p.name)?;
            w.budget(p.budget);
        }
        w.len(self.jobs.len())?;
        for j in &self.jobs {
            w.str(&j.name)?;
            w.str(&j.parent)?;
            w.str(&j.payer)?;
        }
        w.len(self.resources.len())?;
        for r in &self.resources {
            w.str(&r.name)?;
            w.str(&r.payer)?;
            w.u8(u8::from(r.terminal_revoke));
            match &r.kind {
                ResourceKind::Mailbox { rights } => {
                    w.u8(0);
                    w.u64(rights.raw());
                }
                ResourceKind::Sender {
                    mailbox,
                    badge,
                    rights,
                } => {
                    w.u8(1);
                    w.str(mailbox)?;
                    w.u64(*badge);
                    w.u64(rights.raw());
                }
            }
        }
        w.len(self.instances.len())?;
        for i in &self.instances {
            w.str(&i.name)?;
            w.str(&i.image)?;
            match &i.job {
                None => w.u8(0),
                Some(j) => {
                    w.u8(1);
                    w.str(j)?;
                }
            }
            w.budget(i.pool);
            w.len(i.inputs.len())?;
            for input in &i.inputs {
                w.str(&input.name)?;
                w.source(&input.source)?;
                w.optional_rights(input.rights);
            }
            w.len(i.namespace.len())?;
            for b in &i.namespace {
                w.str(&b.prefix)?;
                w.source(&b.source)?;
                w.u64(b.rights.raw());
            }
            w.bytes(&i.settings)?;
        }
        Ok(w.0)
    }
    /// 仅解码格式；发布前必须校验语义及实际平台展开后的授权数。
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(8)? != MAGIC {
            return Err(Error::Format("bad magic"));
        }
        if r.u32()? != VERSION {
            return Err(Error::Format("unsupported version"));
        }
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
        let mut resources = Vec::new();
        for _ in 0..r.count(usize::MAX)? {
            let name = r.string()?;
            let payer = r.string()?;
            let terminal_revoke = match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(Error::Format("bad boolean")),
            };
            let kind = match r.u8()? {
                0 => ResourceKind::Mailbox {
                    rights: r.rights()?,
                },
                1 => ResourceKind::Sender {
                    mailbox: r.string()?,
                    badge: r.u64()?,
                    rights: r.rights()?,
                },
                _ => return Err(Error::Format("unknown resource kind")),
            };
            resources.push(Resource {
                name,
                payer,
                kind,
                terminal_revoke,
            });
        }
        let mut instances = Vec::new();
        for _ in 0..r.count(MAX_INSTANCES)? {
            let name = r.string()?;
            let image = r.string()?;
            let job = match r.u8()? {
                0 => None,
                1 => Some(r.string()?),
                _ => return Err(Error::Format("bad option tag")),
            };
            let pool = r.budget()?;
            let mut inputs = Vec::new();
            for _ in 0..r.count(MAX_INPUTS)? {
                inputs.push(Input {
                    name: r.string()?,
                    source: r.source()?,
                    rights: r.optional_rights()?,
                });
            }
            let mut namespace = Vec::new();
            for _ in 0..r.count(MAX_INPUTS)? {
                namespace.push(NamespaceBinding {
                    prefix: r.string()?,
                    source: r.source()?,
                    rights: r.rights()?,
                });
            }
            let settings = r.bytes()?.to_vec();
            instances.push(Instance {
                name,
                image,
                job,
                pool,
                inputs,
                namespace,
                settings,
            });
        }
        if r.pos != bytes.len() {
            return Err(Error::Format("trailing bytes"));
        }
        Ok(Self {
            pools,
            jobs,
            resources,
            instances,
        })
    }
    pub fn instance(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }
    pub fn pool(&self, name: &str) -> Option<&Pool> {
        self.pools.iter().find(|p| p.name == name)
    }
    pub fn job(&self, name: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.name == name)
    }
    pub fn resource(&self, name: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.name == name)
    }
    pub fn total_budget(&self) -> Result<Budget, Error> {
        let mut total = Budget::default();
        for budget in self
            .pools
            .iter()
            .map(|p| p.budget)
            .chain(self.instances.iter().map(|i| i.pool))
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
    fn payer(&self, name: &str) -> Result<(), Error> {
        if name == "init" || self.pool(name).is_some() || self.instance(name).is_some() {
            Ok(())
        } else {
            Err(invalid("unknown payer", name))
        }
    }
    /// 默认权利的唯一真值；施工母本所需的临时 GRANT/DUPLICATE 不混入资源声明。
    pub fn source_rights(&self, source: &Source) -> Result<Rights, Error> {
        let rights = match source {
            Source::PlatformDescription | Source::Image(_) => {
                Rights::READ | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT
            }
            Source::PlatformRoot => {
                Rights::READ
                    | Rights::WRITE
                    | Rights::WAIT
                    | Rights::MANAGE
                    | Rights::CREATE
                    | Rights::DUPLICATE
                    | Rights::TRANSIT
                    | Rights::GRANT
            }
            Source::Job(_) => {
                Rights::CREATE
                    | Rights::MANAGE
                    | Rights::READ
                    | Rights::WAIT
                    | Rights::DUPLICATE
                    | Rights::TRANSIT
                    | Rights::GRANT
            }
            Source::Pool(_) | Source::SelfPool => {
                Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT
            }
            Source::Resource { name, output } => {
                let r = self
                    .resource(name)
                    .ok_or_else(|| invalid("unknown resource", name))?;
                match (&r.kind, output) {
                    (ResourceKind::Mailbox { rights }, ResourceOutput::Main)
                    | (ResourceKind::Sender { rights, .. }, ResourceOutput::Main) => *rights,
                    (ResourceKind::Sender { .. }, ResourceOutput::Lifetime) => Rights::WAIT,
                    _ => return Err(invalid("mailbox has no lifetime output", name)),
                }
            }
        };
        Ok(rights)
    }
    fn validate_source(
        &self,
        source: &Source,
        rights: Option<Rights>,
        image_exists: &impl Fn(&str) -> bool,
    ) -> Result<(), Error> {
        match source {
            Source::Image(name) if !component(name) || !image_exists(name) => {
                return Err(invalid("missing image", name));
            }
            Source::Job(name) if name != SERVICES_JOB && self.job(name).is_none() => {
                return Err(invalid("unknown job source", name));
            }
            Source::Pool(name) if self.pool(name).is_none() => {
                return Err(invalid("unknown pool source", name));
            }
            _ => {}
        }
        let available = self.source_rights(source)?;
        if let Some(rights) = rights
            && (!rights.is_known() || !available.contains(rights))
        {
            return Err(invalid("route rights exceed source", source));
        }
        Ok(())
    }
    /// 实际平台根数由 init 在任何对象工厂前传入；host 静态校验只使用零展开下界。
    pub fn validate_grants(&self, platform_roots: usize) -> Result<(), Error> {
        for i in &self.instances {
            let mut count = i.namespace.len();
            for input in &i.inputs {
                count = count
                    .checked_add(if input.source == Source::PlatformRoot {
                        platform_roots
                    } else {
                        1
                    })
                    .ok_or_else(|| invalid("grant count overflows", &i.name))?;
            }
            if count > MAX_INPUTS {
                return Err(invalid("too many expanded grants", &i.name));
            }
            // 数量先通过 ABI 上界，再展开名字，避免伪造平台数驱动无界分配。
            let mut names = Vec::with_capacity(count);
            for input in &i.inputs {
                if input.source == Source::PlatformRoot {
                    for n in 0..platform_roots {
                        let name = alloc::format!("{}.{n}", input.name);
                        if name.len() > MAX_NAME {
                            return Err(invalid("expanded input name too long", &name));
                        }
                        names.push(name);
                    }
                } else {
                    names.push(input.name.clone());
                }
            }
            for n in 0..i.namespace.len() {
                names.push(oal::namespace::input_name(n));
            }
            names.sort_unstable();
            if names.windows(2).any(|p| p[0] == p[1]) {
                return Err(invalid("duplicate expanded input name", &i.name));
            }
        }
        Ok(())
    }
    pub fn validate(&self, image_exists: impl Fn(&str) -> bool) -> Result<(), Error> {
        if self.instances.len() > MAX_INSTANCES
            || self.pools.len() > MAX_POOLS
            || self.jobs.len() > MAX_JOBS
        {
            return Err(invalid("too many declarations", "formation"));
        }
        for (n, i) in self.instances.iter().enumerate() {
            if !valid_name(&i.name) || RESERVED_INSTANCES.contains(&i.name.as_str()) {
                return Err(invalid("invalid instance name", &i.name));
            }
            if self.instances[..n].iter().any(|p| p.name == i.name) || self.pool(&i.name).is_some()
            {
                return Err(invalid("duplicate instance or pool name", &i.name));
            }
        }
        for (n, p) in self.pools.iter().enumerate() {
            if !component(&p.name) || RESERVED_INSTANCES.contains(&p.name.as_str()) {
                return Err(invalid("invalid pool name", &p.name));
            }
            if self.pools[..n].iter().any(|q| q.name == p.name) {
                return Err(invalid("duplicate pool", &p.name));
            }
        }
        for (n, j) in self.jobs.iter().enumerate() {
            if !component(&j.name) || j.name == SERVICES_JOB {
                return Err(invalid("invalid job name", &j.name));
            }
            if self.jobs[..n].iter().any(|q| q.name == j.name) {
                return Err(invalid("duplicate job", &j.name));
            }
            self.payer(&j.payer)?;
            let mut parent = j.parent.as_str();
            for _ in 0..=self.jobs.len() {
                if parent == SERVICES_JOB {
                    break;
                }
                parent = &self
                    .job(parent)
                    .ok_or_else(|| invalid("unknown parent job", &j.name))?
                    .parent;
            }
            if parent != SERVICES_JOB {
                return Err(invalid("job parent cycle", &j.name));
            }
        }
        for (n, r) in self.resources.iter().enumerate() {
            if !valid_name(&r.name) {
                return Err(invalid("invalid resource name", &r.name));
            }
            if self.resources[..n].iter().any(|q| q.name == r.name) {
                return Err(invalid("duplicate resource", &r.name));
            }
            self.payer(&r.payer)?;
            let (rights, allowed) = match &r.kind {
                ResourceKind::Mailbox { rights } => {
                    if r.terminal_revoke {
                        return Err(invalid("terminal revoke requires sender", &r.name));
                    }
                    (
                        *rights,
                        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
                    )
                }
                ResourceKind::Sender {
                    mailbox, rights, ..
                } => {
                    if !self
                        .resource(mailbox)
                        .is_some_and(|m| matches!(m.kind, ResourceKind::Mailbox { .. }))
                    {
                        return Err(invalid("sender requires mailbox resource", &r.name));
                    }
                    if r.terminal_revoke
                        && !rights.contains(
                            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                        )
                    {
                        return Err(invalid(
                            "terminal revoke sender lacks required rights",
                            &r.name,
                        ));
                    }
                    (
                        *rights,
                        Rights::WRITE
                            | Rights::WAIT
                            | Rights::TRANSIT
                            | Rights::GRANT
                            | Rights::DUPLICATE,
                    )
                }
            };
            if !rights.is_known() || !allowed.contains(rights) {
                return Err(invalid("invalid resource rights", &r.name));
            }
        }
        for i in &self.instances {
            if !component(&i.image) || !image_exists(&i.image) {
                return Err(invalid("missing image", &i.image));
            }
            if let Some(job) = &i.job
                && self.job(job).is_none()
            {
                return Err(invalid("unknown job", job));
            }
            if i.inputs.len() > MAX_INPUTS {
                return Err(invalid("too many inputs", &i.name));
            }
            for (n, input) in i.inputs.iter().enumerate() {
                if !valid_name(&input.name) || input.name.starts_with(oal::namespace::INPUT_PREFIX)
                {
                    return Err(invalid("invalid or reserved input name", &input.name));
                }
                if i.inputs[..n].iter().any(|p| p.name == input.name) {
                    return Err(invalid("duplicate input", &input.name));
                }
                self.validate_source(&input.source, input.rights, &image_exists)?;
            }
            let mut description = oal::namespace::Description::default();
            for (n, b) in i.namespace.iter().enumerate() {
                self.validate_source(&b.source, Some(b.rights), &image_exists)?;
                let Source::Resource {
                    name,
                    output: ResourceOutput::Main,
                } = &b.source
                else {
                    return Err(invalid("namespace requires sender main output", &b.prefix));
                };
                if !self
                    .resource(name)
                    .is_some_and(|r| matches!(r.kind, ResourceKind::Sender { .. }))
                {
                    return Err(invalid("namespace requires sender main output", &b.prefix));
                }
                if !b
                    .rights
                    .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
                {
                    return Err(invalid("namespace sender lacks call rights", &b.prefix));
                }
                description.bindings.push(oal::namespace::Binding {
                    prefix: b.prefix.clone(),
                    input: oal::namespace::input_name(n),
                });
            }
            description
                .validate()
                .map_err(|_| invalid("invalid namespace description", &i.name))?;
            settings::Settings::parse(&i.settings)
                .map_err(|_| invalid("invalid settings encoding", &i.name))?;
        }
        // Receiver 必须且只能 move 一次；Lifetime 至多一个显式 owner，不复制观察责任。
        for r in &self.resources {
            for output in [ResourceOutput::Main, ResourceOutput::Lifetime] {
                let source = Source::Resource {
                    name: r.name.clone(),
                    output,
                };
                let uses = self
                    .instances
                    .iter()
                    .flat_map(|i| &i.inputs)
                    .filter(|i| i.source == source)
                    .count();
                match (&r.kind, output) {
                    (ResourceKind::Mailbox { .. }, ResourceOutput::Main) if uses != 1 => {
                        return Err(invalid(
                            "receiver requires exactly one input owner",
                            &r.name,
                        ));
                    }
                    (ResourceKind::Sender { .. }, ResourceOutput::Lifetime) if uses > 1 => {
                        return Err(invalid("lifetime has multiple input owners", &r.name));
                    }
                    _ => {}
                }
            }
        }
        self.validate_grants(0)?;
        self.total_budget()?;
        Ok(())
    }
}
#[cfg(test)]
mod tests;
