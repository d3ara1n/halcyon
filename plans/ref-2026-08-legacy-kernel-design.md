# 旧内核设计考古（27b8c98）

> 用途：重写时的「按需参考档案」，不是照抄蓝本；同时是逐模块重写 notes/ 的底稿。
> 证据来源：`git show 27b8c98:os/kernel/src/**` 第一手源码 + `artifacts/erhino_kernel` 反汇编实证（boot 契约）+ shared/ ABI（未被删除，即事实）。所有行级 bug/教训引用见 `plans/review-2026-07-code-review.md` 与 `plans/review-2026-08-mm-map-bug.md`，本文只记设计、不复述根因。
> 一句话总览：**这是一个「协作式内核 + 抢占式用户态」的设计**——内核态全程关中断、无内核线程切换，所有调度转折点都在用户 trap 返回路径上；多 hart 共享一个全局进程/线程环，各 hart 从环上抢线程。

---

## 0. 全局骨架

```
启动：OpenSBI → _start(hartid, dtb) → _awaken → rustc 生成的 main 包装器 → rust_start
     → early_init(sbi → dtb/board → frame → hart) → kernel_init(mm → fs) → main(initfs→进程注册)
     → start_all(HSM 唤醒 secondary) → enter_user → 调度循环
状态：全局 static mut（HARTS / PROC_TABLE / TUNNELS / ROOT / MOUNTPOINTS / FRAME_ALLOCATOR / KERNEL_UNIT / BOARD）
     每 hart 独立：ApplicationHart{ id, scheduler, random }（scheduler 内有 current 线程与独占的 CpuClock）
     共享调度结构：全局进程单链表环 + 每进程线程单链表环（跨 hart 抢）
地址空间（Sv39 用户半区 2^38 = 256GB）：
     [0, brk)              程序（ELF 段）
     [brk, 栈区底)          堆（extend 向上）
     [栈区底, stack_point)  每线程 8MB 栈（stack_point 向下）；stack_point = 2^38 - 256MB
     [stack_point=tunnel_point, 2^38)  隧道区 256MB（tunnel_point + slot*4KB）
     最高页（top_address）  trampoline（用户 trap 入口 + _restore + _switch_internal，恰好 1 页 ASSERT）
内核地址空间：mmio 直映射 [0, _memory_start) + SBI/内核直映射 [_memory_start, _memory_end) + 最高页 trampoline
```

---

## 1. boot 流程（assembly.asm + rt.rs + main.rs）

### ① 设计意图
从 QEMU/OpenSBI 的裸复位状态（a0=hartid, a1=dtb）走到「多核各自进入调度循环」的完整通路。关键决策：**复用 Rust 标准 `#[lang="start"]` 的调用规约当参数传递通道**——把 (hartid, dtb) 伪装成 (argc, argv)。

### ② 模块边界
- `assembly.asm`：`_start`（入口）、`_awaken`（通用 hart 启动装配）、`_park`（停放兜底）、`_kernel_trap`/`_user_trap`/`_restore`/`_switch`（trap 与切换，见 §2）、`_switch_internal`（trampoline 内）。
- `rt.rs`：`#[lang="start"] rust_start`（堆初始化 + early_init + kernel_init + main + SMP 启动 + 进用户态）、panic handler、`heap_rescue`（堆耗尽时从帧池取帧续堆）。
- `main.rs`：initfs tar 解析、/boot 目录树、`Process::from_elf` → `SchedulerImpl::add`。
- `external.rs`：extern "C" 声明链接脚本与汇编符号（`_kernel_end/_stack_size/_stack_start/_heap_start/_frame_start/_memory_start/_memory_end/_trampoline 相关/_kernel_trap/_user_trap/_park/_awaken/_switch`）。

### ③ 关键数据结构与不变量
- **入口契约（反汇编实证）**：`_start` 收到 a0=hartid、a1=dtb。`_start` 保存 a2=dtb、`la a1, main`、j `_awaken`。rustc 为 `#[lang="start"]` 自动生成符号 `main`（`0x8022217e`），其反汇编：
  ```
  mv a2, a1          # a2 = dtb
  sext.w a1, a0      # a1 = (i32)hartid —— 当 argc 用
  la a0, <真正的 user main>
  li a3, 0           # sigpipe
  jalr rust_start    # rust_start(user_main, argc=hartid, argv=dtb, sigpipe=0)
  ```
  于是 rust_start 里 `let dtb_addr = argv as usize` 拿到的就是 dtb。
- **`_awaken(hartid, fn, dtb)`**：`mv tp, a0`（**tp = hartid**，旧内核全生命周期不变）；`sp = _kernel_end - hartid * _stack_size`（per-hart 内核栈，linker 里 `STACK_SIZE × HART_NUM_LIMIT(8)` 一块块排）；`stvec = _kernel_trap`（Direct 模式）；`sstatus.FS = Initial`；`ra = fn; a1 = dtb; ret`。
- **rust_start 初始化顺序（谁先谁后、为什么）**：
  1. 堆 `HEAP_ALLOCATOR.init(_heap_start, _stack_start - _heap_start)`（buddy_system_allocator `LockedHeapWithRescue<32>`）——一切分配的前提。
  2. `early_init`：`sbi::init()`（探测 DBCN/TIME 扩展，AtomicBool 缓存）→ `DeviceTree::from_address(dtb)` → `board::init(tree)` → `frame::init(initfs.addr)`（**帧池 = [_frame_start, initfs tar 起始地址)**）→ `hart::init()`（构建 HARTS 全局 Vec，per-hart 只等 CPU 列表就绪）。
  3. `kernel_init`：`mm::init()`（建内核地址空间、KERNEL_SATP，依赖帧分配器）→ `fs::init()`（Rootfs + 挂 /proc）。
  4. `main()`：打印 banner/CPU/中断控制器信息；解析 initfs tar；建 `/boot`（PrivilegedWriteable）；每个 tar 项 `fs::create_memory_stream`（**内容零拷贝：Stream 节点只存物理地址+长度，直接引用 tar 内存**）；`bin/*` 文件 `Process::from_elf` → `SchedulerImpl::add(process, None)`。
  5. `hart::start_all()`：对每个 Stopped 的 Application hart `sbi::hart_start(id, _awaken, opaque=enter_user)`（secondary 从 OpenSBI WFI 停放中被 HSM 唤醒，进入 `_awaken` 后 a1=opaque=enter_user 直接进调度循环，a2 是垃圾值——不影响）。
  6. `hart::enter_user()`：本 hart（hart 0）走 `go_awaken` → 调度 → `_switch` 进第一个用户进程。
- **secondary hart 与 hart 0 的唯一区别**：hart 0 从 OpenSBI 经 `_start` 进入（带 dtb），secondary 经 HSM `_awaken` 进入（opaque=enter_user）；两者都装配同一套栈/trap/调度入口。
- **`_park`**：置 `sstatus.SIE=1` + `sie.SSIE=1` 后无限 `wfi` 循环。它是 HSM suspend 失败的兜底，但**被 IPI 唤醒会掉进 `_kernel_trap` → kernel_dump panic**（无 Software 中断分支）——这条路径实际是断的（review #8），可用路径是 HSM suspend/resume。

### ④ 与 notes 的对照
- notes/internals.md 的「tp = HartLocal 指针」是**重写新设计**；旧内核 tp 直接等于 hartid（整数），per-hart 状态靠 `HARTS: Vec<HartKind>` 全局表按索引取（每次 `&mut` 别名违规）。重写 notes 时需明确「tp 语义」是旧→新的关键差异。
- 「初始化顺序」notes 未覆盖（internals.md 只有状态分层）——补充：sbi → dtb → frame → hart → mm → fs → initfs 装载 → SMP 启动的依赖链值得写进 internals.md。
- initfs 装载（tar-no-std + `/boot` + Stream 零拷贝引用）在 notes 无对应篇，fs.md 只提 `/initfs`（dtb 引用）——注意 `/initfs`/`/devicetree`/`/src` 这些 fs.md 声称的内核文件**在旧代码里从未创建**（见 §7）。

