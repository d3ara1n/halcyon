//! 两种通用接续表达；真实入口由 RPC 的能力槽交付。

use crate::{
    path::{validate_path, validate_target},
    protocol::Status,
};
use librpc::bytes::{Reader, Writer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderBoundary<'a> {
    pub consumed_bytes: u32,
    pub remaining: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkBoundary<'a> {
    pub parent_bytes: u32,
    pub target: &'a str,
    pub remaining: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary<'a> {
    Provider(ProviderBoundary<'a>),
    Link(LinkBoundary<'a>),
}

impl Boundary<'_> {
    pub const fn capability_count(&self) -> usize {
        match self {
            Self::Provider(_) => 1,
            Self::Link(_) => 0,
        }
    }

    /// 验证原路径的连续覆盖；字节偏移只能指向 UTF-8 分量边界。
    pub fn validate_for(&self, path: &str) -> Result<(), Status> {
        validate_path(path)?;
        match self {
            Self::Provider(value) => {
                validate_path(value.remaining)?;
                let end = usize::try_from(value.consumed_bytes).map_err(|_| Status::Invalid)?;
                if end == 0 || !path.is_char_boundary(end) {
                    return Err(Status::Invalid);
                }
                let suffix = if end == path.len() {
                    ""
                } else if path.as_bytes().get(end) == Some(&b'/') {
                    &path[end + 1..]
                } else {
                    return Err(Status::Invalid);
                };
                if suffix != value.remaining {
                    return Err(Status::Invalid);
                }
            }
            Self::Link(value) => {
                validate_target(value.target)?;
                validate_path(value.remaining)?;
                let parent = usize::try_from(value.parent_bytes).map_err(|_| Status::Invalid)?;
                if !path.is_char_boundary(parent) {
                    return Err(Status::Invalid);
                }
                let start = if parent == 0 {
                    0
                } else if path.as_bytes().get(parent) == Some(&b'/') {
                    parent + 1
                } else {
                    return Err(Status::Invalid);
                };
                let tail = &path[start..];
                let (component, suffix) = tail.split_once('/').unwrap_or((tail, ""));
                if component.is_empty() || suffix != value.remaining {
                    return Err(Status::Invalid);
                }
            }
        }
        Ok(())
    }

    pub fn encoded_len(&self) -> Result<usize, Status> {
        match self {
            Self::Provider(value) => {
                validate_path(value.remaining)?;
                if value.consumed_bytes == 0 {
                    return Err(Status::Invalid);
                }
            }
            Self::Link(value) => {
                validate_target(value.target)?;
                validate_path(value.remaining)?;
            }
        }
        let length = match self {
            Self::Provider(value) => 8usize.checked_add(value.remaining.len()),
            Self::Link(value) => 12usize
                .checked_add(value.target.len())
                .and_then(|len| len.checked_add(value.remaining.len())),
        };
        length.ok_or(Status::Invalid)
    }

    /// 调用方先核 encoded_len 和完整报文容量；领域仍拥有自己的结果标签。
    pub fn write(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Provider(value) => {
                writer.u32(value.consumed_bytes);
                writer.bytes32(value.remaining.as_bytes());
            }
            Self::Link(value) => {
                writer.u32(value.parent_bytes);
                writer.bytes32(value.target.as_bytes());
                writer.bytes32(value.remaining.as_bytes());
            }
        }
    }
}

impl<'a> Boundary<'a> {
    /// 只读取通用接续字段；使用者还须 finish 并相对于本次路径 validate_for。
    pub fn read(result: u32, reader: &mut Reader<'a>) -> Result<Self, Status> {
        match result {
            1 => Ok(Self::Provider(ProviderBoundary {
                consumed_bytes: reader.u32()?,
                remaining: reader.text32()?,
            })),
            2 => Ok(Self::Link(LinkBoundary {
                parent_bytes: reader.u32()?,
                target: reader.text32()?,
                remaining: reader.text32()?,
            })),
            _ => Err(Status::Invalid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_boundary_requires_nonzero_component_end_and_exact_suffix() {
        // 防止错偏移/错后缀把请求转到未覆盖的路径。
        let valid = Boundary::Provider(ProviderBoundary {
            consumed_bytes: 3,
            remaining: "x",
        });
        assert_eq!(valid.validate_for("雪/x"), Ok(()));
        assert_eq!(
            Boundary::Provider(ProviderBoundary {
                consumed_bytes: 3,
                remaining: ""
            })
            .validate_for("雪"),
            Ok(())
        );
        for consumed_bytes in [0, 1, 2, 4, 99, u32::MAX] {
            assert_eq!(
                Boundary::Provider(ProviderBoundary {
                    consumed_bytes,
                    remaining: "x"
                })
                .validate_for("雪/x"),
                Err(Status::Invalid)
            );
        }
        assert_eq!(valid.validate_for("雪/y"), Err(Status::Invalid));
        assert_eq!(valid.validate_for(""), Err(Status::Invalid));
    }

    #[test]
    fn link_boundary_covers_exactly_one_component_between_parent_and_suffix() {
        // parent 不含分隔符；转发恰好覆盖一个分量，不能吞掉后缀。
        let valid = Boundary::Link(LinkBoundary {
            parent_bytes: 3,
            target: "../new",
            remaining: "x/y",
        });
        assert_eq!(valid.validate_for("雪/link/x/y"), Ok(()));
        assert_eq!(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "/new",
                remaining: "Song"
            })
            .validate_for("Latest-Releases/Song"),
            Ok(())
        );
        assert_eq!(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "new",
                remaining: ""
            })
            .validate_for("link"),
            Ok(())
        );
        for parent_bytes in [1, 2, 4, 5, 99, u32::MAX] {
            assert_eq!(
                Boundary::Link(LinkBoundary {
                    parent_bytes,
                    target: "../new",
                    remaining: "x/y"
                })
                .validate_for("雪/link/x/y"),
                Err(Status::Invalid)
            );
        }
        assert_eq!(
            Boundary::Link(LinkBoundary {
                parent_bytes: 3,
                target: "new",
                remaining: "y"
            })
            .validate_for("雪/link/x/y"),
            Err(Status::Invalid)
        );
        assert_eq!(valid.validate_for(""), Err(Status::Invalid));
    }
}
