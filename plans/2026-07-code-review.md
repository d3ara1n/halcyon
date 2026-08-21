# Halcyon 代码 Review 后续计划

> **定位：旧实现 review 档案。** 大部分文件与结构已被重写，本篇只用于提取缺陷模式和回归样本，不作为当前代码事实、待办清单或正确实现参照。当前系统审查见 `plans/reviews/system-audit/`。
>
> 来源：2026-07 对全工程（os / user / shared）的一次完整 review。
> 行号以 review 时为准；本次已修复部分的小幅偏移用 `grep` 定位即可。
> `- [ ]` 可逐条 tick。

---

## 本次已现场修复

- [x] **#1 RNG 卡死** — `rng/lcg.rs:15` 加加法常数（`wrapping_add(11)`，java.util.Random 原版常数）解除 0 不动点；原 `25214903917 * seed` 在 debug 下乘法溢出会 panic（被 seed=0 掩盖），改 `wrapping_mul`。`hart.rs:53` seed 从 `timer.uptime()`（init 时为 0）改为 `riscv::register::time::read()`（真实 mtime）。
- [x] **#3 SimpleLock 解锁内存序** — `shared/src/sync/spin.rs` `unlock` 的 `store(false, Ordering::Relaxed)` → `Release`，保证临界区写对后续获取者可见（RISC-V 弱内存模型下真实正确性问题）。
- [x] **#2 rootfs `unreachable!` 止血** — `fs/rootfs.rs` `find_node_internal` 对非 `Component::Normal` 分支从 `unreachable!()` 改为 `Err(InvalidPath)`，避免用户传 `.`/`..` 触发内核 panic。彻底方案见 P2「Path 层禁止 ./..」。
- [x] **#7 Debug 系统调用 `from_utf8_unchecked`** — `hart/app.rs:152` 改 `String::from_utf8_lossy(&buffer)`，不再让用户字节破坏 `String` 的 UTF-8 不变量。
- [x] **#6 标量全局 `static mut` → Atomic** — `mm.rs` `KERNEL_SATP` → `AtomicUsize`；`sbi.rs` `TIME_SUPPORTED` / `DEBUG_CONSOLE_SUPPORTED` → `AtomicBool`。`#[export_name = "_kernel_satp"]` 保留（AtomicUsize 与 usize 内存布局一致，符号无影响）。

**验证状态**：`os kernel` + `shared` 已 `cargo check` 通过（edition 2024 + 全部依赖升最新 + 上述修复）。`user/rinlib` 的 talc 3→5 迁移已写但**未机器验证**——user workspace 的自定义 `.json` target + build-std 在当前环境 check 不了（cargo 演进，预存配置问题），需在原构建环境 `cargo check` 验证。

**依赖升级（2026-07，全部升最新，os+shared 已验证）**：riscv 0.11→0.14、hashbrown 0.14→0.17、buddy_system_allocator 0.9→0.13（+`alloc` feature）、tar-no-std 0.2→0.4、elf_rs→0.3.1、strum→0.28、num-derive 0.4→0.5、goblin 0.8→0.9、talc 3→5、edition 2021→2024。升级适配：riscv `scause::cause()` 返回 raw usize 需 `.try_into::<Interrupt,Exception>()`、tar `filename()` 返回 `TarFormatString` 需 `.as_str()`、edition 2024（`gen` 保留字→`current_generation`、`unsafe extern`、`#[unsafe(no_mangle)]`/`#[unsafe(export_name)]`、`let_chains` 合法）、nightly `PanicMessage` 直接 Display 不再 `unwrap`。

**edition 2024 撞 #6**：升 edition 2024 后冒出 30 个 `static_mut_refs` 硬错误（PROC_TABLE/FRAME_ALLOCATOR/ROOT/MOUNTPOINTS/HARTS 等），即下文 P0 的 #6。已加 `#![allow(static_mut_refs)]` 临时绕过让编译通过，**#6 现升级为 edition 2024 强制项**。

