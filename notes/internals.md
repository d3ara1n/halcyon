# 内核内部机制

对外行为的设计（任务模型、IPC、FAL）见各专题篇；本篇记录内核内部的结构性决策。

## 全局状态分层

按归属与访问频率分三层管理状态，目标是热路径无锁，而非处处无锁。

| 层 | 归属 | 访问方式 | 典型内容 |
|---|---|---|---|
| hart 私有 | 单个 hart | tp 指针，无锁 | 执行点（当前线程、域指针、trap 锚）、hart 状态 |
| 对象私有 | 进程/线程 | 所属对象的锁、Arc 引用计数 | 内存布局、邮箱、子进程列表 |
| 全局 | 全系统 | `OnceLock` + Spinlock 粗锁 | 进程表、调度域、期限表、mount 表、设备表、帧分配器 |

冷路径的一把大锁在本系统规模（4-5 hart）下争用可忽略；无锁结构（slot array、RCU 等）只在争用真实出现后才值得引入。

就绪队列不在 hart 私有层——它是调度域的共享容器（结构见 `task.md`「调度」），同域 hart 经域容器锁竞争；队列结构与策略封装在调度类内可替换。

## 内核中断模型

**协作式内核：内核态执行流不可被打断。** trap 进入 S 态的瞬间硬件清 SIE，内核代码一口气运行到 sret 回用户态——没有中断嵌套、没有内核抢占、没有跑了一半的内核函数。调度只发生在用户 trap 返回路径上。

这不是与「抢占式内核」并列的选项，而是微内核的推论：长工作一律在用户态服务，内核只做短路径转发——内核路径恒短，协作式恒真。**若某需求看起来必须内核抢占或内核线程才能满足，说明工作被错误地留在了内核里，该修的是架构方向，不是中断模型。**

具体纪律：

- 用户态 SIE=1（timer/IPI 可打断），内核态 SIE=0；内核态期间到达的中断悬置，sret 返回瞬间立即再 trap，以用户态 trap 形态同一路径处理——内核态永远不可能收到中断 trap，`_kernel_trap` 一律致命（转储停机）。
- 内核态 timer 长路径不会「误杀」：悬置中断在 sret 后才生效，调度点照常到达。
- 内核锁只需防跨核争用；锁原语的关中断语义在此模型下是零成本保险，保留以防纪律腐化。
- 阻塞的表达方式是异步 syscall（见 `call.md`），不是内核睡眠——协作式定性下不存在可挂起的内核执行流。

## SBI 平台基线

内核要求 SBI 2.0 或更高版本，并把 TIME、IPI、HSM、DBCN 作为必需扩展；SRST 是可选终态能力，失败时系统永久停放。版本号和必需扩展在启动期通过 BASE 探测，不从机器型号推断。

DBCN 名称中的 Debug Console 仅表示 SBI 扩展类别，在 eRhino 中只承载内核日志和 Debug syscall 的观测输出，不是功能模块。输出为单次非阻塞 best-effort：部分写、零写或错误都允许丢失，任何日志失败均不得改变调度、IPC、内存管理等功能路径。legacy putchar 只在 DBCN 尚未就绪时提供早期 best-effort 诊断，不是运行期兼容层。

## 唤醒所有权

**每个唤醒必须有主人，无主的周期性唤醒（保险 timer、心跳）不存在。** hart 敢睡，当且仅当「自己的期限已上闹钟，或别人的请求会按门铃」：

- **timer = 自己的确定期限**：只有存在特定、确定的期限（时间片、sleep）时才 `set_timer` 后入睡，不会睡过头。期限的主人负责登记时立即 arm 自己的 timer；期限登记不跟随线程迁移。
- **IPI = 他方的请求**：门铃语义（SBI `send_ipi` 无载荷，仅置目标核 SSIP）。发请求的一方必然醒着——睡着的一方发不出请求，逻辑上排除全员睡死。
- **设备中断 = 设备的请求**：主人是设备（中断接入后生效）。

唤醒后的行为统一：清中断源 → 查自己管辖的待办（调度域就绪队列、期限表、跨核请求），有事做事，无事回睡。

### 电源阶梯

第一档 wfi：浅睡，局部使能（`sie`）的中断 pending 即唤醒 wfi 返回，全局 SIE 不 gate wfi——idle 期间 SIE=0 不 trap，醒来查待办继续跑。QEMU 与真硬件皆正确。真硬件深睡态若停 timer，届时引入 tick broadcast / 常开时钟源；更深档为 HSM suspend。全部由应用面驱动逐档引入，唤醒所有权模型不变。

### 停机语义

**静默 = 无唤醒主人 → 停机。** idle 入口检测：就绪队列空（无工作可做）且期限表空（无期限会触发）且无设备中断使能——此时系统永远不会再醒来，语义上已死，调 SBI SRST（System Reset 扩展，`RESET_SHUTDOWN`）关机；QEMU 下模拟器随之退出（集成测试变为自终止）。

