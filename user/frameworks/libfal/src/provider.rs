//! 提供者分发：请求消息 → 提供者操作 → 应答消息的纯编解码路由。
//!
//! 输入为完整 payload（RpcPrefix 已由传输层剥离，从 FalHeader 起）；
//! 输出为应答 body。Handle 槽位：请求 slot 0 恒为 send-once 回复授权
//! （传输层消费），slot 1 为帧锚目录 Handle（传输层解析后不再进入本层）；
//! v1 分发面无出站 Handle（委托与 Handle 属性随相应 kind 接入）。

use alloc::vec::Vec;
use crate::bytes::{DecodeError, Reader, Writer};
use crate::enumerate::EnumerateResponse;
use crate::header::{FalHeader, Kind, Status};
use crate::memfs::{MemFs, MemLookup};
use crate::op::OpAddress;
use crate::FAL_HEADER_LEN;

/// 分发结果：应答 kind 与 body 字节数（写入 `out`）。
pub struct Served {
    pub kind: Kind,
    pub len: usize,
}

/// 处理一条请求：`request` 从 FalHeader 起（不含 RpcPrefix）。
pub fn serve(fs: &mut MemFs, request: &[u8], out: &mut [u8]) -> Result<Served, DecodeError> {
    if request.len() < FAL_HEADER_LEN {
        return Err(DecodeError);
    }
    let header = FalHeader::decode(&request[..FAL_HEADER_LEN])?;
    let body = &request[FAL_HEADER_LEN..];
    let status_only = |out: &mut [u8], status: Status| -> Served {
        let mut writer = Writer::new(out);
        writer.u32(status as u32);
        writer.u32(0);
        Served { kind: header.kind, len: writer.written() }
    };

    match header.kind {
        Kind::Lookup => {
            let (policy, rel) = crate::lookup::LookupRequest::decode(body)?;
            let mut writer = Writer::new(out);
            match fs.lookup(policy, rel) {
                Ok(MemLookup::Found { kind, attributes, size, target }) => {
                    writer.u32(Status::Ok as u32);
                    writer.u32(0);
                    let used = writer.written();
                    let info = crate::lookup::NodeInfo {
                        kind,
                        attributes,
                        size,
                        value: target.as_deref().map(str::as_bytes).unwrap_or_default(),
                    };
                    let len = info.encode(&mut out[used..])?;
                    Ok(Served { kind: header.kind, len: used + len })
                }
                Ok(MemLookup::Link { parent_rel, target, remaining }) => {
                    writer.u32(Status::Ok as u32);
                    writer.u32(1); // 变体：符号链接边界
                    let used = writer.written();
                    let link = crate::lookup::LinkBoundary {
                        consumed: parent_rel.as_bytes(),
                        target: target.as_bytes(),
                        remaining: remaining.as_bytes(),
                    };
                    let len = link.encode(&mut out[used..])?;
                    Ok(Served { kind: header.kind, len: used + len })
                }
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::Enumerate => {
            let (rel, cursor, max_bytes) = crate::enumerate::EnumerateRequest::decode(body)?;
            let mut writer = Writer::new(out);
            match fs.enumerate(rel, cursor, max_bytes) {
                Ok(page) => {
                    writer.u32(Status::Ok as u32);
                    let entries: Vec<crate::enumerate::DirectoryEntry> = page
                        .entries
                        .iter()
                        .map(|(name, kind)| crate::enumerate::DirectoryEntry {
                            kind: *kind,
                            name: name.as_bytes(),
                        })
                        .collect();
                    let response = EnumerateResponse {
                        next_cursor: page.next_cursor,
                        entries: &entries,
                    };
                    let used = writer.written();
                    let len = response.encode(&mut out[used..])?;
                    Ok(Served { kind: header.kind, len: used + len })
                }
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::Read => {
            let (policy, rel) = OpAddress::decode(body)?;
            let mut writer = Writer::new(out);
            match fs.property_read(policy, rel) {
                Ok(value) => {
                    writer.u32(Status::Ok as u32);
                    let used = writer.written();
                    let mut inner = Writer::new(&mut out[used..]);
                    if !inner.sized_bytes(value) {
                        return Err(DecodeError);
                    }
                    Ok(Served { kind: header.kind, len: used + inner.written() })
                }
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::Write => {
            let (policy, rel) = OpAddress::decode(body)?;
            let mut reader = reader_after_address(rel, body)?;
            let value = reader.sized_bytes()?;
            match fs.property_write(policy, rel, value) {
                Ok(()) => Ok(status_only(out, Status::Ok)),
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::Create => {
            let (_, rel, kind, attributes) = crate::op::CreateRequest::decode(body)?;
            match fs.create(rel, kind, attributes) {
                Ok(()) => Ok(status_only(out, Status::Ok)),
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::Link => {
            let (_, rel, target) = crate::op::LinkRequest::decode(body)?;
            match fs.link(rel, target) {
                Ok(()) => Ok(status_only(out, Status::Ok)),
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::Delete => {
            let (_, rel) = OpAddress::decode(body)?;
            match fs.delete(rel) {
                Ok(()) => Ok(status_only(out, Status::Ok)),
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::ReadAt => {
            let (policy, rel, offset, len) = crate::io::ReadAtRequest::decode(body)?;
            let mut writer = Writer::new(out);
            match fs.read_at(policy, rel, offset, len) {
                Ok(bytes) => {
                    writer.u32(Status::Ok as u32);
                    let used = writer.written();
                    let mut inner = Writer::new(&mut out[used..]);
                    if !inner.sized_bytes(bytes) {
                        return Err(DecodeError);
                    }
                    Ok(Served { kind: header.kind, len: used + inner.written() })
                }
                Err(status) => Ok(status_only(out, status)),
            }
        }
        Kind::WriteAt => {
            let (policy, rel, offset, bytes) = crate::io::WriteAtRequest::decode(body)?;
            let mut writer = Writer::new(out);
            match fs.write_at(policy, rel, offset, bytes) {
                Ok(written) => {
                    writer.u32(Status::Ok as u32);
                    writer.u32(written);
                    Ok(Served { kind: header.kind, len: writer.written() })
                }
                Err(status) => Ok(status_only(out, status)),
            }
        }
        // Move/Copy/Open（tunnel 交付）随后续批次接入。
        Kind::Move | Kind::Copy | Kind::Open => Ok(status_only(out, Status::Unsupported)),
    }
}

/// 寻址前奏之后的剩余 body 游标。
fn reader_after_address<'a>(rel: &[u8], body: &'a [u8]) -> Result<Reader<'a>, DecodeError> {
    let rest = &body[10 + rel.len()..];
    Ok(Reader::new(rest))
}

/// 构造完整应答 payload（RpcPrefix 由传输层前置）。
pub fn encode_reply(out: &mut [u8], kind: Kind, body: &[u8]) -> usize {
    let header = FalHeader::new(kind, (FAL_HEADER_LEN + body.len()) as u32);
    header.encode(&mut out[..FAL_HEADER_LEN]);
    out[FAL_HEADER_LEN..FAL_HEADER_LEN + body.len()].copy_from_slice(body);
    FAL_HEADER_LEN + body.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lookup::ResolvePolicy;
    use alloc::vec::Vec;
    use crate::lookup::LookupRequest;
    use crate::node::{NodeAttributes, NodeKind};

    /// 构造完整请求 payload（FalHeader + body）。
    fn build_request(kind: Kind, body: &[u8]) -> Vec<u8> {
        let mut buffer = vec![0u8; FAL_HEADER_LEN + body.len()];
        let header = FalHeader::new(kind, (FAL_HEADER_LEN + body.len()) as u32);
        header.encode(&mut buffer);
        buffer[FAL_HEADER_LEN..].copy_from_slice(body);
        buffer
    }

    fn address(rel: &[u8]) -> Vec<u8> {
        let mut buffer = vec![0u8; 10 + rel.len()];
        OpAddress { policy: ResolvePolicy::FollowAll, rel }.encode(&mut buffer).unwrap();
        buffer
    }

    #[test]
    fn lookup_found_via_dispatch() {
        let mut fs = MemFs::new();
        // 根下创建目录：前奏（rel 空）+ 类型 + 标记。
        let create_body = {
            let mut buffer = vec![0u8; 32];
            let used = {
                let mut writer = Writer::new(&mut buffer);
                writer.u32(ResolvePolicy::FollowAll as u32);
                writer.u32(0);
                writer.u16(5);
                assert!(writer.bytes(b"hello"));
                writer.u32(NodeKind::Directory as u32);
                writer.u32(NodeAttributes::READABLE.raw() | NodeAttributes::EXECUTABLE.raw());
                writer.written()
            };
            buffer.truncate(used);
            buffer
        };
        let request = build_request(Kind::Create, &create_body);
        let mut out = [0u8; 256];
        let served = serve(&mut fs, &request, &mut out).unwrap();
        assert_eq!(served.kind, Kind::Create);
        assert_eq!(u32::from_le_bytes([out[0], out[1], out[2], out[3]]), Status::Ok as u32);

        let mut buffer = vec![0u8; 64];
        let used = LookupRequest { policy: ResolvePolicy::FollowAll, path: b"hello" }
            .encode(&mut buffer)
            .unwrap();
        let request = build_request(Kind::Lookup, &buffer[..used]);
        let served = serve(&mut fs, &request, &mut out).unwrap();
        assert_eq!(served.kind, Kind::Lookup);
        let mut reader = Reader::new(&out[..served.len]);
        assert_eq!(reader.u32().unwrap(), Status::Ok as u32);
        assert_eq!(reader.u32().unwrap(), 0);
        let (kind, _, _, _) = crate::lookup::NodeInfo::decode(&out[8..served.len]).unwrap();
        assert_eq!(kind, NodeKind::Directory);
    }

    #[test]
    fn symlink_boundary_dispatch() {
        let mut fs = MemFs::new();
        let body = {
            let mut buffer = vec![0u8; 64];
            let used = {
                let mut writer = Writer::new(&mut buffer);
                writer.u32(ResolvePolicy::FollowAll as u32);
                writer.u32(0);
                writer.u16(3);
                assert!(writer.bytes(b"lnk"));
                writer.u16(3);
                assert!(writer.bytes(b"tgt"));
                writer.written()
            };
            buffer.truncate(used);
            buffer
        };
        let request = build_request(Kind::Link, &body);
        let mut out = [0u8; 256];
        serve(&mut fs, &request, &mut out).unwrap();

        let mut buffer = vec![0u8; 64];
        let used =
            LookupRequest { policy: ResolvePolicy::FollowAll, path: b"lnk" }.encode(&mut buffer)
                .unwrap();
        let request = build_request(Kind::Lookup, &buffer[..used]);
        let served = serve(&mut fs, &request, &mut out).unwrap();
        let mut reader = Reader::new(&out[..served.len]);
        assert_eq!(reader.u32().unwrap(), Status::Ok as u32);
        assert_eq!(reader.u32().unwrap(), 1); // 链接边界变体
    }

    #[test]
    fn truncated_request_rejected() {
        let mut fs = MemFs::new();
        let mut out = [0u8; 64];
        assert!(serve(&mut fs, &[0u8; 8], &mut out).is_err());
    }

    #[test]
    fn reply_encoding_roundtrip() {
        let mut body = [0u8; 8];
        let mut out = [0u8; 32];
        let served_len = encode_reply(&mut out, Kind::Delete, &body);
        assert_eq!(served_len, FAL_HEADER_LEN + 8);
        let header = FalHeader::decode(&out[..FAL_HEADER_LEN]).unwrap();
        assert_eq!(header.kind, Kind::Delete);
        assert_eq!(header.total_len as usize, FAL_HEADER_LEN + 8);
    }
}
