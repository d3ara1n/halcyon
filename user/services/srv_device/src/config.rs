//! 实例设定：设备管理者在 Ready 前签发的预限定来源。
//!
//! `sources` 为表列表，每项 `name`（单段小写名）、`scope`（当前只接受 `"root"`）与
//! `access`（libdevice `Access` 位名列表）。每项导出 `device.source.<name>` 与其管理端
//! `device.source.<name>.admin`。
use alloc::vec::Vec;
use libdevice::registry::{Access, Scope};

/// 预限定来源的导出名前缀。
pub const SOURCE_EXPORT: &str = "device.source";

const ACCESS_NAMES: [(&str, Access); 7] = [
    ("observe", Access::OBSERVE),
    ("functions", Access::FUNCTIONS),
    ("register", Access::REGISTER),
    ("claim", Access::CLAIM),
    ("admin", Access::ADMIN),
    ("report_stopped", Access::REPORT_STOPPED),
    ("child_register", Access::CHILD_REGISTER),
];

pub struct Prescoped {
    pub name: &'static str,
    pub scope: Scope,
    pub access: Access,
}

pub struct Config {
    pub sources: Vec<Prescoped>,
}

pub struct Invalid(pub &'static str);

fn single_segment(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 16
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

impl Config {
    pub fn parse(bytes: &'static [u8]) -> Result<Self, Invalid> {
        let bad = |reason| move |_| Invalid(reason);
        let root = settings::Settings::parse(bytes)
            .map_err(bad("settings encoding"))?
            .root();
        root.only(&["sources"])
            .map_err(bad("unknown settings key"))?;
        let list = match root.get("sources") {
            Some(value) => value.as_list().map_err(bad("sources is not a list"))?,
            None => {
                return Ok(Self {
                    sources: Vec::new(),
                });
            }
        };
        if list.len() > crate::server::PRESCOPED_MAX {
            return Err(Invalid("too many pre-scoped sources"));
        }
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(list.len())
            .map_err(|_| Invalid("source storage"))?;
        for item in list.iter() {
            let table = item.as_table().map_err(bad("source is not a table"))?;
            table
                .only(&["name", "scope", "access"])
                .map_err(bad("unknown source key"))?;
            let name = table.str("name").map_err(bad("source name"))?;
            if !single_segment(name) || sources.iter().any(|s: &Prescoped| s.name == name) {
                return Err(Invalid("source name invalid or duplicated"));
            }
            let scope = match table.str("scope").map_err(bad("source scope"))? {
                "root" => Scope::Root,
                _ => return Err(Invalid("source scope unsupported")),
            };
            let mut access = None::<Access>;
            for value in table.list("access").map_err(bad("source access"))?.iter() {
                let text = value.as_str().map_err(bad("access name"))?;
                let (_, bit) = ACCESS_NAMES
                    .iter()
                    .find(|(known, _)| *known == text)
                    .ok_or(Invalid("access name unknown"))?;
                access = Some(access.map_or(*bit, |acc| acc.union(*bit)));
            }
            sources.push(Prescoped {
                name,
                scope,
                access: access.ok_or(Invalid("source access empty"))?,
            });
        }
        Ok(Self { sources })
    }
}
