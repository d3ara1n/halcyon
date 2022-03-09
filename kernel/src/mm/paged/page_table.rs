use core::mem::size_of;

use alloc::vec;
use alloc::vec::Vec;

use crate::config::PAGE_SIZE;

use super::{
    address::{PhysicalAddress, PhysicalPageNumber, VirtualAddress},
    frame_allocator::{frame_alloc, FrameTracker},
};

/// 64 bits(u64)
/// 64-54           53-28       27-19   18-10   9-8 7 6 5 4 3 2 1 0
/// Reserved(10)    PPN2(26)    PPN1(9) PPN0(9) RSW D A G U X W R V
/// PPN(44): 指示下一级页表所在物理页号，PPN * PAGE_SIZE + VPN(页表级数) * PTE_SIZE 就可以定位到下一级的 PTE
/// RSW: Reserved for Supervisor Software
/// D: Dirty 处理器记录自从页表项上的这一位被清零之后，页表项的对应虚拟页面是否被修改过；
/// A: Accessed 处理器记录自从页表项上的这一位被清零之后，页表项的对应虚拟页面是否被访问过；
/// G: Global 不知道干嘛的；
/// U: User 控制索引到这个页表项的对应虚拟页面是否在 CPU 处于 U 特权级的情况下是否被允许访问；
/// XWR, X(Executable), W(Writeable), R(Readable): 分别控制索引到这个页表项的对应虚拟页面是否允许读/写/执行；
/// Valid: 仅当位 V 为 1 时，页表项才是合法的；
#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry(usize);

impl PageTableEntry {
    pub fn new(v: usize) -> Self {
        PageTableEntry(v)
    }

    pub fn is_valid(&self) -> bool {
        self.0 & 0b1 != 0
    }

    pub fn is_leaf(&self) -> bool {
        self.0 & 0b1110 != 0
    }
}

pub struct PageTable {
    //entries: [PageTableEntry; PAGE_SIZE  as usize / size_of::<PageTableEntry>()], // 一个页表占用 4kb， 用来存放512个页表项（由于4k对齐，其本身可以被放进一个页内
    level: u8, // 当前表的级数
    location: PhysicalPageNumber,
    tracked_frames: Vec<FrameTracker>,
}

impl PageTable {
    pub fn new(level: u8) -> Self {
        // 在帧空间初始化一个页表
        let frame = frame_alloc().expect("frame used up");
        Self {
            level: level,
            location: frame.page_number,
            tracked_frames: vec![frame], // 自己会占用一个帧
        }
    }

    pub fn map(&self, va: VirtualAddress, pa: PhysicalAddress, size: usize) {
        //
    }

    pub fn lookup(&self, va: VirtualAddress) -> &mut PageTableEntry {
        let vpn = va.page_number();
        let index = match self.level {
            2 => vpn.2,
            1 => vpn.1,
            _ => vpn.0,
        };
        PhysicalAddress::from(self.location)
            .get_mut_offset::<PageTableEntry>((index as usize * size_of::<PageTableEntry>()) as u64)
    }
}

impl From<PageTableEntry> for PageTable{
    fn from(v: PageTableEntry) -> Self {
        //check if valid
        if v.is_leaf(){
            //None
            panic!("its a leaf");
        }
        todo!()
    }
}

impl Drop for PageTable{
    fn drop(&mut self){
        for i in &self.tracked_frames{
            drop(i);
        }
    }
}