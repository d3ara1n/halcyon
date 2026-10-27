//! 运行期设备身份。数字只用于相关性，不能代替 capability 或认证 context。

use core::num::NonZeroU64;
use librpc::bytes::{Reader, Writer};

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceId {
    incarnation: NonZeroU64,
    serial: NonZeroU64,
}

impl DeviceId {
    pub const ENCODED_LEN: usize = 16;

    pub fn new(incarnation: u64, serial: u64) -> Result<Self, Error> {
        Ok(Self {
            incarnation: NonZeroU64::new(incarnation).ok_or(Error::Invalid)?,
            serial: NonZeroU64::new(serial).ok_or(Error::Invalid)?,
        })
    }

    pub fn incarnation(self) -> u64 {
        self.incarnation.get()
    }

    pub fn serial(self) -> u64 {
        self.serial.get()
    }

    pub fn encode(self, writer: &mut Writer<'_>) {
        writer.reserve(Self::ENCODED_LEN);
        writer.u64(self.incarnation());
        writer.u64(self.serial());
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Self::new(reader.u64()?, reader.u64()?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BindingRef {
    device: DeviceId,
    epoch: NonZeroU64,
}

impl BindingRef {
    pub const ENCODED_LEN: usize = DeviceId::ENCODED_LEN + 8;

    pub fn new(device: DeviceId, epoch: u64) -> Result<Self, Error> {
        Ok(Self {
            device,
            epoch: NonZeroU64::new(epoch).ok_or(Error::InvalidEpoch)?,
        })
    }

    pub fn device(self) -> DeviceId {
        self.device
    }

    pub fn epoch(self) -> u64 {
        self.epoch.get()
    }

    pub fn encode(self, writer: &mut Writer<'_>) {
        writer.reserve(Self::ENCODED_LEN);
        self.device.encode(writer);
        writer.u64(self.epoch());
    }

    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Self::new(DeviceId::decode(reader)?, reader.u64()?)
    }
}

/// 仅由一个登记 backing 持有；incarnation 来自其实际根 mailbox KOID。
pub struct Ids {
    incarnation: NonZeroU64,
    highest: u64,
}

impl Ids {
    pub fn new(mailbox_koid: u64) -> Result<Self, Error> {
        Ok(Self {
            incarnation: NonZeroU64::new(mailbox_koid).ok_or(Error::Invalid)?,
            highest: 0,
        })
    }

    pub fn allocate(&mut self) -> Result<DeviceId, Error> {
        let serial = self.highest.checked_add(1).ok_or(Error::Exhausted)?;
        let id = DeviceId::new(self.incarnation.get(), serial)?;
        self.highest = serial;
        Ok(id)
    }

    /// 每次枚举固定这个有限终点；新登记由下一轮扫描及 Watch 提示覆盖。
    pub fn scan_end(&self) -> u64 {
        self.highest
    }
}
