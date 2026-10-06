//! 单对象等待的安全封装；多对象由显式 WaitSet 聚合。

use crate::{call::sys_wait_one, time::Deadline};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    wait::WaitResult,
};

/// 相对毫秒只在入口转换一次，零表示无限。
pub fn wait_one(
    handle: Handle,
    signals: ObjectSignals,
    timeout_ms: u64,
) -> Result<WaitResult, SystemCallError> {
    wait_until(handle, signals, crate::time::timeout_millis(timeout_ms)?)
}

pub fn wait_until(
    handle: Handle,
    signals: ObjectSignals,
    deadline: Deadline,
) -> Result<WaitResult, SystemCallError> {
    sys_wait_one(handle, signals, deadline).map(WaitResult::from_observed)
}

/// 非阻塞重读真实来源；用于最终超时裁决，不能用空 WaitSet 候选批替代。
pub fn probe(handle: Handle, signals: ObjectSignals) -> Result<ObjectSignals, SystemCallError> {
    wait_until(handle, signals, Deadline::at(0)).map(|result| result.observed)
}
