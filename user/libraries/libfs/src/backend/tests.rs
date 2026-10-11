use super::*;
use crate::{authority::GrantLease, value::Value};
use core::cell::Cell;
use libbudget::{Budget, Taxonomy};
use libexecution::wake::Wake;
struct TestWake;
impl Wake for TestWake {
    fn publish(&self) {}
}
struct Fixture {
    backend: MemoryBackend,
    access: Option<AccessSnapshot>,
    account: AccountView<FsResource>,
    execution: AccountView<libexecution::ExecutionResource>,
}
impl Fixture {
    fn new() -> Self {
        let mut limits = [0; FsResource::COUNT + libexecution::ExecutionResource::COUNT];
        limits[FsResource::Node.slot()] = 32;
        limits[FsResource::Binding.slot()] = 2;
        limits[FsResource::Bytes.slot()] = 256 * 1024;
        limits[FsResource::Grant.slot()] = 16;
        limits[FsResource::WaitSource.slot()] = 16;
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let slots: [_; FsResource::COUNT] = core::array::from_fn(|n| budget.slot(n).unwrap());
        let execution = account
            .view(&core::array::from_fn::<
                _,
                { libexecution::ExecutionResource::COUNT },
                _,
            >(|n| {
                budget.slot(FsResource::COUNT + n).unwrap()
            }))
            .unwrap();
        let account = account.view(&slots).unwrap();
        let backend = MemoryBackend::with_bindings(&account, 32, 2, 8, Rc::new(TestWake)).unwrap();
        let access = Some(Self::access(
            &account,
            &execution,
            backend.root().unwrap().clone(),
            FsRights::ALL,
        ));
        Self {
            backend,
            access,
            account,
            execution,
        }
    }
    fn access(
        account: &AccountView<FsResource>,
        execution: &AccountView<libexecution::ExecutionResource>,
        root: NodeRef,
        rights: FsRights,
    ) -> AccessSnapshot {
        // 复用原 backing host fixture 的本地授权构造；不伪造 kernel sender/收到能力。
        let counter = Arc::new(Counter::new(1));
        let slot = Counter::try_acquire(&counter).unwrap();
        AccessSnapshot {
            root,
            rights,
            output_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
            funding: crate::Funding::new(account, execution).unwrap(),
            lease: Rc::new(GrantLease {
                active: Cell::new(true),
                _slot: slot,
                _grant_charge: account.acquire(FsResource::Grant, 1).unwrap(),
                _observer_charge: account.acquire(FsResource::WaitSource, 1).unwrap(),
                _storage_charge: account
                    .acquire(FsResource::Bytes, core::mem::size_of::<GrantLease>())
                    .unwrap(),
            }),
        }
    }
    fn scope(&self) -> &AccessSnapshot {
        self.access.as_ref().unwrap()
    }
    fn management(&self, root: NodeRef) -> crate::management::ManagementSnapshot {
        // 纯 backing 授权 fixture：独立收费/lease，不构造 Kernel sender 或输入能力。
        let access = Self::access(&self.account, &self.execution, root, FsRights::ALL);
        crate::management::ManagementSnapshot {
            root: access.root,
            max_rights: access.rights,
            output_transport: access.output_transport,
            funding: access.funding,
            lease: access.lease,
        }
    }
    fn create(&mut self, parent: &NodeRef, name: &str, kind: NodeKind, initial: &[u8]) -> NodeRef {
        let prepared = self
            .backend
            .prepare_create(
                self.scope(),
                parent.clone(),
                name,
                kind,
                FsRights::ALL,
                initial,
            )
            .unwrap();
        self.backend
            .commit(prepared)
            .unwrap_or_else(|f| panic!("commit failed: {:?}", f.status))
            .unwrap()
    }
    fn binding_member(&mut self, parent: &NodeRef, name: &str) -> Binding {
        // 仅建立纯 backing 的名字/记录关系，不创建或伪造能力；生产 Bind 的
        // issuer/shared 签发准入由真实 provider 消费者验证。
        let binding = self.backend.reserve_binding(
            FsRights::ALL, self.scope().output_transport(), &self.account,
        ).unwrap();
        let slot = Counter::try_acquire(&self.backend.members).unwrap();
        let entry = self.backend.prepare_member(
            parent, name, MemberKind::Binding(binding.id()), Some(slot), &self.account,
        ).unwrap();
        self.backend.nodes.link(&binding.reference);
        self.backend.directory_mut(parent).members.insert_prepared(entry);
        self.backend.bump(parent);
        binding
    }
}
#[test]
fn late_unbind_cannot_remove_replacement_and_boundary_rechecks_original_id() {
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let management = f.management(root.clone());
    let original = f.binding_member(&root, "mount");
    let Resolved::Provider { binding: selected, parent, .. } =
        f.backend.resolve(f.scope(), "mount/value", true).unwrap() else {
            panic!("binding fixture did not resolve a provider");
        };
    let stale = f.backend.prepare_unbind(&management, root.clone(), "mount").unwrap();
    let current = f.backend.prepare_unbind(&management, root.clone(), "mount").unwrap();
    f.backend.commit(current).unwrap_or_else(|failed| panic!("unbind failed: {:?}", failed.status));
    let replacement = f.binding_member(&root, "mount");
    assert_ne!(original.id(), replacement.id());
    let failed = f.backend.commit(stale).err().unwrap();
    assert_eq!(failed.status, Status::Conflict);
    let Resolved::Provider { binding: live, .. } =
        f.backend.resolve(f.scope(), "mount/value", true).unwrap() else {
            panic!("replacement fixture did not resolve a provider");
        };
    assert_eq!(live.id(), replacement.id());
    assert!(matches!(
        f.backend.check_binding_boundary(f.scope(), &selected, &parent),
        Err(Status::Revoked),
    ));
    f.backend.check_binding_boundary(f.scope(), &replacement, &parent).unwrap();
    assert!(matches!(f.backend.management_parent(&management, "mount/nested"),
        Err(Status::CrossFilesystem)));
    // 同名替代仍 linked；旧 pin 最后释放前，实际 Binding Charge 不退款。
    assert_eq!(f.account.usage(FsResource::Binding).0, 2);
    drop(failed);
    drop(selected);
    drop(original);
    f.backend.maintenance_step(8).unwrap();
    assert_eq!(f.account.usage(FsResource::Binding).0, 1);
    drop(live);
    drop(replacement);
    drop(parent);
    drop(management);
    drop(root);
}
#[test]
fn length_commit_hides_tail_busy_trimming_refunds_and_sparse_growth_never_revives_bytes() {
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let stream = f.create(&root, "bytes", NodeKind::Stream, &[]);
    let bytes = [0x75; crate::data::BLOCK_BYTES];
    for block in 0..3 {
        let write = f.backend.prepare_write(f.scope(), stream.clone(),
            (block * crate::data::BLOCK_BYTES) as u64, &bytes).unwrap();
        f.backend.commit(write).unwrap_or_else(|failed| panic!("write failed: {:?}", failed.status));
    }
    let original_bytes = f.account.usage(FsResource::Bytes).0;
    let stale = f.backend.prepare_write(f.scope(), stream.clone(), 0, b"old").unwrap();
    let length = f.backend.prepare_length(f.scope(), stream.clone(), 3).unwrap();
    f.backend.commit(length).unwrap_or_else(|failed| panic!("length failed: {:?}", failed.status));
    assert_eq!(f.backend.stream_length(&stream).unwrap(), 3);
    assert!(f.backend.stream_is_trimming(&stream).unwrap());
    let mut short = [0; 16];
    assert_eq!(f.backend.read_at(f.scope(), &stream, 0, &mut short).unwrap(), 3);
    assert_eq!(&short[..3], &[0x75; 3]);
    assert!(matches!(f.backend.prepare_write(f.scope(), stream.clone(), 3, b"new"),
        Err(Status::Busy)));
    assert!(matches!(f.backend.prepare_length(f.scope(), stream.clone(), 4096),
        Err(Status::Busy)));
    let failed = f.backend.commit(stale).err().unwrap();
    assert_eq!(failed.status, Status::Conflict);
    drop(failed);
    assert_eq!(f.account.usage(FsResource::Bytes).0, original_bytes,
        "logical truncation prematurely refunded physical blocks");
    assert!(f.backend.take_data_change());
    // 只推进正式维护，原 SetLength 准备/caller 已不存在；每步工作仍受预算约束。
    while f.backend.stream_is_trimming(&stream).unwrap() {
        let progress = f.backend.maintenance_step(1).unwrap();
        assert!(progress.work_done <= 1);
    }
    assert!(f.backend.take_data_change(), "trimming completion did not wake blocked writers");
    let trimmed_bytes = f.account.usage(FsResource::Bytes).0;
    assert!(original_bytes - trimmed_bytes >= 2 * crate::data::BLOCK_BYTES);
    let expanded = (3 * crate::data::BLOCK_BYTES) as u64;
    let length = f.backend.prepare_length(f.scope(), stream.clone(), expanded).unwrap();
    f.backend.commit(length).unwrap_or_else(|failed| panic!("growth failed: {:?}", failed.status));
    assert_eq!(f.account.usage(FsResource::Bytes).0, trimmed_bytes, "sparse growth allocated holes");
    let mut output = [0xff; 3 * crate::data::BLOCK_BYTES];
    assert_eq!(f.backend.read_at(f.scope(), &stream, 0, &mut output).unwrap(), output.len());
    assert_eq!(&output[..3], &[0x75; 3]);
    assert!(output[3..].iter().all(|byte| *byte == 0));
    drop(stream);
    drop(root);
}
#[test]
fn management_parent_rejects_links_and_rechecks_live_root_scope() {
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let inside = f.create(&root, "inside", NodeKind::Directory, &[]);
    let outside = f.create(&root, "outside", NodeKind::Directory, &[]);
    let selected = f.create(&inside, "selected", NodeKind::Directory, &[]);
    let link = f.create(&inside, "link", NodeKind::SymbolicLink, b"selected");
    let management = f.management(inside.clone());
    let (parent, name) = f
        .backend
        .management_parent(&management, "selected/mount")
        .unwrap();
    assert_eq!(parent.id(), selected.id());
    assert_eq!(name, "mount");
    assert!(matches!(
        f.backend.management_parent(&management, "link/mount"),
        Err(Status::TypeMismatch)
    ));
    assert!(matches!(
        f.backend.management_parent(&management, ""),
        Err(Status::Invalid)
    ));
    let destination = Fixture::access(&f.account, &f.execution, outside.clone(), FsRights::ALL);
    let rename = f
        .backend
        .prepare_rename(
            f.scope(),
            inside.clone(),
            "selected",
            &destination,
            "moved",
            false,
        )
        .unwrap();
    f.backend
        .commit(rename)
        .unwrap_or_else(|failed| panic!("rename failed: {:?}", failed.status));
    assert_eq!(
        f.backend.check_management_scope(&management, &parent),
        Err(Status::Permission)
    );
    // 迟到准备固定原目录引用，不能通过同名新目录恢复作用域。
    let replacement = f.create(&inside, "selected", NodeKind::Directory, &[]);
    assert_ne!(replacement.id(), parent.id());
    assert_eq!(
        f.backend.check_management_scope(&management, &parent),
        Err(Status::Permission)
    );
    management.lease.active.set(false);
    assert_eq!(
        f.backend.check_management_scope(&management, &replacement),
        Err(Status::Revoked)
    );
    drop((
        root,
        inside,
        outside,
        selected,
        link,
        parent,
        replacement,
        management,
        destination,
    ));
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.access = None;
        self.backend.seal();
        while !self.backend.is_empty() {
            self.backend.retire_step(32).unwrap();
        }
        assert_eq!(self.account.usage(FsResource::Node).0, 0);
        assert_eq!(self.account.usage(FsResource::Binding).0, 0);
    }
}

