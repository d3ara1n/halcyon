# 内核内部机制

内核边界与协作式执行方向见 [`../ideas/kernel.md`](../ideas/kernel.md)；本篇记录全局状态、锁、中断、唤醒和停机的当前实现。

## 全局状态分层

按归属与访问频率分三层管理状态，目标是热路径无锁，而非处处无锁。

| 层 | 归属 | 访问方式 | 典型内容 |
|---|---|---|---|
| hart 私有 | 单个 hart | tp 指针，无锁 | 执行点（当前线程、域指针、trap 锚）、hart 状态 |
| 对象私有 | 进程/线程 | 所属对象的锁、Arc 引用计数 | 内存布局、邮箱、子进程列表 |
| 全局 | 全系统 | `OnceLock`、原子终态与 Spinlock 粗锁 | RuntimeGate、调度域、帧分配器、identity allocators |

当前冷路径使用粗粒度锁；调度域内公平类是共享单锁 FIFO。`os/runtime_gate` 是无锁的全局启动终态，只允许 `Preparing -> Ready | Failed`，具体发布边界由 [`execution-context.md`](execution-context.md) 记录。per-hart timeout queue 已按唤醒所有权拆分，HandleTable 与对象状态仍由各对象锁保护。

就绪队列不在 hart 私有层——它是调度域的共享容器（结构见 `task.md`「调度」），同域 hart 经域容器锁竞争；队列结构与策略封装在调度类内可替换。

## 内核中断模型

trap 进入 S 态时硬件清 SIE；内核执行到用户返回或调度出口期间不接受嵌套中断、没有内核抢占或可挂起内核线程。调度只发生在用户 trap 返回路径上。该结构的方向理由见 [`../ideas/kernel.md`](../ideas/kernel.md)「协作式内核」。

具体纪律：

- U 态下 `sstatus.SIE` 的值被忽略；已委托且在 `sie` 中启用的来源可以陷入 S 态。S 态稳态 SIE=0，内核期间到达的来源由有界安全点服务；尚未消费的 pending 在返回 U 态后按规范重新 trap。S 态同步异常仍走 fatal 入口（转储停机）。
- 内核态 timer 长路径不会「误杀」：悬置中断在 sret 后才生效，调度点照常到达。
- 内核锁只需防跨核争用；锁原语的关中断语义在此模型下是零成本保险，保留以防纪律腐化。
- 阻塞的表达方式是异步 syscall（见 `call.md`），不是内核睡眠——协作式定性下不存在可挂起的内核执行流。

## SBI 平台基线

内核要求 SBI 2.0 或更高版本，并把 TIME、IPI、HSM、DBCN 作为必需扩展；SRST 是可选平台后端，不支持时不阻止启动，显式系统复位请求会向用户态返回 `NotSupported`。版本号和必需扩展在启动期通过 BASE 探测，不从机器型号推断。

DBCN 名称中的 Debug Console 仅表示 SBI 扩展类别，在 eRhino 中只承载内核日志和 Debug syscall 的观测输出，不是功能模块。输出为单次非阻塞 best-effort：部分写、零写或错误都允许丢失，任何日志失败均不得改变调度、IPC、内存管理等功能路径。legacy putchar 只在 DBCN 尚未就绪时提供早期 best-effort 诊断，不是运行期兼容层。

## 唤醒所有权

**每个唤醒必须有主人，无主的周期性唤醒（保险 timer、心跳）不存在。** hart 敢睡，当且仅当「自己的期限已上闹钟，或别人的请求会按门铃」：

