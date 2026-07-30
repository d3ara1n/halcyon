use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use erhino_shared::{
    mem::{Address, MemoryRegionAttribute, PageNumber},
    proc::{ExecutionState, Pid, Tid},
    sync::spin::SimpleLock,
};
use flagset::FlagSet;
use lock_api::RawMutex;

use crate::{
    external::_user_trap,
    hart::{self, HartKind, HartId},
    mm::{
        page::{PageEntryImpl, PageTableEntry, PAGE_BITS, PAGE_SIZE},
        unit::{AddressSpace, MemoryUnit},
        ProcessAddressRegion,
    },
    sync::up::UpSafeCell,
    task::{
        proc::{Process, ProcessHealth},
        thread::Thread,
    },
    timer::Timer,
    trap::TrapFrame,
};

use super::{ScheduleContext, Scheduler};

type Shared<T> = UpSafeCell<T>;

// timeslice in ms
const QUANTUM: usize = 20;

static mut PROC_TABLE: ProcessTable = ProcessTable::new();

const THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

pub struct UnfairContext {
    hartid: HartId,
    process: Arc<Shared<ProcessCell>>,
    thread: Arc<Shared<ThreadCell>>,
    scheduled: bool,
}

impl ScheduleContext for UnfairContext {
    fn pid(&self) -> Pid {
        self.process.id
    }

    fn tid(&self) -> Tid {
        self.thread.id
    }

    fn process(&self) -> &mut Process {
        &mut self.process.get_mut().inner
    }

    fn thread(&self) -> &mut Thread {
        &mut self.thread.get_mut().inner
    }

    fn trapframe(&self) -> &'static mut TrapFrame {
        self.process.struct_at(self.thread.trapframe)
    }

    fn add_proc(&self, proc: Process) -> Pid {
        let table = unsafe { &mut PROC_TABLE };
        table.add(proc, Some(self.process.id))
    }

    fn add_thread(&self, thread: Thread) -> Tid {
        self.process.get_mut().add(thread)
    }

    fn schedule(&mut self) {
        self.scheduled = true;
    }

    fn find<F: FnMut(&mut Process)>(&self, pid: Pid, mut action: F) -> bool {
        if self.process.id == pid {
            action(self.process());
            true
        } else {
            if let Some(p) = unsafe { &PROC_TABLE }.find_process(pid) {
                p.state_lock.lock();
                let mutable = p.get_mut();
                action(&mut mutable.inner);
                unsafe { p.state_lock.unlock() };
                true
            } else {
                false
            }
        }
    }
}

struct ProcessLayout {
    // 跳板地址向上是跳板页，向下是 TrapFrame
    trampoline: Address,
    stack_point: Address,
    break_point: Address,
    thread_count: usize,
}

impl ProcessLayout {
    pub fn new(trampoline: Address, stack: Address, heap: Address) -> Self {
        Self {
            trampoline: trampoline,
            stack_point: stack,
            break_point: heap,
            thread_count: 1,
        }
    }

    pub fn is_address_in(&self, addr: Address) -> ProcessAddressRegion {
        match MemoryUnit::<PageEntryImpl>::is_address_in(addr) {
            AddressSpace::Invalid => ProcessAddressRegion::Invalid,
            AddressSpace::Kernel => {
                let diff = (self.trampoline - addr) / TRAPFRAME_SIZE;
                ProcessAddressRegion::TrapFrame(diff as Tid)
            }
            AddressSpace::User => {
                if addr < self.break_point {
                    ProcessAddressRegion::Program
                } else {
                    let diff = (self.stack_point - addr - 1) / THREAD_STACK_SIZE;
                    let count = self.thread_count;
                    if diff < count {
                        ProcessAddressRegion::Stack(diff as Tid)
                    } else {
                        ProcessAddressRegion::Heap
                    }
                }
            }
        }
    }
}

