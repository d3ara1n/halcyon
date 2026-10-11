//! 正式 provider 共享 Node/Grant 上限：真实拒绝、另一入口及退休后恢复。
use super::{deadline, limits};
use alloc::{string::String, vec::Vec};
use erhino_shared::object::Rights;
use libfs::{
    FsRights,
    client::{Client, ClientError},
    node::NodeKind,
    protocol::Status,
};
use libkernel::ipc::message::MailboxSender;
use liboal::Namespace;
use librpc::Caller;
use srv_fs::deployment::{GRANTS, NODES};

pub(super) fn check_nodes(namespace: &Namespace, caller: &mut Caller) {
    let base = "/quota-nodes";
    let recovered_b = "/tenant-b/quota-nodes/recovered";
    let mut client = Client::new(namespace, caller, limits());
    client
        .create(base, NodeKind::Directory, FsRights::ALL, &[], deadline())
        .expect("node quota directory create failed");
    let mut held: Vec<String> = Vec::new();
    held.try_reserve_exact(NODES)
        .expect("node quota storage failed");
    let mut quota = false;
    // NODES 是包括 H 所有的 root 在内的总额度，不按猜测的现有占用减出魔数。
    // 一个追加探测只用于核实际 Quota；成功项不可能用完包含 root 的整个 NODES。
    for index in 0..=NODES {
        let path = alloc::format!("{base}/n-{index}");
        match client.create(&path, NodeKind::Directory, FsRights::ALL, &[], deadline()) {
            Ok(()) => held.push(path),
            Err(ClientError::Status(Status::Quota)) => {
                quota = true;
                break;
            }
            Err(error) => panic!("node quota admission failed: {error:?}"),
        }
    }
    assert!(
        quota && !held.is_empty() && held.len() < NODES,
        "provider node quota missing"
    );
    assert!(
        matches!(
            client.create(
                recovered_b,
                NodeKind::Directory,
                FsRights::ALL,
                &[],
                deadline()
            ),
            Err(ClientError::Status(Status::Quota))
        ),
        "other entry bypassed shared node quota"
    );
    let released = held.pop().expect("node quota fixture owner missing");
    client
        .remove(&released, deadline())
        .expect("node quota owner removal failed");
    let until = deadline();
    loop {
        match client.create(recovered_b, NodeKind::Directory, FsRights::ALL, &[], until) {
            Ok(()) => break,
            Err(ClientError::Status(Status::Quota)) => assert!(
                !libkernel::time::expired(until).expect("node recovery clock failed"),
                "removed node did not retire and refund"
            ),
            Err(error) => panic!("node quota recovery failed: {error:?}"),
        }
    }
    // 只有实际再准入才确认退款，不把 Remove 回复当作物理退休。
    assert!(
        matches!(
            client.create(
                &released,
                NodeKind::Directory,
                FsRights::ALL,
                &[],
                deadline()
            ),
            Err(ClientError::Status(Status::Quota))
        ),
        "recovered node failed to consume shared capacity"
    );
    client
        .remove(recovered_b, deadline())
        .expect("recovered node removal failed");
    for path in held {
        client
            .remove(&path, deadline())
            .expect("node quota fixture cleanup failed");
    }
    client
        .remove(base, deadline())
        .expect("node quota directory cleanup failed");
    libkernel::debug!("FS1 shared Node quota, other entry rejection and owner refund passed");
}

fn derive(
    client: &mut Client<'_>,
    path: &str,
    until: erhino_shared::time::Deadline,
) -> Result<MailboxSender, ClientError> {
    let transport = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    client.derive_directory(
        path,
        FsRights::TRAVERSE | FsRights::READ_PROPERTY | FsRights::ENUMERATE,
        transport,
        transport,
        until,
    )
}

pub(super) fn check_grants(namespace: &Namespace, caller: &mut Caller) {
    let base = "/quota-grants";
    let other = "/tenant-b/quota-grants";
    let mut client = Client::new(namespace, caller, limits());
    client
        .create(base, NodeKind::Directory, FsRights::ALL, &[], deadline())
        .expect("grant quota directory create failed");
    let mut held = Vec::new();
    held.try_reserve_exact(GRANTS)
        .expect("grant quota storage failed");
    let mut quota = false;
    for _ in 0..=GRANTS {
        match derive(&mut client, base, deadline()) {
            Ok(sender) => held.push(sender),
            Err(ClientError::Status(Status::Quota)) => {
                quota = true;
                break;
            }
            Err(error) => panic!("grant quota admission failed: {error:?}"),
        }
    }
    assert!(
        quota && !held.is_empty() && held.len() < GRANTS,
        "provider grant quota missing"
    );
    assert!(
        matches!(
            derive(&mut client, other, deadline()),
            Err(ClientError::Status(Status::Quota))
        ),
        "other entry bypassed shared grant quota"
    );
    // 关闭最后真实 sender，随后实际准入确认 Lifetime/条目/来源已退役。
    drop(held.pop().expect("grant quota sender owner missing"));
    let until = deadline();
    let recovered = loop {
        match derive(&mut client, other, until) {
            Ok(sender) => break sender,
            Err(ClientError::Status(Status::Quota)) => assert!(
                !libkernel::time::expired(until).expect("grant recovery clock failed"),
                "closed grant owner did not retire and refund"
            ),
            Err(error) => panic!("grant quota recovery failed: {error:?}"),
        }
    };
    assert!(
        matches!(
            derive(&mut client, base, deadline()),
            Err(ClientError::Status(Status::Quota))
        ),
        "recovered grant failed to consume shared capacity"
    );
    drop(recovered);
    drop(held);
    // 再取得一项有界成功发行，确保批量关闭没有留下整个表的占用。
    let until = deadline();
    loop {
        match derive(&mut client, base, until) {
            Ok(sender) => {
                drop(sender);
                break;
            }
            Err(ClientError::Status(Status::Quota)) => assert!(
                !libkernel::time::expired(until).expect("grant drain clock failed"),
                "grant owners did not drain"
            ),
            Err(error) => panic!("grant quota drain failed: {error:?}"),
        }
    }
    client
        .remove(base, deadline())
        .expect("grant quota directory cleanup failed");
    libkernel::debug!("FS1 shared Grant quota, other entry rejection and owner refund passed");
}
