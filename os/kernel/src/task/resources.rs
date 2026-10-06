//! Pool metadata 容量槽与真实付款凭证；地址空间容量由其 planner 契约独立拥有。

use alloc::sync::Arc;
use core::alloc::Layout;
use erhino_shared::{call::SystemCallError, memory_pool::METADATA_SLOT_BYTES};
use memory_pool::AllocatedCredit;

use super::memory_pool::MemoryPool;

pub(crate) const REGION_SLOTS_PER_ADDRESS_SPACE: usize = 4096;
pub(crate) const MEMORY_CHANGES_PER_ADDRESS_SPACE: usize = 16;

fn layout_slots(layout: Layout) -> u64 {
    let bytes = u64::try_from(layout.size()).expect("metadata layout exceeds RV64 size");
    bytes.max(1).div_ceil(METADATA_SLOT_BYTES)
}

/// Arc 的控制头成本与 T 对齐后的 live 存储槽；不承诺外部 Weak 尾部已释放。
pub(crate) fn arc_slots<T>() -> u64 {
    let (layout, _) = Layout::new::<[usize; 2]>()
        .extend(Layout::new::<T>())
        .expect("metadata Arc layout exceeds allocation limit");
    layout_slots(layout.pad_to_align())
}

pub(crate) fn initial_slots(fixed: u64) -> u64 {
    let supply = crate::frame::heap_capacity_bytes();
    let occupied = crate::heap::allocated_bytes();
    let available = supply
        .checked_sub(occupied)
        .expect("boot heap exceeds system supply");
    (available as u64 / METADATA_SLOT_BYTES)
        .checked_sub(fixed)
        .expect("boot heap cannot fund root metadata layout")
}

/// 唯一 charge 强持付款 Pool；既不计 capability 数，也不另开退款 Work。
/// 宿主将此字段放在覆盖资源之后；退款归其 live 值/容量 owner，而非逻辑 len。
pub(crate) struct MetadataCharge {
    credit: Option<AllocatedCredit>,
    pool: Arc<MemoryPool>,
}
impl MetadataCharge {
    pub(crate) fn payer(&self) -> &Arc<MemoryPool> {
        &self.pool
    }
    pub(crate) fn arc<T>(pool: &Arc<MemoryPool>) -> Result<Self, SystemCallError> {
        let credit = MemoryPool::charge_metadata(pool, arc_slots::<T>())?;
        Ok(Self {
            credit: Some(credit),
            pool: pool.clone(),
        })
    }
}

pub(crate) type MetadataBuffer<T> = funded_storage::Buffer<T, MetadataCharge>;

/// 金融授权由实际操作选定；容器只保存已付 charge，不保存默认付款政策。
pub(crate) struct MetadataFunding<'a>(pub(crate) &'a Arc<MemoryPool>);

pub(crate) fn map_storage_error(
    error: funded_storage::StorageError<SystemCallError>,
) -> SystemCallError {
    match error {
        funded_storage::StorageError::Funding(error) => error,
        funded_storage::StorageError::Layout => SystemCallError::ReachLimit,
        funded_storage::StorageError::Allocation => SystemCallError::OutOfMemory,
    }
}

impl funded_storage::Funding for MetadataFunding<'_> {
    type Charge = MetadataCharge;
    type Error = SystemCallError;
    fn charge(&self, layout: Layout) -> Result<MetadataCharge, SystemCallError> {
        let credit = MemoryPool::charge_metadata(self.0, layout_slots(layout))?;
        Ok(MetadataCharge {
            credit: Some(credit),
            pool: self.0.clone(),
        })
    }
}
impl Drop for MetadataCharge {
    fn drop(&mut self) {
        if let Some(credit) = self.credit.take() {
            self.pool.return_metadata(credit);
        }
    }
}