// 只有 next, prev 需要用 ring_lock, head 用 head_lock, inner 和 layout 则需要手动获得中断安全锁
struct ProcessCell {
    inner: Process,
    id: Pid,
    parent: Pid,
    layout: ProcessLayout,
    head: Option<Arc<Shared<ThreadCell>>>,
    head_lock: SimpleLock,
    next: Option<Arc<Shared<ProcessCell>>>,
    prev: Option<Weak<Shared<ProcessCell>>>,
    ring_lock: SimpleLock,
    state_lock: SimpleLock,
}

const TRAPFRAME_SIZE: usize = 1024;
const TRAPFRAME_HOLD: usize = PAGE_SIZE / TRAPFRAME_SIZE;

impl ProcessCell {
    pub fn new(proc: Process, pid: Pid, parent: Pid, layout: ProcessLayout) -> Self {
        let mut mutable = proc;
        mutable
            .map(
                layout.trampoline >> PAGE_BITS,
                _user_trap as usize >> PAGE_BITS,
                1,
                MemoryRegionAttribute::Execute
                    | MemoryRegionAttribute::Write
                    | MemoryRegionAttribute::Read,
                true,
            )
            .expect("spawn process cell but no frame available for trampoline");
        Self {
            inner: mutable,
            id: pid,
            parent,
            layout: layout,
            head: None,
            head_lock: SimpleLock::new(),
            next: None,
            prev: None,
            ring_lock: SimpleLock::new(),
            state_lock: SimpleLock::new(),
        }
    }

    pub fn address_of_trapframe<E: PageTableEntry>(trampoline: Address, id: Tid) -> Address {
        // 最高页留给跳板，倒数第二个开始向下分配
        let start_page_number = (trampoline >> PAGE_BITS) - 1;
        let block = id as usize / TRAPFRAME_HOLD;
        let index = id as usize % TRAPFRAME_HOLD;
        ((start_page_number - block) << PAGE_BITS) + index * TRAPFRAME_SIZE
    }

    pub fn address_of_stack(stack: Address, id: Tid) -> Address {
        stack - (id as usize * THREAD_STACK_SIZE) - 1
    }

    pub fn move_next(&self, current: &Arc<Shared<ThreadCell>>) -> Option<Arc<Shared<ThreadCell>>> {
        current.ring_lock.lock();
        if let Some(next) = &current.next {
            unsafe { current.ring_lock.unlock() };
            Some(next.clone())
        } else {
            unsafe { current.ring_lock.unlock() };
            None
        }
    }

    pub fn _move_next_until(
        &self,
        current: &Arc<Shared<ThreadCell>>,
        pred: fn(&Arc<Shared<ThreadCell>>) -> bool,
    ) -> Option<Arc<Shared<ThreadCell>>> {
        let mut one = current.clone();
        while !pred(&one) {
            if let Some(next) = self.move_next(&one) {
                one = next
            } else {
                return None;
            }
        }
        Some(one)
    }

    pub fn _count_if_match_until(&self, pred: fn(&Arc<Shared<ThreadCell>>) -> bool) -> usize {
        self.head_lock.lock();
        let mut count = 0usize;
        if let Some(head) = &self.head {
            unsafe { self.head_lock.unlock() };
            let mut one = head.clone();
            while pred(&one) {
                count += 1;
                if let Some(next) = self.move_next(&one) {
                    one = next;
                } else {
                    break;
                }
            }
        } else {
            unsafe { self.head_lock.unlock() };
        }
        count
    }

    fn find_gap(&self) -> Option<Arc<Shared<ThreadCell>>> {
        self.head_lock.lock();
        if let Some(head) = &self.head {
            unsafe { self.head_lock.unlock() };
            let mut current = head.clone();
            while let Some(next) = self.move_next(&current) {
                if current.id + 1 == next.id {
                    current = next;
                } else {
                    break;
                }
            }
            Some(current)
        } else {
            unsafe { self.head_lock.unlock() };
            None
        }
    }