---

## 2. trap.rs + 汇编 trap 路径

### ① 设计意图
两套 trap 帧分离：**内核 trap 帧**（内核栈上，512B，只存通用+条件浮点）服务内核态 trap（理论只有 timer）；**用户 TrapFrame**（trampoline 页内，552B，repr(C)）服务用户态 trap，同时是「内核与用户态共享的切换上下文」。

### ② 模块边界
- `trap.rs`：`TrapFrame` 结构、`SystemCallRequest`、`TrapCause` 枚举、`handle_kernel_trap`（汇编从 `_kernel_trap` 调）、`handle_user_trap`（汇编从 trampoline 调，返回 (satp, trapframe)）。
- `assembly.asm`：`_kernel_trap`（内核栈压栈/弹栈）、`_user_trap`（trampoline 内，用户现场→内核现场）、`_restore`（内核现场→用户现场）、`_switch`/`_switch_internal`（kernel→user 首次进入）。
- 依赖方向：trap.rs → hart.rs（`this_hart`/`HartKind`/`arranged_context`）、mm.rs（KERNEL_SATP）。

### ③ 关键数据结构与不变量（重写验证点）
- **内核 trap 帧布局（kernel stack 顶部，512B）**：`x[0..32]` @ 0..248（x2=sp @16、x4=tp @32 照存），浮点 `f[0..32]` @ 256..504 **仅当 `sstatus.FS==Dirty(3)`** 保存，存后把 FS 清为 Clean（避免下次重复保存）。参数 a0=scause、a1=stval。返回时先看 FS 是否又变 Dirty 决定是否恢复浮点，然后**恢复除 x2(sp)、x4(tp) 外的全部通用寄存器**，`addi sp,sp,512; sret`。注释明言「no need to restore sp, tp, they are always the same」——不变量：内核不迁移线程，sp=per-hart 栈、tp=hartid 在 S 态恒定。**这是重写的关键验证点：tp 必须保存进内核帧（x4@32）但故意不恢复**，新设计若引入内核线程迁移则此假设作废。
- **用户 TrapFrame（repr(C)，552B，trampoline 页内）**：

  | 偏移 | 字段 | 说明 |
  |---|---|---|
  | 0..255 | `x: [u64; 32]` | 含用户 tp（x4 @32） |
  | 256..511 | `f: [u64; 32]` | FS 条件保存 |
  | 512 | `pc` | sepc |
  | 520 | `kernel_tp` | **内核 tp（=hartid）**，`_restore` 每次写、`_user_trap` 读它定位内核栈 |
  | 528 | `kernel_satp` | 用户 trap 时切换用 |
  | 536 | `kernel_trap` | 即 `_kernel_trap` 地址 |
  | 544 | `user_trap` | 即 trampoline 地址（stvec 目标） |

- **`_user_trap` 流程（trampoline）**：`csrrw t6, sscratch, t6` 交换拿 trapframe 地址 → 存 32 通用寄存器（含用户 tp）→ FS 条件存浮点+清 Dirty → `sepc → 512` → `ld tp, 520`（拿 hartid）→ 用 `_stack_size_abs/_kernel_end_abs` 算内核栈 sp → `stvec ← 536` → `satp ← 528`（sfence×2）→ a0=scause, a1=stval → `jalr _handle_user_trap_abs`。
- **返回路径**：handle_user_trap 返回 (satp, trapframe) → 装回 satp → 落入 `_restore`：`sscratch ← trapframe`、`stvec ← 544`、`sepc ← 512`、**`sd tp, 520`**（存当前内核 tp）、FS 条件恢复浮点、恢复 32 通用寄存器（含用户 tp）→ sret。
- **`_switch(kernel_satp, trampoline, satp, trapframe)`**：先切内核 satp，把 `_switch_internal` 地址掩低 12 位加上 trampoline 基址跳到**映射在两空间同地址的 trampoline 页**执行；`_switch_internal`：置 `sstatus.SUM|SPIE`、清 `SPP`（回 U 态）、开 `sie.STIE|SSIE`、装用户 satp、再跳到 `_restore` 偏移。→ 用户态 SIE=1，timer/IPI 可打断。
- **分发**：`handle_kernel_trap` 只认 `SupervisorTimer → todo!("nested supervisor timer")`，其余 `kernel_dump()` panic（内核态 trap 一律视为致命，含 IPI 到停放 hart 的 SSI）。`handle_user_trap` 按 scause 分 `TimerInterrupt / Breakpoint / UserEnvCall / Load|Store|InstructionPageFault` 转 `hart.trap(TrapCause)`，未知 trap `unimplemented!` panic。
- **`TrapFrame::init`**：`x[2]=stack`、**`x[4]=tid`（用户态 tp 直接放线程号，TLS 语义未实现）**、`x[10/11]=[pid,parent]`（初始参数）、`pc=entry`、`kernel_satp=KERNEL_SATP`、`kernel_trap=_kernel_trap`、`user_trap=trampoline`。
- **`SystemCallRequest`**：借 trapframe；`extract_syscall` 取 a7（x17）为调用号、a0-a3（x10..13）为参数；`write_response` 写 `x[10]=0, x[11]=ret`；`write_error` 写 `x[10]=err, x[11]=0`——**ABI 恒为 a0=错误码、a1=返回值**。`move_next_instruction` 即 `pc += 4`。
- 设计点：内核态跑 syscall 时**始终在 KERNEL_SATP 上**，用户内存靠 `process.read/write → translate → 物理指针直写`（无 uaccess 机制、无用户页映射进内核空间）。trap.rs 注释里还记录了一个未解决场景：剩余时间片太短时进程回用户态立即再 trap，可能造成该 hart 串行性失效（设想方案：user_trap 时关 stie/seie，未实现）。

### ④ 与 notes 的对照
- internals.md「trap 进出与上下文切换必须保存/恢复 tp」在旧实现中的真实形态：用户 tp 作为 x[4] 普通寄存器保存/恢复；内核 tp 单独存 520 字段、由 _restore 写 / _user_trap 读。重写可照搬该布局。
- notes 未覆盖：内核 trap 帧「sp/tp 存而不恢复」的假设（内核不迁移线程）——若重写引入内核线程/迁移，必须重设计。
- call.md 的异步模型（Pending/Fed）与 trap 路径无关地未落地（见 §5）。

---

## 3. hart.rs + hart/app.rs

### ① 设计意图
hart 模块承载「每个应用核的完整执行状态」：自己的调度器实例（含当前线程与独占时钟）、随机数、idle 停放。**trap 分发与系统调用实现都集中在 app.rs**（`ApplicationHart::trap` → `handle_system_call`）。

### ② 模块边界
- `hart.rs`：`HARTS: static mut Vec<HartKind>`（按 hartid 索引，非 mmu 的 CPU 用 `Disabled` 占位）、`HartKind = Disabled | Application(ApplicationHart<...>)`、`init/start_all/enter_user/send_ipi/hartid/this_hart/get_hart`、类型别名（`SchedulerImpl = UnfairScheduler<CpuClock>`、`TimerImpl = CpuClock`、`RandomImpl = LcGenerator`）。
- `hart/app.rs`：`ApplicationHart<S, R> { id, scheduler: S, random: R }`；`trap()`（用户 trap 分发）、`handle_system_call()`（全部系统调用）、`arranged_context/go_awaken/go_idle`、`awake_idle()`（全局 IPI 唤醒 idle hart）。
- 依赖方向：app.rs → sched（Scheduler trait 方法）、fs、mm、task/ipc、sbi。

