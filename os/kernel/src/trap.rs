use core::fmt::Display;

use erhino_shared::{
    call::{KernelCall, SystemCall},
    mem::PageNumber,
};
use flagset::FlagSet;
use num_traits::FromPrimitive;
use riscv::register::{
    mcause::{Exception, Interrupt, Mcause, Trap},
    mepc, mhartid,
};

use crate::{
    mm::page::PageTableEntryFlag,
    println,
    proc::sch::{self, enter_user_mode},
    sync::{hart::HartLock, Lock},
    timer,
};

// 内核陷入帧只会在第一个陷入时用到，之后大概率用不到，所以第一个陷入帧应该分配在一个垃圾堆(_memory_end - 4096)或者栈上
// 这么做是为了避免多核同时写入，但寻思了一下，根本不会去读，那多核写就写呗，写坏了也无所谓
// 那么！就这么决定了，这个内核陷入帧是只写的！也就是它是所谓的垃圾堆！
#[export_name = "_kernel_trap"]
static KERNEL_TRAP: TrapFrame = TrapFrame::new();

#[no_mangle]
unsafe fn handle_trap(hartid: usize, cause: Mcause, frame: &mut TrapFrame) {
    match cause.cause() {
        Trap::Interrupt(Interrupt::MachineTimer) => {
            timer::tick(hartid);
        }
        Trap::Interrupt(Interrupt::MachineSoft) => {
            panic!("Machine Soft Interrupt at hart#{}", hartid);
            // its time to schedule process!
        }
        Trap::Exception(Exception::Breakpoint) => {
            panic!("user breakpoint at hart#{}: frame=\n{}", hartid, frame);
        }
        Trap::Exception(Exception::StoreFault) => {
            panic!("Store/AMO access fault hart#{}: frame=\n{}", hartid, frame);
        }
        Trap::Exception(Exception::MachineEnvCall) => {
            let call_id = frame.x[17];
            if let Some(call) = KernelCall::from_u64(call_id) {
                match call {
                    KernelCall::EnterUserSpace => {
                        enter_user_mode(hartid);
                        frame.pc += 4
                    }
                }
            } else {
                panic!("unsupported kernel call: {}", call_id);
            }
        }
        Trap::Exception(Exception::UserEnvCall) => {
            let call_id = frame.x[17];
            if let Some(call) = SystemCall::from_u64(call_id) {
                match call {
                    SystemCall::Extend => unsafe {
                        if let Some(process) = sch::current_process(hartid) {
                            let start = frame.x[10];
                            let end = start + frame.x[11];
                            let bits = frame.x[12] & 0b111;
                            let flags = if bits & 0b100 == 0b100 {
                                PageTableEntryFlag::Executable
                            } else {
                                PageTableEntryFlag::Valid
                            } | if bits & 0b010 == 0b010 {
                                PageTableEntryFlag::Writeable
                            } else {
                                PageTableEntryFlag::Valid
                            } | if bits & 0b001 == 0b001 {
                                PageTableEntryFlag::Readable
                            } else {
                                PageTableEntryFlag::Valid
                            };
                            process.memory.fill(
                                (start >> 12) as PageNumber,
                                ((end - start) >> 12) as PageNumber,
                                PageTableEntryFlag::Valid | PageTableEntryFlag::User | flags,
                            );
                        }
                    },
                    _ => todo!("handle something"),
                }
                frame.pc += 4;
            } else {
                println!("unsupported system call {}", call_id);
                // kill the process or do something
            }
        }
        _ => panic!(
            "unknown trap cause at hart#{}: cause={:#x}, frame=\n{}",
            hartid,
            cause.bits(),
            frame
        ),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapFrame {
    // 0-255
    pub x: [u64; 32],
    // 256 - 511
    pub f: [u64; 32],
    // 512-519
    pub satp: u64,
    // 520-527
    pub status: u64,
    // 528-535
    pub pc: u64,
}

impl TrapFrame {
    pub const fn new() -> Self {
        Self {
            x: [0; 32],
            f: [0; 32],
            satp: 0,
            status: 0,
            pc: 0,
        }
    }
}

impl Display for TrapFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "Registers")?;
        writeln!(f, "ra={:#016x}, sp={:#016x}", self.x[1], self.x[2])?;
        writeln!(f, "gp={:#016x}, tp={:#016x}", self.x[3], self.x[4])?;
        writeln!(f, "fp={:#016x}", self.x[8])?;
        writeln!(f, "a0={:#016x}, a1={:#016x}", self.x[10], self.x[11])?;
        writeln!(f, "a2={:#016x}, a3={:#016x}", self.x[12], self.x[13])?;
        writeln!(f, "a4={:#016x}, a5={:#016x}", self.x[14], self.x[15])?;
        writeln!(f, "a6={:#016x}, a7={:#016x}", self.x[16], self.x[17])?;
        writeln!(
            f,
            "mepc={:#x}, satp={:#x}, status={:#x}",
            self.pc, self.satp, self.status
        )
    }
}
