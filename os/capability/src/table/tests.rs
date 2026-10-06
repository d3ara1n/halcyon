//! 表与持有计数的契约测试。每个用例对应 capability 分案中的一条目标契约。

use alloc::{rc::Rc, vec, vec::Vec};
use erhino_shared::object::{Handle, Rights};

use super::*;
use crate::{Holders, ZeroPolicy};

#[derive(Debug)]
struct TestCharge {
    live: Rc<core::cell::Cell<usize>>,
    bytes: usize,
}
impl Drop for TestCharge {
    fn drop(&mut self) {
        self.live.set(self.live.get() - self.bytes);
    }
}
struct TestFunding {
    live: Rc<core::cell::Cell<usize>>,
    limit: usize,
}
impl Funding for TestFunding {
    type Charge = TestCharge;
    type Error = ();
    fn charge(&self, layout: core::alloc::Layout) -> Result<TestCharge, ()> {
        let value = self.live.get().checked_add(layout.size()).ok_or(())?;
        if value > self.limit {
            return Err(());
        }
        self.live.set(value);
        Ok(TestCharge {
            live: self.live.clone(),
            bytes: layout.size(),
        })
    }
}
fn funding() -> TestFunding {
    TestFunding {
        live: Rc::new(core::cell::Cell::new(0)),
        limit: usize::MAX,
    }
}

/// 测试对象：Rc 只作存储引用，authority 计数在 Holders 中。
#[derive(Clone, Debug)]
struct Obj(Rc<Holders>);

impl Authority for Obj {
    fn holders(&self) -> &Holders {
        &self.0
    }
}

fn obj(policy: ZeroPolicy) -> Obj {
    Obj(Rc::new(Holders::new(policy)))
}

const RW: Rights = Rights::from_raw(Rights::READ.raw() | Rights::WRITE.raw());
const MOVE: Rights = Rights::GRANT;

fn all() -> Rights {
    RW | Rights::DUPLICATE | MOVE
}

fn install(table: &mut CapTable<Obj, TestCharge>, o: &Obj, rights: Rights) -> Handle {
    table
        .insert(Cap::mint(o.clone(), rights).unwrap(), &funding())
        .map_err(|(c, e)| {
            let _ = c.release();
            e
        })
        .unwrap()
}

fn close_release(table: &mut CapTable<Obj, TestCharge>, h: Handle) -> Option<LastCapability<Obj>> {
    table.close(h).unwrap().and_then(Cap::release)
}

// 最后持有：Terminal 归零恰好产生一次事件，并在同一原子操作中封闭铸造。
#[test]
fn terminal_last_release_emits_once_and_seals() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let a = install(&mut t, &o, all());
    let copy = t.get(a, Rights::NONE).unwrap().derive(RW).unwrap();
    let b = t.insert(copy, &funding()).map_err(|(_, e)| e).unwrap();
    assert!(close_release(&mut t, a).is_none());
    assert!(close_release(&mut t, b).is_some());
    assert!(o.0.is_sealed());
    let err = Cap::mint(o.clone(), RW).unwrap_err();
    assert_eq!(err, TableError::Sealed);
}

// Remintable 归零不发事件，可重新铸造（ProcessControl 语义）。
#[test]
fn remintable_zero_is_silent_and_reopens() {
    let o = obj(ZeroPolicy::Remintable);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let a = install(&mut t, &o, RW);
    assert!(close_release(&mut t, a).is_none());
    assert_eq!(o.0.count(), 0);
    let b = install(&mut t, &o, RW);
    assert!(close_release(&mut t, b).is_none());
}

// 移动（extract → 另一表 insert）不产生伪零窗口：计数始终不为 0。
#[test]
fn move_never_passes_through_zero() {
    let o = obj(ZeroPolicy::Terminal);
    let mut src = CapTable::try_new(&funding()).unwrap();
    let mut dst = CapTable::try_new(&funding()).unwrap();
    let h = install(&mut src, &o, all());
    let moved = src.extract(&[(h, RW)], MOVE, &funding()).unwrap();
    assert_eq!(o.0.count(), 1);
    let mut moved = moved.into_iter();
    let nh = dst
        .insert(moved.next().unwrap(), &funding())
        .map_err(|(_, e)| e)
        .unwrap();
    assert_eq!(dst.get(nh, Rights::NONE).unwrap().rights(), RW);
    assert_eq!(
        src.get(h, Rights::NONE).unwrap_err(),
        TableError::StaleHandle
    );
    assert!(close_release(&mut dst, nh).is_some());
}