### ③ 关键数据结构与不变量
- **tp 与 hartid 关系**：`hartid() = 读 tp`；`this_hart() = get_hart(hartid())` → `&mut HARTS[id]`（static mut 别名，P0#6）。**旧内核没有 HartLocal 结构，per-hart 状态 = 全局表 + &mut**。
- `HartStatus`：Stopped/Suspended/Started（映射 `sbi::hart_get_status` 返回值 0|2|6/4/1|3）。
- **每 hart 的调度循环**：用户 trap → `handle_user_trap` → `hart.trap(cause)` → 处理完 → `arranged_context()`：`scheduler.context()` 返回 `(pid, trampoline, satp, trapframe)`，否则 `go_idle()`。返回的 trapframe 就是下一个线程的恢复现场——**一次 trap 处理完必然切到某个（可能是新的）线程现场 sret 回用户**。
- `go_idle()`：`scheduler.cancel()`（put_off 定时器）→ `IDLE_HARTS |= 1<<id` → `sbi::hart_suspend(0, _awaken, opaque=enter_user)` → 失败则 `_park()` 死等。被 `awake_idle()` 的 IPI 或新进程唤醒（add 时调用）后经 `_awaken → enter_user → go_awaken` 复活。
- `go_awaken()`：`scheduler.schedule()` → 有上下文则 `_switch`，无则 `go_idle`。
- **`handle_user_trap` 里 trap() 的返回路径**：`(_, satp, trampoline) = arranged_context()`——注意这里 `trampoline` 变量实际装的是**线程 trapframe 地址**（scheduler.context 第 4 元），命名误导，重写时改清晰。
- `trap(cause)` 分发：
  - TimerInterrupt → `scheduler.schedule()`（抢占点）。
  - Breakpoint → 进程标 `Dead(-0x114514)` + schedule（调试器用）。
  - PageFault(addr, op) → `scheduler.is_address_in(addr)`：Stack → `fill(页, R|W)` 补栈页；TrapFrame → `fill(reserved=true)` 补 trapframe 页；其余 `todo!()` panic。
  - EnvironmentCall → `with_context`：`extract_syscall` → `handle_system_call` → `Ok(Some) 写响应+PC+4` / `Err 写错误+PC+4` / `Ok(None) 不前进 PC、调度走人`。
- 死代码：`_send_ipi/_clear_ipi/_stop/_handle_remote_call`（Remote Call 概念留了骨架没接通，IPI 到停放 hart 会 panic）。

### ④ 与 notes 的对照
- call.md 的 Remote Call（ApplicationHart ↔ ApplicationHart，IPI，无返回值）——**只有 `_handle_remote_call` 空壳与 `_send_ipi` 死代码**，未实现。
- internals.md「hart 私有层」在旧内核的形态是「HartKind 持调度器实例」而不是 tp 直指 HartLocal——重写 notes 要写清这个演变。
- device 相关：旧内核没有 per-hart 设备中断（PLIC 只解析了地址，无中断使能/转发，见 §10）。

---

## 4. task/proc/thread/sched

### ① 设计意图
任务模型的落地：进程 = 资源容器（页表/内存/邮箱/信号/隧道槽/权限），线程 = 执行单元（entry/state/邮箱/调度字段）。调度器用「全局进程环 + 每进程线程环」的共享结构，多 hart 协作式从环上抢线程；公平性用 per-thread「代数（generation）」算法。

### ② 模块边界
- `task.rs`：仅 mod 声明。`sched.rs`：`Scheduler` trait（add/find/snapshot/is_address_in/schedule/cancel/context/with_context）+ `ScheduleContext` trait（pid/tid/process/thread/trapframe/add_proc/add_thread/schedule/find）。`sched/enough.rs`：**空文件**（「公平调度器」占位）。
- `thread.rs`：`Thread { entry_point, state: ExecutionState, mailbox: Mailbox }`（线程自带邮箱）。
- `proc.rs`：`Process` 结构 + ELF 装载 + 内存操作 + 隧道槽管理。
- `unfair.rs`：全部调度数据结构与算法。
- 依赖方向：app.rs/hart → Scheduler trait → unfair（PROC_TABLE）；procfs → Scheduler::find/snapshot；fs/ipc 被 syscall 层直接调用。

