# 单调时间与 RPC 全调用期限

## 完成证据

- ClockGeometry/Deadline 的可读与可编程尾部、checked/u128 换算、精确频率和 CSR 采样排序已接通；shared host、内核 check、七面 clippy 通过。
- Wait/Sleep 初始就绪与 At(0) 优先、安装前复核、timer 注册/取消、满箱等待到期、Send 过期不消费 once/moves、已提交交付不因期限撤回、跨线程因果读取均有真实组合。
- 运行期协作停止前置已完成：Ready 后停止请求使用已发布 admitted raw ID 广播现代 IPI，所有 4 个 virt hart 实际停在 `hart::park`，PARKED mask 为 `15`；自 IPI 的 SSIP 会先清理，停止不遍历 timer 或伪造 Timeout。
- 当前提交基线为 `d22b9d7` 加本轮后续提交；分支 `task/fal-service-capabilities`。实现真值见 `notes/impls/time.md`，停止结构见归档的 runtime-stop 前置。
- 日志：`artifacts/check/time-final-{core,release,sifive,nofd,clippy,host}.log`、`time-runtime-stop-{probe,gdb}.log`。并行构建曾触发共享 boot-package 临时文件竞争，串行 release 重跑通过；失败日志保留，不作为 guest 失败。
- 不包含跨硬件 epoch 连续时间（第 8 节唯一未来范围）、RPC/Outbox/服务政策或 FAL；验收可靠性任务的完整 stress 截断/概率失败继续独立延期。

> 状态：公共时间前置已完成并归档。方向参考 `notes/ideas/{time,wait,rpc,call}.md`，本文件安排的公共时钟、Deadline 换算、绝对 Wait/Sleep/Send ABI 与既有消费者已接通；[公共对象前置](todo-2026-09-13-public-ipc-wait-prerequisites.md) 与运行期协作停止前置均已完成。后续 RPC/服务政策由 [执行前置](../todo-2026-09-13-service-runtime-prerequisites.md) 和 [FAL 总计划](../todo-2026-09-fal-service-capabilities.md) 消费。

## 当前施工位置

由 checkpoint `de981a6` 恢复施工，researcher/explorer 显式使用 `agentrouter-openai-responses/deepseek-v4-flash`，不改全局配置。以下记录最终实现与验证边界。首轮按固定 Zicsr CSR Access Ordering 修正 time 读的内存/CSR 双侧 fence 与编译器屏障；ClockState 独立拥有高水位/失败，可隔离编排旧样本、origin 一 tick、真正回退、epoch end 与并发失败。ClockGeometry 最大期限改由可读且可编程尾 tick 推导，补极端/低频与整除临界测试。实际实现见 `notes/impls/time.md`。

首轮 shared host、virt core 和七面 lint 通过，debug 反汇编确认 fence 序列，日志在 `artifacts/check/time-*`。定点 reviewer 未见实现 bug，要求补 `frequency=1_953_125` 整除末端 assert，已加入；源码阶段与隔离样本不证明实际多 hart 所有时序，任务仍未完成。下一闭包是 Wait/Sleep 的初始命中/At0/安装延期与发送到期 ownership，随后真实超时/跨 hart 时间读取和时间失败收束；RPC/Outbox/offer 属后续任务。

ClockGeometry/Deadline/ClockSnapshot、kernel clock、调度/启动换算、绝对 WaitMany/Sleep/Send 以及 rinlib 接口已写入。typed Packet、投递阶段错误、同步 Caller 与异步 dispatcher 已写入同一期限语义。TimerQueue 已增加保留 token 的 park/reschedule，注册期为全部 live 项预留恢复所需堆容量；libsrv WorkQueue 每任务预付一个可停用期限槽，有限 u64 最大纳秒仍是有限期限，不以整数哨兵代替 Infinite。真实服务期限政策仍待正式任务与 FAL 状态机接通。此前 shared host 23 项（含 Deadline 编码/非整千频率/epoch 上限）通过，用户背压沿用原 Deadline；这不完成本任务的 ClockGeometry 全边界、并发高水位/不可逆失败、安装/到期和完整时间消费者组合门。

时间实现已通过本任务的构建、lint、host、core、平台和停止取证门；RPC/任务计时草稿仍归执行前置审视，不能用这些源码宣称后续消费者已成立。RPC/任务计时草稿归执行前置审视，不能用这些源码宣称消费者已成立；不得恢复相对内核路径来绕过未接通消费者。

## 1. 事实与目标

开工前的缺口是 Caller 无限 `send_blocking` 后才启动相对回复等待、缺公共时钟及截断 timebase。当前相对内核路径/截断换算已删除，代码已有单一公共时间与绝对 ABI；本任务继续完成其边界、失败及真实消费者验证，不把既有草稿等同交付。

