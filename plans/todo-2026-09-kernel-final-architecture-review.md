# 内核主线完成后的最终架构收口 Review

## 性质

本记录拥有**全局架构收口的条件性Review与旧固定Review范围归并**。2026-10-07校准：对象链、当前Sv39和运行时分层已经交付，不再等待旧ProcessBindMemory、rinlib或ProcessDrain迁移；FAL、正式服务装配及设备仍按独立分案继续。本记录不替代[五批Review统一报告](review-2026-10-12-consolidated.md)的当前有效性与后续责任，也不要求先完成一次全项目Review才能开始新领域设计；`ac9d7d9` 原始两项P1的更正依据见该总报告，不再从历史结论启动修复。

全局收口需在选定交付范围和实际消费者形成后，判断机制的长期职责、共同变化的边界和迁移残留。早期数据面固定提交及观察保留历史，不将旧对象名和阶段前置当作当前代码事实；近期可独立进行下文的旧Review范围归并。

## 已登记提交范围

本表是已完成批次的事后审查入口，只登记固定提交与证据，不提前执行 Review。已有实施方案仅供参考；审查以整体需求、正确性和长期结构为依据，允许推翻方案，不以“已按方案完成”免除机制论证。

| 提交 | 批次与范围 | 验证基线 |
|---|---|---|
| `d00604a05d11d22656c02d0270161ba6301657d4` | 多页 Tunnel/RNL2 切片 8/9：几何 ABI、共同 backing、完整 lease、Endpoint owner、RV64 共享访问、真实消费者、失败/退役及旧路径删除；同时重校栈容量与布局派生审计 | host debug/release、七面 clippy、just check、默认与全速完整 acceptance（stress 16/16）、RNL2 14 项模型测试、审计工具 6 项测试和共享访问反汇编均通过 |

该批对应 [`数据面计划`](archived/todo-2026-09-memory-object-data-plane.md)，统一审查时重点重新取证：

- Create/Attach 的结果与 shootdown 是否都来自唯一 lease 几何，所有 Commit 前失败是否保全 Invitation 和 affine owner；close 保活强环是否只在 Commit 后形成并按完成点解除。
- 完整 lease 的无空洞范围、连续对象 offset、页覆盖去重和最终 permit 退役，是否不依赖单 RegionKey/fragment 或当前物理连续性。
- 安全 owner 是否封死直接/间接 raw close 旁路；协议终态后是否停止共享访问，清理失败是否保留责任及可查询诊断。
- 共享访问的 Rust/LLVM/RV64 平台边界是否成立；host 合规模型与独立 guest 非合作改写证据不能互相替代或夸大为语言形式证明。
- 独立物理 cursor、u64 回绕、几何 shadow、EOF、Invited 期发布、ack→重查→wait 和部分完成错误是否组合闭合。
- backing/metadata/work 容量是否覆盖真实最后析构与退款调用链；12KiB guard 派生审计与两平台 256KiB 栈是否有完整布局、代码生成和运行证据，不能把单帧扫描当调用链证明。

切片 10 已另行完成实现与完整验收并提交为 `606b59db22f071943a4bfc2c174633454e449401`：删除 raw allocation adapter/tracker、boot-held 直接拥有几何、统一 funding 自检、完整清零与切分退款、child 来源保活。固定提交代码复核已并入[基础对象链原始Review](archived/review-2026-10-12-substrate-object-chain.md)，未确认新的 frame/source/selftest 反例；当前处置从[统一报告](review-2026-10-12-consolidated.md)接手，不在本计划重复安排。证据见数据面档案及 `artifacts/frame-source/`。正式 FAL Open 与 RPC deadline 的独立能力缺口不因数据面提交而完成。

## 触发条件

全局架构Review的执行条件：

- 明确本次要审查的系统能力范围及其实际消费者；范围内公开契约、owner、失败收束和验证基线已经形成。FAL/设备未交付的目标不得记为现状。
- shared ABI、libkernel及相关库/服务/验收消费者已在选定范围同步；当前已完成的对象链不再重复列为待施工前置。
- 建立固定提交或提交序列，说明相对已有Review的增量和未覆盖边界，避免审查浮动HEAD或重复findings。
- 不以旧多页Tunnel、ProcessBindMemory、rinlib等名称作为必须恢复的技术前提。近期局部Review及范围归并不等待全系统能力完成。

## 审查范围

### 结构与所有权

- 当前ProcessShell/Body、VmShell、MemoryPool、MemoryObjectCore、Backing/View、Tunnel Connection/Endpoint/Invitation的实际类型关系；历史ObjectBacking/ObjectView按固定目标追溯，不假定仍存在；
- frame、quota、charge、metadata容量、Handle、WritePermit、线程续体、Remote completion和维护Work的唯一owner与退款终点；
- Lock Ladder、锁外funding、提交后必成与自主维护退休的统一责任链；
- anonymous、object、Tunnel、boot-held backing 是否共享正确的机制、并保留必要的生命周期差异。

