# 内核内部机制

内核边界与协作式执行方向见 [`../ideas/kernel.md`](../ideas/kernel.md)；本篇记录全局状态、锁、中断、唤醒和停机的当前实现。

## 全局状态分层

按归属与访问频率分三层管理状态，目标是热路径无锁，而非处处无锁。

| 层 | 归属 | 访问方式 | 典型内容 |
|---|---|---|---|
| hart 私有 | 单个 hart | tp 指针，无锁 | 执行点（当前线程、域指针、trap 锚）、hart 状态 |
| 对象私有 | 进程/线程 | 所属对象的锁、Arc 引用计数 | 内存布局、邮箱、子进程列表 |
| 全局 | 全系统 | `OnceLock` + Spinlock 粗锁 | 调度域、帧分配器、PID/JobId 分配器 |

当前冷路径使用粗粒度锁；调度域内公平类是共享单锁 FIFO。per-hart timeout queue 已按唤醒所有权拆分，HandleTable 与对象状态仍由各对象锁保护。

就绪队列不在 hart 私有层——它是调度域的共享容器（结构见 `task.md`「调度」），同域 hart 经域容器锁竞争；队列结构与策略封装在调度类内可替换。

## 内核中断模型

trap 进入 S 态时硬件清 SIE；内核执行到用户返回或调度出口期间不接受嵌套中断、没有内核抢占或可挂起内核线程。调度只发生在用户 trap 返回路径上。该结构的方向理由见 [`../ideas/kernel.md`](../ideas/kernel.md)「协作式内核」。

具体纪律：

- U 态下 `sstatus.SIE` 的值被忽略；已委托且在 `sie` 中启用的来源可以陷入 S 态。S 态稳态 SIE=0，内核期间到达的来源保持 pending，返回 U 态后在规范允许的有界时间内重新 trap；`_kernel_trap` 一律致命（转储停机）。
- 内核态 timer 长路径不会「误杀」：悬置中断在 sret 后才生效，调度点照常到达。
- 内核锁只需防跨核争用；锁原语的关中断语义在此模型下是零成本保险，保留以防纪律腐化。
- 阻塞的表达方式是异步 syscall（见 `call.md`），不是内核睡眠——协作式定性下不存在可挂起的内核执行流。

## SBI 平台基线

内核要求 SBI 2.0 或更高版本，并把 TIME、IPI、HSM、DBCN 作为必需扩展；SRST 是可选终态能力，失败时系统永久停放。版本号和必需扩展在启动期通过 BASE 探测，不从机器型号推断。

DBCN 名称中的 Debug Console 仅表示 SBI 扩展类别，在 eRhino 中只承载内核日志和 Debug syscall 的观测输出，不是功能模块。输出为单次非阻塞 best-effort：部分写、零写或错误都允许丢失，任何日志失败均不得改变调度、IPC、内存管理等功能路径。legacy putchar 只在 DBCN 尚未就绪时提供早期 best-effort 诊断，不是运行期兼容层。

## 唤醒所有权

**每个唤醒必须有主人，无主的周期性唤醒（保险 timer、心跳）不存在。** hart 敢睡，当且仅当「自己的期限已上闹钟，或别人的请求会按门铃」：

- **timer = 自己的确定到期点**：时间片或 WaitContext Timeout 登记在发起 hart 的索引最小堆；登记、arm 与到期弹出由 owner hart 完成，跨 hart 提前完成只按稳定 token 注销，不远程重编程 timer。
- **IPI = 他方的请求**：门铃语义（SBI `send_ipi` 无载荷，仅置目标核 SSIP）。发请求的一方必然醒着——睡着的一方发不出请求，逻辑上排除全员睡死。
- **设备中断 = 设备的请求**：主人是设备（中断接入后生效）。

唤醒后的行为统一：清中断源 → 查自己管辖的待办（调度域就绪队列、TimerQueue、跨核请求），有事做事，无事回睡。

### 电源阶梯

当前只有 wfi 浅睡：局部使能（`sie`）的中断 pending 使 wfi 返回，全局 SIE 不 gate wfi；idle 期间 SIE=0 不进入 trap，醒来后检查待办。

### 停机语义

idle 入口的静默谓词检查所有调度域无 Ready、全部 admitted hart 已登记 idle、所有 per-hart TimerQueue 为空。满足时不存在内核内生唤醒主人，调用 SBI SRST `RESET_SHUTDOWN`；不支持 shutdown 的平台保持停放。

