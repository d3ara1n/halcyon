//! MMIO 强制执行者的通知仲裁。队列、此状态和维护激活/唤醒在同一门内提交。
//! 新队尾不唤醒正在等待的队首；关闭只能打断普通观察，不能越过 fence。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Active,
    Dependency,
    Fence,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioNotify {
    None,
    Activate,
    WakeDependency,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioIdle {
    Idle,
    Ready,
    Done,
}

pub struct MmioExecutorState {
    phase: Phase,
    closing: bool,
}
impl Default for MmioExecutorState {
    fn default() -> Self {
        Self::new()
    }
}
impl MmioExecutorState {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            closing: false,
        }
    }
    /// None 表示队列已封口；Some(None) 表示接管责任但已有执行/等待 owner。
    pub fn submit(&mut self) -> Option<MmioNotify> {
        if self.closing {
            return None;
        }
        Some(if self.phase == Phase::Idle {
            self.phase = Phase::Active;
            MmioNotify::Activate
        } else {
            MmioNotify::None
        })
    }
    /// 调用者先安装退出租约，再在同一队列门内发布关闭和执行返回的通知动作。
    pub fn close(&mut self) -> MmioNotify {
        assert!(!self.closing, "MMIO executor closed twice");
        self.closing = true;
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Active;
                MmioNotify::Activate
            }
            Phase::Dependency => {
                self.phase = Phase::Active;
                MmioNotify::WakeDependency
            }
            Phase::Active | Phase::Fence => MmioNotify::None,
            Phase::Closed => unreachable!("closed executor accepted a new close"),
        }
    }
    pub fn begin_step(&mut self) {
        assert!(
            matches!(self.phase, Phase::Active | Phase::Fence),
            "MMIO executor ran without notification"
        );
        self.phase = Phase::Active;
    }
    /// 返回 true 后，调用者仍在同一门内 arm 并保存对应 WakeToken。
    pub fn arm_dependency(&mut self) -> bool {
        assert_eq!(
            self.phase,
            Phase::Active,
            "MMIO dependency armed outside active step"
        );
        if self.closing {
            return false;
        }
        self.phase = Phase::Dependency;
        true
    }
    pub fn dependency_hint(&mut self) -> bool {
        if self.phase != Phase::Dependency {
            return false;
        }
        self.phase = Phase::Active;
        true
    }
    pub fn detach_dependency(&mut self) {
        assert!(matches!(self.phase, Phase::Active | Phase::Dependency));
        self.phase = Phase::Active;
    }
    pub fn ready(&mut self) {
        self.phase = Phase::Active;
    }
    pub fn wait_fence(&mut self) {
        assert_eq!(self.phase, Phase::Active);
        self.phase = Phase::Fence;
    }
    /// 与生产者共用同一队列门复检，禁止“先观察空队列、锁外才激活”的窗口。
    pub fn finish_idle(&mut self, has_queued: bool) -> MmioIdle {
        assert_eq!(self.phase, Phase::Active);
        if has_queued {
            return MmioIdle::Ready;
        }
        if self.closing {
            self.phase = Phase::Closed;
            MmioIdle::Done
        } else {
            self.phase = Phase::Idle;
            MmioIdle::Idle
        }
    }
}
