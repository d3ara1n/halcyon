//! 文件宿主的出生输入名、导出名与实例设定。
//!
//! 设定键：
//! - `registry`（布尔）：是否在本宿主承载服务 registry；
//! - `entries`（表列表，1..=[`MAX_ENTRIES`]）：每入口 `watches`、`sessions` 份额；
//! - `publish`（字符串，仅不承载 registry 时必需）：把首入口文件根登记到外部 registry 的服务名。
use crate::deployment::{MAX_ENTRIES, SESSIONS, WATCHES};
use erhino_shared::call::SystemCallError;

/// 本实例 Pool 的 CREATE 别名。
pub const POOL_INPUT: &str = "memory.pool";
/// 不承载 registry 时，向外部 registry 登记首入口的发布授权。
pub const AUTHORITY_INPUT: &str = "service.authority";

/// 第 N 个入口的导出名前缀；完整名为 `<前缀>.<N>`。
pub const FILE_ROOT_EXPORT: &str = "file.root";
pub const MANAGEMENT_EXPORT: &str = "file.management";
pub const DIRECTORY_EXPORT: &str = "service.directory";
pub const AUTHORITY_EXPORT: &str = "service.authority";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub watches: usize,
    pub sessions: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    pub registry: bool,
    entries: [Entry; MAX_ENTRIES],
    count: usize,
    pub publish: Option<&'a str>,
}

impl<'a> Config<'a> {
    pub fn entries(&self) -> &[Entry] {
        &self.entries[..self.count]
    }

    /// 解析并校验实例设定；任何越界或缺键都拒绝启动。
    pub fn parse(bytes: &'a [u8]) -> Result<Self, SystemCallError> {
        let invalid = |_| SystemCallError::IllegalArgument;
        let root = settings::Settings::parse(bytes).map_err(invalid)?.root();
        root.only(&["registry", "entries", "publish"])
            .map_err(invalid)?;
        let registry = root.bool("registry").map_err(invalid)?;
        let list = root.list("entries").map_err(invalid)?;
        if list.is_empty() || list.len() > MAX_ENTRIES {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut entries = [Entry {
            watches: 0,
            sessions: 0,
        }; MAX_ENTRIES];
        for (slot, value) in entries.iter_mut().zip(list.iter()) {
            let table = value.as_table().map_err(invalid)?;
            table.only(&["watches", "sessions"]).map_err(invalid)?;
            *slot = Entry {
                watches: table.integer("watches").map_err(invalid)?,
                sessions: table.integer("sessions").map_err(invalid)?,
            };
            if slot.watches == 0
                || slot.watches > WATCHES
                || slot.sessions == 0
                || slot.sessions > SESSIONS
            {
                return Err(SystemCallError::IllegalArgument);
            }
        }
        let publish = match root.get("publish") {
            Some(value) => Some(value.as_str().map_err(invalid)?),
            None => None,
        };
        if registry == publish.is_some() {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            registry,
            entries,
            count: list.len(),
            publish,
        })
    }
}
