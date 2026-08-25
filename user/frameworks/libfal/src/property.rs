//! 属性类型系统：语义类型、`Array<T>` 与 `Handle[T]` 的线编码。
//!
//! 值是什么由属性类型决定，协议对具体用途零特判：读整数得到整数，
//! 读 `Handle[T]` 得到 Handle，是同一条协议路径。`Handle[T]` 的
//! Handle 本体经消息 Handle move 交付（不在字节里），值编码只声明
//! T 与所在槽位；每次读取都是一次授权铸造，写入即向属性转移一项。

use crate::bytes::{DecodeError, DecodeResult, Reader, Writer};

/// 属性值尺寸上限：FAL 版本常量，不超过消息 payload 上限；
/// 更大的内容属于流。
pub const VALUE_MAX: usize = 4096;

/// 语义类型标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ValueType {
    Integer = 1,
    Decimal = 2,
    String = 3,
    Blob = 4,
    /// 一个指向 T 类对象的授权项；Handle 经消息槽位交付。
    HandleRef = 5,
    /// 数组：元素类型 + 定长 u32 计数 + 逐项 u16 前缀编码。
    Array = 6,
}

impl ValueType {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Integer),
            2 => Some(Self::Decimal),
            3 => Some(Self::String),
            4 => Some(Self::Blob),
            5 => Some(Self::HandleRef),
            6 => Some(Self::Array),
            _ => None,
        }
    }
}

/// `Handle[T]` 的 T：所指对象的种类，供写入校验与读取方判别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HandleKindTag {
    Any = 0,
    Directory = 1,
    MailboxSender = 2,
    NotificationSignaler = 3,
}

impl HandleKindTag {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Any),
            1 => Some(Self::Directory),
            2 => Some(Self::MailboxSender),
            3 => Some(Self::NotificationSignaler),
            _ => None,
        }
    }
}

/// watch 属性的事件位（位集由协议版本扩展，内核不解释）。
pub const WATCH_CREATE: u64 = 1 << 0;
pub const WATCH_DELETE: u64 = 1 << 1;
pub const WATCH_MODIFY: u64 = 1 << 2;
pub const WATCH_RENAME: u64 = 1 << 3;

/// 编码侧的属性值线形：`tag` 之后按类型展开。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropertyValue<'a> {
    Integer(i64),
    Decimal(f64),
    Str(&'a [u8]),
    Blob(&'a [u8]),
    /// T 与 Handle 所在消息槽位（槽位 ≥ 1）。
    Handle { kind: HandleKindTag, slot: u16 },
    /// 数组：元素类型 + 调用方预编码的逐项字节段。
    Array { element: ValueType, items: &'a [EncodedItem<'a>] },
}

/// 数组项：逐项 u16 前缀编码的字节段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedItem<'a>(pub &'a [u8]);

/// 解码侧的属性值线形：数组以元素类型 + 原始字节段返回，
/// 元素体的逐项遍历由调用方按元素类型完成。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecodedValue<'a> {
    Integer(i64),
    Decimal(f64),
    Str(&'a [u8]),
    Blob(&'a [u8]),
    Handle { kind: HandleKindTag, slot: u16 },
    Array { element: ValueType, body: &'a [u8] },
}

impl PropertyValue<'_> {
    /// 编码进 `out`，返回写入字节数；超限返回 [`DecodeError`]。
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let mut writer = Writer::new(out);
        match self {
            Self::Integer(value) => {
                writer.u32(ValueType::Integer as u32)?;
                writer.u32(0)?;
                writer.u64(*value as u64)?;
            }
            Self::Decimal(value) => {
                writer.u32(ValueType::Decimal as u32)?;
                writer.u32(0)?;
                writer.u64(value.to_bits())?;
            }
            Self::Str(value) => {
                writer.u32(ValueType::String as u32)?;
                writer.u32(0)?;
                writer.sized_bytes(value)?;
            }
            Self::Blob(value) => {
                writer.u32(ValueType::Blob as u32)?;
                writer.u32(0)?;
                writer.sized_bytes(value)?;
            }
            Self::Handle { kind, slot } => {
                writer.u32(ValueType::HandleRef as u32)?;
                writer.u32(*kind as u32)?;
                writer.u16(*slot)?;
                writer.u16(0)?;
            }
            Self::Array { element, items } => {
                writer.u32(ValueType::Array as u32)?;
                writer.u32(*element as u32)?;
                writer.u32(items.len() as u32)?;
                writer.u32(0)?;
                for item in items.iter() {
                    writer.sized_bytes(item.0)?;
                }
            }
        }
        Ok(writer.written())
    }
}

