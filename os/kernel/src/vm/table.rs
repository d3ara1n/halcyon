//! 页表帧承载：分支表 owner 以帧号寄存于 branch PTE，charge 并入空间聚合账。
//!
//! 设计见 plans/todo-2026-10-01-memory-redesign.md 设计轮次 3：
//! - 同一空间表页全部来自固定空间账户；同源校验在不可失败的 `adopt` 之前
//!   （AnySpace 准备期检查 owner 账户）。
//! - `reclaim` 是所有权移交，不是退款：返回物理帧 + 恰好一页 charge 的完整
//!   affine owner，由调用者在 TLB 安全条件满足、锁外放帧后才退款。
//! - 聚合账 `Option<MemoryCharge>` 表示 0/N 页；页数与 `TableTree` 分支数守恒。
//! - root 独立持完整 `FundedTableFrame`，不入聚合账。

use funded_frame::QuotaCredit;
use page_table::{FrameNumber, TableFrameMemory};

use crate::{frame::FundedTableFrame, mm, task::memory_pool::MemoryCharge};

/// 一个地址空间的表帧访问与聚合 charge（TableCharge）。
pub(crate) struct TableMem {
    charge: Option<MemoryCharge>,
}

impl TableMem {
    pub(crate) const fn new() -> Self {
        Self { charge: None }
    }
}

impl TableFrameMemory for TableMem {
    type FrameOwner = FundedTableFrame;

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; page_table::ENTRIES] {
        // SAFETY: 帧由 branch PTE 寄存（adopt 后帧号即唯一身份）或由调用者持有
        // owner；页对齐、经直映射访问，树的 `&mut` 保证独占。
        unsafe { &mut *(mm::phys_to_virt(frame.addr()) as *mut _) }
    }

    fn adopt(&mut self, owner: FundedTableFrame) {
        let (_frame, charge) = owner.into_adopted();
        match self.charge.as_mut() {
            None => self.charge = Some(charge),
            Some(total) => {
                if let Err(failure) = total.merge(charge) {
                    // 同源已在准备期校验；此处失败为结构性错误。
                    let (error, _) = failure.into_parts();
                    panic!("table charge merge failed after source check: {error:?}");
                }
            }
        }
    }

    fn reclaim(&mut self, frame: FrameNumber) -> FundedTableFrame {
        let total = self
            .charge
            .as_mut()
            .expect("reclaim with empty table charge");
        let one = if total.pages() == 1 {
            self.charge.take().expect("table charge present")
        } else {
            total.split(1).expect("table charge split of one page")
        };
        FundedTableFrame::from_adopted(frame, one)
    }
}
