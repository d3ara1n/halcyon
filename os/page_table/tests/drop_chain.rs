//! Drop 链记账完整性：建树 → 映射（含 mega 分裂）→ 销毁，
//! 断言每个分配帧恰好归还一次、无泄漏、无重复。

use page_table::{
    FrameMemory, FrameNumber, MapError, Ppn, PreparedTranslation, Pte, ReservedTableFrame,
    TableTree, Vpn,
};
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Default)]
struct Ledger {
    live: HashSet<u64>,
}

static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

fn with_ledger<R>(f: impl FnOnce(&mut Ledger) -> R) -> R {
    let mut g = LEDGER.lock().unwrap();
    f(g.as_mut().unwrap())
}

#[derive(Default)]
struct Tables(std::collections::HashMap<u64, *mut [page_table::Pte; 512]>);
unsafe impl Send for Tables {}
static LEDGER2: Mutex<Option<Tables>> = Mutex::new(None);

struct Mem;

struct ReservedFrame {
    frame: FrameNumber,
    committed: bool,
}

impl ReservedTableFrame for ReservedFrame {
    fn number(&self) -> FrameNumber {
        self.frame
    }

    fn commit(mut self) -> FrameNumber {
        self.committed = true;
        self.frame
    }
}

impl Drop for ReservedFrame {
    fn drop(&mut self) {
        if !self.committed {
            Mem.free_frame(self.frame);
        }
    }
}

impl FrameMemory for Mem {
    type ReservedFrame = ReservedFrame;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, page_table::FrameExhausted> {
        let frame = with_ledger(|ledger| {
            let mut number = 1u64;
            while ledger.live.contains(&number) {
                number += 1;
            }
            ledger.live.insert(number);
            number
        });
        LEDGER2
            .lock()
            .unwrap()
            .get_or_insert_with(Tables::default)
            .0
            .insert(
                frame,
                Box::into_raw(Box::new(std::array::from_fn(|_| Pte::invalid()))),
            );
        Ok(ReservedFrame {
            frame: FrameNumber(frame as usize),
            committed: false,
        })
    }
    fn free_frame(&mut self, frame: FrameNumber) {
        let in_live = with_ledger(|l| l.live.take(&(frame.0 as u64))).is_some();
        assert!(
            in_live,
            "freed unheld frame {:#x} (absent from live set)",
            frame.0
        );
        let p = LEDGER2
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .0
            .remove(&(frame.0 as u64));
        assert!(p.is_some(), "freed frame without storage {:#x}", frame.0);
        unsafe { drop(Box::from_raw(p.unwrap())) };
    }
    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; 512] {
        let mut g = LEDGER2.lock().unwrap();
        let t = g
            .as_mut()
            .unwrap()
            .0
            .get_mut(&(frame.0 as u64))
            .expect("accessed unheld table");
        unsafe { &mut **t }
    }
}

fn flags() -> u64 {
    page_table::flags::V | page_table::flags::R | page_table::flags::W | page_table::flags::U
}

fn publish(tree: &mut TableTree<Mem, 3>, prepared: PreparedTranslation<ReservedFrame>) {
    tree.publish(prepared);
}

fn map(
    tree: &mut TableTree<Mem, 3>,
    vpn: Vpn,
    count: usize,
    ppn: Ppn,
    flags: u64,
) -> Result<(), MapError> {
    let prepared = tree.prepare_map(vpn, count, ppn, flags)?;
    publish(tree, prepared);
    Ok(())
}

#[test]
fn drop_chain_accounting() {
    *LEDGER.lock().unwrap() = Some(Ledger::default());
    let data_frames: Vec<u64> = (0x1000..0x1100).collect(); // 模拟数据帧（不经 Mem 分配）

    {
        let mut tree = TableTree::<Mem, 3>::new(Mem).unwrap();
        // ELF 式逐页映射（低地址区）
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
        // 大跨度映射触发多级分支与 mega 分裂
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
        // mega 区域部分覆盖 → 分裂路径
        map(&mut tree, Vpn(0x300 + 0x11), 1, Ppn(0x9000), flags()).unwrap();
        let _ = &mut tree;
    } // Drop：递归释放全部表帧

    with_ledger(|l| {
        assert!(l.live.is_empty(), "leaked table frames: {:?}", l.live);
    });
}