/// 准备记录还没有能力或目录链接，仍占真实 Binding/Bytes；最后 pin 消散才退款。
#[test]
fn invisible_binding_preparation_has_independent_capacity_and_last_pin_refund() {
    let mut f = Fixture::new();
    let bytes = f.account.usage(FsResource::Bytes).0;
    let nodes = f.account.usage(FsResource::Node).0;
    let first = f
        .backend
        .reserve_binding(
            FsRights::ALL,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            &f.account,
        )
        .unwrap();
    let second = f
        .backend
        .reserve_binding(
            FsRights::ALL,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            &f.account,
        )
        .unwrap();
    assert_ne!(first.id(), second.id());
    assert!(!f.backend.binding_active(&first));
    assert_eq!(f.account.usage(FsResource::Binding).0, 2);
    assert_eq!(f.account.usage(FsResource::Node).0, nodes);
    assert!(f.account.usage(FsResource::Bytes).0 > bytes);
    assert!(matches!(
        f.backend
            .reserve_binding(FsRights::ALL, Rights::WRITE, &f.account),
        Err(Status::Quota)
    ));
    let retained = first.clone();
    drop(first);
    assert_eq!(f.backend.retire_step(1).unwrap().work_done, 0);
    assert_eq!(f.account.usage(FsResource::Binding).0, 2);
    drop(retained);
    f.backend.retire_step(1).unwrap();
    assert_eq!(f.account.usage(FsResource::Binding).0, 1);
    drop(second);
    f.backend.retire_step(1).unwrap();
    assert_eq!(f.account.usage(FsResource::Binding).0, 0);
    assert_eq!(f.account.usage(FsResource::Node).0, nodes);
    assert_eq!(f.account.usage(FsResource::Bytes).0, bytes);
}

