//! eRhino 系统复位 ABI。平台后端的协议和值不跨越此边界。

/// 系统复位完成后应达到的用户可见终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ResetAction {
    /// 结束当前系统实例。
    Shutdown = 1,
    /// 以完整平台重新初始化为目标重新启动。
    Reboot = 2,
}

impl TryFrom<u64> for ResetAction {
    type Error = ();

    fn try_from(raw: u64) -> Result<Self, Self::Error> {
        match raw {
            value if value == Self::Shutdown as u64 => Ok(Self::Shutdown),
            value if value == Self::Reboot as u64 => Ok(Self::Reboot),
            _ => Err(()),
        }
    }
}

/// 用户态政策提交的复位原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ResetReason {
    /// 正常政策请求。
    Requested = 1,
    /// 系统故障触发。
    SystemFailure = 2,
}

impl TryFrom<u64> for ResetReason {
    type Error = ();

    fn try_from(raw: u64) -> Result<Self, Self::Error> {
        match raw {
            value if value == Self::Requested as u64 => Ok(Self::Requested),
            value if value == Self::SystemFailure as u64 => Ok(Self::SystemFailure),
            _ => Err(()),
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<ResetAction>() == 4);
    assert!(core::mem::size_of::<ResetReason>() == 4);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_rejects_unknown_and_wide_values() {
        assert_eq!(ResetAction::try_from(1), Ok(ResetAction::Shutdown));
        assert_eq!(ResetAction::try_from(2), Ok(ResetAction::Reboot));
        assert_eq!(ResetAction::try_from(0), Err(()));
        assert_eq!(ResetAction::try_from(u32::MAX as u64 + 1), Err(()));
    }

    #[test]
    fn reason_rejects_unknown_and_wide_values() {
        assert_eq!(ResetReason::try_from(1), Ok(ResetReason::Requested));
        assert_eq!(ResetReason::try_from(2), Ok(ResetReason::SystemFailure));
        assert_eq!(ResetReason::try_from(0), Err(()));
        assert_eq!(ResetReason::try_from(u32::MAX as u64 + 1), Err(()));
    }
}
