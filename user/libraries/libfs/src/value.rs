//! 纯 Property 值：固定标量或同质一维数组，没有 capability、Record 或递归值。

use crate::protocol::{BODY_MAX, Status};
use alloc::{string::String, vec::Vec};
use librpc::bytes::{Reader, Writer};

pub const HEADER_LEN: usize = 12;
pub const VALUE_MAX: usize = BODY_MAX - 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Scalar {
    Bool = 1,
    Signed64 = 2,
    Unsigned64 = 3,
    Float64 = 4,
    String = 5,
    Blob = 6,
}
impl Scalar {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Bool),
            2 => Some(Self::Signed64),
            3 => Some(Self::Unsigned64),
            4 => Some(Self::Float64),
            5 => Some(Self::String),
            6 => Some(Self::Blob),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Item<'a> {
    Bool(bool),
    Signed64(i64),
    Unsigned64(u64),
    Float64(f64),
    String(&'a str),
    Blob(&'a [u8]),
}

#[derive(Debug, Clone, Copy)]
pub struct Property<'a> {
    bytes: &'a [u8],
    scalar: Scalar,
    array: bool,
    count: u32,
}
impl<'a> Property<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Status> {
        if bytes.len() > VALUE_MAX {
            return Err(Status::ValueTooLarge);
        }
        let mut reader = Reader::new(bytes);
        let scalar = Scalar::from_raw(reader.u32()?).ok_or(Status::Invalid)?;
        let array = match reader.u32()? {
            0 => false,
            1 => true,
            _ => return Err(Status::Invalid),
        };
        let count = reader.u32()?;
        if !array && count != 1 {
            return Err(Status::Invalid);
        }
        let minimum = match scalar {
            Scalar::Bool => 1,
            Scalar::Signed64 | Scalar::Unsigned64 | Scalar::Float64 => 8,
            Scalar::String | Scalar::Blob => 4,
        };
        if usize::try_from(count)
            .map_err(|_| Status::Invalid)?
            .checked_mul(minimum)
            .ok_or(Status::Invalid)?
            > reader.remaining()
        {
            return Err(Status::Invalid);
        }
        for _ in 0..count {
            read_item(scalar, &mut reader)?;
        }
        reader.finish()?;
        Ok(Self {
            bytes,
            scalar,
            array,
            count,
        })
    }
    pub fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn scalar(self) -> Scalar {
        self.scalar
    }
    pub fn is_array(self) -> bool {
        self.array
    }
    pub fn count(self) -> u32 {
        self.count
    }
    pub fn items(self) -> Items<'a> {
        Items {
            reader: Reader::new(&self.bytes[HEADER_LEN..]),
            scalar: self.scalar,
            remaining: self.count,
        }
    }
    pub fn get<T: FromProperty>(self) -> Result<T, Status> {
        T::from_property(self)
    }
    fn one(self) -> Result<Item<'a>, Status> {
        if self.array {
            return Err(Status::TypeMismatch);
        }
        self.items().next().ok_or(Status::Invalid)
    }
}
fn read_item<'a>(scalar: Scalar, reader: &mut Reader<'a>) -> Result<Item<'a>, Status> {
    Ok(match scalar {
        Scalar::Bool => Item::Bool(match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(Status::Invalid),
        }),
        Scalar::Signed64 => Item::Signed64(reader.u64()? as i64),
        Scalar::Unsigned64 => Item::Unsigned64(reader.u64()?),
        Scalar::Float64 => Item::Float64(f64::from_bits(reader.u64()?)),
        Scalar::String => Item::String(reader.text32()?),
        Scalar::Blob => Item::Blob(reader.bytes32()?),
    })
}
pub struct Items<'a> {
    reader: Reader<'a>,
    scalar: Scalar,
    remaining: u32,
}
impl<'a> Iterator for Items<'a> {
    type Item = Item<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        // 迭代器只能来自完整验证后的不可变 Property，字段长度和值均已核对。
        Some(read_item(self.scalar, &mut self.reader).expect("validated property item"))
    }
}

