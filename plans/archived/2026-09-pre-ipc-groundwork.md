# IPC 前地基工程：身份、所有权与等待模型

> 状态：**已实施**（2026-09）。设计决策与实施结果如下；遗留问题见文末。

## 实施结果

- D1–D6 全部落地，`just check` / shared / user 三侧零警告零错误，host 测试全过。
- virt 10 轮压测全过：四服务回收、pm 睡眠唤醒、静默停机；停机余帧恒为
  31381 = 启动 31632 + bootstrap 回收 5 − 堆 256，每一帧都归还（P3 泄漏修复的直接验证）。
- **sifive_u 卡死根因确认并修复**：P1（slot/raw 混用）与 P5（Extend 单位错配）
  叠加。旧 ABI 下 rinlib 每进程误申请 16 MiB，128 MiB 板上必然 OOM；
  OOM 后 rinlib panic 路径经 `debug!` 的 `alloc::format` 重入自己持有的
  分配器锁自旋死锁（四 hart 用户态满速空转、无任何输出的怪象由此而来）。
  字节语义 + `debug!` 零分配化后，服务稳定完成，静默判定在多数轮次收敛。
- 附带修复：console 日志的 `{:<width$}` 动态宽度触发 core fmt
  `unreachable_unchecked` 致命 panic（嵌套 Arguments 场景），改为栈上手动填充。

## 遗留：sifive_u 非确定性内核野跳转（下一轮归因）

仍有约 1/3 轮次出现 S 态致命 trap：**cause=0xc（取指页故障）、stval=0x0、
sepc=0x0**——内核执行流野跳转到地址 0。完整 GPR 转储已取得（fatal 路径
输出有效）。该 trap 使所在 hart 停驻，进而阻塞静默收敛。属
KNOWN_ISSUES 既定的「trap 与执行环境契约缺口」领域，已有直接证据，
待专项归因；在此之前不加平台专用补丁。

---

# 原始设计（存档）

## 问题清单（已逐一核实源码）

| # | 问题 | 位置 | 性质 |
|---|------|------|------|
| P1 | IDLE_MASK 用 raw hartid 置位，与 slot 位图比较；IPI 目标集错位 | sched.rs idle/enqueue | sifive_u 静默不收敛的候选根因 |
| P2 | 自旋锁 acquire 无 Acquire 语义（lr.w 无 .aq），release 却是 Release store | sync.rs | RVWMO 下跨核临界区可见性缺口 |
| P3 | Process↔Thread 强引用环，main_thread 从不清空，reap 不真正释放帧 | proc.rs | 资源泄漏，IPC 引用面会放大 |
| P4 | Waiting/wake 是 sleep 专用桩：期限存 Pid、硬编码主线程、无取消仲裁、登记→Park 窗口有跨核双容器竞态 | sched.rs/syscall.rs | IPC 阻塞-唤醒的地基缺失 |
| P5 | uaccess 无读写方向（只验映射不验 R/W/X）；Extend 单位错配（内核按页、rinlib 按字节）、无溢出检查、失败半提交 | mm.rs/syscall.rs/rinlib rt.rs | IPC 消息拷贝的基础设施缺失 |

另有两处顺带清理：`clear_context` 未清 current_thread/fp_enabled（悬挂指针）；
shared/ 死桩（ExecutionState Pending/Fed/Rid、QueueLock 数据竞争实现、
Semaphore Relaxed 桩）与当前模型冲突。

## 设计

### D1 hart 身份：slot/raw 分离贯彻到运行期

registry.rs 头注释已写明三种事实不混用，实现违背了它。修法：

- `HartLocal` 增加公开 `slot()` 访问器（字段已有）。
- `idle()` 置位改 `1u64 << slot`；`is_quiescent` 与 `ipi_slots` 不动
  （它们已是 slot 语义）。
- 验收：sifive_u 5 核下静默判定应当收敛（若不收敛则另有原因，另行归因，
  不在本计划内追）。

### D2 锁内存序：删手写 asm，统一原子 CAS

`RawSpinlock::acquire` 的 LR/SC 换成 `AtomicU32::compare_exchange`
循环（Acquire 成功 / Relaxed 失败），与 `try_lock` 同构；release 保持
`store(0, Release)`。理由：4-5 hart 规模下 CAS 循环与手写 LR/SC 无可测
性能差；删掉一段 unsafe asm，两条路径一种写法，内存序正确性由 std 原子
语义背书而非手写栅栏。

### D3 所有权：单向 Thread → Process，反向一律 Weak

- `Thread.process: Arc<Process>` 保留（线程天然属于进程）。
- **删除 `Process.main_thread`**（唯一消费者是 wake_expired，见 D4 后
  不再需要）。
- 一切「从等待对象找线程」的反向引用持 `Weak<Thread>`；upgrade 失败
  即线程已死，惰性丢弃登记。
- 回收路径不变（reap 在调度循环单 hart 独占点执行），但 drop 后引用
  计数真正归零，帧真正归还。加回归观测：重复 spawn/exit 后帧池余量
  不单调下降（启动日志已有 frame 余量输出，人工比对即可）。

### D4 通用等待模型：Waiter + generation + 发布时序

把 sleep 专用桩泛化为 notes/call.md 描述的 KernelRequest 形态（本轮只
实现期限等待一类，结构对 IPC 等待开放）：

