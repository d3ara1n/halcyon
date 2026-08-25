//! fs：内存 FAL 提供者 + 同进程自客户端的验收负载。
//!
//! 提供者与客户端同居一进程：每次请求仍经内核 mailbox 真路径往返
//! （send → provider receive → serve → send-once 回复 → 客户端 receive），
//! 「泵」在等待回复期间服务提供者邮箱。演示场景覆盖：目录/属性/流
//! 创建、目录枚举分页、属性读写（含 Array 类型系统）、符号链接边界
//! 与客户端展开、偏移读写。

#![no_std]

use alloc::{fmt::Write, string::String, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    message::HandleMove,
    object::{Handle, HandlePair, ObjectSignals, Rights},
    wait::{WaitItem, WaitReason},
};
use libfal::{
    enumerate,
    header::{FalHeader, Kind, Status},
    io,
    lookup::{self, LookupRequest, NodeInfo, ResolvePolicy},
    memfs::MemFs,
    node::{NodeAttributes, NodeKind},
    op,
    property::{self, EncodedItem},
    provider,
    PROTOCOL_ID,
};
use libfs::{
    prefix::PrefixTable,
    resolve::{LookupOutcome, Position, WalkTransport},
};
use rinlib::{
    ipc::{
        message::{create as mailbox_create, make_send_once, receive, send},
        object::duplicate,
        wait::wait_many,
    },
    preclude::*,
};

/// 泵等待的期限（毫秒）：演示负载的诊断上限，正常往返毫秒级完成。
const PUMP_DEADLINE_MS: u64 = 500;

struct Fs {
    provider: MemFs,
    owner: Handle,
    peer: Handle,
    reply: HandlePair,
    txid: u64,
}

impl Fs {
    fn new() -> Self {
        let provider_mailbox = mailbox_create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
        )
        .expect("provider mailbox create failed");
        let reply = mailbox_create(
            Rights::READ | Rights::WAIT,
            Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
        )
        .expect("reply mailbox create failed");
        Self {
            provider: MemFs::new(),
            owner: provider_mailbox.owner,
            peer: provider_mailbox.peer,
            reply,
            txid: 1,
        }
    }

    /// 一次经内核 mailbox 的调用-服务往返；返回 FalHeader 之后的应答 body。
    fn call(&mut self, kind: Kind, body: &[u8], anchor: Handle) -> Result<Vec<u8>, Status> {
        self.txid += 1;
        let mut payload = [0u8; 512];
        let used = build_request(&mut payload, self.txid, kind, body);

        // slot 0：一次性回复授权（携 TRANSFER 以便随消息转移）；
        // slot 1：帧锚目录（副本，不消耗本地 grant）。
        let reply_once = make_send_once(self.reply.peer, Rights::WRITE | Rights::TRANSFER)
            .map_err(map_system)?;
        let anchor_dup = duplicate(anchor, Rights::WRITE | Rights::TRANSFER)
            .map_err(map_system)?;
        let moves = [
            HandleMove { handle: reply_once, rights: Rights::WRITE | Rights::TRANSFER },
            HandleMove { handle: anchor_dup, rights: Rights::WRITE },
        ];
        send(self.peer, PROTOCOL_ID, &payload[..used], &moves).map_err(map_system)?;

        loop {
            let items = [
                WaitItem::new(self.owner, ObjectSignals::READABLE, 0),
                WaitItem::new(self.reply.owner, ObjectSignals::READABLE, 1),
            ];
            let result = wait_many(&items, PUMP_DEADLINE_MS).map_err(map_system)?;
            if result.reason == WaitReason::Deadline as u32 {
                return Err(Status::Internal);
            }
            if result.item_index == 1 {
                break;
            }
            self.serve_one();
        }

        let message = receive(self.reply.owner).map_err(map_system)?;
        if message.header.kind != PROTOCOL_ID || !message.handles.is_empty() {
            return Err(Status::Internal);
        }
        let prefix = librpc::RpcPrefix::decode(&message.payload)
            .map_err(|_| Status::Internal)?;
        if prefix.kind != librpc::RpcMessageKind::Response || prefix.txid != self.txid {
            return Err(Status::Internal);
        }
        let header_start = librpc::PREFIX_LEN;
        let header = FalHeader::decode(
            &message.payload[header_start..header_start + libfal::FAL_HEADER_LEN],
        )
        .map_err(|_| Status::Internal)?;
        if header.kind != kind {
            return Err(Status::Internal);
        }
        let total = librpc::PREFIX_LEN + libfal::FAL_HEADER_LEN;
        Ok(message.payload[total..].to_vec())
    }

    /// 服务提供者邮箱的队头请求：解码 → memfs → 经 send-once 回复。
    fn serve_one(&mut self) {
        let message = receive(self.owner).expect("provider receive failed");
        if message.header.kind != PROTOCOL_ID || message.handles.len() < 1 {
            return;
        }
        let prefix = match librpc::RpcPrefix::decode(&message.payload) {
            Ok(prefix) => prefix,
            Err(_) => return,
        };
        let request = &message.payload[librpc::PREFIX_LEN..];
        let mut out = [0u8; 512];
        let served = match provider::serve(&mut self.provider, request, &mut out) {
            Ok(served) => served,
            Err(_) => {
                // 解码违约：以 Internal 状态应答，保持协议闭环。
                let mut writer = libfal::bytes::Writer::new(&mut out);
                writer.u32(Status::Internal as u32);
                writer.u32(0);
                provider::Served { kind: Kind::Lookup, len: writer.written() }
            }
        };

        let mut reply = [0u8; 544];
        let prefix_out = librpc::RpcPrefix::new(librpc::RpcMessageKind::Response, prefix.txid);
        prefix_out.encode(&mut reply);
        let len = provider::encode_reply(
            &mut reply[librpc::PREFIX_LEN..],
            served.kind,
            &out[..served.len],
        );
        // 单 outstanding 调用下回复箱至多一条在途，永不触满。
        send(message.handles[0], PROTOCOL_ID, &reply[..librpc::PREFIX_LEN + len], &[])
            .expect("reply send failed");
    }
}

