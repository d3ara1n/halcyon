//! 本轮冻结资源表：准入时支付完整容量，逐项接收，不发布残表。

use alloc::vec::Vec;
use core::ops::Range;
use erhino_shared::{
    message::PAYLOAD_MAX,
    object::{HandleDescription, ObjectKind, Rights},
};
use libbudget::{AccountView, Charge, Taxonomy};
use librpc::bytes::{Reader, Writer};

use crate::Error;
pub mod oal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceResource {
    Publication,
    Binding,
    Bytes,
    Source,
    Use,
    Request,
    ControlRequest,
    ControlBytes,
    Watch,
    Function,
}

impl Taxonomy for DeviceResource {
    const COUNT: usize = 10;

    fn slot(self) -> usize {
        match self {
            Self::Publication => 0,
            Self::Binding => 1,
            Self::Bytes => 2,
            Self::Source => 3,
            Self::Use => 4,
            Self::Request => 5,
            Self::ControlRequest => 6,
            Self::ControlBytes => 7,
            Self::Watch => 8,
            Self::Function => 9,
        }
    }
}

/// 登记时冻结的管理存储上界，不限制单个设备的资源数为 IPC handle 数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub entries: u32,
    pub text_bytes: u32,
}

/// 一个表项随同实际 capability 交付的描述；协议身份不等于授权。
/// `protocol == 0` 表示内核资源，非零协议仅用于 MailboxSender。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub kind: ObjectKind,
    pub rights: Rights,
    pub protocol: u64,
}

impl Descriptor {
    pub const ENCODED_LEN: usize = 24;

    pub fn validate(self) -> Result<(), Error> {
        if !self.rights.is_known() {
            return Err(Error::Permission);
        }
        // 首批只收已确定可重复别名的资源/供应入口，不接受 affine 出口。
        match self.kind {
            ObjectKind::MailboxSender if self.protocol != 0 => Ok(()),
            ObjectKind::MemoryObject
            | ObjectKind::NotificationObserver
            | ObjectKind::MmioControl
            | ObjectKind::MmioUse
            | ObjectKind::IrqControl
            | ObjectKind::IrqUse
            | ObjectKind::DmaDomainControl
            | ObjectKind::DmaUse
            | ObjectKind::DmaMapping
                if self.protocol == 0 =>
            {
                Ok(())
            }
            _ => Err(Error::Unsupported),
        }
    }

    /// 安装回复时以实际 HandleDescribe 复核；不能只信任线上 kind/rights。
    pub fn check_handle(self, actual: &HandleDescription) -> Result<(), Error> {
        self.validate()?;
        if actual.kind != self.kind as u32
            || !actual
                .rights
                .contains(self.rights | Rights::DUPLICATE | Rights::TRANSIT)
        {
            return Err(Error::Permission);
        }
        Ok(())
    }

    pub fn encode(self, w: &mut Writer<'_>) {
        w.reserve(Self::ENCODED_LEN);
        w.u32(self.kind as u32);
        w.u32(0);
        w.u64(self.rights.raw());
        w.u64(self.protocol);
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        let raw = r.u32()?;
        let kind = match raw {
            x if x == ObjectKind::MailboxSender as u32 => ObjectKind::MailboxSender,
            x if x == ObjectKind::MemoryObject as u32 => ObjectKind::MemoryObject,
            x if x == ObjectKind::NotificationObserver as u32 => ObjectKind::NotificationObserver,
            x if x == ObjectKind::MmioControl as u32 => ObjectKind::MmioControl,
            x if x == ObjectKind::MmioUse as u32 => ObjectKind::MmioUse,
            x if x == ObjectKind::IrqControl as u32 => ObjectKind::IrqControl,
            x if x == ObjectKind::IrqUse as u32 => ObjectKind::IrqUse,
            x if x == ObjectKind::DmaDomainControl as u32 => ObjectKind::DmaDomainControl,
            x if x == ObjectKind::DmaUse as u32 => ObjectKind::DmaUse,
            x if x == ObjectKind::DmaMapping as u32 => ObjectKind::DmaMapping,
            _ => return Err(Error::Unsupported),
        };
        if r.u32()? != 0 {
            return Err(Error::Invalid);
        }
        let value = Self {
            kind,
            rights: Rights::from_raw(r.u64()?),
            protocol: r.u64()?,
        };
        value.validate()?;
        Ok(value)
    }
}

