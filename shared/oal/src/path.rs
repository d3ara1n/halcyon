//! 入口根相对的线上路径；不访问 backing，也不施加领域字符规则。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPath;

pub fn validate_path(path: &str) -> Result<(), InvalidPath> {
    if path.is_empty() {
        return Ok(());
    }
    for component in path.split('/') {
        validate_name(component)?;
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<(), InvalidPath> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        Err(InvalidPath)
    } else {
        Ok(())
    }
}

/// 链接目标属于客户端视图路径，可以含斜杠、`.` 和 `..`。
pub fn validate_target(target: &str) -> Result<(), InvalidPath> {
    if target.is_empty() || target.contains('\0') {
        Err(InvalidPath)
    } else {
        Ok(())
    }
}

/// 前缀与 cwd 只接受已规范化的绝对路径，不隐式消去分量。
pub fn validate_absolute(path: &str) -> Result<(), InvalidPath> {
    validate_path(path.strip_prefix('/').ok_or(InvalidPath)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_paths_and_permissive_targets() {
        for p in ["", "雪/é", "a*b/?/\\"] {
            assert_eq!(validate_path(p), Ok(()));
        }
        assert!(validate_path(&alloc::string::String::from("a").repeat(700)).is_ok());
        for p in ["/", "/a", "a/", "a//b", ".", "a/../b", "a/./b", "a\0b"] {
            assert!(validate_path(p).is_err());
        }
        for p in ["/", "/雪", "/a*b/é"] {
            assert!(validate_absolute(p).is_ok());
        }
        for p in ["", "a", "//", "/a/", "/.."] {
            assert!(validate_absolute(p).is_err());
        }
        assert!(validate_target("../a//./b").is_ok());
        assert!(validate_target("").is_err());
        assert!(validate_target("a\0").is_err());
    }
}