### ③ 关键数据结构与不变量
- **`PROC_TABLE: static mut ProcessTable`**：`{ generation: AtomicUsize, pid_generator: AtomicUsize(从1), head: Option<Arc<ProcessCell>>, head_lock, last: Option<Weak<ProcessCell>>, last_lock }`——全局进程**单链表**（next 强引用、prev 弱引用；遍历到尾部可 repeat 回绕 head）。
- **`ProcessCell`**：`{ inner: Process, id, parent, layout: ProcessLayout, head: Option<Arc<ThreadCell>>(线程环头), head_lock, next, prev, ring_lock, state_lock }`。
- **`ThreadCell`**：`{ inner: Thread, id: Tid, generation, last_tick_time, timeslice, trapframe: Address, next, run_lock, ring_lock }`。
- **`ProcessLayout`**：`{ trampoline, stack_point, break_point, thread_count }`；`is_address_in` 用 `MemoryUnit::is_address_in`（静态，按地址区间）判 User/Invalid/Kernel，Kernel 区用 `(trampoline - addr)/TRAPFRAME_SIZE` 反查 Tid（**无下溢保护**，addr>trampoline 时 wrap，review P1），User 区用 `(stack_point - addr - 1)/THREAD_STACK_SIZE` 判 Stack(线程) 或 Heap。
- **常量**：`QUANTUM = 20ms`（时间片）、`TRAPFRAME_SIZE = 1024`（每页 4 个，`TRAPFRAME_HOLD`）、`THREAD_STACK_SIZE = 8MB`、`TUNNEL_LIMIT = 65536`。
- **地址空间布局**：见 §0。trapframe 从 trampoline 页向下逐页分配（每页 4 个）；栈从 `stack_point` 向下每线程 8MB（初始 sp = `stack - id*8MB - 1`）。
- **进程创建 `ProcessTable::add`**：新 pid → `ProcessLayout::new(top_address & !0xFFF, proc.stack_point(), proc.break_point())` → 主线程 `Thread::new(entry)` → `ProcessCell::new`（**把 trampoline 页 map 进进程地址空间，R|W|X, reserved**）→ `cell.add(main)` → 入环。`ProcessCell::add`：`find_gap` 找线程 tid 空洞（并发创建复用槽）→ `address_of_trapframe/address_of_stack` 算位置 → `ensure_page_created`（trapframe 页，reserved）→ `struct_at::<TrapFrame>(...).init(entry, stack, trampoline, tid, [pid, parent])` → 插线程环。
- **`struct_at<T>(addr)`**：`translate(addr)` 得物理地址直接解引用——**内核在进程物理内存上原地读写结构体**（trapframe 就是进程地址空间里的一块）。
- **上下文切换路径（无内核栈切换）**：所有切换都发生在「用户 trap → 内核处理 → arranged_context → sret 到另一个现场」。内核态不阻塞、不切换；`schedule()` 只是记账+选下一个+设定时器+改 `self.current`。
- **`schedule()`**：当前线程 `timeslice += uptime - last_tick_time`；Running→Ready；`run_lock.unlock`；`find_next()`；新线程 `timer.schedule_next(QUANTUM - t.timeslice)`；`current = next`。`cancel()` = `timer.put_off()`。
- **⚠️ 核内抢占实际失效（重大考古发现）**：`last_tick_time` 在 `ThreadCell::new` 初始化为 0 后**从不被写入**（grep 证实只有 332/333 声明、346/347 初始化、692-698 读取三处），于是 `schedule()` 里 `timeslice = (last_tick_time == 0) ? 0 : ...` 恒为 0 → `thread.timeslice` 恒 0 → `check_grow()` 的 `timeslice < QUANTUM` 恒真 → 代数竞争永不触发。而 `find_next` 从 `self.current` 开始先判当前线程（此时它刚被置 Ready 且 run_lock 刚被释放）→ **pred 恒通过 → 每 hart 永远重选自己当前的线程**。后果：单核下第一个进程独占 CPU（= mm-map-bug「单核挂起」的直接根因：fs 阻塞后 pm/init 永不被调度）；4 核下各核靠 `run_lock.try_lock` 竞争不同的进程才「看起来能跑」。「unfair」名副其实——不是没写公平，是公平记账死了。重写必须修复记账写入点或换 per-cpu 队列。
- **`find_next` 的 pred**：进程 `health == Healthy` && 线程 `state == Ready` && `run_lock.try_lock()` 成功；随后**主线程（tid==0）信号优先**：有 pending + 有 handler + 未在 handling → `backup(trapframe)`、`x[10]=dequeue()`、`pc=handler`、`grow()`、Running（这是信号注入点）；否则 `check_grow()`（`timeslice < QUANTUM` 直接可跑；耗尽则清零并做**代数竞争**：`generation.fetch_max(self.generation) == self.generation` 才能 +1 获得下一轮——线程级公平，注释自承「彻底放弃进程公平」）。
- **`with_context(func)`**（syscall 上下文）：state_lock → 构造 UnfairContext → func → **若 `signal.has_complete_uncleared()` → restore(trapframe) + clear**（SignalReturn 完成路径）→ 若 `context.scheduled`（即 syscall 里调了 `schedule()`）→ `self.schedule()`。
- **锁体系（五把，无锁序文档，review P1）**：`head_lock`（进程表头）、`last_lock`（表尾）、`ring_lock`（每 cell 的链表互斥）、`state_lock`（每进程状态/内部互斥）、`run_lock`（每线程可运行性，try_lock 作选中标记）。全部 `SimpleLock`（不关中断）。
- **进程生命周期**：只有 `Healthy / Dead(ExitCode)` 两态；Dead 仅让 find_next 跳过，**对象与页帧永不回收**（review P1「进程回收缺失」）。`ExecutionState`（shared）：Ready/Running/Pending(Rid)/Fed(Rid)/Dead——调度器只认 Ready/Running，**Pending/Fed 是死枚举**。
- **`Process` 字段**：`memory: MemoryUnit, usage, entry_point, break_point, stack_point, tunnel_point, permissions, tunnels: Vec<Endpoint>, mailbox, health, signal`。
- **`Process::from_elf`**：elf_rs；校验 `machine==RISC_V && elftype==ET_EXEC`；LOAD 段 `fill(vpn, pages, attrs)` + `write(addr, content, memsz)`（write 越界补 0 → BSS）；`brk = next_power_of_two(段尾)`；`permissions = All`（**不检查、不继承**）。`usage.program/page` 记账。
- **内存操作**：`fill`（匿名）/`map`（固定）/`free` 均包 MemoryUnit + usage 记账；`extend` **要求 size 为 2 的幂**（否则 MisalignedAddress，语义错，mm-map-bug 注意 5），返回新堆末尾；`read/write` 逐页 translate 直拷；`has_permission` **定义但零调用**（权限系统空壳）。
- **隧道槽**：`tunnels: Vec<Endpoint>`（index 复用找空洞），`TUNNEL_LIMIT=65536` 上限。

### ④ 与 notes 的对照
- task.md「进程=资源容器、线程=执行容器」与实现一致（线程才有 state/mailbox，进程持资源）。
- task.md 的权限继承（子进程 ≤ 父）**未实现**：from_elf 直接 All、无任何 syscall 检查权限。
- call.md 的 Pending/Fed 与 Kernel Request **未实现**（见 §5）。
- task.md 内存布局图与实现吻合（程序/堆/栈/共享内存区——实现把共享内存区具体化为隧道区 256MB）。
- notes 未覆盖调度细节：全局单环 vs per-cpu 队列的取舍、「代数」公平算法的存在与「放弃进程公平」的注释——重写 notes 应记录这个反面教材。

---

## 5. syscall 面

### ① 设计意图
统一入口（a7=调用号）→ 内核分发 → (a0=err, a1=ret)。同步为主；异步的形态是「不前进 PC、重新调度、下次原样重进」。

### ② 模块边界
- 全部在 `hart/app.rs::ApplicationHart::handle_system_call`（静态方法），无注册表、无模块化：`match SystemCall` 枚举（`num_traits::FromPrimitive`，shared/call.rs 定义，编号 0x00-0x7b）。
- 调用方：`trap()` 的 EnvironmentCall 分支（with_context 内）。
- ABI 常量：`SystemCallError`（shared，0x00-0x34）。

### ③ 关键数据结构与不变量
- **已实现 19 个**：Die(0x0 杀内核)、Debug(0x1 读用户内存打印)、Exit(0x10)、Extend(0x50)、ThreadSpawn(0x22)、TunnelBuild/Link/Dispose(0x60-62)、SignalSet/Send/Return(0x32/31/30)、Access/Inspect/Create/Read/Write(0x70-73,78,79)、Send/Peek/Receive(0x40/41/43)。
- **未实现（match 兜底 `unimplemented!()` panic，用户可触发内核 panic）**：ExecuteBytes/File、ThreadExit/Yield/Join/Kill、Discard、Map、Free、TunnelRequest/Response、Modify/Delete/Copy/Move/Open、Mount/Unmount。
- **同步/异步机制**：`Ok(Some(ret))` → `write_response + move_next_instruction`（PC+4，完成）；`Err(err)` → `write_error + move_next`；**`Ok(None)` → `ctx.schedule()`（不前进 PC）**——旧内核的「异步」= 调度走人、PC 原地、下次运行同一现场重进同一次 ecall。当前只有 `Exit` 用 Ok(None)（进程已死无所谓）。**Kernel Request / Pending(Rid) / Fed(Rid) 状态机从未实现**——ExecutionState 枚举里有但调度器只认 Ready/Running，app.rs 从不写。
- **用户内存访问**：`process.read/write`（translate 直拷物理内存，见 §4）。FAL 类调用（Access/Inspect/Create/Read/Write）共用「path 两参（ptr+len）+ 可选 buffer」模式，样板代码逐字复制 5 份（review P2/P3），错误映射 `FilesystemAbstractLayerError → SystemCallError` 手写复制、`ForeignMountPoint => todo!()`。
- **错误路径**：`write_error` 只写 a0；错误码进 trapframe，用户态由 rinlib 读。系统调用内部失败多用 `Err(...)`，但 `process.write` 内部失败直接 `.expect("kill process if failed")`——**注释说是杀进程，实现是 panic 杀整机**（review P1 #5）。
- `Inspect` 的 buffer 序列化：`DentryObject` + name，按 8 对齐填充，返回值=写入对象数（遍历子项直到 buffer 满）。

### ④ 与 notes 的对照
- call.md 的同步/异步分类在实现里只有「同步 + 重试」两种真实形态；Pending/Fed 是纯设计。**重写 notes 需明确：异步 = Kernel Request 队列 + Pending/Fed 状态机，旧实现的「重调度重试」是过渡形态、已证不可行**（单核挂起与此相关，mm-map-bug 附带发现）。
- call.md 的 Remote Call 未实现（§3）。
- 系统调用全集与实现子集的差距（7 类未实现）应记入 notes/call.md 的「实现状态」节。

