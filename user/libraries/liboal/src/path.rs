//! 入口根相对的线上路径；不访问 backing，也不施加领域字符规则。

use crate::protocol::Status;

pub fn validate_path(path: &str) -> Result<(), Status> {
    oal::path::validate_path(path).map_err(|_| Status::Invalid)
}
pub fn validate_name(name: &str) -> Result<(), Status> {
    oal::path::validate_name(name).map_err(|_| Status::Invalid)
}
/// 链接目标可含斜杠、`.` 和 `..`，不强制规范化。
pub fn validate_target(target: &str) -> Result<(), Status> {
    oal::path::validate_target(target).map_err(|_| Status::Invalid)
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