最终结构是一个公共时钟、一个显式 Deadline 类型、一个内核绝对等待/投递契约。相对输入只在用户态入口转换，不保留旧内核相对期限路径。原因不仅是多阶段重置：用户态计算剩余时间与进入内核之间可以被抢占，只有内核直接接收原绝对时点才能避免再次推迟期限。

## 2. 固定外部证据与适用边界

依据 `references/CONTRACTS.md` 定位：

- `normative/riscv-isa-v20250508/src/counters.adoc`「"Zicntr" Extension for Base Counters and Timers」中 RDTIME：64 位计数器、固定频率来源、跨 hart 一 tick 同步与可观察非倒退说明；
- 同版本 `machine.adoc`「Machine Timer (mtime and mtimecmp) Registers」：mtime 固定频率、u64 回绕、比较为无符号比较；time CSR 是 mtime 的只读影子，更新允许延迟；
- 同版本 `zicsr.adoc`「CSR Access Ordering」：CSR read 归 I，默认不与内存有序；`rv32.adoc`「Memory Ordering Instructions」定义 R/W 与 I/O 的 fence 集合。公共时间采样同时保留 asm 编译器内存同步约束；
- `normative/riscv-sbi-v3.0/src/ext-time.adoc`「Set Timer」：stime_value 是绝对硬件时点，不能把用户相对毫秒直接作为该值。

本阶段公开一个**不跨硬件计数器回绕的时钟 epoch**，同时受 u64 纳秒表示范围约束。支持区间从 boot origin 到两种表示上限中较早者；最大硬件值保留给关闭定时，不作为有限 deadline。请求越界返回 ClockRange 错误。检测到越过该硬件 epoch、明显回退或换算溢出时 fail closed，不返回倒退时间、不把队列改成 wrapping 排序。跨硬件 epoch 的连续时钟扩展不是当前承诺，唯一触发与完成标准见本文件第 8 节。

该边界来自 ISA 的无符号比较与表示范围，不是任意的短运行时限。验证要覆盖接近边界时的显式拒绝，不能只用通常接近零的 QEMU 计数器证明。

## 3. 类型和换算

### 3.1 公共接口

共享固定宽 `ClockSnapshot`：`now_ns: u64`、`max_deadline_ns: u64`、`resolution_ns: u64`、零 reserved。`MonotonicNow` 无 capability，返回本次启动的同一时间域。rinlib 暴露 `Instant`/`Duration` 和 checked 运算，不将 ns 单位称为 ns 精度。

`Deadline` 的 wire 为 `{kind: u32, reserved: u32, at_ns: u64}`：Infinite 的 at_ns 必须为零，At 可为零。未知 kind/reserved 拒绝。相对 Duration 在入口一次 checked 加法并校验 max_deadline；旧的零毫秒无限哨兵不进入新内核接口。

### 3.2 单一 ClockGeometry

保留 DTB 的精确 frequency_hz 和 boot_origin_ticks。所有公开读数、相对便利调用、调度量子、Sleep、启动期限和 SBI 编程从该真值换算，删除 `TICKS_PER_MS` 及反推频率。

- elapsed ns：`floor((ticks - origin) * 1_000_000_000 / frequency_hz)`，中间用 u128；
- 有限 deadline 的硬件点：`origin + ceil(at_ns * frequency_hz / 1_000_000_000)`，全部 checked；
- resolution 为名义 timebase tick 的 ceil 纳秒单位，不会报告零，不据此承诺可观察更新频率或调度精度；
- max_deadline 从最后可编程 tick 与 u64 ns 上限推导，并确保向上取整后仍可编程；
- 不对过期时点先做可能越界的未来换算，先按当前公共时间判断。

跨 hart 公开读数用一个轻量 AtomicU64 高水位保持非倒退，不引入每次读时的全局锁。读取之前取得已有高水位、随后采样 raw time，区分真正回退与“旧采样迟于另一 hart 发布”的并发；允许规范内的一 tick 差异，不能把较早采样被调度延迟误判成时钟损坏。越过支持 epoch 的失败状态保持不可逆，不能 clamp 一个严重回退而继续承诺时间前进。

## 4. 内核投递和等待

### 4.1 WaitMany / Sleep

WaitMany 接收结构化请求及绝对 Deadline。初始观察若已有合法命中可立即完成，否则已到期返回 Timeout；有限 At(0) 因而支持非阻塞观察。异步安装、timer 登记与各次重试始终保留同一个期限。

内部 timer queue 保留可直接编程的硬件绝对 tick，不重新从安装时刻加 duration。注册之前重新确认到期，不能让 park/安装间隔延长总期限。Sleep 与调度时钟共享换算，不维护第二套单位和溢出政策。

rinlib 的相对便利入口调用一次 MonotonicNow 后转为绝对核心；原调用者统一迁移 Infinite/At，不并存两套内核 syscall 路径。

### 4.2 Send

Send 的结构化输入携带 Deadline。输入与 capability 验证、Delivery/消息/队列容量预留完成后，在 HandleTable → Mailbox 临界区中，以最后的期限检查作为提交线性化点的一部分；之后只有不可失败发布。