---

## 6. ipc（message / tunnel / signal）

### ① 设计意图
三种 IPC 按 notes/ipc.md 落地：邮箱（消息）、隧道（共享页）、信号（抢占无负载匿名）。旧实现全部是**内核侧原语**，Runnel 等流式协议留给用户态约定。

### ② 模块边界
- `task/ipc/{message,signal,tunnel}.rs`：纯数据结构与操作，无系统调用逻辑（syscall 在 app.rs）。
- `TUNNELS: Mutex<SimpleLock, Vec<Tunnel>>`（app.rs，lock_api Mutex 包 SimpleLock）——**全局隧道表，非 per-process**。
- `Endpoint`（proc.rs 的隧道槽记录）。
- shared：`MessageDigest`（#[repr(C)] {sender, kind, time, payload_length}）、`SignalMap = u64`、`SystemSignal { Terminate=1<<0, Notify=1<<1 }`。

### ③ 关键数据结构与不变量
- **Message/Mailbox**：`Message { sender, kind, time(恒0), content: Vec<u8> }`；`Mailbox { inbox: Option<Message> }`——**单槽非阻塞**，`put` 满返回 false。操作链：`Send` → 目标进程的 `process.mailbox`；`Peek` → 若线程槽空闲（`thread.mailbox.available()`）则从 `process.mailbox.take()` 到 `thread.mailbox` 并写 digest（**偷走占位**，Peek 后只有该线程能 Receive）；`Receive` → `thread.mailbox.take()`，**长度必须精确相等**否则 IllegalArgument；`Discard` 未实现。空邮箱 Peek/Receive → ObjectNotAvailable；线程槽被占 → ReachLimit。
- **Tunnel**：`Tunnel { key, owner: Pid, first: Option<(Pid, PageNumber)>, second: Option<(Pid, PageNumber)>, frame: FrameTracker }`——一个隧道 = 一个共享物理帧 + 两个端点。`key` 为 48bit LCG 随机数（防碰撞）。`Build`：`frame::borrow(1)` → 全局表登记 → 返回 key；`Link`：进程隧道槽 `tunnel_insert(key)` → `tunnel.link(pid, addr>>PAGE_BITS)` → `process.map(隧道页, R|W)` → 返回映射地址；`Dispose`：`unlink` → 双端点都空则删隧道 + free 帧 + `tunnel_eject` 槽位回收。**link 鉴权失效**：第二端点要求 `pid == owner || first == owner`——`first == owner` 恒真（link 时 owner=first.pid），任意进程可挂第二端点（review P2）。**Runnel 协议与 TunnelRequest/Response 中断未实现**（shared 有 syscall 编号，app.rs 无分支）。
- **Signal**：`SignalControlBlock`（**进程内字段，非全局**）`{ x[32], f[32], pc, mask, pending, handling, complete, handler: Option<Address> }`。`set_handler(mask, addr)`、`enqueue`（位或 pending）、`is_accepted(signal & mask != 0)`、`dequeue`（逐位扫描取最低位，review P3 建议 trailing_zeros）、`backup(trapframe)`/`restore(trapframe)`（整帧复制）。
  - **注入路径（唯一）**：调度器 find_next 主线程分支（§4）——backup 现场 → `x[10]=signal`、`pc=handler` → 用户态跑 handler。
  - **返回路径**：用户 `SignalReturn` → `is_handling` 检查 → `complete()`（置 complete 标志）→ **with_context 尾部**发现 `has_complete_uncleared` → `restore(trapframe)` + clear。
  - 限制：只注入主线程（tid==0）；内核自身不发信号（只有进程间 SignalSend）；信号不打断执行流、只在调度选择点生效（notes「抢占」语义打折）。

### ④ 与 notes 的对照
- message.md 的 Send/Peek/Discard/Receive 与实现一致，但「阻塞直到收到」的注释 vs 单槽非阻塞实现矛盾（review P2，以 call.rs 注释「block until received」为准是文档漂移）；「Pid=0 发给内核」未实现（find(0) 找不到进程）。
- tunnel.md 的 Runnel（1k×3 缓冲 + 1k 控制块）是**用户态协议**，内核只提供「key 标记随机共享页」——notes 应明确这个分工与「内核不参与则接收端盲等」的缺陷记录。
- signal.md「内核通过信号发中断请求和进程操作」——旧实现只有进程间信号；设备中断→信号（device.md 的设备租借）未实现。

---

## 7. fs / FAL

### ① 设计意图
FAL（Filesystem Abstract Layer）：统一内核文件系统与外部文件系统服务的抽象。内核侧 = Rootfs（内存树）+ 挂载点 + 本地转发；用户态文件系统服务 = 远程挂载，设计上经消息转发。**旧内核只走通「本地挂载」半边，远程转发是整条链路的空白**。

### ② 模块边界
- shared/fal.rs：ABI——`FileSystem` trait（**只有 6 方法**：is_property_supported / is_stream_supported / lookup / create / read / write，没有 delete/copy/move/open）、`Dentry`、`DentryMeta`（Directory(Vec<Dentry>)/Link/File(FileKind)/MountPoint(Mid)）、`DentryType`（repr(u8)）、`DentryObject`（#[repr(C)] 线格式，**6 字节 padding 未初始化 + 原生端序**，review P2）、`DentryAttribute`（R/W/X + Privileged 三件套，特权位存在即要求特权）、`FilesystemAbstractLayerError`（含 `ForeignMountPoint(Path, Mid)`）。
- `fs.rs`：`ROOT: static mut OnceCell<Rootfs>`、`MOUNTPOINTS: static mut Vec<LocalMountpoint>`（注释：init 后只读不上锁）、`LocalMountpoint = Proc(Procfs)`（单变体）、`Mid = (index+1)<<32`（本地）或 pid（远程）、`redirect_with`（本地转发核心）、`lookup/create/write/read/measure/make_objects/make_directory`、`mount_local/mount_remote`。
- `rootfs.rs`：内存文件系统实现。`procfs.rs`：动态进程信息。`sysfs.rs`、`fal.rs`、`device.rs`：**空文件**（模块声明，review P3）。

### ③ 关键数据结构与不变量
- **本地挂载转发机制**：`redirect_with(op, fs, path)`：`op(fs, path)` 出错且为 `ForeignMountPoint(rem, mid)` → `get_local_fs(mid)` 命中本地 → **递归 `redirect_with(op, 本地fs, rem)`**；未命中（远程服务）→ 原样返回 → **syscall 层 `ForeignMountPoint => todo!()`**。所以：`/proc` 查询可达（本地直达），远程文件系统服务完全未接入（`Mount/Unmount` syscall 无调用方、转发消息协议在 shared 从未定义、`MountPoint` 无 Attributes 检查）。
- **fs::init 只做了**：`Rootfs::new()` + `rootfs.mount("/proc", 1<<32)` + `MOUNTPOINTS.push(Proc)`。**notes/fs.md 声称的 `/sys`、`/initfs`、`/devicetree`、`/src` 从未创建**。
- **Rootfs**：`Node = UpSafeCell<LocalDentry>`；`LocalDentry { name, created, modified, kind, attr }`；`LocalDentryKind { Directory(UpSafeCell<Vec<Node>>, SimpleLock), Link(Path), File(LocalFile), MountPoint(Mid) }`；`LocalFile { Stream(Address, usize), Property(LocalProperty) }`——**Stream 是外部内存指针**（initfs tar 内容零拷贝引用）；`LocalProperty` 7 类型（Boolean/Integer/Integers/Decimal/Decimals/String/Blob）。
  - `find_node_internal`：按 Component::Normal 逐级找；**遇 MountPoint → `collect_remaining + prepend + make_root` 把剩余路径转绝对路径 → ForeignMountPoint(rem, mid)**（满足 fal.md「内核保证发给 fs 实现的路径为绝对路径」）。
  - `FileSystem for Rootfs`：create 只支持 Directory + 6 种属性（**Stream 需专用 create_stream**）；read 属性 `to_bytes(length)`（截断语义各类型不一）、Stream 从物理地址 `from_raw_parts` 拷 `min(length, len)`；write 按类型校验长度 + `replace`（**不更新 modified_at**，恒 0）；`Blob` 的 meta size 报 `8*len`（疑似笔误）。
  - **特权属性（Privileged*）只当元数据存，从不检查权限**（`has_permission` 零调用）。