#[test]
fn retirement_waits_for_live_references_and_keeps_ready_until_drain() {
    // 无退休项时存活根不能令维护任务持续 Runnable；封口且仍有引用时应等释放通知。
    let mut f = Fixture::new();
    assert_eq!(f.backend.retirement_state(), RetirementState::Waiting);
    let progress = f.backend.retire_step(1).unwrap();
    assert_eq!(progress.work_done, 0);
    assert!(!progress.done);
    assert_eq!(f.backend.retirement_state(), RetirementState::Waiting);
    f.backend.seal();
    assert_eq!(f.backend.retirement_state(), RetirementState::Waiting);
    f.access = None;
    assert_eq!(f.backend.retirement_state(), RetirementState::Ready);
    while f.backend.has_retire_work() {
        f.backend.retire_step(1).unwrap();
    }
    assert_eq!(f.backend.retirement_state(), RetirementState::Empty);
    assert!(f.backend.is_empty());
}

#[test]
fn oal_projection_reads_the_same_backing_as_property_and_stream_operations() {
    // 无投影树：Set 改值形状后 Lookup 的标记自然变化，Stream ByteLength 不等于 Property 编码大小。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let mut value = [0; 128];
    let n = Value::Bool(true).encode(&mut value).unwrap();
    let node = f.create(&root, "p", NodeKind::Property, &value[..n]);
    let mut out = [0; 128];
    let Discovery::Found(info) = f.backend.lookup(f.scope(), "p", &mut out).unwrap() else {
        panic!("expected local info")
    };
    assert_eq!(info.as_bytes().len(), 68);
    assert!(info.contains(markers::BOOLEAN));
    assert_eq!(info.tags().count(), 0);
    let n = Value::Signed64Array(&[1, 2]).encode(&mut value).unwrap();
    let set = f
        .backend
        .prepare_set(f.scope(), node.clone(), &value[..n])
        .unwrap();
    drop(
        f.backend
            .commit(set)
            .unwrap_or_else(|f| panic!("set failed: {:?}", f.status)),
    );
    let Discovery::Found(info) = f.backend.lookup(f.scope(), "p", &mut out).unwrap() else {
        panic!("expected info")
    };
    assert_eq!(info.as_bytes().len(), 88);
    assert!(info.contains(markers::INTEGER));
    assert!(info.contains(markers::ARRAY));
    assert!(!info.contains(markers::BOOLEAN));
    assert_eq!(f.backend.property(f.scope(), &node).unwrap().count(), 2);
    let stream = f.create(&root, "s", NodeKind::Stream, &[]);
    let write = f
        .backend
        .prepare_write(f.scope(), stream.clone(), 3, b"abc")
        .unwrap();
    drop(
        f.backend
            .commit(write)
            .unwrap_or_else(|f| panic!("write failed: {:?}", f.status)),
    );
    let Discovery::Found(info) = f.backend.lookup(f.scope(), "s", &mut out).unwrap() else {
        panic!("expected info")
    };
    assert_eq!(info.as_bytes().len(), 80);
    assert_eq!(info.tags().next().unwrap().value, &6u64.to_le_bytes());
    let mut bytes = [0; 6];
    assert_eq!(
        f.backend
            .read_at(f.scope(), &stream, 0, &mut bytes)
            .unwrap(),
        6
    );
    assert_eq!(&bytes, b"\0\0\0abc");
}
#[test]
fn one_name_admission_rejects_unenumerable_create_and_rename_before_mutation() {
    // D-F2: 2000字节 Stream 名使非终页超限，拒绝时不能改旧名字或创建半节点。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let long = "x".repeat(2000);
    assert!(matches!(
        f.backend.prepare_create(
            f.scope(),
            root.clone(),
            &long,
            NodeKind::Stream,
            FsRights::ALL,
            &[]
        ),
        Err(Status::EntryTooLarge)
    ));
    let node = f.create(&root, "old", NodeKind::Stream, &[]);
    assert!(matches!(
        f.backend
            .prepare_rename(f.scope(), root.clone(), "old", f.scope(), &long, false),
        Err(Status::EntryTooLarge)
    ));
    assert!(
        matches!(f.backend.resolve(f.scope(),"old",false),Ok(Resolved::Local(v))if v.id()==node.id())
    );
    assert!(matches!(
        f.backend.resolve(f.scope(), &long, false),
        Err(Status::NotFound)
    ));
    for kind in [
        NodeKind::Directory,
        NodeKind::Property,
        NodeKind::Stream,
        NodeKind::SymbolicLink,
    ] {
        let max = (BODY_MAX - 44 - kind.max_info_len()) / 2;
        assert!(admit_member(&"n".repeat(max), kind.max_info_len()).is_ok());
        assert_eq!(
            admit_member(&"n".repeat(max + 1), kind.max_info_len()),
            Err(Status::EntryTooLarge)
        );
    }
}
#[test]
fn name_pages_are_ordered_and_continuations_do_not_repeat_long_original_path() {
    // D-F1: 长名字第一页合法且空 path cursor 续页可编码；静态扫描完整无重复。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let dir = f.create(&root, "dir", NodeKind::Directory, &[]);
    let a = f.create(&dir, "a", NodeKind::Directory, &[]);
    let z = f.create(&dir, "雪", NodeKind::Directory, &[]);
    let b = f.create(&dir, "é", NodeKind::Directory, &[]);
    let mut out = [0; BODY_MAX];
    let Enumeration::Page(page) = f
        .backend
        .enumerate(f.scope(), 7, "dir", &[], 1, &mut out)
        .unwrap()
    else {
        panic!("expected page")
    };
    assert_eq!(page.entries().next().unwrap().name, "a");
    let cursor = page.next_cursor().to_vec();
    assert!(!page.done());
    let Enumeration::Page(page) = f
        .backend
        .enumerate(f.scope(), 7, "", &cursor, 8, &mut out)
        .unwrap()
    else {
        panic!("expected page")
    };
    let names = page
        .entries()
        .map(|e| e.name)
        .collect::<alloc::vec::Vec<_>>();
    assert_eq!(names, ["é", "雪"]);
    assert!(page.done());
    drop((a, z, b));
}
#[test]
fn cursor_rechecks_live_directory_scope_after_rename_and_cannot_expand_authority() {
    // 游标 serial 只选位置：目录在范围内改名仍可读，移出授权根必须 CursorInvalid。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let allowed = f.create(&root, "allowed", NodeKind::Directory, &[]);
    let outside = f.create(&root, "outside", NodeKind::Directory, &[]);
    let dir = f.create(&allowed, "dir", NodeKind::Directory, &[]);
    let a = f.create(&dir, "a", NodeKind::Directory, &[]);
    let b = f.create(&dir, "b", NodeKind::Directory, &[]);
    let scope = Fixture::access(&f.account, &f.execution, allowed.clone(), FsRights::ALL);
    let mut out = [0; BODY_MAX];
    let Enumeration::Page(page) = f
        .backend
        .enumerate(&scope, 7, "dir", &[], 1, &mut out)
        .unwrap()
    else {
        panic!("expected page")
    };
    let cursor = page.next_cursor().to_vec();
    let rename = f
        .backend
        .prepare_rename(f.scope(), allowed.clone(), "dir", &scope, "renamed", false)
        .unwrap();
    drop(
        f.backend
            .commit(rename)
            .unwrap_or_else(|f| panic!("rename failed: {:?}", f.status)),
    );
    assert!(matches!(
        f.backend.enumerate(&scope, 7, "", &cursor, 1, &mut out),
        Ok(Enumeration::Page(_))
    ));
    let rename = f
        .backend
        .prepare_rename(
            f.scope(),
            allowed.clone(),
            "renamed",
            &Fixture::access(&f.account, &f.execution, outside.clone(), FsRights::ALL),
            "dir",
            false,
        )
        .unwrap();
    drop(
        f.backend
            .commit(rename)
            .unwrap_or_else(|f| panic!("move failed: {:?}", f.status)),
    );
    assert!(matches!(
        f.backend.enumerate(&scope, 7, "", &cursor, 1, &mut out),
        Err(Status::CursorInvalid)
    ));
    assert!(matches!(
        f.backend.enumerate(f.scope(), 8, "", &cursor, 1, &mut out),
        Err(Status::CursorInvalid)
    ));
    drop((scope, a, b, dir, allowed, outside));
}
#[test]
fn distinct_creates_prepared_in_one_directory_commit_independently() {
    // 独占创建保护各自名字；另一个兄弟项提交不能废弃已付费的准备。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let prepare = |name| {
        f.backend.prepare_create(f.scope(), root.clone(), name, NodeKind::Stream, FsRights::ALL, &[]).unwrap()
    };
    let first = prepare("first");
    let second = prepare("second");
    let first_id = first.created_target().unwrap().id();
    let second_id = second.created_target().unwrap().id();
    assert_ne!(first_id, second_id);
    for pending in [first, second] {
        f.backend.commit(pending).unwrap_or_else(|failed| panic!("independent create rejected: {:?}", failed.status));
    }
    for (name, id) in [("first", first_id), ("second", second_id)] {
        let Resolved::Local(actual) = f.backend.resolve(f.scope(), name, true).unwrap() else {
            panic!("created local file became boundary");
        };
        assert_eq!(actual.id(), id);
    }
}

