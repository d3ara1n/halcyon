#![no_std]

//! 栈窗口布局：内核 per-hart 栈虚拟分区的单一几何真值（纯逻辑，host 可测）。
//!
//! 每槽布局（低 → 高，stride = stack_size + 2×guard）：
//!
//! ```text
//! [ 槽底 guard | formal (stack_size − emergency) | emergency guard | emergency ]
//! ```
//!
//! 物理侧每槽连续打包 `stack_size` 字节（formal 段 + emergency 段相邻），
//! 两个 guard 是纯虚拟洞、不占帧。guard 跨度必须不小于构建审计允许的
//! 单函数最大栈帧——否则一次 sp 下调可整体越过 guard 落入相邻映射段，
//! 「溢出即时可见」失效。该不变量由 audit_elf.py 在构建期从 ELF 符号表
//! 读取 `STACK_GUARD` 强制（内核无从得知帧上限，不在此校验）。
//!
//! 数字真值链：链接脚本定义 `STACK_GUARD`/`EMERGENCY_SIZE`/`STACK_SIZE`/
//! `HART_NUM_LIMIT`/`STACK_WINDOW_VA_BASE` → 汇编 `_ENTRY_CONSTS` 物化 →
//! 内核以本 crate 构造校验。布局数字只写一处（链接脚本）。

use core::ops::Range;

/// 页大小（sv39 4KiB 叶粒度；与 page_table::PAGE_BITS 一致）。
pub const PAGE_SIZE: usize = 4096;

/// sv39 单个 vpn2 槽跨度（1GiB）：栈窗口只占顶槽一个。
pub const VPN2_SPAN: usize = 1 << 30;

/// 布局不变量违约原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// 槽数为零。
    NoSlots,
    /// guard/emergency/stack_size/物理基址非页倍数。
    NotPageAligned,
    /// formal 段为空（stack_size ≤ emergency）。
    EmptyFormal,
    /// 窗口基址不在期望 vpn2 槽（或未按 1GiB 对齐）。
    WrongWindowSlot,
    /// 总跨度超出单个 vpn2 槽容量。
    SpanExceedsSlot,
}

/// 经构造校验的栈窗口布局。所有查询方法以此前提案的不变量为前提。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackWindowLayout {
    window: usize,
    slots: usize,
    stack_size: usize,
    guard: usize,
    emergency: usize,
    phys_base: usize,
}

impl StackWindowLayout {
    /// 由链接期常量构造并整体校验；任何违约使构造失败，
    /// 非法配置在启动期（或 host 测试）即被拒绝，绝不静默接受。
    pub fn new(
        window: usize,
        slots: usize,
        stack_size: usize,
        guard: usize,
        emergency: usize,
        phys_base: usize,
        top_slot: usize,
    ) -> Result<Self, LayoutError> {
        if slots == 0 {
            return Err(LayoutError::NoSlots);
        }
        if guard % PAGE_SIZE != 0
            || emergency % PAGE_SIZE != 0
            || stack_size % PAGE_SIZE != 0
            || phys_base % PAGE_SIZE != 0
        {
            return Err(LayoutError::NotPageAligned);
        }
        if stack_size <= emergency {
            return Err(LayoutError::EmptyFormal);
        }
        if window % VPN2_SPAN != 0 || (window >> 30) & 0x1FF != top_slot {
            return Err(LayoutError::WrongWindowSlot);
        }
        let stride = stack_size
            .checked_add(guard.checked_mul(2).ok_or(LayoutError::SpanExceedsSlot)?)
            .ok_or(LayoutError::SpanExceedsSlot)?;
        let span = stride
            .checked_mul(slots)
            .ok_or(LayoutError::SpanExceedsSlot)?;
        if span > VPN2_SPAN {
            return Err(LayoutError::SpanExceedsSlot);
        }
        Ok(Self {
            window,
            slots,
            stack_size,
            guard,
            emergency,
            phys_base,
        })
    }

    pub const fn window(&self) -> usize {
        self.window
    }

    pub const fn slots(&self) -> usize {
        self.slots
    }

    pub const fn stack_size(&self) -> usize {
        self.stack_size
    }

    pub const fn guard(&self) -> usize {
        self.guard
    }

    pub const fn emergency(&self) -> usize {
        self.emergency
    }

