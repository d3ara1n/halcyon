//! 首批 boot 配对来源。声明与对应只读映像在同次出生委托中交付。
use libdevice::Error;
use librpc::bytes::{Reader, Writer};

pub const BOOTSTRAP: usize = 0;
pub const POOL: usize = 1;
pub const DRIVER_JOB: usize = 2;
pub const CLAIM_SOURCE: usize = 3;
pub const IMAGE_BASE: usize = 4;
pub const VERSION: u32 = 1;
pub const HEADER_BYTES: usize = 8;
pub const MANAGER_READY_CONTROL: usize = 0;
pub const MANAGER_READY_OBSERVER: usize = 1;
pub const MANAGER_READY_SLOTS: usize = 2;
pub const MANAGER_READY_BYTES: usize = 16;

/// 两队列的真实Mailbox KOID，与两个实际sender槽逐项核对；数字不授予权限。
#[derive(Clone, Copy)]
pub struct ManagerReady {
    pub control: u64,
    pub observer: u64,
}
impl ManagerReady {
    pub fn encode(self) -> Result<[u8; MANAGER_READY_BYTES], Error> {
        if self.control == 0 || self.observer == 0 || self.control == self.observer {
            return Err(Error::Invalid);
        }
        let mut out = [0; MANAGER_READY_BYTES];
        let mut w = Writer::new(&mut out);
        w.u64(self.control);
        w.u64(self.observer);
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        let info = Self {
            control: r.u64()?,
            observer: r.u64()?,
        };
        r.finish()?;
        info.encode()?;
        Ok(info)
    }
}

/// 首批执行者的实际Pool配置：ABI主栈之外8 MiB映像/堆/执行存储，1 MiB内核元数据。
pub const WORKER_POOL: erhino_shared::memory_pool::MemoryPoolBudget =
    erhino_shared::memory_pool::MemoryPoolBudget {
        pages: ((erhino_shared::proc::PROCESS_MAIN_STACK_SIZE + 8 * 1024 * 1024)
            / erhino_shared::proc::PROCESS_PAGE_SIZE) as u64,
        metadata: 1024 * 1024 / erhino_shared::memory_pool::METADATA_SLOT_BYTES,
    };

