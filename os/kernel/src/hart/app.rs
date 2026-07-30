use core::{
    arch::asm,
    mem::size_of,
    slice::from_raw_parts,
    sync::atomic::{AtomicUsize, Ordering},
};

use alloc::{string::String, vec::Vec};
use erhino_shared::{
    call::{SystemCall, SystemCallError},
    fal::{DentryAttribute, DentryObject, DentryType, FilesystemAbstractLayerError},
    mem::{Address, MemoryRegionAttribute},
    message::MessageDigest,
    path::Path,
    proc::{ExitCode, Pid, SignalMap},
    sync::spin::SimpleLock,
};
use flagset::FlagSet;
use lock_api::Mutex;
use num_traits::FromPrimitive;

use crate::{
    debug,
    external::{_awaken, _park, _switch},
    fs::{self},
    mm::{
        frame,
        page::{PAGE_BITS, PAGE_SIZE},
        ProcessAddressRegion, KERNEL_SATP,
    },
    println,
    rng::RandomGenerator,
    sbi,
    task::{
        ipc::{message::Message, tunnel::Tunnel},
        proc::{ProcessHealth, ProcessTunnelError},
        sched::{ScheduleContext, Scheduler},
        thread::Thread,
    },
    trap::TrapCause,
};

use super::{enter_user, send_ipi, HartStatus, HartId};

static IDLE_HARTS: AtomicUsize = AtomicUsize::new(0);
static TUNNELS: Mutex<SimpleLock, Vec<Tunnel>> = Mutex::new(Vec::new());

pub struct ApplicationHart<S, R> {
    id: usize,
    scheduler: S,
    random: R,
}

