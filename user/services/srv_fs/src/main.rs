//! fs：内存 FAL 提供者 + 同进程自客户端的验收负载。
//!
//! 提供者与客户端同居一进程：每次请求仍经内核 mailbox 真路径往返
//! （send → provider receive → serve → send-once 回复 → 客户端 receive），
//! 「泵」在等待回复期间服务提供者邮箱。演示场景覆盖：目录/属性/流
//! 创建、目录枚举分页、属性读写（含 Array 类型系统）、符号链接边界
//! 与客户端展开、偏移读写。

#![no_std]

mod server;

use alloc::{fmt::Write, string::String, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};
use libfal::lookup::ResolvePolicy::{FollowAll, NoFollowFinal};
use libfal::{
    PROTOCOL_ID, enumerate,
    header::{FalHeader, Kind, Status},
    io,
    lookup::{self, LookupRequest, NodeInfo, ResolvePolicy},
    node::{NodeAttributes, NodeKind},
    op,
    property::{self, EncodedItem},
    provider,
};
use libfs::{
    prefix::PrefixTable,
    resolve::{LookupOutcome, Position, WalkTransport},
};
use rinlib::{
    ipc::{
        capability::Capability,
        message::{Mailbox, MailboxSender},
        object::duplicate,
    },
    preclude::*,
};

/// 应答承载：消息上限扣除 RpcPrefix 与 FalHeader——由协议常量推导。
const REPLY_BODY_MAX: usize =
    erhino_shared::message::PAYLOAD_MAX - librpc::PREFIX_LEN - libfal::FAL_HEADER_LEN;

// 承载交叉校验：最坏属性值应答（状态字 + sized 前缀 + 满尺寸值）必须
// 装得下——VALUE_MAX 由同一条路径推导，此断言钉住两侧不漂移。
const _: () = assert!(
    REPLY_BODY_MAX >= 4 + 2 + libfal::property::VALUE_MAX,
    "reply budget must carry maximal property value"
);

/// 解码/编码违约时的 Internal 状态应答。
fn internal_served(out: &mut [u8]) -> provider::Served {
    let mut writer = libfal::bytes::Writer::new(out);
    writer.reserve(8);
    writer.u32(Status::Internal as u32);
    writer.u32(0);
    provider::Served {
        kind: Kind::Lookup,
        len: writer.written(),
    }
}

/// 路径长度协议校验：超长属输入违约，在组包入口显式拒绝——编码层
/// 对合法输入不可失败。
fn check_path_len(bytes: &[u8]) -> Result<(), Status> {
    if bytes.len() > libfal::PATH_MAX {
        Err(Status::IllegalPath)
    } else {
        Ok(())
    }
}

struct Fs {
    sender: MailboxSender,
    caller: librpc::Caller,
    worker: Option<rinlib::thread::JoinHandle<()>>,
}

impl Fs {
    fn new() -> Self {
        let owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
            .expect("provider mailbox create failed");
        let minted = owner
            .mint(
                0,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
            )
            .expect("provider sender mint failed");
        let worker = rinlib::thread::Builder::new()
            .spawn(move || server::run(owner))
            .expect("provider Runtime thread spawn failed");
        Self {
            sender: minted.sender,
            caller: librpc::Caller::new(),
            worker: Some(worker),
        }
    }

    /// 一次经内核 mailbox 的调用-服务往返；返回 FalHeader 之后的应答 body。
    fn call(&mut self, kind: Kind, body: &[u8], anchor: Handle) -> Result<Vec<u8>, Status> {
        if libfal::FAL_HEADER_LEN + body.len()
            > erhino_shared::message::PAYLOAD_MAX - librpc::PREFIX_LEN
        {
            return Err(Status::IllegalArgument);
        }
        let mut payload = alloc::vec![0u8; libfal::FAL_HEADER_LEN + body.len()];
        FalHeader::new(kind, payload.len() as u32).encode(&mut payload);
        payload[libfal::FAL_HEADER_LEN..].copy_from_slice(body);
        let mut request = librpc::Request::new(PROTOCOL_ID, &payload).map_err(map_system)?;
        let anchor_dup = unsafe {
            Capability::from_raw(
                duplicate(anchor, Rights::WRITE | Rights::TRANSIT).map_err(map_system)?,
            )
        };
        request
            .push(anchor_dup, Rights::WRITE)
            .map_err(|failure| map_system(failure.error))?;
        let reply = self
            .caller
            .call(&self.sender, rinlib::time::Deadline::INFINITE, request)
            .map_err(|error| {
                debug!("fs: RPC call failed: {:?}", error);
                Status::Internal
            })?;
        if !reply.handles.is_empty() {
            return Err(Status::Internal);
        }
        if reply.payload.len() < libfal::FAL_HEADER_LEN {
            return Err(Status::Internal);
        }
        let header = FalHeader::decode(&reply.payload[..libfal::FAL_HEADER_LEN])
            .map_err(|_| Status::Internal)?;
        if header.kind != kind {
            return Err(Status::Internal);
        }
        Ok(reply.payload[libfal::FAL_HEADER_LEN..].to_vec())
    }