// 批量移交任一项校验失败时表不变。
#[test]
fn failed_extract_keeps_every_source() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let ok = install(&mut t, &o, all());
    let no_move = install(&mut t, &o, RW);
    assert_eq!(
        t.extract(&[(ok, RW), (no_move, RW)], MOVE, &funding())
            .unwrap_err(),
        TableError::RightsDenied
    );
    assert_eq!(
        t.extract(&[(ok, RW), (ok, RW)], MOVE, &funding())
            .unwrap_err(),
        TableError::DuplicateHandle
    );
    assert!(t.get(ok, Rights::NONE).is_ok());
    assert!(t.get(no_move, Rights::NONE).is_ok());
    let _ = close_release(&mut t, ok);
    let _ = close_release(&mut t, no_move);
}

// 复制需 DUPLICATE 且不能放大 rights。
#[test]
fn derive_requires_duplicate_and_never_amplifies() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let plain = install(&mut t, &o, RW);
    let dup = install(&mut t, &o, Rights::READ | Rights::DUPLICATE);
    assert_eq!(
        t.get(plain, Rights::NONE)
            .unwrap()
            .derive(Rights::READ)
            .unwrap_err(),
        TableError::RightsDenied
    );
    assert_eq!(
        t.get(dup, Rights::NONE).unwrap().derive(RW).unwrap_err(),
        TableError::RightsDenied
    );
    let _ = close_release(&mut t, plain);
    let _ = close_release(&mut t, dup);
}

// 返还预约：无并发 Close 时失败恢复原 handle，成功时槽失效。
#[test]
fn claim_restore_and_commit() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let h = install(&mut t, &o, RW);
    let (cap, ticket) = t.claim(h, Rights::WRITE).unwrap();
    assert_eq!(t.get(h, Rights::NONE).unwrap_err(), TableError::Claimed);
    assert!(matches!(t.restore(ticket, cap), Restore::Restored));
    assert!(t.get(h, Rights::WRITE).is_ok());

    let (cap, ticket) = t.claim(h, Rights::WRITE).unwrap();
    t.commit_claim(ticket);
    assert_eq!(t.get(h, Rights::NONE).unwrap_err(), TableError::StaleHandle);
    assert!(t.is_empty());
    assert!(cap.release().is_some());
}

// 业务提交前的失败必须保留原权限；成功移动仅裁剪一次且不制造 authority 伪零。
#[test]
fn claimed_transfer_attenuates_only_at_commit() {
    let object = obj(ZeroPolicy::Terminal);
    let mut source = CapTable::try_new(&funding()).unwrap();
    let mut destination = CapTable::try_new(&funding()).unwrap();
    let handle = install(&mut source, &object, RW | Rights::TRANSIT);
    let (mut cap, ticket) = source.claim(handle, Rights::TRANSIT).unwrap();
    assert_eq!(cap.attenuate(Rights::MANAGE), Err(TableError::RightsDenied));
    assert!(matches!(source.restore(ticket, cap), Restore::Restored));
    assert_eq!(
        source.get(handle, Rights::WRITE).unwrap().rights(),
        RW | Rights::TRANSIT
    );

    let (mut cap, ticket) = source.claim(handle, Rights::TRANSIT).unwrap();
    cap.attenuate(Rights::READ).unwrap();
    source.commit_claim(ticket);
    let received = destination
        .insert(cap, &funding())
        .map_err(|(_, error)| error)
        .unwrap();
    assert_eq!(object.0.count(), 1);
    assert_eq!(
        destination.get(received, Rights::WRITE).unwrap_err(),
        TableError::RightsDenied
    );
    assert!(close_release(&mut destination, received).is_some());
}

// Close 撤销返还资格而不取消操作；失败回滚不复活已关闭 handle。
#[test]
fn close_revokes_claim_without_cancelling_operation() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let h = install(&mut t, &o, RW);
    let (cap, ticket) = t.claim(h, Rights::WRITE).unwrap();
    assert!(t.close(h).unwrap().is_none());
    assert_eq!(o.0.count(), 1, "operation still owns the authority");
    // 槽被复用后，旧预约仍不能写入新槽。
    let reused = install(&mut t, &o, RW);
    assert_eq!(reused.slot(), h.slot());
    match t.restore(ticket, cap) {
        Restore::Released(last) => assert!(last.is_none()),
        Restore::Restored => panic!("closed claim must not restore"),
    }
    assert!(t.get(reused, Rights::NONE).is_ok());
    assert!(close_release(&mut t, reused).is_some());
}

