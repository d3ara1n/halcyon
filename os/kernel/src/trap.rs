use core::{fmt::Display, sync::atomic::Ordering};

use erhino_shared::{
    call::{SystemCall, SystemCallError},
    mem::{Address, MemoryOperation}, proc::Tid,
};
use num_traits::FromPrimitive;
use riscv::{
    interrupt::{Trap, supervisor::{Exception, Interrupt}},
    register::{satp, scause::{self, Scause}, sepc, stval},
};

use crate::{
    external::_kernel_trap,
    hart::{self, HartKind},
    mm::KERNEL_SATP,
};

pub struct SystemCallRequest<'context> {
    trapframe: &'context mut TrapFrame,
    pub call: SystemCall,
    pub arg0: usize,
    pub arg1: usize,
    pub arg2: usize,
    pub arg3: usize,
}

impl<'context> SystemCallRequest<'context> {
    pub fn write_error(&mut self, error: SystemCallError) {
        self.trapframe.x[10] = error as u64;
        self.trapframe.x[11] = 0;
    }

    pub fn write_response(&mut self, ret: usize) {
        self.trapframe.x[10] = 0;
        self.trapframe.x[11] = ret as u64;
    }
}

#[derive(Debug)]
#[allow(unused)]
pub enum TrapCause {
    Unknown,
    SoftwareInterrupt,
    ExternalInterrupt,
    TimerInterrupt,
    EnvironmentCall,
    Breakpoint,
    PageFault(Address, MemoryOperation),
}

#[repr(C)]
pub struct TrapFrame {
    // 0-255
    pub x: [u64; 32],
    // 256 - 511
    pub f: [u64; 32],
    // 512
    pub pc: u64,
    // 以下是临时数据，与 TrapFrame 所属的进程无关，只读
    // 520 Currently the hart it running in. Guaranteed by trap_vector in assembly.asm
    kernel_tp: u64,
    // 528
    kernel_satp: u64,
    // 536
    kernel_trap: u64,
    // 544
    user_trap: u64,
}

impl TrapFrame {
    pub fn init(
        &mut self,
        entry_point: Address,
        stack_address: Address,
        user_trap: Address,
        thread_id: Tid,
        registers: [u64; 2],
    ) {
        self.x = [0; 32];
        self.x[4] = thread_id as u64;
        self.x[10] = registers[0];
        self.x[11] = registers[1];
        self.f = [0; 32];
        self.x[2] = stack_address as u64;
        self.pc = entry_point as u64;
        self.kernel_tp = 0u64;
        self.kernel_satp = KERNEL_SATP.load(Ordering::Relaxed) as u64;
        self.kernel_trap = _kernel_trap as u64;
        self.user_trap = user_trap as u64;
    }

    pub fn extract_syscall(&mut self) -> Option<SystemCallRequest> {
        let which = self.x[17] as usize;
        if let Some(call) = SystemCall::from_usize(which) {
            let arg0 = self.x[10] as usize;
            let arg1 = self.x[11] as usize;
            let arg2 = self.x[12] as usize;
            let arg3 = self.x[13] as usize;
            Some(SystemCallRequest {
                trapframe: self,
                call: call,
                arg0,
                arg1,
                arg2,
                arg3,
            })
        } else {
            None
        }
    }

    pub fn move_next_instruction(&mut self) {
        self.pc += 4;
    }
}

impl Display for TrapFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "Registers with hartid={}", self.kernel_tp)?;
        writeln!(f, "ra={:#016x}, sp={:#016x}", self.x[1], self.x[2])?;
        writeln!(f, "gp={:#016x}, tp={:#016x}", self.x[3], self.x[4])?;
        writeln!(f, "fp={:#016x}", self.x[8])?;
        writeln!(f, "a0={:#016x}, a1={:#016x}", self.x[10], self.x[11])?;
        writeln!(f, "a2={:#016x}, a3={:#016x}", self.x[12], self.x[13])?;
        writeln!(f, "a4={:#016x}, a5={:#016x}", self.x[14], self.x[15])?;
        writeln!(f, "a6={:#016x}, a7={:#016x}", self.x[16], self.x[17])?;
        writeln!(f, "sepc={:#x}", self.pc)
    }
}

fn kernel_dump() -> ! {
    let cause = scause::read().bits();
    let val = stval::read();
    let pc = sepc::read();
    let satp = satp::read().bits();
    panic!(
        "kernel trapped!\ncause={:#x},val={:#x}\nsatp={:#x},pc={:#x}",
        cause, val, satp, pc
    )
}

#[unsafe(no_mangle)]
unsafe fn handle_kernel_trap(cause: Scause, _val: usize) {
    match cause.cause().try_into::<Interrupt, Exception>().unwrap() {
        // 也有可能是进程剩余时间片太短，还没进入用户空间就触发异常，直接转发给 hart 会导致 hart 的串行特性失效，一种解决办法是 user_ trap 时关闭 stie 和 seie
        Trap::Interrupt(Interrupt::SupervisorTimer) => todo!("nested supervisor timer"),
        _ => kernel_dump(),
    }
}

#[unsafe(no_mangle)]
unsafe fn handle_user_trap(cause: Scause, val: usize) -> (usize, Address) {
    if let HartKind::Application(hart) = hart::this_hart() {
        match cause.cause().try_into::<Interrupt, Exception>().unwrap() {
            Trap::Interrupt(Interrupt::SupervisorTimer) => hart.trap(TrapCause::TimerInterrupt),
            Trap::Exception(exception) => match exception {
                Exception::Breakpoint => hart.trap(TrapCause::Breakpoint),
                Exception::UserEnvCall => hart.trap(TrapCause::EnvironmentCall),
                Exception::LoadPageFault => {
                    hart.trap(TrapCause::PageFault(val as Address, MemoryOperation::Read))
                }
                Exception::StorePageFault => {
                    hart.trap(TrapCause::PageFault(val as Address, MemoryOperation::Write))
                }
                Exception::InstructionPageFault => hart.trap(TrapCause::PageFault(
                    val as Address,
                    MemoryOperation::Execute,
                )),
                _ => unimplemented!("unknown exception: {}:{:#x}", cause.bits(), val),
            },
            _ => {
                unimplemented!("unknown trap from user: {}:{:#x}", cause.bits(), val)
            }
        }
        let (_, satp, trampoline) = hart.arranged_context();
        (satp, trampoline)
    } else {
        unimplemented!("only application hart would trigger user trap, how this hart get here?")
    }
}