#[test]
fn create_rechecks_parent_scope_after_directory_move() {
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let allowed = f.create(&root, "allowed", NodeKind::Directory, &[]);
    let outside = f.create(&root, "outside", NodeKind::Directory, &[]);
    let selected = f.create(&allowed, "selected", NodeKind::Directory, &[]);
    let scope = Fixture::access(&f.account, &f.execution, allowed.clone(), FsRights::ALL);
    let destination = Fixture::access(&f.account, &f.execution, outside, FsRights::ALL);
    let pending = f.backend.prepare_create(
        &scope, selected, "late", NodeKind::Stream, FsRights::ALL, &[],
    ).unwrap();
    let movement = f.backend.prepare_rename(
        f.scope(), allowed, "selected", &destination, "moved", false,
    ).unwrap();
    f.backend.commit(movement).unwrap_or_else(|failed| panic!("move failed: {:?}", failed.status));
    let failure = f.backend.commit(pending).err().unwrap();
    assert_eq!(failure.status, Status::Permission);
    assert!(matches!(
        f.backend.resolve(f.scope(), "outside/moved/late", true),
        Err(Status::NotFound),
    ));
}

#[test]
fn independent_create_rechecks_version_exhaustion_before_commit() {
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    f.backend.node_mut(&root).unwrap().version = u64::MAX - 1;
    let first = f.backend.prepare_create(
        f.scope(), root.clone(), "first", NodeKind::Stream, FsRights::ALL, &[],
    ).unwrap();
    let second = f.backend.prepare_create(
        f.scope(), root, "second", NodeKind::Stream, FsRights::ALL, &[],
    ).unwrap();
    f.backend.commit(first).unwrap_or_else(|failed| panic!("last version create failed: {:?}", failed.status));
    let failure = f.backend.commit(second).err().unwrap();
    assert_eq!(failure.status, Status::Resource);
    drop(failure);
    assert_eq!(f.account.usage(FsResource::Node).0, 2);
    assert!(matches!(f.backend.resolve(f.scope(), "second", true), Err(Status::NotFound)));
}