    pub fn add(&mut self, thread: Thread) -> Tid {
        let option = self.find_gap();
        let tid = if let Some(gap) = &option {
            gap.id + 1
        } else {
            0 as Tid
        };
        let generation = unsafe { &PROC_TABLE }.current_generation();
        let trapframe = Self::address_of_trapframe::<PageEntryImpl>(self.layout.trampoline, tid);
        let stack = ProcessCell::address_of_stack(self.layout.stack_point, tid);
        let entry = thread.entry_point;
        let mut cell = ThreadCell::new(thread, tid, generation, trapframe);
        self.ensure_page_created(
            trapframe >> PAGE_BITS,
            MemoryRegionAttribute::Write | MemoryRegionAttribute::Read,
            true,
        );
        self.struct_at::<TrapFrame>(trapframe).init(
            entry,
            stack,
            self.layout.trampoline,
            tid,
            [self.id as u64, self.parent as u64],
        );
        if let Some(gap) = &option {
            gap.ring_lock.lock();
            let last = gap.get_mut();
            let next = last.next.take();
            cell.next = next;
            last.next = Some(Arc::new(Shared::new(cell)));
            unsafe { gap.ring_lock.unlock() };
        } else {
            self.head_lock.lock();
            self.head = Some(Arc::new(Shared::new(cell)));
            unsafe { self.head_lock.unlock() };
        }
        tid
    }

    pub fn ensure_page_created<A: Into<FlagSet<MemoryRegionAttribute>> + Copy>(
        &mut self,
        number: PageNumber,
        attributes: A,
        reserved: bool,
    ) {
        self.state_lock.lock();
        self.inner
            .fill(number, 1, attributes, reserved)
            .expect("process memory for scheduling create failed");
        unsafe { self.state_lock.unlock() };
    }

    pub fn struct_at<'context, T: Sized>(&self, addr: Address) -> &'context mut T {
        let physical = self
            .inner
            .translate(addr)
            .expect("the page struct at has not been created");
        unsafe { &mut *(physical as *mut T) }
    }
}

struct ThreadCell {
    inner: Thread,
    id: Tid,
    generation: usize,
    last_tick_time: usize,
    timeslice: usize,
    trapframe: Address,
    next: Option<Arc<Shared<ThreadCell>>>,
    run_lock: SimpleLock,
    ring_lock: SimpleLock,
}

impl ThreadCell {
    pub fn new(inner: Thread, tid: Tid, initial_gen: usize, trapframe: Address) -> Self {
        Self {
            inner,
            id: tid,
            generation: initial_gen,
            last_tick_time: 0,
            timeslice: 0,
            trapframe,
            next: None,
            run_lock: SimpleLock::new(),
            ring_lock: SimpleLock::new(),
        }
    }

    pub fn check_grow(&mut self) -> bool {
        if self.timeslice < QUANTUM {
            true
        } else {
            self.timeslice = 0;
            let table = unsafe { &PROC_TABLE };
            if table
                .generation
                .fetch_max(self.generation, Ordering::Relaxed)
                == self.generation
            {
                self.generation += 1;
                true
            } else {
                false
            }
        }
    }

    pub fn grow(&mut self) {
        self.generation += 1;
    }
}

// 在调度开始时这个 head 必须有东西，因为“没有任何一个线程或所有线程全部失活时内核失去意义”
struct ProcessTable {
    generation: AtomicUsize,
    pid_generator: AtomicUsize,
    head: Option<Arc<Shared<ProcessCell>>>,
    head_lock: SimpleLock,
    last: Option<Weak<Shared<ProcessCell>>>,
    last_lock: SimpleLock,
}

impl ProcessTable {
    pub const fn new() -> Self {
        Self {
            generation: AtomicUsize::new(0),
            pid_generator: AtomicUsize::new(1),
            head: None,
            head_lock: SimpleLock::new(),
            last: None,
            last_lock: SimpleLock::new(),
        }
    }

