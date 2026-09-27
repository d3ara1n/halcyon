# 用户态映射 owner 与线程生命周期施工计划

> 状态：**终局原因与 ThreadCreate/ThreadStart 机制面已实施，ThreadRegistry/owner 迁移待施工**。本记录拥有线程出生（ThreadCreate/Start）、终局原因替换退出码、用户态线程三 owner 与协作式 cleanup 的施工范围、顺序和验证门。契约由 [task](../notes/ideas/task.md) 拥有（「终局原因」「线程：执行单元」「用户态线程资源收束」），组合边界由 [framework](../notes/ideas/framework.md)、入口交接由 [bootstrap](../notes/ideas/bootstrap.md#启动映像的寿命) 拥有。审阅 findings 的处置记录在 [runtime-thread 审阅](review-2026-09-26-runtime-thread-design.md)。普通 mapping/堆 arena 的其它退休需求不在本轮范围，保留在末节。

## 目标与非目标

目标：

1. 线程出生统一为 ThreadCreate（挂起）→ 启动；Building 线程由 ProcessStart 批量发布，Running 线程由 ThreadStart 发布。Start 不分配、不因资源失败。
2. 删除进程/线程退出码；进程终局记录结构化 `TerminationCause`。
3. 用户态线程责任拆为 ThreadRecord（libthread Registry）、共享 ThreadControl、`ResultCell<T>` 三个 owner；任意用户代码只在用户线程执行；无专职 driver，协作式 cleanup。
4. 入口线程经 `EntryThreadDescriptor` 一次采用，与运行期线程共用状态机。
5. 删除 safe `thread::exit`、同步等待的 JoinHandle Drop、abandoned 计数和 `wait_and_release` 的 `expect`。

非目标：TLS、unwind、detach、scoped thread、专职 reaper、及时回收承诺、多 Rust 入口、ThreadKill。

## ABI 草案（`shared/erhino_shared`）

| 项 | 旧 | 新 |
|---|---|---|
| 退出码 | `ExitCode = i64`、`Termination::to_exit_code`、`ProgramResult` | 删除 |
| 线程离场 | `ThreadExit(code)` | `ThreadExit()`，无参数、不返回 |
| 进程自终 | `ProcessExit(code)` | `ProcessTerminate()`、`ProcessAbort()`，无参数、不返回 |
| 执行需求 | `ProcessStart(builder, profile)` | `ProcessBindExecution(builder, profile)`，Building 期一次；`ProcessStart(builder)` 无执行参数 |
| 线程出生 | `ProcessAttach(builder, ctx)`、`ThreadSpawn(ctx) -> ThreadSpawnResult` | `ThreadCreate(target, ctx, out) -> ThreadCreateResult`；target 为 Builder（Building）或 self 哨兵（Running） |
| 运行期启动 | 无 | `ThreadStart(control)`；只接受 self 进程中挂起线程的 Control，需 MANAGE right |
| 终局快照 | `ProcessExitReason{None,Exited,Fault,Killed,Abandoned}` + `code: i64` | `TerminationCause{None,Completed,Terminated,Aborted,Killed,Faulted,Abandoned}` + 定长 `detail`（Killed：来源类别；Faulted：tid、`ProcessFaultCode`、地址；其余为零） |

`ThreadCreateResult` 固定宽：`tid`、`control` 表项值；Building 形态中 control 是**目标进程**表项值，不是调用者 owner。ThreadControl rights 带 MANAGE（Running 形态授予调用者，Building 形态安装进目标表时同样授予，以便入口 runtime 统一处理，但 Building 线程只能由 ProcessStart 发布，ThreadStart 对非 Running 进程的线程返回 BadState）。调用号直接重排，不保留旧号。

### 终局切面的协议裁决

终局原因与无退出码可先于 ThreadCreate/Start 独立迁移。`ProcessKill` 仍是幂等的非等待请求：在进程 lifecycle 锁内首次冻结 `Killed` 时报告 Accepted；若目标已进入 Terminating/Dead 则报告 AlreadyTerminating。两者都不等待线程退出或 Drain；自杀式调用不返回。这个返回值是内核线性化事实，不是调用者可写的退出码。双 Kill 的竞速消费者根据两个返回值断言恰有一次 Accepted，快照只记录 Killed；其它终局先获胜时 Kill 返回 AlreadyTerminating。不存在内核可区分的 Job 来源：JobKill 最终使用 ProcessControl，不给 Killed 编造第二个来源类别。

`Faulted` detail 包含 tid、稳定 fault 类型及用户地址。同步用户 trap 用 `stval`，syscall 用户内存访问/输出写失败用发起线程 tid 与失败的用户 VA，fault 类型为独立的 `UserAccess`，不伪装为指令/访存陷阱。ThreadCreate/Grant 等已提交输出写失败同样归 `Faulted(UserAccess)`；无法确定具体失败字节时记录本次输出起始 VA，不借用用户可写退出码。`Completed`、`Terminated`、`Aborted`、`Abandoned` 的 detail 全零。

### ThreadCreate/ThreadStart 施工裁决

`ThreadCreate(target, ctx, out)` 是两条出生通道的单一 ABI：`target` 只能是 `ProcessBuilder`（Building 外部组装，要求 MANAGE、Bound AddressSpace、已 BindExecution）或 self 哨兵（Running 当前进程隐含作用域）；不接受 `ProcessControl`。一次调用准备完整挂起线程、全寿命 Ready 准入、Created 成员、嵌入式发布节点和 `ThreadControl`。输出固定为 `{tid, control}`；Building 的 control 是原子安装到目标表的 child-local 表项值，Running 的 control 是调用者表内句柄。输出写失败按发起 tid/VA 冻结 `Faulted(UserAccess)`，不伪造可重试状态。

`ThreadStart(control)` 无输出、无分配，只把 Created 发布为 Ready。它要求 ThreadControl 带 `MANAGE`；`WAIT` 只负责观察，Close 不要求 MANAGE。非 Created、所属进程仍 Building、进程已 Terminating/Dead 均拒绝；Building 线程只能由 ProcessStart 批量发布。Start 前关闭最后一个 Control 或进程终止时，Created 线程必须经 `ThreadDeparture::finalize` 发布 DONE。`ProcessStart` 只检查 Bound、至少一个 Created、Job 门和 `building_ops==1`，沿嵌入发布链有界发布，不再选择执行域、预留 staged Vec 或另建 Ready 容量。

Staging/Spawning 合并为 Created；`ProcessAttach`、`ThreadSpawn`、`ThreadSpawnResult` 删除。bootstrap 复用同一内核 ThreadCreate 核心和 Created/准入/离场路径，初始 Control 安装与 `EntryThreadDescriptor` 同源。上述 MANAGE right、target 形状和 Building 上的 ThreadStart 拒绝是本记录后续施工的单一真值。

## 内核改动点（`os/kernel`）

| 位置（施工时复核行号） | 改动 |
|---|---|
| `task/process.rs` Start 路径 | BindExecution 与 ThreadCreate 已承担 execution/准入；Start 只做门检、关闭组装权、发布链 |
| `task/process.rs` / `proc.rs` Attach | 并入 ThreadCreate(Building)：要求已 BindExecution；创建时取得全寿命 Ready 准入、预留目标表 Handle 槽并原子安装 Control |
| `proc.rs` ThreadCreate(self) | 挂起成员态 `Created`，不入 Ready；ThreadStart：Created→Ready，无分配 |
| `lifecycle.rs` 成员状态 | `Staging/Spawning` 已合并为 `Created`（挂起、已准入、不在任何执行容器）；Start 以嵌入成员链节点有界发布 |
| `lifecycle.rs` Staging/Created 终止路径（原直接 drop） | 必须经 `ThreadDeparture::finalize` 发布 DONE；Control 关闭挂起线程时同样离场 |
| `lifecycle.rs` 末线程冻结（原 `normal_code.expect`） | 冻结 `Completed`；ProcessTerminate/Abort/Kill/Fault/Abandon 各自冻结原因，首达者胜 |
| `trap.rs`、fault 路径 | 写 `Faulted` detail，不再编码为特殊 code |
| bootstrap（`startup` 内嵌 Attach） | 复用 BindExecution + ThreadCreate 同源机制；init 的 StartupBlock 写 `EntryThreadDescriptor` |
| selftest（task/wait_set/tunnel） | 按新调用签名迁移 |

线性化保持不变：Building 操作 lease、Start/终止关闭登记、execution gate 顺序不改。Start 发布尾段的上界沿用现有 ReadyBatch 上界来源。

## 用户态类型

libkernel（由当前 rinlib 拆出，见[分层计划](todo-2026-09-runtime-kernel-library-separation.md)）：`ThreadControl` owner（Close on Drop）、`thread_create`/`thread_start`、unsafe `thread_exit()`、`process_terminate()`/`process_abort()`、`TerminationCause` 解码。

libthread：

```rust
pub trait ThreadEnvironment { type Child: Send; type Residue: Send; /* prepare / enter / leave / retire（retire 有界不停驻） */ }
pub struct ThreadRegistry<E: ThreadEnvironment> { /* 固定容量槽、claim、seal */ }
pub struct ThreadFactory<'r, E> { /* &Registry + 栈策略 */ }
pub struct PreparedThread<F, T, E> { /* 槽、栈、packet、ResultCell、挂起 Control */ }
pub struct Thread { control: Arc<ThreadControl> }
pub struct JoinHandle<T> { cell: Arc<ResultCell<T>>, thread: Thread }
pub enum JoinOutcome<T> { Returned(T), EnterFailed(EnvError), LeaveFailed(T, EnvError), Incomplete }

impl<E> ThreadRegistry<E> {
    pub fn reap(&self, budget: usize) -> ReapProgress; // 协作推进唯一入口
    pub fn adopt_entry(&self, desc: EntryThreadDescriptor, residue: E::Residue) -> Result<Thread, AdoptError>;
    pub fn seal(&self);
}
```

`ResultCell` 状态 `Empty/Published/ReceiverGone` 与交错表见 task「结果发布」。record 字段：栈 `MappedRegion`、`Option<E::Residue>`、`Arc<ThreadControl>`、cleanup 进度；不含 F/T，无 vtable。槽 claim 用原子 CAS，reap 只处理 claim 成功且 DONE 已观察的槽。

libruntime：`_start` 汇编入口 → 无堆校验 → allocator → `ThreadRegistry` 静态实例（容量来自启动配置/链接常量，按下节）→ `adopt_entry`（原子 once）→ StartupContext → `main`；`Ok` → wrapper 离场路径（reap + `thread_exit()`），`Err`/panic/OOM → 日志 + `process_abort()`。

## 容量与预算

Registry 容量是程序装配给出的并发线程上限，属于程序配置（由 runtime 入口宏参数或启动 payload 给出），不是库常量；槽存储在 Registry 建立时一次分配。每个 prepare 的栈、packet 与 ResultCell 走普通可失败分配，在 ThreadCreate 前完成；ThreadCreate 之后的路径不分配。ResultCell 的寿命独立于槽，按堆内存计。

## 交错与失败表

| 场景 | 期望 |
|---|---|
| prepare 任一步失败 | 反向返还，未创建内核线程 |
| ThreadCreate 成功、调用者放弃 PreparedThread | 关闭 Control → 内核离场并发 DONE；record 进入普通 reap；F 在放弃者线程 Drop |
| ThreadStart 被拒（进程终止中） | 返还 PreparedThread，放弃路径同上 |
| 子线程早于 JoinHandle 被构造即结束 | 无竞争：record 与 cell 在 Start 前已稳定 |
| JoinHandle Drop 早于/晚于发布 | CAS 仲裁，T 恰一次在用户线程 Drop |
| 延迟领取 | 槽已被 reap 回收，cell 仍可领取 |
| raw ThreadExit 绕过 wrapper | DONE + Empty → Incomplete |
| Unmap Busy | reap 返回 Wait，下次推进重试 |
| Unmap/Close 永久错误 | 保留 record，进程完整性故障（ProcessAbort） |
| Created 线程所在进程被 Kill | 离场发布 DONE，等待者不挂死 |
| 第二次进入 `_start` | adopt once 失败 → ProcessAbort |
| 入口未采用前进程终止 | ProcessDrain 收回栈与 Control |
| main 返回、worker 仍运行 | 进程继续；末线程离场 → Completed |

### 当前未闭合责任
- bootstrap 初始线程已复用 ThreadCreate 的 ThreadControl、MANAGE rights、Created/Departure 与预留 Ready 准入；EntryThreadDescriptor 已由 shared/startup v1 校验，初始 Control 通过 descriptor 传递。剩余一次采用与 ThreadRegistry 登记归 Registry/libruntime 切面。
- `ProcessStart` 已改为 Create 时每线程预留 ReadyBatch、lifecycle 锁内挂接并摘取 Thread 内嵌发布链、锁外逐项发布；bootstrap 也复用单线程 Create/Ready admission，不再有普通路径的 Start 承接 Vec。
- `user/libraries/libprocess/src/lib.rs` 已恢复 `Spawned.control` 为监督用 ProcessControl；ThreadCreate 的 child-local ThreadControl 只写入 EntryThreadDescriptor。后续 Registry/libruntime 负责一次采用与入口 ThreadRecord owner。
- bootstrap descriptor ABI v1 已落地：StartupBlock 内固定 section，包含 child-local ThreadControl、stack reservation、usable 范围及 kind/version/len；init 与普通 launcher 共用 `shared/startup` 校验，普通 launcher 在 ThreadCreate(Building) 后经 Building Write 回填 control。descriptor 的一次采用与 ThreadRegistry 登记留给后续 libthread/libruntime 切面。
- ThreadRegistry/owner 尚未施工：当前 `rinlib::JoinHandle` 仍直接持有 Control、栈和 Result packet；后续需迁为固定槽 Registry、共享 ThreadControl、ResultCell 三 owner，并由 libruntime 负责入口 Registry 实例与 descriptor adopt。

### ProcessStart 发布结构裁决

已知不变量：Start 不分配，不因资源不足失败；每条线程的 Ready 准入在 ThreadCreate 时取得。路线已按锁序选定：

源码核对结果：选择路线 2。`lifecycle` 锁为 rank 600，Ready 队列使用 LEAF rank 150；现有锁序明确禁止在 lifecycle 锁内取得持久 Ready 集合锁，`ReadyBatch::publish` 也会取得调度类锁。因此不能在 lifecycle 临界区直接入队。必须在线程 Create 时预留并挂接嵌入式发布节点，Start 在 lifecycle 锁内只完成 Building→Running、Created→Ready 和发布链摘取，锁外把已预留节点交给调度器；Start 不分配，且不再使用 `Vec<Arc<Thread>>` 承接。

## 消费者迁移

- `user/rinlib/src/rt.rs`：删除 `to_exit_code` 路径；main 返回 Ok 只走 `ThreadExit()`，Err/panic/OOM 走 `ProcessAbort()`。
- `user/rinlib/src/thread.rs`、`process.rs`、`call.rs`：迁到 libkernel/libthread 新 API；删除 safe `exit`、`UserStack` abandoned 计数、`wait_and_release`。
- `srv_init/main.rs`（子进程成功判定旧 `reason==Exited && code==0`）→ `cause==Completed`，并按 `TerminationDetail` 校验。
- `srv_init/race.rs`：以旧 code 区分竞速胜者的用例改为比较 `TerminationCause`；双 Kill 通过真实 `KillOutcome::Accepted/AlreadyTerminating` 报告胜者，其它竞速只断言允许的终因。
- `srv_init/public_ipc.rs`、`test_hammer`、`test_target`、`shared/elf/audit_user_elf.rs`：按新签名迁移。
- libprocess launcher：BindExecution、ThreadCreate(Building)、写 `EntryThreadDescriptor`。

## 施工顺序

1. shared ABI（调用号、`TerminationCause`、`TerminationDetail`、`ThreadCreateResult`、`EntryThreadDescriptor`、删除 ExitCode）。**终局原因子切面已完成。**
2. 内核：BindExecution → ThreadCreate/Created 态/Start 链发布 → ThreadStart → 挂起线程离场 DONE → 终局原因 → bootstrap 同源 → selftest。
3. libkernel 封装（与分层计划的 rinlib 拆分同步进行，不保留 alias）。
4. libthread：ResultCell → Registry/reap → Factory/Prepared → 入口 adopt；host 单测覆盖 ResultCell 交错与 Registry claim。
5. libruntime `_start`、main 语义、abort 路径。
6. 消费者迁移并删除旧路径。
7. `just check`、`just clippy`、QEMU 路线（按 [构建手册](BUILD-AND-TEST.md)）与交付前结构检查。

终局原因与 ThreadCreate/ThreadStart/bootstrap/ProcessStart 机制面已完成开发检查；完整用户态验收与 ThreadRegistry/owner/libruntime 仍属后续切面。

## 验证门

- host：ResultCell 四路交错（发布先/后于放弃、Incomplete）、T 恰一次 Drop 计数；Registry 并发 claim 唯一、seal 后拒绝新槽。捕获错误：双重 Drop、泄漏、双 claim。终局子切面已执行 `cargo test -p libprocess --lib --target aarch64-apple-darwin`，8 项通过。
- 内核 selftest：Start 调用链无分配；Created 线程被 Kill/Close 发布 DONE；Start/Seal/Kill/ThreadCreate 竞争线性化保持。
- QEMU：race 系列按 cause 判定通过；新增/改造用例覆盖放弃 PreparedThread、延迟领取、main 先返回 worker 后离场得 Completed、panic 得 Aborted、fault 得 Faulted detail。
- 结构：grep 确认无 `ExitCode`、`ThreadSpawn`、`ProcessAttach`、`ProcessExitReason`、abandoned 计数、safe `thread::exit` 残留。

## 删除清单

`ExitCode`、`ProgramResult`、`Termination::to_exit_code`、`ProcessExitReason`、snapshot `code` 字段、`ProcessExit`、`ProcessAttach`、`ThreadSpawn`/`ThreadSpawnResult`、lifecycle `Staging`/`Spawning` 态与 Start staged Vec、`rinlib::thread::{exit, JoinHandle::Drop 等待, UserStack abandoned}`、`wait_and_release`。

## 范围外残留

普通 MappedRegion/堆 arena 的运行期退休：当前调用者均显式 unmap 或进程寿命持有，无失败丢失。触发条件：出现需动态管理大量 mapping 的长期服务；届时在本记录续写或另立计划。

## 跨会话接力

终局原因切面已从 shared ABI 经内核、rinlib、libprocess 与现有监督消费者接通。ThreadCreate/ThreadStart 已接通 shared ABI、MANAGE right、ProcessBindExecution、Running 期挂起创建/启动、Building 目标控制输出和 Created 放弃离场；完整 srv_init 消费迁移及 Registry/owner/libruntime 分层仍未闭合；bootstrap 已复用 ThreadCreate 核心并交付 EntryThreadDescriptor v1。ProcessStart 普通路径已改为预留准入与嵌入发布链。开发证据为 `artifacts/dev/bootstrap-descriptor-check-final.log`、`artifacts/dev/bootstrap-descriptor-user-check-final.log`、`artifacts/dev/bootstrap-startup-test-final.log`、`artifacts/dev/bootstrap-libprocess-test-final.log`；完整用户态 target 验收尚未执行，直接检查 srv_init/test_hammer 受既有 feature/no_std 目标前提阻断。下一步进入 ThreadRegistry/owner/libruntime，接通 descriptor 一次采用与普通入口记录。
