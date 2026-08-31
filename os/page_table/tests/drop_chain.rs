//! 显式 drain 链记账完整性：建树 → 映射（含 mega 分裂）→ max_work=1 拆卸，
//! 断言每个 owner 恰好归还一次、无泄漏、无重复。

use page_table::{
    DrainStep, FrameNumber, MapError, Ppn, Pte, TableFrameMemory, TableFrameOwner, TableTree, Vpn,
};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Default)]
struct Ledger {
    live: HashSet<u64>,
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

fn with_ledger<R>(f: impl FnOnce(&mut Ledger) -> R) -> R {
    let mut guard = LEDGER.lock().unwrap();
    f(guard.as_mut().unwrap())
}

#[derive(Default)]
struct Tables(HashMap<u64, *mut [page_table::Pte; 512]>);

unsafe impl Send for Tables {}

static TABLES: Mutex<Option<Tables>> = Mutex::new(None);

struct Mem;

struct Owner(FrameNumber);

impl TableFrameOwner for Owner {
    fn number(&self) -> FrameNumber {
        self.0
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let frame = self.0;
        let in_live = with_ledger(|ledger| ledger.live.take(&(frame.0 as u64))).is_some();
        assert!(
            in_live,
            "returned unheld frame {:#x} (absent from live set)",
            frame.0
        );
        let storage = TABLES
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .0
            .remove(&(frame.0 as u64));
        assert!(
            storage.is_some(),
            "returned frame without storage {:#x}",
            frame.0
        );
        // SAFETY: allocation was created by Box::into_raw for this unique owner and removed once.
        unsafe { drop(Box::from_raw(storage.unwrap())) };
    }
}

fn allocate_owner() -> Owner {
    let frame = with_ledger(|ledger| {
        let mut number = 1u64;
        while ledger.live.contains(&number) {
            number += 1;
        }
        assert!(ledger.live.insert(number));
        number
    });
    let previous = TABLES
        .lock()
        .unwrap()
        .get_or_insert_with(Tables::default)
        .0
        .insert(
            frame,
            Box::into_raw(Box::new(std::array::from_fn(|_| Pte::invalid()))),
        );
    assert!(previous.is_none());
    Owner(FrameNumber(frame as usize))
}

fn supply(count: usize) -> Vec<Owner> {
    (0..count).map(|_| allocate_owner()).collect()
}

impl TableFrameMemory for Mem {
    type FrameOwner = Owner;

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; 512] {
        let mut guard = TABLES.lock().unwrap();
        let table = guard
            .as_mut()
            .unwrap()
            .0
            .get_mut(&(frame.0 as u64))
            .expect("accessed unheld table");
        // SAFETY: tests serialize through TABLES; TableTree is the only accessor while alive.
        unsafe { &mut **table }
    }
}

fn flags() -> u64 {
    page_table::flags::V | page_table::flags::R | page_table::flags::W | page_table::flags::U
}

fn map(
    tree: &mut TableTree<Mem, 3>,
    vpn: Vpn,
    count: usize,
    ppn: Ppn,
    flags: u64,
) -> Result<(), MapError> {
    let preflight = tree.preflight_map(vpn, count, ppn, flags)?;
    let prepared = tree
        .prepare(preflight, supply(preflight.required_frames()))
        .map_err(|failure| failure.error)?;
    let outcome = tree.publish(prepared);
    assert!(outcome.unused.is_empty());
    assert!(outcome.retired.is_empty());
    Ok(())
}

#[test]
fn max_work_one_drain_returns_every_owner_once() {
    *LEDGER.lock().unwrap() = Some(Ledger::default());
    *TABLES.lock().unwrap() = Some(Tables::default());
    let data_frames: Vec<u64> = (0x1000..0x1100).collect();

    let root = allocate_owner();
    let mut tree = TableTree::<Mem, 3>::new(Mem, root);
    for vpn in 0x10..0x50 {
        map(
            &mut tree,
            Vpn(vpn),
            1,
            Ppn(data_frames[vpn - 0x10] as usize),
            flags(),
        )
        .unwrap();
    }
    for i in 0..4 {
        map(
            &mut tree,
            Vpn(0x200 + i * 0x150),
            0x40,
            Ppn(0x5000 + i * 0x400),
            flags(),
        )
        .unwrap();
    }
    map(&mut tree, Vpn(0x300 + 0x11), 1, Ppn(0x9000), flags()).unwrap();

    let mut cursor = tree.begin_drain();
    let mut work = 0usize;
    loop {
        match tree.drain_step(&mut cursor) {
            DrainStep::Progress => work += 1,
            DrainStep::Retired(owner) => {
                work += 1;
                drop(owner);
            }
            DrainStep::Complete => break,
        }
    }
    assert!(
        work > page_table::ENTRIES,
        "drain did not advance incrementally"
    );
    drop(tree.finish_drain());

    with_ledger(|ledger| {
        assert!(
            ledger.live.is_empty(),
            "leaked table frames: {:?}",
            ledger.live
        );
    });
    assert!(TABLES.lock().unwrap().as_ref().unwrap().0.is_empty());
}