    pub fn current_generation(&self) -> usize {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn new_pid(&self) -> Pid {
        self.pid_generator.fetch_add(1, Ordering::Relaxed) as Pid
    }

    pub fn add(&mut self, proc: Process, parent: Option<Pid>) -> Pid {
        let pid = self.new_pid();
        let parent_id = if let Some(parent) = parent {
            parent
        } else {
            pid
        };
        let layout = ProcessLayout::new(
            PageEntryImpl::top_address() & !0xFFF,
            proc.stack_point(),
            proc.break_point(),
        );
        let main = Thread::new(proc.entry_point());
        let mut cell = ProcessCell::new(proc, pid, parent_id, layout);
        cell.add(main);
        self.add_cell(cell);
        pid
    }

    fn add_cell(&mut self, mut cell: ProcessCell) {
        self.last_lock.lock();
        if let Some(last) = &self.last {
            let upgrade = last.upgrade().expect("last exists but pointers to null");
            upgrade.ring_lock.lock();
            let mutable = upgrade.get_mut();
            cell.prev = Some(last.clone());
            let sealed = Arc::new(Shared::new(cell));
            self.last = Some(Arc::downgrade(&sealed));
            unsafe { self.last_lock.unlock() };
            mutable.next = Some(sealed);
            unsafe { upgrade.ring_lock.unlock() };
        } else {
            // head & last both None
            self.head_lock.lock();
            let sealed = Arc::new(Shared::new(cell));
            self.last = Some(Arc::downgrade(&sealed));
            self.head = Some(sealed);
            unsafe {
                self.last_lock.unlock();
                self.head_lock.unlock();
            }
        }
    }

    pub fn move_next_process(
        &self,
        current: &Arc<Shared<ProcessCell>>,
        repeat: bool,
    ) -> Option<Arc<Shared<ProcessCell>>> {
        current.ring_lock.lock();
        if let Some(next) = &current.next {
            unsafe { current.ring_lock.unlock() };
            Some(next.clone())
        } else if repeat {
            unsafe { current.ring_lock.unlock() };
            self.head_lock.lock();
            if let Some(head) = &self.head {
                unsafe { self.head_lock.unlock() };
                Some(head.clone())
            } else {
                unsafe { self.head_lock.unlock() };
                None
            }
        } else {
            unsafe { current.ring_lock.unlock() };
            None
        }
    }

    pub fn move_next_process_until<F: Fn(&Arc<Shared<ProcessCell>>) -> bool>(
        &self,
        current: &Arc<Shared<ProcessCell>>,
        pred: F,
        repeat: bool,
    ) -> Option<Arc<Shared<ProcessCell>>> {
        let mut one_proc = current.clone();
        let start_proc = current.id;
        while !pred(&one_proc) {
            if let Some(next_proc) = self.move_next_process(&one_proc, true) {
                if !repeat && next_proc.id == start_proc {
                    return None;
                }
                one_proc = next_proc;
            } else {
                return None;
            }
        }
        Some(one_proc)
    }

    pub fn move_next_thread(
        &self,
        proc: &Arc<Shared<ProcessCell>>,
        thread: &Arc<Shared<ThreadCell>>,
    ) -> Option<(Arc<Shared<ProcessCell>>, Arc<Shared<ThreadCell>>)> {
        if let Some(next) = proc.move_next(thread) {
            Some((proc.clone(), next))
        } else {
            let mut next_proc_option = self.move_next_process(proc, true);
            while let Some(next_proc) = &next_proc_option {
                next_proc.head_lock.lock();
                if let Some(next_thread) = &next_proc.head {
                    unsafe { next_proc.head_lock.unlock() };
                    return Some((next_proc.clone(), next_thread.clone()));
                } else {
                    unsafe { next_proc.head_lock.unlock() };
                    next_proc_option = self.move_next_process(proc, true);
                }
            }
            None
        }
    }

    pub fn move_next_thread_until(
        &self,
        proc: &Arc<Shared<ProcessCell>>,
        thread: &Arc<Shared<ThreadCell>>,
        pred: fn(&Arc<Shared<ProcessCell>>, &Arc<Shared<ThreadCell>>) -> bool,
        repeat: bool,
    ) -> Option<(Arc<Shared<ProcessCell>>, Arc<Shared<ThreadCell>>)> {
        let mut one_proc = proc.clone();
        let mut one_thread = thread.clone();
        let start_proc = proc.id;
        let start_thread = thread.id;
        while !pred(&one_proc, &one_thread) {
            if let Some((next_proc, next_thread)) = self.move_next_thread(&one_proc, &one_thread) {
                if !repeat && next_proc.id == start_proc && next_thread.id == start_thread {
                    return None;
                }
                one_proc = next_proc;
                one_thread = next_thread;
            } else {
                return None;
            }
        }
        Some((one_proc, one_thread))
    }

    pub fn find_process(&self, pid: Pid) -> Option<Arc<Shared<ProcessCell>>> {
        let pred = |p: &Arc<Shared<ProcessCell>>| pid == p.id;
        self.head_lock.lock();
        if let Some(proc) = &self.head {
            unsafe { self.head_lock.unlock() };
            self.move_next_process_until(proc, pred, false)
        } else {
            unsafe { self.head_lock.unlock() };
            None
        }
    }
}

pub struct UnfairScheduler<T> {
    hartid: HartId,
    timer: T,
    current: Option<(Arc<Shared<ProcessCell>>, Arc<Shared<ThreadCell>>)>,
}

impl<T: Timer> UnfairScheduler<T> {
    pub const fn new(hartid: HartId, timer: T) -> Self {
        Self {
            hartid,
            timer,
            current: None,
        }
    }

