# Review 收口节奏与分批计划

> 本篇归档后只保留五批独立 Review 的历史执行事实。用户已授权并完成统一核定，当前唯一入口为[统一报告](../review-2026-10-12-consolidated.md)；下文“两项 P1 开放”等表述属于当时状态，当前处置以统一报告的原编号映射和更正证据为准。

## 状态与目的

状态：**前五批独立 Review 已全部完成；本轮没有待执行任务。后续统一整理在其他会话另行承接。**

本计划记录本轮独立 Review 的执行顺序、任务组合、代码基础分工和报告入口。它不替代各专项 Review 的固定提交、审查范围或 findings；本轮只完成独立审查和状态登记，不负责合并多份报告。

本轮按代码基础划分任务，完成已登记独立审查，避免重复审查已被替换的旧机制。第一批 P1 仍保持开放；后续报告不替代其修复、验证或延期登记。统一整理、交叉去重、覆盖分析和最终修复方案设计由其他会话另行开展。

## 执行原则

1. **按代码基础分工**：合并任务只为减少重复读取和重复定位；同一代码基础由一个审查任务负责，相关调用方可作为其范围内的消费者核对。
2. **先当前交付、后历史提交**：当前代码仍在使用的交付优先；固定提交已被后续重构替换的计划先做范围重核定，不照旧目标直接执行。
3. **独立报告优先**：每个任务完成自己的固定对象审查并产出报告；不等待其他报告，也不要求本阶段完成交叉归并。
4. **交界只作分工说明**：发现跨越其他任务范围时，在报告中标出依赖和交界即可；finding 是否重复、最终归属和修复顺序留给后续统筹整理。
5. **固定目标不漂移**：每次只审查计划登记的提交或提交序列；当前 HEAD 只能用于核对消费者和后续变化，不能替代固定对象。

## 分批顺序

### 第一批：基础责任链与当前服务交付（两轨并行）

本批拆为两条责任边界清晰的 Review 轨道。两条轨道各自产出一份 `review-*.md` 报告，不互相修改对方报告或计划。

#### 轨道 A：基础内核/运行时责任链

执行会话：**HighHolly**

报告：[基础对象链 Review](review-2026-10-12-substrate-object-chain.md)

入口：[对象链修复与机制收敛 Review](todo-2026-10-07-object-chain-convergence-review.md)

- 固定目标：`ac9d7d9364770a69c8135e918d512069980c06bb`。
- 核心范围：VM、funding、Work、Job、SourceRegistration、维护执行、对象关闭与最终退款，以及真实用户态消费者。
- 先确认：owner/authority、锁序、提交后责任、Busy/超时/取消/迟到、跨 hart、最后引用和退款终点。
- 合并审查：[库存来源与启动自检](todo-2026-09-frame-source-selftest-review.md)，固定目标 `606b59db22f071943a4bfc2c174633454e449401`；公共操作所有权、旧 Runtime/RPC/Outbox 只作为历史范围输入。
- 明确排除：FAL/fs/libservice 业务语义、Notification/Watch/Bind/Copy、验收脚本和设备/DMA。

#### 轨道 B：当前 FAL/OAL/fs/服务目录与验收

执行会话：**MistyFinch**

报告：[FAL 服务交付 Review](review-2026-10-12-fal-service-delivery.md)

入口：[OAL、文件与服务目录交付 Review](todo-2026-10-11-oal-fs-service-review.md)

- 固定目标：`a1ca124fdb70c1f20034f3e7d24d587f4323c98e`。
- 核心范围：liboal、libfs、libservice、Notification、执行付款、RPC 支撑、Watch、Bind、流 Finish、Copy 及实际服务消费者。
- 合并审查：[验收运行整理](todo-2026-10-11-acceptance-cleanup-review.md)，固定目标 `a06e455d064429d2e6e9984b03474a62104fb9e2`，作为同一当前交付的验收/证据面。
- 历史 FAL 集成、库基线、注册发现设计和 FAL 内核闭包只做范围输入，不按已替换旧路径单独重跑。
- 明确排除：内核对象链基础 owner、VM/funding/Work/Job 底层契约、设备、磁盘持久化和正式服务监督。

