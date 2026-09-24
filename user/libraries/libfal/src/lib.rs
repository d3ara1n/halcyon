//! FAL2 用户态协议、目录后端与客户端库。
//!
//! 消息 payload 布局：`[RpcPrefix][protocol::Header][body]`。RPC 请求
//! slot 0 恒为 send-once 回复授权；FAL2 当前请求不携带临时 anchor，
//! 各应答按 [`protocol::Response::capability_count`] 声明能力数量。
//!
//! 所有整数 little-endian；写者置零保留区，接收者验证已知必需版本、
//! 长度和不变量，不得依赖本机 `usize`、结构体填充或未声明字节序。

#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]

extern crate alloc;

pub mod authority;
pub mod backend;
pub mod bytes;
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod data;
pub mod node;
pub mod protocol;
pub mod resource;
pub mod route;
pub mod store;
pub mod value;
pub mod watch;

#[cfg(target_arch = "riscv64")]
pub mod grant;
#[cfg(target_arch = "riscv64")]
pub mod provider;

/// 路径字节（UTF-8）的协议上限。
pub const PATH_MAX: usize = 512;
