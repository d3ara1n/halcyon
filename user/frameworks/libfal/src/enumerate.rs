//! 目录枚举（Enumerate）：固定宽目录项头 + 内联名字 + 不透明 cursor 分页。
//!
//! 请求相对 Handle slot 1 的目录；cursor 为提供者不透明 u64（0 = 起始，
//! 应答 0 = 枚举完毕）。页预算由请求声明，应答受消息上限约束；并发
//! 修改导致的 cursor 失效以 [`Status::CursorInvalid`](crate::header::Status::CursorInvalid)
//! 报告。

use crate::{
    bytes::{DecodeError, DecodeResult, Reader, Writer},
    lookup::ResolvePolicy,
    node::NodeKind,
};

/// Enumerate 请求 body：寻址前奏 + cursor + 页预算。
pub struct EnumerateRequest<'a> {
    /// 相对 Handle slot 1 目录的路径。
    pub rel: &'a [u8],
    /// 0 = 从头开始；否则为上次应答的 next_cursor。
    pub cursor: u64,
    /// 本页名字 + 目录项头字节的预算。
    pub max_bytes: u32,
}

impl EnumerateRequest<'_> {
    /// 线长：寻址前奏 + cursor u64 + max_bytes u32 + reserved u32。
    pub fn encoded_len(&self) -> usize {
        8 + 2 + self.rel.len() + 8 + 4 + 4
    }

    pub fn encode(&self, out: &mut [u8]) -> usize {
        let mut writer = Writer::new(out);
        writer.reserve(self.encoded_len());
        writer.u32(ResolvePolicy::FollowAll as u32);
        writer.u32(0);
        writer.sized_bytes(self.rel);
        writer.u64(self.cursor);
        writer.u32(self.max_bytes);
        writer.u32(0);
        writer.written()
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(&[u8], u64, u32)> {
        let mut reader = Reader::new(bytes);
        let policy = ResolvePolicy::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let reserved = reader.u32()?;
        let rel = reader.sized_bytes()?;
        let cursor = reader.u64()?;
        let max_bytes = reader.u32()?;
        let tail = reader.u32()?;
        reader.finish()?;
        if reserved != 0 || tail != 0 || policy != ResolvePolicy::FollowAll {
            return Err(DecodeError);
        }
        Ok((rel, cursor, max_bytes))
    }
}

/// 目录项头：类型 + 内联名字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry<'a> {
    pub kind: NodeKind,
    pub name: &'a [u8],
}

/// Enumerate 应答 body 固定部分：next_cursor u64 + count u32 + reserved u32。
pub const RESPONSE_FIXED_LEN: usize = 8 + 4 + 4;

/// 单个目录项的线字节数：sized name + kind u32 + reserved u32。
/// 与提供者的页预算记账共用同一口径。
pub const ENTRY_OVERHEAD: usize = 2 + 4 + 4;

/// Enumerate 应答 body：next_cursor + 项数 + 目录项序列。
pub struct EnumerateResponse<'a> {
    /// 0 = 枚举完毕；否则回带续查。
    pub next_cursor: u64,
    pub entries: &'a [DirectoryEntry<'a>],
}

impl EnumerateResponse<'_> {
    /// 线长：固定头 + 逐目录项（sized name + kind u32 + reserved u32）。
    pub fn encoded_len(&self) -> usize {
        RESPONSE_FIXED_LEN
            + self.entries.iter().map(|e| ENTRY_OVERHEAD + e.name.len()).sum::<usize>()
    }

    /// 编码不可失败：页预算（含应答承载折算）由提供者前置保证。
    pub fn encode(&self, out: &mut [u8]) -> usize {
        let mut writer = Writer::new(out);
        writer.reserve(self.encoded_len());
        writer.u64(self.next_cursor);
        writer.u32(self.entries.len() as u32);
        writer.u32(0);
        for entry in self.entries {
            writer.sized_bytes(entry.name);
            writer.u32(entry.kind as u32);
            writer.u32(0);
        }
        writer.written()
    }
}

/// 逐项解码目录项；项流必须整体消费，尾差即协议违约。
pub fn decode_entries(
    bytes: &[u8],
    count: usize,
) -> DecodeResult<impl Iterator<Item = DecodeResult<DirectoryEntry<'_>>>> {
    let mut probe = Reader::new(bytes);
    for _ in 0..count {
        let name_len = probe.u16()? as usize;
        probe.bytes(name_len)?;
        probe.u32()?;
        probe.u32()?;
    }
    probe.finish()?;
    Ok(EntryIter { reader: Reader::new(bytes), remaining: count })
}

/// 应答头部（next_cursor + 计数 + 保留区）解码，返回项字节段。
pub fn decode_response_header<'a>(bytes: &'a [u8]) -> DecodeResult<(u64, usize, &'a [u8])> {
    let mut reader = Reader::new(bytes);
    let next_cursor = reader.u64()?;
    let count = reader.u32()? as usize;
    let reserved = reader.u32()?;
    if reserved != 0 {
        return Err(DecodeError);
    }
    Ok((next_cursor, count, reader.bytes(reader.remaining())?))
}

struct EntryIter<'a> {
    reader: Reader<'a>,
    remaining: usize,
}

impl<'a> Iterator for EntryIter<'a> {
    type Item = DecodeResult<DirectoryEntry<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some((|| {
            let name_len = self.reader.u16()? as usize;
            let name = self.reader.bytes(name_len)?;
            let kind = NodeKind::from_u32(self.reader.u32()?).ok_or(DecodeError)?;
            let reserved = self.reader.u32()?;
            if reserved != 0 {
                return Err(DecodeError);
            }
            Ok(DirectoryEntry { kind, name })
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = EnumerateRequest { rel: b"a/b", cursor: 0x1234, max_bytes: 512 };
        let used = request.encode(&mut buffer);
        assert_eq!(EnumerateRequest::decode(&buffer[..used]).unwrap(), (b"a/b".as_slice(), 0x1234, 512));
    }

    #[test]
    fn response_roundtrip() {
        let mut buffer = [0u8; 96];
        let entries = [
            DirectoryEntry { kind: NodeKind::Directory, name: b"boot" },
            DirectoryEntry { kind: NodeKind::Stream, name: b"readme" },
            DirectoryEntry { kind: NodeKind::Property, name: b"version" },
        ];
        let response = EnumerateResponse { next_cursor: 7, entries: &entries };
        let used = response.encode(&mut buffer);

        // 应答头（cursor + 计数）解码，随后逐项解码。
        let (next_cursor, count, entry_bytes) =
            decode_response_header(&buffer[..used]).unwrap();
        assert_eq!(next_cursor, 7);
        assert_eq!(count, 3);
        let items: Vec<_> = decode_entries(entry_bytes, count)
            .unwrap()
            .map(|item| item.unwrap())
            .collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], entries[0]);
        assert_eq!(items[1], entries[1]);
        assert_eq!(items[2], entries[2]);
    }

    #[test]
    fn truncated_entry_stream_is_rejected() {
        // 单项（名字 4 字节 + kind + 保留区）被截断时整体拒绝。
        let mut buffer = [0u8; 10];
        let mut writer = Writer::new(&mut buffer);
        writer.reserve(writer.remaining());
        writer.u16(4);
        writer.bytes(b"boot");
        writer.u32(NodeKind::Directory as u32);
        let used = writer.written();
        // 保留区缺失
        assert!(decode_entries(&buffer[..used], 1).is_err());
    }
}