#### 第一批当前结果

- HighHolly 的 [基础对象链 Review 报告](review-2026-10-12-substrate-object-chain.md)确认两项 P1 实际反例：
  - SourceRegistration 取消后的迟到注册回执只进入 `RemovePending`，未触发 Remove 或重新调度，来源/任务可能永久停驻并阻塞 Runtime close；
  - IPI 失败后无定时器 idle hart 缺少结构性唤醒或重试保证，可能永久停留在 `wfi`，无法消费 `runtime_stop`。
- MistyFinch 的 [FAL 服务交付 Review 报告](review-2026-10-12-fal-service-delivery.md)没有新的可达代码 finding，记录五项 P2 证据缺口，继续由既有 owner 和 FAL 主计划承接。
- 第一批审查任务已完成；两项 P1 仍由基础对象链报告记录，后续统一整理和修复方案另行安排。

### 第二批：时间/运输与执行/库边界（两轨并行）

经用户明确决定，本批不等待第一批 P1 修复；目的仅是先完成固定提交取证，避免修复基于过时或可由后续 Review 消除的判断。本批 finding 不关闭、不覆盖第一批 F1/F2。

#### 轨道 A：公共时间与运输

执行会话：**MossyFalcon**

报告：[时间与运输 Review](review-2026-10-12-transport-time.md)

合并入口：

- [公共时间与期限](todo-2026-09-13-monotonic-time-rpc-deadline-review.md)，固定目标 `c6e0a84`；
- [消息运输](todo-2026-09-14-message-transport-review.md)，固定目标 `3060dd8`；
- [流运输与 Runnel](todo-2026-09-14-stream-transport-review.md)，固定目标 `a2aabed`。

边界：只审 ClockGeometry/Deadline、Mailbox/Packet、Producer/Consumer、Endpoint/Channel、Delivery/Lifetime、映射 owner 和运输/时间交界；不审 RPC Dispatcher/Outbox 执行准入或 FAL/fs 业务语义。

#### 轨道 B：执行准入与库边界

执行会话：**GoldRidge**

报告：[执行与库边界 Review](review-2026-10-12-execution-library.md)

合并入口：

- [通用执行、RPC 与 Outbox](todo-2026-09-15-runtime-admission-review.md)，固定提交序列 `a3891b00` → `5de2780f` → `e0b5c459` → `4e18e5e`；
- [用户态库知识归属与命名](todo-2026-09-21-library-knowledge-ownership-review.md)，固定目标 `96ee03b`。

边界：只审执行准入、Dispatcher/Outbox、结果/取消/迟到责任和当前 libkernel/libprocess/libexecution/libruntime 的依赖/命名边界；不重复 SourceRegistration 的 F1、对象链基础 owner 或运输细节。

#### 第二批执行规则

- 两个审查任务各自固定目标、读取代码并形成自己的报告；
- 任务之间只按代码基础划分，避免多个代理重复审查同一批代码；
- 报告可以引用其他报告中的交界事实，但不要求本阶段判断是否重复、是否覆盖完整或哪个 finding 最终成立；
- 本批报告只作为后续统一整理的输入，不在此处产出最终修复方案。

### 第三批：正式服务装配与设备体系（两轨并行）

本批转向当前仍在主线上、尚未完成设计闭合的两个领域。它们不重新审查前两批固定提交，也不把设计审查当作代码能力验收。

#### 轨道 A：正式服务装配与监督

执行会话：**RubyRobin**

报告：[正式服务装配设计 Review](review-2026-10-12-service-assembly.md)

入口：[正式服务装配与监督设计](../todo-2026-10-07-service-assembly.md)

范围：启动依赖、就绪判断、服务发布后的装配、授权交付、监督恢复、进程死亡、重连、失败升级和停止接点；核对与 FAL 服务目录、关机编排和设备设计的单向接点。

排除：FAL 注册发现协议自身、设备资源/IRQ/DMA 细节、全系统关机状态机、已完成对象链和前两批固定提交的重新审查。

