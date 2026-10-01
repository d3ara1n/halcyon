//! 地址空间：ledger、页表树与表帧账户（拆自 task/proc.rs）。

pub(crate) mod space;
pub(crate) mod table;
pub(crate) mod remote;
pub(crate) mod change;
pub(crate) mod retire;
pub(crate) mod fault;