约束：每种 Waiting 必须有可枚举的主人（sleep 的主人是期限表登记；未来的 IPC 等待的主人是发送能力集），新增等待源时静默谓词同步扩展，否则误停机。设备接入后，使能的设备中断即主人，生产系统因常驻设备（console 等）永不静默——静默停机是「无事可做」的诚实终态，不是超时退出。

### IPI 门铃与 remote call

IPI 通路分两层：**门铃**（`send_ipi` + SSIP 处理 + 醒后查待办）与**参数帧**（跨核请求槽结构：tag / 载荷 / 完成通知）。门铃是唤醒机制的一部分，随调度落地；参数帧等第一个真实消费者出现再定形——无消费者不建框架。remote call 是这套「门铃 + 参数帧」的正式名字：跨核内核态通信的传输层，可预见消费者为 TLB shootdown（ASID 优化启用时）、负载均衡窃取通知、内核调试通道。

## tp 寄存器

内核态的 tp 不是 thread pointer，是 hart pointer。每个 hart 启动时设置 tp 指向自己的 HartLocal 结构，此后保持不变量：

**内核态运行期间，tp ≡ 当前 hart 的 HartLocal 地址。**

推论：

- trap 进出与上下文切换必须保存/恢复 tp（tp 即通用寄存器 x4，用户态用它做 TLS，内核态占用它）
- HartLocal 按 cache line 对齐；跨 hart 交互只能走全局层或 IPI，HartLocal 严格私有
- 数组包装处唯一一处 `unsafe impl Sync`，SAFETY 注明上述访问不变量

## 地址空间

Sv39，canonical 半区边界即用户/内核分界：

- 用户：低半区 `[0, 2^38)`（256GiB），完全归用户。
- 内核：高半区 `[0xFFFFFFC0_00000000, 2^39)`，含内核镜像与物理内存直映射（phys_to_virt 线性偏移）；vmalloc 等区域 M4+ 按需扩展。
- 每个用户页表 root 复制内核高半区顶层项（创建时拷 8-16 字节）→ 任意时刻内核代码恒可执行：用户 trap 不切 satp，satp 更换只发生在进程地址空间更换。
- 内核稳态 SUM=0；用户 VA 只在显式 user-copy guard 内直接访问，不回退到软件遍历页表 translate。
- secondary hart 从 HSM Bare 状态经永久 identity/高半区别名过渡页表进入正式高半区环境。

设计依据：共享高半区让用户 trap 直接进入共同内核入口，不需要 trampoline 页或双地址空间切换；SUM guard 则把用户访问权限收敛到显式边界。

## 执行上下文与 hart 能力

bootstrap、HartId/HartSlot、现代 DT capability、共同 trap、CSR、UserContext/FP、调度域和指令发布的统一契约见 [`execution-context.md`](execution-context.md)。

内核态运行期间 tp 指向当前 HartLocal；正式 sscratch 恒指同一 HartLocal；SPP 是 trap 来源的唯一真值。U 态忽略 sstatus.SIE，内核以 `sie` 精确选择 SSIP/STIP 等来源，S 态协作式执行期间 SIE 恒为 0。

无 MMU hart 不能进入当前共享高半区内核，形态仍是 AMP 独立镜像、物理内存 carveout 与 IPI 通信；它不属于本执行环境的 admitted hart 集合。

## 锁原语

内核自研 Spinlock：原子 CAS 争用（acquire 成功路径 Acquire、release Release，内存序由原子语义背书），持有期间关本地中断（sstatus.SIE 清零）。关中断是正确性要求而非优化——中断处理函数若获取本 hart 正持有的锁，同核死锁。

Spinlock 包装为 `lock_api::RawMutex`，同一实现注入两处：

- talc 堆分配器（TalcLock），堆分配纳入统一的中断纪律
- 全局容器（`OnceLock<Spinlock<T>>`）

睡眠锁与协作式定性不相容（内核无挂起的执行流，见「内核中断模型」）；长等待的表达方式是异步 syscall，不是内核睡眠。

## 堆与物理帧

分层管理两种语义不同的资源：

- 帧池（os/frame_pool，自研 in-band 空闲链）：管页粒度、物理连续、整批生死（进程退出整批归还）的物理帧；元数据内嵌空闲区间首帧，零堆依赖。DTB 内存段剔除启动占用后注册；FrameTracker RAII 归还。设计细节见 notes/mm.md「帧池」。
- 堆（talc）：管任意尺寸小对象；内存源 FrameSource 从帧池按需取 1MiB 连续帧块建立堆区（talc 支持多块不连续区域），帧块所有权随 claim 终身归堆。设备树解析（os/dtb，就地游标）与启动路径零堆依赖，保证帧池先于堆就绪的线性引导序。

理由：帧与小对象生命周期模式不同，分层使碎片域独立、锁独立；这也是 Linux（buddy+slab）的通行分层。堆→帧池单向依赖（不逆向），彻底消除帧池元数据碰堆的运行时环。

## 进程表

`ProcessTable` 封装 `OnceLock<Spinlock<BTreeMap<Pid, Arc<Process>>>>`，只暴露 get/insert/remove。pid 由 `AtomicU64` 单调递增分配，不复用。内部实现可替换（如 slot array），调用方无感。
