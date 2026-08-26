# 任务模型实现

> 任务是什么、进程与线程的分工等概念层见 [`../ideas/task.md`](../ideas/task.md)；本篇记录其在内核中的落地。线程持有 UserContext（用户现场，每线程一份）与调度状态；进程持有资源和从 ELF 得出的执行能力需求。完整上下文契约见 [`execution-context.md`](execution-context.md)。

## 用户地址空间布局

低半区 `[0, 2^38)`，无 trampoline 与隧道区——共享内核映射后用户半区完整归用户：

```
[0, brk')             ELF 段（text/rodata/data/bss，LOAD 段原样映射）
[brk', block_end)     StartupBlock（只读，launch 映射；见 startup.md）
[block_end, 栈区底)   堆，Extend 向上扩展，逐页映射
[栈区底, 2^38)        栈区：主线程栈 8MiB 钉在半区顶，未来线程栈向下生长
```

- 堆扩展（Extend）从当前 brk 起逐页映射，返回新 brk；brk 基点在 launch 时越过启动块，块与堆结构性互不重叠。物理连续性不做要求，虚拟连续性由「从 brk 起步映射」结构性保证。
- 主线程初始 sp = `2^38`（栈顶，16 字节对齐）；a0 = StartupBlock 基址、a1 = 块字节数是入口参数（rinlib 启动契约，见 [startup.md](startup.md)）。
- 用户 tp 置 0（rinlib 未用 TLS；引入 TLS 时再定义 ABI）。
- ASID 恒 0，地址空间切换时 `sfence.vma` 全量冲刷；ASID 分配（sv39 仅 9 位，需复用策略）作为优化留待演进——启用时同步引入 remote call 的 TLB shootdown 消费者。

## 调度：域—类—执行点三层组合

```
执行点（每 hart 一份，HartLocal）        调度域（共享）                    调度类（策略容器）
┌─────────────────────────┐        ┌──────────────────────┐      ┌────────────────────┐
│ current: Option<Thread> │        │ SchedDomain           │◀─────│ trait SchedClass    │
│ domain: &SchedDomain    │───────▶│  classes: 优先级序数组 │      │ enqueue / pick /    │
│ （调度循环 + idle 循环）  │        │ （M3: [Fair] 单类）     │      │ has_ready           │
└─────────────────────────┘        └──────────────────────┘      └────────────────────┘
```

- **执行点**：hart 的运行现场（当前线程、所属域、trap 锚），见 `internals.md`「tp 寄存器」。调度循环与 idle 循环是执行点的行为。
- **调度域**：一组能力兼容且策略相同的 hart 共享的调度类层次，hart 经 HartLocal 指向所属域。硬件 capability 是准入事实，domain 是 capability 与调度策略的派生对象；能力需求不是调度 class。线程只归属一个 compatible domain，跨域迁移显式转移队列所有权。域内类按优先级序查询，先到先得。
- **调度类**：一类线程的就绪容器 + 选择策略。实现整体可替换（轮转队列 / 无锁队列 / 窃取），加优先级类 = 向域的类数组插项——扩展是横向加项，不改结构。

时间片为固定量子，tickless：调度循环每次新 dispatch 前调用 `arm_quantum`，Resume 热路径不重置量子；同时取全局期限表最早项与量子截止的较近者设置本 hart timer。公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。

ProcessWrite 可由其他 hart 通过物理直映射填充新可执行帧。当前没有代码代次与 active-hart 集合，因此调度循环在每次新 dispatch 前执行本 hart `fence.i`；首次执行和迁移都不会观察帧复用前的旧指令缓存。Running process 没有 Building 写入口，Resume 热路径无需重复同步。

### 单一归属不变量

任意线程任意时刻恰处于一个归属：

```
某类队列（Ready） ｜ 某 hart 的 current（Running） ｜ 无容器（Waiting/Dead）
```

容器成员资格是状态真值，`Thread.state` 只是镜像；全部转换经调度器入口（`enqueue` / `pick` / `wake`）在锁内完成，锁序单向：期限表锁 → 类锁。