#[test]
fn prepared_requests_own_capacity_and_recheck_revoke_and_name_conflicts() {
    // 两个同时准备各持自己的槽/节点；一个提交后另一个不能覆盖同一名字。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let first = f
        .backend
        .prepare_create(
            f.scope(),
            root.clone(),
            "same",
            NodeKind::Directory,
            FsRights::ALL,
            &[],
        )
        .unwrap();
    let second = f
        .backend
        .prepare_create(
            f.scope(),
            root.clone(),
            "same",
            NodeKind::Directory,
            FsRights::ALL,
            &[],
        )
        .unwrap();
    assert_eq!(f.account.usage(FsResource::Node).0, 3);
    drop(
        f.backend
            .commit(first)
            .unwrap_or_else(|f| panic!("create failed: {:?}", f.status)),
    );
    let failure = f.backend.commit(second).err().unwrap();
    assert_eq!(failure.status, Status::Conflict);
    drop(failure);
    assert_eq!(f.account.usage(FsResource::Node).0, 2);
    let pending = f
        .backend
        .prepare_create(
            f.scope(),
            root.clone(),
            "revoked",
            NodeKind::Directory,
            FsRights::ALL,
            &[],
        )
        .unwrap();
    f.scope().lease.active.set(false);
    let failure = f.backend.commit(pending).err().unwrap();
    assert_eq!(failure.status, Status::Revoked);
    drop(failure);
}
#[test]
fn directory_issue_rechecks_selected_scope_ceiling_and_complete_transport_limits() {
    // 已选择目录移出母 root 后不能提交；sender 与 output 上限独立，不以低位运输位代替。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let allowed = f.create(&root, "allowed", NodeKind::Directory, &[]);
    let outside = f.create(&root, "outside", NodeKind::Directory, &[]);
    let selected = f.create(&allowed, "selected", NodeKind::Directory, &[]);
    let parent = Fixture::access(&f.account, &f.execution, allowed.clone(), FsRights::ALL);
    let policy = crate::authority::Issuance {
        rights: FsRights::CREATE,
        sender_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        output_transport: Rights::NONE,
    };
    assert_eq!(
        f.backend.check_directory_issue(&parent, &selected, policy),
        Ok(())
    );
    for widened in [
        crate::authority::Issuance {
            sender_transport: policy.sender_transport | Rights::GRANT,
            ..policy
        },
        crate::authority::Issuance {
            output_transport: Rights::GRANT,
            ..policy
        },
    ] {
        assert_eq!(
            f.backend.check_directory_issue(&parent, &selected, widened),
            Err(Status::Permission)
        );
    }
    f.backend.node_mut(&selected).unwrap().ceiling = FsRights::CREATE;
    assert_eq!(
        f.backend.check_directory_issue(&parent, &selected, policy),
        Err(Status::Permission)
    );
    f.backend.node_mut(&selected).unwrap().ceiling = FsRights::ALL;
    let rename = f
        .backend
        .prepare_rename(
            f.scope(),
            allowed.clone(),
            "selected",
            &Fixture::access(&f.account, &f.execution, outside.clone(), FsRights::ALL),
            "selected",
            false,
        )
        .unwrap();
    drop(
        f.backend
            .commit(rename)
            .unwrap_or_else(|failed| panic!("move failed: {:?}", failed.status)),
    );
    assert_eq!(
        f.backend.check_directory_issue(&parent, &selected, policy),
        Err(Status::Permission)
    );
    parent.lease.active.set(false);
    assert_eq!(
        f.backend.check_directory_issue(&parent, &selected, policy),
        Err(Status::Revoked)
    );
}
#[test]
fn admitted_directory_use_survives_ordinary_owner_drop_but_revoke_blocks_rename() {
    // 普通 owner 消散只释放引用；显式撤销活动位才否定已准入使用与未提交事务。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let selected = f.create(&root, "dir", NodeKind::Directory, &[]);
    let file = f.create(&root, "file", NodeKind::Stream, &[]);
    let admitted = f.scope().clone();
    f.access = None;
    let policy = crate::authority::Issuance {
        rights: FsRights::CREATE,
        sender_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        output_transport: Rights::NONE,
    };
    assert_eq!(
        f.backend
            .check_directory_issue(&admitted, &selected, policy),
        Ok(())
    );
    let destination = Fixture::access(&f.account, &f.execution, selected.clone(), FsRights::ALL);
    destination.lease.active.set(false);
    assert!(matches!(
        f.backend
            .prepare_rename(&admitted, root.clone(), "file", &destination, "file", false),
        Err(Status::Revoked)
    ));
    admitted.lease.active.set(false);
    assert_eq!(
        f.backend
            .check_directory_issue(&admitted, &selected, policy),
        Err(Status::Revoked)
    );
    drop((destination, admitted, file, selected, root));
}
#[test]
fn rename_refuses_different_actual_backing_and_descendant_cycles() {
    // 同进程/同 mailbox 的可能组合不能替代真实 backing 判定；父关系提供防环依据。
    let mut f = Fixture::new();
    let other = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let node = f.create(&root, "a", NodeKind::Directory, &[]);
    let child = f.create(&node, "child", NodeKind::Directory, &[]);
    assert!(matches!(
        f.backend
            .prepare_rename(f.scope(), root.clone(), "a", other.scope(), "b", false),
        Err(Status::CrossFilesystem)
    ));
    assert!(matches!(
        f.backend.prepare_rename(
            f.scope(),
            root.clone(),
            "a",
            &Fixture::access(&f.account, &f.execution, child.clone(), FsRights::ALL),
            "a",
            false
        ),
        Err(Status::Invalid)
    ));
}