/// 单表项 wire；index、描述、两份 u32 长度前缀，实际 capability 恰为一个槽。
/// 外层领域头大小由该协议传入；不把固定运输容量复制成另一个常量。
pub struct Entry<'a> {
    pub index: u32,
    pub descriptor: Descriptor,
    pub name: &'a str,
    pub metadata: &'a [u8],
}

impl<'a> Entry<'a> {
    pub const FIXED_LEN: usize = 4 + Descriptor::ENCODED_LEN + 4 + 4;

    pub fn encoded_len(&self) -> Result<usize, Error> {
        self.descriptor.validate()?;
        liboal::path::validate_name(self.name).map_err(|_| Error::Invalid)?;
        if self.name.len() > oal::MAX_RESOURCE_NAME {
            return Err(Error::Invalid);
        }
        u32::try_from(self.name.len()).map_err(|_| Error::Invalid)?;
        u32::try_from(self.metadata.len()).map_err(|_| Error::Invalid)?;
        Self::FIXED_LEN
            .checked_add(self.name.len())
            .and_then(|n| n.checked_add(self.metadata.len()))
            .ok_or(Error::Invalid)
    }

    pub fn encode(&self, outer_header: usize, output: &mut [u8]) -> Result<usize, Error> {
        let len = self.encoded_len()?;
        Self::check_transport(len, outer_header)?;
        if output.len() < len {
            return Err(Error::Invalid);
        }
        let mut w = Writer::new(output);
        w.u32(self.index);
        self.descriptor.encode(&mut w);
        w.bytes32(self.name.as_bytes());
        w.bytes32(self.metadata);
        Ok(w.written())
    }

    pub fn decode(
        bytes: &'a [u8],
        outer_header: usize,
        handle_count: usize,
    ) -> Result<Self, Error> {
        Self::check_transport(bytes.len(), outer_header)?;
        if handle_count != 1 {
            return Err(Error::Invalid);
        }
        let mut r = Reader::new(bytes);
        let value = Self {
            index: r.u32()?,
            descriptor: Descriptor::decode(&mut r)?,
            name: r.text32()?,
            metadata: r.bytes32()?,
        };
        r.finish()?;
        value.encoded_len()?;
        Ok(value)
    }

    fn check_transport(len: usize, outer_header: usize) -> Result<(), Error> {
        if librpc::PREFIX_LEN
            .checked_add(outer_header)
            .and_then(|n| n.checked_add(len))
            .is_none_or(|n| n > PAYLOAD_MAX)
        {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }
}

struct Item<C> {
    capability: C,
    descriptor: Descriptor,
    name: Range<usize>,
    metadata: Range<usize>,
}

/// C 是实际拥有的能力；生产调用点必须在放入前用 check_handle 检查相应句柄，
/// 失败或部分表析构时由 C 正常 Close。内核安装费不包含在本表领域预留里。
pub struct Collecting<C> {
    table: Frozen<C>,
    limits: Limits,
    expected: Option<u32>,
    reply_header: usize,
}

pub struct Frozen<C> {
    items: Vec<Item<C>>,
    text: Vec<u8>,
    // 缓冲及能力先销毁，然后退回实际领域占用。
    _bytes: Charge,
    _extra_bytes: Option<Charge>,
}

pub struct Resource<'a, C> {
    pub capability: &'a C,
    pub descriptor: Descriptor,
    pub name: &'a str,
    pub metadata: &'a [u8],
}

impl<C> Collecting<C> {
    /// 冻结表容量的唯一布局计算，供实际provider在接受Prepare前预留领域付款。
    pub fn storage_bytes(limits: Limits) -> Result<usize, Error> {
        (limits.entries as usize)
            .checked_mul(core::mem::size_of::<Item<C>>())
            .and_then(|n| n.checked_add(limits.text_bytes as usize))
            .ok_or(Error::Invalid)
    }

