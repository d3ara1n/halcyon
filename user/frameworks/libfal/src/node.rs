//! 节点模型：类型判别、目录标记与协议级路径契约。

/// 节点类型（挂载点不是节点类型，见 fal.md「命名空间」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum NodeKind {
    Directory = 1,
    Property = 2,
    Stream = 3,
    SymbolicLink = 4,
}

impl NodeKind {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Directory),
            2 => Some(Self::Property),
            3 => Some(Self::Stream),
            4 => Some(Self::SymbolicLink),
            _ => None,
        }
    }
}

/// 目录标记：Read 可枚举、Write 可增删条目、eXecute 可穿越进入下级。
/// 标记之上更细的权限表达由提供者策略决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeAttributes(u32);

impl NodeAttributes {
    pub const NONE: Self = Self(0);
    pub const READABLE: Self = Self(1 << 0);
    pub const WRITEABLE: Self = Self(1 << 1);
    pub const EXECUTABLE: Self = Self(1 << 2);

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for NodeAttributes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitAnd for NodeAttributes {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// 路径分量的非法字节：通配符与分隔符。
const ILLEGAL_COMPONENT_BYTES: &[u8] = b"/*?\\";

/// 路径合法性：协议固定契约，客户端库与提供者各自执行同一规则。
///
/// 规则：UTF-8；`/` 分隔；不允许空段、`.`、`..` 与通配符；请求路径
/// 相对消息所附 Handle（不以 `/` 起头）；根以空路径表达。
pub fn validate_path(path: &[u8]) -> bool {
    if path.len() > crate::PATH_MAX {
        return false;
    }
    if core::str::from_utf8(path).is_err() {
        return false;
    }
    if path.is_empty() {
        return true; // 请求根（Handle 本身）
    }
    if path[0] == b'/' || path[path.len() - 1] == b'/' {
        return false;
    }
    path.split(|&b| b == b'/').all(|segment| {
        !segment.is_empty()
            && segment != b"."
            && segment != b".."
            && !segment.iter().any(|&b| ILLEGAL_COMPONENT_BYTES.contains(&b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_relative_clean_paths() {
        assert!(validate_path(b""));
        assert!(validate_path(b"a"));
        assert!(validate_path(b"a/b/c"));
        assert!(validate_path(b"boot/bin/srv_init"));
    }

    #[test]
    fn rejects_absolute_dot_dot_and_empty_segments() {
        assert!(!validate_path(b"/a"));
        assert!(!validate_path(b"a/"));
        assert!(!validate_path(b"a//b"));
        assert!(!validate_path(b"a/./b"));
        assert!(&b"a/../b"[..3] == b"a/." && !validate_path(b"a/../b"));
        assert!(!validate_path(b"a/"));
        assert!(!validate_path(&[0xFF, 0xFE]));
    }

    #[test]
    fn rejects_wildcards() {
        assert!(!validate_path(b"a/*"));
        assert!(!validate_path(b"*.tar"));
        assert!(!validate_path(b"a/b?c"));
        assert!(!validate_path(b"back\\slash"));
    }

    #[test]
    fn rejects_overlong_paths() {
        let long = [b'a'; crate::PATH_MAX + 1];
        assert!(!validate_path(&long));
    }
}