impl<S: Scheduler, R: RandomGenerator> ApplicationHart<S, R> {
    pub const fn new(hartid: HartId, scheduler: S, random: R) -> Self {
        Self {
            id: hartid,
            scheduler,
            random,
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn arranged_context(&mut self) -> (Pid, usize, Address) {
        if let Some((pid, _, satp, trapframe)) = self.scheduler.context() {
            (pid, satp, trapframe)
        } else {
            self.go_idle()
        }
    }

    pub fn _send_ipi(&self) -> bool {
        sbi::send_ipi(1, self.id as isize).is_ok()
    }

    pub fn _clear_ipi(&self) {
        // clear sip.SSIP => sip[1] = 0
        let mut sip: usize;
        unsafe {
            asm!("csrr {o}, sip",  o = out(reg) sip);
            asm!("csrw sip, {i}",i = in(reg) sip & !2);
        }
    }

    pub fn get_status(&self) -> Option<HartStatus> {
        if let Ok(ret) = sbi::hart_get_status(self.id) {
            match ret {
                0 | 2 | 6 => Some(HartStatus::Started),
                1 | 3 => Some(HartStatus::Stopped),
                4 => Some(HartStatus::Suspended),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn start(&self) -> bool {
        sbi::hart_start(self.id, _awaken as usize, enter_user as usize).is_ok()
    }

    pub fn suspend(&self) -> bool {
        sbi::hart_suspend(0, _awaken as usize, enter_user as usize).is_ok()
    }

    pub fn _stop(&self) -> bool {
        sbi::hart_stop().is_ok()
    }

    fn go_idle(&mut self) -> ! {
        debug!("#{} idle", self.id());
        self.scheduler.cancel();
        IDLE_HARTS.fetch_or(1 << self.id, Ordering::Relaxed);
        self.suspend();
        unsafe { _park() }
    }

    pub fn go_awaken(&mut self) -> ! {
        debug!("#{} awaken", self.id());
        self.scheduler.schedule();
        if let Some((_, trampoline, satp, trapframe)) = self.scheduler.context() {
            unsafe { _switch(KERNEL_SATP.load(Ordering::Relaxed), trampoline, satp, trapframe) }
        } else {
            self.go_idle()
        }
    }

    fn _handle_remote_call() {}

    fn handle_system_call(
        context: &mut S::Context,
        call: SystemCall,
        arg0: usize,
        arg1: usize,
        arg2: usize,
        arg3: usize,
        random: &mut R,
    ) -> Result<Option<usize>, SystemCallError> {
        let process = context.process();
        match call {
            SystemCall::Die => {
                panic!("Die die die!");
            }
            SystemCall::Debug => {
                let address = arg0;
                let length = arg1;
                match process.read(address, length) {
                    Ok(buffer) => {
                        let str = String::from_utf8_lossy(&buffer);
                        println!(
                            "\x1b[0;34mUSER\x1b[0m {}({}): {}",
                            context.pid(),
                            context.tid(),
                            str
                        );
                        Ok(Some(length))
                    }
                    Err(err) => Err(err.into()),
                }
            }
            SystemCall::Exit => {
                let code = arg0 as ExitCode;
                process.health = ProcessHealth::Dead(code);
                Ok(None)
            }
            SystemCall::Extend => {
                let bytes = arg0;
                debug!(
                    "app.extend Pid={} request {:#x} bytes",
                    context.pid(),
                    bytes
                );
                process
                    .extend(bytes)
                    .map_err(|err| err.into())
                    .map(|r| Some(r))
            }
            SystemCall::ThreadSpawn => {
                let func_pointer = arg0 as Address;
                let thread = Thread::new(func_pointer);
                let tid = context.add_thread(thread);
                Ok(Some(tid as usize))
            }
            SystemCall::TunnelBuild => {
                if let Some(frame) = frame::borrow(1) {
                    let mut rng = random.next();
                    let mut tunnels = TUNNELS.lock();
                    while tunnels.iter().any(|t| t.key() == rng) {
                        rng = random.next();
                    }
                    let tunnel = Tunnel::new(rng, context.pid(), frame);
                    tunnels.push(tunnel);
                    Ok(Some(rng))
                } else {
                    Err(SystemCallError::OutOfMemory)
                }
            }
            SystemCall::TunnelLink => {
                let key = arg0;
                let mut tunnels = TUNNELS.lock();
                let mut found: Option<&mut Tunnel> = None;
                for tunnel in tunnels.iter_mut() {
                    if tunnel.key() == key {
                        found = Some(tunnel);
                        break;
                    }
                }
                if let Some(tunnel) = found {
                    match process.tunnel_insert(key) {
                        Ok(slot) => {
                            let addr = process.tunnel_point() + PAGE_SIZE * slot;
                            let occupied = !tunnel.link(context.pid(), addr >> PAGE_BITS);
                            if !occupied {
                                if process
                                    .map(
                                        addr >> PAGE_BITS,
                                        tunnel.page_number(),
                                        1,
                                        MemoryRegionAttribute::Read | MemoryRegionAttribute::Write,
                                        false,
                                    )
                                    .is_ok()
                                {
                                    Ok(Some(addr))
                                } else {
                                    tunnel.unlink(context.pid());
                                    Err(SystemCallError::MemoryNotAccessible)
                                }
                            } else {
                                Err(SystemCallError::ObjectNotAccessible)
                            }
                        }
                        Err(err) => match err {
                            ProcessTunnelError::ReachLimit => Err(SystemCallError::ReachLimit),
                        },
                    }
                } else {
                    Err(SystemCallError::ObjectNotFound)
                }
            }
            SystemCall::TunnelDispose => {
                let key = arg0;
                let mut tunnels = TUNNELS.lock();
                let mut found: Option<&mut Tunnel> = None;
                let mut delete_index = 0usize;
                for (index, tunnel) in tunnels.iter_mut().enumerate() {
                    if tunnel.key() == key {
                        delete_index = index;
                        found = Some(tunnel);
                        break;
                    }
                }
                if let Some(tunnel) = found {
                    if let Some((delete, number)) = tunnel.unlink(context.pid()) {
                        if delete {
                            tunnels.swap_remove(delete_index);
                        }
                        // TODO: 进程退出的时候检查所有 owner 为 pid 的对象，确定 second == None && first == None | pid 后删除（可省略 unlink）
                        // 进程退出的时候直接按照 tunnels 表删就可以，不需要逐个检查
                        process.free(number, 1).expect("kill process if failed");
                        process.tunnel_eject(key);
                        Ok(Some(0))
                    } else {
                        Err(SystemCallError::ObjectNotAccessible)
                    }
                } else {
                    Err(SystemCallError::ObjectNotFound)
                }
            }
            SystemCall::SignalSet => {
                let mask = arg0;
                let handler = arg1;
                process.signal.set_handler(mask as SignalMap, handler);
                Ok(Some(0))
            }
            SystemCall::SignalSend => {
                let pid = arg0 as Pid;
                let signal = arg1 as SignalMap;
                let mut accepted = false;
                if context.find(pid, |target| {
                    if target.signal.is_accepted(signal) {
                        target.signal.enqueue(signal);
                        accepted = true;
                    }
                }) {
                    Ok(Some(if accepted { 1 } else { 0 }))
                } else {
                    Err(SystemCallError::ObjectNotFound)
                }
            }
            SystemCall::SignalReturn => {
                if process.signal.is_handling() {
                    process.signal.complete();
                    Ok(Some(0))
                } else {
                    Err(SystemCallError::FunctionNotAvailable)
                }
            }
            SystemCall::Access => {
                let address: Address = arg0;
                let length: usize = arg1;
                match process.read(address, length) {
                    Ok(path_buffer) => {
                        if let Ok(str) = String::from_utf8(path_buffer) {
                            if let Ok(path) = Path::from(&str) {
                                match fs::lookup(path) {
                                    Ok(dentry) => Ok(Some(fs::measure(&dentry))),
                                    Err(err) => match err {
                                        FilesystemAbstractLayerError::NotAccessible => {
                                            Err(SystemCallError::ObjectNotAccessible)
                                        }
                                        FilesystemAbstractLayerError::InvalidPath => {
                                            Err(SystemCallError::IllegalArgument)
                                        }
                                        FilesystemAbstractLayerError::NotFound => {
                                            Err(SystemCallError::ObjectNotFound)
                                        }
                                        FilesystemAbstractLayerError::ForeignMountPoint(
                                            _rem,
                                            _mid,
                                        ) => {
                                            todo!();
                                            Ok(None)
                                        }
                                        _ => Err(SystemCallError::InternalError),
                                    },
                                }
                            } else {
                                Err(SystemCallError::IllegalArgument)
                            }
                        } else {
                            Err(SystemCallError::IllegalArgument)
                        }
                    }
                    Err(err) => Err(err.into()),
                }
            }
            SystemCall::Inspect => {
                let path_address = arg0;
                let path_length = arg1;
                let buffer_address = arg2;
                let buffer_length = arg3;
                match process.read(path_address, path_length) {
                    Ok(path_buffer) => {
                        if let Ok(str) = String::from_utf8(path_buffer) {
                            if let Ok(path) = Path::from(&str) {
                                match fs::lookup(path) {
                                    Ok(dentry) => {
                                        let mut obj = Vec::<(DentryObject, String)>::new();
                                        fs::make_objects(&dentry, &mut obj);
                                        let mut copied = 0usize;
                                        let mut count = 0usize;
                                        for (d, s) in &obj {
                                            let size = size_of::<DentryObject>();
                                            // 写入对象必须按 8 对齐， DentryObject 已经保证 8 byte 对齐了，就差 name 了。
                                            let name_length = (s.len() + 8 - 1) & !(8 - 1);
                                            if copied + size + name_length <= buffer_length {
                                                process.write(buffer_address + copied, unsafe{
                                                    from_raw_parts(d as *const DentryObject as *const u8, size)
                                                }, size).expect("return user an error if failed or kill process");
                                                process
                                                    .write(
                                                        buffer_address + copied + size,
                                                        s.as_bytes(),
                                                        s.len(),
                                                    )
                                                    .expect("error!");
                                                copied += size + name_length;
                                                count += 1;
                                            } else {
                                                // 空间不够就不继续了
                                                break;
                                            }
                                        }
                                        Ok(Some(count))
                                    }
                                    Err(err) => match err {
                                        FilesystemAbstractLayerError::NotAccessible => {
                                            Err(SystemCallError::ObjectNotAccessible)
                                        }
                                        FilesystemAbstractLayerError::InvalidPath => {
                                            Err(SystemCallError::IllegalArgument)
                                        }
                                        FilesystemAbstractLayerError::NotFound => {
                                            Err(SystemCallError::ObjectNotFound)
                                        }
                                        FilesystemAbstractLayerError::ForeignMountPoint(
                                            _rem,
                                            _mid,
                                        ) => {
                                            todo!();
                                            Ok(None)
                                        }
                                        _ => Err(SystemCallError::InternalError),
                                    },
                                }
                            } else {
                                Err(SystemCallError::IllegalArgument)
                            }
                        } else {
                            Err(SystemCallError::IllegalArgument)
                        }
                    }
                    Err(err) => Err(err.into()),
                }
            }
            SystemCall::Create => {
                let path_address = arg0;
                let path_length = arg1;
                if let Some(kind) = DentryType::from_u8(arg2 as u8) {
                    if let Ok(attr) = FlagSet::<DentryAttribute>::new(arg3 as u8) {
                        match process.read(path_address, path_length) {
                            Ok(path_buffer) => {
                                if let Ok(str) = String::from_utf8(path_buffer) {
                                    if let Ok(path) = Path::from(&str) {
                                        match fs::create(path, kind, attr) {
                                            Ok(()) => Ok(Some(0)),
                                            Err(err) => match err {
                                                FilesystemAbstractLayerError::NotAccessible => {
                                                    Err(SystemCallError::ObjectNotAccessible)
                                                }
                                                FilesystemAbstractLayerError::InvalidPath => {
                                                    Err(SystemCallError::IllegalArgument)
                                                }
                                                FilesystemAbstractLayerError::NotFound => {
                                                    Err(SystemCallError::ObjectNotFound)
                                                }
                                                FilesystemAbstractLayerError::ForeignMountPoint(
                                                    _rem,
                                                    _mid,
                                                ) => {
                                                    todo!();
                                                    Ok(None)
                                                }
                                                _ => Err(SystemCallError::InternalError),
                                            },
                                        }
                                    } else {
                                        Err(SystemCallError::IllegalArgument)
                                    }
                                } else {
                                    Err(SystemCallError::IllegalArgument)
                                }
                            }
                            Err(err) => Err(err.into()),
                        }
                    } else {
                        Err(SystemCallError::IllegalArgument)
                    }
                } else {
                    Err(SystemCallError::IllegalArgument)
                }
            }
            SystemCall::Read => {
                let path_address = arg0;
                let path_length = arg1;
                let buffer_address = arg2;
                let buffer_length = arg3;
                match process.read(path_address, path_length) {
                    Ok(path_buffer) => {
                        if let Ok(str) = String::from_utf8(path_buffer) {
                            if let Ok(path) = Path::from(&str) {
                                match fs::read(path, buffer_length) {
                                    Ok(bytes) => {
                                        if let Ok(written) =
                                            process.write(buffer_address, &bytes, bytes.len())
                                        {
                                            Ok(Some(written))
                                        } else {
                                            Err(SystemCallError::MemoryNotAccessible)
                                        }
                                    }
                                    Err(err) => match err {
                                        FilesystemAbstractLayerError::NotAccessible => {
                                            Err(SystemCallError::ObjectNotAccessible)
                                        }
                                        FilesystemAbstractLayerError::InvalidPath => {
                                            Err(SystemCallError::IllegalArgument)
                                        }
                                        FilesystemAbstractLayerError::NotFound => {
                                            Err(SystemCallError::ObjectNotFound)
                                        }
                                        FilesystemAbstractLayerError::ForeignMountPoint(
                                            _rem,
                                            _mid,
                                        ) => {
                                            todo!();
                                            Ok(None)
                                        }
                                        _ => Err(SystemCallError::InternalError),
                                    },
                                }
                            } else {
                                Err(SystemCallError::IllegalArgument)
                            }
                        } else {
                            Err(SystemCallError::IllegalArgument)
                        }
                    }
                    Err(err) => Err(err.into()),
                }
            }
            SystemCall::Write => {
                let path_address = arg0;
                let path_length = arg1;
                let buffer_address = arg2;
                let buffer_length = arg3;
                match process.read(path_address, path_length) {
                    Ok(path_buffer) => {
                        if let Ok(str) = String::from_utf8(path_buffer) {
                            if let Ok(path) = Path::from(&str) {
                                match process.read(buffer_address, buffer_length) {
                                    Ok(buffer) => match fs::write(path, buffer) {
                                        Ok(()) => Ok(Some(0000usize)),
                                        Err(err) => match err {
                                            FilesystemAbstractLayerError::NotAccessible => {
                                                Err(SystemCallError::ObjectNotAccessible)
                                            }
                                            FilesystemAbstractLayerError::InvalidPath => {
                                                Err(SystemCallError::IllegalArgument)
                                            }
                                            FilesystemAbstractLayerError::NotFound => {
                                                Err(SystemCallError::ObjectNotFound)
                                            }
                                            FilesystemAbstractLayerError::ForeignMountPoint(
                                                _rem,
                                                _mid,
                                            ) => {
                                                todo!();
                                                Ok(None)
                                            }
                                            _ => Err(SystemCallError::InternalError),
                                        },
                                    },
                                    Err(err) => Err(err.into()),
                                }
                            } else {
                                Err(SystemCallError::IllegalArgument)
                            }
                        } else {
                            Err(SystemCallError::IllegalArgument)
                        }
                    }
                    Err(err) => Err(err.into()),
                }
            }
            SystemCall::Send => {
                let target = arg0 as Pid;
                let kind = arg1;
                let buffer_address = arg2 as Address;
                let buffer_length = arg3;
                let mut error: Option<SystemCallError> = None;
                if context.find(target, |to| {
                    match process.read(buffer_address, buffer_length) {
                        Ok(buffer) => {
                            let msg = Message::new(context.pid(), kind, buffer);
                            if !to.mailbox.put(msg) {
                                error = Some(SystemCallError::ObjectNotAvailable)
                            }
                        }
                        Err(err) => error = Some(err.into()),
                    }
                }) {
                    if let Some(err) = error {
                        Err(err)
                    } else {
                        Ok(Some(0))
                    }
                } else {
                    Err(SystemCallError::ObjectNotFound)
                }
            }
            SystemCall::Peek => {
                let buffer_address = arg0 as Address;
                let buffer_length = arg1;
                if buffer_length == size_of::<MessageDigest>() {
                    let thread = context.thread();
                    if thread.mailbox.available() {
                        if let Some(message) = process.mailbox.take() {
                            let digest = message.digest();
                            thread.mailbox.put(message);
                            let bytes = unsafe {
                                from_raw_parts(
                                    (&digest as *const MessageDigest) as *const u8,
                                    size_of::<MessageDigest>(),
                                )
                            };
                            match process.write(buffer_address, bytes, size_of::<MessageDigest>()) {
                                Ok(_) => Ok(Some(1)),
                                Err(err) => return Err(err.into()),
                            }
                        } else {
                            Err(SystemCallError::ObjectNotAvailable)
                        }
                    } else {
                        Err(SystemCallError::ReachLimit)
                    }
                } else {
                    Err(SystemCallError::IllegalArgument)
                }
            }
            SystemCall::Receive => {
                let buffer_address = arg0 as Address;
                let buffer_length = arg1;
                let thread = context.thread();
                if let Some(message) = thread.mailbox.take() {
                    if message.len() == buffer_length {
                        match process.write(buffer_address, message.content(), buffer_length) {
                            Ok(written) => Ok(Some(written)),
                            Err(err) => Err(err.into()),
                        }
                    } else {
                        Err(SystemCallError::IllegalArgument)
                    }
                } else {
                    Err(SystemCallError::ObjectNotAvailable)
                }
            }
            _ => unimplemented!("unimplemented syscall: {:?}", call),
        }
    }

    pub fn trap(&mut self, cause: TrapCause) {
        // 同步 ecall 会直接操作并获得结果，PC+4
        // 异步 ecall 则只会将 task 状态设置为 Pending，PC 保持原样。调度器在解除其 Pending 状态成为 Fed 后重新加入调度，并触发 ecall，写入结果
        match cause {
            TrapCause::TimerInterrupt => {
                self.scheduler.schedule();
            }
            TrapCause::Breakpoint => {
                // for debugger
                self.scheduler.with_context(|ctx| {
                    ctx.process().health = ProcessHealth::Dead(-0x114514);
                    println!(
                        "#{} Pid={} Tid={} requested a breakpoint",
                        self.id,
                        ctx.pid(),
                        ctx.tid()
                    );
                });
                self.scheduler.schedule();
            }
            TrapCause::PageFault(address, op) => {
                if let Some(region) = self.scheduler.is_address_in(address) {
                    match region {
                        ProcessAddressRegion::Stack(_) => {
                            self.scheduler.with_context(|ctx| {
                                ctx.process()
                                    .fill(
                                        address >> PAGE_BITS,
                                        1,
                                        MemoryRegionAttribute::Write | MemoryRegionAttribute::Read,
                                        false,
                                    )
                                    .expect("fill stack failed, to be killed");
                            });
                        }
                        ProcessAddressRegion::TrapFrame(_) => {
                            self.scheduler.with_context(|ctx| {
                                ctx.process()
                                    .fill(
                                        address >> PAGE_BITS,
                                        1,
                                        MemoryRegionAttribute::Write | MemoryRegionAttribute::Read,
                                        true,
                                    )
                                    .expect("fill trapframe failed, to be killed");
                            });
                        }
                        _ => self.scheduler.with_context(|ctx| {
                            todo!(
                                "unexpected {:?} memory page fault at: {:#x}, dump: \n{}",
                                op,
                                address,
                                ctx.trapframe()
                            )
                        }),
                    }
                } else {
                    unreachable!(
                        "previous process triggered page fault while it is not in current field"
                    )
                }
            }
            TrapCause::EnvironmentCall => self.scheduler.with_context(|ctx| {
                let trapframe = ctx.trapframe();
                // 只有同步调用才会前进下一个指令
                // `handle_system_call` 返回 Ok(SystemCallProcedureResult)，包含 Finished(usize) 和 Pending()，后者会切换到其他进程
                let mut syscall = trapframe
                    .extract_syscall()
                    .expect("invalid sys call triggered");
                match Self::handle_system_call(
                    ctx,
                    syscall.call,
                    syscall.arg0,
                    syscall.arg1,
                    syscall.arg2,
                    syscall.arg3,
                    &mut self.random,
                ) {
                    // Some 为同步调用，立即返回结果
                    Ok(Some(res)) => {
                        syscall.write_response(res);
                        trapframe.move_next_instruction()
                    }
                    // None 为异步调用，触发调度
                    Ok(None) => ctx.schedule(),
                    Err(err) => {
                        syscall.write_error(err);
                        trapframe.move_next_instruction();
                    }
                }
            }),
            _ => {
                todo!("unimplemented trap cause {:?}", cause)
            }
        }
    }
}

pub fn awake_idle() -> bool {
    let map = IDLE_HARTS.load(Ordering::Relaxed);
    send_ipi(map)
}