    /// Claim 前调用，避免提交后才为整表找存储。实际 Binding/任务费用另由 owner
    /// 按自身布局预留；不按虚构任务常量替执行器收费。
    pub fn reserve(
        limits: Limits,
        reply_header: usize,
        account: &AccountView<DeviceResource>,
    ) -> Result<Self, Error> {
        Entry::check_transport(Entry::FIXED_LEN, reply_header)?;
        let entries = usize::try_from(limits.entries).map_err(|_| Error::Invalid)?;
        let text_bytes = usize::try_from(limits.text_bytes).map_err(|_| Error::Invalid)?;
        let bytes = Self::storage_bytes(limits)?;
        let charge = account.acquire(DeviceResource::Bytes, bytes)?;
        let mut items = Vec::new();
        let mut text = Vec::new();
        items
            .try_reserve_exact(entries)
            .map_err(|_| Error::Resource)?;
        text.try_reserve_exact(text_bytes)
            .map_err(|_| Error::Resource)?;
        let actual = items
            .capacity()
            .checked_mul(core::mem::size_of::<Item<C>>())
            .and_then(|n| n.checked_add(text.capacity()))
            .ok_or(Error::Invalid)?;
        let extra = if actual > bytes {
            // allocator 允许给出更大的 Vec 容量；完整准入前补齐真实容量账。
            Some(account.acquire(DeviceResource::Bytes, actual - bytes)?)
        } else {
            None
        };
        Ok(Self {
            table: Frozen {
                items,
                text,
                _bytes: charge,
                _extra_bytes: extra,
            },
            limits,
            expected: None,
            reply_header,
        })
    }

    pub fn prepared(&mut self, count: u32) -> Result<(), Error> {
        if count > self.limits.entries {
            return Err(Error::Quota);
        }
        match self.expected {
            Some(old) if old != count => Err(Error::Conflict),
            _ => {
                self.expected = Some(count);
                Ok(())
            }
        }
    }

    pub fn push(&mut self, entry: &Entry<'_>, capability: C) -> Result<(), Error> {
        let count = self.expected.ok_or(Error::NotReady)?;
        Entry::check_transport(entry.encoded_len()?, self.reply_header)?;
        if entry.index as usize != self.table.items.len() || entry.index >= count {
            return Err(Error::Conflict);
        }
        if self.table.get(entry.name).is_some() {
            return Err(Error::Conflict);
        }
        let begin = self.table.text.len();
        let name_end = begin.checked_add(entry.name.len()).ok_or(Error::Quota)?;
        let end = name_end
            .checked_add(entry.metadata.len())
            .ok_or(Error::Quota)?;
        if end > self.limits.text_bytes as usize {
            return Err(Error::Quota);
        }
        // 上限和索引已核对，所有 push/extend 均落在准入时的完整预留内。
        self.table.text.extend_from_slice(entry.name.as_bytes());
        self.table.text.extend_from_slice(entry.metadata);
        self.table.items.push(Item {
            capability,
            descriptor: entry.descriptor,
            name: begin..name_end,
            metadata: name_end..end,
        });
        Ok(())
    }

    /// 原表已接收的真实前缀，不另存准备进度计数。
    pub fn received(&self) -> usize {
        self.table.len()
    }

    #[expect(
        clippy::result_large_err,
        reason = "残表必须完整返还实际 capability、预留缓冲及付款，不为错误路径追加分配"
    )]
    pub fn finish(self) -> Result<Frozen<C>, Self> {
        if self.expected.map(|n| n as usize) != Some(self.table.items.len()) {
            return Err(self);
        }
        Ok(self.table)
    }
}

impl<C> Frozen<C> {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 服务端仍须在取得前核本轮 ResourcesReady/范围，停止后拒绝新 Get。
    pub fn get(&self, name: &str) -> Option<Resource<'_, C>> {
        self.items
            .iter()
            .position(|item| &self.text[item.name.clone()] == name.as_bytes())
            .and_then(|index| self.at(index))
    }

    pub fn at(&self, index: usize) -> Option<Resource<'_, C>> {
        let item = self.items.get(index)?;
        Some(Resource {
            capability: &item.capability,
            descriptor: item.descriptor,
            // 唯一写入点来自已验证的 &str，缓冲冻结后不再修改。
            name: core::str::from_utf8(&self.text[item.name.clone()])
                .expect("resource name lost UTF-8 validity"),
            metadata: &self.text[item.metadata.clone()],
        })
    }
}