// 表关闭后返还入口关闭：预约即使未被 Close 也释放。
// shut 是准入关闭点；尚未清扫的 Occupied 槽也不能再接受查询或新 claim。
#[test]
fn shut_rejects_new_acceptance_before_drain() {
    let object = obj(ZeroPolicy::Terminal);
    let mut table = CapTable::try_new(&funding()).unwrap();
    let handle = install(&mut table, &object, RW);
    table.shut();
    assert_eq!(
        table.get(handle, Rights::READ).unwrap_err(),
        TableError::Closed
    );
    assert_eq!(
        table.claim(handle, Rights::WRITE).unwrap_err(),
        TableError::Closed
    );
    assert_eq!(object.0.count(), 1, "shut does not drop occupied owners");
    assert!(close_release(&mut table, handle).is_some());
}

#[test]
fn shut_table_releases_instead_of_restoring() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let h = install(&mut t, &o, RW);
    let (cap, ticket) = t.claim(h, Rights::WRITE).unwrap();
    t.shut();
    match t.restore(ticket, cap) {
        Restore::Released(last) => assert!(last.is_some()),
        Restore::Restored => panic!("shut table must not accept returns"),
    }
    let late = Cap::mint(o.clone(), RW);
    assert_eq!(late.unwrap_err(), TableError::Sealed);
}

// 安装失败返还 owner，不在内核中丢失 authority。
#[test]
fn failed_install_returns_owner() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_with_limit(1, &funding()).unwrap();
    let h = install(&mut t, &o, RW);
    let (cap, err) = t
        .insert(Cap::mint(o.clone(), RW).unwrap(), &funding())
        .unwrap_err();
    assert_eq!(err, TableError::ReachLimit);
    assert!(cap.release().is_none());
    let reservation = t.reserve(0, &funding()).unwrap();
    let (caps, err) = t
        .commit(reservation, vec![Cap::mint(o.clone(), RW).unwrap()])
        .unwrap_err();
    assert_eq!(err, TableError::BadReservation);
    for c in caps {
        assert!(c.release().is_none());
    }
    assert!(close_release(&mut t, h).is_some());
}

// 预留回滚后暂存 handle 数值永不复活。
#[test]
fn rollback_invalidates_reserved_values() {
    let mut t: CapTable<Obj, TestCharge> = CapTable::try_new(&funding()).unwrap();
    let r = t.reserve(2, &funding()).unwrap();
    let staged: Vec<Handle> = r.handles().to_vec();
    t.rollback(r);
    for h in staged {
        assert_eq!(t.get(h, Rights::NONE).unwrap_err(), TableError::StaleHandle);
    }
}

// 对象提交后表 shut 只撤销返还资格，返回恰好一次最后持有事件。
#[test]
fn shut_before_reserved_commit_returns_last_holder_events() {
    let terminal = obj(ZeroPolicy::Terminal);
    let observation = obj(ZeroPolicy::Remintable);
    let mut table = CapTable::try_new(&funding()).unwrap();
    let reservation = table.reserve(3, &funding()).unwrap();
    let staged = reservation.handles().to_vec();
    let caps = vec![
        Cap::mint(terminal.clone(), RW).unwrap(),
        Cap::mint(terminal.clone(), RW).unwrap(),
        Cap::mint(observation.clone(), RW).unwrap(),
    ];
    table.shut();
    match table.commit(reservation, caps).unwrap() {
        Commit::Released {
            handles,
            mut events,
        } => {
            assert_eq!(&*handles, staged.as_slice());
            assert_eq!(events.len(), 1);
            assert!(Rc::ptr_eq(&events.pop().unwrap().0.0, &terminal.0));
        }
        Commit::Installed(_) => panic!("shut table published a reservation"),
    }
    assert!(table.is_empty());
    assert_eq!(terminal.0.count(), 0);
    assert!(terminal.0.is_sealed());
    assert_eq!(observation.0.count(), 0);
    assert!(!observation.0.is_sealed());
    for handle in staged {
        assert_eq!(
            table.get(handle, Rights::NONE).unwrap_err(),
            TableError::Closed
        );
    }
}

// 回收已摘掉预留槽仍不能取消已提交对象；正常释放事件收束其协议。
#[test]
fn drained_reservation_still_releases_committed_object_after_shut() {
    let object = obj(ZeroPolicy::Terminal);
    let mut table = CapTable::try_new(&funding()).unwrap();
    let reservation = table.reserve(1, &funding()).unwrap();
    let cap = Cap::mint(object.clone(), RW).unwrap();
    table.shut();
    let mut cursor = 0;
    loop {
        match table.take_next_bounded(&mut cursor, 1) {
            TakeNext::Progress => {}
            TakeNext::Exhausted => break,
            TakeNext::Cap(cap) => {
                let _ = cap.release();
                panic!("reservation unexpectedly contained a published capability");
            }
        }
    }
    match table.commit(reservation, vec![cap]).unwrap() {
        Commit::Released { events, .. } => assert_eq!(events.len(), 1),
        Commit::Installed(_) => panic!("drained reservation was published"),
    }
    assert_eq!(object.0.count(), 0);
    assert!(object.0.is_sealed());
}