### 迁移残留

全仓搜索并分类：

- raw frame allocation 与 transitional adapter；
- 单页、单 PA、单 translation 特化路径；
- 重复的Map validation、来源/许可传递和仅服务迁移阶段的存储或分支；旧`BackingPlanFailure`等只在对应历史提交中追溯；
- 仅为某个阶段或验收 workload 存在的字段、常量和分支；
- 重复的 owner/permit/charge 真值；
- 兼容层、旧 ABI、旧注释、旧实现文档和计划中的过时事实；
- 只验证“资源下降”而未证明精确守恒的验收逻辑。

### 阶段回溯

逐批回看 1、2、3、4、5、6A、6B、6C、6D、6E 以及其后的 MemoryObject/Tunnel/Runnel 提交，回答：

- 阶段引入的机制在最终架构中是否仍存在；
- 若仍存在，它是否已被统一到最终 owner/transaction seam；
- 若不再存在，哪些 C/D/E/F 仍依赖它；
- 是否有阶段性错误分类、容量限制或测试政策泄漏到长期接口。

## 输出分类

每个发现必须归入且只能归入以下一类：

### 长期保留

已经属于最终架构的机制。记录最终职责、唯一 owner、禁止的旁路和对应验证。

### 统一重构

语义正确但实现分叉、命名不统一或仍停留在过渡接口。记录目标 B、迁移顺序、受影响调用点和完成后的结构不变量。

### 明确删除

只为旧 A 或阶段性路径存在的 C/D/E/F。记录删除前置、删除范围、全仓搜索模式和删除后的验证。

### 另案立项

不是残留，而是独立能力缺口或后续设计问题。转入唯一的 plans/ 计划，不在本 review 中顺手扩大范围。

## 执行顺序

1. 固定审查目标与能力范围，重建该目标上的类型图、所有权、锁阶、失败边界和ABI归属；
2. 建立全仓符号与调用图，标记每个阶段引入的机制；
3. 生成残留矩阵，明确 A、B、C/D/E/F 关系；
4. 先删除重复真值和旧旁路，再统一最终 owner/transaction seam；
5. findings要求实施时，由唯一报告承接内核、shared、libkernel、相关库/服务和测试调用点迁移；
6. 删除旧类型、字段、adapter、兼容分支与过时文档；
7. 做第二次全仓残留搜索，确认 C/D/E/F 不再被引用；
8. 运行 host debug/release、clippy、`just check`、virt core/stress/release、异构路线、sifive_u 与完整 acceptance；
9. 更新 notes/impls、plans/COMPASS 和长期规范；
10. 由独立 reviewer 做最终只读复核，确认代码已从 A 收敛到 B，而不是保留 A+B 双轨。

## 完成标准

- 最终 owner/permit/charge/retire 图可以从代码和实现文档独立复原；
- 所有生产路径不存在未登记的 raw allocation、单页特化或旧 ABI 旁路；
- 每个保留的阶段机制都有最终职责，不再靠“过渡”“以后删除”解释存在；
- 每个删除项已从代码、测试、注释、notes 和 plans 清除，或已转入唯一的另案计划；
- 选定范围内的内核、shared、libkernel、相关库/服务和验收负载使用一致接口；
- 完整验证矩阵通过，且资源守恒、错误分类、锁阶和 Commit 后零分配均有可观察证据；
- Review 结论写入归档文档，后续不再重复把已删除的 C/D/E/F 引回主线。

## 旧 Review 范围归并

2026-10-07按`refactor/system-redesign` / `f174d8e`核定COMPASS中12份旧Review的范围。此次只读计划整理保留原固定目标，不是独立代码Review，不宣告原提交无缺陷，也不把后续修复当作原提交通过。各旧计划保留证据，本节拥有其当时的适用范围和分流；后续若机制已被重构或固定对象已由当前报告合并审查，登记按 Review 收口计划归档，不再占用当前执行队列。