pub trait FromProperty: Sized {
    fn from_property(value: Property<'_>) -> Result<Self, Status>;
}
impl FromProperty for bool {
    fn from_property(value: Property<'_>) -> Result<Self, Status> {
        match value.one()? {
            Item::Bool(v) => Ok(v),
            _ => Err(Status::TypeMismatch),
        }
    }
}
impl FromProperty for i64 {
    fn from_property(value: Property<'_>) -> Result<Self, Status> {
        match value.one()? {
            Item::Signed64(v) => Ok(v),
            Item::Unsigned64(v) => i64::try_from(v).map_err(|_| Status::Range),
            _ => Err(Status::TypeMismatch),
        }
    }
}
impl FromProperty for u64 {
    fn from_property(value: Property<'_>) -> Result<Self, Status> {
        match value.one()? {
            Item::Unsigned64(v) => Ok(v),
            Item::Signed64(v) => u64::try_from(v).map_err(|_| Status::Range),
            _ => Err(Status::TypeMismatch),
        }
    }
}
impl FromProperty for usize {
    fn from_property(value: Property<'_>) -> Result<Self, Status> {
        usize::try_from(u64::from_property(value)?).map_err(|_| Status::Range)
    }
}
impl FromProperty for f64 {
    fn from_property(value: Property<'_>) -> Result<Self, Status> {
        match value.one()? {
            Item::Float64(v) => Ok(v),
            _ => Err(Status::TypeMismatch),
        }
    }
}
impl FromProperty for String {
    fn from_property(value: Property<'_>) -> Result<Self, Status> {
        let Item::String(text) = value.one()? else {
            return Err(Status::TypeMismatch);
        };
        let mut out = String::new();
        out.try_reserve_exact(text.len())
            .map_err(|_| Status::Resource)?;
        out.push_str(text);
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Value<'a> {
    Bool(bool),
    Signed64(i64),
    Unsigned64(u64),
    Float64(f64),
    String(&'a str),
    Blob(&'a [u8]),
    BoolArray(&'a [bool]),
    Signed64Array(&'a [i64]),
    Unsigned64Array(&'a [u64]),
    Float64Array(&'a [f64]),
    StringArray(&'a [&'a str]),
    BlobArray(&'a [&'a [u8]]),
}
impl Value<'_> {
    fn shape(&self) -> (Scalar, bool, usize) {
        match self {
            Self::Bool(_) => (Scalar::Bool, false, 1),
            Self::Signed64(_) => (Scalar::Signed64, false, 1),
            Self::Unsigned64(_) => (Scalar::Unsigned64, false, 1),
            Self::Float64(_) => (Scalar::Float64, false, 1),
            Self::String(_) => (Scalar::String, false, 1),
            Self::Blob(_) => (Scalar::Blob, false, 1),
            Self::BoolArray(v) => (Scalar::Bool, true, v.len()),
            Self::Signed64Array(v) => (Scalar::Signed64, true, v.len()),
            Self::Unsigned64Array(v) => (Scalar::Unsigned64, true, v.len()),
            Self::Float64Array(v) => (Scalar::Float64, true, v.len()),
            Self::StringArray(v) => (Scalar::String, true, v.len()),
            Self::BlobArray(v) => (Scalar::Blob, true, v.len()),
        }
    }
    pub fn encoded_len(&self) -> Result<usize, Status> {
        let (scalar, _, count) = self.shape();
        u32::try_from(count).map_err(|_| Status::ValueTooLarge)?;
        let payload = match scalar {
            Scalar::Bool => Some(count),
            Scalar::Signed64 | Scalar::Unsigned64 | Scalar::Float64 => count.checked_mul(8),
            Scalar::String | Scalar::Blob => match self {
                Self::String(v) => 4usize.checked_add(v.len()),
                Self::Blob(v) => 4usize.checked_add(v.len()),
                Self::StringArray(v) => v
                    .iter()
                    .try_fold(0usize, |sum, v| sum.checked_add(4)?.checked_add(v.len())),
                Self::BlobArray(v) => v
                    .iter()
                    .try_fold(0usize, |sum, v| sum.checked_add(4)?.checked_add(v.len())),
                _ => None,
            },
        }
        .and_then(|n| n.checked_add(HEADER_LEN))
        .ok_or(Status::ValueTooLarge)?;
        if payload > VALUE_MAX {
            return Err(Status::ValueTooLarge);
        }
        Ok(payload)
    }
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Status> {
        let len = self.encoded_len()?;
        if out.len() < len {
            return Err(Status::Resource);
        }
        let (scalar, array, count) = self.shape();
        let mut w = Writer::new(out);
        w.u32(scalar as u32);
        w.u32(u32::from(array));
        w.u32(count as u32);
        match self {
            Self::Bool(v) => w.u8(u8::from(*v)),
            Self::Signed64(v) => w.u64(*v as u64),
            Self::Unsigned64(v) => w.u64(*v),
            Self::Float64(v) => w.u64(v.to_bits()),
            Self::String(v) => w.bytes32(v.as_bytes()),
            Self::Blob(v) => w.bytes32(v),
            Self::BoolArray(v) => {
                for x in *v {
                    w.u8(u8::from(*x));
                }
            }
            Self::Signed64Array(v) => {
                for x in *v {
                    w.u64(*x as u64);
                }
            }
            Self::Unsigned64Array(v) => {
                for x in *v {
                    w.u64(*x);
                }
            }
            Self::Float64Array(v) => {
                for x in *v {
                    w.u64(x.to_bits());
                }
            }
            Self::StringArray(v) => {
                for x in *v {
                    w.bytes32(x.as_bytes());
                }
            }
            Self::BlobArray(v) => {
                for x in *v {
                    w.bytes32(x);
                }
            }
        }
        Ok(w.written())
    }
}

pub struct StoredProperty {
    bytes: Vec<u8>,
    _charge: libbudget::Charge,
}
impl StoredProperty {
    pub fn prepare(
        bytes: &[u8],
        account: &libbudget::AccountView<crate::resource::FsResource>,
    ) -> Result<Self, Status> {
        Property::decode(bytes)?;
        let mut stored = Vec::new();
        stored
            .try_reserve_exact(bytes.len())
            .map_err(|_| Status::Resource)?;
        let charge = account
            .acquire(crate::resource::FsResource::Bytes, stored.capacity())
            .map_err(|_| Status::Quota)?;
        stored.extend_from_slice(bytes);
        Ok(Self {
            bytes: stored,
            _charge: charge,
        })
    }
    pub fn value(&self) -> Property<'_> {
        // 唯一构造入口已完整验证，bytes 私有且不变；这里只恢复借用字段，不全量重验。
        let scalar = Scalar::from_raw(u32::from_le_bytes(
            self.bytes[..4].try_into().expect("validated scalar field"),
        ))
        .expect("validated scalar kind");
        let array = self.bytes[4] == 1;
        let count =
            u32::from_le_bytes(self.bytes[8..12].try_into().expect("validated count field"));
        Property {
            bytes: &self.bytes,
            scalar,
            array,
            count,
        }
    }
}

#[cfg(test)]
mod tests;