Sleep/有限 WaitMany 的主人是 TimerQueue entry。纯 IPC 等待者本身不阻止静默：全部 hart idle 时没有用户执行流能够投递消息或信号。当前没有设备中断等待源，静默谓词也不包含设备项。

### IPI 门铃与 remote call

当前 IPI 只实现门铃：`send_ipi` 置目标 SSIP，目标清源后检查调度与终止待办；没有共享参数帧或 remote-call queue。Remote Call 的方向见 [`../ideas/call.md`](../ideas/call.md)。

## tp 寄存器

内核态的 tp 不是 thread pointer，是 hart pointer。每个 hart 启动时设置 tp 指向自己的 HartLocal 结构，此后保持不变量：

**内核态运行期间，tp ≡ 当前 hart 的 HartLocal 地址。**

推论：

- trap 进出与上下文切换必须保存/恢复 tp（tp 即通用寄存器 x4，用户态用它做 TLS，内核态占用它）
- HartLocal 按 cache line 对齐；跨 hart 交互只能走全局层或 IPI，HartLocal 严格私有
- 数组包装处唯一一处 `unsafe impl Sync`，SAFETY 注明上述访问不变量

## 地址空间

当前系统选择 Sv39；用户低半区、共享内核高半区、栈窗口、uaccess 与页表所有权由 [`mm.md`](mm.md) 唯一记录。调度侧非 Resume trap 出口切回正式内核 root 后才清 active，详见 [`execution-context.md`](execution-context.md)。

## 执行上下文与 hart 能力

bootstrap、HartId/HartSlot、现代 DT capability、共同 trap、CSR、UserContext/FP、调度域和指令发布的统一契约见 [`execution-context.md`](execution-context.md)。

内核态运行期间 tp 指向当前 HartLocal；正式 sscratch 恒指同一 HartLocal；SPP 是 trap 来源的唯一真值。U 态忽略 sstatus.SIE，内核以 `sie` 精确选择 SSIP/STIP 等来源，S 态协作式执行期间 SIE 恒为 0。

无 MMU hart 不进入当前共享高半区内核，也不属于 admitted hart 集合；当前没有 AMP runtime。

## 锁原语

内核自研三件：

- **`RawSpinlock`**：CAS 自旋原语（acquire 成功路径 Acquire、release Release，内存序由原子语义背书），无中断语义，是下面两者的内部实现；
- **`Spinlock<T>`**：内核容器锁，获取期间关本地中断（sstatus.SIE 清零）。关中断是正确性要求而非优化——中断处理函数若获取本 hart 正持有的锁，同核死锁；构造点声明锁序 rank 参与 Lock Ladder 断言（机制与秩表见 `task.md`「锁序契约」）；
- **`RankedRawSpinlock<const RANK>`**：实现 `lock_api::RawMutex`，供 talc 注入（TalcLock 控制实例化，秩只能走 const 泛型），trait 路径同样参与断言。

睡眠锁与协作式定性不相容（内核无挂起的执行流，见「内核中断模型」）；长等待的表达方式是异步 syscall，不是内核睡眠。

## 堆与物理帧

分层管理两种语义不同的资源：

- 帧池（os/frame_pool，自研 in-band 空闲链）：管页粒度、物理连续、整批生死（进程退出整批归还）的物理帧；元数据内嵌空闲区间首帧，零堆依赖。DTB 内存段剔除启动占用后注册；FrameTracker RAII 归还。设计细节见 [`mm.md`](mm.md)「帧池」。
- 堆（talc）：管任意尺寸小对象；内存源 FrameSource 从帧池按需取 1MiB 连续帧块建立堆区（talc 支持多块不连续区域），帧块所有权随 claim 终身归堆。设备树解析（os/dtb，就地游标）与启动路径零堆依赖，保证帧池先于堆就绪的线性引导序。

理由：帧与小对象生命周期模式不同，分层使碎片域独立、锁独立；这也是 Linux（buddy+slab）的通行分层。堆→帧池单向依赖（不逆向），彻底消除帧池元数据碰堆的运行时环。

## 进程容器与 PID

内核没有全局进程表：未 Dead 进程的生命周期根是 Job 直接成员表（`MemberEntry::Process(Arc<Process>)`，`task/job.rs`），root Job 由内核 static anchor 强持。PID 与 JobId 由 `AtomicU64` 单调分配器分配、不复用；它们不构成全局操作入口。PID 是所属 Job 直接进程成员表的键，JobId 是直接 child Job 表的键；两者也作为 JobDerive 选择子和 provenance 诊断值。所有权图与成员表机制见 [`task.md`](task.md)「Job、Building process 与发布」。