- **Procfs**：`FsLayer` 枚举路径解析：`/proc`（Root 列所有 pid，经 `SchedulerImpl::snapshot`）、`/proc/{pid}`、`/proc/{pid}/pid`、`/proc/{pid}/memory/{page,program,heap,stack}`（经 `SchedulerImpl::find(pid)` 读 `usage` 字段转 i64 字节）。**read 忽略 length 恒返 8 字节**（与 rootfs 尊重 length 的契约不一致，review P1）；create/write Unsupported；注释里的 `/proc/{pid}/traits/` 未实现。
- **路径**：`Path { inner: String }`、`Component { Root, Current, Parent, Normal }`；`Path::from` 只拒 `\0` **允许 ./..**（与 fal.md 禁止冲突，review P2）；`is_qualified` 用 `contains(".")` 子串判断（`file.txt` 误判）；约 40% 死代码为 ./.. 服务（review P3）。

### ④ 与 notes 的对照
- fal.md 的抽象（Access/Inspect/Modify + Create/Delete/Move/Copy + Stream/Property + 绝对路径无 ./..）与实现子集差距大：Modify/Delete/Copy/Move/Open 未实现；挂载点转发（本地已通、远程空白）；「属性最大 512 字节」未落地；DentryObject 的 `in_use` 字段已从代码删除（文档漂移）。
- fs.md 声称的内核文件系统表只有 Rootfs+`/proc` 落地；`/sys`、`/initfs`、`/devicetree` 是设计。挂载点语义注释（对挂载点 Access/Inspect 不受权限影响、Delete 会转发到挂载文件系统根）是**代码里的设计意图**，值得搬进 notes。
- service.md 的 `/src/{identifier} → /proc/{pid}` 服务注册机制未实现（/src 不存在、traits 不存在）。
- device.md 的 `/sys/dev/` 设备列表未实现（见 §10）。

---

## 8. mm（frame / page / unit / usage）

### ① 设计意图
四层分包：帧分配（buddy）→ 单表页表操作 → 进程地址空间（MemoryUnit）→ 门面/记账。目标是 per-process 页表隔离 + 内核直映射 + trampoline 高位映射。**只记分工，bug 根因见 mm-map-bug.md，不重复**。

### ② 模块边界（严格单向）
```
frame（帧池）← page（单表+帧账本）← unit（地址空间递归映射）← proc / mm 门面
usage（页计数）旁挂 Process，无依赖
```
- `frame.rs`：帧池薄壳。`page.rs`：PTE 编码 + 单张表操作。`unit.rs`：MemoryUnit（页表根 + 递归映射 + 翻译 + 空间判定）。`usage.rs`：`MemoryUsage { page, program, heap, stack }` 四计数器（procfs 数据源）。`mm.rs`：门面 + KERNEL_UNIT/KERNEL_SATP + ProcessAddressRegion + init。

### ③ 关键数据结构与不变量
- **frame.rs**：`static mut FRAME_ALLOCATOR: OnceCell<LockedFrameAllocator<32>>`（buddy 0.13，32 级，自带 spin）；`init(end_addr)` 帧池 = `[_frame_start, end_addr)`（end = initfs 地址）；`alloc(count)` 连续帧 + **整块清零**；`borrow → FrameTracker { number, count }` RAII（Drop → dealloc）；`add_frame` 供 DTB 多段内存追加。
- **page.rs**：`PAGE_SIZE=4096, PAGE_BITS=12`；`PageEntryImpl = PageTableEntry39`（Sv39 三级 9 位，`PageTableEntryPrimitive<u64, 56, 3, 9>`）；`PageTable<E> { location, entries: &'static mut [E], branches: HashMap<usize, PageTable<E>>, managed: HashMap<usize, FrameTracker> }`——**branches 记子表视图、managed 记子表物理帧**；`PageEntryFlag`：V/R/W/X/U/G/A/D + **Cow/CowWriteable（定义未用，死代码）** + `Prefab*` 组合（**内核页恒 AD=1**，注释：有些实现要求访问前 AD=1）；`PageEntryType { Invalid, Leaf(ppn, flags), Branch(&PageTable) }`；`ensure_leaf_created`（固定物理）/`ensure_managed_leaf_created`（闭包取帧+记账，**遇已存在 leaf 静默 set_flags 改权限**——bug 链条第一环）/`split_page_into_table`（大页分裂，**level 0 非法**）。
- **unit.rs**：`MemoryUnit<E> { identity: u16(ASID), root: PageTable<E>, where_the_frame_tracker_of_root_for_recycling_put: FrameTracker }`（根帧 RAII，Drop 归还）；`satp() = (mode_code<<60) | (identity<<44) | root_ppn`；`is_address_in`：`addr < 2^38`=User、`top-2^38 < addr <= top`=Kernel、否则 Invalid；`fill`（匿名）/`map`（固定 ppn）/`free`/`translate`。
  - **映射算法（bug 温床）**：`map_internal(container, vpn, ppn, count, flags, level)` 三路——level 0 叶表循环 / 对齐大页（`is_page_number_aligned_to` 通过则 2MiB 大页或整表下放）/ 未对齐降级；`free_internal` 对称。**未对齐分支的容器/级别传参 copy-paste 错位 = mm-map-bug 根因**。结构缺陷：把「当前表、当前级别、剩余数量」耦合在递归参数里，三路写法高度相似。
- **mm.rs**：`KERNEL_UNIT: static mut OnceCell<MemoryUnit>`、`KERNEL_SATP: AtomicUsize`（`#[export_name="_kernel_satp"]` 供汇编）；`ProcessAddressRegion { Invalid, Unknown, Program, Heap, Stack(Tid), TrapFrame(Tid) }`；`init()`：identity=0 单元 → `map(0x0, 0x0, memory_start, PrefabKernelDevice)`（mmio 直映射）→ `map(memory_start, memory_start, memory_end-memory_start, PrefabKernelProgram)`（SBI+内核）→ `map(top, _user_trap, 1, PrefabKernelTrampoline)`（最高页）→ `KERNEL_UNIT.set` + `KERNEL_SATP.store`。**内核无 trapframe 映射**（注释：kernel has no trap frame）。
- **大页**：2MiB（L1）支持 + 分裂；`fill/map/free` 返回页数用于 usage 记账。
- **锁粒度**：帧池全局单锁；per-process 页表无锁（假设单 hart 访问——4 核共享进程即 UB，P0#4）。

### ④ 与 notes 的对照
- notes 无 mm 专题篇；internals.md 的全局层（帧分配器）描述与实现一致，但「对象私有层」在旧实现里是**无锁 + 多 hart 别名 UB**，不是安全的锁+Arc——重写 notes 需把 mm 的分层、大页、AD 约定、trampoline 高位映射写清楚（建议新增 mm 篇或并入 internals.md）。
- `MemoryUnit` 的 ASID 字段（identity）在旧实现恒 0（未用 ASID 功能，satp 里只占位）——重写可复用或删除。

