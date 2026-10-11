//! init 原任务族上的真实 Copy 场景；没有独立运行体或域内假服务。
use super::{deadline, limits};
use crate::RootSupervisor;
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{message::PAYLOAD_MAX, object::Rights};
use libfs::{
    FsRights,
    client::{
        Client, ClientError,
        copy::{Cause, CopyTask, Creation, Kind, Outcome, Progress},
    },
    node::NodeKind,
    protocol::{IoEnd, Status},
    value::Value,
};
use libkernel::ipc::{capability::Capability, message::MailboxSender, object::duplicate};
use liboal::Namespace;
use librpc::{Caller, exchange::CallCause};

const CHUNK: usize = PAYLOAD_MAX - 256;
const LENGTH: usize = CHUNK * 6;
const SOURCE: &str = "/primary/init-copy-source";
const PROPERTY: &str = "/primary/init-copy-property";

fn namespace(
    primary: &MailboxSender,
    secondary: &MailboxSender,
) -> Result<Arc<Namespace>, &'static str> {
    let mut ns = Namespace::new();
    for (prefix, sender) in [("/primary", primary), ("/secondary", secondary)] {
        let raw = duplicate(
            sender.as_handle(),
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|_| "Copy entry narrowing failed")?;
        // SAFETY: duplicate 返回唯一新表项，typed owner 立即接管，不提升原权限。
        let cap = unsafe { Capability::from_raw(raw) };
        let (sender, description) =
            MailboxSender::from_capability(cap).map_err(|_| "Copy entry role invalid")?;
        if description.rights.contains(Rights::DUPLICATE) {
            return Err("Copy shared entry retained duplicate authority");
        }
        ns.bind(
            prefix,
            Arc::try_new(sender).map_err(|_| "Copy entry storage failed")?,
        )
        .map_err(|_| "Copy binding failed")?;
    }
    Arc::try_new(ns).map_err(|_| "Copy Namespace storage failed")
}
fn task(
    ns: &Arc<Namespace>,
    source: &str,
    destination: &str,
    kind: Kind,
) -> Result<CopyTask, &'static str> {
    let budget = limits()
        .start(
            libkernel::time::snapshot().map_err(|_| "Copy clock failed")?,
            deadline()?,
        )
        .map_err(|_| "Copy budget invalid")?;
    CopyTask::new(
        ns.clone(),
        source,
        destination,
        kind,
        FsRights::ALL,
        srv_fs::deployment::STREAM_TUNNEL_BYTES as u32,
        budget,
    )
    .map_err(|error| {
        libkernel::debug!("Copy preparation failed: {:?}", error);
        "Copy preparation failed"
    })
}
fn start(
    root: &mut RootSupervisor,
    tasks: impl IntoIterator<Item = CopyTask>,
) -> Result<(), &'static str> {
    let mut pending = Vec::new();
    for task in tasks {
        pending
            .try_reserve(1)
            .map_err(|_| "Copy batch storage failed")?;
        pending.push(task);
    }
    root.start_copies(pending)
}
fn finish(root: &mut RootSupervisor) -> Result<Vec<crate::CopyObservation>, &'static str> {
    root.await_read()?;
    let results = core::mem::take(&mut root.read_world().copies);
    root.close_read()?;
    Ok(results)
}
fn complete(root: &mut RootSupervisor, copy: CopyTask) -> Result<Progress, &'static str> {
    start(root, [copy])?;
    let results = finish(root)?;
    match results[0].outcome {
        Some(Outcome::Complete { progress }) => Ok(progress),
        other => {
            libkernel::debug!("Copy expected completion: {:?}", other);
            Err("Copy did not complete")
        }
    }
}
fn create(
    client: &mut Client<'_>,
    path: &str,
    kind: NodeKind,
    bytes: &[u8],
) -> Result<(), &'static str> {
    client
        .create(path, kind, FsRights::ALL, bytes, deadline()?)
        .map_err(|error| {
            libkernel::debug!("Copy fixture create failed at {}: {:?}", path, error);
            "Copy fixture create failed"
        })
}
fn pattern(offset: usize) -> u8 {
    ((offset * 37 + 11) % 251) as u8
}
fn source(client: &mut Client<'_>, path: &str) -> Result<(), &'static str> {
    create(client, path, NodeKind::Stream, &[])?;
    let mut bytes = [0; CHUNK];
    for offset in (0..LENGTH).step_by(CHUNK) {
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = pattern(offset + i);
        }
        let written = client
            .write_at(path, offset as u64, &bytes, deadline()?)
            .map_err(|_| "Copy fixture write failed")?;
        if written.count != CHUNK as u64 || written.cause != Status::Ok {
            return Err("Copy fixture write was partial");
        }
    }
    Ok(())
}
fn contents(client: &mut Client<'_>, path: &str, length: usize) -> Result<(), &'static str> {
    for offset in (0..length).step_by(CHUNK) {
        let count = CHUNK.min(length - offset);
        let read = client
            .read_at(path, offset as u64, count as u32, deadline()?)
            .map_err(|_| "Copy content read failed")?;
        if read.count != count as u64
            || read.cause != Status::Ok
            || read
                .data
                .iter()
                .enumerate()
                .any(|(i, b)| *b != pattern(offset + i))
        {
            return Err("Copy content differs from source");
        }
    }
    let end = client
        .read_at(path, length as u64, 1, deadline()?)
        .map_err(|_| "Copy EOF read failed")?;
    if end.count != 0 || end.end != IoEnd::Eof {
        return Err("Copy target length differs from source");
    }
    Ok(())
}
fn remove(client: &mut Client<'_>, path: &str) -> Result<(), &'static str> {
    client
        .remove(path, deadline()?)
        .map_err(|_| "Copy fixture removal failed")
}

pub(crate) fn exercise(
    root: &mut RootSupervisor,
    primary: &MailboxSender,
    secondary: &MailboxSender,
) -> Result<(), &'static str> {
    let ns = namespace(primary, secondary)?;
    let mut caller = Caller::new();
    let mut client = Client::new(&ns, &mut caller, limits());
    source(&mut client, SOURCE)?;
    let mut encoded = [0; 20];
    let used = Value::Unsigned64(0x81927364)
        .encode(&mut encoded)
        .map_err(|_| "Copy Property encoding failed")?;
    create(&mut client, PROPERTY, NodeKind::Property, &encoded[..used])?;
    complete(
        root,
        task(
            &ns,
            PROPERTY,
            "/secondary/init-copy-property",
            Kind::Property,
        )?,
    )?;
    if client
        .get::<u64>("/secondary/init-copy-property", deadline()?)
        .map_err(|_| "Copy Property read failed")?
        != 0x81927364
    {
        return Err("Copy Property differs from source");
    }
    libkernel::debug!("FAL2 independent property copy passed");
    let progress = complete(
        root,
        task(&ns, SOURCE, "/secondary/init-copy-stream", Kind::Stream)?,
    )?;
    if !matches!(progress,Progress::Stream(p) if p.local_read==LENGTH as u64 && p.transported==LENGTH as u64 && p.destination_confirmed==LENGTH as u64 && p.source_terminal.is_some() && p.destination_terminal.is_some())
    {
        return Err("Copy final progress invalid");
    }
    contents(&mut client, "/secondary/init-copy-stream", LENGTH)?;
    libkernel::debug!("FAL2 independent cross-provider stream Copy passed");
    libkernel::debug!("FAL2 independent property Copy and stream ReadAt passed");

    // 两个真实活任务共享同一无 DUP Namespace/Dispatcher；取消一个不取消另一个。
    start(
        root,
        [
            task(&ns, SOURCE, "/secondary/init-copy-cancel", Kind::Stream)?,
            task(&ns, SOURCE, "/secondary/init-copy-concurrent", Kind::Stream)?,
        ],
    )?;
    root.await_copy_progress(0, 1)?;
    // 暂不推进客户端 Copy，以正式 ReadAt 确认目标已提交非零前缀。
    // provider 独立推进；它同一次 advance 在 commit 后更新 accepted，
    // 因而后继 ReadAt 可见数据时已越过接受点，不依赖调度碰巧命中。
    let accepted_until = deadline()?;
    loop {
        let prefix = client
            .read_at("/secondary/init-copy-cancel", 0, 1, accepted_until)
            .map_err(|_| "Copy accepted prefix observation failed")?;
        if prefix.count != 0 {
            if prefix.data != [pattern(0)] {
                return Err("Copy accepted prefix differs from source");
            }
            break;
        }
        if libkernel::time::expired(accepted_until)
            .map_err(|_| "Copy accepted prefix clock failed")?
        {
            return Err("Copy destination did not accept transported prefix");
        }
    }
    root.cancel_copy(0)?;
    let outcomes = finish(root)?;
    let transported = match outcomes[0].outcome {
        Some(Outcome::Failed {
            creation: Creation::Created,
            progress: Progress::Stream(p),
            cause: Cause::Cancelled,
            cleanup_pending: false,
        }) if p.transported > 0
            && p.transported < LENGTH as u64
            && p.destination_confirmed <= p.transported =>
        {
            p.transported
        }
        other => {
            libkernel::debug!("Copy prefix cancellation result: {:?}", other);
            return Err("Copy cancellation result invalid");
        }
    };
    if !matches!(outcomes[1].outcome, Some(Outcome::Complete { .. })) {
        return Err("Copy cancellation affected other task");
    }
    contents(&mut client, "/secondary/init-copy-concurrent", LENGTH)?;
    let prefix = client
        .read_at("/secondary/init-copy-cancel", 0, CHUNK as u32, deadline()?)
        .map_err(|_| "Copy cancelled target disappeared")?;
    if prefix.count == 0
        || prefix
            .data
            .iter()
            .enumerate()
            .any(|(i, b)| *b != pattern(i))
    {
        return Err("Copy cancelled prefix corrupted");
    }
    let beyond = client
        .read_at("/secondary/init-copy-cancel", transported, 1, deadline()?)
        .map_err(|_| "Copy cancelled bound read failed")?;
    if beyond.count != 0 || beyond.end != IoEnd::Eof {
        return Err("Copy cancellation accepted beyond transported bound");
    }
    libkernel::debug!("FS1 Copy shared no-DUP concurrent cancellation passed");
    libkernel::debug!("FS1 Copy cancellation after destination accepted prefix passed");

    create(
        &mut client,
        "/primary/init-copy-empty",
        NodeKind::Stream,
        &[],
    )?;
    let empty = complete(
        root,
        task(
            &ns,
            "/primary/init-copy-empty",
            "/secondary/init-copy-empty",
            Kind::Stream,
        )?,
    )?;
    if !matches!(empty,Progress::Stream(p) if p.local_read==0 && p.transported==0 && p.destination_confirmed==0)
    {
        return Err("empty Copy progress invalid");
    }
    contents(&mut client, "/secondary/init-copy-empty", 0)?;
    start(
        root,
        [task(
            &ns,
            SOURCE,
            "/secondary/init-copy-stream",
            Kind::Stream,
        )?],
    )?;
    let conflict = finish(root)?;
    if !matches!(
        conflict[0].outcome,
        Some(Outcome::Failed {
            creation: Creation::NotCreated,
            cause: Cause::Status(Status::Exists),
            cleanup_pending: false,
            ..
        })
    ) {
        return Err("Copy replaced existing target");
    }
    contents(&mut client, "/secondary/init-copy-stream", LENGTH)?;
    let mut pre_cancel = task(
        &ns,
        SOURCE,
        "/secondary/init-copy-unattempted",
        Kind::Stream,
    )?;
    pre_cancel.stop();
    start(root, [pre_cancel])?;
    if !matches!(
        finish(root)?[0].outcome,
        Some(Outcome::Failed {
            creation: Creation::NotAttempted,
            cleanup_pending: false,
            ..
        })
    ) {
        return Err("pre-admission Copy cancellation attempted target");
    }
    libkernel::debug!("FAL2 independent Copy conflict, empty and cancellation variants passed");

    // 具体写会话绑定原对象；旧名字替换后任务既不重选，也不回删替代者。
    start(
        root,
        [task(
            &ns,
            SOURCE,
            "/secondary/init-copy-replaced",
            Kind::Stream,
        )?],
    )?;
    root.await_copy_progress(0, 1)?;
    client
        .rename(
            "/secondary/init-copy-replaced",
            "/secondary/init-copy-pinned",
            false,
            deadline()?,
        )
        .map_err(|_| "Copy target rename failed")?;
    create(
        &mut client,
        "/secondary/init-copy-replaced",
        NodeKind::Property,
        &encoded[..used],
    )?;
    if !matches!(finish(root)?[0].outcome, Some(Outcome::Complete { .. })) {
        return Err("Copy lost original target after rename");
    }
    contents(&mut client, "/secondary/init-copy-pinned", LENGTH)?;
    if client
        .get::<u64>("/secondary/init-copy-replaced", deadline()?)
        .map_err(|_| "Copy deleted replacement")?
        != 0x81927364
    {
        return Err("Copy modified replacement");
    }
    libkernel::debug!("FAL2 independent exclusive target Create and pinned cleanup passed");

    // sealed 活 waiter：暂停领取但保持真实 Task/txid，Driver 必须保留失败完成。
    start(
        root,
        [task(
            &ns,
            PROPERTY,
            "/secondary/init-copy-sealed",
            Kind::Property,
        )?],
    )?;
    root.set_copy_hold(0, true)?;
    root.await_copy_sent(0, false)?;
    root.seal_copy_transport()?;
    if !root
        .read_world()
        .rpc
        .as_ref()
        .expect("Copy transport remains owned")
        .has_completed()
    {
        return Err("sealed Dispatcher stole live waiter completion");
    }
    root.set_copy_hold(0, false)?;
    let sealed = finish(root)?;
    if !matches!(
        sealed[0].outcome,
        Some(Outcome::Failed {
            cause: Cause::Transport(CallCause::Shutdown),
            cleanup_pending: false,
            ..
        })
    ) {
        return Err("Copy did not receive sealed transport result");
    }
    libkernel::debug!("FS1 sealed Dispatcher live Copy waiter completion passed");

    // 暂停并不取消原业务期限；到期事件必须唤醒仍活任务并完成原清理。
    start(
        root,
        [task(
            &ns,
            PROPERTY,
            "/secondary/init-copy-expired",
            Kind::Property,
        )?],
    )?;
    root.set_copy_hold(0, true)?;
    root.await_copy_sent(0, false)?;
    let expired = finish(root)?;
    if !matches!(
        expired[0].outcome,
        Some(Outcome::Failed {
            creation: Creation::NotAttempted,
            cause: Cause::Transport(CallCause::Timeout),
            cleanup_pending: false,
            ..
        })
    ) {
        return Err("held Copy lost its business deadline wake");
    }
    libkernel::debug!("FS1 held Copy business deadline wake and cleanup passed");

    // 实际 Sent→取消窗口：尚未接收成功回复时保留 Unknown，不自动重放。
    for (kind, from, to) in [
        (
            Kind::Property,
            PROPERTY,
            "/secondary/init-copy-unknown-property",
        ),
        (Kind::Stream, SOURCE, "/secondary/init-copy-unknown-stream"),
    ] {
        start(
            root,
            [
                task(&ns, from, to, kind)?,
                task(
                    &ns,
                    PROPERTY,
                    "/secondary/init-copy-successor",
                    Kind::Property,
                )?,
            ],
        )?;
        root.set_copy_hold(1, true)?;
        root.await_copy_sent(0, true)?;
        // Sent 不等于提交：只观察一次，名字可见/尚不可见都是合法结果。
        let observed = match kind {
            Kind::Property => client.get_value(to, deadline()?).map(|_| ()),
            Kind::Stream => client.read_at(to, 0, 1, deadline()?).map(|_| ()),
        };
        let visible = match observed {
            Ok(()) => true,
            Err(ClientError::Status(Status::NotFound)) => false,
            Err(error) => {
                libkernel::debug!("Copy Sent observation failed: {:?}", error);
                return Err("Copy Sent observation failed");
            }
        };
        libkernel::debug!(
            "FS1 Copy Sent target observation: kind {:?}, visible {}",
            kind,
            visible
        );
        root.cancel_copy(0)?;
        // 后继 waiter 保持存活，原回复邮箱实际处理旧 txid 后才恢复其业务。
        root.await_copy_late_reply()?;
        root.set_copy_hold(1, false)?;
        root.await_read()?;
        if root.read_world().ignored_rpc_replies == 0 {
            return Err("Copy shared transport did not discard a late response");
        }
        let unknown = finish(root)?;
        if !matches!(
            unknown[0].outcome,
            Some(Outcome::Failed {
                creation: Creation::Unknown,
                cause: Cause::Cancelled,
                cleanup_pending: false,
                ..
            })
        ) {
            return Err("sent Copy creation was falsely classified");
        }
        // 后继 Copy 的请求/回复由原 Dispatcher 处理，不借另一 Caller 证明隔离。
        if !matches!(unknown[1].outcome, Some(Outcome::Complete { .. })) {
            return Err("Copy successor failed after late response");
        }
        if client
            .get::<u64>("/secondary/init-copy-successor", deadline()?)
            .map_err(|_| "Copy reply isolation failed")?
            != 0x81927364
        {
            return Err("Copy reply isolation value invalid");
        }
        // 未知目标留给 provider 的原 backing/停止 owner，不按名字删除它。
        remove(&mut client, "/secondary/init-copy-successor")?;
    }
    libkernel::debug!("FAL2 independent Create uncertainty and reply isolation passed");
    for path in [
        SOURCE,
        PROPERTY,
        "/primary/init-copy-empty",
        "/secondary/init-copy-property",
        "/secondary/init-copy-stream",
        "/secondary/init-copy-cancel",
        "/secondary/init-copy-concurrent",
        "/secondary/init-copy-empty",
        "/secondary/init-copy-replaced",
        "/secondary/init-copy-pinned",
    ] {
        remove(&mut client, path)?;
    }
    Ok(())
}

pub(crate) fn start_shutdown(
    root: &mut RootSupervisor,
    primary: &MailboxSender,
    secondary: &MailboxSender,
) -> Result<(), &'static str> {
    let ns = namespace(primary, secondary)?;
    let mut caller = Caller::new();
    source(
        &mut Client::new(&ns, &mut caller, limits()),
        "/primary/init-copy-shutdown-source",
    )?;
    start(
        root,
        [task(
            &ns,
            "/primary/init-copy-shutdown-source",
            "/secondary/init-copy-shutdown-target",
            Kind::Stream,
        )?],
    )?;
    root.await_copy_progress(0, 1)
}
pub(crate) fn finish_shutdown(
    root: &mut RootSupervisor,
    primary: &MailboxSender,
) -> Result<(), &'static str> {
    let outcome = finish(root)?;
    if !matches!(outcome[0].outcome,Some(Outcome::Failed {creation:Creation::Created,progress:Progress::Stream(p),cleanup_pending:false,..}) if p.transported>0 && p.transported<LENGTH as u64 && p.destination_confirmed<=p.transported)
    {
        libkernel::debug!("Copy provider shutdown result: {:?}", outcome[0].outcome);
        return Err("Copy provider shutdown progress invalid");
    }
    let ns = super::namespace(primary)?;
    let mut caller = Caller::new();
    remove(
        &mut Client::new(&ns, &mut caller, limits()),
        "/init-copy-shutdown-source",
    )?;
    libkernel::debug!("FAL2 in-flight Copy provider shutdown passed");
    Ok(())
}
