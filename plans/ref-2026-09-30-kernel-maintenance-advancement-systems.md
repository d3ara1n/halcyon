# 协作式内核长路径维护操作推进系统参照

> 【只读参考资料】2026-09-30 为 Halcyon「不可抢占、无内核线程」约束下长路径无界操作（分配/回收/撤销）如何推进收集的外部系统事实。参照只用于拓宽选项，不替 Halcyon 作设计推导；取舍归 [内核有界维护执行重构](todo-2026-09-29-kernel-maintenance-execution.md)。
> 取证方式：官方手册/规范/源码/原论文核实（版本见来源清单）；标注「未核实」处为官方来源缺口。

## 问题定义与口径

约束：内核态不可抢占、无内核线程、不能长期占据 CPU；已提交的维护工作必须不依赖调用者继续调用而最终完成。对照维度：谁驱动推进、分块/续点原语、公平与优先、成本归属、失败边界（回收可否被拒绝）。

## 1. seL4（唯一同约束直接可比者）

版本：manual v16.0.0、TOCS'14、RTSS'11/EuroSys'12/APSys'12、RTAS'18、master 源码。

- **执行模型**：用户线程可抢占（256 级优先级 tickless）；内核执行期间其他线程不可进入（单内核栈、事件驱动）。IRQ 在内核执行期间禁用，仅在「carefully-placed interrupt points」处理：「On detection of a pending interrupt, we explicitly return through the function call stack to the kernel/user boundary... The interrupted event is restarted」——被中断的内核操作**回退后整体重启**（重查前置条件）。非抢占是 L4 传统，动因为平均性能与单栈低内存（TOCS'14 §3.4）。
- **incremental consistency**（原文）：「The exceptions are object initialisation, revocation, and destruction, whose operations are either inherently unbounded, or have large finite bounds. We make these operations preemptable by using an incremental consistency design pattern.」拆分方式**不是跨 syscall**，而是单次 syscall 内推进、遇抢占点返回用户态；进度存内核：「we store the state of progress either in the object itself, or in the last capability referencing the object being destroyed; we term such a capability a zombie.」
- **推进者继承**：「If a destroy operation is preempted by another thread invoking the same object, the preemptor will simply continue where the first thread was preempted (a form of priority and time slice inheritance)」；原线程重启后可能发现 cap 已失效而直接返回。
- 源码：CDT 子树删除在 `src/object/cnode.c` 为**迭代循环**（`finaliseSlot()` 的 `while` 循环：finaliseCap → reduceZombie → `preemptionPoint()`）；zombie cap 计数（`cap_zombie_cap_set_capZombieNumber(n-1)`）即持久化游标；`capSwapForDelete` 可把删除留给后续调用。
- **工作单位常数**：`preemptionPoint()`（src/model/preemption.c）累计 `ksWorkUnitsCompleted`，每 `CONFIG_MAX_NUM_WORK_UNITS_PER_PREEMPTION`（默认 **100**）个 delete/revoke 迭代检查一次 pending IRQ（MCS 下还查预算）；Retype fanout ≤ 256；untyped 清零按 2^8 字节 chunk 检查。
- **抢占点研究**：RTSS'11 在长循环末尾布点、遇抢占仍执行 ≥1 次迭代保证进度（防 livelock），据此删除 lazy scheduling、补插创建/删除路径布点，使最坏中断延迟降为**与对象数无关的常数**（实测 ARM Cortex-A8 closed 492µs / open 1.74ms）；EuroSys'12 实测案例系统最坏中断延迟 189,117 cycles（32 位 cap 逐位解码为主因），IPC fastpath 不可抢占；APSys'12 与全抢占 QNX 对照：seL4 最坏中断延迟 ~32k cycles vs QNX ~22k（1.5–2.2×）。
- **MCS 成本记账**：内核代线程消耗记在其 scheduling context 上；内核入口检查预算，不足则「假装定时器已到期」；不变量「any thread in the scheduling queues must have enough budget to exit the kernel」→ **调度精度 = 2×内核 WCET**；只在 SC 切换时结算。passive server：服务器无自身 SC，客户端经 seL4_Call 捐出 SC、经 reply cap 归还，可无限嵌套。timeout exception：预算尽被抢占且注册 handler 时发 IPC，handler 可给应急预算/永久加预算/降级/杀死/回滚。
- **无内核内延迟工作设施**（无 softirq/bottom-half/工作队列）；中断整体推给用户态（IRQHandler cap 绑 notification，用户态驱动收 badge 信号）。idle 线程为 `wfi` 停等，空闲时内核不做维护。

