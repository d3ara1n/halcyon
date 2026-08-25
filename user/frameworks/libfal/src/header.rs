//! FAL header（紧随 RpcPrefix）与协议状态码。

use crate::bytes::{DecodeError, DecodeResult, Reader, Writer};

/// 当前 FAL 协议版本。
pub const FAL_VERSION: u16 = 1;

/// 协议 kind：请求/应答共用的操作判别。
///
/// kind 号是版本冻结的契约；动词自含对象——Read/Write 整值语义指向
/// Property，At 后缀是定位读写指向 Stream，无前缀无歧义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Kind {
    /// 走路三值应答：Found（含元数据与 value 尾）/ Delegate / Link。
    Lookup = 0x01,
    /// 目录枚举：固定宽目录项头 + 内联名字，cursor 分页。
    Enumerate = 0x02,
    /// 在目录下创建节点（目录/属性/流）。
    Create = 0x03,
    /// 创建符号链接（持久化 target 文本）。
    Link = 0x04,
    /// 读属性整值。
    Read = 0x05,
    /// 写属性整值（替换）。
    Write = 0x06,
    /// 打开流：应答转交一次性 tunnel peer invitation（slot 1）。
    Open = 0x07,
    /// 偏移读流：字节直接置于应答 payload。
    ReadAt = 0x08,
    /// 偏移写流。
    WriteAt = 0x09,
    /// 移动节点。
    Move = 0x0A,
    /// 复制节点。
    Copy = 0x0B,
    /// 删除节点。
    Delete = 0x0C,
}

impl Kind {
    pub const fn from_u16(raw: u16) -> Option<Self> {
        match raw {
            0x01 => Some(Self::Lookup),
            0x02 => Some(Self::Enumerate),
            0x03 => Some(Self::Create),
            0x04 => Some(Self::Link),
            0x05 => Some(Self::Read),
            0x06 => Some(Self::Write),
            0x07 => Some(Self::Open),
            0x08 => Some(Self::ReadAt),
            0x09 => Some(Self::WriteAt),
            0x0A => Some(Self::Move),
            0x0B => Some(Self::Copy),
            0x0C => Some(Self::Delete),
            _ => None,
        }
    }
}

/// 应答状态码（0 = 成功）。
///
/// 终态到调用失败的映射由客户端库完成；提供者不得发明未列出的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Ok = 0,
    /// 路径不存在（含悬空符号链接的终段）。
    NotFound = 1,
    /// 路径中途分量不是目录。
    NotADirectory = 2,
    /// 权限不足（rights 被裁剪）。
    NotAccessible = 3,
    /// 路径违反编码契约（空段、`.`、`..`、通配符、非 UTF-8、超长）。
    IllegalPath = 4,
    /// 参数违反 kind 不变量（偏移越界、类型不符等）。
    IllegalArgument = 5,
    /// 提供者或节点不支持该 kind。
    Unsupported = 6,
    /// 创建目标已存在。
    Exists = 7,
    /// 符号链接展开次数超限（40，整次解析计）。
    TooManyLinks = 8,
    /// 枚举 cursor 失效（并发修改）或未知。
    CursorInvalid = 9,
    /// Handle slot 的对象种类与 kind 期望不符。
    HandleKindMismatch = 10,
    /// op 内部行走遭遇符号链接：客户端展开后重试。
    SymbolicLinkEncountered = 11,
    /// 提供者内部错误。
    Internal = 0xFFFF,
}

impl Status {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Ok),
            1 => Some(Self::NotFound),
            2 => Some(Self::NotADirectory),
            3 => Some(Self::NotAccessible),
            4 => Some(Self::IllegalPath),
            5 => Some(Self::IllegalArgument),
            6 => Some(Self::Unsupported),
            7 => Some(Self::Exists),
            8 => Some(Self::TooManyLinks),
            9 => Some(Self::CursorInvalid),
            10 => Some(Self::HandleKindMismatch),
            11 => Some(Self::SymbolicLinkEncountered),
            0xFFFF => Some(Self::Internal),
            _ => None,
        }
    }
}

/// FAL header：自有版本、kind、总长度与保留区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FalHeader {
    pub version: u16,
    pub kind: Kind,
    /// FAL 消息总长度（header + body），与外层长度交叉校验。
    pub total_len: u32,
}

impl FalHeader {
    pub const fn new(kind: Kind, total_len: u32) -> Self {
        Self { version: FAL_VERSION, kind, total_len }
    }

    /// 编码进 16 字节定宽区（保留区置零）；短缓冲即协议错误。
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<()> {
        let mut writer = Writer::new(out);
        writer.u16(self.version)?;
        writer.u16(self.kind as u16)?;
        writer.u32(self.total_len)?;
        writer.u64(0)?;
        Ok(())
    }

    /// 从 16 字节定宽区解码并验证版本与保留区。
    pub fn decode(bytes: &[u8]) -> DecodeResult<Self> {
        let mut reader = Reader::new(bytes);
        let version = reader.u16()?;
        let raw_kind = reader.u16()?;
        let total_len = reader.u32()?;
        let reserved = reader.u64()?;
        if version != FAL_VERSION || reserved != 0 {
            return Err(DecodeError);
        }
        let kind = Kind::from_u16(raw_kind).ok_or(DecodeError)?;
        reader.finish()?;
        Ok(Self { version, kind, total_len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip_all_kinds() {
        let mut buffer = [0u8; 16];
        for kind in [
            Kind::Lookup,
            Kind::Enumerate,
            Kind::Create,
            Kind::Link,
            Kind::Read,
            Kind::Write,
            Kind::Open,
            Kind::ReadAt,
            Kind::WriteAt,
            Kind::Move,
            Kind::Copy,
            Kind::Delete,
        ] {
            let header = FalHeader::new(kind, 0x1234);
            header.encode(&mut buffer);
            assert_eq!(FalHeader::decode(&buffer).unwrap(), header);
        }
    }

    #[test]
    fn header_rejects_unknown_and_dirty_reserved() {
        let mut buffer = [0u8; 16];
        FalHeader::new(Kind::Lookup, 8).encode(&mut buffer);
        buffer[2] = 0x0D;
        assert_eq!(FalHeader::decode(&buffer), Err(DecodeError));
        buffer[2] = 0x01;
        buffer[15] = 1;
        assert_eq!(FalHeader::decode(&buffer), Err(DecodeError));
    }

    #[test]
    fn status_roundtrip() {
        for raw in 0..=11u32 {
            assert_eq!(Status::from_u32(raw).map(|s| s as u32), Some(raw));
        }
        assert_eq!(Status::from_u32(0xFFFF).map(|s| s as u32), Some(0xFFFF));
        assert_eq!(Status::from_u32(12), None);
    }
}
