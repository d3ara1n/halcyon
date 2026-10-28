//! 正式独立消费者的文件订阅：旧序号确认、直接目录事件与弱目标终态。
use super::{deadline, limits};
use libfs::{
    FsRights,
    client::{Client, ClientError, WatchControl},
    node::NodeKind,
    protocol::{self, Status, WatchInfo, WatchReason, WatchState},
    value::Value,
};
use libkernel::shared::object::ObjectSignals;
use liboal::Namespace;
use librpc::Caller;

fn query(control: &WatchControl, caller: &mut Caller) -> WatchInfo {
    control
        .query(caller, deadline())
        .expect("file watch query failed")
}
fn ack(control: &WatchControl, caller: &mut Caller, sequence: u64) -> WatchInfo {
    control
        .ack(caller, sequence, deadline())
        .expect("file watch ack failed")
}
fn unready(control: &WatchControl) {
    assert_eq!(
        libkernel::ipc::wait::probe(control.waiter.as_handle(), ObjectSignals::READABLE)
            .expect("file watch readiness query failed"),
        ObjectSignals::NONE
    );
}
pub(super) fn check(namespace: &Namespace, caller: &mut Caller, prefix: &str) {
    let directory = alloc::format!("{prefix}/consumer-watch");
    let property = alloc::format!("{directory}/value");
    let denied = alloc::format!("{directory}/denied");
    Client::new(namespace, caller, limits())
        .create(
            &directory,
            NodeKind::Directory,
            FsRights::ALL,
            &[],
            deadline(),
        )
        .expect("watch directory create failed");
    let dir_watch = Client::new(namespace, caller, limits())
        .watch(&directory, protocol::WATCH_BUSINESS, deadline())
        .expect("directory watch creation failed");
    let initial = query(&dir_watch.control, caller);
    assert_eq!(initial.pending, protocol::WATCH_INITIAL);
    assert_eq!(ack(&dir_watch.control, caller, initial.sequence).pending, 0);
    assert_eq!(ack(&dir_watch.control, caller, initial.sequence).pending, 0);
    unready(&dir_watch.control);
    assert!(matches!(
        dir_watch
            .control
            .ack(caller, initial.sequence + 1, deadline()),
        Err(ClientError::Status(Status::Invalid))
    ));
    assert!(matches!(
        Client::new(namespace, caller, limits()).watch(
            &directory,
            protocol::WATCH_INITIAL,
            deadline()
        ),
        Err(ClientError::Status(Status::Invalid))
    ));

    let mut bytes = [0; 20];
    let used = Value::Unsigned64(1)
        .encode(&mut bytes)
        .expect("watch property fixture invalid");
    Client::new(namespace, caller, limits())
        .create(
            &property,
            NodeKind::Property,
            FsRights::ALL,
            &bytes[..used],
            deadline(),
        )
        .expect("watch property create failed");
    let created = query(&dir_watch.control, caller);
    assert_ne!(created.pending & protocol::WATCH_CREATE, 0);
    ack(&dir_watch.control, caller, created.sequence);
    Client::new(namespace, caller, limits())
        .create(
            &denied,
            NodeKind::Property,
            FsRights::READ_PROPERTY,
            &bytes[..used],
            deadline(),
        )
        .expect("watch restricted property create failed");
    assert!(matches!(
        Client::new(namespace, caller, limits()).watch(
            &denied,
            protocol::WATCH_BUSINESS,
            deadline()
        ),
        Err(ClientError::Status(Status::Permission))
    ));
    let directory_before = query(&dir_watch.control, caller);
    ack(&dir_watch.control, caller, directory_before.sequence);

    let file_watch = Client::new(namespace, caller, limits())
        .watch(
            &property,
            protocol::WATCH_MODIFY | protocol::WATCH_UNBOUND,
            deadline(),
        )
        .expect("property watch creation failed");
    let roles = libkernel::ipc::object::query(file_watch.control.waiter.as_handle())
        .expect("file observer role query failed");
    assert_eq!(
        roles.kind,
        libkernel::shared::object::ObjectKind::NotificationObserver as u32
    );
    assert!(roles.rights.contains(
        libkernel::shared::object::Rights::WAIT | libkernel::shared::object::Rights::TRANSIT
    ));
    assert_eq!(
        roles.rights
            & (libkernel::shared::object::Rights::READ
                | libkernel::shared::object::Rights::SIGNAL
                | libkernel::shared::object::Rights::MANAGE
                | libkernel::shared::object::Rights::GRANT),
        libkernel::shared::object::Rights::NONE
    );
    let before = query(&file_watch.control, caller);
    ack(&file_watch.control, caller, before.sequence);
    Client::new(namespace, caller, limits())
        .set(&property, Value::Unsigned64(2), deadline())
        .expect("watched property set failed");
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>(&property, deadline())
            .expect("watched property recheck failed"),
        2
    );
    let stale = ack(&file_watch.control, caller, before.sequence);
    assert!(stale.sequence > before.sequence);
    assert_ne!(stale.pending & protocol::WATCH_MODIFY, 0);
    let directory_after = query(&dir_watch.control, caller);
    assert_eq!(directory_after.sequence, directory_before.sequence);
    assert_eq!(
        directory_after.pending, 0,
        "directory watch propagated child content"
    );
    let observed = query(&file_watch.control, caller);
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>(&property, deadline())
            .expect("watched state reread failed"),
        2
    );
    assert_eq!(
        ack(&file_watch.control, caller, observed.sequence).pending,
        0
    );
    assert_eq!(
        ack(&file_watch.control, caller, observed.sequence).pending,
        0
    );
    unready(&file_watch.control);

    Client::new(namespace, caller, limits())
        .remove(&property, deadline())
        .expect("watched property unlink failed");
    let until = deadline();
    let mut saw_unbound = false;
    let terminal = loop {
        let snapshot = file_watch
            .control
            .query(caller, until)
            .expect("retiring file watch query failed");
        saw_unbound |= snapshot.pending & protocol::WATCH_UNBOUND != 0;
        assert!(matches!(
            Client::new(namespace, caller, limits()).get_value(&property, until),
            Err(ClientError::Status(Status::NotFound))
        ));
        if snapshot.state == WatchState::Terminal {
            break snapshot;
        }
        file_watch
            .control
            .ack(caller, snapshot.sequence, until)
            .expect("retiring file watch ack failed");
        let ready = libkernel::ipc::wait::wait_until(
            file_watch.control.waiter.as_handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            until,
        )
        .expect("file watch terminal wait failed");
        assert!(
            ready.observed.intersects(ObjectSignals::READABLE),
            "file watch retirement lost notification"
        );
    };
    assert_eq!(terminal.reason, WatchReason::TargetRetired);
    assert!(saw_unbound, "file watch lost unbound notification");
    assert_ne!(terminal.pending & protocol::WATCH_TERMINAL, 0);
    assert_eq!(
        ack(&file_watch.control, caller, terminal.sequence).pending,
        0
    );
    assert_eq!(
        file_watch
            .control
            .cancel(caller, deadline())
            .expect("terminal file watch cancel failed")
            .reason,
        WatchReason::TargetRetired
    );
    drop(file_watch);

    let zero = Client::new(namespace, caller, limits())
        .watch(&directory, 0, deadline())
        .expect("zero-mask watch creation failed");
    assert_ne!(zero.info.pending & protocol::WATCH_INITIAL, 0);
    let cancelled = zero
        .control
        .cancel(caller, deadline())
        .expect("zero-mask watch cancel failed");
    assert_eq!(cancelled.state, WatchState::Terminal);
    assert_eq!(cancelled.reason, WatchReason::Cancelled);
    assert_ne!(cancelled.pending & protocol::WATCH_TERMINAL, 0);
    assert_eq!(
        zero.control
            .cancel(caller, deadline())
            .expect("repeated watch cancel failed"),
        cancelled
    );
    drop(zero);
    Client::new(namespace, caller, limits())
        .remove(&denied, deadline())
        .expect("watch restricted property removal failed");
    let removed = query(&dir_watch.control, caller);
    assert_ne!(removed.pending & protocol::WATCH_DELETE, 0);
    dir_watch
        .control
        .cancel(caller, deadline())
        .expect("directory watch cancel failed");
    drop(dir_watch);
    let mut held = alloc::vec::Vec::new();
    // 每项 Watch 是独立准入操作；总饱和不以固定壁钟窗口判定。
    let mut last_admitted_ns = 0;
    let attempts = srv_fs::deployment::WATCHES
        .checked_add(1)
        .expect("configured watch capacity cannot represent its saturation probe");
    loop {
        assert!(
            held.len() < attempts,
            "watch admission exceeded configured saturation bound: count={}",
            held.len()
        );
        let quota_until = deadline();
        held.try_reserve(1)
            .expect("watch control fixture allocation failed");
        match Client::new(namespace, caller, limits()).watch(&directory, 0, quota_until) {
            Ok(watch) => {
                held.push(watch);
                last_admitted_ns = libkernel::time::snapshot()
                    .expect("watch admission clock failed")
                    .now_ns;
            }
            Err(ClientError::Status(Status::Quota)) => break,
            Err(error) => panic!(
                "watch admission failed before quota: {:?}, count={}, last_ns={}, deadline={:?}",
                error,
                held.len(),
                last_admitted_ns,
                quota_until
            ),
        }
    }
    let released = held.pop().expect("watch quota admitted no controls");
    let libfs::client::WatchControl { sender, waiter } = released.control;
    drop(sender);
    // 只保留 Observer；它不能把最后控制已关闭的订阅保活。
    let closed =
        libkernel::ipc::wait::wait_until(waiter.as_handle(), ObjectSignals::CLOSED, deadline())
            .expect("watch control retirement wait failed");
    assert!(
        closed.observed.intersects(ObjectSignals::CLOSED),
        "observer kept subscription alive"
    );
    let recovered = Client::new(namespace, caller, limits())
        .watch(&directory, 0, deadline())
        .expect("closed watch control did not refund admission");
    recovered
        .control
        .cancel(caller, deadline())
        .expect("recovered watch cancel failed");
    drop(recovered);
    drop(waiter);
    for watch in held {
        watch
            .control
            .cancel(caller, deadline())
            .expect("held watch cancel failed");
    }
    Client::new(namespace, caller, limits())
        .remove(&directory, deadline())
        .expect("watch directory cleanup failed");
    libkernel::debug!("FS1 independent Watch contract passed: {}", prefix);
}