fn build_request(out: &mut [u8], txid: u64, kind: Kind, body: &[u8]) -> usize {
    let prefix = librpc::RpcPrefix::new(librpc::RpcMessageKind::Request, txid);
    prefix.encode(out);
    let header = FalHeader::new(kind, (libfal::FAL_HEADER_LEN + body.len()) as u32);
    header.encode(&mut out[librpc::PREFIX_LEN..]);
    let start = librpc::PREFIX_LEN + libfal::FAL_HEADER_LEN;
    out[start..start + body.len()].copy_from_slice(body);
    start + body.len()
}

fn map_system(error: SystemCallError) -> Status {
    debug!("fs: syscall rejected: {:?}", error);
    Status::Internal
}

/// 应答状态字解析；Ok 时返回状态之后的应答 body。
fn expect_ok(reply: &[u8]) -> Result<&[u8], Status> {
    if reply.len() < 4 {
        return Err(Status::Internal);
    }
    let status = u32::from_le_bytes([reply[0], reply[1], reply[2], reply[3]]);
    match Status::from_u32(status) {
        Some(Status::Ok) => Ok(&reply[4..]),
        Some(other) => Err(other),
        None => Err(Status::Internal),
    }
}

impl WalkTransport for Fs {
    fn lookup(
        &mut self,
        dir: Handle,
        policy: ResolvePolicy,
        path: &str,
    ) -> Result<LookupOutcome, Status> {
        let mut body = [0u8; 128];
        let used = LookupRequest { policy, path: path.as_bytes() }
            .encode(&mut body)
            .map_err(|_| Status::IllegalPath)?;
        let reply = self.call(Kind::Lookup, &body[..used], dir)?;
        let rest = expect_ok(&reply)?;
        if rest.len() < 4 {
            return Err(Status::Internal);
        }
        let variant = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        match variant {
            0 => {
                let (kind, attributes, size, _) =
                    NodeInfo::decode(&rest[4..]).map_err(|_| Status::Internal)?;
                Ok(LookupOutcome::Found(libfs::resolve::NodeSummary { kind, attributes, size }))
            }
            1 => {
                let (consumed, target, remaining) =
                    lookup::LinkBoundary::decode(&rest[4..]).map_err(|_| Status::Internal)?;
                let to_string = |bytes: &[u8]| {
                    String::from_utf8(bytes.to_vec()).map_err(|_| Status::Internal)
                };
                Ok(LookupOutcome::Link {
                    consumed: to_string(consumed)?,
                    target: to_string(target)?,
                    remaining: to_string(remaining)?,
                })
            }
            _ => Err(Status::Internal),
        }
    }
}

impl Fs {
    fn create(&mut self, position: &Position, name: &str, kind: NodeKind, attributes: NodeAttributes) -> Result<(), Status> {
        let rel = join_rel(&position.rel, name);
        let body = {
            let mut buffer = [0u8; 128];
            let used = op::CreateRequest {
                address: op::OpAddress { policy: ResolvePolicy::FollowAll, rel: rel.as_bytes() },
                kind,
                attributes,
            }
            .encode(&mut buffer)
            .map_err(|_| Status::IllegalPath)?;
            buffer[..used].to_vec()
        };
        let reply = self.call(Kind::Create, &body, position.anchor)?;
        expect_ok(&reply).map(|_| ())
    }