---

## 环境阻塞（解决后才能编译验证全部改动）

- [x] **`tar-no-std` 0.2→0.4** — 已升级并适配（见上「依赖升级」）。

---

## P0 · 并发 soundness（核心，决定「能在 SMP 下不随机崩」）

### #4 UpSafeCell 误用为 SMP 共享（别名 UB）

`sync/up.rs` 的 `UpSafeCell` 名字明示「Uniprocessor SafeCell」，`get_mut(&self) -> &mut T` 从 `&self` 造 `&mut`，靠 `unsafe impl<T> Sync` 强制共享。但 `task/sched/unfair.rs:34` `type Shared<T> = UpSafeCell<T>` 把全局 `ProcessCell`/`ThreadCell` 包成 `Arc<UpSafeCell<...>>`，多 hart 同时 `get_mut()` = Rust 别名模型 UB（字段级 `SimpleLock` 只保护逻辑互斥，不保护类型互斥）。

**三选一（需拍板）**：

- **方案 A（推荐，平衡）**：把 `Arc<UpSafeCell<T>>` + 字段级 `SimpleLock` 的混合模型，整体替换为 `Arc<lock_api::Mutex<SimpleLock, T>>`，`.get_mut()` → `.lock()` 返回 guard。`unfair.rs` 几十处手写 `lock.lock()`/`unsafe { lock.unlock() }` 改成 guard 自动释放。模型干净，改动量大但机械。
- **方案 B（小改，放弃真 SMP）**：保留 `UpSafeCell`，但在架构上约束「同一时刻只有一个 hart 运行用户进程」——加一把全局「调度大锁」，syscall 入口获取、调度时释放。其它 hart 仅 idle/IPI 唤醒。放弃真并行，但 sound。
- **方案 C（彻底，最大工作量）**：per-cpu run queue，`PROC_TABLE` 元数据用真锁，调度器无全局共享可变状态。真 SMP。

> 选定方向前建议先确认：当前是否真的有多 hart 同时跑用户进程？（`awake_idle`/`send_ipi` 存在，但可能只是备用唤醒。）若实际单 hart 运行，B 的紧迫性可降级，但仍不 sound。

### #6 复合类型全局 `static mut`（运行期可变，真并发 UB）

> **edition 2024 已把本项变成硬编译错误**（30 个 `static_mut_refs`），现以 `#![allow(static_mut_refs)]` 临时绕过。修本项时即可去掉该 allow。这是架构性改动，与 #4 的 PROC_TABLE 耦合，建议作为独立重构专项。

`core::cell::OnceCell` 非 `Sync`，靠 `static mut` + `unsafe` 强制共享；`.get_mut()` 在运行期被多 hart 调用 = 别名 UB。

| 位置 | 全局 | 访问模式 |
|---|---|---|
| `mm/frame.rs:10` | `FRAME_ALLOCATOR` | alloc/dealloc 频繁 `.get_mut().lock()`（内部 `LockedFrameAllocator` 自带锁，但外层 OnceCell 别名救不了） |
| `mm.rs:19` | `KERNEL_UNIT` | 多处 `.get_mut()` |
| `fs.rs:22,25` | `ROOT` / `MOUNTPOINTS` | 每次 FAL syscall `.get_mut()`；`MOUNTPOINTS` 注释说「init 后只读」但仍是 `static mut Vec` |
| `board.rs:14` | `BOARD` | `.get()` 返回 `&T`（init 后只读，但 `static mut` + `OnceCell` 非 Sync 仍违规） |
| `hart.rs:23` | `HARTS` | `get_hart()` 返回 `&'static mut HartKind`（每 hart 取 `&mut Vec` 别名） |
| `unfair.rs:39` | `PROC_TABLE` | 与 #4 绑定 |

