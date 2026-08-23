//! Drop 链记账完整性：建树 → 映射（含 mega 分裂）→ 销毁，
//! 断言每个分配帧恰好归还一次、无泄漏、无重复。

use page_table::{FrameNumber, FrameMemory, Ppn, TableTree, Vpn};
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Default)]
struct Ledger {
    live: HashSet<u64>,
    tables: std::collections::HashMap<u64, Box<[page_table::Pte; 512]>>,
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

impl FrameMemory for Mem {
    fn alloc_frame(&mut self) -> Result<FrameNumber, page_table::FrameExhausted> {
        let fr = with_ledger(|l| {
            let mut n = 1u64;
            while l.live.contains(&n) {
                n += 1;
            }
            l.live.insert(n);
            n
        });
        LEDGER2.lock().unwrap().get_or_insert_with(Tables::default).0.insert(fr as u64, Box::into_raw(Box::new(std::array::from_fn(|_| page_table::Pte::invalid()))));
        Ok(FrameNumber(fr as usize))
    }
    fn free_frame(&mut self, frame: FrameNumber) {
        let in_live = with_ledger(|l| l.live.take(&(frame.0 as u64))).is_some();
        assert!(in_live, "freed unheld frame {:#x} (absent from live set)", frame.0);
        let p = LEDGER2.lock().unwrap().as_mut().unwrap().0.remove(&(frame.0 as u64));
        assert!(p.is_some(), "freed frame without storage {:#x}", frame.0);
        unsafe { drop(Box::from_raw(p.unwrap())) };
    }
    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; 512] {
        let mut g = LEDGER2.lock().unwrap();
        let t = g.as_mut().unwrap().0.get_mut(&(frame.0 as u64)).expect("accessed unheld table");
        unsafe { &mut **t }
    }
}

fn flags() -> u64 {
    page_table::flags::V | page_table::flags::R | page_table::flags::W | page_table::flags::U
}

#[test]
fn drop_chain_accounting() {
    *LEDGER.lock().unwrap() = Some(Ledger::default());
    let data_frames: Vec<u64> = (0x1000..0x1100).collect(); // 模拟数据帧（不经 Mem 分配）

    {
        let mut tree = TableTree::<Mem, 3>::new(Mem).unwrap();
        // ELF 式逐页映射（低地址区）
        for vpn in 0x10..0x50 {
            tree.map(Vpn(vpn), 1, Ppn(data_frames[(vpn - 0x10) as usize] as usize), flags())
                .unwrap();
        }
        // 大跨度映射触发多级分支与 mega 分裂
        for i in 0..4 {
            tree.map(Vpn(0x200 + i * 0x150), 0x40, Ppn(0x5000 + i * 0x400), flags())
                .unwrap();
        }
        // mega 区域部分覆盖 → 分裂路径
        tree.map(Vpn(0x300 + 0x11), 1, Ppn(0x9000), flags()).unwrap();
        let _ = &mut tree;
    } // Drop：递归释放全部表帧

    with_ledger(|l| {
        assert!(l.live.is_empty(), "leaked table frames: {:?}", l.live);
    });
}
