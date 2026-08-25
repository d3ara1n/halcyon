//! libfs：客户端命名空间库——前缀表、逐子树委托走路与符号链接展开
//! （契约见 notes/ideas/fal.md「命名空间」「走路」）。
//!
//! 命名空间是进程私有的「名字前缀 → 目录 Handle」路由表；走路引擎
//! 在客户端展开符号链接（迭代式组件队列 + 逻辑目录栈，`..` 不上行
//! 穿过权限上界），抵达终点后以 `Position`（帧锚 Handle + 相对后缀）
//! 寻址后续操作。rinlib 保持纯运行时，本库不依赖 syscall——传输由
//! [`resolve::WalkTransport`] 抽象注入，host 可测；真实传输随 fs 集成
//! 批次接入。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod prefix;
pub mod resolve;

/// 整次路径解析的符号链接展开上限（对齐 Linux path_resolution(7) 与
/// notes/ideas/fal.md「走路」）。
pub const SYMLINK_LIMIT: usize = 40;

/// 整次解析累计处理的组件数上限。
pub const COMPONENT_LIMIT: usize = 4096;

/// 整次解析累计的名字字节上限。
pub const BYTE_LIMIT: usize = 65536;
