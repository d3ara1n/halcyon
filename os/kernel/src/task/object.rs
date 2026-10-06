//! 内核对象共同部分：稳定身份、授权与纯电平来源。

use alloc::sync::Arc;
use core::any::Any;

use erhino_shared::object::{ObjectSignals, Rights};

/// 仅用于诊断和内核内部关联的对象身份；不是用户凭据。
pub type Koid = u64;

static NEXT_KOID: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

/// 内核对象身份的唯一铸造口。KernelObject 经 [`ObjectHeader`] 取得；需要类型化
/// 身份的对象核心（Pool 的 `PoolId`、MemoryObject 的 `ObjectId`）直接从这里取，
/// 因此全系统只有一个对象身份序列。
pub fn try_mint_koid() -> Option<Koid> {
    NEXT_KOID.allocate()
}

/// 单对象订阅额度；使协作式信号发布路径有明确工作上界。
pub use erhino_shared::object::ObjectKind;

/// 只含稳定身份的共同头；对象状态和订阅必须与类型数据共用一把对象锁。
pub struct ObjectHeader {
    koid: Koid,
}

impl ObjectHeader {
    /// 用户可触达对象使用的 fallible identity 铸造口。
    pub fn try_new() -> Option<Self> {
        try_mint_koid().map(|koid| Self { koid })
    }

    pub fn new() -> Self {
        Self::try_new().expect("kernel object identity exhausted")
    }

    pub const fn koid(&self) -> Koid {
        self.koid
    }
}

/// 所有可经 Handle 引用的内核对象。
pub trait KernelObject: Any + Send + Sync {
    fn header(&self) -> &ObjectHeader;

    /// capability 持有计数（表内、在途与事务暂持；普通 Arc 不计入）。
    fn holders(&self) -> &capability::Holders;

    /// 最后 capability 消失（仅 Terminal 对象，锁外调用）。只执行对象同步域
    /// 已确定的后续动作；长工作发布到内嵌 Work。
    fn on_last_capability(&self) {}
    fn kind(&self) -> ObjectKind;

    fn related_id(&self) -> u64 {
        0
    }
    fn badge(&self) -> u64 {
        0
    }

    /// 观察存储的强 use；不增加 capability 计数，节点 owner 保持它直至 detach。
    fn wait_source(&self) -> Option<Arc<dyn super::wait_source::ObservationSource>> {
        None
    }

    /// 该类型的授权上限。
    fn allowed_rights(&self) -> Option<Rights>;

    /// 该类型能观察的合法电平位；None 表示对象不是可等待对象。
    fn allowed_signals(&self) -> Option<ObjectSignals>;

    fn as_any(&self) -> &dyn Any;
}

pub type ObjectRef = Arc<dyn KernelObject>;
