//! 自动重放：sifive_u 实测轮次（f3）的帧池操作序列。

use frame_pool::{FramePool, PoolMemory, RegionNode};
use page_table::FrameNumber;

struct Mem;
const CAP: usize = 0x90000;
static mut RAM: [u8; CAP * 16] = [0; CAP * 16];

impl PoolMemory for Mem {
    fn read_meta(&mut self, fr: FrameNumber) -> RegionNode {
        let i = fr.0 * 16;
        unsafe {
            let b = &RAM[i..i + 16];
            RegionNode {
                len: usize::from_le_bytes(b[0..8].try_into().unwrap()),
                next: usize::from_le_bytes(b[8..16].try_into().unwrap()),
            }
        }
    }
    fn write_meta(&mut self, fr: FrameNumber, n: RegionNode) {
        let i = fr.0 * 16;
        unsafe {
            RAM[i..i + 8].copy_from_slice(&n.len.to_le_bytes());
            RAM[i + 8..i + 16].copy_from_slice(&n.next.to_le_bytes());
        }
    }
    fn clear_frames(&mut self, b: FrameNumber, c: usize) {
        let i = b.0 * 16;
        unsafe { RAM[i..i + c * 16].fill(0) }
    }
}

#[test]
fn replay_f3() {
    let mut owned = [false; CAP];
    let mut p = FramePool::new(Mem);
    p.add_region(FrameNumber(0x80290), FrameNumber(0x86000));
    { let got = p.alloc_contiguous(8).expect("alloc"); assert_eq!(got.0, 548856); for fr in got.0..got.0 + 8 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    for fr in 548856..548856 + 8 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548856"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548856), 8);
    { let got = p.alloc_contiguous(8).expect("alloc"); assert_eq!(got.0, 548856); for fr in got.0..got.0 + 8 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    for fr in 548856..548856 + 8 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548856"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548856), 8);
    { let got = p.alloc_contiguous(256).expect("alloc"); assert_eq!(got.0, 548608); for fr in got.0..got.0 + 256 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    // RANGE 0x80200000-0x80205000
    for fr in 524800..524805 { p.dealloc(FrameNumber(fr), 1); }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 524804); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 524803); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 524802); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 524801); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 524800); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548607); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548606); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548605); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548604); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548603); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548602); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548601); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548600); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548599); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548598); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548597); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548596); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548595); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 548594); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(2048).expect("alloc"); assert_eq!(got.0, 546546); for fr in got.0..got.0 + 2048 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546545); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546544); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546543); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546542); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546541); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546540); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546539); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546538); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546537); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546536); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546535); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546534); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546533); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546532); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546531); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546530); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546529); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546528); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546527); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546526); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546525); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546524); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546523); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546522); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546521); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546520); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546519); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546518); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546517); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546516); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546515); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546514); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546513); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546512); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546511); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546510); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546509); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546508); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546507); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546506); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546505); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546504); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546503); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546502); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546501); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546500); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546499); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546498); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546497); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546496); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546495); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546494); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546493); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546492); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546491); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546490); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546489); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546488); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546487); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546486); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546485); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546484); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546483); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546482); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546481); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546480); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546479); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546478); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546477); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546476); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546475); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 546474); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(2048).expect("alloc"); assert_eq!(got.0, 544426); for fr in got.0..got.0 + 2048 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544425); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544424); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544423); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544422); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544421); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544420); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544419); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544418); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544417); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544416); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544415); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544414); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544413); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544412); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544411); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544410); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544409); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544408); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544407); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544406); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544405); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544404); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544403); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544402); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544401); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544400); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544399); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544398); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544397); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544396); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 544395); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(2048).expect("alloc"); assert_eq!(got.0, 542347); for fr in got.0..got.0 + 2048 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542346); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542345); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542344); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542343); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542342); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542341); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542340); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542339); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542338); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542337); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542336); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542335); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542334); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542333); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542332); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542331); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542330); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542329); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542328); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542327); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542326); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542325); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542324); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 542323); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(2048).expect("alloc"); assert_eq!(got.0, 540275); for fr in got.0..got.0 + 2048 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 540274); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 540273); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 540272); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 540271); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    { let got = p.alloc_contiguous(1).expect("alloc"); assert_eq!(got.0, 540270); for fr in got.0..got.0 + 1 { assert!(!owned[fr as usize], "双重分配 {fr:#x}"); owned[fr as usize] = true; } }
    for fr in 546545..546545 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=546545"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(546545), 1);
    for fr in 524804..524804 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=524804"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(524804), 1);
    for fr in 524803..524803 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=524803"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(524803), 1);
    for fr in 524800..524800 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=524800"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(524800), 1);
    for fr in 548607..548607 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548607"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548607), 1);
    for fr in 548606..548606 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548606"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548606), 1);
    for fr in 548605..548605 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548605"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548605), 1);
    for fr in 548604..548604 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548604"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548604), 1);
    for fr in 548603..548603 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548603"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548603), 1);
    for fr in 548602..548602 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548602"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548602), 1);
    for fr in 548601..548601 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548601"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548601), 1);
    for fr in 548600..548600 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548600"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548600), 1);
    for fr in 548599..548599 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548599"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548599), 1);
    for fr in 548598..548598 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548598"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548598), 1);
    for fr in 548597..548597 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548597"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548597), 1);
    for fr in 548596..548596 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548596"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548596), 1);
    for fr in 548595..548595 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548595"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548595), 1);
    for fr in 548594..548594 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=548594"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(548594), 1);
    for fr in 546546..546546 + 2048 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=546546"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(546546), 2048);
    for fr in 540269..540269 + 1 { assert!(owned[fr as usize], "未持有即归还 {fr:#x} b=540269"); owned[fr as usize] = false; }
    p.dealloc(FrameNumber(540269), 1);
}