#[test]
fn df1_long_original_path_and_cursor_fit_the_independent_wire_bounds() {
    // 计划 D-F1 原反例：path=2200，首名字=1825；续页只发空 path。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let outer_name = "o".repeat(1600);
    let inner_name = "i".repeat(599);
    let outer = f.create(&root, &outer_name, NodeKind::Directory, &[]);
    let inner = f.create(&outer, &inner_name, NodeKind::Directory, &[]);
    let path = alloc::format!("{outer_name}/{inner_name}");
    assert_eq!(path.len(), 2200);
    let long = "a".repeat(1825);
    let first = f.create(&inner, &long, NodeKind::Directory, &[]);
    let last = f.create(&inner, "z", NodeKind::Directory, &[]);
    let mut out = [0; BODY_MAX];
    let Enumeration::Page(page) = f
        .backend
        .enumerate(f.scope(), 7, &path, &[], 1, &mut out)
        .unwrap()
    else {
        panic!("expected page")
    };
    assert!(!page.done());
    assert_eq!(4 + page.as_bytes().len(), 3722);
    let cursor = page.next_cursor().to_vec();
    assert_eq!(cursor.len(), 1845);
    let next = liboal::protocol::Request::Enumerate {
        path: "",
        cursor: &cursor,
        max_entries: 1,
    };
    let mut frame = [0; BODY_MAX + liboal::protocol::HEADER_LEN];
    let encoded = liboal::protocol::encode_request(
        &next,
        erhino_shared::time::Deadline::INFINITE,
        &mut frame,
    )
    .unwrap();
    assert_eq!(encoded - liboal::protocol::HEADER_LEN, 1857);
    let Enumeration::Page(page) = f
        .backend
        .enumerate(f.scope(), 7, "", &cursor, 1, &mut out)
        .unwrap()
    else {
        panic!("expected continuation")
    };
    assert_eq!(page.entries().next().unwrap().name, "z");
    assert!(page.done());
    drop((first, last, inner, outer));
}
#[test]
fn derived_sender_and_new_output_each_obey_full_transport_ceiling() {
    // 两个完整运输字段各自受母上限限制；缺必需业务位不能由隐式附加修复。
    use crate::authority::Issuance;
    let f = Fixture::new();
    let mut parent = f.scope().clone();
    let callable = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    parent.output_transport = callable | Rights::DUPLICATE;
    let legal = Issuance {
        rights: FsRights::READ_STREAM,
        sender_transport: callable,
        output_transport: callable,
    };
    assert_eq!(legal.validate_derived(&parent, FsRights::ALL), Ok(()));
    assert_eq!(
        Issuance {
            sender_transport: callable | Rights::GRANT,
            ..legal
        }
        .validate_derived(&parent, FsRights::ALL),
        Err(Status::Permission)
    );
    assert_eq!(
        Issuance {
            output_transport: callable | Rights::READ,
            ..legal
        }
        .validate_derived(&parent, FsRights::ALL),
        Err(Status::Permission)
    );
    assert_eq!(
        Issuance {
            sender_transport: Rights::TRANSIT,
            ..legal
        }
        .validate_policy(),
        Err(Status::Permission)
    );
    assert_eq!(
        Issuance {
            output_transport: Rights::from_raw(1u64 << 32),
            ..legal
        }
        .validate_policy(),
        Err(Status::Invalid)
    );
    assert_eq!(
        legal.validate_derived(&parent, FsRights::READ_STREAM),
        Err(Status::Permission)
    );
    parent.lease.active.set(false);
    assert_eq!(
        legal.validate_derived(&parent, FsRights::ALL),
        Err(Status::Revoked)
    );
    drop(parent);
}