#[test]
fn open_reserved_commit_installs_without_changing_holder_counts() {
    let object = obj(ZeroPolicy::Terminal);
    let mut table = CapTable::try_new(&funding()).unwrap();
    let reservation = table.reserve(1, &funding()).unwrap();
    let handles = match table
        .commit(reservation, vec![Cap::mint(object.clone(), RW).unwrap()])
        .unwrap()
    {
        Commit::Installed(handles) => handles,
        Commit::Released { .. } => panic!("open table discarded committed capabilities"),
    };
    assert_eq!(object.0.count(), 1);
    assert_eq!(table.get(handles[0], Rights::NONE).unwrap().rights(), RW);
    assert!(close_release(&mut table, handles[0]).is_some());
}

// 计数有准入上限：到达上限时铸造报错，释放始终成功。
#[test]
fn holder_limit_rejects_mint_not_release() {
    let o = Obj(Rc::new(Holders::with_limit(ZeroPolicy::Terminal, 1)));
    let a = Cap::mint(o.clone(), RW).unwrap();
    assert_eq!(
        Cap::mint(o.clone(), RW).unwrap_err(),
        TableError::ReachLimit
    );
    assert!(a.release().is_some());
}

// 有界回收：每次至多扫描预算内槽位，并清理预约与预留。
#[test]
fn bounded_drain_respects_budget_and_clears_claims() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new(&funding()).unwrap();
    let a = install(&mut t, &o, RW);
    let b = install(&mut t, &o, RW);
    let (claimed, _ticket) = t.claim(b, Rights::WRITE).unwrap();
    let staged = t.reserve(1, &funding()).unwrap();
    t.shut();
    let mut cursor = 0;
    let mut released = Vec::new();
    loop {
        match t.take_next_bounded(&mut cursor, 1) {
            TakeNext::Cap(cap) => released.push(cap),
            TakeNext::Progress => {}
            TakeNext::Exhausted => break,
        }
    }
    assert_eq!(released.len(), 1);
    assert!(t.is_empty());
    assert_eq!(t.get(a, Rights::NONE).unwrap_err(), TableError::Closed);
    for h in staged.handles() {
        assert_eq!(t.get(*h, Rights::NONE).unwrap_err(), TableError::Closed);
    }
    let _ = staged;
    assert!(released.pop().unwrap().release().is_none());
    assert!(claimed.release().is_some());
}

#[test]
fn storage_failure_rolls_back_logical_slots_and_keeps_real_capacity_paid() {
    let slot = core::mem::size_of::<Slot<Obj>>();
    let scratch =
        2 * (core::mem::size_of::<Handle>() + core::mem::size_of::<LastCapability<Obj>>());
    let funds = TestFunding {
        live: Rc::new(core::cell::Cell::new(0)),
        limit: 3 * slot + scratch,
    };
    let mut table: CapTable<Obj, TestCharge> = CapTable::try_new(&funds).unwrap();
    assert_eq!(funds.live.get(), slot);
    assert_eq!(
        table.reserve(2, &funds).unwrap_err(),
        TableError::FundingDenied
    );
    assert!(table.is_empty());
    // 第一块增长真实完成；逻辑预约回滚不虚构 shrink，也不提前退仍保留的容量。
    assert_eq!(funds.live.get(), 2 * slot);
    assert!(
        table
            .slots
            .iter()
            .all(|slot| !matches!(slot.state, SlotState::Reserved(_)))
    );
    drop(table);
    assert_eq!(funds.live.get(), 0);
}

#[test]
fn installation_scratch_is_paid_until_its_owner_is_released() {
    let funds = funding();
    let mut table: CapTable<Obj, TestCharge> = CapTable::try_new(&funds).unwrap();
    let baseline = funds.live.get();
    let reservation = table.reserve(0, &funds).unwrap();
    assert_eq!(funds.live.get(), baseline);
    table.rollback(reservation);
    let reservation = table.reserve(1, &funds).unwrap();
    let capacity = table.slots.capacity() * core::mem::size_of::<Slot<Obj>>();
    let scratch = core::mem::size_of::<Handle>() + core::mem::size_of::<LastCapability<Obj>>();
    assert_eq!(funds.live.get(), capacity + scratch);
    table.rollback(reservation);
    assert_eq!(funds.live.get(), capacity);
    drop(table);
    assert_eq!(funds.live.get(), 0);
}