**方案**：显式加 `spin` 依赖（项目已通过 `buddy_system_allocator` 的 `use_spin` feature 间接引入 `spin`，直接用合理）。
- init 一次后「外壳不变、内部自带锁」的（`FRAME_ALLOCATOR`/`KERNEL_UNIT`/`ROOT`）：外层换 `spin::Once<T>`，`.get()` 返回 `Option<&T>`，不再 `.get_mut()`。
- init 后只读的（`MOUNTPOINTS`/`BOARD`）：同样 `spin::Once`。
- `HARTS`：`get_hart` 改返回 `&HartKind`（不可变），需可变处用内部可变性；或改 per-cpu 数组 `Box<[HartSlot]>`，每 hart 只动自己的槽。

---

## P1 · 健壮性

### syscall 路径 `.expect()` 杀内核（#5）〔用户暂缓〕

`hart/app.rs:263/363/370/658/670/694`、`unfair.rs:174/315/323`。注释写「kill process」「return user an error」但实现是 `.expect()` 杀整机。改 `Err(SystemCallError::...)`，syscall 层把进程标 `ProcessHealth::Dead` 后 reschedule。**前置依赖：进程回收（见下）必须先有，否则标了 Dead 没人清理。**

### idle hart 被 IPI 唤醒 → `handle_kernel_trap` panic（#8）〔用户暂缓〕

`assembly.asm` `_park` 显式开 `sstatus.SIE` + `sie.SSIE`；`awake_idle()` 发 IPI 置 `sip.SSIP`；wfi 醒来陷入 `handle_kernel_trap`，而该函数（`trap.rs:145-151`）对非 Timer 中断全走 `_ => kernel_dump()` → panic。
- 改 `handle_kernel_trap` 处理 `SupervisorSoftware` 中断（清 `SSIP` 后 `sret` 回 wfi）；**或** `_park` 保持 `SIE=0`，依赖 RISC-V「wfi 在局部 pending 中断时返回而不陷入」的语义。

### 进程回收缺失（内存泄漏）

`ProcessHealth::Dead` 只让 `find_next` pred 跳过，进程对象（`MemoryUnit` + 所有页帧）永远留在 ring 里。`unfair.rs` 注释写「to be killed」但 killer 不存在。
- 新增回收路径：`Dead` 进程从 ring 摘除 → `Drop` 释放 `MemoryUnit`/页帧 → 回收 Tid/Pid 槽位。需配合 #4 的锁改造，避免回收时其它 hart 正在遍历 ring。

### `handle_kernel_trap` timer 分支 `todo!`

`trap.rs:148` `todo!("nested supervisor timer")` = 内核态发生 supervisor timer 即 panic。至少 `warn!` 后 `sret`。

### `PageTableIter` 自引用 + `UnsafeCell` 递归（脆弱）

`mm/page.rs:421-467`。迭代器把子迭代器塞进 `Box<UnsafeCell<..>>` 再递归调 `self.next()`，`self` 与 `*inner` 同时被可变借用，未证明不别名。`Display for MemoryUnit`（`unit.rs:595`）依赖它。
- 改显式栈 `Vec<(&PageTable<E>, usize, usize)>`，无 `UnsafeCell`、无递归 `self.next()`。

### `Process::extend` 要求 size 为 2 的幂

`task/proc.rs:196-214` 失败返回 `MisalignedAddress`（语义错）。放宽为页对齐 `size & (PAGE_SIZE-1) == 0`，错误归 `InvalidArgument`。关联 `break_point` 的 `next_power_of_two()`（`proc.rs:141`，`usize::MAX` 附近溢出 panic）。

### `is_address_in` 的 usize 减法下溢

`unfair.rs:120-141` `(self.trampoline - addr)` 在 `addr>trampoline` 时 wrap，`diff as Tid` 截断。改 `checked_sub` + 显式区间校验。

### console 锁死锁风险

`console.rs:8,92-97` 注释自承「SpinLock causes deadlock while trap」。panic_handler 又调 `println!`，若 panic 发生在持锁 hart 则他人永等。改「关中断 + try_lock 失败降级无锁 SBI 直写」，panic 路径用独立 bypass。

