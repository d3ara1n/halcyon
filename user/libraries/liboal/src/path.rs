//! 入口根相对的线上路径；不访问 backing，也不施加领域字符规则。

use crate::protocol::Status;

pub fn validate_path(path: &str) -> Result<(), Status> {
    if path.is_empty() {
        return Ok(());
    }
    for component in path.split('/') {
        validate_name(component)?;
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<(), Status> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        Err(Status::Invalid)
    } else {
        Ok(())
    }
}

/// 链接目标属于客户端视图路径，可以含斜杠、`.` 和 `..`。
pub fn validate_target(target: &str) -> Result<(), Status> {
    if target.is_empty() || target.contains('\0') {
        Err(Status::Invalid)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_paths_allow_root_and_ordinary_characters_without_legacy_limit() {
        // 计划的线上路径合同：空串是根，通配符/反斜杠为普通字符，无 512 上限。
        for path in ["", "星/é", "a*b/?/\\"] {
            assert_eq!(validate_path(path), Ok(()));
        }
        assert_eq!(validate_path(&"a".repeat(700)), Ok(()));
    }

    #[test]
    fn wire_paths_reject_empty_dot_and_nul_components() {
        // 防止未规范化路径或隐藏分量进入实际 provider。
        for path in ["/", "/a", "a/", "a//b", ".", "a/../b", "a/./b", "a\0b"] {
            assert_eq!(validate_path(path), Err(Status::Invalid));
        }
        assert_eq!(validate_target("../a//./b"), Ok(()));
        assert_eq!(validate_target(""), Err(Status::Invalid));
        assert_eq!(validate_target("a\0"), Err(Status::Invalid));
    }
}