---

## 9. sync / up.rs

### ① 设计意图
旧内核的同步工具箱：**没有中断纪律**。UpSafeCell 用于「单持有者/单核独占」数据；SimpleLock 用于一切需要互斥的地方（不关中断）；五把调度锁、console 锁都是它。

### ② 模块边界
- `sync.rs`：mod 声明。`sync/up.rs`：`UpSafeCell<T>`。
- shared/sync/spin.rs：`SimpleLock`（AtomicBool CAS + spin_loop，lock_api RawMutex）、`QueueLock`（MCL 队列锁，内核未使用）。
- 使用面：`Arc<UpSafeCell<ProcessCell/ThreadCell>>`（unfair.rs）、rootfs `LocalDentry` 与目录子表、TUNNELS 与 console 的 `Mutex<SimpleLock, T>`。

### ③ 关键数据结构与不变量
- **`UpSafeCell<T>`**：`UnsafeCell` + `get/get_mut` 无锁直取 + Deref/DerefMut + `unsafe impl Sync`——注释层面的契约是「访问者自行保证单核/单持有者」，**实际被 4 核共享调度结构滥用**（`get_mut` 多核并发 = 别名 UB，P0#4）。
- **SimpleLock**：CAS 自旋，`lock`/`unlock`（RawMutex）/`try_lock`；**不关本地中断**。console 注释自承：「SpinLock is causing deadlock while trap」——panic 路径 println 与持锁 hart 交叉死锁的隐患（review P1）。
- 内核里**没有睡眠锁、没有 per-hart 锁、没有关中断的锁原语**；中断屏蔽靠硬件自动（trap 进入 S 态即 SIE=0）——所以内核态代码天然不可抢占，锁临界区不会被同核中断打断，**跨核才有争用**（这解释为什么 SimpleLock 不关中断也能跑——但 panic/console 例外）。
- notes/internals.md 的「Spinlock 持有期间关 SIE」是**重写新设计**（正确性要求：中断处理函数若获取本 hart 正持有的锁会死锁）。

### ④ 与 notes 的对照
- internals.md 的锁原语章节（LR/SC + 关 SIE + lock_api 注入 talc/全局容器）描述的是重写目标，旧实现是 SimpleLock/UpSafeCell 的无纪律版本——重写 notes 应加一节「旧实现为什么不够」或直接以新设计为准。
- 「睡眠锁在任务模型落地后再引入」——与旧实现一致（没有可睡眠上下文）。

---

## 10. board / console / sbi / timer / device

### ① 设计意图
板级抽象把 dtb 解析成内核可用的静态信息（CPU 列表/中断控制器地址/initfs 位置），外设驱动留给用户态（设备租借理念的雏形，但内核侧只有骨架）。

### ② 模块边界
- `board.rs`：`BOARD: static mut OnceCell<BoardInfo>`、`BoardInfo { tree, map: DeviceMap, initfs: Option<(Address, usize)> }`、`from_device_tree` 解析、`this_board()`。
- `board/device.rs`：`DeviceMap { cpus: Vec<Cpu>, intrc: InterruptController }` + `DeviceMapBuilder`；`cpu.rs`：`Cpu { id, frequency, mmu }` + `MmuType`；`intrc.rs`：`InterruptController { address, size }`（仅 PLIC 地址）；`generic.rs`：`GenericDevice` 扁平设备描述（**stub，未接入**）；`memory.rs`：**空**。
- `sbi.rs`：SBI 全扩展封装。`console.rs`：输出宏 + Console。`timer.rs`：Timer trait + CpuClock。`rng.rs`：RandomGenerator + LcGenerator。
- dtb_parser 子模块（硬依赖）：`DeviceTree::from_address`（magic 0xd0d0feed）。

### ③ 关键数据结构与不变量
- **board 解析**：`/chosen/initfs` 的 `reg` → initfs (addr, len)；`/cpus/timebase-frequency` + 每个 cpu 节点（`reg`=hartid、`mmu-type` 字符串、`clock-frequency` 兜底 timebase、**freq != 0 才收**）→ Cpu；兼容 `riscv,plic0` 的中断控制器 + `reg` → intrc；`build()` 要求 intrc 存在。**MmuType::Bare 的 CPU 被 hart::init 过滤**（sifive_u cpu0 无 mmu-type 且 disabled）。
- **sbi**：legacy（set_timer/putchar）+ Base + Time + IPI + HSM（hart_start/stop/get_status/suspend）+ SystemReset + DBCN；`init()` 探测 DBCN/TIME（AtomicBool 缓存）；`is_time_supported` 决定 set_timer 走 TIME 扩展还是 legacy。
- **console**：`LOCKED_CONSOLE: Mutex<SimpleLock, Console>`；Console::write_str → DBCN（`debug_console_write`）或逐字符 legacy putchar；宏 print/println/debug（仅 debug_assertions）/info/warning/error。**panic 路径与 console 锁交叉 = 潜在死锁**（注释自承）。
- **timer**：`Timer { uptime(), schedule_next(ms), put_off() }`；`CpuClock { frequency, uptime }`；`schedule_next(ms)`：`time = mtime`，`uptime = time*1000/freq`（毫秒），`set_timer(time + ms*freq/1000)`；`put_off` 设 `usize::MAX - 1`（注释：MAX 会 +1 溢出成 0 直接触发，**防溢出处理**）。CpuClock 是调度器独占资源（每 hart 一个）。
- **rng**：`RandomGenerator { next() }`；`LcGenerator`（Java Random 常数 25214903917 + 11，48bit 掩码，wrapping_mul 防溢出）；**种子 = `time::read()`**（真实 mtime，避免全核同种子——旧版用 timer.uptime() 在 init 时为 0）。
- **device（设备租借）**：**只有 `GenericDevice` 描述结构 + Builder 注释（Handle/依赖排序方案）+ intrc 地址**；`/sys/dev/` 列表、寄存器映射给进程、中断转发到信号——全部未实现。

### ④ 与 notes 的对照
- device.md 的设备租借（生设备列表 /sys/dev、寄存器映射、中断转发信号、驱动向其他进程暴露设备）——**内核侧完全未实现**，只有 GenericDevice 的 stub 与 PLIC 地址解析。重写 notes 应明确这是「设计目标」而非「已有能力」。
- notes 无 sbi/console/timer 篇（纯机制）；`debug_console`/TIME 扩展探测与回退策略、put_off 防溢出、LCG 种子来源等实测细节值得写进 internals.md 的机制节。

---

## 附 A：notes 重写建议清单（每篇一行）