#### 轨道 B：设备体系与中断接点设计

执行会话：**BoldDick**

报告：[设备体系设计 Review](review-2026-10-12-device-system.md)

入口：[设备体系整体设计与逐层细化](../todo-2026-09-device-system-design.md)

范围：D0/D1 当前设计、设备/驱动/管理域对象关系、身份作用域、authority/付款、绑定/租约/供应依赖、IRQ 静默/唤醒/订阅所有权接点，以及 D2-D4 进入施工前的闭合门。

排除：具体设备 ABI/驱动代码施工、MMIO/IRQ/DMA 实现、服务装配政策细节、全系统关机状态机；这些只记录交界，不在本批实现。

#### 第三批执行规则

- 两个会话各自读取当前设计计划、实现说明和已有承接项，独立形成报告；
- 设计 Review 只判断契约、责任边界、未决项和施工前置是否闭合，不宣称代码或系统能力已经交付；
- 设备/服务交界只在报告中标注，不在本批合并 finding 或形成最终修复方案；
- 本批报告只作为后续统一整理的输入，不自动启动代码施工。

旧固定提交 Review 的处理：已被第二批报告或当前重构替代的登记已移入 `plans/archived/`；其历史事实只供统一整理追溯，不再占用后续 Review 批次。

### 第四批：Notification 精确失败与平台保留内存（两轨并行）

本批处理两个当前仍有独立未闭合责任的领域：Notification N6 精确失败/费用窗口，以及平台保留内存与 reusable/DMA 生命周期。两者分别属于 IPC 角色实现和平台内存/设备资源设计，不重复审查前三批报告。

#### 轨道 A：Notification N6

执行会话：**GrandAspen**

报告：[Notification N6 Review](review-2026-10-12-notification-n6.md)

入口：[Notification 观察角色与统一等待契约](../todo-2026-10-10-notification-observer-roles.md)

范围：`MintObserver` 的 quota/allocator/KOID/输出写回/Frozen/接受后 Close/逐壳费用和失败回滚；现有 typed Owner/Signaler/Observer、Wait/WaitSet、Startup/退出与真实消费者只作责任链上下文。

排除：FAL/fs Watch 业务协议自身、对象链基础 owner、Runnel/Dispatcher 执行语义、设备和 DMA。无法注入的窗口记录为证据缺口，不补建测试专用服务或内核 ABI。

#### 轨道 B：平台保留内存生命周期

执行会话：**SteadyOrchid**

报告：[平台保留内存生命周期 Review](review-2026-10-12-platform-reserved-memory.md)

入口：[平台保留内存完整生命周期](../todo-2026-09-platform-reserved-memory-lifecycle.md)

范围：Devicetree `reserved-memory` 静态/动态放置、`no-map`/`reusable`、region identity、动态布局事务、reusable loan/reclaim、DMA/IOMMU 边界、direct-map/FramePool/MemoryPool 单一物理真值和实施前决策门。

排除：具体设备/驱动 ABI、完整设备 D1-D6 重审、服务装配政策、全系统关机状态机和对象链已完成部分。设备 Review 只作设计交界参考。

#### 第四批执行规则

- 两个会话各自固定当前设计入口和实现/外部契约事实，独立产出报告；
- Notification 报告不把未注入 N6 窗口写成代码 bug；平台内存报告不把设计门缺失写成当前设备实现缺陷；
- 两份报告只记录各自 findings、证据边界和交界，不做多报告统筹或修复方案；
- 本批报告只作为后续统一整理的输入，不自动启动代码施工。

### 第五批：系统关机编排设计（单轨）

本批只安排[系统关机编排设计](../todo-2026-09-system-shutdown-orchestration.md)。预算隔离仍是条件触发，电源管理服务只有未来设计占位，内核最终架构 Review 尚未达到全局触发条件；本批不为这些入口创建提前 Review。StoneLynx 未分配任务，未产生报告。

执行会话：**PrimeMeadow**

报告：[系统关机编排设计 Review](review-2026-10-12-shutdown-orchestration.md)

