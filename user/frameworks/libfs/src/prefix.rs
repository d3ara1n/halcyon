//! 前缀表：名字前缀 → 目录 Handle，最长前缀匹配。

use alloc::{string::String, vec::Vec};

use erhino_shared::object::Handle;

/// 前缀表条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// 规范化为无尾 `/` 的绝对前缀；根为 `/`。
    pub prefix: String,
    pub directory: Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixError {
    /// 前缀不是合法的绝对前缀（不以 `/` 起头、空段、`.`、`..`）。
    pub kind: BadPrefix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadPrefix {
    NotAbsolute,
    IllegalSegment,
}

/// 前缀表：插入即规范化，重复前缀替换（重挂载）。
#[derive(Debug, Default)]
pub struct PrefixTable {
    entries: Vec<MountEntry>,
}

impl PrefixTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// 规范化前缀：`/a/b/` → `/a/b`；`/` 保持 `/`。
    pub fn normalize(prefix: &str) -> Result<String, PrefixError> {
        if !prefix.starts_with('/') {
            return Err(PrefixError { kind: BadPrefix::NotAbsolute });
        }
        if prefix == "/" {
            return Ok(String::from("/"));
        }
        let trimmed = prefix.strip_suffix('/').unwrap_or(prefix);
        if trimmed[1..].split('/').any(|s| s.is_empty() || s == "." || s == "..") {
            return Err(PrefixError { kind: BadPrefix::IllegalSegment });
        }
        Ok(String::from(trimmed))
    }

    /// 挂载：插入或替换同前缀条目。
    pub fn mount(&mut self, prefix: &str, directory: Handle) -> Result<(), PrefixError> {
        let prefix = Self::normalize(prefix)?;
        if let Some(entry) = self.entries.iter_mut().find(|e| e.prefix == prefix) {
            entry.directory = directory;
        } else {
            self.entries.push(MountEntry { prefix, directory });
        }
        Ok(())
    }

    /// 卸载：移除条目并返回其 Handle（关闭由调用方决定）。
    pub fn unmount(&mut self, prefix: &str) -> Option<Handle> {
        let prefix = Self::normalize(prefix).ok()?;
        let index = self.entries.iter().position(|e| e.prefix == prefix)?;
        Some(self.entries.remove(index).directory)
    }

    /// 最长前缀匹配：返回命中条目与相对后缀（`/` 根条目匹配一切，
    /// 后缀为去掉前缀后去掉首个 `/` 的部分；命中根则后缀为整段路径
    /// 去掉开头 `/`）。无条目命中返回 None。
    pub fn match_path<'a>(&'a self, path: &'a str) -> Option<(&'a MountEntry, &'a str)> {
        let mut best: Option<(&'a MountEntry, &'a str)> = None;
        for entry in &self.entries {
            let suffix = if entry.prefix == "/" {
                match path.strip_prefix('/') {
                    Some(rest) => rest,
                    None => continue,
                }
            } else {
                let rest = match path.strip_prefix(&entry.prefix) {
                    Some(rest) => rest,
                    None => continue,
                };
                // 前缀匹配必须在段边界：/a 不匹配 /ab。
                if !rest.is_empty() && !rest.starts_with('/') {
                    continue;
                }
                if rest.is_empty() {
                    ""
                } else {
                    match rest.strip_prefix('/') {
                        Some(rest) => rest,
                        None => continue,
                    }
                }
            };
            let better = match best {
                None => true,
                Some((current, _)) => entry.prefix.len() > current.prefix.len(),
            };
            if better {
                best = Some((entry, suffix));
            }
        }
        best
    }

    pub fn entries(&self) -> &[MountEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(raw: u64) -> Handle {
        Handle::from_raw(raw)
    }

    #[test]
    fn mount_normalizes_and_replaces() {
        let mut table = PrefixTable::new();
        table.mount("/a/b/", handle(1)).unwrap();
        assert_eq!(table.entries()[0].prefix, "/a/b");
        table.mount("/a/b", handle(2)).unwrap();
        assert_eq!(table.entries().len(), 1);
        assert_eq!(table.entries()[0].directory, handle(2));
    }

    #[test]
    fn rejects_bad_prefixes() {
        let mut table = PrefixTable::new();
        assert_eq!(table.mount("a/b", handle(1)), Err(PrefixError { kind: BadPrefix::NotAbsolute }));
        assert_eq!(
            table.mount("/a/../b", handle(1)),
            Err(PrefixError { kind: BadPrefix::IllegalSegment })
        );
        assert_eq!(table.mount("/a//b", handle(1)), Err(PrefixError { kind: BadPrefix::IllegalSegment }));
    }

    #[test]
    fn longest_prefix_wins_with_correct_suffix() {
        let mut table = PrefixTable::new();
        table.mount("/", handle(10)).unwrap();
        table.mount("/a", handle(11)).unwrap();
        table.mount("/a/b", handle(12)).unwrap();

        let (entry, suffix) = table.match_path("/a/b/c/d").unwrap();
        assert_eq!(entry.prefix, "/a/b");
        assert_eq!(suffix, "c/d");

        let (entry, suffix) = table.match_path("/a/x").unwrap();
        assert_eq!(entry.prefix, "/a");
        assert_eq!(suffix, "x");

        let (entry, suffix) = table.match_path("/q").unwrap();
        assert_eq!(entry.prefix, "/");
        assert_eq!(suffix, "q");

        let (entry, suffix) = table.match_path("/ab").unwrap();
        assert_eq!(entry.prefix, "/");
        assert_eq!(suffix, "ab");

        // 挂载点本体：后缀为空（即该 grant 的根）。
        let (entry, suffix) = table.match_path("/a/b").unwrap();
        assert_eq!(entry.prefix, "/a/b");
        assert_eq!(suffix, "");
    }

    #[test]
    fn unmount_removes_entry() {
        let mut table = PrefixTable::new();
        table.mount("/a", handle(3)).unwrap();
        assert_eq!(table.unmount("/a"), Some(handle(3)));
        assert!(table.match_path("/a/x").is_none());
        assert_eq!(table.unmount("/a"), None);
    }
}