    fn shutdown(&mut self) {
        self.sender
            .send(server::STOP_KIND, &[])
            .expect("provider stop send failed");
        self.worker
            .take()
            .expect("provider worker already joined")
            .join();
    }
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
        let check = check_path_len(path.as_bytes());
        check?;
        let request = LookupRequest {
            policy,
            path: path.as_bytes(),
        };
        let mut body = alloc::vec![0u8; request.encoded_len()];
        let used = request.encode(&mut body);
        let reply = self.call(Kind::Lookup, &body[..used], dir)?;
        let rest = expect_ok(&reply)?;
        if rest.len() < 4 {
            return Err(Status::Internal);
        }
        let variant = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        match variant {
            0 => {
                let (kind, attributes, size, value) =
                    NodeInfo::decode(&rest[4..]).map_err(|_| Status::Internal)?;
                Ok(LookupOutcome::Found(libfs::resolve::NodeSummary {
                    kind,
                    attributes,
                    size,
                    value: alloc::vec::Vec::from(value),
                }))
            }
            1 => {
                let (consumed, target, remaining) =
                    lookup::LinkBoundary::decode(&rest[4..]).map_err(|_| Status::Internal)?;
                let to_string =
                    |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| Status::Internal);
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
    fn create(
        &mut self,
        position: &Position,
        name: &str,
        kind: NodeKind,
        attributes: NodeAttributes,
    ) -> Result<(), Status> {
        let rel = join_rel(&position.rel, name);
        check_path_len(rel.as_bytes())?;
        let request = op::CreateRequest {
            address: op::OpAddress {
                policy: FollowAll,
                rel: rel.as_bytes(),
            },
            kind,
            attributes,
        };
        let mut buffer = alloc::vec![0u8; request.encoded_len()];
        let used = request.encode(&mut buffer);
        let reply = self.call(Kind::Create, &buffer[..used], position.anchor)?;
        expect_ok(&reply).map(|_| ())
    }

    fn link(&mut self, position: &Position, name: &str, target: &str) -> Result<(), Status> {
        let rel = join_rel(&position.rel, name);
        check_path_len(rel.as_bytes())?;
        check_path_len(target.as_bytes())?;
        let request = op::LinkRequest {
            address: op::OpAddress {
                policy: FollowAll,
                rel: rel.as_bytes(),
            },
            target: target.as_bytes(),
        };
        let mut buffer = alloc::vec![0u8; request.encoded_len()];
        let used = request.encode(&mut buffer);
        let reply = self.call(Kind::Link, &buffer[..used], position.anchor)?;
        expect_ok(&reply).map(|_| ())
    }

    fn enumerate(&mut self, position: &Position) -> Result<Vec<(String, NodeKind)>, Status> {
        let mut entries = Vec::new();
        let mut cursor = 0u64;
        loop {
            check_path_len(position.rel.as_bytes())?;
            let request = enumerate::EnumerateRequest {
                rel: position.rel.as_bytes(),
                cursor,
                max_bytes: 256,
            };
            let mut buffer = alloc::vec![0u8; request.encoded_len()];
            let used = request.encode(&mut buffer);
            let reply = self.call(Kind::Enumerate, &buffer[..used], position.anchor)?;
            let rest = expect_ok(&reply)?;
            let (next, count, entry_bytes) =
                enumerate::decode_response_header(rest).map_err(|_| Status::Internal)?;
            for item in
                enumerate::decode_entries(entry_bytes, count).map_err(|_| Status::Internal)?
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
        check_path_len(position.rel.as_bytes())?;
        let request = op::WriteRequest {
            address: op::OpAddress {
                policy: FollowAll,
                rel: position.rel.as_bytes(),
            },
            value,
        };
        let mut buffer = alloc::vec![0u8; request.encoded_len()];
        let used = request.encode(&mut buffer);
        let reply = self.call(Kind::Write, &buffer[..used], position.anchor)?;
        expect_ok(&reply).map(|_| ())
    }

    fn read(&mut self, position: &Position) -> Result<Vec<u8>, Status> {
        check_path_len(position.rel.as_bytes())?;
        let address = op::OpAddress {
            policy: FollowAll,
            rel: position.rel.as_bytes(),
        };
        let mut buffer = alloc::vec![0u8; address.encoded_len()];
        let used = address.encode(&mut buffer);
        let reply = self.call(Kind::Read, &buffer[..used], position.anchor)?;
        let rest = expect_ok(&reply)?;
        let mut reader = libfal::bytes::Reader::new(rest);
        let value = reader.sized_bytes().map_err(|_| Status::Internal)?;
        Ok(value.to_vec())
    }

    fn read_at(&mut self, position: &Position, offset: u64, len: u32) -> Result<Vec<u8>, Status> {
        check_path_len(position.rel.as_bytes())?;
        let request = io::ReadAtRequest {
            address: op::OpAddress {
                policy: FollowAll,
                rel: position.rel.as_bytes(),
            },
            offset,
            len,
        };
        let mut buffer = alloc::vec![0u8; request.encoded_len()];
        let used = request.encode(&mut buffer);
        let reply = self.call(Kind::ReadAt, &buffer[..used], position.anchor)?;
        let rest = expect_ok(&reply)?;
        let mut reader = libfal::bytes::Reader::new(rest);
        let bytes = reader.sized_bytes().map_err(|_| Status::Internal)?;
        Ok(bytes.to_vec())
    }

    fn write_at(&mut self, position: &Position, offset: u64, bytes: &[u8]) -> Result<u32, Status> {
        check_path_len(position.rel.as_bytes())?;
        let request = io::WriteAtRequest {
            address: op::OpAddress {
                policy: FollowAll,
                rel: position.rel.as_bytes(),
            },
            offset,
            bytes,
        };
        let mut buffer = alloc::vec![0u8; request.encoded_len()];
        let used = request.encode(&mut buffer);
        let reply = self.call(Kind::WriteAt, &buffer[..used], position.anchor)?;
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
    assert!(
        table
            .mount("/", fs.sender.as_handle())
            .expect("mount root failed")
            .is_none()
    );
    // 调试参考：前缀表是 fs 进程的私有状态（无查询 ABI，init 不可见），
    // 开发期挂载后自打印内部状态。
    for entry in table.entries() {
        debug!(
            "\x1b[36mfs-table\x1b[0m: {} -> directory handle {:#x}",
            entry.prefix,
            entry.directory.raw()
        );
    }
    let rw = NodeAttributes::READABLE | NodeAttributes::WRITEABLE;
    let rwx = rw | NodeAttributes::EXECUTABLE;

    let root =
        libfs::resolve::resolve(&mut fs, &table, "/", FollowAll).expect("root resolve failed");

    // 目录与属性创建。
    fs.create(&root, "hello", NodeKind::Directory, rwx)
        .expect("create /hello failed");
    let hello = libfs::resolve::resolve(&mut fs, &table, "/hello", FollowAll)
        .expect("resolve /hello failed");
    fs.create(&hello, "world", NodeKind::Property, rw)
        .expect("create /hello/world failed");

    // 属性写读：Integers（Array<Integer> 类型系统往返）。
    let world = libfs::resolve::resolve(&mut fs, &table, "/hello/world", FollowAll)
        .expect("resolve /hello/world failed");
    let first = 114514i64.to_le_bytes();
    let second = (-1919810i64).to_le_bytes();
    let items = [EncodedItem(&first), EncodedItem(&second)];
    let encoded = {
        let mut buffer = [0u8; 64];
        let used = property::PropertyValue::Array {
            element: property::ValueType::Integer,
            items: &items,
        }
        .encode(&mut buffer);
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

    // 符号链接：NoFollowFinal 查询链接本身——target 随 NodeInfo value 尾返回。
    fs.link(&hello, "lnk", "world")
        .expect("create /hello/lnk failed");
    let link_node = libfs::resolve::resolve(&mut fs, &table, "/hello/lnk", NoFollowFinal)
        .expect("no-follow resolve failed");
    assert_eq!(link_node.info.kind, NodeKind::SymbolicLink);
    assert_eq!(link_node.info.value, b"world");
    debug!(
        "lnk target via Found value: {:?}",
        core::str::from_utf8(&link_node.info.value)
    );
    let via_link = libfs::resolve::resolve(&mut fs, &table, "/hello/lnk", FollowAll)
        .expect("symlink resolve failed");
    assert_eq!(via_link.info.kind, NodeKind::Property);
    assert_eq!(via_link.rel, "hello/world");
    debug!("symlink /hello/lnk -> resolved to {:?}", via_link.info.kind);

    // 流：偏移写读。
    fs.create(&root, "bin", NodeKind::Directory, rwx)
        .expect("create /bin failed");
    let bin =
        libfs::resolve::resolve(&mut fs, &table, "/bin", FollowAll).expect("resolve /bin failed");
    fs.create(&bin, "srv_init", NodeKind::Stream, rw)
        .expect("create /bin/srv_init failed");
    let stream = libfs::resolve::resolve(&mut fs, &table, "/bin/srv_init", FollowAll)
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
    fs.shutdown();
}