范围：用户态关机意图、准入封闭、受管对象范围、服务/文件系统/设备停止顺序、正常退出/拒绝/失败/无响应、期限与升级、强制终止后的回收、shutdown/reboot/system failure 区分、`SystemReset` 授权与提交失败，以及根监督者存活/意外退出两种终局。

排除：电源管理服务独立设计、设备内部 MMIO/IRQ/DMA 语义、FAL 注册业务协议、对象链/执行/RPC/运输固定提交 Review；交界只作分工说明。

第五批报告已完成；本批不要求先修复前四批 findings，也不进行多报告统筹。

## 条件触发事项

- [内核主线完成后的最终架构收口 Review](../todo-2026-09-kernel-final-architecture-review.md)：等主要用户态消费者、FAL、服务装配和设备范围形成后，再选择明确的固定提交序列；不作为本轮清仓任务。
- [Review 未闭合承接项](../todo-2026-08-26-review-carryover.md)：设备/中断设计进入对应责任链时执行，不提前插入当前批次。
- [用户态分层与线程生命周期设计审阅](review-2026-09-26-runtime-thread-design.md)：历史报告已由运行时分层归档、对象链交付和后续统一整理承接，不重新建立独立首审。

## 审查任务完成条件

每个审查任务开始前：

- 固定目标、父提交、实际消费者和与前批的增量已经核对；
- 计划明确本批不审查的旧机制和不覆盖的后续变更；
- 已确认没有其他并行任务负责同一代码基础；
- 执行者已读 `plans/REVIEW.md`、`KNOWN_ISSUES.md`、相关工作记录和实现说明。

每个审查任务完成后：

- 有独立 Review 报告，记录固定对象、审查范围、findings 或无 finding 结论、证据边界和残余风险；
- 报告不修改生产代码、其他 Review 报告或共享导航；
- 报告中的交界问题只标注相关代码基础和参考报告，不要求本阶段完成归并；
- 完成后通过约定渠道回报主会话，等待后续统一整理。

## 后续统一整理阶段

前五批独立 Review 报告已形成。后续统一整理由其他会话另行承接，不属于本轮待执行任务；其工作范围为：

- 汇总多份报告并识别重复 finding；
- 对照固定提交和当前代码，判断哪些发现仍然成立、哪些被后续证据消除或改写；
- 明确 finding 的唯一责任 owner、修复顺序、验证门和跨报告依赖；
- 形成最终修复方案及其唯一工作记录。

本轮未执行上述工作，不把任何单份报告当作最终修复方案。审查任务完成不表示报告中的 findings、设计缺口或证据缺口已经关闭。

## 当前接力

前五批共九份独立报告均已产出，无运行中的审查任务或待分配批次。

| 批次 | 执行者与报告 | 状态 |
|---|---|---|
| 第一批 | HighHolly：[基础对象链](review-2026-10-12-substrate-object-chain.md)；MistyFinch：[FAL 服务交付](review-2026-10-12-fal-service-delivery.md) | 审查完成 |
| 第二批 | MossyFalcon：[时间与运输](review-2026-10-12-transport-time.md)；GoldRidge：[执行与库边界](review-2026-10-12-execution-library.md) | 审查完成 |
| 第三批 | RubyRobin：[正式服务装配](review-2026-10-12-service-assembly.md)；BoldDick：[设备体系](review-2026-10-12-device-system.md) | 审查完成 |
| 第四批 | GrandAspen：[Notification N6](review-2026-10-12-notification-n6.md)；SteadyOrchid：[平台保留内存](review-2026-10-12-platform-reserved-memory.md) | 审查完成 |
| 第五批 | PrimeMeadow：[系统关机编排](review-2026-10-12-shutdown-orchestration.md)；StoneLynx 未分配任务 | 审查完成 |

第一批 F1/F2 仍保持开放。前五批只提供独立审查报告和新增取证，不构成修复、复核或多报告统筹。本轮到此结束，保留本记录作为报告索引与后续接力入口；统一整理在其他会话另行开展，本会话没有下一步审查任务。