/// 首批只使用有限的静态 identity 精确匹配；identity 的业务字段由 PCI/USB 等领域拥有。
#[derive(Clone, Copy, Debug)]
pub struct Declaration<'a> {
    pub key: &'a str,
    pub identity: &'a [u8],
    pub priority: u32,
    /// 每个可复用进程可承载的绑定数，不是全实现的进程/Run上限。
    pub capacity: u32,
    /// 实际装配是否允许多个绑定复用同一进程；与物理信任/子登记授权独立。
    pub reusable: bool,
    /// init 的真实装配政策，不从程序自报数据、匹配字符串或路径授予。
    pub child_registrar: bool,
    pub trusted_physical: bool,
    pub image_bytes: u64,
}
impl Declaration<'_> {
    fn validate(&self) -> Result<(), Error> {
        if self.key.is_empty()
            || self.key.len() > crate::view::MAX_IMPLEMENTATION_KEY
            || self.identity.is_empty()
            || self.capacity == 0
            || self.image_bytes == 0
        {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }
}
pub fn encoded_bytes(declarations: &[Declaration<'_>]) -> Result<usize, Error> {
    declarations
        .iter()
        .try_fold(HEADER_BYTES, |length, declaration| {
            declaration.validate()?;
            length
                .checked_add(32)
                .and_then(|n| n.checked_add(declaration.key.len()))
                .and_then(|n| n.checked_add(declaration.identity.len()))
                .ok_or(Error::Invalid)
        })
}
pub fn encode(declarations: &[Declaration<'_>], out: &mut [u8]) -> Result<usize, Error> {
    let length = encoded_bytes(declarations)?;
    if length > out.len() {
        return Err(Error::Invalid);
    }
    let mut writer = Writer::new(out);
    writer.u32(VERSION);
    writer.u32(u32::try_from(declarations.len()).map_err(|_| Error::Invalid)?);
    for declaration in declarations {
        writer.bytes32(declaration.key.as_bytes());
        writer.bytes32(declaration.identity);
        writer.u32(declaration.priority);
        writer.u32(declaration.capacity);
        writer.u32(
            u32::from(declaration.child_registrar)
                | (u32::from(declaration.trusted_physical) << 1)
                | (u32::from(declaration.reusable) << 2),
        );
        writer.u32(0);
        writer.u64(declaration.image_bytes);
    }
    Ok(writer.written())
}
pub struct Catalog<'a> {
    bytes: &'a [u8],
    count: usize,
}
impl<'a> Catalog<'a> {
    pub fn decode(bytes: &'a [u8], grants: usize) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        if reader.u32()? != VERSION {
            return Err(Error::Invalid);
        }
        let count = reader.u32()? as usize;
        if count.checked_add(IMAGE_BASE) != Some(grants) {
            return Err(Error::Invalid);
        }
        let value = Self {
            bytes: &bytes[HEADER_BYTES..],
            count,
        };
        let mut records = value.records();
        for index in 0..count {
            let record = records.next().ok_or(Error::Invalid)??;
            record.validate()?;
            // 支持声明和映像的一一身份不能被同名后项覆盖。
            for previous in value.records().take(index) {
                if previous?.key == record.key {
                    return Err(Error::Conflict);
                }
            }
        }
        if records.reader.remaining() != 0 {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn records(&self) -> Records<'a> {
        Records {
            reader: Reader::new(self.bytes),
            remaining: self.count,
        }
    }
    pub fn image_slot(&self, index: usize) -> Option<usize> {
        (index < self.count)
            .then(|| IMAGE_BASE.checked_add(index))
            .flatten()
    }
    /// 同一优先级的多个匹配项明确 Ambiguous；不通过进程启动/Claim 竞速选择。
    pub fn select(&self, identity: &[u8], previous: Option<usize>) -> Result<Option<usize>, Error> {
        let ceiling = previous
            .map(|index| {
                let record = self.records().nth(index).ok_or(Error::Invalid)??;
                if record.identity != identity {
                    return Err(Error::Invalid);
                }
                Ok(record.priority)
            })
            .transpose()?;
        let mut selected = None;
        let mut priority = 0;
        let mut ambiguous = false;
        for (index, record) in self.records().enumerate() {
            let record = record?;
            if record.identity != identity
                || ceiling.is_some_and(|ceiling| record.priority >= ceiling)
            {
                continue;
            }
            if selected.is_none() || record.priority > priority {
                selected = Some(index);
                priority = record.priority;
                ambiguous = false;
            } else if record.priority == priority {
                ambiguous = true;
            }
        }
        if ambiguous {
            Err(Error::Conflict)
        } else {
            Ok(selected)
        }
    }
}
pub struct Records<'a> {
    reader: Reader<'a>,
    remaining: usize,
}
impl<'a> Iterator for Records<'a> {
    type Item = Result<Declaration<'a>, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some((|| {
            let key = self.reader.text32()?;
            let identity = self.reader.bytes32()?;
            let priority = self.reader.u32()?;
            let capacity = self.reader.u32()?;
            let flags = self.reader.u32()?;
            if flags & !7 != 0 || self.reader.u32()? != 0 {
                return Err(Error::Invalid);
            }
            Ok(Declaration {
                key,
                identity,
                priority,
                capacity,
                reusable: flags & 4 != 0,
                child_registrar: flags & 1 != 0,
                trusted_physical: flags & 2 != 0,
                image_bytes: self.reader.u64()?,
            })
        })())
    }
}

