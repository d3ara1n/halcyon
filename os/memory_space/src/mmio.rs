//! 一次 MMIO Map 的发布与片段责任；不访问物理页、页表或设备寄存器。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioMapPhase {
    Preparing,
    PublishGranted,
    Live,
    RollingBack,
    Revoking,
    Detached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioMapError {
    Closed,
    Busy,
    Capacity,
}

/// `fragments` 同时计入已准入、尚未提交的切界责任，避免撤销漏过迟到 split。
/// 片段释放只能在对应映射翻译同步后（或从未发布的回滚中）提交。
pub struct MmioMapState {
    phase: MmioMapPhase,
    stop_requested: bool,
    operation_finished: bool,
    fragments: usize,
}

impl Default for MmioMapState {
    fn default() -> Self {
        Self::new()
    }
}

impl MmioMapState {
    /// Map 准备 owner 从出生起持一份完整片段责任。
    pub const fn new() -> Self {
        Self {
            phase: MmioMapPhase::Preparing,
            stop_requested: false,
            operation_finished: false,
            fragments: 1,
        }
    }

    pub const fn phase(&self) -> MmioMapPhase {
        self.phase
    }
    pub const fn fragments(&self) -> usize {
        self.fragments
    }

    /// 调用者须在同一个资源准入门内复核祖先状态。
    pub fn grant_publish(&mut self) -> Result<(), MmioMapError> {
        if self.stop_requested || self.phase != MmioMapPhase::Preparing {
            return Err(MmioMapError::Closed);
        }
        self.phase = MmioMapPhase::PublishGranted;
        Ok(())
    }

    /// 原 VM 操作完成不可逆提交。许可之后的 Stop 不能取消该已接受发布。
    pub fn commit(&mut self) {
        assert_eq!(
            self.phase,
            MmioMapPhase::PublishGranted,
            "MMIO commit without publication grant"
        );
        self.phase = if self.stop_requested {
            MmioMapPhase::Revoking
        } else {
            MmioMapPhase::Live
        };
    }

    pub fn request_stop(&mut self) {
        self.stop_requested = true;
        self.phase = match self.phase {
            MmioMapPhase::Preparing => MmioMapPhase::RollingBack,
            MmioMapPhase::Live => MmioMapPhase::Revoking,
            phase => phase,
        };
        self.settle();
    }

    /// 原 Map 操作的最后回执：准备失败或提交成功都恰好执行一次。
    pub fn finish_operation(&mut self) {
        assert!(!self.operation_finished, "MMIO operation finished twice");
        self.operation_finished = true;
        if matches!(
            self.phase,
            MmioMapPhase::Preparing | MmioMapPhase::PublishGranted
        ) {
            self.phase = MmioMapPhase::RollingBack;
        }
        self.settle();
    }

    /// 在锁外付款/准备节点之后取得一次切界责任。提交只移动该责任，不再取资源锁。
    pub fn reserve_fragment(&mut self) -> Result<(), MmioMapError> {
        if self.stop_requested
            || matches!(
                self.phase,
                MmioMapPhase::RollingBack | MmioMapPhase::Revoking | MmioMapPhase::Detached
            )
        {
            return Err(MmioMapError::Closed);
        }
        if self.phase != MmioMapPhase::Live {
            return Err(MmioMapError::Busy);
        }
        self.fragments = self
            .fragments
            .checked_add(1)
            .ok_or(MmioMapError::Capacity)?;
        Ok(())
    }

    pub fn release_fragment(&mut self) {
        self.fragments = self
            .fragments
            .checked_sub(1)
            .expect("MMIO fragment released twice");
        self.settle();
    }

    fn settle(&mut self) {
        if self.operation_finished && self.fragments == 0 {
            self.phase = MmioMapPhase::Detached;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_before_grant_prevents_publication_and_waits_for_rollback() {
        let mut state = MmioMapState::new();
        assert_eq!(state.reserve_fragment(), Err(MmioMapError::Busy));
        state.request_stop();
        assert_eq!(state.grant_publish(), Err(MmioMapError::Closed));
        state.release_fragment();
        assert_eq!(state.phase(), MmioMapPhase::RollingBack);
        state.finish_operation();
        assert_eq!(state.phase(), MmioMapPhase::Detached);
    }

    #[test]
    fn granted_publication_survives_stop_until_committed_work_and_views_finish() {
        let mut state = MmioMapState::new();
        state.grant_publish().unwrap();
        state.request_stop();
        assert_eq!(state.phase(), MmioMapPhase::PublishGranted);
        state.commit();
        state.finish_operation();
        assert_eq!(state.phase(), MmioMapPhase::Revoking);
        assert_eq!(state.reserve_fragment(), Err(MmioMapError::Closed));
        state.release_fragment();
        assert_eq!(state.phase(), MmioMapPhase::Detached);
    }

    #[test]
    fn prepared_split_remains_owned_across_stop_and_original_fragment_retirement() {
        let mut state = MmioMapState::new();
        state.grant_publish().unwrap();
        state.commit();
        state.reserve_fragment().unwrap();
        state.finish_operation();
        state.request_stop();
        state.release_fragment();
        assert_eq!(state.fragments(), 1);
        assert_eq!(state.phase(), MmioMapPhase::Revoking);
        // 迟到切界无论变成新 View 还是取消，都必须归还已经准入的那份责任。
        state.release_fragment();
        assert_eq!(state.phase(), MmioMapPhase::Detached);
    }

    #[test]
    fn ordinary_unmap_also_detaches_and_failed_output_never_becomes_live() {
        let mut mapped = MmioMapState::new();
        mapped.grant_publish().unwrap();
        mapped.commit();
        mapped.finish_operation();
        mapped.release_fragment();
        assert_eq!(mapped.phase(), MmioMapPhase::Detached);
        let mut failed = MmioMapState::new();
        failed.grant_publish().unwrap();
        failed.release_fragment();
        assert_eq!(failed.phase(), MmioMapPhase::PublishGranted);
        failed.finish_operation();
        assert_eq!(failed.phase(), MmioMapPhase::Detached);
    }
}