## 线程状态

`Ready / Running / Waiting / Dead`。Waiting：线程不在任何容器，等待其登记的内核请求完成；请求完成时 `wake()` 直接回 Ready——结果已写入 TrapFrame，无中间态。

### 等待的所有权与仲裁

- **强引用随容器走**：线程的 Arc 恰由其所在容器持有——就绪队列、执行点调度循环、或等待条目。等待条目强持有等待中的线程；不存在从容器反向到线程的长期指针（进程不回指线程），退出回收的 Drop 链因此能真正释放帧。
- **发布时序**：「可被唤醒」严格晚于「离开一切 hart 引用」——dispatcher 只把等待意图写入 HartLocal 私有槽，调度循环在 `clear_context` 之后的 Park 分支才向全局等待结构发布。完成方永远见不到仍在本 hart 执行的线程，双容器竞态在结构上不可能。
- **代数仲裁**：每次阻塞创建独立 `WaitContext/WaitCore`，状态为 `Installing → Armed → Finishing → Done`。对象命中、deadline 与取消候选通过同一个 outcome 竞争；唯一赢家取得线程所有权并负责跨对象清理。期限表强持同一 WaitContext，过期扫描只 offer Deadline，不存在每线程 `wait_gen` 平行机制。

## Job、Building process 与发布

root Job 由 `boot.rs` 铸造，完整 JobControl 作为 init 的启动 Handle。JobCreate 派生层级；ProcessCreate 必须持 CREATE，生成空 AddressSpace、HandleTable 与 affine ProcessBuilder。Building process 不进入进程表和调度器，关闭 builder 直接释放半成品。

ProcessStart 在提交前预构造主线程，并分别在进程表与公平类队列放入 reservation marker；marker 不参与查找、pick 或 `has_ready`。GRANT/StartupBlock/output 全部准备成功后，提交只替换预留项，不分配。PID 单调不复用，`parent_pid` 只供诊断，授权仅来自 Job/Process capabilities。完整事务见 [`startup.md`](startup.md)。

> 演进点：marker 预留（reserve/commit/rollback）当前由公平类的自由函数承载，不在 `SchedClass` trait 契约内。接入 D64 eligibility 时，ProcessStart 的发布必须按线程执行需求路由进兼容域——届时需把预留语义上收为所有类实现的接口，或由域层提供统一的预留通道；不得把现有自由函数当稳定接口直接跨域复用。

当前 ProcessControl 随 Running process 绑定，进程回收后保留轻量 CLOSED/exit-code 壳供 WaitMany 观察；关闭 control 不终止进程。显式 kill 与状态查询尚未接入。D64 profile 在 capability-derived 调度域接线前明确拒绝，避免线程落到不兼容 hart。

## 生命周期

- **创建**：唯一 init 由内核从 BootPackage initial ELF 构造；后续进程由用户态 `libprocess` 驱动 ProcessBuilder 映射、回填并发布。内核不解析 initfs 或服务拓扑。
- **退出**（Exit syscall / 用户态页故障）：当前线程不再入队 → 调度循环回收：摘进程表 → Drop 地址空间（表帧随页表树 Drop、数据帧 RAII 归还）→ ProcessControl 发布 CLOSED。当前一进程一线程，因此切换点已保证无其他 hart 触达该地址空间；ThreadSpawn 前必须增加线程成员表、active-hart 收束与远端 TLB invalidate/ack，不能延用该前提。
- **用户态页故障一律杀进程**：本内核无按需分配，所有区域创建时显式映射，fault 即程序缺陷。打印诊断行（pid / sepc / 故障地址 / 操作）后走退出路径，绝不 panic 内核。

## sleep

第一个异步系统调用（模型见 [`call.md`](call.md)），用于验证整条异步通路：ms > 0 时登记期限后线程转 Waiting；期限到达由 timer 唤醒 `wake()` 回 Ready，sret 后 a0 = NoError。期限表全局共享，登记时由发起 hart 立即 arm 自己的 timer（唤醒所有权，见 `internals.md`）。