    pub const fn phys_base(&self) -> usize {
        self.phys_base
    }

    /// 每槽 VA 步长 = stack_size + 2×guard。
    pub const fn stride(&self) -> usize {
        self.stack_size + 2 * self.guard
    }

    /// 全窗口 VA 跨度。
    pub const fn span(&self) -> usize {
        self.stride() * self.slots
    }

    /// formal 段字节数。
    pub const fn formal_span(&self) -> usize {
        self.stack_size - self.emergency
    }

    /// slot 槽的 VA 基（槽底 guard 起点）。
    pub const fn slot_base(&self, slot: usize) -> usize {
        self.window + slot * self.stride()
    }

    /// 槽底 guard 洞（不映射；formal 栈向下溢出的第一落点）。
    pub const fn bottom_guard_range(&self, slot: usize) -> Range<usize> {
        let base = self.slot_base(slot);
        base..base + self.guard
    }

    /// formal 栈段（调度循环与 syscall 路径使用）。
    pub const fn formal_range(&self, slot: usize) -> Range<usize> {
        let base = self.slot_base(slot) + self.guard;
        base..base + self.formal_span()
    }

    /// emergency 与 formal 之间的 guard 洞（不映射；emergency 溢出不
    /// 再静默踩入 formal 栈）。
    pub const fn emergency_guard_range(&self, slot: usize) -> Range<usize> {
        let base = self.formal_range(slot).end;
        base..base + self.guard
    }

    /// emergency 栈段（fatal 路径专用，占槽顶）。
    pub const fn emergency_range(&self, slot: usize) -> Range<usize> {
        let base = self.emergency_guard_range(slot).end;
        base..base + self.emergency
    }

    /// formal sp 起点（= emergency 段基）。
    pub const fn formal_top(&self, slot: usize) -> usize {
        self.formal_range(slot).end
    }

    /// emergency sp 起点（= 槽顶）。
    pub const fn emergency_top(&self, slot: usize) -> usize {
        self.emergency_range(slot).end
    }

    /// VA 是否落在任一 guard 洞内：内核栈溢出的第一现场特征。
    pub fn in_guard(&self, va: usize) -> bool {
        let Some((_, rem)) = self.slot_offset(va) else {
            return false;
        };
        rem < self.guard
            || (self.guard + self.formal_span() <= rem
                && rem < 2 * self.guard + self.formal_span())
    }

    /// 窗口内已映射页的 VA→PA 换算（与建表互逆）；guard 洞与窗口外
    /// 返回 None。
    pub fn translate(&self, va: usize) -> Option<usize> {
        let (slot, rem) = self.slot_offset(va)?;
        let slot_pa = self.phys_base + slot * self.stack_size;
        if rem < self.guard {
            return None; // 槽底 guard
        }
        let formal = rem - self.guard;
        if formal < self.formal_span() {
            return Some(slot_pa + formal);
        }
        let emergency_off = formal - self.formal_span();
        if emergency_off < self.guard {
            return None; // emergency guard
        }
        Some(slot_pa + self.formal_span() + (emergency_off - self.guard))
    }

    /// slot 槽全部应映射页的 (va, pa) 序列（4KiB 粒度，建表输入）。
    pub fn mappings(&self, slot: usize) -> SlotMappings {
        SlotMappings {
            layout: *self,
            slot,
            page: 0,
            pages: self.stack_size / PAGE_SIZE,
        }
    }

    /// VA 在窗口内时返回 (slot, 槽内偏移)。
    fn slot_offset(&self, va: usize) -> Option<(usize, usize)> {
        if va < self.window {
            return None;
        }
        let off = va - self.window;
        let slot = off / self.stride();
        if slot >= self.slots {
            return None;
        }
        Some((slot, off % self.stride()))
    }
}

/// 一槽的逐页映射迭代器。
#[derive(Debug, Clone)]
pub struct SlotMappings {
    layout: StackWindowLayout,
    slot: usize,
    page: usize,
    pages: usize,
}

