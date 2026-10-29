//! 设备领域的身份、登记轮次、占用与供应责任。
//!
//! 这些机制不发行权限，不启动执行器。服务端必须先按实际认证的 sender context
//! 解析授权、范围与付款，再在同一本地提交边界推进；远端完成不能由运输错误代替。

#![no_std]
#![cfg_attr(target_arch = "riscv64", feature(allocator_ext))]

extern crate alloc;

pub mod binding;
#[cfg(target_arch = "riscv64")]
pub mod binding_work;
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod funding;
pub mod identity;
pub mod protocol;
pub mod provision;
pub mod publication;
pub mod registry;
pub mod resource;
pub mod watch;

/// 领域错误；运输层的 Unsent/Unknown 仍由 librpc 保持。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Permission,
    Invalid,
    Conflict,
    Quota,
    Resource,
    NotFound,
    StaleInstance,
    StaleEpoch,
    InvalidEpoch,
    Busy,
    NotReady,
    Closed,
    Unavailable,
    Unsupported,
    Exhausted,
}

impl From<librpc::bytes::DecodeError> for Error {
    fn from(_: librpc::bytes::DecodeError) -> Self {
        Self::Invalid
    }
}

impl From<erhino_shared::call::SystemCallError> for Error {
    fn from(error: erhino_shared::call::SystemCallError) -> Self {
        use erhino_shared::call::SystemCallError;
        match error {
            SystemCallError::QuotaExceeded => Self::Quota,
            SystemCallError::RightsDenied => Self::Permission,
            SystemCallError::ObjectClosed => Self::Unavailable,
            _ => Self::Resource,
        }
    }
}
