//! OAL 的目录发现表示与协议；不持有领域 backing 或解释领域标记。

#![cfg_attr(not(test), no_std)]
#![cfg_attr(target_arch = "riscv64", feature(allocator_api))]

extern crate alloc;

pub mod boundary;
pub mod budget;
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod info;
pub mod namespace;
pub mod path;
pub mod protocol;
pub mod view;
pub mod walk;

pub use boundary::{Boundary, LinkBoundary, ProviderBoundary};
pub use budget::{FollowBudget, FollowLimits, LocalError};
#[cfg(target_arch = "riscv64")]
pub use client::{Client, ClientError, EnumerationCursor, EnumerationPage, Lookup};
pub use info::{DIRECTORY, MarkerId, ObjectInfo, Tag};
pub use namespace::Namespace;