## 2. Fiasco.OC / NOVA / 经典 L4

- **Fiasco.OC**（源码）：内核常规执行**可被 IRQ 与高优先级线程打断**，仅短临界区持 `cpu_lock`（「must only be held for very short amounts of time」）；每 CPU 有 `Kernel_thread`；IRQ 是内核对象，命中时把自己排入绑定线程（以 IPC 交付）；mapdb 为带树锁的 mapping tree，unmap 一次性遍历，**无官方成本上界陈述**（未核实）。
- **NOVA**（spec 2014-06-23 + EuroSys'10）：EC 分「Kernel thread / Virtual CPU」两形态（指被调度的普通线程 EC，非内核内部工作线程）；中断不经 EC——每 GSI 一个 semaphore，内核直接 up；**revoke 在单次 hypercall 内递归完成，无分块/续点/成本陈述**；异常/拦截/VM-exit 全部记账到触发 EC 的当前 SC（SC 捐赠即 priority/bandwidth inheritance）；边界控制靠调用侧（SM down 的 TSC 超时、call 的 DB 非阻塞）；IPC 路径实测 79–199 cycles。
- **经典 L4**（X.2 r7）：recursive unmap 是 **API 语义**（「unmapped recursively in all address spaces…」），可按 r/w/x 位掩码部分撤销；MDB「conceptually each mapping consists of a tree of address spaces, however this tree is implemented as a linked list」；执行时间上界无官方陈述。Liedtke SOSP'95 立场：「implement outside of kernel whatever is possible」；内核仅 IPC/线程/地址空间。

## 3. Tock

- 内核胶囊事件驱动、协作式、不可抢占；进程可时间片抢占或协作运行。`kernel_loop` 每迭代：先服务 pending IRQ 与 deferred call（逐个、可让位新中断），再跑进程；进程执行中出现中断/deferred call 即从安全点退回主循环。
- deferred call 为**编译期固定 task 集合**，pending 至多每 task 一项（固定编号集合）。
- **分配即时拒绝**：无内核全局堆，grant/PCB 位于进程自己的固定连续 RAM 区顶部（进程经 MPU 不可访问），分配从该进程配额划出，失败清单明列。
- **终止同步一次完成**：`terminate()` 「frees grants and any queued tasks for this process」，无分批——有界的根源是内存按拥有区归属。upcall 不打断 Running 进程，入队待 yield 交付。

## 4. Hubris

- **构建期消除**：固定系统级资源分配；运行时无任务创建/销毁、无动态分配；任务表、内存区域、中断路由均为编译期常量，启动无解析无失败点。
- 重启由应用指定的 supervisor 任务决定（内核只发故障通知），与首次启动共用同一代码路径，generation 计数递增；重启延迟权在 supervisor 政策。
- 内核 strict priority 抢占、同级协作无时间片；算法复杂度按构建期因子缩放。

## 5. Exokernel（Aegis/ExOS, SOSP'95）

- 内核只做分配、回收、所有权与保护检查；CPU 表示为时间片线性向量，与物理内存同法分配。
- **visible revocation 对话**：内核发回收请求，libOS 自选资源让出；不服从则升级为命令（「50 µs 内归还」时限）；仍不服从则内核强行断开 secure binding、登记 repossession vector 并投递 repossession 异常；每 libOS 保底 5–10 物理页不可收回。官方明确拒绝「不响应即杀」（硬实时界不可推理）。
- 公平机制：后续 timer 中断记入 excess time counter，超阈值销毁环境（原型）或内核手工切换（设想）——限制上下文保存时间。

## 6. Genode（Foundations 26.05）

