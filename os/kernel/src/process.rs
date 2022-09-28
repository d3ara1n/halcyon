use core::arch::global_asm;
use core::fmt::{Debug, Formatter};
use core::ops::BitAnd;
use core::ptr::null_mut;

use elf_rs::{
    Elf, ElfAbi, ElfFile, ElfHeader, ElfMachine, ElfType, ProgramHeaderFlags, ProgramType,
};
use flagset::FlagSet;

use crate::paged::page_table::{PageTableEntry, PageTableEntryFlag};
use crate::paged::unit::MemoryUnit;
use crate::process::error::ProcessSpawnError;
use crate::trap::TrapFrame;
use crate::utils::calculate_instruction_length;
use crate::{_kernel_end, _kernel_start, alloc, println, PageTable};

pub mod error;
pub mod ipc;
pub mod scheduler;
pub mod signal;

pub type ExitCode = i32;
pub type Pid = u32;
pub type Address = u64;

// 进程地址空间分配
const PROCESS_STACK_ADDRESS: Address = 0x40_0000_0000 - 1; // 256GB

#[derive(PartialEq, Clone)]
pub enum ProcessState {
    Idle,
    Running,
    Sleeping,
    Dead,
}

// 服务进程: fs, net, driver, adv_ipc
// 服务进程会一直处于 Sleeping 阶段而被跳过,当其他进程使用系统调用与服务进程通信,其会被运行并设置状态为 Running.

pub struct Process {
    trap: TrapFrame,
    // set by scheduler
    pid: Pid,
    parent: Pid,
    memory: MemoryUnit,
    state: ProcessState,
    signal_handler_address: Address,
    exit_code: ExitCode,
}

impl Process {

    pub fn from_elf(bytes: &[u8]) -> Result<Self, ProcessSpawnError> {
        if let Ok(elf) = Elf::from_bytes(bytes) {
            let mut process = Self {
                trap: TrapFrame::zero(),
                pid: 0,
                // 没有设置过那就直接送给 init0 当子进程
                parent: 0,
                memory: MemoryUnit::new(PageTable::new(2, alloc().unwrap())),
                state: ProcessState::Idle,
                signal_handler_address: 0,
                exit_code: 0,
            };
            process.trap.pc = elf.entry_point();
            process.trap.satp = process.memory.satp();
            process.trap.status = 1 << 7 | 1 << 5 | 1 << 4;
            let header = elf.elf_header();
            // TODO: validate RV64 from flags parameter
            if header.machine() != ElfMachine::RISC_V || header.elftype() != ElfType::ET_EXEC {
                return Err(ProcessSpawnError::WrongTarget);
            }
            for ph in elf.program_header_iter() {
                if ph.ph_type() == ProgramType::LOAD {
                    process.memory.write(
                        ph.vaddr(),
                        ph.content(),
                        ph.memsz() as usize,
                        Self::flags_to_permission(ph.flags()),
                    );
                }
            }
            // map stack
            process.memory.write(
                PROCESS_STACK_ADDRESS - 4095,
                &[0; 4095],
                0,
                PageTableEntryFlag::UserReadWrite,
            );
            process
                .memory
                .write(0x4000, &[0; 0x8000], 0, PageTableEntryFlag::UserReadWrite);
            // set context
            process.trap.x[2] = PROCESS_STACK_ADDRESS;
            Ok(process)
        } else {
            Err(ProcessSpawnError::BrokenBinary)
        }
    }

    pub fn set_signal_handler(&mut self, address: u64) {
        self.signal_handler_address = address;
    }

    pub fn cleanup(self) {
        self.memory.free();
    }

    pub fn fork(&self) -> Process {
        let mut proc = Self {
            trap: self.trap.clone(),
            pid: 0,
            parent: self.pid,
            memory: self.memory.fork(),
            exit_code: self.exit_code,
            signal_handler_address: self.signal_handler_address,
            state: self.state.clone(),
        };
        proc.trap.satp = self.memory.satp();
        proc
    }

    pub fn write_generic_register(&mut self, index: usize, value: u64) {
        self.trap.x[index] = value;
    }

    pub fn write_float_register(&mut self, index: usize, value: u64) {
        self.trap.f[index] = value;
    }

    pub fn set_return_value_in_register(&mut self, value: u64) {
        self.write_generic_register(10, value);
    }

    pub fn move_to_next_instruction(&mut self) {
        let ins = self.memory.read_byte(self.trap.pc).unwrap();
        let length = calculate_instruction_length(ins);
        self.trap.pc += length as u64 / 8;
    }

    fn flags_to_permission(
        flags: ProgramHeaderFlags,
    ) -> impl Into<FlagSet<PageTableEntryFlag>> + Clone {
        let mut perm = PageTableEntryFlag::Valid | PageTableEntryFlag::User;
        let bits = flags.bits();
        if bits & 0b1 == 1 {
            perm |= PageTableEntryFlag::Executable;
        }
        if bits & 0b10 == 0b10 {
            perm |= PageTableEntryFlag::Writeable;
        }
        if bits & 0b100 == 0b100 {
            perm |= PageTableEntryFlag::Readable;
        }
        perm
    }
}