### 持锁顺序无文档

`unfair.rs` head/last/ring/state/run 五把锁交叉持有，无 lock-order 注释。至少写一行全局锁序不变量。

---

## P2 · 架构与语义一致性

### Path 层彻底禁止 `./..`（#2 根因）

`shared/src/path.rs`：`is_valid` 只拒 `\0`，允许 `.`/`..`（设计文档 `notes/fal.md:35` 禁止）。**本次只在 rootfs 止血**。彻底做法：`Path::from` 解析后拒绝 `Component::{Current,Parent}`，并删掉为 `./..` 服务的死代码（见 P3「path.rs 40% 死代码」）。`is_qualified`（`path.rs:82`）用子串 `contains(".")` 是错的，会把 `file.txt` 误判，应组件级判断。

### `Send` syscall 违背设计

`task/ipc/message.rs:47-54`、`call.rs:83` 注释「block until received」，实现是单槽非阻塞，满即返 `ObjectNotAvailable`。统一文档与行为（要么真阻塞，要么改文档为非阻塞试探）。

### `Tunnel::link` 鉴权形同虚设

`task/ipc/tunnel.rs:32-46`。第二次 link 时 `first == self.owner` 恒真（owner 刚被赋值为 first），任何 pid 都能挂第二个端点。条件应为 `pid == self.owner || pid == first`。

### `DentryType` 映射自相矛盾

`shared/src/fal.rs:119` vs `:135`：String 属性经 `DentryMeta` 报 `Stream`，经 `PropertyKind` 报 `String`。`DentryType::String` 定义了从不产生。统一一个。

### `DentryObject` `#[repr(C)]` padding 泄漏 + 端序写死

`shared/src/fal.rs:142-150`、`hart/app.rs:361-363`。`#[repr(C)]` 结构含 6 字节未初始化 padding，`from_raw_parts(d as *const _ as *const u8, size)` 拷给用户态 = 信息泄漏 + MIRI 报错；原生端序写进 ABI。
- **正解**：用上已声明却零引用的 `serde` + `postcard`（见 P3「死依赖」），得到稳定、长度前缀、无 padding 的线格式；或显式 `#[repr(C, packed)]` + 逐字段序列化。

### syscall ABI 上限 4 参数

`user/rinlib/src/call.rs:33` 用 `a0–a3`，而 RISC-V ecall 给 `a0–a5`（6 个）。FAL 全是「path 占两参」就因这个限制，后续加 syscall 会撞墙。扩到 6 参。

### `redirect_with`/`measure`/`make_objects` 重复递归 + `todo!()`

`fs.rs:67-87/105-135/137-186`，5 个 FAL syscall arm 各含一份 `FilesystemAbstractLayerError → SystemCallError` 复制 + `ForeignMountPoint => todo!()`。抽 `impl From<FilesystemAbstractLayerError> for SystemCallError`，mountpoint 下沉合并到一个 helper。

### procfs/read 忽略 length、write 不更新 modified_at

`fs/procfs.rs:273` 忽略 `length` 恒返回 8 字节（rootfs 尊重 length，契约不一致）；`fs/rootfs.rs:581-700` `replace` 不更新 `modified_at`（永远 == created_at == 0）。`make_objects`（`fs.rs:154-162`）目录子项时间戳/size 全填 0。`Message::time`（`ipc/message.rs:16`）恒 0。

### `LocalProperty::to_bytes` 截断规则不一

`fs/rootfs.rs:341-371` 各 variant 截断规则不同，String 变体会切坏 UTF-8 导致 write 路径 `from_utf8` 失败（`rootfs.rs:678`）。

### 文档漂移

`notes/fal.md:80` 仍列 `in_use: bool`（代码 commit `20f4852` 已删）。

---

## P3 · 代码清理（无意义劳动）

### 死依赖（声明零引用）