    fn link(&mut self, position: &Position, name: &str, target: &str) -> Result<(), Status> {
        let rel = join_rel(&position.rel, name);
        let body = {
            let mut buffer = [0u8; 128];
            let used = op::LinkRequest {
                address: op::OpAddress { policy: ResolvePolicy::FollowAll, rel: rel.as_bytes() },
                target: target.as_bytes(),
            }
            .encode(&mut buffer)
            .map_err(|_| Status::IllegalPath)?;
            buffer[..used].to_vec()
        };
        let reply = self.call(Kind::Link, &body, position.anchor)?;
        expect_ok(&reply).map(|_| ())
    }

    fn enumerate(
        &mut self,
        position: &Position,
    ) -> Result<Vec<(String, NodeKind)>, Status> {
        let mut entries = Vec::new();
        let mut cursor = 0u64;
        loop {
            let body = {
                let mut buffer = [0u8; 128];
                let request = enumerate::EnumerateRequest {
                    rel: position.rel.as_bytes(),
                    cursor,
                    max_bytes: 256,
                };
                let used = request.encode(&mut buffer);
                buffer[..used].to_vec()
            };
            let reply = self.call(Kind::Enumerate, &body, position.anchor)?;
            let rest = expect_ok(&reply)?;
            let (next, count, entry_bytes) =
                enumerate::decode_response_header(rest).map_err(|_| Status::Internal)?;
            for item in enumerate::decode_entries(entry_bytes, count)
                .map_err(|_| Status::Internal)?
            {
                let item = item.map_err(|_| Status::Internal)?;
                entries.push((
                    String::from_utf8(item.name.to_vec()).map_err(|_| Status::Internal)?,
                    item.kind,
                ));
            }
            if next == 0 {
                return Ok(entries);
            }
            cursor = next;
        }
    }

    fn write(&mut self, position: &Position, value: &[u8]) -> Result<(), Status> {
        let body = {
            let mut buffer = [0u8; 256];
            let mut writer = libfal::bytes::Writer::new(&mut buffer);
            writer.u32(ResolvePolicy::FollowAll as u32);
            writer.u32(0);
            writer.u16(position.rel.len() as u16);
            writer.bytes(position.rel.as_bytes());
            if !writer.sized_bytes(value) {
                return Err(Status::IllegalArgument);
            }
            let used = writer.written();
            buffer[..used].to_vec()
        };
        let reply = self.call(Kind::Write, &body, position.anchor)?;
        expect_ok(&reply).map(|_| ())
    }

    fn read(&mut self, position: &Position) -> Result<Vec<u8>, Status> {
        let body = {
            let mut buffer = [0u8; 64];
            let used = op::PropertyReadRequest {
                address: op::OpAddress {
                    policy: ResolvePolicy::FollowAll,
                    rel: position.rel.as_bytes(),
                },
            }
            .encode(&mut buffer)
            .map_err(|_| Status::IllegalPath)?;
            buffer[..used].to_vec()
        };
        let reply = self.call(Kind::Read, &body, position.anchor)?;
        let rest = expect_ok(&reply)?;
        let mut reader = libfal::bytes::Reader::new(rest);
        let value = reader.sized_bytes().map_err(|_| Status::Internal)?;
        Ok(value.to_vec())
    }

    fn read_at(&mut self, position: &Position, offset: u64, len: u32) -> Result<Vec<u8>, Status> {
        let body = {
            let mut buffer = [0u8; 96];
            let used = io::ReadAtRequest {
                address: op::OpAddress {
                    policy: ResolvePolicy::FollowAll,
                    rel: position.rel.as_bytes(),
                },
                offset,
                len,
            }
            .encode(&mut buffer)
            .map_err(|_| Status::IllegalPath)?;
            buffer[..used].to_vec()
        };
        let reply = self.call(Kind::ReadAt, &body, position.anchor)?;
        let rest = expect_ok(&reply)?;
        let mut reader = libfal::bytes::Reader::new(rest);
        let bytes = reader.sized_bytes().map_err(|_| Status::Internal)?;
        Ok(bytes.to_vec())
    }

    fn write_at(&mut self, position: &Position, offset: u64, bytes: &[u8]) -> Result<u32, Status> {
        let body = {
            let mut buffer = [0u8; 192];
            let used = io::WriteAtRequest {
                address: op::OpAddress {
                    policy: ResolvePolicy::FollowAll,
                    rel: position.rel.as_bytes(),
                },
                offset,
                bytes,
            }
            .encode(&mut buffer)
            .map_err(|_| Status::IllegalPath)?;
            buffer[..used].to_vec()
        };
        let reply = self.call(Kind::WriteAt, &body, position.anchor)?;
        let rest = expect_ok(&reply)?;
        if rest.len() < 4 {
            return Err(Status::Internal);
        }
        Ok(u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]))
    }
}