    fn find_next(&self) -> Option<(Arc<Shared<ProcessCell>>, Arc<Shared<ThreadCell>>)> {
        let table = unsafe { &PROC_TABLE };
        let pred: fn(&Arc<Shared<ProcessCell>>, &Arc<Shared<ThreadCell>>) -> bool = |p, t| {
            let mut pass = false;
            p.state_lock.lock();
            if p.inner.health == ProcessHealth::Healthy {
                if t.inner.state == ExecutionState::Ready && t.run_lock.try_lock() {
                    let thread = t.get_mut();
                    // 如果是主线程，不在处理信号且有信号要处理则获得优先权无视代数判定（但会增加代数
                    if t.id == 0
                        && p.inner.signal.has_pending()
                        && !p.inner.signal.is_handling()
                        && p.inner.signal.has_handler()
                    {
                        let process = p.get_mut();
                        let trapframe = p.struct_at::<TrapFrame>(t.trapframe);
                        process.inner.signal.backup(trapframe);
                        trapframe.x[10] = process.inner.signal.dequeue();
                        trapframe.pc = p.inner.signal.handler().unwrap() as u64;
                        thread.grow();
                        thread.inner.state = ExecutionState::Running;
                        pass = true;
                    } else if thread.check_grow() {
                        thread.inner.state = ExecutionState::Running;
                        pass = true;
                    } else {
                        unsafe { t.run_lock.unlock() };
                    }
                }
            }
            unsafe { p.state_lock.unlock() };
            pass
        };
        if let Some((p, t)) = &self.current {
            table.move_next_thread_until(p, t, pred, false)
        } else {
            table.head_lock.lock();
            let mut next_proc_option = table.head.clone();
            unsafe { table.head_lock.unlock() };
            while let Some(next_proc) = next_proc_option {
                next_proc.head_lock.lock();
                if let Some(thread) = &next_proc.head {
                    unsafe { next_proc.head_lock.unlock() };
                    return table.move_next_thread_until(&next_proc, thread, pred, false);
                } else {
                    unsafe { next_proc.head_lock.unlock() };
                    next_proc_option = table.move_next_process(&next_proc, true);
                }
            }
            None
        }
    }
}

impl<T: Timer> Scheduler for UnfairScheduler<T> {
    type Context = UnfairContext;
    fn add(proc: Process, parent: Option<Pid>) -> Pid {
        let table = unsafe { &mut PROC_TABLE };
        let pid = table.add(proc, parent);
        hart::app::awake_idle();
        pid
    }