/// 具体worker出生配置；实际授权仍只来自同次grant与后续认证Activate。
#[derive(Clone, Copy)]
pub struct WorkerConfig {
    pub capacity: u32,
}
impl WorkerConfig {
    pub const BYTES: usize = 16;
    pub fn encode(self) -> Result<[u8; Self::BYTES], Error> {
        if self.capacity == 0 {
            return Err(Error::Invalid);
        }
        let mut bytes = [0; Self::BYTES];
        bytes[..8].copy_from_slice(b"DRVCONF1");
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..].copy_from_slice(&self.capacity.to_le_bytes());
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::BYTES
            || &bytes[..8] != b"DRVCONF1"
            || bytes[8..12] != VERSION.to_le_bytes()
        {
            return Err(Error::Invalid);
        }
        let capacity = u32::from_le_bytes(bytes[12..].try_into().map_err(|_| Error::Invalid)?);
        if capacity == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self { capacity })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 选择依据是明确优先级/歧义规则；映像槽数来自真实出生委托，不能暗增候选。
    #[test]
    fn unique_priority_wins_and_equal_priority_is_ambiguous() {
        let first = Declaration {
            key: "one",
            identity: b"pci:0c0330",
            priority: 1,
            capacity: 1,
            reusable: false,
            child_registrar: true,
            trusted_physical: true,
            image_bytes: 64,
        };
        let mut second = Declaration {
            key: "two",
            priority: 2,
            ..first
        };
        let mut bytes = [0; 256];
        let used = encode(&[first, second], &mut bytes).unwrap();
        let catalog = Catalog::decode(&bytes[..used], IMAGE_BASE + 2).unwrap();
        assert_eq!(catalog.select(b"pci:0c0330", None), Ok(Some(1)));
        assert_eq!(catalog.select(b"usb:03/01/01", None), Ok(None));
        assert!(Catalog::decode(&bytes[..used], IMAGE_BASE + 3).is_err());
        second.priority = 1;
        let used = encode(&[first, second], &mut bytes).unwrap();
        assert_eq!(
            Catalog::decode(&bytes[..used], IMAGE_BASE + 2)
                .unwrap()
                .select(b"pci:0c0330", None),
            Err(Error::Conflict)
        );
        second.key = first.key;
        let used = encode(&[first, second], &mut bytes).unwrap();
        assert!(matches!(
            Catalog::decode(&bytes[..used], IMAGE_BASE + 2),
            Err(Error::Conflict)
        ));
    }

    // 降序和下层歧义来自选择合同；原游标不能越界或跨identity，也不能回到高优先级。
    #[test]
    fn finite_candidates_descend_and_lower_ties_remain_ambiguous() {
        let mut records = [
            Declaration {
                key: "top",
                identity: b"usb:kbd",
                priority: u32::MAX,
                capacity: 2,
                reusable: true,
                child_registrar: false,
                trusted_physical: false,
                image_bytes: 64,
            },
            Declaration {
                key: "low",
                identity: b"usb:kbd",
                priority: 1,
                capacity: 2,
                reusable: true,
                child_registrar: false,
                trusted_physical: false,
                image_bytes: 64,
            },
            Declaration {
                key: "middle",
                identity: b"usb:kbd",
                priority: 2,
                capacity: 2,
                reusable: true,
                child_registrar: false,
                trusted_physical: false,
                image_bytes: 64,
            },
            Declaration {
                key: "tie",
                identity: b"usb:kbd",
                priority: 2,
                capacity: 2,
                reusable: true,
                child_registrar: false,
                trusted_physical: false,
                image_bytes: 64,
            },
        ];
        let mut bytes = [0; 1024];
        let used = encode(&records, &mut bytes).unwrap();
        let catalog = Catalog::decode(&bytes[..used], IMAGE_BASE + records.len()).unwrap();
        assert_eq!(catalog.select(b"usb:kbd", None), Ok(Some(0)));
        assert_eq!(catalog.select(b"usb:kbd", Some(0)), Err(Error::Conflict));
        assert_eq!(catalog.select(b"pci:other", Some(0)), Err(Error::Invalid));
        assert_eq!(
            catalog.select(b"usb:kbd", Some(records.len())),
            Err(Error::Invalid)
        );
        records[3].priority = 0;
        let used = encode(&records, &mut bytes).unwrap();
        let catalog = Catalog::decode(&bytes[..used], IMAGE_BASE + records.len()).unwrap();
        assert_eq!(catalog.select(b"usb:kbd", None), Ok(Some(0)));
        assert_eq!(catalog.select(b"usb:kbd", Some(0)), Ok(Some(2)));
        assert_eq!(catalog.select(b"usb:kbd", Some(2)), Ok(Some(1)));
        assert_eq!(catalog.select(b"usb:kbd", Some(1)), Ok(Some(3)));
        assert_eq!(catalog.select(b"usb:kbd", Some(3)), Ok(None));
    }
}