impl Iterator for SlotMappings {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.page >= self.pages {
            return None;
        }
        let layout = &self.layout;
        let slot = self.slot;
        let formal_pages = layout.formal_span() / PAGE_SIZE;
        let (va, pa) = if self.page < formal_pages {
            let va = layout.formal_range(slot).start + self.page * PAGE_SIZE;
            (va, layout.phys_base + slot * layout.stack_size + self.page * PAGE_SIZE)
        } else {
            let index = self.page - formal_pages;
            let va = layout.emergency_range(slot).start + index * PAGE_SIZE;
            (
                va,
                layout.phys_base + slot * layout.stack_size
                    + layout.formal_span()
                    + index * PAGE_SIZE,
            )
        };
        self.page += 1;
        Some((va, pa))
    }
}

const _: () = assert!(PAGE_SIZE == 1 << 12);

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// 链接脚本同名常量的镜像（仅测试向量用；真值在 linker.ld）。
    const GUARD: usize = 0x3000;
    const EMERGENCY: usize = 0x1000;
    const TOP_SLOT: usize = 511;
    const WINDOW: usize = 0xFFFF_FFFF_C000_0000;

    /// qemu virt 向量：8 槽 × 0x40000。
    fn virt_board() -> StackWindowLayout {
        StackWindowLayout::new(WINDOW, 8, 0x40000, GUARD, EMERGENCY, 0x80493000, TOP_SLOT)
            .expect("virt layout must be valid")
    }

    /// qemu sifive_u 向量：8 槽 × 0xA000。
    fn sifive_board() -> StackWindowLayout {
        StackWindowLayout::new(WINDOW, 8, 0xA000, GUARD, EMERGENCY, 0x802B3000, TOP_SLOT)
            .expect("sifive_u layout must be valid")
    }

    #[test]
    fn virt_span_fits_two_leaf_units() {
        let layout = virt_board();
        assert_eq!(layout.stride(), 0x40000 + 2 * GUARD);
        assert_eq!(layout.span(), 0x46000 * 8);
        assert_eq!(layout.span(), 0x230000); // 跨两个 2MiB 叶表单元
    }

    #[test]
    fn sifive_span_fits_single_leaf() {
        let layout = sifive_board();
        assert_eq!(layout.stride(), 0xA000 + 2 * GUARD);
        assert_eq!(layout.span(), 0x10000 * 8);
        assert!(layout.span() <= 1 << 21);
    }

    #[test]
    fn slot_ranges_partition_stride() {
        let layout = virt_board();
        for slot in 0..layout.slots() {
            let bottom = layout.bottom_guard_range(slot);
            let formal = layout.formal_range(slot);
            let emerg_guard = layout.emergency_guard_range(slot);
            let emerg = layout.emergency_range(slot);
            // 相邻无缝、覆盖整槽、互不重叠。
            assert_eq!(bottom.end, formal.start);
            assert_eq!(formal.end, emerg_guard.start);
            assert_eq!(emerg_guard.end, emerg.start);
            assert_eq!(emerg.end, layout.slot_base(slot) + layout.stride());
            // 槽间也无缝。
            assert_eq!(emerg.end, layout.bottom_guard_range(slot + 1).start);
        }
        // formal sp 起点紧邻 emergency guard 洞，槽顶即 emergency sp 起点。
        assert_eq!(layout.formal_top(0), layout.emergency_guard_range(0).start);
        assert_eq!(layout.emergency_top(0), layout.window + layout.stride());
    }

    #[test]
    fn translate_round_trips_every_mapped_page() {
        for layout in [virt_board(), sifive_board()] {
            for slot in 0..layout.slots() {
                let slot_pa = layout.phys_base() + slot * layout.stack_size();
                let mut mapped = std::vec::Vec::new();
                for (va, pa) in layout.mappings(slot) {
                    assert_eq!(va % PAGE_SIZE, 0);
                    assert_eq!(pa % PAGE_SIZE, 0);
                    // 物理侧恰为本槽连续 [slot_pa, slot_pa + stack_size)。
                    assert!((slot_pa..slot_pa + layout.stack_size()).contains(&pa));
                    assert_eq!(layout.translate(va), Some(pa));
                    mapped.push((va, pa));
                }
                assert_eq!(mapped.len(), layout.stack_size() / PAGE_SIZE);
                // 物理页两两不重（formal 与 emergency 各占一段）。
                let mut pas: std::vec::Vec<_> = mapped.iter().map(|(_, pa)| *pa).collect();
                pas.sort();
                pas.dedup();
                assert_eq!(pas.len(), mapped.len());
            }
        }
    }

    #[test]
    fn guards_are_unmapped_and_flagged() {
        let layout = virt_board();
        for slot in 0..layout.slots() {
            for va in [layout.bottom_guard_range(slot).start, layout.bottom_guard_range(slot).end - 1] {
                assert!(layout.in_guard(va));
                assert_eq!(layout.translate(va), None);
            }
            let eg = layout.emergency_guard_range(slot);
            for va in [eg.start, eg.end - 1] {
                assert!(layout.in_guard(va));
                assert_eq!(layout.translate(va), None);
            }
            // 映射段内不是 guard。
            assert!(!layout.in_guard(layout.formal_range(slot).start));
            assert!(!layout.in_guard(layout.emergency_range(slot).end - 1));
            // 窗口外不是 guard。
            assert!(!layout.in_guard(layout.window - 1));
            assert!(!layout.in_guard(layout.window + layout.span()));
        }
    }

    /// 核心不变量：任何 ≤ guard 的单帧下探从映射段内任意 sp 出发，
    /// 要么仍落在槽内映射段、要么落入 guard 洞——永不进入相邻槽。
    #[test]
    fn max_frame_jump_cannot_cross_guard() {
        let max_frame = 0x2800; // audit_elf.py DEFAULT_MAX_FRAME
        for layout in [virt_board(), sifive_board()] {
            assert!(layout.guard() >= max_frame);
            for slot in 0..layout.slots() {
                let slot_lo = layout.slot_base(slot);
                let slot_hi = layout.slot_base(slot) + layout.stride();
                for sp in (layout.formal_range(slot).start..=layout.formal_top(slot)).step_by(0x100) {
                    let target = sp - max_frame;
                    assert!(target >= slot_lo, "formal jump escapes slot");
                }
                let emerg = layout.emergency_range(slot);
                for sp in (emerg.start..=emerg.end).step_by(0x100) {
                    let target = sp - max_frame;
                    assert!(target >= layout.emergency_guard_range(slot).start);
                    assert!(target < slot_hi);
                }
            }
        }
    }

    #[test]
    fn rejects_illegal_configurations() {
        let good = (WINDOW, 8, 0x40000, GUARD, EMERGENCY, 0x80493000, TOP_SLOT);
        // 非顶槽窗口。
        assert_eq!(
            StackWindowLayout::new(0xFFFF_FFFF_8000_0000, 8, 0x40000, GUARD, EMERGENCY, 0x80493000, TOP_SLOT)
                .unwrap_err(),
            LayoutError::WrongWindowSlot
        );
        // 窗口槽位与期望顶槽不符。
        assert_eq!(
            StackWindowLayout::new(WINDOW, 8, 0x40000, GUARD, EMERGENCY, 0x80493000, 510)
                .unwrap_err(),
            LayoutError::WrongWindowSlot
        );
        // 跨度超出单 vpn2 槽。
        assert_eq!(
            StackWindowLayout::new(WINDOW, 8, 0x8000_0000, GUARD, EMERGENCY, 0x80493000, TOP_SLOT)
                .unwrap_err(),
            LayoutError::SpanExceedsSlot
        );
        // 非页倍数。
        assert_eq!(
            StackWindowLayout::new(WINDOW, 8, 0x40000, 0x2800, EMERGENCY, 0x80493000, TOP_SLOT)
                .unwrap_err(),
            LayoutError::NotPageAligned
        );
        // formal 为空。
        assert_eq!(
            StackWindowLayout::new(WINDOW, 8, EMERGENCY, GUARD, EMERGENCY, 0x80493000, TOP_SLOT)
                .unwrap_err(),
            LayoutError::EmptyFormal
        );
        // 物理基址未对齐。
        assert_eq!(
            StackWindowLayout::new(WINDOW, 8, 0x40000, GUARD, EMERGENCY, 0x80493001, TOP_SLOT)
                .unwrap_err(),
            LayoutError::NotPageAligned
        );
        // 零槽。
        assert_eq!(
            StackWindowLayout::new(WINDOW, 0, 0x40000, GUARD, EMERGENCY, 0x80493000, TOP_SLOT)
                .unwrap_err(),
            LayoutError::NoSlots
        );
        // 合法基线仍通过（元组解构验证默认参数可用）。
        let _ = StackWindowLayout::new(good.0, good.1, good.2, good.3, good.4, good.5, good.6)
            .expect("baseline must remain valid");
    }
}