- **session close 由 parent 驱动、两阶段**：child 向 parent 发 close → parent 路由到 server → server 释放该 session quota 项下全部资源 → session quota 原路逐跳转回 client。失败边界：server 违约在抽回预算时被检测；处置属 parent 政策（杀掉 server 或垫自己的 quota）；server 顽固不关时 parent 可整体销毁之。
- cap 配额与 RAM 并行同法（`Ram_quota`/`Cap_quota`）；PD session 关闭销毁其下全部 dataspaces 并把终值预算转回 reference PD。
- **base-hw 内核**：core 与内核融为单一特权组件；内核是**单栈状态机**，永不阻塞、**内核态不做任何分配**（内核对象分配由 core 用用户态 resource trading 完成）；状态转换由 syscall/中断/异常触发，进入-改状态-退出；SMP 每核一个 kernel thread + 全局自旋锁串行化；回收在特权态同步执行，无分批机制。

## 7. Barrelfish

- 每核 CPU driver 单线程、不可抢占、关中断、无动态内存分配、无阻塞操作；「Each such handler executes in bounded time and runs to completion」。
- **O(1) 纪律明文**（TN-013）：内核操作不应超过 O(1)（个别 O(log n) 亦被认为过久）；「All non constant time operations should be punted to the monitor」。
- **monitor（用户态）驱动跨核回收**：跨核 capability 传递、retype/revoke 需全局协调，由 monitors 间两阶段提交保证全核一致排序；删除协议分步推进——`KernelCmd_Revoke_mark_target/mark_relations`、`Delete_step`、`Clear_step`、`Delete_foreigns`；monitor 与普通进程一同时间片（若 monitor 停止，工作停摆——内核不持有待办）。

## 8. Composite / Speck（RTAS'15）

- 内核无锁、非抢占、无内核线程；「All paths in the SPECK kernel have bounded execution time」——最长循环是清零一页；唯一 syscall 是 KOTBL 索引上的对象操作；争用时 CAS 失败即返回失败、不重试。实测最大开销：IPC 6024 cycles、cap 激活/去激活 8079/7464 cycles。
- **时间门控零状态回收**：去激活时取时间戳入对象头/LIVENESS TABLE；**不显式 free**——同槽位后续 `activate` 仅当静默期（自去激活起 `WCET_k + WCET_cas`）已过才成功；正确性前提正是内核非抢占（引用至多存活一个内核 WCET）。把「完成」转嫁给后续分配者的进入检查。
- TLB：每核私有时间戳于线程切换时自行 flush（第二静默期）；或由专用高优先级周期用户级 flush 组件推进（每次 flush 后立即阻塞）。
- 物理帧按 32 帧束 retype、per-core per-bundle 计数、全核计数为零方可 retype；稀有操作可隔离到专用核；fault 处理组件可用 `introspect` 清理崩溃 manager 的资源表；不变式：无内核内存泄漏（全部可自 boot 组件资源表传递可达）。

## 9. QNX（对照：可抢占路线）

- 微内核只含「short execution path」服务；「Operations requiring significant work (e.g., process loading) were assigned to external processes/threads」。
- **完全可抢占**：「The OS is fully preemptible, even while passing messages between processes; it resumes the message pass where it left off before preemption」——消息传递内部分段、抢占后从断点续跑。
- 进程创建/销毁/内存/路径名管理在 procnto（与 Process Manager 合一）；procnto 是多线程 resource manager，清理由其中**普通可抢占线程**推进，依赖优先级继承。

## 10. managarm / thor（对照：异步内核线程路线）

- 全异步微内核：几乎所有 syscall 异步，完成经无锁环形缓冲通知用户态；官方自称「务实」。
- **有内核 fiber（= kernel thread）**：初始化后半段在 kernel fiber 上运行；异步工作用 C++20 协程（libasync），每线程有 `mainWorkQueue`，异步提交挂 detached coroutine 续作；可 `asyncBlockCurrentInterruptible` 挂起。
- 调度按「不公正度」u(t) 选线程，时间片 = u(t) 差 + 粒度常数（如 10ms）。内存/capability 回收如何推进无官方文档（未核实）。

## 11. 跨系统对照表

