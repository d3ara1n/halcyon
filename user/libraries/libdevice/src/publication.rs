//! Registrar 单调登记账：高水位与仍存活的关系由一个 owner 保存。

use libbudget::{AccountView, Charge};
use ordered_table::{OrderedTable, PreparedEntry};

use crate::{Error, resource::DeviceResource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Present,
    Withdrawn,
}

pub struct Publication<T> {
    presence: Presence,
    value: T,
    // value 先析构，再归还它的记录与节点费用；额外动态存储由 T 自身付款。
    _unit: Charge,
    _bytes: Charge,
}

impl<T> Publication<T> {
    pub fn presence(&self) -> Presence {
        self.presence
    }

    pub fn value(&self) -> &T {
        &self.value
    }
}

pub struct PreparedPublication<T> {
    context: u64,
    entry: PreparedEntry<Publication<T>, ()>,
}

pub struct Failure<T> {
    pub error: Error,
    pub prepared: PreparedPublication<T>,
}

/// 每个实际 Registrar context 一份。调用者仅在 seq 确定或撤出后发行下一轮；
/// 同一 context 的多份 sender 别名共享此账，不能各自重新从 1 开始。
pub struct Publications<T> {
    context: u64,
    highest: u64,
    sealed: bool,
    entries: OrderedTable<Publication<T>, ()>,
}

impl<T> Publications<T> {
    pub fn new(authenticated_context: u64, limit: usize) -> Result<Self, Error> {
        if authenticated_context == 0 || limit == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            context: authenticated_context,
            highest: 0,
            sealed: false,
            entries: OrderedTable::new(limit),
        })
    }

    pub fn highest(&self) -> u64 {
        self.highest
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    pub fn get(&self, seq: u64) -> Result<&Publication<T>, Error> {
        if seq == 0 || seq > self.highest {
            return Err(Error::Invalid);
        }
        self.entries.get(seq).ok_or(Error::Closed)
    }

    /// 不分配、不改变高水位。重复 Register 由调用者比较完整静态输入和实际
    /// Provision endpoint 身份，再返回此记录的原控制别名；不能只比较 seq。
    pub fn existing(&self, seq: u64) -> Result<Option<&T>, Error> {
        if let Some(entry) = self.entries.get(seq) {
            return if entry.presence == Presence::Present && !self.sealed {
                Ok(Some(&entry.value))
            } else {
                Err(Error::Closed)
            };
        }
        self.check_new(seq)?;
        Ok(None)
    }

    fn check_new(&self, seq: u64) -> Result<(), Error> {
        if seq == 0 {
            return Err(Error::Invalid);
        }
        if seq <= self.highest || self.sealed {
            return Err(Error::Closed);
        }
        if self.highest.checked_add(1) != Some(seq) {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    /// 名字、父关系、范围和静态输入验证由登记 backing 完成；
    /// 预备节点尚不发布，Quota/分配失败不消费 seq。
    pub fn prepare(
        &self,
        seq: u64,
        value: T,
        account: &AccountView<DeviceResource>,
    ) -> Result<PreparedPublication<T>, Error> {
        self.check_new(seq)?;
        let unit = account.acquire(DeviceResource::Publication, 1)?;
        let bytes = account.acquire(
            DeviceResource::Bytes,
            PreparedEntry::<Publication<T>, ()>::allocation_bytes(),
        )?;
        let entry = self
            .entries
            .prepare_insert(
                seq,
                Publication {
                    presence: Presence::Present,
                    value,
                    _unit: unit,
                    _bytes: bytes,
                },
                &libbudget::AllocatorBacked,
            )
            .map_err(|e| match e {
                ordered_table::InsertError::Limit(_) => Error::Quota,
                ordered_table::InsertError::Storage(e, _) => libbudget::allocator_error(e).into(),
            })?;
        Ok(PreparedPublication {
            context: self.context,
            entry,
        })
    }

    pub fn commit(&mut self, prepared: PreparedPublication<T>) -> Result<(), Failure<T>> {
        let seq = prepared.entry.key();
        let checked = if prepared.context != self.context {
            Err(Error::Permission)
        } else {
            self.check_new(seq)
        }
        .and_then(|()| {
            if self.entries.has_room() {
                Ok(())
            } else {
                Err(Error::Quota)
            }
        });
        if let Err(error) = checked {
            return Err(Failure { error, prepared });
        }
        self.highest = seq;
        self.entries.insert_prepared(prepared.entry);
        Ok(())
    }

    /// 可以先于 Register 关闭恰好下一轮；不分配、不依赖业务额度。
    /// 返回仍存活的原记录，供稳定清理 owner 停止实际 Binding。
    pub fn withdraw(&mut self, seq: u64) -> Result<Option<&T>, Error> {
        if seq == 0 {
            return Err(Error::Invalid);
        }
        if seq > self.highest {
            if self.highest.checked_add(1) != Some(seq) {
                return Err(Error::Invalid);
            }
            self.highest = seq;
        }
        Ok(self.entries.get_mut(seq).map(|entry| {
            entry.presence = Presence::Withdrawn;
            &entry.value
        }))
    }

    /// 封口即时禁止准入，清理游标按 seq 逐项走；不在这里递归遍历。
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    pub fn next(&self, after: u64) -> Option<(u64, &Publication<T>)> {
        self.entries
            .next_after(Some(&after))
            .map(|(seq, entry)| (*seq, entry))
    }

    /// 只在绑定、迟到任务及已发行控制壳全部退休后摘记录；
    /// 该操作不是 Withdraw，也不负责硬件停止。
    pub fn remove_retired(&mut self, seq: u64) -> Result<Publication<T>, Error> {
        let entry = self.get(seq)?;
        if entry.presence != Presence::Withdrawn {
            return Err(Error::NotReady);
        }
        // 以上同一 &mut self 中已经确认存在，删除不能被并发打断。
        let entry = self.entries.remove(seq).expect("publication disappeared");
        Ok(entry.into_value())
    }
}