```rust
// sched.rs（或新 wait.rs）
pub struct Waiter { thread: Weak<Thread>, gen: u64 }
enum WaitKind { Sleep }            // IPC: Recv{...}, Tunnel{...} 后续横向加入

struct DeadlineEntry { at: u64, waiter: Waiter, kind: WaitKind }
```

- **Thread 增 `wait_gen: AtomicU64`**：每次登记新等待自增；等待条目
  携带登记时的 gen。完成方（wake_expired / 未来 IPC 唤醒方）upgrade 后
  校验 gen 相等才写结果并入队；不等即过期登记（已被取消或已被完成），
  丢弃。取消 = bump gen。单次完成事务由此结构保证，无需每线程状态机
  字段（单一归属不变量：容器成员资格仍是唯一真值，不引入镜像状态）。
- **发布时序闭合双容器竞态**：syscall dispatcher 不再直接写全局期限表，
  只把等待意图（kind + 参数）存入 HartLocal 的 park 槽，返回 Park；
  调度循环在 `clear_context`（补全后含 current_thread/fp_enabled）
  **之后**、drop 线程 Arc 之前，取 park 槽内容向全局等待结构发布。
  由此「线程可被唤醒」严格晚于「线程离开一切 hart 引用」，任何完成方
  看到的线程必然无容器。登记期限与 arm timer 语义不变（发起 hart
  立即 arm，唤醒所有权）。
- `wake_expired` 完成动作按 kind 分派：Sleep 写 a0=NoError、sepc+4；
  写帧与入队之间无锁间隙（gen 校验在先，线程此刻必然无容器）。
- 静默谓词不变（期限表空仍是唯一 Waiting 主人集合；IPC 等待加入时
  同步扩展，戒律已入档）。

### D5 uaccess 集中化 + Extend 语义固化

- 新增 `uaccess` 模块（内核侧唯一用户内存边界）：
  `copy_from_user(space, dst: &mut [u8], src: usize)` /
  `copy_to_user(space, dst: usize, src: &[u8])`。校验：区间不溢出、
  不出用户半区、逐页 translate 且 PTE 含 U 标志与所需方向权限
  （from 需 R，to 需 W）；长度上限（单次 ≤ 1 MiB，防恶意长度）。
  SumGuard 收编进模块内部，调用方不再手写 SUM 与裸指针。
  debug_print 改走此模块。
- **Extend 语义固化：字节单位 sbrk 语义**。页数是内核实现细节，不得
  泄漏到 ABI——用户进程只知道「申请 N 字节，得到 [旧堆顶, 新堆顶)
  的内存」，不知道也不需要知道页大小。内核内部向上取整到页粒度，
  返回新堆顶（页对齐字节地址）。单次申请上限 256 MiB。
- `extend_heap(bytes)` 加 checked arithmetic；
  中途失败回滚：记录成功映射数，出错时截断 `frames` 并 unmap 已映页，
  brk 不前进——映射事务要么全成要么全无。
- rinlib 输出缓冲类型修正：sys_receive/sys_read/sys_inspect/sys_peek
  的 buffer 参数改 `&mut [u8]`（ABI 不变，消除 Rust UB 面；
  MessageDigest/token 语义属 IPC 契约阶段，不动）。rinlib 侧 Extend
  调用点改字节语义：lang_start 传 INITIAL_HEAP_SIZE 字节；
  HeapRecuse::acquire 传 layout.size 字节，实际获得区域大小由
  返回的新堆顶与旧堆顶差值计算（内核取整结果，用户侧不猜页大小）。

### D6 顺带清理

- `clear_context` 补清 current_thread、fp_enabled。
- shared/src/proc.rs：删 `ExecutionState`（Pending/Fed/Rid 与现行
  Waiting 模型冲突且无人使用）、`Rid` 类型。
- shared/src/sync/：删 `QueueLock`（非原子 bool/裸指针，数据竞争桩）、
  `Semaphore`（Relaxed 语义不正确且无人用）；保留 allocator 在用的
  `SimpleLock`。
- 删除前逐个 grep 确认零消费者。

## 不做的事

- 不引入函数表/trait 分发/宏注册——顶层 match + 三值 Outcome 方向正确。
- 不动 IPC 契约（message/signal/tunnel 语义）与 FAL。
- 不为 sifive_u 添加平台专用机制（D1 修复后若静默仍不收敛，按
  KNOWN_ISSUES 既定路径另行归因）。

## 验证

1. `just check` + shared/user `cargo check` 全绿；host 单测全过。
2. `just virt`：四服务装载运行回收、fs 干净被杀、pm 睡眠唤醒、静默停机。
3. virt 多轮压测（≥10 轮）无随机停滞。
4. `just sifive_u`：观察静默判定是否收敛（D1 的直接验证）。
5. 帧池余量观测：四服务跑完后 free frames 与基线一致（P3 修复验证）。

## 提交切分

1. `fix(kernel): hart 身份统一与锁内存序`（D1+D2+D6 clear_context）
2. `refactor(kernel): 线程所有权单向化与通用等待模型`（D3+D4）
3. `fix(kernel): uaccess 集中化与 Extend 语义固化`（D5，含 rinlib 同步）
4. `chore(shared): 清理与现行模型冲突的死桩`（D6 其余）