#[test]
fn prepared_rename_rechecks_ancestor_moves_before_committing_directory_edges() {
    // 防环条件属于提交不变量：未变化的源/目的成员版本不能替代祖先链复核。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let x = f.create(&root, "X", NodeKind::Directory, &[]);
    let a = f.create(&x, "A", NodeKind::Directory, &[]);
    let y = f.create(&root, "Y", NodeKind::Directory, &[]);
    let z = f.create(&y, "Z", NodeKind::Directory, &[]);
    let d = f.create(&z, "D", NodeKind::Directory, &[]);
    let to_d = Fixture::access(&f.account, &f.execution, d.clone(), FsRights::ALL);
    let to_a = Fixture::access(&f.account, &f.execution, a.clone(), FsRights::ALL);
    let first = f
        .backend
        .prepare_rename(f.scope(), x.clone(), "A", &to_d, "A", false)
        .unwrap();
    let second = f
        .backend
        .prepare_rename(f.scope(), y.clone(), "Z", &to_a, "Z", false)
        .unwrap();
    drop(
        f.backend
            .commit(second)
            .unwrap_or_else(|f| panic!("ancestor move failed: {:?}", f.status)),
    );
    let failure = f.backend.commit(first).err().unwrap();
    assert_eq!(failure.status, Status::Invalid);
    drop(failure);
    assert!(
        matches!(f.backend.resolve(f.scope(),"X/A/Z/D",false),Ok(Resolved::Local(node))if node.id()==d.id())
    );
    drop((to_d, to_a, d, z, y, a, x));
}