fn join_rel(base: &str, name: &str) -> String {
    if base.is_empty() {
        String::from(name)
    } else {
        let mut joined = String::from(base);
        joined.push('/');
        joined.push_str(name);
        joined
    }
}

fn main() {
    debug!("Hello, fs!");
    let mut fs = Fs::new();
    let mut table = PrefixTable::new();
    table.mount("/", fs.peer).expect("mount root failed");
    let rw = NodeAttributes::READABLE | NodeAttributes::WRITEABLE;
    let rwx = rw | NodeAttributes::EXECUTABLE;

    let root = libfs::resolve::resolve(&mut fs, &table, "/", ResolvePolicy::FollowAll)
        .expect("root resolve failed");

    // 目录与属性创建。
    fs.create(&root, "hello", NodeKind::Directory, rwx).expect("create /hello failed");
    let hello = libfs::resolve::resolve(&mut fs, &table, "/hello", ResolvePolicy::FollowAll)
        .expect("resolve /hello failed");
    fs.create(&hello, "world", NodeKind::Property, rw).expect("create /hello/world failed");

    // 属性写读：Integers（Array<Integer> 类型系统往返）。
    let world = libfs::resolve::resolve(&mut fs, &table, "/hello/world", ResolvePolicy::FollowAll)
        .expect("resolve /hello/world failed");
    let first = 114514i64.to_le_bytes();
    let second = (-1919810i64).to_le_bytes();
    let items = [EncodedItem(&first), EncodedItem(&second)];
    let encoded = {
        let mut buffer = [0u8; 64];
        let used = property::PropertyValue::Array { element: property::ValueType::Integer, items: &items }
            .encode(&mut buffer)
            .expect("property encode failed");
        buffer[..used].to_vec()
    };
    fs.write(&world, &encoded).expect("property write failed");
    let value = fs.read(&world).expect("property read failed");
    match property::DecodedValue::decode(&value).expect("property decode failed") {
        property::DecodedValue::Array { element, body } => {
            debug!("world = Array<{:?}> {} bytes", element, body.len());
        }
        other => panic!("unexpected property value: {:?}", other),
    }

    // 符号链接：创建后 FollowAll 解析应展开至 world 属性。
    fs.link(&hello, "lnk", "world").expect("create /hello/lnk failed");
    let via_link =
        libfs::resolve::resolve(&mut fs, &table, "/hello/lnk", ResolvePolicy::FollowAll)
            .expect("symlink resolve failed");
    assert_eq!(via_link.info.kind, NodeKind::Property);
    assert_eq!(via_link.rel, "hello/world");
    debug!("symlink /hello/lnk -> resolved to {:?}", via_link.info.kind);

    // 流：偏移写读。
    fs.create(&root, "bin", NodeKind::Directory, rwx).expect("create /bin failed");
    let bin = libfs::resolve::resolve(&mut fs, &table, "/bin", ResolvePolicy::FollowAll)
        .expect("resolve /bin failed");
    fs.create(&bin, "srv_init", NodeKind::Stream, rw).expect("create /bin/srv_init failed");
    let stream = libfs::resolve::resolve(&mut fs, &table, "/bin/srv_init", ResolvePolicy::FollowAll)
        .expect("resolve /bin/srv_init failed");
    fs.write_at(&stream, 0, &[0x7f, b'E', b'L', b'F', 2, 1, 1, 0])
        .expect("write_at failed");
    let magic = fs.read_at(&stream, 0, 8).expect("read_at failed");
    debug!("srv_init first 8 bytes: {:x?}", magic);

    // 目录枚举分页（页预算压到单页两项以验证 cursor 续查）。
    let mut listing = String::from("root entries:\n");
    let entries = fs.enumerate(&root).expect("enumerate / failed");
    for (name, kind) in &entries {
        writeln!(&mut listing, "  {} {:?}", name, kind).unwrap();
    }
    let hello_entries = fs.enumerate(&hello).expect("enumerate /hello failed");
    for (name, kind) in &hello_entries {
        writeln!(&mut listing, "  hello/{:?} {:?}", name, kind).unwrap();
    }
    debug!("{}", listing);
    assert!(entries.iter().any(|(name, _)| name == "hello"));
    assert!(hello_entries.iter().any(|(name, _)| name == "world"));

    debug!("fs acceptance passed");
}