/// 两个真实发行入口共享全局容量，局部额度与退款分别沿自己的控制 owner。
pub(super) fn check_accounts(namespace: &Namespace, caller: &mut Caller) {
    let capacity = srv_fs::deployment::Capacity::new(srv_fs::startup::Profile::AcceptancePrimary)
        .expect("acceptance capacity invalid");
    use srv_fs::deployment::WATCHES;
    let mut a = alloc::vec::Vec::new();
    let mut b = alloc::vec::Vec::new();
    a.try_reserve_exact(capacity.entry_watches(0))
        .expect("tenant A watch storage failed");
    b.try_reserve_exact(WATCHES)
        .expect("tenant B watch storage failed");
    for _ in 0..capacity.entry_watches(0) {
        let watch = Client::new(namespace, caller, limits())
            .watch("/", 0, deadline())
            .expect("tenant A watch admission failed");
        let initial = query(&watch.control, caller);
        ack(&watch.control, caller, initial.sequence);
        a.push(watch);
    }
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).watch("/", 0, deadline()),
            Err(ClientError::Status(Status::Quota))
        ),
        "tenant A local watch quota missing"
    );
    for _ in capacity.entry_watches(0)..WATCHES {
        let watch = Client::new(namespace, caller, limits())
            .watch("/tenant-b", 0, deadline())
            .expect("tenant B rejected while global watch quota remained");
        let initial = query(&watch.control, caller);
        ack(&watch.control, caller, initial.sequence);
        b.push(watch);
    }
    assert!(
        b.len() < capacity.entry_watches(1),
        "tenant B fixture exhausted local quota"
    );
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).watch("/tenant-b", 0, deadline()),
            Err(ClientError::Status(Status::Quota))
        ),
        "global watch quota missing"
    );
    let released = a.pop().expect("tenant A control owner missing");
    let WatchControl { sender, waiter } = released.control;
    drop(sender);
    libkernel::ipc::wait::wait_until(waiter.as_handle(), ObjectSignals::CLOSED, deadline())
        .expect("tenant A control retirement failed");
    // CLOSED 是观察条件；实际准入成功才确认记录/permit 已退休退款。
    let until = deadline();
    let recovered = loop {
        match Client::new(namespace, caller, limits()).watch("/tenant-b", 0, until) {
            Ok(watch) => break watch,
            Err(ClientError::Status(Status::Quota)) => {
                assert!(
                    !libkernel::time::expired(until).expect("watch recovery clock failed"),
                    "tenant A retired control did not refund global quota"
                );
            }
            Err(error) => panic!("tenant B recovery failed: {error:?}"),
        }
    };
    b.push(recovered);
    drop(waiter);
    for watch in a.into_iter().chain(b) {
        let initial = query(&watch.control, caller);
        ack(&watch.control, caller, initial.sequence);
        watch
            .control
            .cancel(caller, deadline())
            .expect("tenant watch cancel failed");
        let WatchControl { sender, waiter } = watch.control;
        drop(sender);
        libkernel::ipc::wait::wait_until(waiter.as_handle(), ObjectSignals::CLOSED, deadline())
            .expect("tenant cancelled control retirement failed");
    }
    libkernel::debug!("FS1 tenant A local quota, B global quota and owner refund passed");
}
