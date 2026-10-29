//! 驱动声明表与 worker 出生配置。声明表来自 srv_driver 的实例设定，映像为同名命名输入。
use alloc::vec::Vec;
use libdevice::Error;

pub const VERSION: u32 = 1;

/// 首批执行者的实际Pool配置：ABI主栈之外8 MiB映像/堆/执行存储，1 MiB内核元数据。
pub const WORKER_POOL: erhino_shared::memory_pool::MemoryPoolBudget =
    erhino_shared::memory_pool::MemoryPoolBudget {
        pages: ((erhino_shared::proc::PROCESS_MAIN_STACK_SIZE + 8 * 1024 * 1024)
            / erhino_shared::proc::PROCESS_PAGE_SIZE) as u64,
        metadata: 1024 * 1024 / erhino_shared::memory_pool::METADATA_SLOT_BYTES,
    };

/// 一项驱动声明；`key` 即实现映像的组件名，srv_driver 据此取输入 `driver.image.<key>`。
/// identity 的业务字段由 PCI/USB 等领域拥有，这里只做精确匹配。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declaration<'a> {
    pub key: &'a str,
    pub identity: &'a [u8],
    pub priority: u32,
    /// 每个可复用进程可承载的绑定数，不是全实现的进程/Run上限。
    pub capacity: u32,
    /// 是否允许多个绑定复用同一进程；与物理信任/子登记授权独立。
    pub reusable: bool,
    /// 装配政策授予，不从程序自报数据、匹配字符串或路径授予。
    pub child_registrar: bool,
    pub trusted_physical: bool,
}
impl Declaration<'_> {
    fn validate(&self) -> Result<(), Error> {
        if self.key.is_empty()
            || self.key.len() > crate::view::MAX_IMPLEMENTATION_KEY
            || self.identity.is_empty()
            || self.capacity == 0
        {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }
}

const KEYS: [&str; 7] = [
    "implementation",
    "identity",
    "priority",
    "capacity",
    "reusable",
    "child_registrar",
    "trusted_physical",
];

