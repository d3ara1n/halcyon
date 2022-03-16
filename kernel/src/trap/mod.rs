use core::arch::global_asm;

use alloc::boxed::Box;
use riscv::register::{
    mtvec::TrapMode,
    scause::{self, Exception, Interrupt, Trap},
    sscratch,
    sstatus::{self, Sstatus},
    stvec, mepc,
};

global_asm!(include_str!("trap.S"));

extern "C" {
    fn trap_vector();
}

#[repr(C)]
pub struct TrapFrame {
    x: [usize; 32],
    fx: [usize; 32],
    satp: usize,
    //stack: *mut u8,
}

impl TrapFrame {
    pub fn new() -> Self {
        Self {
            x: [0; 32],
            fx: [0; 32],
            satp: 0,
            //stack: 0
        }
    }
}

pub fn init() {
    // sscratch 指向 TrapFrame 所在地址，用以储存寄存器
    unsafe {
        stvec::write(trap_vector as usize, TrapMode::Direct);
    }
}

// trap_vector 在处理完之后会跳转到这
#[no_mangle]
extern "C" fn handle_trap() {
    let cause = scause::read();
    match cause.cause() {
        Trap::Interrupt(Interrupt::SupervisorTimer) => println!("TIMER!"),
        Trap::Exception(Exception::Breakpoint) => {
            //TODO: 这一步应该在 asm 里完成
            println!("BREAKPOINT!");
            let mut mepc = mepc::read();
            mepc += 4;
            mepc::write(mepc);
        }
        _ => panic!("TRAP!"),
    };
}