#[test]
fn remove_and_replacement_recheck_directory_contents_changed_during_preparation() {
    // 给目标目录增项不会改其父版本，但非空目录仍不可删除或被替换。
    let mut f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let d = f.create(&root, "D", NodeKind::Directory, &[]);
    let s = f.create(&root, "S", NodeKind::Directory, &[]);
    let remove = f
        .backend
        .prepare_remove(f.scope(), root.clone(), "D")
        .unwrap();
    let replace = f
        .backend
        .prepare_rename(f.scope(), root.clone(), "S", f.scope(), "D", true)
        .unwrap();
    let child = f.create(&d, "child", NodeKind::Directory, &[]);
    for prepared in [remove, replace] {
        let failure = f.backend.commit(prepared).err().unwrap();
        assert_eq!(failure.status, Status::NotEmpty);
        drop(failure);
    }
    assert!(
        matches!(f.backend.resolve(f.scope(),"D/child",false),Ok(Resolved::Local(node))if node.id()==child.id())
    );
    assert!(
        matches!(f.backend.resolve(f.scope(),"S",false),Ok(Resolved::Local(node))if node.id()==s.id())
    );
}

#[test]
fn symlink_body_is_charged_while_prepared_and_refunded_when_abandoned() {
    // 长正文是实际常驻字节，不能仅收费短名字和固定节点。
    let f = Fixture::new();
    let root = f.backend.root().unwrap().clone();
    let before = f.account.usage(FsResource::Bytes).0;
    let target = "t".repeat(2048);
    let prepared = f
        .backend
        .prepare_create(
            f.scope(),
            root,
            "link",
            NodeKind::SymbolicLink,
            FsRights::ALL,
            target.as_bytes(),
        )
        .unwrap();
    assert!(f.account.usage(FsResource::Bytes).0 >= before + target.len());
    drop(prepared);
    assert_eq!(f.account.usage(FsResource::Bytes).0, before);
}