| 原计划 / 固定目标 | 当前可核实事实 | 后续分流 |
|---|---|---|
| [容量校准](archived/todo-2026-09-25-capacity-calibration-review.md) / `3dd7c4f` | 内核旧MetadataSponsor/Counter容量政策与virt-capacity已替换/移除 | 旧固定快照已归档，不再单独执行；当前硬隔离由预算分案拥有 |
| [FAL集成基线](archived/todo-2026-09-13-fal-integration-baseline-review.md) / `d22b9d7` | FAL/消息消费者仍在，但旧集成基线已被当前 FAL/OAL/fs 交付替代 | 固定快照已归档，历史范围由当前 FAL 服务 Review 和统一整理追溯 |
| [公共时间与期限](archived/todo-2026-09-13-monotonic-time-rpc-deadline-review.md) / `c6e0a84` | ClockGeometry/Deadline、时钟采样和期限消费者仍在 | 已并入时间与运输 Review 报告，不再单独执行旧计划 |
| [库存来源与自检](archived/todo-2026-09-frame-source-selftest-review.md) / `606b59d` | BootHeldExtent、frame/selftest和库存/供给机制仍有正式消费者；已并入基础对象链 Review，未确认新的 frame/source/selftest 反例 | 后续只在 funded owner 或启动自检契约发生独立变化时登记增量 Review |
| [设计审查后续内存](archived/todo-2026-09-design-audit-followup-review.md) / `4b27ce6`、`8aa7bc2` | VM外围已重写为Operation/Change/Backing/Views | 固定快照已归档，当前VM问题由对象链/内存记录承接 |
| [消息运输](archived/todo-2026-09-14-message-transport-review.md) / `3060dd8` | Packet、MailboxSender/SendOnce及真实消息消费者仍在 | 已并入时间与运输 Review 报告 |
| [流运输与Runnel](archived/todo-2026-09-14-stream-transport-review.md) / `a2aabed` | typed Producer/Consumer和真实消费者仍在 | 已并入时间与运输 Review 报告 |
| [公共操作所有权](archived/todo-2026-09-18-public-operation-ownership-review.md) / `8e0467a` | DebtLedger、FairBudget、RetirementTicket及旧work_ledger/ProcessDrain已删除 | 固定快照已归档，替代机制由对象链 Review 承接 |
| [执行/RPC/Outbox](archived/todo-2026-09-15-runtime-admission-review.md) / `a3891b0`至`4e18e5e` | libsrv已迁为libexecution，Dispatcher/Outbox仍在 | 已并入执行与库边界 Review 报告 |
| [库知识归属与命名](archived/todo-2026-09-21-library-knowledge-ownership-review.md) / `96ee03b` | 用户态库分层已继续演进 | 已并入执行与库边界 Review 报告 |
| [FAL库基线](archived/todo-2026-09-21-fal-library-baseline-review.md) / `dfcf7a3` | 旧 FAL 库基线已被当前 OAL/fs/service 交付替代 | 固定快照已归档 |
| [FAL注册发现设计](archived/todo-2026-09-21-fal-service-design-review.md) / `1000270`，实现对照`dda8a5b` | 注册发现已进入当前 FAL/service 交付 | 固定设计快照已归档，正式装配政策由服务装配设计承接 |

`ac9d7d9` 的独立Review原始事实见[历史报告](archived/review-2026-10-12-substrate-object-chain.md)，当前结论与后续责任由[统一报告](review-2026-10-12-consolidated.md)拥有，本表不重复安排。后续选定局部审查时，在对应计划写清固定目标、仍有效的契约、相对已有审查的增量及排除项；发现问题后按REVIEW手册形成独立报告，不在本表登记第二套findings。旧计划归档只表示不再执行该旧快照，不表示原提交无缺陷或已通过。

本次归并产物是历史范围矩阵。时间/运输、执行/RPC/库边界已分别由当前 Review 报告完成；库存来源/自检已并入基础对象链报告；旧 FAL、容量和内存后续固定快照已归档。全面代码Review、FAL重设计、DMA接入及锁改造不作为轻量任务承诺。

## 早期观察登记（历史）

以下保留早期基线中的观察点，不描述当前源码。后续审查先按固定提交核定机制是否存在；其中`proc.rs`、ObjectBacking、rinlib及旧metadata等已由对象链替换，不能据此重新安排旧结构修复：

- `os/kernel/src/task/tunnel.rs` 已贯通多页 Tunnel、MemoryObjectCore/ObjectBacking 多 extent 投影与完整 lease 退役，rinlib owner/RNL2/现有消费者已同步；未来按最终提交范围复核是否还存在阶段性特化或重复 authority；
- `os/kernel/src/task/proc.rs` 当前存在 `BackingPlanFailure`、重复 Map validation 与 `backing_permits` 多层传递；需在最终 MemoryObject/backing planner 完成后判断哪些应统一；
- `frame.rs` 普通页由私有 claim 与来源 charge 合成 funded owner，boot-held 直接持几何并经唯一 adopt 接管；旧 raw allocation adapter/tracker 已删除。统一自检与真实 child 来源保活已验证，未来按固定提交复核来源分型、切分及析构顺序，不重新把旧 adapter 视为现状；
- 固定 split metadata 上界、验收 workload 分支和 Pool 守恒观测方式需结合最终并发/碎片模型重新证明；
- 1–5 与 6A–6D 的既有 review 计划只验证对应提交，不替代本次全局架构收口。

本计划不得提前执行局部重构；触发条件满足前，只能在相关实现计划中登记新的候选观察点和明确的语义缺口。