/// 已校验的声明表：实现键唯一、字段合法。
pub struct Catalog<'a> {
    declarations: Vec<Declaration<'a>>,
}
impl<'a> Catalog<'a> {
    /// 从实例设定的 `declarations` 列表解析；任一字段越界、未知键或重复实现键即拒绝。
    pub fn from_settings(bytes: &'a [u8]) -> Result<Self, Error> {
        let invalid = |_| Error::Invalid;
        let root = settings::Settings::parse(bytes).map_err(invalid)?.root();
        root.only(&["declarations"]).map_err(invalid)?;
        let list = root.list("declarations").map_err(invalid)?;
        let mut declarations = Vec::new();
        declarations
            .try_reserve_exact(list.len())
            .map_err(|_| Error::Resource)?;
        for value in list.iter() {
            let table = value.as_table().map_err(invalid)?;
            table.only(&KEYS).map_err(invalid)?;
            let declaration = Declaration {
                key: table.str("implementation").map_err(invalid)?,
                identity: table.str("identity").map_err(invalid)?.as_bytes(),
                priority: table.integer("priority").map_err(invalid)?,
                capacity: table.integer("capacity").map_err(invalid)?,
                reusable: table.bool("reusable").map_err(invalid)?,
                child_registrar: table.bool("child_registrar").map_err(invalid)?,
                trusted_physical: table.bool("trusted_physical").map_err(invalid)?,
            };
            declaration.validate()?;
            // 声明与映像一一对应，同名实现不能被后项覆盖。
            if declarations
                .iter()
                .any(|previous: &Declaration<'_>| previous.key == declaration.key)
            {
                return Err(Error::Conflict);
            }
            declarations.push(declaration);
        }
        Ok(Self { declarations })
    }
    pub fn count(&self) -> usize {
        self.declarations.len()
    }
    pub fn records(&self) -> impl Iterator<Item = Result<Declaration<'a>, Error>> + '_ {
        self.declarations.iter().copied().map(Ok)
    }
    /// 同一优先级的多个匹配项明确 Ambiguous；不通过进程启动/Claim 竞速选择。
    pub fn select(&self, identity: &[u8], previous: Option<usize>) -> Result<Option<usize>, Error> {
        let ceiling = previous
            .map(|index| {
                let record = self.declarations.get(index).ok_or(Error::Invalid)?;
                if record.identity != identity {
                    return Err(Error::Invalid);
                }
                Ok(record.priority)
            })
            .transpose()?;
        let mut selected = None;
        let mut priority = 0;
        let mut ambiguous = false;
        for (index, record) in self.declarations.iter().enumerate() {
            if record.identity != identity
                || ceiling.is_some_and(|ceiling| record.priority >= ceiling)
            {
                continue;
            }
            if selected.is_none() || record.priority > priority {
                selected = Some(index);
                priority = record.priority;
                ambiguous = false;
            } else if record.priority == priority {
                ambiguous = true;
            }
        }
        if ambiguous {
            Err(Error::Conflict)
        } else {
            Ok(selected)
        }
    }
}

/// 具体worker出生配置；实际授权仍只来自同次grant与后续认证Activate。
#[derive(Clone, Copy)]
pub struct WorkerConfig {
    pub capacity: u32,
}
impl WorkerConfig {
    pub const BYTES: usize = 16;
    pub fn encode(self) -> Result<[u8; Self::BYTES], Error> {
        if self.capacity == 0 {
            return Err(Error::Invalid);
        }
        let mut bytes = [0; Self::BYTES];
        bytes[..8].copy_from_slice(b"DRVCONF1");
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..].copy_from_slice(&self.capacity.to_le_bytes());
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::BYTES
            || &bytes[..8] != b"DRVCONF1"
            || bytes[8..12] != VERSION.to_le_bytes()
        {
            return Err(Error::Invalid);
        }
        let capacity = u32::from_le_bytes(bytes[12..].try_into().map_err(|_| Error::Invalid)?);
        if capacity == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self { capacity })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::BTreeMap, string::String, vec};
    use settings::Owned;

    fn declaration(key: &str, identity: &str, priority: i64) -> Owned {
        let mut table = BTreeMap::new();
        table.insert(String::from("implementation"), Owned::Str(key.into()));
        table.insert(String::from("identity"), Owned::Str(identity.into()));
        table.insert(String::from("priority"), Owned::Int(priority));
        table.insert(String::from("capacity"), Owned::Int(2));
        table.insert(String::from("reusable"), Owned::Bool(true));
        table.insert(String::from("child_registrar"), Owned::Bool(false));
        table.insert(String::from("trusted_physical"), Owned::Bool(false));
        Owned::Table(table)
    }

    fn encode(items: Vec<Owned>) -> Vec<u8> {
        let mut root = BTreeMap::new();
        root.insert(String::from("declarations"), Owned::List(items));
        settings::encode(&root).unwrap()
    }

    // 选择依据是明确优先级/歧义规则；同名实现被拒绝。
    #[test]
    fn unique_priority_wins_and_equal_priority_is_ambiguous() {
        let bytes = encode(vec![
            declaration("one", "pci:0c0330", 1),
            declaration("two", "pci:0c0330", 2),
        ]);
        let catalog = Catalog::from_settings(&bytes).unwrap();
        assert_eq!(catalog.select(b"pci:0c0330", None), Ok(Some(1)));
        assert_eq!(catalog.select(b"usb:03:01:01", None), Ok(None));
        let tie = encode(vec![
            declaration("one", "pci:0c0330", 1),
            declaration("two", "pci:0c0330", 1),
        ]);
        assert_eq!(
            Catalog::from_settings(&tie)
                .unwrap()
                .select(b"pci:0c0330", None),
            Err(Error::Conflict)
        );
        let duplicate = encode(vec![
            declaration("one", "pci:0c0330", 1),
            declaration("one", "pci:0c0330", 2),
        ]);
        assert!(matches!(
            Catalog::from_settings(&duplicate),
            Err(Error::Conflict)
        ));
    }

    // 降序和下层歧义来自选择合同；原游标不能越界或跨identity，也不能回到高优先级。
    #[test]
    fn finite_candidates_descend_and_lower_ties_remain_ambiguous() {
        let bytes = encode(vec![
            declaration("top", "usb:kbd", i64::from(u32::MAX)),
            declaration("low", "usb:kbd", 1),
            declaration("middle", "usb:kbd", 2),
            declaration("tie", "usb:kbd", 2),
        ]);
        let catalog = Catalog::from_settings(&bytes).unwrap();
        assert_eq!(catalog.select(b"usb:kbd", None), Ok(Some(0)));
        assert_eq!(catalog.select(b"usb:kbd", Some(0)), Err(Error::Conflict));
        assert_eq!(catalog.select(b"pci:other", Some(0)), Err(Error::Invalid));
        assert_eq!(catalog.select(b"usb:kbd", Some(4)), Err(Error::Invalid));
        let bytes = encode(vec![
            declaration("top", "usb:kbd", i64::from(u32::MAX)),
            declaration("low", "usb:kbd", 1),
            declaration("middle", "usb:kbd", 2),
            declaration("tie", "usb:kbd", 0),
        ]);
        let catalog = Catalog::from_settings(&bytes).unwrap();
        assert_eq!(catalog.select(b"usb:kbd", Some(0)), Ok(Some(2)));
        assert_eq!(catalog.select(b"usb:kbd", Some(2)), Ok(Some(1)));
        assert_eq!(catalog.select(b"usb:kbd", Some(1)), Ok(Some(3)));
        assert_eq!(catalog.select(b"usb:kbd", Some(3)), Ok(None));
    }

    #[test]
    fn malformed_declarations_are_rejected() {
        let mut bad = BTreeMap::new();
        bad.insert(String::from("implementation"), Owned::Str("x".into()));
        assert!(Catalog::from_settings(&encode(vec![Owned::Table(bad)])).is_err());
        assert!(Catalog::from_settings(&encode(vec![declaration("", "pci:x", 1)])).is_err());
        assert!(Catalog::from_settings(&encode(vec![declaration("x", "pci:x", -1)])).is_err());
    }
}