| notes 篇 | 建议 |
|---|---|
| task.md | 保留（进程=资源容器/线程=执行容器与实现一致）；补充地址空间布局图（含 256MB 隧道区）、说明权限继承在旧实现是空壳 |
| ipc.md | 保留三分类；补充隧道中断（TunnelRequest/Response）未实现、远程挂载转发未实现 |
| message.md | 保留 Send/Peek/Discard/Receive 语义；标注单槽非阻塞 vs 旧注释「block until received」矛盾、Pid=0 内核通道未实现；重写若改队列需先定语义 |
| call.md | 需大改：真实分发结构（无注册表直接 match）、异步现状（旧实现=重调度重试，Pending/Fed 未落地，已证单核挂起相关）、错误码 ABI（a0=err, a1=ret）、未实现 syscall 清单 |
| signal.md | 保留抢占无负载匿名；补充注入点（仅主线程、调度选择点）与返回机制（SignalReturn → with_context 尾部 restore）；内核→进程信号未实现 |
| tunnel.md | 保留 Runnel 描述；明确内核只提供「key 标记随机共享页」原语、Runnel 与中断是用户态约定且未实现；保留「接收端盲等/死锁」缺陷记录 |
| framework.md | 保留原样（libsrv/libfal/libdrv 用户态蓝图，内核无涉） |
| service.md | 保留概念；标注 /src 目录与 /proc/{pid}/traits 均未实现，服务注册机制是空白 |
| device.md | 需补充：/sys/dev、设备租借、寄存器映射+中断转发全部未实现；旧内核只解析 PLIC 地址；GenericDevice 是 stub |
| fs.md | 需修正：实际只有 /proc 落地；/sys、/initfs、/devicetree、/src 未创建；挂载仅 Rootfs 支持、本地转发已通、远程转发空白 |
| fal.md | 保留抽象（Access/Inspect/Modify/Create/Delete/Move/Copy + Stream/Property + 绝对路径禁 ./..）；标注实现只覆盖 access/inspect/create/read/write、ForeignMountPoint 在 syscall 层 todo、DentryObject 线格式需重设计（padding/端序）、特权位检查未实现 |
| ecs.md | 保留原样（用户态服务蓝图，与内核无关） |
| internals.md | 保留作为重写目标文档；补一节「旧实现对照」：旧 tp=hartid 非 HartLocal、锁无中断纪律、UpSafeCell/static mut 滥用、全局表取 per-hart 状态 |

## 附 B：重写决策清单

### 可直接复用（经过实践验证）
1. **trampoline + 双 trap 帧整套结构**（552B TrapFrame 布局、sscratch 交换、FS 条件浮点、stvec/satp 切换顺序、`_switch`/`_restore` 偏移跳转）——QEMU 实测跑通，重写直接照搬；保留「内核帧存 sp/tp 而不恢复」的注释，但要重新审视其前提（内核不迁移线程）。
2. **boot 契约**：`_start(hartid, dtb)` + rustc `main` 包装器（argc=hartid, argv=dtb）的复用套路——反汇编实证可行，重写可沿用或显式传参。
3. **per-hart 内核栈**（`_kernel_end - hartid*_stack_size`）+ HSM start/suspend + `_awaken` 入口复用——secondary 进用户态的路径已验证；`_park` 兜底路径要修（IPI 唤醒会 panic）或删。
4. **帧分配器**：buddy_system_allocator + `FrameTracker` RAII + 帧池 `[_frame_start, initfs)` + `add_frame` 多段——简单可靠。
5. **Sv39 三级页表 + 2MiB 大页 + 分裂**的基本能力——保留能力，换算法骨架（见下）。
6. **Rootfs 树 + LocalDentry 元数据模型**（含 7 种属性类型、Stream=外部内存指针的零拷贝引用）——与 fal.md 匹配，重写可复用；补属性值 512B 上限与修改时间戳。
7. **本地挂载转发**（`redirect_with` + `Mid=(index+1)<<32` + 剩余路径转绝对路径再下发）——机制正确，可作为远程挂载转发（消息打包）的骨架。
8. **Procfs 的 FsLayer 路径解析模式**（动态生成 Dentry 树 + `Scheduler::find/snapshot` 读实时数据）——复用；修 read 忽略 length。
9. **MessageDigest + 单槽 Mailbox + Peek 预留到线程区**的流程设计——思路可保留，重写若改多槽队列则同步改 rinlib。
10. **SignalControlBlock 的 backup/restore 整帧现场模型**——注入/恢复思路正确；注入点扩到任意线程、pending 用 trailing_zeros。
11. **board 解析模式**（chosen/initfs、timebase、PLIC compatible、mmu-type 过滤 Bare）——复用；扩展 generic device 列表。
12. **CpuClock 毫秒换算 + put_off 的 `usize::MAX-1` 防溢出、LCG 种子取真实 mtime**——细节已验证，照抄。

### 必须推翻（有教训，引用 plans/ 两篇）
1. **手写多级页表递归映射算法**（map_internal/free_internal 三路分支）——`plans/review-2026-08-mm-map-bug.md` 全档：未对齐分支 container/level 传错、`ensure_managed_leaf_created` 静默改权限清 X、`split_page_into_table` level 0 非法、进程隔离假象。替代：区域切段（先按对齐边界切段再逐段单一路径）或堆大页对齐约定；`LeafTable/MidTable` 类型区分 + level 0 禁 non-leaf 的 debug_assert；映射冲突显式报错（同权限幂等/异权限 EntryOverwrite）；页表纯逻辑上 host 单测（用 mm-map-bug 的 `vpn=65, count=8192, brk=0x21000` 用例）。
2. **共享调度环 + 多 hart 抢 + `Arc<UpSafeCell>` get_mut**——`plans/review-2026-07-code-review.md` P0#4：SMP 别名 UB。改 per-cpu run queue 或全局调度大锁（方案 A/B/C 三选一，重写计划 M3）。
3. **全局 static mut 状态**——review P0#6（edition 2024 后 30 个 `static_mut_refs` 硬错误）：PROC_TABLE/FRAME_ALLOCATOR/ROOT/MOUNTPOINTS/HARTS/KERNEL_UNIT/BOARD 全部改安全抽象（internals.md 的 OnceLock+Spinlock / ProcessTable 封装）。
4. **进程/线程对象永不回收**——review P1：Dead 只被 find_next 跳过，页帧泄漏；用户态 fault 用 `todo!()` panic 杀整机（app.rs:674）——用户 fault 的终态是杀进程（回收资源、通知 pm）。
5. **无锁序 + 不关中断的 spin**——review P1 + console 注释「SpinLock is causing deadlock while trap」：按 internals.md 的中断纪律重写（持有期间关 SIE），panic 路径独立 bypass。
6. **单核调度挂起 = 核内抢占失效**——`plans/review-2026-08-mm-map-bug.md` 附带发现；本考古已定位直接根因：`last_tick_time` 无写入点 → `timeslice` 恒 0 → `check_grow` 恒真 → `find_next` 恒重选当前线程（unfair.rs 692-698 vs 346 初始化）。4 核「能跑」只是各核 try_lock 抢到不同进程的假象。重写必须修复记账写入点或换 per-cpu run queue；与「异步=重调度重试」同源，需 Kernel Request/Pending/Fed 真状态机（notes/call.md 目标）。
7. **FAL 远程转发空白**——`ForeignMountPoint => todo!()` 在 fs.rs + app.rs 共 10 处、Mount/Unmount 无调用方、转发消息协议未定义（重写计划 M4 验收点）。
8. **Path 允许 ./.. + is_qualified 子串误判 + 40% 死代码**——review P2/P3：Path 层拒绝 Current/Parent、组件级判断、删死代码。
9. **DentryObject `#[repr(C)]` 原生端序 + padding 泄漏**——review P2：用已依赖的 serde+postcard 或显式 packed+逐字段序列化。
10. **进程权限空壳**（from_elf 直接 All、has_permission 零调用）——review 泛论：要么实现（子进程 ≤ 父继承 + syscall 检查）要么删接口，不留假权限。
11. **用户 tp=Tid 占位**（`TrapFrame::init` x[4]=tid）——TLS 语义未实现；重写明确用户 tp 的用途（TLS 或保持 tid 并写进 ABI 文档）。
12. **`Process::extend` 2 的幂限制 + 返回末尾反推 base**——mm-map-bug 注意 5：改返回 (base, end) 或保证连续性并在文档写明。
13. **单槽邮箱 vs 阻塞注释矛盾**——review P2：定语义（非阻塞单槽 or 真队列），文档与实现对齐。