| 系统 | 推进者 | 分块/续点原语 | 公平与优先 | 成本归属 | 失败边界 |
|---|---|---|---|---|---|
| seL4 | 调用者 syscall 内；遇 IRQ 回退重启；同对象新调用者继承 | 每 ~100 work units 检查一次；进度存对象/zombie cap | 抢占者继承（优先级/时间片继承） | 记发起线程（MCS 扣 SC，入口查预算） | 删除不可拒绝 |
| Fiasco.OC | 调用者线程（内核可被打断） | 无续点；mapdb 一次性遍历 | 静态优先级抢占 | 归调用线程 | 未核实 |
| NOVA | 调用者 hypercall 一次完成 | 无分块 | 优先级抢占 + SC 捐赠 | 全部记触发 EC 的 SC | 调用侧超时 |
| 经典 L4 | 调用者 UNMAP 一次递归完成 | MDB 为进度载体，无续点 | 静态优先级 | 归发起线程（未述时限） | 未核实 |
| Tock | 内核主循环同步完成 | deferred call 固定集合、逐个可让位 IRQ | 内核工作优先于进程 | 无记账（静态配额） | 分配即时拒绝；terminate 不可拒 |
| Hubris | 构建期消除；supervisor 驱动重启 | 无需分块 | strict priority | 无（静态） | 无分配可失败 |
| Exokernel | libOS 响应；内核限期后强制 | 对话 + 时限（~50µs）+ repossession | excess time counter 限保存时间 | 内核只做检查 | 请求可无视；强制不可拒；保底页不可收 |
| Genode | parent 编排、server 执行、预算回流 | 无分块；两阶段协议 | quota 即准入权 | server 侧 quota 记账 | server 可拖延但可检测；parent 处置 |
| Barrelfish | 用户态 monitor 分步驱动 | 两阶段提交 + step 内核命令 | monitor 与进程同时间片 | 无 | 非常数操作被挡在内核外 |
| Composite/Speck | 时间门控：再分配者进入检查 | 时间戳 + 静默期；32 帧束 | 无锁争用即失败 | WCET 常数上界 | retype 可拒绝（计数非零） |
| QNX | procnto 普通线程推进 | 消息传递内部分段续跑 | 优先级抢占（1–255） | 归执行线程 | 内核调用不可拒绝 |
| managarm/thor | 调用线程协程续作 + 内核 fiber | 异步提交/完成分离 | u(t) 公平 + 粒度常数 | 归调用线程 | 未核实 |

谱系归纳（事实层面）：回收推进共五类——① 调用点同步完成（Tock、Genode base-hw）；② 安全点/主循环分批（seL4 incremental consistency、Tock deferred calls、Speck 周期组件）；③ 用户态服务承担（Barrelfish monitor、Exokernel libOS、Hubris supervisor、QNX procnto、Genode parent 协议）；④ 构建期消除（Hubris）；⑤ 零状态时间门控（Speck：完成转嫁给后续分配的进入检查）。

## 12. 与 Halcyon 维护执行计划的对照要点（事实性）

以下为外部事实与 [维护执行计划](todo-2026-09-29-kernel-maintenance-execution.md) 现有文本的对应关系，供施工前设计门参考：

- **已被多系统印证的计划选择**：「责任产生前准备、队列挂接不分配、最后 capability 释放不得临时申请工作槽」↔ seL4 zombie 把进度存进垂死对象（同一原则的无队列实现）；「普通新建可在准入拒绝、合法 Close 不可拒」↔ Tock 边界划分一致；「可运行维护不得因预算耗尽永久入睡」↔ RTSS'11 遇抢占仍执行 ≥1 迭代的反活锁保证；「先减少总工作（整拥有体释放、进程整体退休）」↔ Tock/Hubris 证明内存按拥有区归属时清理天然有界；「延迟清零成本计入再分配」↔ Speck 时间门控是该思想的系统化版本。
- **可进入设计门权衡的三个已验证机制**：① work-unit 常数计数（seL4 默认 100/检查点）——比按对象规模分批更可测量；② 进度载体放对象内（zombie）vs 放独立队列——无独立队列路线的成熟先例；③ 维护执行成本记账到来源（MCS 扣 SC + 入口预算检查）——计划以「准入约束存量」约束未完责任的数量，未约束执行消耗，二者互补。
- **Halcyon 独有承诺（调研范围内无系统具备）**：「无 runnable 用户线程但有 Ready 维护：返回循环边界继续」——已提交维护在无调用者、无用户线程时继续推进。seL4 zombie 等下一个触碰者/原线程重启；Barrelfish 等 monitor；Speck 不推进只延迟复用；Tock/Genode 在调用点同步完成。该差异是「活性不依赖调用者继续调用」的实质内容。
- **可测量性要求**：seL4 的抢占点方案经 RTSS'11/EuroSys'12 两轮实测（最坏中断延迟降为常数、量化 32k cycles）才成立；计划「每轮预算」同样需要落到可测的 WCET/中断延迟证据；MCS 不变量「调度精度 = 2×内核 WCET」给出把维护上界与调度精度挂钩的先例。

