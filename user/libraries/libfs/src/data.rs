//! 稀疏分块流数据；一次定位写预留最多消息范围覆盖的块，Commit 不分配。

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    mem::ManuallyDrop,
    sync::atomic::{AtomicUsize, Ordering},
};

static ABANDONED_BLOCKS: AtomicUsize = AtomicUsize::new(0);
use crate::resource::FsResource;
use erhino_shared::{call::SystemCallError, message::PAYLOAD_MAX};
use libbudget::{AccountView, Charge};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

pub const BLOCK_BYTES: usize = 4096;
struct Block {
    bytes: Box<[u8; BLOCK_BYTES]>,
    slot: Option<Permit>,
    _charge: Charge,
}

pub struct Data {
    blocks: ManuallyDrop<OrderedTable<Block, ()>>,
    slots: Arc<Counter>,
    length: u64,
    version: u64,
    trimming: Option<Trimming>,
}
struct Trimming {
    first_discarded_block: u64,
    cursor: Option<u64>,
}
pub struct PreparedLength {
    expected_version: u64,
    next_version: u64,
    length: u64,
}

pub struct PreparedWrite {
    blocks: Vec<PreparedEntry<Block, ()>>,
    expected_version: u64,
    next_version: u64,
    length: u64,
}

impl Data {
    pub fn new(account: &AccountView<FsResource>) -> Result<Self, SystemCallError> {
        let capacity = (account.usage(FsResource::Bytes).1 / BLOCK_BYTES).max(1);
        Ok(Self {
            blocks: ManuallyDrop::new(OrderedTable::new(capacity)),
            slots: Arc::try_new(Counter::new(capacity))
                .map_err(|_| SystemCallError::OutOfMemory)?,
            length: 0,
            version: 1,
            trimming: None,
        })
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn is_trimming(&self) -> bool {
        self.trimming.is_some()
    }
    pub fn prepare_length(&self, length: u64) -> Result<PreparedLength, SystemCallError> {
        if self.is_trimming() {
            return Err(SystemCallError::ObjectBusy);
        }
        Ok(PreparedLength {
            expected_version: self.version,
            next_version: self
                .version
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?,
            length,
        })
    }
    pub fn validates_length(&self, length: &PreparedLength) -> bool {
        self.version == length.expected_version && !self.is_trimming()
    }
    pub fn commit_length(&mut self, length: PreparedLength) {
        assert!(
            self.validates_length(&length),
            "stream length changed before validated commit"
        );
        if length.length < self.length {
            let block = length.length / BLOCK_BYTES as u64;
            let within = (length.length % BLOCK_BYTES as u64) as usize;
            if within != 0
                && let Some(last) = self.blocks.get_mut(block)
            {
                // 一次提交最多清零最后保留块的尾部；增长不能复活截掉的旧字节。
                last.bytes[within..].fill(0);
            }
            let first = block + u64::from(within != 0);
            let cursor = first.checked_sub(1);
            if self.blocks.next_after::<u64>(cursor.as_ref()).is_some() {
                self.trimming = Some(Trimming {
                    first_discarded_block: first,
                    cursor,
                });
            }
        }
        self.length = length.length;
        self.version = length.next_version;
    }
    /// 每项实际删除才释放块/permit/Charge；清理责任留在 Data，不依赖调用者。
    pub fn trim_step(&mut self, budget: usize) -> usize {
        let mut work = 0;
        while work < budget {
            let Some(trimming) = self.trimming.as_mut() else {
                break;
            };
            let Some((&key, _)) = self.blocks.next_after::<u64>(trimming.cursor.as_ref()) else {
                self.trimming = None;
                break;
            };
            assert!(
                key >= trimming.first_discarded_block,
                "trimming selected a visible block"
            );
            trimming.cursor = Some(key);
            let _ = self
                .blocks
                .remove(key)
                .map(ordered_table::PreparedEntry::into_value);
            work += 1;
        }
        if self.trimming.as_ref().is_some_and(|trimming| {
            self.blocks
                .next_after::<u64>(trimming.cursor.as_ref())
                .is_none()
        }) {
            self.trimming = None;
        }
        work
    }

    pub fn read(&self, offset: u64, out: &mut [u8]) -> usize {
        let actual = self.length.saturating_sub(offset).min(out.len() as u64) as usize;
        let mut done = 0;
        while done < actual {
            let position = offset + done as u64;
            let key = position / BLOCK_BYTES as u64;
            let within = (position % BLOCK_BYTES as u64) as usize;
            let size = (BLOCK_BYTES - within).min(actual - done);
            let target = &mut out[done..done + size];
            if let Some(block) = self.blocks.get(key) {
                target.copy_from_slice(&block.bytes[within..within + size]);
            } else {
                target.fill(0);
            }
            done += size;
        }
        actual
    }

    pub fn prepare_write(
        &self,
        offset: u64,
        input: &[u8],
        account: &AccountView<FsResource>,
    ) -> Result<PreparedWrite, SystemCallError> {
        if self.is_trimming() {
            return Err(SystemCallError::ObjectBusy);
        }
        if input.len() > PAYLOAD_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
        let end = offset
            .checked_add(input.len() as u64)
            .ok_or(SystemCallError::IllegalArgument)?;
        let next_version = self
            .version
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        let count = if input.is_empty() {
            0
        } else {
            ((end - 1) / BLOCK_BYTES as u64 - offset / BLOCK_BYTES as u64 + 1) as usize
        };
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact(count)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let mut done = 0;
        while done < input.len() {
            let position = offset + done as u64;
            let key = position / BLOCK_BYTES as u64;
            let within = (position % BLOCK_BYTES as u64) as usize;
            let size = (BLOCK_BYTES - within).min(input.len() - done);
            let charge = account.acquire(
                FsResource::Bytes,
                BLOCK_BYTES + PreparedEntry::<Block, ()>::allocation_bytes(),
            )?;
            let old = self.blocks.get(key);
            let slot = if old.is_none() {
                Some(
                    Counter::try_acquire(&self.slots)
                        .map_err(|_| SystemCallError::QuotaExceeded)?,
                )
            } else {
                None
            };
            let allocation = Box::<[u8; BLOCK_BYTES]>::try_new_zeroed()
                .map_err(|_| SystemCallError::OutOfMemory)?;
            // SAFETY: allocation 由 try_new_zeroed 完整初始化；全零对 u8 数组有效。
            let mut bytes = unsafe { allocation.assume_init() };
            if let Some(old) = old {
                bytes.copy_from_slice(old.bytes.as_ref());
            }
            bytes[within..within + size].copy_from_slice(&input[done..done + size]);
            let block = Block {
                bytes,
                slot,
                _charge: charge,
            };
            blocks.push(
                self.blocks
                    .prepare_insert_candidate(key, block, &libbudget::AllocatorBacked)
                    .map_err(|error| match error {
                        ordered_table::InsertError::Limit(_) => SystemCallError::QuotaExceeded,
                        ordered_table::InsertError::Storage(error, _) => {
                            libbudget::allocator_error(error)
                        }
                    })?,
            );
            done += size;
        }
        Ok(PreparedWrite {
            blocks,
            expected_version: self.version,
            next_version,
            length: if input.is_empty() {
                self.length
            } else {
                self.length.max(end)
            },
        })
    }

    pub fn validates(&self, write: &PreparedWrite) -> bool {
        self.version == write.expected_version && !self.is_trimming()
    }
    pub fn commit(&mut self, write: PreparedWrite) {
        assert!(
            self.validates(&write),
            "stream data changed before validated commit"
        );
        for mut block in write.blocks {
            let key = block.key();
            if let Some(mut old) = self
                .blocks
                .remove(key)
                .map(ordered_table::PreparedEntry::into_value)
            {
                block.value_mut().slot = old.slot.take();
            }
            self.blocks.insert_prepared(block);
        }
        self.length = write.length;
        self.version = write.next_version;
    }

    pub fn retire_step(&mut self, budget: usize) -> usize {
        let mut done = 0;
        while done < budget {
            let Some((&key, _)) = self.blocks.next_after::<u64>(None) else {
                break;
            };
            let _ = self
                .blocks
                .remove(key)
                .map(ordered_table::PreparedEntry::into_value);
            done += 1;
        }
        if self.blocks.is_empty() {
            self.length = 0;
            self.trimming = None;
        }
        done
    }
    pub fn retired(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        if self.blocks.is_empty() {
            // SAFETY: 该 ManuallyDrop 表只在这里手工析构一次；空表没有业务块。
            unsafe {
                ManuallyDrop::drop(&mut self.blocks);
            }
        } else {
            ABANDONED_BLOCKS.fetch_add(self.blocks.len(), Ordering::Relaxed);
        }
    }
}

pub fn abandoned_blocks() -> usize {
    ABANDONED_BLOCKS.load(Ordering::Relaxed)
}
