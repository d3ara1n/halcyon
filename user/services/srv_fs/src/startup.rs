//! 文件宿主的出生角色与显式业务政策。运输能力、业务权限、Scope 与付款独立采用。
use crate::deployment::{MAX_ENTRIES, SESSIONS, WATCHES};
use erhino_shared::{call::SystemCallError, object::Rights};
use libfs::FsRights;
use libservice::protocol::{AccessRights, PublishRights};

pub const POOL_INPUT: &str = "memory.pool";
/// 非 registry 宿主的主动 Register 客户端；不是本宿主的编号 authority 角色。
pub const AUTHORITY_INPUT: &str = "service.authority";
pub const FILE_RECEIVER: &str = "file.receiver";
pub const MANAGEMENT_RECEIVER: &str = "file.management.receiver";
pub const SERVICE_RECEIVER: &str = "service.receiver";
pub const FILE_ROOT: &str = "file.root";
pub const MANAGEMENT: &str = "file.management";
pub const DIRECTORY: &str = "service.directory";
pub const AUTHORITY: &str = "service.authority";

#[derive(Clone, Copy, Debug)]
pub struct FilePolicy {
    pub rights: FsRights,
    pub output: Rights,
}
#[derive(Clone, Copy, Debug)]
pub struct DirectoryPolicy<'a> {
    pub scope: &'a str,
    pub rights: AccessRights,
    pub output: Rights,
}
#[derive(Clone, Copy, Debug)]
pub struct AuthorityPolicy<'a> {
    pub scope: &'a str,
    pub rights: PublishRights,
    pub output: Rights,
}
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub watches: usize,
    pub sessions: usize,
    pub file: FilePolicy,
    pub management: FilePolicy,
    pub directory: Option<DirectoryPolicy<'a>>,
    pub authority: Option<AuthorityPolicy<'a>>,
}
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    pub registry: bool,
    entries: [Option<Entry<'a>>; MAX_ENTRIES],
    count: usize,
    pub publish: Option<&'a str>,
}
fn invalid(_: settings::Error) -> SystemCallError {
    SystemCallError::IllegalArgument
}
fn bits(
    table: settings::Table<'_>,
    key: &str,
    names: &[(&str, u64)],
) -> Result<u64, SystemCallError> {
    let mut result = 0;
    for value in table.list(key).map_err(invalid)?.iter() {
        let name = value.as_str().map_err(invalid)?;
        let bit = names
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, bit)| *bit)
            .ok_or(SystemCallError::IllegalArgument)?;
        if result & bit != 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        result |= bit;
    }
    Ok(result)
}
fn output(table: settings::Table<'_>) -> Result<Rights, SystemCallError> {
    Ok(Rights::from_raw(bits(
        table,
        "output",
        &[
            ("read", Rights::READ.raw()),
            ("write", Rights::WRITE.raw()),
            ("signal", Rights::SIGNAL.raw()),
            ("wait", Rights::WAIT.raw()),
            ("manage", Rights::MANAGE.raw()),
            ("create", Rights::CREATE.raw()),
            ("duplicate", Rights::DUPLICATE.raw()),
            ("transit", Rights::TRANSIT.raw()),
            ("grant", Rights::GRANT.raw()),
        ],
    )?))
}
fn file(table: settings::Table<'_>) -> Result<FilePolicy, SystemCallError> {
    table.only(&["rights", "output"]).map_err(invalid)?;
    let raw = bits(
        table,
        "rights",
        &[
            ("traverse", FsRights::TRAVERSE.raw() as u64),
            ("enumerate", FsRights::ENUMERATE.raw() as u64),
            ("read_property", FsRights::READ_PROPERTY.raw() as u64),
            ("write_property", FsRights::WRITE_PROPERTY.raw() as u64),
            ("read_stream", FsRights::READ_STREAM.raw() as u64),
            ("write_stream", FsRights::WRITE_STREAM.raw() as u64),
            ("create", FsRights::CREATE.raw() as u64),
            ("remove", FsRights::REMOVE.raw() as u64),
            ("watch", FsRights::WATCH.raw() as u64),
            ("describe", FsRights::DESCRIBE.raw() as u64),
            ("derive", FsRights::DERIVE.raw() as u64),
        ],
    )?;
    Ok(FilePolicy {
        rights: FsRights::from_raw(raw as u32).ok_or(SystemCallError::IllegalArgument)?,
        output: output(table)?,
    })
}
fn scope(table: settings::Table<'_>) -> Result<&str, SystemCallError> {
    table
        .only(&["scope", "rights", "output"])
        .map_err(invalid)?;
    let scope = table
        .get("scope")
        .ok_or(settings::Error::Missing)
        .and_then(|value| value.as_str())
        .map_err(invalid)?;
    if scope != "/" && !libservice::protocol::valid_name(scope) {
        return Err(SystemCallError::IllegalArgument);
    }
    Ok(scope)
}
impl<'a> Config<'a> {
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &Entry<'a>> {
        self.entries[..self.count]
            .iter()
            .map(|entry| entry.as_ref().expect("parsed entry missing"))
    }
    pub fn entry(&self, index: usize) -> &Entry<'a> {
        self.entries[index].as_ref().expect("parsed entry missing")
    }
    pub fn parse(bytes: &'a [u8]) -> Result<Self, SystemCallError> {
        let root = settings::Settings::parse(bytes).map_err(invalid)?.root();
        root.only(&["registry", "entries", "publish"])
            .map_err(invalid)?;
        let registry = root.bool("registry").map_err(invalid)?;
        let list = root.list("entries").map_err(invalid)?;
        // Pool + 两个文件 receiver；registry 多一个 receiver，非 registry 多一个主动客户端。
        // Namespace 的实际展开由出生层另外约束，不从本地政策伪造额外 grant 容量。
        let maximum = (erhino_shared::proc::PROCESS_MAX_GRANTS - 4) / if registry { 8 } else { 4 };
        if list.is_empty() || list.len() > maximum || list.len() > MAX_ENTRIES {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut entries = [None; MAX_ENTRIES];
        for (slot, value) in entries.iter_mut().zip(list.iter()) {
            let table = value.as_table().map_err(invalid)?;
            table
                .only(if registry {
                    &[
                        "watches",
                        "sessions",
                        "file",
                        "management",
                        "directory",
                        "authority",
                    ]
                } else {
                    &["watches", "sessions", "file", "management"]
                })
                .map_err(invalid)?;
            let watches = table.integer("watches").map_err(invalid)?;
            let sessions = table.integer("sessions").map_err(invalid)?;
            if watches == 0 || watches > WATCHES || sessions == 0 || sessions > SESSIONS {
                return Err(SystemCallError::IllegalArgument);
            }
            let directory = if registry {
                let role = table.table("directory").map_err(invalid)?;
                Some(DirectoryPolicy {
                    scope: scope(role)?,
                    rights: AccessRights::from_raw(bits(
                        role,
                        "rights",
                        &[
                            ("describe", 1),
                            ("enumerate", 2),
                            ("get_entry", 4),
                            ("watch", 8),
                        ],
                    )? as u32)
                    .ok_or(SystemCallError::IllegalArgument)?,
                    output: output(role)?,
                })
            } else {
                None
            };
            let authority = if registry {
                let role = table.table("authority").map_err(invalid)?;
                Some(AuthorityPolicy {
                    scope: scope(role)?,
                    rights: PublishRights::from_raw(bits(
                        role,
                        "rights",
                        &[("create", 1), ("replace", 2), ("delegate", 4)],
                    )? as u32)
                    .ok_or(SystemCallError::IllegalArgument)?,
                    output: output(role)?,
                })
            } else {
                None
            };
            *slot = Some(Entry {
                watches,
                sessions,
                file: file(table.table("file").map_err(invalid)?)?,
                management: file(table.table("management").map_err(invalid)?)?,
                directory,
                authority,
            });
        }
        let publish = root
            .get("publish")
            .map(|value| value.as_str().map_err(invalid))
            .transpose()?;
        if registry == publish.is_some()
            || publish.is_some_and(|name| !libservice::protocol::valid_name(name))
        {
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

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::{collections::BTreeMap, string::String, vec, vec::Vec};
    use settings::Owned;

    fn strings(values: &[&str]) -> Owned {
        Owned::List(
            values
                .iter()
                .map(|value| Owned::Str(String::from(*value)))
                .collect(),
        )
    }
    fn table(values: &[(&str, Owned)]) -> BTreeMap<String, Owned> {
        values
            .iter()
            .map(|(key, value)| (String::from(*key), value.clone()))
            .collect()
    }
    fn file_policy() -> Owned {
        Owned::Table(table(&[
            ("rights", strings(&["read_property"])),
            ("output", strings(&["write", "wait", "transit"])),
        ]))
    }
    fn entry(registry: bool) -> BTreeMap<String, Owned> {
        let mut entry = table(&[
            ("watches", Owned::Int(1)),
            ("sessions", Owned::Int(1)),
            ("file", file_policy()),
            ("management", file_policy()),
        ]);
        if registry {
            entry.insert(
                String::from("directory"),
                Owned::Table(table(&[
                    ("scope", Owned::Str(String::from("/"))),
                    ("rights", strings(&["get_entry"])),
                    ("output", strings(&[])),
                ])),
            );
            entry.insert(
                String::from("authority"),
                Owned::Table(table(&[
                    ("scope", Owned::Str(String::from("storage"))),
                    ("rights", strings(&["create"])),
                    ("output", strings(&[])),
                ])),
            );
        }
        entry
    }
    fn encode(registry: bool, entries: Vec<Owned>) -> Vec<u8> {
        let mut root = table(&[
            ("registry", Owned::Bool(registry)),
            ("entries", Owned::List(entries)),
        ]);
        if !registry {
            root.insert(String::from("publish"), Owned::Str(String::from("storage")));
        }
        settings::encode(&root).expect("test settings encode failed")
    }
    #[test]
    fn roles_keep_business_transport_and_scope_separate() {
        let bytes = encode(true, vec![Owned::Table(entry(true))]);
        let config = Config::parse(&bytes).unwrap();
        let entry = config.entry(0);
        assert_eq!(entry.file.rights, FsRights::READ_PROPERTY);
        assert!(!entry.file.rights.contains(FsRights::DERIVE));
        assert!(!entry.file.output.contains(Rights::GRANT));
        let directory = entry.directory.unwrap();
        assert_eq!(directory.rights, AccessRights::GET_ENTRY);
        assert_eq!(directory.scope, "/");
        assert_eq!(directory.output, Rights::NONE);
        assert_eq!(entry.authority.unwrap().scope, "storage");
    }
    #[test]
    fn missing_or_misspelled_policy_never_grants_defaults() {
        let mut missing = entry(false);
        missing.remove("file");
        assert!(Config::parse(&encode(false, vec![Owned::Table(missing)])).is_err());
        for rights in [strings(&["all"]), strings(&["create", "create"])] {
            let mut malformed = entry(false);
            malformed.insert(
                String::from("file"),
                Owned::Table(table(&[("rights", rights), ("output", strings(&[]))])),
            );
            assert!(Config::parse(&encode(false, vec![Owned::Table(malformed)])).is_err());
        }
        let mut registry = entry(true);
        registry.remove("authority");
        assert!(Config::parse(&encode(true, vec![Owned::Table(registry)])).is_err());
        assert!(Config::parse(&encode(false, vec![Owned::Table(entry(true))])).is_err());
    }
    #[test]
    fn entry_limit_comes_from_birth_pairs_not_old_export_count() {
        // 64 项出生授权：四个固定输入，普通入口四项、registry 入口八项。
        assert!(Config::parse(&encode(false, vec![Owned::Table(entry(false)); 15])).is_ok());
        assert!(Config::parse(&encode(false, vec![Owned::Table(entry(false)); 16])).is_err());
        assert!(Config::parse(&encode(true, vec![Owned::Table(entry(true)); 7])).is_ok());
        assert!(Config::parse(&encode(true, vec![Owned::Table(entry(true)); 8])).is_err());
    }
}