    fn find<F: FnMut(&mut Process)>(pid: Pid, mut action: F) -> bool {
        if let Some(p) = unsafe { &PROC_TABLE }.find_process(pid) {
            let owned = {
                if let HartKind::Application(hart) = hart::this_hart() {
                    hart.arranged_context().0 == pid
                } else {
                    false
                }
            };
            if !owned {
                p.state_lock.lock();
            }
            let mutable = p.get_mut();
            action(&mut mutable.inner);
            if !owned {
                unsafe { p.state_lock.unlock() };
            }
            true
        } else {
            false
        }
    }

    fn snapshot() -> Vec<Pid> {
        let table = unsafe { &PROC_TABLE };
        let mut ids = Vec::<Pid>::new();
        table.head_lock.lock();
        if let Some(head) = &table.head {
            unsafe { table.head_lock.unlock() };
            let mut current = Some(head.clone());
            while let Some(p) = current {
                ids.push(p.id);
                current = table.move_next_process(&p, false)
            }
        } else {
            unsafe { table.head_lock.unlock() };
        }
        ids
    }

    fn is_address_in(&self, addr: Address) -> Option<ProcessAddressRegion> {
        if let Some((p, _)) = &self.current {
            p.state_lock.lock();
            let result = p.layout.is_address_in(addr);
            unsafe { p.state_lock.unlock() };
            Some(result)
        } else {
            None
        }
    }

    fn schedule(&mut self) {
        // 采用 smooth 的代数算法，由于该算法存在进程间公平问题，干脆取消进程级别的公平比较，直接去保证线程公平，彻底放弃进程公平。
        if let Some((_, t)) = &self.current {
            let timeslice = if t.last_tick_time == 0 {
                0
            } else {
                self.timer.uptime() - t.last_tick_time
            };
            let thread = t.get_mut();
            thread.timeslice += timeslice;
            if t.inner.state == ExecutionState::Running {
                thread.inner.state = ExecutionState::Ready;
            }
            unsafe { t.run_lock.unlock() };
        }
        let next = self.find_next();
        if let Some((_, t)) = &next {
            let remaining = QUANTUM - t.timeslice;
            self.timer.schedule_next(remaining);
        }
        self.current = next;
    }

    fn cancel(&mut self) {
        self.timer.put_off();
    }

    fn context(&self) -> Option<(Pid, Address, usize, Address)> {
        if let Some((p, t)) = &self.current {
            let satp = p.inner.page_table_token();
            Some((p.id, p.layout.trampoline, satp, t.trapframe))
        } else {
            None
        }
    }

    fn with_context<F: FnMut(&mut Self::Context)>(&mut self, mut func: F) {
        let schedule_request: bool;
        if let Some((p, t)) = &self.current {
            p.state_lock.lock();
            let mut context = UnfairContext {
                hartid: self.hartid,
                process: p.clone(),
                thread: t.clone(),
                scheduled: false,
            };
            func(&mut context);
            if context.process.inner.signal.has_complete_uncleared() {
                let mutable = context.process.get_mut();
                let trapframe = mutable.struct_at::<TrapFrame>(t.trapframe);
                mutable.inner.signal.restore(trapframe);
                mutable.inner.signal.clear_complete();
            }
            schedule_request = context.scheduled;
            unsafe { p.state_lock.unlock() };
        } else {
            unreachable!("it's called only when a process requesting some system function")
        }
        if schedule_request {
            self.schedule();
        }
    }
}
