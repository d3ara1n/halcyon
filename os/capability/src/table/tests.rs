//! 表与持有计数的契约测试。每个用例对应 capability 分案中的一条目标契约。

use alloc::{rc::Rc, vec, vec::Vec};
use erhino_shared::object::{Handle, Rights};

use super::*;
use crate::{Holders, ZeroPolicy};

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

fn install(table: &mut CapTable<Obj>, o: &Obj, rights: Rights) -> Handle {
    table
        .insert(Cap::mint(o.clone(), rights).unwrap())
        .map_err(|(c, e)| {
            let _ = c.release();
            e
        })
        .unwrap()
}

fn close_release(table: &mut CapTable<Obj>, h: Handle) -> Option<LastCapability<Obj>> {
    table.close(h).unwrap().and_then(Cap::release)
}

// 最后持有：Terminal 归零恰好产生一次事件，并在同一原子操作中封闭铸造。
#[test]
fn terminal_last_release_emits_once_and_seals() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new().unwrap();
    let a = install(&mut t, &o, all());
    let copy = t.get(a, Rights::NONE).unwrap().derive(RW).unwrap();
    let b = t.insert(copy).map_err(|(_, e)| e).unwrap();
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
    let mut t = CapTable::try_new().unwrap();
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
    let mut src = CapTable::try_new().unwrap();
    let mut dst = CapTable::try_new().unwrap();
    let h = install(&mut src, &o, all());
    let moved = src.extract(&[(h, RW)], MOVE).unwrap();
    assert_eq!(o.0.count(), 1);
    let mut moved = moved.into_iter();
    let nh = dst.insert(moved.next().unwrap()).map_err(|(_, e)| e).unwrap();
    assert_eq!(dst.get(nh, Rights::NONE).unwrap().rights(), RW);
    assert_eq!(src.get(h, Rights::NONE).unwrap_err(), TableError::StaleHandle);
    assert!(close_release(&mut dst, nh).is_some());
}

// 批量移交任一项校验失败时表不变。
#[test]
fn failed_extract_keeps_every_source() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new().unwrap();
    let ok = install(&mut t, &o, all());
    let no_move = install(&mut t, &o, RW);
    assert_eq!(
        t.extract(&[(ok, RW), (no_move, RW)], MOVE).unwrap_err(),
        TableError::RightsDenied
    );
    assert_eq!(
        t.extract(&[(ok, RW), (ok, RW)], MOVE).unwrap_err(),
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
    let mut t = CapTable::try_new().unwrap();
    let plain = install(&mut t, &o, RW);
    let dup = install(&mut t, &o, Rights::READ | Rights::DUPLICATE);
    assert_eq!(
        t.get(plain, Rights::NONE).unwrap().derive(Rights::READ).unwrap_err(),
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
    let mut t = CapTable::try_new().unwrap();
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

// Close 撤销返还资格而不取消操作；失败回滚不复活已关闭 handle。
#[test]
fn close_revokes_claim_without_cancelling_operation() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new().unwrap();
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
#[test]
fn shut_table_releases_instead_of_restoring() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new().unwrap();
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
    let mut t = CapTable::try_with_limit(1).unwrap();
    let h = install(&mut t, &o, RW);
    let (cap, err) = t.insert(Cap::mint(o.clone(), RW).unwrap()).unwrap_err();
    assert_eq!(err, TableError::ReachLimit);
    assert!(cap.release().is_none());
    let reservation = t.reserve(0).unwrap();
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
    let mut t: CapTable<Obj> = CapTable::try_new().unwrap();
    let r = t.reserve(2).unwrap();
    let staged: Vec<Handle> = r.handles().to_vec();
    t.rollback(r);
    for h in staged {
        assert_eq!(t.get(h, Rights::NONE).unwrap_err(), TableError::StaleHandle);
    }
}

// 计数有准入上限：到达上限时铸造报错，释放始终成功。
#[test]
fn holder_limit_rejects_mint_not_release() {
    let o = Obj(Rc::new(Holders::with_limit(ZeroPolicy::Terminal, 1)));
    let a = Cap::mint(o.clone(), RW).unwrap();
    assert_eq!(Cap::mint(o.clone(), RW).unwrap_err(), TableError::ReachLimit);
    assert!(a.release().is_some());
}

// 有界回收：每次至多扫描预算内槽位，并清理预约与预留。
#[test]
fn bounded_drain_respects_budget_and_clears_claims() {
    let o = obj(ZeroPolicy::Terminal);
    let mut t = CapTable::try_new().unwrap();
    let a = install(&mut t, &o, RW);
    let b = install(&mut t, &o, RW);
    let (claimed, _ticket) = t.claim(b, Rights::WRITE).unwrap();
    let staged = t.reserve(1).unwrap();
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
    assert_eq!(t.get(a, Rights::NONE).unwrap_err(), TableError::StaleHandle);
    for h in staged.handles() {
        assert_eq!(t.get(*h, Rights::NONE).unwrap_err(), TableError::StaleHandle);
    }
    let _ = staged;
    assert!(released.pop().unwrap().release().is_none());
    assert!(claimed.release().is_some());
}
