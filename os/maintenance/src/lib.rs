#![no_std]
#![forbid(unsafe_code)]

//! 内核有界维护执行的纯逻辑：工作记录状态机、资金账户 DRR 账本与
//! 每 hart 用户/维护两类别时间轮转器。
//!
//! 本 crate 不持锁、不存储记录、不接收回调、不认识对象种类。内核在
//! 维护队列锁内调用这些纯值转换，按返回的 [`record::Action`] 等结果在
//! 锁外执行挂接、强根交还与 step。成本单位为平台 time tick。

pub mod account;
pub mod record;
pub mod rotor;