到期返回专用投递前错误，source moves 与 send-once 不消费。满箱调用也不隐含等待；用户态按同一 Deadline 观察 WRITABLE/CLOSED 后重试。时钟读取不反向取低秩锁。

成功入箱后不能因期限到达回滚。这个保证只限制本次投递准入，不是业务执行 deadline。

## 5. 后续 RPC 与服务的期限契约

本节是后续任务需满足的期限契约参考，不在时间前置任务中扩展 RPC/FAL 实现。PendingCall/owner/计时消费由执行前置接入，服务连接/offer/idle 政策由业务计划接入：

1. 调用入口冻结 Deadline；
2. 所有投递尝试、背压等待和 ReplyPort/WaitSet 等待传同一个值；
3. Receive、protocol/txid/framing 验证后，在接受回复前再次核验；
4. 过期回复释放全部附带 owner/Delivery，不成为成功结果；
5. Unsent 超时归还完整请求能力；Sent 超时报 OutcomeUnknown，不自动重试；
6. 私有 Caller 废弃自身端口，共享 dispatcher 只退休对应 pending 项；
7. 已接受的结果不因随后 endpoint CLOSED 被改写。

libsrv 在每个有界任务轮次处理已到期政策，使用用户态 deadline heap 选最早时点等待 WaitSet。任务注册预付期限槽；改期和暂时停用保留 token/generation，不分配、不在每次期限触发后重新注册。Infinite 以 parked 状态不进入活动堆，有限期限保持全部 u64 表示语义；到期先 park 同一槽并发出任务输入，完成退休才 cancel。Open/Attach/Start 消费同一客户端连接期限；provider offer、Starting registration、outbox 和明确协商的 idle 政策使用各自绝对时点。无进展事件不能更新 idle 期限。

Deadline 不承诺线程在目标纳秒立即被调度；它限定投递和结果接受的线性化条件。

## 6. 施工顺序与归属

1. 接入 ClockGeometry、ClockSnapshot、Deadline、MonotonicNow，替换所有内核时间换算真值。
2. shared/kernel/rinlib 统一绝对 WaitMany/Sleep/Send 输入，完整处理越界与过期不消费。
3. 与公共对象任务共同核对 Send/Wait 的提交点、非阻塞初始命中、失败重试和原期限不重置，迁移已有相对时间消费者与便利入口。
4. 完成所有内核时间真值、共享 ABI/用户封装和现有调用者，执行公共时间的边界/静态/必要组合验证，再把已成立的能力交给执行前置。
5. 同步/异步 RPC、服务 heap、offer/outbox/idle 等后续消费由各自任务实现并验证，不回填进公共时间任务作为缺失前置。

本文件不拥有 Mailbox 对象拆分、Lifetime、Delivery、WaitSet 或 FAL 状态机；它们只在整体计划安排，避免重复施工真值。

## 7. 完成与验证

host 覆盖精确频率和非整千频率、取整、checked overflow、支持 epoch 末端、跨 hart 旧采样交错、初始就绪与已到期、park 安装延迟、发送提交到期及所有权、迟到回复隔离。

QEMU 通过真实满箱阻塞到期、发送前被延迟后仍不投递、已投递超时、携带 capability 的迟到回复、后续正常调用、跨 hart 读取、Open offer 不 Attach 等路径验证。全仓 host、静态与 QEMU 组合门由整体计划统一执行。

公共时间任务完成门：全部内核时间来源、Deadline 换算、绝对 ABI/用户封装和既有消费者接通，旧相对内核路径/截断换算删除，边界与组合证据可定位。Caller/dispatcher、真实服务政策和其失败 owner 的验证由后续执行/业务任务安排；FAL 总体门仍须复核完整期限链。提交后再登记固定提交的代码 Review。

## 8. 唯一延后项：跨硬件 epoch 连续时间

现状/目标：第一版明确拒绝跨 raw counter 回绕的期限；未来若需要跨越该边界持续运行，必须建立扩展 epoch、硬件比较器跨边界维护与所有内核 timer 的一致排序，不能只给 MonotonicNow 加 wrapping_sub。

位置：未来 clock 模块、sched/timer_queue 与本篇对应的时间方向文档。触发：平台 raw 计数范围/起始值使声明的支持区间不足实际运行要求，或系统开始承诺跨该边界连续服务。

自然序：先取证平台可用的回绕/持续唤醒机制 → 扩展时钟 epoch 与比较器协议 → 迁移全部 timer/Deadline → 在边界前后长期 idle、活跃任务、多 hart 和 pending RPC 上验证无倒退、无永久丢定时 → 删除单 epoch 能力限制。本条是唯一真值，不在其他计划重复立案。当前期限主体完成而本条仍未触发时，先将本条完整移入独立未来 todo 并更新导航，再归档本文件；不能把它当作已经实现。
