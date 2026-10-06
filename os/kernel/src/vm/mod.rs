//! 地址空间：ledger、页表树与表帧账户（拆自 task/proc.rs）。

pub(crate) mod backing;
pub(crate) mod change;
mod completion;
pub(crate) mod operation;
pub(crate) mod remote;
pub(crate) mod retire;
pub(crate) mod space;
pub(crate) mod table;
pub(crate) mod views;