- [ ] `serde` + `postcard`（os/kernel + user/frameworks/libsrv）— 序列化全是手写 `#[repr(C)]` 拷贝。**要么删，要么真用上**（真用上正好解决上面 `DentryObject` padding）。
- [ ] `goblin = "0.8"`（os/kernel）— ELF 解析已换 `elf_rs`（`task/proc.rs:2`）。
- [ ] `semver = "1.0"`（os/kernel）— 零引用。
- [ ] `num-derive = "0.4"`（os/kernel）— 内核只用 `num_traits::FromPrimitive` trait，derive 在 shared。

### 空文件 / 孤儿模块

- [ ] `os/kernel/src/fal.rs`（0 行，`fs.rs` 声明）
- [ ] `os/kernel/src/fs/sysfs.rs`（0 行，`fs.rs:20` 声明，notes 列为支持文件系统）
- [ ] `os/kernel/src/device.rs`（0 行，无人 `mod device`，真孤儿）
- [ ] `os/kernel/src/board/device/memory.rs`（0 行，`board/device.rs:15` 声明）
- [ ] `os/kernel/src/task/sched/enough.rs`（0 行，「公平调度器」占位）

→ 删声明，或 `todo!()` 让意图显式。

### 死代码

- [ ] `shared/src/sync/spin.rs` `QueueLock`（24-95 行，70+ 行）从未引用，且含跨线程非原子 `*mut Ticket` 访问（真 data race）。删，或 `Ticket::next` 改 `AtomicPtr` + Acquire/Release。
- [ ] `shared/src/path.rs` ~40% 死：`Component::{Current,Parent}`、`qualify`/`is_qualified`/`Div<&str>`/`collect_remaining`/`make_root`/`prepend` 全为 `./..` 服务（设计禁止 `./..`）。配合 P2「Path 禁止 ./..」一并清。
- [ ] `mm/page.rs:28-37` `Cow`/`CowWriteable` flag 定义从未实现 CoW；`PrefabKernelTrapframe` 定义未用；`Prefab*` 是 flag 或运算别名，塞进 enum 当 variant 语义混乱，改 `FlagSet` 常量。
- [ ] `unfair.rs:212/228` `_move_next_until`/`_count_if_match_until` 整函数 dead（`_` 前缀）。
- [ ] `hart/app.rs:75/79/109/131` `_send_ipi`/`_clear_ipi`/`_stop`/`_handle_remote_call` 全 dead。

### 注释掉的代码 / 无用定义

- [ ] `Justfile:48` `#cargo clean --manifest-path user/Cargo.toml`、`:102` `# make_sdcard 可以先稍稍`
- [ ] `Justfile:8/34-36` `DEBUGGER_OPTIONS`/`GDB_BINARY`/`GDB_TARGET` 定义后无 recipe 引用
- [ ] `user/rinlib/src/dbg.rs:17` `if let Err(_) = ... {}` 空操作 → `let _ = sys_debug(...)`

### 5× 复制粘贴（消灭可砍 ~90 行）

- [ ] `hart/app.rs` Access/Inspect/Create/Read/Write（`311-329/380-398/421-439/476-494/518-536`）逐字复制「读 path → from_utf8 → Path::from → fs::op → 错误映射」+ `ForeignMountPoint => todo!()`。抽 `fn with_user_path(proc, addr, len, F) -> Result<..>` + P2 的 `From<...> for SystemCallError`。
- [ ] `user/rinlib/src/fs/components.rs:99-163` 手写 `[b0,b1,…b7]` 转 i64/f64 重复 6 次 → `i64::from_ne_bytes(bytes[..8].try_into().unwrap())`。

### 手写轮子（一行可替）

- [ ] `task/ipc/signal.rs:48-64` 循环 64 次找最低置位 bit → `pending.trailing_zeros()`；`backup/restore`（`:92-106`）索引循环 → `copy_from_slice`。
- [ ] `mm/frame.rs:64-68` 逐 `u64` 清零 → `ptr::write_bytes`。
- [ ] `fs/rootfs.rs:603-668` `i64::from_ne_bytes([...])` ×4 → `chunks_exact(8).map(...)`。
- [ ] 手写 `(x + 7) & !7` 对齐算术重复 3 次（rinlib/fs.rs 等）→ 抽 `align_up(x, 8)`。

