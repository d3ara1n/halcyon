use core::fmt::Display;

use erhino_shared::mem::{Address, MemoryRegionAttribute, PageNumber};
use flagset::FlagSet;

use crate::debug;

use super::{
    frame::{self, FrameTracker},
    page::{
        PageEntryFlag, PageEntryType, PageEntryWriteError, PageTable, PageTableEntry, PAGE_BITS,
    },
};

#[derive(Debug)]
pub enum MemoryUnitError {
    EntryNotFound,
    RanOutOfFrames,
    EntryOverwrite,
}

pub enum AddressSpace {
    User,
    Invalid,
    Kernel,
}

pub struct MemoryUnit<E: PageTableEntry + Sized + 'static> {
    identity: u16,
    root: PageTable<E>,
    where_the_frame_tracker_of_root_for_recycling_put: FrameTracker,
}

impl<E: PageTableEntry + Sized + 'static> MemoryUnit<E> {
    pub fn new(id: u16) -> Result<Self, MemoryUnitError> {
        if let Some(frame) = frame::borrow(1) {
            Ok(Self {
                identity: id,
                root: PageTable::<E>::from(frame.start()),
                where_the_frame_tracker_of_root_for_recycling_put: frame,
            })
        } else {
            Err(MemoryUnitError::RanOutOfFrames)
        }
    }

    pub fn satp(&self) -> usize {
        let mode = PAGE_BITS + E::DEPTH * E::SIZE;
        let mode_code = match mode {
            32 => 1,
            39 => 8,
            48 => 9,
            56 => 10,
            _ => 0,
        };
        (mode_code << 60)
            | ((self.identity as usize) << 44)
            | self
                .where_the_frame_tracker_of_root_for_recycling_put
                .start()
    }

    pub fn is_address_in(addr: Address) -> AddressSpace {
        let top = E::top_address();
        let size = E::space_size();
        if addr < size {
            AddressSpace::User
        } else if addr <= top && addr > top - size {
            AddressSpace::Kernel
        } else {
            AddressSpace::Invalid
        }
    }

    pub fn fill<F: Into<FlagSet<PageEntryFlag>> + Copy>(
        &mut self,
        vpn: PageNumber,
        count: usize,
        flags: F,
    ) -> Result<usize, MemoryUnitError> {
        Self::map_internal(&mut self.root, vpn, None, count, flags, E::DEPTH - 1)
    }

    pub fn map<F: Into<FlagSet<PageEntryFlag>> + Copy>(
        &mut self,
        vpn: PageNumber,
        ppn: PageNumber,
        count: usize,
        flags: F,
    ) -> Result<usize, MemoryUnitError> {
        Self::map_internal(&mut self.root, vpn, Some(ppn), count, flags, E::DEPTH - 1)
    }

    pub fn free(&mut self, vpn: PageNumber, count: usize) -> Result<usize, MemoryUnitError> {
        Self::free_internal(&mut self.root, vpn, count, E::DEPTH - 1)
    }

    pub fn translate(&self, addr: Address) -> Option<(Address, FlagSet<MemoryRegionAttribute>)> {
        let offset = addr & 0xFFF;
        if let Some((ppn, attributes)) = self.locate(addr >> PAGE_BITS) {
            Some(((ppn << PAGE_BITS) + offset, attributes))
        } else {
            None
        }
    }

    fn free_internal(
        container: &mut PageTable<E>,
        vpn: PageNumber,
        count: usize,
        level: usize,
    ) -> Result<usize, MemoryUnitError> {
        let start = Self::index_of_vpn(vpn, level);
        if level == 0 {
            let mut freed = 0usize;
            for i in 0..count {
                if container.free_entry(start + i).is_some() {
                    freed += 1;
                }
            }
            Ok(freed)
        } else {
            let fake_end = Self::index_of_vpn(vpn + count, level);
            let end = if fake_end == 0 {
                let size = PageTable::<E>::entry_count();
                if start == 0 {
                    if count >= size.pow(level as u32) {
                        size
                    } else {
                        0
                    }
                } else {
                    size
                }
            } else {
                fake_end
            };
            if end > start {
                if Self::is_page_number_aligned_to(Some(vpn), level) {
                    let size = PageTable::<E>::entry_count().pow(level as u32);
                    let mut remaining = count;
                    let mut round = 0usize;
                    let round_space = PageTable::<E>::entry_count() - start;
                    let mut freed = 0usize;
                    while remaining >= size && round < round_space {
                        if container.is_table_created(start + round) {
                            let table = container
                                .get_table_mut(start + round)
                                .expect("there must be a table");
                            let offset = round * size;
                            Self::free_internal(table, vpn + offset, size, level - 1)
                                .map(|f| freed += f)?;
                        } else if container.free_entry(start + round).is_some() {
                            freed += 1;
                        }
                        remaining -= size;
                        round += 1;
                    }
                    if remaining > 0 {
                        Self::free_internal(container, vpn + (size * round), remaining, level)
                            .map(|f| freed + f)
                    } else {
                        Ok(freed)
                    }
                } else {
                    let size = PageTable::<E>::entry_count();
                    let max_remaining =
                        (size - Self::index_of_vpn(vpn, level - 1)) * size.pow(level as u32 - 1);
                    let remaining = if count > max_remaining {
                        max_remaining
                    } else {
                        count
                    };
                    let mut freed = 0usize;
                    if let Some(table) = container.get_table_mut(start) {
                        Self::free_internal(table, vpn, remaining, level - 1)
                            .map(|f| freed += f)?;
                        if count > max_remaining {
                            Self::free_internal(
                                container,
                                vpn + max_remaining,
                                count - max_remaining,
                                level,
                            )
                            .map(|f| freed + f)
                        } else {
                            Ok(freed)
                        }
                    } else if container.is_entry_created(start).is_some() {
                        // 是大页，不处理
                        // 这里可以返回错误
                        Ok(0)
                    } else {
                        // 没有大页也没有表，美哉
                        Ok(0)
                    }
                }
            } else {
                // _:[1]:123:../_:[1]:223:..
                // 对于不跨越的情况直接转发给 container[1].table() 处理
                if let Some(table) = container.get_table_mut(start) {
                    Self::free_internal(table, vpn, count, level - 1)
                } else if container.is_entry_created(start).is_some() {
                    // 是大页，不处理
                    // 这里可以返回错误
                    Ok(0)
                } else {
                    // 没有表也没有大页，美哉
                    Ok(0)
                }
            }
        }
    }

    fn map_internal<F: Into<FlagSet<PageEntryFlag>> + Copy>(
        container: &mut PageTable<E>,
        vpn: PageNumber,
        ppn: Option<PageNumber>,
        count: usize,
        flags: F,
        level: usize,
    ) -> Result<usize, MemoryUnitError> {
        // 标记 _:[N]:..
        // _ container 所在位置。递归不考虑外围循环，_ 之前可能也存在级别，但当前前轮递归无法考虑故省略
        // [N] 当前处理级别
        // .. 省略但存在可能多个级别
        let start = Self::index_of_vpn(vpn, level);
        if level == 0 {
            // ..:_:[123]/..:_:[223]
            // 不需要关心 count 是否存在溢出当前级别的问题。上级会处理好（且对 count 不溢出做检查和保证）。
            let mut mapped = 0usize;
            if let Some(number) = ppn {
                for i in 0..count {
                    Self::into_result(container.ensure_leaf_created(start + i, number + i, flags))?;
                }
            } else {
                for i in 0..count {
                    Self::into_result(container.ensure_managed_leaf_created(
                        start + i,
                        || {
                            mapped += 1;
                            frame::borrow(1)
                        },
                        flags,
                    ))?;
                }
            }
            Ok(mapped)
        } else {
            let fake_end = Self::index_of_vpn(vpn + count, level);
            let end = if fake_end == 0 {
                let size = PageTable::<E>::entry_count();
                if start == 0 {
                    if count >= size.pow(level as u32) {
                        size
                    } else {
                        0
                    }
                } else {
                    size
                }
            } else {
                fake_end
            };
            // ..:start:.. 到 ..:end:.. 可能跨越节也可能不跨越
            if end > start {
                // _:[2]:A:../_:[4]:B:..
                // 这里只取出第一段 [2] 并处理，剩下的段转发给下次同轮递归的这一分支
                if Self::is_page_number_aligned_to(Some(vpn), level)
                    && Self::is_page_number_aligned_to(ppn, level)
                {
                    // _:[2]:00/_:[4]:B
                    // 超级页的包含的最小页数量
                    let size = PageTable::<E>::entry_count().pow(level as u32);
                    let mut remaining = count;
                    let mut round = 0usize;
                    let round_space = PageTable::<E>::entry_count() - start;
                    // start + round 段可以成为超级页
                    let mut mapped = 0usize;
                    while remaining >= size && round < round_space {
                        if container.is_table_created(start + round) {
                            // 已经有一个页表了，那就转发给下一级
                            let table = container
                                .get_table_mut(start + round)
                                .expect("there must be a table");
                            let offset = round * size;
                            Self::map_internal(
                                table,
                                vpn + offset,
                                if let Some(number) = ppn {
                                    Some(number + offset)
                                } else {
                                    None
                                },
                                size,
                                flags,
                                level - 1,
                            )
                            .map(|f| mapped += f)?;
                        } else {
                            if let Some(number) = ppn {
                                Self::into_result(
                                    container
                                        .create_leaf(start + round, number + (round * size), flags)
                                        .map(|_| true),
                                )?;
                            } else {
                                Self::into_result(container.ensure_managed_leaf_created(
                                    start + round,
                                    || {
                                        mapped += size;
                                        frame::borrow(size)
                                    },
                                    flags,
                                ))?;
                            }
                        }
                        remaining -= size;
                        round += 1;
                    }
                    if remaining > 0 {
                        let next = if let Some(number) = ppn {
                            Some(number + (size * round))
                        } else {
                            None
                        };
                        Self::map_internal(
                            container,
                            vpn + (size * round),
                            next,
                            remaining,
                            flags,
                            level,
                        )
                        .map(|f| mapped + f)
                    } else {
                        // 没有剩下！
                        Ok(mapped)
                    }
                } else {
                    // _:[2]:3:../_:[4]:13:..
                    // [2] 成为表转发给次级
                    let size = PageTable::<E>::entry_count();
                    let max_remaining =
                        (size - Self::index_of_vpn(vpn, level - 1)) * size.pow(level as u32 - 1);
                    let remaining = if count > max_remaining {
                        max_remaining
                    } else {
                        count
                    };
                    let mut mapped = 0usize;
                    match container.ensure_table_created(start, || {
                        mapped += 1;
                        frame::borrow(1)
                    }) {
                        Ok(table) => {
                            Self::map_internal(table, vpn, ppn, remaining, flags, level - 1)
                                .map(|f| mapped += f)?;
                            if count > max_remaining {
                                // vpn + remaining = _:[3]:00
                                // 虽然末尾都是0但ppn依旧无法对齐，不是超级页，且接下来都不是超级页
                                let next = if let Some(number) = ppn {
                                    Some(number + max_remaining)
                                } else {
                                    None
                                };
                                Self::map_internal(
                                    container,
                                    vpn + max_remaining,
                                    next,
                                    count - max_remaining,
                                    flags,
                                    level,
                                )
                                .map(|f| mapped + f)
                            } else {
                                // 没有剩下！
                                Ok(mapped)
                            }
                        }
                        // 有表会进入 Ok 不会来到 Err。有大页但 ppn 肯定没对齐，没法打散映射，直接报错
                        Err(PageEntryWriteError::BranchExists)
                        | Err(PageEntryWriteError::LeafExists(_, _)) => {
                            Err(MemoryUnitError::EntryOverwrite)
                        }
                        Err(PageEntryWriteError::TrackerUnavailable) => {
                            Err(MemoryUnitError::RanOutOfFrames)
                        }
                    }
                }
            } else {
                // _:[1]:123:../_:[1]:223:..
                // 对于不跨越的情况直接转发给 container[1].table() 处理
                let mut mapped = 0usize;
                match container.ensure_table_created(start, || {
                    mapped += 1;
                    frame::borrow(1)
                }) {
                    Ok(table) => Self::map_internal(table, vpn, ppn, count, flags, level - 1)
                        .map(|f| mapped + f),
                    Err(PageEntryWriteError::LeafExists(o, p)) => {
                        // container[start] 是大页
                        let set = flags.into();
                        if Self::is_flags_extended(&p, &set) {
                            if let Some(number) = ppn {
                                if number == o {
                                    let (table, created) = Self::split_page_into_table(
                                        container,
                                        vpn,
                                        ppn,
                                        PageTable::<E>::entry_count(),
                                        level,
                                        p,
                                    )?;
                                    mapped += created;
                                    Self::map_internal(table, vpn, ppn, count, flags, level - 1)
                                        .map(|f| mapped + f)
                                } else {
                                    // 映射要求不匹配，无法覆盖
                                    Err(MemoryUnitError::EntryOverwrite)
                                }
                            } else {
                                let (table, created) = Self::split_page_into_table(
                                    container,
                                    vpn,
                                    ppn,
                                    PageTable::<E>::entry_count(),
                                    level,
                                    p,
                                )?;
                                mapped += created;
                                Self::map_internal(table, vpn, ppn, count, flags, level - 1)
                                    .map(|f| mapped + f)
                            }
                        } else {
                            Ok(mapped)
                        }
                    }
                    Err(PageEntryWriteError::BranchExists) => Err(MemoryUnitError::EntryOverwrite),
                    Err(PageEntryWriteError::TrackerUnavailable) => {
                        Err(MemoryUnitError::RanOutOfFrames)
                    }
                }
            }
        }
    }

    fn locate(&self, vpn: PageNumber) -> Option<(PageNumber, FlagSet<MemoryRegionAttribute>)> {
        Self::locate_internal(&self.root, vpn, E::DEPTH - 1)
    }

    fn locate_internal(
        container: &PageTable<E>,
        vpn: PageNumber,
        level: usize,
    ) -> Option<(PageNumber, FlagSet<MemoryRegionAttribute>)> {
        let index = Self::index_of_vpn(vpn, level);
        match container.get_entry_type(index) {
            PageEntryType::Invalid => None,
            PageEntryType::Leaf(number, flags) => {
                let mut attr: FlagSet<MemoryRegionAttribute> = MemoryRegionAttribute::None.into();
                if flags.contains(PageEntryFlag::Executable) {
                    attr |= MemoryRegionAttribute::Execute;
                }
                if flags.contains(PageEntryFlag::Writeable) {
                    attr |= MemoryRegionAttribute::Write;
                }
                if flags.contains(PageEntryFlag::Readable) {
                    attr |= MemoryRegionAttribute::Read;
                }
                Some((number + Self::offset_of_vpn(vpn, level), attr))
            }
            PageEntryType::Branch(table) => Self::locate_internal(table, vpn, level - 1),
        }
    }

    fn split_page_into_table<F: Into<FlagSet<PageEntryFlag>> + Copy>(
        container: &mut PageTable<E>,
        vpn: PageNumber,
        ppn: Option<PageNumber>,
        count: usize,
        level: usize,
        original: F,
    ) -> Result<(&mut PageTable<E>, usize), MemoryUnitError> {
        let index = Self::index_of_vpn(vpn, level);
        let mut mapped = 0usize;
        container.free_entry(index);
        if let Ok(table) = container.ensure_table_created(index, || {
            mapped += 1;
            frame::borrow(1)
        }) {
            let small_size = PageTable::<E>::entry_count().pow((level - 1) as u32);
            if let Some(number) = ppn {
                for i in 0..count {
                    Self::into_result(
                        table
                            .create_leaf(i, number + small_size * i, original)
                            .map(|_| true),
                    )?;
                }
            } else {
                for i in 0..count {
                    Self::into_result(table.ensure_managed_leaf_created(
                        i,
                        || {
                            mapped += small_size;
                            frame::borrow(small_size)
                        },
                        original,
                    ))?;
                }
            }
            Ok((table, mapped))
        } else {
            Err(MemoryUnitError::RanOutOfFrames)
        }
    }

    fn is_flags_extended(
        original: &FlagSet<PageEntryFlag>,
        may_extended: &FlagSet<PageEntryFlag>,
    ) -> bool {
        if may_extended.contains(PageEntryFlag::Readable)
            & !original.contains(PageEntryFlag::Readable)
        {
            return true;
        }
        if may_extended.contains(PageEntryFlag::Writeable)
            & !original.contains(PageEntryFlag::Writeable)
        {
            return true;
        }
        if may_extended.contains(PageEntryFlag::Executable)
            & !original.contains(PageEntryFlag::Executable)
        {
            return true;
        }
        false
    }

    fn index_of_vpn(vpn: PageNumber, level: usize) -> usize {
        // 9|9|9
        (vpn >> (level * E::SIZE)) & ((1 << E::SIZE) - 1)
    }

    fn offset_of_vpn(vpn: PageNumber, level: usize) -> usize {
        // 9|9|9
        vpn & ((1 << (level * E::SIZE)) - 1)
    }

    fn is_page_number_aligned_to(number: Option<PageNumber>, level: usize) -> bool {
        if let Some(inner) = number {
            inner & ((1 << (level * E::SIZE)) - 1) == 0
        } else {
            true
        }
    }

    fn into_result(input: Result<bool, PageEntryWriteError>) -> Result<bool, MemoryUnitError> {
        match input {
            Err(err) => match err {
                PageEntryWriteError::BranchExists | PageEntryWriteError::LeafExists(_, _) => {
                    Err(MemoryUnitError::EntryOverwrite)
                }
                PageEntryWriteError::TrackerUnavailable => Err(MemoryUnitError::RanOutOfFrames),
            },
            Ok(i) => Ok(i),
        }
    }
}