impl<'a> DecodedValue<'a> {
    /// 解码并验证：保留区置零、槽位 ≥ 1、数组体可按计数完整遍历。
    pub fn decode(bytes: &'a [u8]) -> DecodeResult<Self> {
        let mut reader = Reader::new(bytes);
        let tag = ValueType::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let value = match tag {
            ValueType::Integer => {
                let reserved = reader.u32()?;
                let raw = reader.u64()?;
                if reserved != 0 {
                    return Err(DecodeError);
                }
                Self::Integer(raw as i64)
            }
            ValueType::Decimal => {
                let reserved = reader.u32()?;
                let raw = reader.u64()?;
                if reserved != 0 {
                    return Err(DecodeError);
                }
                Self::Decimal(f64::from_bits(raw))
            }
            ValueType::String => {
                let reserved = reader.u32()?;
                let text = reader.sized_bytes()?;
                if reserved != 0 {
                    return Err(DecodeError);
                }
                Self::Str(text)
            }
            ValueType::Blob => {
                let reserved = reader.u32()?;
                let blob = reader.sized_bytes()?;
                if reserved != 0 {
                    return Err(DecodeError);
                }
                Self::Blob(blob)
            }
            ValueType::HandleRef => {
                let kind = HandleKindTag::from_u32(reader.u32()?).ok_or(DecodeError)?;
                let slot = reader.u16()?;
                let reserved = reader.u16()?;
                if reserved != 0 || slot < 1 {
                    return Err(DecodeError);
                }
                Self::Handle { kind, slot }
            }
            ValueType::Array => {
                let element = ValueType::from_u32(reader.u32()?).ok_or(DecodeError)?;
                let count = reader.u32()? as usize;
                let reserved = reader.u32()?;
                if reserved != 0 {
                    return Err(DecodeError);
                }
                let body = reader.bytes(reader.remaining())?;
                let mut probe = Reader::new(body);
                for _ in 0..count {
                    let len = probe.u16()? as usize;
                    probe.bytes(len)?;
                }
                probe.finish()?;
                Self::Array { element, body }
            }
        };
        reader.finish()?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_roundtrips() {
        let mut buffer = [0u8; 64];
        for (encoded, decoded) in [
            (PropertyValue::Integer(-1919810i64), DecodedValue::Integer(-1919810)),
            (PropertyValue::Decimal(3.5f64), DecodedValue::Decimal(3.5)),
            (PropertyValue::Str(b"hello"), DecodedValue::Str(b"hello")),
            (PropertyValue::Blob(&[1, 2, 3]), DecodedValue::Blob(&[1, 2, 3])),
        ] {
            let used = encoded.encode(&mut buffer).unwrap();
            assert_eq!(DecodedValue::decode(&buffer[..used]).unwrap(), decoded);
        }
    }

    #[test]
    fn handle_ref_roundtrip_and_slot_guard() {
        let mut buffer = [0u8; 16];
        let value = PropertyValue::Handle { kind: HandleKindTag::MailboxSender, slot: 1 };
        let used = value.encode(&mut buffer).unwrap();
        assert_eq!(
            DecodedValue::decode(&buffer[..used]).unwrap(),
            DecodedValue::Handle { kind: HandleKindTag::MailboxSender, slot: 1 }
        );

        // slot 0 违约：交付槽位从 1 起排布。
        let mut writer = Writer::new(&mut buffer);
        writer.u32(ValueType::HandleRef as u32);
        writer.u32(HandleKindTag::Any as u32);
        writer.u16(0);
        writer.u16(0);
        let used = writer.written();
        assert_eq!(DecodedValue::decode(&buffer[..used]), Err(DecodeError));
    }

    #[test]
    fn array_body_roundtrip() {
        let mut buffer = [0u8; 64];
        let first = 114514i64.to_le_bytes();
        let second = (-1919810i64).to_le_bytes();
        let items = [EncodedItem(&first), EncodedItem(&second)];
        let value = PropertyValue::Array { element: ValueType::Integer, items: &items };
        let used = value.encode(&mut buffer).unwrap();
        match DecodedValue::decode(&buffer[..used]).unwrap() {
            DecodedValue::Array { element, body } => {
                assert_eq!(element, ValueType::Integer);
                let mut reader = Reader::new(body);
                let first_len = reader.u16().unwrap() as usize;
                assert_eq!(reader.bytes(first_len).unwrap(), first);
                let second_len = reader.u16().unwrap() as usize;
                assert_eq!(reader.bytes(second_len).unwrap(), second);
                assert_eq!(reader.finish(), Ok(()));
            }
            other => panic!("unexpected decode: {:?}", other),
        }
    }
}