- **timer = 自己的确定到期点**：Thread出生内嵌稳定pin节点，per-hart侵入AVL按期限/地址排序，载荷持Thread/epoch。到期先摘链再门外提示，由执行owner复检完成；跨hart取消只摘匹配epoch，不远程编程timer。
- **IPI = 他方的请求**：SBI只置SSIP，Ready/Pending电平拥有真实责任。发送成功与合并门铃下由idle登记/复检握手保证重新读电平；规范允许发送失败，失败进入下述不可恢复停止政策，不能只凭发送方尚醒就推导目标一定推进。
- **设备中断 = 设备的请求**：PLIC 将已授权 source 交固定 context，内核记录领取，实际驱动负责检查硬件并 Ack；[IRQ owner](device.md#irq-使用观察与撤销)拥有消费、撤销和观察生命周期。

正常唤醒后清 SSIP 门铃，读取调度域 Ready、期限与运输 Pending；外部 IRQ 在睡前/醒后最多 claim 一源，claim 不等于 complete。无工作回睡。若 runtime_stop 已 requested，先进入停止检查，不再挑选工作。

### idle 与系统复位

idle 只表达「当前 hart 暂无可运行线程」：hart 在所属调度域登记 idle 位，双重检查 Ready 后进入 WFI；入队方据 idle 位选择 IPI 目标。idle 位是调度唤醒路由状态，不参与整机生命周期推断。Sleep 与有限 WaitOne 的唤醒主人仍是 per-hart TimerQueue entry。

显式平台终局由SystemReset对象触发；内部不可恢复clock/门铃故障由runtime_stop协作停驻。SystemReset是无等待叶子对象，bootstrap交primordial capability给init，调用要求MANAGE；自有ResetAction/ResetReason显式映射SRST，不透传SBI编码。

对象内 `in_flight` 原子门保证全系统同一时刻至多一个平台请求；竞争返回 `ObjectBusy`。SBI `NOT_SUPPORTED` 映射为 `NotSupported`，其余后端拒绝及规范所称成功调用的异常返回映射为 `InternalError`；失败会释放门，内核不以永久停放伪装 reset 成功。

### IPI 门铃与 remote call

IPI只置目标SSIP，不携载荷；[vm/remote.rs](../../os/kernel/src/vm/remote.rs)的每hart固定槽与Pending电平拥有运输责任，目标在trap/调度安全点有界消费。Ready、maintenance、Freeze与remote通知共用runtime_stop::notify_slots：先发布责任，raw发送失败则Release request，不在可能持锁的调用栈中park；停止广播自身使用raw发送避免递归。

runtime_stop的REQUESTED是不可逆全局事实，继续运行的U有有限硬件quantum，M/syscall有结构性工作界；主循环、trap锁外与U准入边界Acquire复检。无期限WFI本身不执行责任，后续任意唤醒先check再调度。若平台持续不能送门铃，允许该hart保持WFI，不把PARKED当全admitted完成barrier，不等它参与退款。首个STOPPED转换者只报告一次，已进入检查的hartRust park；终态资源留至重启，不保证撤销既发设备/DMA。

## tp 寄存器

内核态的 tp 不是 thread pointer，是 hart pointer。每个 hart 启动时设置 tp 指向自己的 HartLocal 结构，此后保持不变量：

**内核态运行期间，tp ≡ 当前 hart 的 HartLocal 地址。**

推论：

- trap 进出与上下文切换必须保存/恢复 tp（tp 即通用寄存器 x4，用户态用它做 TLS，内核态占用它）
- HartLocal 按 cache line 对齐；跨 hart 交互只能走全局层或 IPI，HartLocal 严格私有
- 数组包装处唯一一处 `unsafe impl Sync`，SAFETY 注明上述访问不变量

## 地址空间

当前系统选择 Sv39；用户低半区、共享内核高半区、栈窗口、uaccess 与页表所有权由 [`mm.md`](mm.md) 唯一记录。uaccess 对用户 VA 的直接 copy 只存在于持 AddressSpace 锁的 `SumGuard` 临界区；guard 的 `csrs/csrc sstatus.SUM` inline asm 刻意保留编译器内存 clobber（只声明 `nostack`，不得声明 `nomem`），保证 release 优化后用户内存访问仍严格位于 SUM 开启与关闭之间。调度侧非 Resume trap 出口切回正式内核 root 后才清 active，详见 [`execution-context.md`](execution-context.md)。

## 执行上下文与 hart 能力

bootstrap、HartId/HartSlot、现代 DT capability、共同 trap、CSR、UserContext/FP、调度域和指令发布的统一契约见 [`execution-context.md`](execution-context.md)。

内核态运行期间 tp 指向当前 HartLocal；正式 sscratch 恒指同一 HartLocal；SPP 是 trap 来源的唯一真值。U 态忽略 sstatus.SIE，内核以 `sie` 精确选择 SSIP/STIP 等来源，S 态协作式执行期间 SIE 恒为 0。

无 MMU hart 不进入当前共享高半区内核，也不属于 admitted hart 集合；当前没有 AMP runtime。

## 锁原语

内核自研三件：

- **`RawTicketLock`**：FIFO 发号，serving Release 移交所有权；接班读取 serving 用 Acquire，Try 只在无排队票时预留下一票，并获取前持有者的 Release 写入。裸原语不管理中断；`Spinlock` 与 allocator 的入口均先屏蔽本 hart IRQ，保证每 hart 每锁至多一个在途票。等待 owner 数至多 H-1，临界区内数据操作及更高门仍须独立计界；
- **`Spinlock<T>`**：内核容器锁，获取期间关本地中断（sstatus.SIE 清零）。关中断是正确性要求而非优化——中断处理函数若获取本 hart 正持有的锁，同核死锁；构造点声明锁序 rank 参与 Lock Ladder 断言（机制与秩表见 `task.md`「锁序契约」）；
- **`RankedRawSpinlock<const RANK>`**：同一 RawTicketLock 实现 `lock_api::RawMutex`，供 talc 注入；获锁前保存/屏蔽本地 IRQ，持有者唯一写 saved 状态，解锁后恢复。Guard 为同 hart、不可 Send，trait 路径参与 ladder。

睡眠锁与协作式定性不相容（内核无挂起的执行流，见「内核中断模型」）；长等待的表达方式是异步 syscall，不是内核睡眠。

## 堆与物理帧

分层管理三种语义不同的资源：

- 物理供给（os/memory_supply，固定 workspace planner）：启动期按 permanent、boot-held、system、user-free 分类；FramePool metadata、16×1 MiB heap chunks 与当前为零的 recovery 子预算以不同 ticket 类型隔离，失败不发布部分计划。
- 用户帧库存（os/frame_pool，外置元数据分级order树）：只发布user-free与生命周期结束的boot-held；步骤由固定arena数与地址位宽限定。普通claim锁外清零后commit来源Pool charge；页表root持funded owner，中间表adopt/reclaim把PTE物理身份与空间聚合charge配对；保留内容的BootHeldExtent只从验证后的启动移交点adopt。具体所有权与退款由[mm.md](mm.md)拥有。
- 内核堆（talc）：管任意尺寸小对象；`SystemSource` 在 allocator 忙碌时只从独立 `SYSTEM_SUPPLY` O(1) 消费一个启动期预清零的heap ticket，相邻最近arena时extend、非相邻则独立claim，end仅在allocator锁下更新；不扫描/清零，不进入用户FramePool或recovery，完整供给契约由[mm](mm.md)拥有。Talc 找不到可用块且无法再取得 ticket 时普通 metadata 分配 OOM；对象级 admission 是独立机制。启用 talc `counters` 后，Ready 建立采样基线，用户 trap 出口、成功 Bind 与获得 SystemReset authority 的 reset 前读取 claimed/allocated/available/fragment 计数并更新原子采样极值；复制快照并释放 HEAP 锁后才读独立的 system ticket 计数并写日志。`chunks` 是已永久 claim 的数量，不等于耗尽的堆；采样极值不覆盖单次 syscall 内短暂峰值。`source_failures` 与布局只记 SystemSource 取不到 ticket 或 claim 失败的请求；这些计数和 aggregate available 均不提供 allocator 最大连续可分配块。

三层的碎片域、锁与失败边界彼此独立。planner 使 FramePool metadata 和堆供血都不再反向依赖用户库存，从物理所有权上消除“库存元数据由自身供血”及 HEAP→POOL 的运行时环。

## 进程容器与 PID

内核没有全局进程表：未 Dead 进程的生命周期根是 Job 直接成员表（`MemberEntry::Process(Arc<ProcessShell>)`，`task/job.rs`），root Job 由内核 static anchor 强持。PID 与 JobId 分别由 `monotonic_id::AtomicId64` 的独立 domain 分配，最大值最后发行一次后永久 Exhausted，绝不回绕复用；它们不构成全局操作入口。PID 是所属 Job 直接进程成员表的键，JobId 是直接 child Job 表的键；两者也作为 JobDerive 选择子和 provenance 诊断值。所有权图与成员表机制见 [`task.md`](task.md)「Job、Building process 与发布」。
## Commit 后债务账本

[`maintenance.rs`](../../os/kernel/src/maintenance.rs) 承载 WorkRecord、付款 Account DRR 与 per-hart 用户/维护轮转。记录/队列同受 MAINTENANCE ticket 锁保护，业务 step 在锁外。来源活动重复激活：Idle 接管新强根，Queued 合并，Running 置 notified；finish(Idle) 若 notified 则回 Queued，否则回 Idle，本轮强根锁外 drop，不触及已新激活的一轮。Done 永久终结，没有额外 Quiescent 状态。

Queue拥有本轮/下一轮两个账户环，各有强cursor与强next、弱prev；Account一组链接和所属ring身份，Ledger拥有started、i128余额、剩余机会与in_flight。本轮空才切役，环外新激活进下一轮，已有成员只追加Ready。每次访问一个本轮账户：仅首次补q tick及q次机会，正信用且有机会才预扣cap=ceil(q/HART_NUM_LIMIT)，每grant消耗一次机会。退款归原轮、在途不重发信用；全部grant结清且费用或机会耗尽才结束轮次移next，封轮丢正保债。真正无Ready/在途才idle摘环。选中强根、旧链接与记录最终析构都在QUEUE外；finish后再激活可和旧grant尾部重叠，最后settle负责是否移环，Blocked责任不必DONE才退执行环。持续0费用也在有限grant数后封轮，不强收虚构tick；在途gate仍可暂时等待其他hart，未承诺占满所有空闲CPU。

per-hart Execution唯一持有Rotor与U起点；政策量子U10ms/M2ms、账户100µs按平台Geometry的唯一向上取整换算。Rotor每轮q tick及q次机会，选Run/同轮Resume各消费一次，charge仅计实际费用。选定U类后UserDispatchScope覆盖线程选择失败、等待续体、准入失败/reap、用户现场、syscall及最终owner释放；失败选择结束scope后重新仲裁，不带U计时idle。准备结束先扣U时间，再打开U段装填timer；类别费用截止是活动段起点+原余额，装填的时钟/锁/SBI成本也归U，不从装填时刻延长额度。已领取的最后机会可完成本段，耗尽只拒下一领取。trap暂停处理紧急服务，Resume须取得原轮下一机会且线程绝对量子未到期；重新计时和重装timer不补费用/机会/线程量子。remote/timer到期项紧急服务与未选类别的共享仲裁不归U/M；费用或机会耗尽的Resume改Requeue，各dispatch出口scope关闭尾部。M每安全点至多一个账户/原子，空pick/欠账/在途仲裁同属该次M机会并扣完整raw elapsed。无另一类可运行时继续借用，正信用不跨轮积攒。领域step只返回Outcome，超出记债，量子不宣称执行上界。Ledger与Rotor用i128 checked算术，epoch/hart在途界由kernel提供，不饱和截断。

当前费用来源为clock::now_ticks，即平台TIME墙钟；始末同值仍全额退预扣，机会门独立保证换出。每次机会至少产生一tick费用时，费用先于机会门耗尽；机会先耗尽的轮次只保证有限推进，不声明原CPU份额偏差界。TIME/默认QEMU区间不是精确物理CPU时间，可靠执行计量的独立平台条件由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md#执行计量前置保留目标不以调度补丁替代缺失平台合同)拥有；没有未接通meter facade或最低伪费用。

Account最后析构先take父强根，收束自身metadata/付款Pool、旧欠账移交和等锁；M区间的实际清理时间直接charge父Ledger，同QUEUE与父在途grant线性化，不补信用/伪造grant。父保活使其析构在本层测量终点后独立发生。每hart独立原子M类别事实与累计已归父cleanup，Dtor不重借Execution；grant用其始末差扣除已经直接计入父的时间，M批仍计完整区间。非M清理不向M账本注入新费用，U dispatch中的清理由U区间涵盖；明确排除的紧急/仲裁开销沿共享边界。结束采样后的固定记账/解锁、Account T析构后的Arc弱尾deallocation为显式共享尾部。成本诊断仅发布编译器静态类型名字和原子最大值，不分配/保活Work；观测最大值不证明WCET或账户公平。

Work使用出生内嵌Record，step接收Arc<Self>：执行器原根保留到finish结算后的门外析构，receiver clone支撑领域arm；receiver Drop不是最后引用。Blocked根在Record字段，Running根在执行器局部。依赖经arm→登记→复检转WakeToken。publish合并通知及wake活跃记录的多余Arc在QUEUE门内归还：Queued/Running/Blocked的执行根由同一门保护，此处只执行非最后引用计数。wake对Idle/Done返回潜在退休根，调用方带出全部领域门后销毁，kick亦在门外。Work::record只借稳定内嵌字段，不取业务锁、分配或回调。Job的有界封口、Change/Retire/Builder/Shell共用该协议，不需要自Weak升级锁。运输FIFO由vm::remote拥有，VM领域owner见[mm](mm.md)。

完整M原子包含队列领取、step、finish、最后执行根析构、Account父归账与settlement。业务数量由各Work的有限单元界定；AVL/slot容器依实际付款容量计高阶成本，Bootstrap准备还包含独立heap gap搜索与页/metadata准入。空私有数组收束先清业务值、最后只释放真实allocation；多份消息authority依次释放，费用和父根析构计入当前原子，量子不是执行时长上界。源码入口见[maintenance](../../os/kernel/src/maintenance.rs)、[reclaim](../../os/kernel/src/task/process/reclaim.rs)及[ChangeWork](../../os/kernel/src/vm/change.rs)。

完成回执的内部DONE/internal_done与wake状态提交在同一SignalSource门内完成，等待者不能先观察新完成事实、finish并释放执行根，随后才让发布者归还其凭证。只由Shell使用的私有wait_done保存单一依赖责任；来源门内没有Hint或业务回调，返回的潜在退休根与kick都在门外处理。VM start/quiescence分别登记Builder/Shell；它们可在状态门外承担真实最后析构，不凭持有VmShell Arc推断保活其他宿主。Shell在DropBody前已取走退休来源，最后析构不再返回Retire。宿主费用、access/ingress回执和DONE的顺序由[task](task.md)、[mm](mm.md)拥有；格式化/冷诊断不承担业务进展。
