//! FAL 线协议：用户态客户端库与目录提供者之间的固定宽协议
//! （契约见 notes/ideas/fal.md）。内核对文件系统零感知。
//!
//! 消息 payload 布局：`[RpcPrefix][FalHeader][body]`——前缀嵌入，
//! 不构成双层信封。Handle 槽约定：slot 0 恒为 send-once 回复授权；
//! slot 1 恒为请求对象（目录 / 属性 / 流句柄）；其余槽位按 kind 声明，
//! 应答中的 invitation 等交付物从 slot 1 起排布。消息 `kind` 字段为
//! [`PROTOCOL_ID`]，供单邮箱多路分发。
//!
//! 所有整数 little-endian；写者置零保留区，接收者验证已知必需版本、
//! 长度和不变量，不得依赖本机 `usize`、结构体填充或未声明字节序。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod bytes;
pub mod enumerate;
pub mod header;
pub mod io;
pub mod lookup;
pub mod memfs;
pub mod node;
pub mod op;
pub mod property;
pub mod provider;

use librpc::PREFIX_LEN;

/// 消息 `kind` 字段：FAL 协议标识（"FAL1"）。
pub const PROTOCOL_ID: u64 = 0x4641_4c31;

/// FAL header 紧随 RpcPrefix，字节数。
pub const FAL_HEADER_LEN: usize = 16;

/// FAL header 在 payload 中的起始偏移。
pub const FAL_HEADER_OFFSET: usize = PREFIX_LEN;

/// body 在 payload 中的起始偏移。
pub const BODY_OFFSET: usize = PREFIX_LEN + FAL_HEADER_LEN;

/// 路径字节（UTF-8）的协议上限。
pub const PATH_MAX: usize = 512;
