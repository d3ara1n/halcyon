use erhino_shared::PageNumber;
use flagset::FlagSet;

use crate::println;

use super::{
    frame::frame_alloc,
    page::{PageLevel, PageTable, PageTableEntryFlag, PageTableError},
};

pub struct MemoryUnit<'root> {
    root: PageTable<'root>,
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum MemoryUnitError {
    PageNumberOutOfBound,
    RanOutOfFrames,
}

impl<'root> MemoryUnit<'root> {
    pub fn new() -> Self {
        Self {
            root: PageTable::new(frame_alloc(1).unwrap() as u64, PageLevel::Giga),
        }
    }

    pub fn map<F: Into<FlagSet<PageTableEntryFlag>> + Copy>(
        &mut self,
        vpn: PageNumber,
        ppn: PageNumber,
        count: usize,
        max_level: PageLevel,
        flags: F,
    ) -> Result<(), MemoryUnitError> {
        // 写法注意 u64 溢出以及运算中不能有(小-大)
        let start = vpn;
        let end = vpn + count as u64;
        if max_level == PageLevel::Kilo {
            for i in 0u64..count as u64 {
                self.map_one(vpn + i, ppn + i, PageLevel::Kilo, flags)?;
            }
        } else {
            // 保证 end >= r; l >= start
            let r = max_level.floor(end);
            let l = if start == max_level.floor(start) {
                start
            } else {
                max_level.ceil(start)
            };
            if r >= l {
                // 保证 start <= l <= r <= end
                // r..end 段
                if end != r {
                    self.map(
                        r,
                        r - start + ppn,
                        (end - r) as usize,
                        max_level.next_level().unwrap(),
                        flags,
                    )?;
                }
                if r != l {
                    // l..r 段，这一部分是对齐的
                    let mid_count = max_level.measure((r - l) as usize) as u64;
                    for i in 0u64..mid_count {
                        self.map_one(
                            l + max_level.shift(i),
                            l - start + ppn + max_level.shift(i),
                            max_level,
                            flags,
                        )?;
                    }
                }
                if l != start {
                    self.map(
                        start,
                        ppn,
                        (l - start) as usize,
                        max_level.next_level().unwrap(),
                        flags,
                    )?;
                }
            } else {
                self.map(vpn, ppn, count, max_level.next_level().unwrap(), flags)?;
            }
        }
        Ok(())
    }

    pub fn map_one<F: Into<FlagSet<PageTableEntryFlag>>>(
        &mut self,
        vpn: PageNumber,
        ppn: PageNumber,
        level: PageLevel,
        flags: F,
    ) -> Result<(), MemoryUnitError> {
        Self::map_one_internal(vpn, ppn, level, flags, PageLevel::Giga, &mut self.root)
    }

    fn map_one_internal<F: Into<FlagSet<PageTableEntryFlag>>>(
        vpn: PageNumber,
        ppn: PageNumber,
        target_level: PageLevel,
        flags: F,
        current_level: PageLevel,
        root: &mut PageTable,
    ) -> Result<(), MemoryUnitError> {
        let index = current_level.extract(vpn);
        if let Some(entry) = root.entry_mut(index) {
            if target_level == current_level {
                entry.set(ppn, 0, flags);
                Ok(())
            } else {
                if let Some(frame) = frame_alloc(1) {
                    let mut table = entry.set_as_page_table(frame, current_level);
                    Self::map_one_internal(
                        vpn,
                        ppn,
                        target_level,
                        flags,
                        current_level.next_level().unwrap(),
                        &mut table,
                    )
                } else {
                    Err(MemoryUnitError::RanOutOfFrames)
                }
            }
        } else {
            Err(MemoryUnitError::PageNumberOutOfBound)
        }
    }

    pub fn write<F: Into<FlagSet<PageTableEntryFlag>>>(
        &mut self,
        vpn: PageNumber,
        data: &[u8],
        count: usize,
        flags: F,
    ) {
        //
    }

    pub fn unmap(&'root mut self, vpn: PageNumber) -> Result<(), MemoryUnitError> {
        todo!();
    }
}
