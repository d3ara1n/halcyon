//! 客户端视图中的词法路径；被抵消的分量不访问远端。

use crate::{FollowBudget, LocalError, path::validate_path};
use alloc::string::String;

/// Namespace 前缀和显式 cwd 必须已经是规范绝对路径；不静默修正绑定。
pub fn validate_absolute(path: &str) -> Result<(), LocalError> {
    let relative = path.strip_prefix('/').ok_or(LocalError::InvalidPath)?;
    validate_path(relative).map_err(|_| LocalError::InvalidPath)
}

pub fn normalize(path: &str, cwd: &str, budget: &mut FollowBudget) -> Result<String, LocalError> {
    budget.consume_bytes(cwd.len())?;
    validate_absolute(cwd)?;
    let base = if path.starts_with('/') { "/" } else { cwd };
    Ok(expand(base, &[path], budget)?.path)
}

pub(crate) struct Expanded {
    pub path: String,
    /// 记录展开途中最浅父位置；离开过入口根就不能保留该帧，即使最终又回到其下。
    pub retained_depth: usize,
}

pub(crate) fn depth(path: &str) -> usize {
    if path == "/" {
        0
    } else {
        path.bytes().filter(|byte| *byte == b'/').count()
    }
}

pub(crate) fn copy_text(text: &str, budget: &mut FollowBudget) -> Result<String, LocalError> {
    budget.consume_bytes(text.len())?;
    let mut out = String::new();
    out.try_reserve_exact(text.len())
        .map_err(|_| LocalError::Resource)?;
    out.push_str(text);
    Ok(out)
}

pub(crate) fn expand(
    base: &str,
    parts: &[&str],
    budget: &mut FollowBudget,
) -> Result<Expanded, LocalError> {
    let mut capacity = base.len();
    for part in parts {
        capacity = capacity
            .checked_add(part.len())
            .and_then(|n| n.checked_add(1))
            .ok_or(LocalError::BudgetExceeded)?;
    }
    // 包括被词法抵消的输入，循环不能用 .. 消去已扣的扫描/内存额度。
    budget.consume_bytes(capacity)?;
    validate_absolute(base)?;
    if parts.iter().any(|part| part.contains('\0')) {
        return Err(LocalError::InvalidPath);
    }
    let mut out = String::new();
    out.try_reserve_exact(capacity)
        .map_err(|_| LocalError::Resource)?;
    out.push_str(base);
    let mut level = depth(base);
    let mut retained_depth = level;
    for part in parts {
        for component in part.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    if out == "/" {
                        return Err(LocalError::InvalidPath);
                    }
                    // out 是规范绝对路径，至少含开头的 '/'；rfind 不依赖远端事实。
                    let end = out.rfind('/').ok_or(LocalError::InvalidPath)?;
                    out.truncate(end.max(1));
                    level -= 1;
                    retained_depth = retained_depth.min(level);
                }
                name => {
                    if out != "/" {
                        out.push('/');
                    }
                    out.push_str(name);
                    level += 1;
                }
            }
        }
    }
    Ok(Expanded {
        path: out,
        retained_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use erhino_shared::time::Deadline;
    fn budget() -> FollowBudget {
        FollowBudget::new(10, 4096, Deadline::at(100)).unwrap()
    }

    #[test]
    fn absolute_relative_and_cancelled_components_are_pure_lexical_paths() {
        // missing/.. 不查询 missing；重复斜杠和 . 只在本地折叠。
        for (path, cwd, expected) in [
            ("missing/../x", "/", "/x"),
            ("../c//./d", "/a/b", "/a/c/d"),
            ("/x//y/../z/", "/a", "/x/z"),
            ("/new/", "/", "/new"),
            ("/new/.", "/", "/new"),
            ("", "/a", "/a"),
            ("///", "/", "/"),
            ("雪/../é", "/", "/é"),
        ] {
            assert_eq!(normalize(path, cwd, &mut budget()).unwrap(), expected);
        }
        for path in ["../x", "/a/../../b", "a\0b"] {
            assert_eq!(
                normalize(path, "/", &mut budget()),
                Err(LocalError::InvalidPath)
            );
        }
    }

    #[test]
    fn binding_prefixes_and_cwd_are_canonical_and_absolute() {
        // bind 不规范化以免两个不同写法隐式替换；cwd 同样是明确的视图位置。
        for prefix in ["/", "/a", "/a/b", "/雪"] {
            assert_eq!(validate_absolute(prefix), Ok(()));
        }
        for prefix in ["", "a", "/a/", "/a//b", "/a/.", "/a/..", "/a\0"] {
            assert_eq!(validate_absolute(prefix), Err(LocalError::InvalidPath));
        }
    }

    #[test]
    fn expansion_records_crossed_roots_even_when_target_returns_below_them() {
        // ../b/x 曾离开 /a/b，不能因最终同前缀而复用 /a/b 的旧授权帧。
        let result = expand("/a/b", &["../b/x"], &mut budget()).unwrap();
        assert_eq!(result.path, "/a/b/x");
        assert_eq!(result.retained_depth, 1);
        let mut short = FollowBudget::new(1, 3, Deadline::at(100)).unwrap();
        assert_eq!(
            normalize("a/../b", "/", &mut short),
            Err(LocalError::BudgetExceeded)
        );
        // 规范化完整目标时末名字也耗扫描/展开预算，不能只计父目录。
        let mut short = FollowBudget::new(2, 8, Deadline::at(100)).unwrap();
        assert_eq!(
            normalize("/long-basename", "/", &mut short),
            Err(LocalError::BudgetExceeded)
        );
    }
}