### 命名 / 拼写

- [ ] `user/rinlib/src/lib.rs:15` `pub mod preclude;`（4 文件引用）→ `prelude`。「Preclude」意为「阻止」。
- [ ] `HeapRecuse`/`heap_rescue`/`LockedHeapWithRescue`（`rinlib/src/rt.rs:12`、`os/kernel/src/rt.rs`）→ 统一 `Rescue`（「Recuse」非词）。
- [ ] `mm/unit.rs:31` 字段名 `where_the_frame_tracker_of_root_for_recycling_put` → `root_frame`。
- [ ] `expect` 消息：`rinlib/src/rt.rs:81,100` `"this can't be wrong"`、`ipc/signal.rs:21/41` `"this wont failed"`、`hart/app.rs:145` `"Die die die!"`、`:370` `"error!"`。

---

## P4 · 现代化与工程化

### unsafe 纪律（149 处 unsafe，0 处 `// SAFETY:`）

对所有裸指针解引用、`from_raw_parts_mut`、`transmute`、汇编 `ecall`/`csrw` 交互补 `// SAFETY:` 注释，说明不变量。后续 review 与重构的前提。

### 测试（全工程零测试）

- [ ] 至少先给纯逻辑加单测：`shared/path.rs`、`fs/rootfs.rs` 字节编解码、`rng/lcg.rs` 序列、`signal.rs` dequeue。
- [ ] `shared` 是 host-able no_std 库，可直接 `cargo test`。

### 依赖升级（均落后一小版）

- [ ] `riscv = "0.11"` → 0.12
- [ ] `hashbrown = "0.14"` → 0.15
- [ ] `buddy_system_allocator = "0.9"` → 0.11
- [ ] `goblin = "0.8"` → 0.9（但见 P3 死依赖，建议直接删）

### toolchain 钉版 / 冗余 feature

- [ ] `rust-toolchain` 只写 `nightly`，未钉日期 → 改 `nightly-YYYY-MM-DD`。项目依赖 `lang_items`/`alloc_error_handler`/`panic_info_message`/`let_chains`，不钉版迟早 build break。
- [ ] `#![feature(let_chains)]`（`main.rs:2`）在 Rust 1.88 已稳定，冗余，删后改报 warning。

### Justfile

- [ ] `artifact_dir`（`:50-60`）三个 `if [ ! -d ]` → 一句 `mkdir -p artifacts/build artifacts/initfs`
- [ ] `initfs.tar` 用 `find | tar` 顺序非确定（`:81`）→ 显式 `sort`，利可复现构建
- [ ] `linker.ld` `INCLUDE "../artifacts/memory.x"` 相对路径耦合 `build_kernel` 先 cp → 改 `-T` 绝对路径经 RUSTFLAGS 传
- [ ] 内核 `riscv64-elf-ld` vs 用户 target JSON 的 `rust-lld`，linker 不一致 → 统一
- [ ] 加 `test` / `check` / `clippy` / `fmt` recipe

### `.code-workspace`

- [ ] `terminal.integrated.env.linux` 在 macOS host 上无效 → `.osx` 或删
- [ ] `cSpell.words` ~110 词是噪音，可选精简

---

## 附：reviewer 误报（已澄清）

- `task/ipc/signal.rs:10` 「`static mut SIGNAL_HANDLER`」— **不存在**。`SignalControlBlock` 是进程内字段，非全局。该条忽略。
- `SimpleLock::lock`（`spin.rs:119`）调 `self.is_locked()` — 来自 `lock_api::RawMutex` 的默认实现（`try_lock` 后立即 `unlock`），编译能过（shared 已 check）。只是自旋效率低（反复获取/释放），非正确性问题，可后续优化。