impl<E: PageTableEntry + Sized + 'static> Display for MemoryUnit<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(
            f,
            "Memory Mapping at {:#x}\n     Visual    ->    Physical   (  Length  )=0bDAGUXWRV",
            self.where_the_frame_tracker_of_root_for_recycling_put
                .start()
                << PAGE_BITS
        )?;
        let highest = 1 << (E::SIZE * E::DEPTH - 1);
        let upper_bits = (E::top_address() >> PAGE_BITS) - (highest - 1);
        let vpn_fmt: fn(Address, usize, usize) -> Address = |x, h, u| {
            if x & h != 0 {
                u | x
            } else {
                x
            }
        };
        let mut start_v = 0usize;
        let mut start_p = 0usize;
        let mut aggregated = 0usize;
        let mut bits = 0u64;
        let mut dirty = false;
        for (vpn, ppn, count, flags) in &self.root {
            let flag_bits = flags.bits();
            if start_v + aggregated == vpn && start_p + aggregated == ppn && flag_bits == bits {
                aggregated += count;
                dirty = true;
            } else {
                if dirty {
                    writeln!(
                        f,
                        "{:#015x}->{:#015x}({:#010x})={:#010b}",
                        vpn_fmt(start_v, highest, upper_bits),
                        start_p,
                        aggregated,
                        bits
                    )?;
                }
                start_v = vpn;
                start_p = ppn;
                aggregated = count;
                bits = flag_bits;
                dirty = true;
            }
        }
        if dirty {
            write!(
                f,
                "{:#015x}->{:#015x}({:#010x})={:#010b}",
                vpn_fmt(start_v, highest, upper_bits),
                start_p,
                aggregated,
                bits
            )?;
        }
        Ok(())
    }
}