## 参考来源清单

- seL4：manual v16.0.0（sel4.systems/Info/Docs/seL4-manual-latest.pdf）；TOCS'14（sel4.org/Research/pdfs/comprehensive-formal-verification-os-microkernel.pdf）；RTSS'11 nicta 4863；EuroSys'12 nicta 5391；APSys'12 nicta 5859；RTAS'18 Lyons_MAH_18.pdf；seL4/seL4 master（config.cmake、src/model/preemption.c、src/object/cnode.c、src/kernel/boot.c、src/arch/arm/64/idle.S）
- Fiasco.OC：github.com/kernkonzept/fiasco master（src/kern/{cpu_lock,context,kernel_thread,irq,mapping,kobject_mapdb}.cpp）
- NOVA：github.com/udosteinberg/NOVA doc/specification.pdf（2014-06-23，§2–§5）；hypervisor.org/eurosys2010.pdf
- 经典 L4：l4ka.org/l4ka/l4-x2-r7.pdf（§4.2）；trustworthy.systems/publications/papers/L4UM_x.2.pdf（§7.2）；os.itec.kit.edu/downloads/publ_1995_liedtke_ukernel-construction.pdf
- Tock：book.tockos.org/doc/{processes,scheduling,design}；docs.tockos.org/src/kernel/{scheduler,deferred_call}.rs；docs.tockos.org/kernel/process/trait.process；github.com/tock/tock master kernel/src/process.rs
- Hubris：hubris.eng.oxide.computer/reference；github.com/oxidecomputer/hubris FAQ.mkdn
- Exokernel：pages.cs.wisc.edu/~bart/736/papers/exo-sosp95.pdf（§2–§5）
- Genode：genode.org/documentation/genode-foundations/26.05/{architecture/Resource_trading,under_the_hood/Execution_on_bare_hardware_(base-hw)}.html；github.com/genodelabs/genode repos/base/include/base/child.h
- Barrelfish：barrelfish.org/publications/{barrelfish_sosp09,TN-000-Overview,TN-013-CapabilityManagement}.pdf
- Composite：faculty.cs.gwu.edu/gparmer/publications/rtas15speck.pdf（§IV–V）
- QNX：qnx.com/developers/docs/8.0 与 qdn.qnx.com/developers/docs/7.1（sys_arch：kernel_SERVICES、proc；resmgr/multithread）
- managarm：docs.managarm.org/handbook（impl-notes/thor/{initialization,scheduling}、sys-arch/libasync）；archive.fosdem.org/2022 managarm slides；github.com/managarm/managarm master

## Gaps（未核实/存疑）

1. Fiasco.OC：mapdb unmap 最坏成本上界；无官方长操作陈述。
2. NOVA：内核执行期间 IRQ 可否打入；revoke 无分块/成本陈述。
3. 经典 L4：UNMAP 执行时间上界；L3 原始资料未检索。
4. managarm/thor：内存与 capability 回收的推进方式无官方文档；「worklet」术语未核实。
5. Genode base-hw：特权态同步回收无上界表述。
6. Tock：「pending 至多每 task 一项」为源码机制推断，官方文档未明文。
7. RTSS'11/EuroSys'12/APSys'12 延迟数值依测量平台（ARM Cortex-A8 等），不可直接移植到 RV64 平台。
