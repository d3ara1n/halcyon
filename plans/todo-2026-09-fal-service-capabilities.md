# FAL 服务能力与公共 IPC 前置

> 状态：F0–F3f 与 F4-1/F4-2/F4-3 已在未提交工作树接通，并完成分层、结构与组合验证；正式服务来源登记拒绝、OOM、强制 close 和 Gate source refusal 因无稳定外部注入入口保留为验证限制，不宣称已注入。F4 已完成独立 `test_fal` 的 A/B 能力链、业务消费者迁移、RPC→FAL→Create 生命周期、provider 退出观察和整体验收；当前基本 FAL 交付完成，后续只处理固定提交 Review 或明确触发的验证限制。
>
> 本文件唯一拥有基本 FAL 的剩余设计、施工、残留与整体完成门，不另建平行设计计划。统一流程遵循 `AGENTS.md`；方向契约进入 `notes/ideas/`，实现事实进入 `notes/impls/`，本文只记录任务特有的决策、依赖和证据。固定提交 Review 保留原证据，不重复安排本文件拥有的实施。

## 1. 接手基线与证据

### 当前代码与文档基线

- 开发分支：`task/fal-service-capabilities`，从本地 `master` 的 `5d406a4` 分出。
- F1–F3c 历史连续实现基线：`dfcf7a349fe6d9e2836bb7c8179ff7e96c8ce20a`；库知识、目录与命名重排：`96ee03b0d1641c86ed8ab05951bad6954ea84db4`；早期文档接手 HEAD `3607f22` 当时工作树干净。当前 HEAD/工作树以文末跨会话断点为准。
- 跨阶段边界与 F3d 设计固定提交：`1000270ef2b54c35bc56acda10943dd94c0e7e12`；[未来设计 Review](todo-2026-09-21-fal-service-design-review.md) 在提交后登记，待实现闭包完成后执行，不阻塞 F3d，也不作为运行能力交付证据。
- 当前实现入口：[`FAL`](../notes/impls/fal.md)、[`RPC`](../notes/impls/rpc.md)、[`Runtime`](../notes/impls/runtime.md)、[`Runnel`](../notes/impls/runnel.md)、[`启动`](../notes/impls/startup.md)。组件命名与依赖先读 [`user/README.md`](../user/README.md) 和 [`user/libraries/README.md`](../user/libraries/README.md)。
- 方向入口：`notes/ideas/{fal,fs,service,framework,message,wait,time,rpc,tunnel,runnel}.md`。方向文档不是实现完成证据；当前代码也不自动决定未来边界。

| 基线 | 已成立的责任与证据入口 | 后续不得误读为 |
|---|---|---|
| 公共对象、观察与退休 | [公共前置档案](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md)、`notes/impls/ipc.md` | 需要恢复旧 WaitSet Seal/Drain 或用户态退休编排 |
| 公共时间 | `c6e0a84`；[期限档案](archived/todo-2026-09-monotonic-time-rpc-deadline.md)、`notes/impls/time.md` | FAL 已有完整连接/运行期政策 |
| 消息、流运输、执行与 RPC/Outbox | [执行前置交付导航](todo-2026-09-13-service-runtime-prerequisites.md)；Runtime 是观察与任务寿命 owner，Runnel 已有 poll/SourcePlan 及 init↔pm 消费者 | FAL 正式文件 Open/流已由 F3e/F4 接通，不恢复旧 Runnel 观察层 |
| 公共操作所有权 | `8e0467a`；[公共操作档案](archived/todo-2026-09-14-public-operation-ownership.md) | 业务已拥有注册或流状态机 |
| F0–F2 | 严格 FAL2、GrantTable/稳定节点、双独立 provider、route/Delegate、下游退出与 Outbox abandoned；[固定基线审查](todo-2026-09-21-fal-library-baseline-review.md) | route-management endpoint 已是注册权威 |
| F3a–F3c | 同域 Move、Record/Handle/Take、无 capability 属性 Copy、Watch；同一固定基线与 `notes/impls/fal.md` | F3d–F4 已补齐注册/发现、Open、流 Copy、退出观察与整体验收 |
| 库知识与命名 | `96ee03b`；[库重排档案](archived/todo-2026-09-21-library-knowledge-ownership.md)、[固定审查](todo-2026-09-21-library-knowledge-ownership-review.md) | 已存在 libservice 或通用服务目录后端 |

FAL1、`MemFs`、slot-1 anchor、self-client、旧泵、原始 Runnel 工厂和无消费者观察草稿均已删除，不再列作未来删除项。历史混合集成基线 `d22b9d7` 与旧 F0 盘点只用于追溯，不用于恢复旧路径。

### 验证证据边界

- F3b/F3c 阶段已有 host、target、七面 lint、core/release 开发证据；具体阶段记录可从 `git show 3607f22:plans/todo-2026-09-fal-service-capabilities.md` 查询，固定目标提交仍由上述 Review 文件拥有。
- 较新的库重排收口记录了同工作树的完整 `THROTTLE=100 just acceptance`；L5 删除空库后另过 `just check`、七面 lint、默认 50% `just virt`。该证据验证当时的 F0–F3c 及公共基线，不覆盖尚不存在的 F3d–F3f。
- 库重排后的 host 基线包括 libfal 27、libfs 17、libexecution 22、libbudget 6 项。测试数量仅是历史快照，不作为未来完成标准。
- 双 provider 退出证据包括：业务已提交但回复满箱，停止后 `abandoned=1`；下游 Derive 已投递到静默 Mailbox，停止后 `downstream_abandoned=1`。两端最后完成授权、观察、后端、运输 owner 和账户收束。
- 完整日志在本机 `artifacts/`，不随 Git 交付。异机按影响面重跑，不能把历史日志路径视为可复现证据。历史墙钟敏感现场见[只读档案](archived/ref-2026-09-acceptance-timing-flake.md)，不是当前开放缺陷，也不授权忽略新失败或重跑直到绿。

## 2. 交付范围与非目标

基本 FAL 交付：稳定目录授权与路径解析、跨 provider Delegate、原子属性/Record 与能力出口、affine Take、同域 Move、无能力属性 Copy、直接成员/节点 Watch、服务注册与发现、单工 Open、普通流 Copy，以及全部真实消费者与正常/失败/取消/退出/超时/退休/退款责任。

- 普通 Create 已是目标存在即失败，不另造语义相同的 CreateExclusive opcode。
- Move 只在同 provider、同事务域承诺原子性；跨域返回 CrossDevice。
- Copy 不覆盖、不隐含 copy+delete、不承诺跨 provider 原子性，允许部分目标；带 capability 的属性 Copy 不在基本范围。
- Open 只打开现有流，使用显式 offset/范围；不隐含创建、append、truncate、快照、持久落盘或原子文件替换。
- Watch 是有界合并的失效提示，不是事件历史；不承诺递归、跨 provider 或重放。
- 服务发现分发 capability，后续直接调用；不强制全系统经过一个全局 registry，不承诺健康检查、自动重启、无缝滚动升级或透明重试非幂等调用。

[扩展操作计划](todo-2026-09-fal-extended-operations.md) 唯一承接递归操作、快照/持久性/原子替换、capability 属性 Copy、扩展 Watch、append/组合 Open。本计划不复制这些待办；新需求若触发，先修订边界与依赖，不能暗中扩大基本操作。

CPU 预约、KernelMemoryBudget、设备/中断/DMA、BufferQueue、通用异步语言运行时、系统关机编排及电源管理不在本专题。开放不可信创建域前须满足 [KernelMemoryBudget](todo-2026-09-14-kernel-memory-budget.md) 的前置；当前有界准入不等于跨域资源隔离。[映射 owner 延期项](todo-2026-09-14-user-memory-owner-lifecycle.md) 不能承接当前 Open 必须完成的 Endpoint 关闭与退款；若现有 owner 无法闭合正常清理，须先重排前置。

## 3. 技术依赖与施工顺序

```text
已成立的公共对象/时间/运输/执行/RPC/记账 + F0–F3c
                         ↓
                剩余能力设计闭包
                  ├─ F3d 注册/发现与目录投影
                  └─ F3e Open ─→ F3f 流 Copy
                         ↓
                F4 独立 test_fal 与组合收口
```

F4 汇合 F3d、F3e、F3f 的全部责任。施工顺序保持「设计 → F3d → F3e → F3f → F4」，但 F3e 不因排在后面就技术依赖服务发现。F3d 的发现失效组合实际消费 F3c Watch；不再把本次承诺的 Watch 组合列成可选项。

| 任务 | 当前状态 | 前置与真实消费者 | 完成边界 |
|---|---|---|---|
| 剩余能力设计闭包 | F3d–F3f 与 F4 当前设计、实现、结构复核和组合验收均已收口；验证限制单独保留 | Memory/Registry 双域、A→B 委托与发现、A/B provider、`test_fal` 与 init 监督消费者 | 当前承诺完成；未来正式注入入口出现时沿所属机制复核，不在测试进程重建机制 |
| F3d（含 F3a/F3c 回溯修复） | 当前未提交工作树已完成代码、结构复核与组合验收；保留正式来源拒绝注入限制 | Memory/Registry 双域、A→B 委托与发现、init namespace 消费者 | §8.10 修复与证据已记录；继续保留现有代码供 F3e 依赖，不归档整体 FAL 记录 |
| F3e | 已完成当前 Open/流闭包；完整验收、正式消费者与结构检查见第 9 节，特定故障注入缺口有唯一触发记录 | F3b、typed Tunnel/Runnel、Runtime/Outbox；provider 与正式流客户端 | F3f 在已验证的流完成/退款契约上组合 Copy，不制造新运输运行体 |
| F3f | 条件 Open、独占目标与双端 Copy 已接通；§10 有当前证据及故障注入限制 | F3e、稳定位置与独占 Create；libfs 双端复制调用者 | 双端最终失败等未验场景归 F4-3，揭示生产缺陷即回 §10 修复 |
| F4 | F4-1、F4-2、F4-3 均已完成；RPC→FAL→Create、普通业务迁移、退出观察、双端清理和完整故障组合已闭合 | F3d–F3f 的正式 A/B provider、`test_fal` 与 init 监督消费者 | `just check`、七面 clippy、完整 `just acceptance` 通过；OOM/强制 close/Gate source refusal 因无稳定正式注入入口保留验证限制 |

公共接缝按共同契约与长期变化边界组织，不按文件或 opcode 拆成独立交付。可为未来能力提前设计或建设独立基础；涉及当前责任链的结构调整在本闭包整体完成，独立前置另行立案并链接。实施顺序不限制设计视野，不为维持局部步骤而引入随后必拆的补丁。

## 4. 剩余能力设计闭包

### 目标与责任

本任务拥有跨 F3d–F3f 的责任审计与设计，不重审全部已交付公共机制，也不代替各闭包开工前的详细设计。设计实施者负责决策；普通内部取舍自行裁决，改变已确认外部语义、无法闭合或与 notes 目标冲突时再交用户确认。

起始代码落点：

- `srv_init/src/main.rs`：双 provider 启动授权、bootstrap root grant、route 装配与验收消费者；监督 owner 在 `supervisor.rs`。
- `srv_fs/src/server.rs`：唯一 World/Runtime，持有 `libfal::provider::State`；服务编排拥有 Ingress、RequestTask、Outbox、DelegateTask、ReadTask，provider 库拥有 State/Retirement/Grant/Watch；Dispatcher、route、注册控制和监督政策不下沉。
- `libfal/src/{authority,grant,backend,store,value,protocol,resource}.rs`：授权快照、稳定节点、预备事务、Record 与能力出口；`libfs/src/{client,prefix,resolve}.rs`：namespace 和路径客户端。
- `srv_fs/src/watch.rs`：provider-local 发布表，当前 8 项；与 Runtime 唤醒请求容量的组合需在接入发现负载时核定。
- `librunnel/src/lib.rs`、`libexecution`、`librpc`：既有 poll/SourcePlan、任务/观察、Dispatcher/Outbox。以现有消费者为依据，不把旧观察草稿恢复为第二套 API。

上述用户库路径均位于 `user/libraries/`，服务位于 `user/services/`。这是调查起点，不是最终模块设计。

### 必交产物

| 产物 | 内容与完成标准 |
|---|---|
| 消费者和能力拓扑 | 点名注册承载者、首个发布者、发现后实际调用者、Open/Copy 调用者、监督者；标注 direct grant 与动态获取边界，不以 Record 往返冒充服务发现 |
| 领域与接口图 | libservice 状态到 FAL 投影、provider 执行到后端、libfs 到流客户端的接口；MemoryBackend 和首个服务目录共同检验接缝，不让下层解释上层服务状态 |
| owner/authority/付款图 | 每种控制权、endpoint 母本/副本、快照、NodeRef、观察、任务、Outbox、映射、Charge 的创建、移交与最终释放；引用环和静默消散路径明确 |
| 状态和线性化表 | 注册/绑定/可见性、Open/数据/最终结果/退休、Copy 双端进度各自的提交点及条件校验；同一责任不由两套状态分别作真值 |
| 协议与失败矩阵 | 正常、拒绝、资源不足、未投递、已投递未知、取消、超时、退出、清理失败；每个结果标副作用、返回 owner、恢复/重试权限与终态 |
| 资源与有界性清单 | 各领域账户、PoolBinding、槽/字节/来源/回复/清理预付、容量依据；业务提交和停止后不依赖不可保证的补分配 |
| 实施与验证映射 | 每项接口对应调用者、迁移/删除项、测试断言、影响面与前置证据；第 7 节残留有唯一 owner 和删除条件 |

具体 wire、状态机、接口候选在本计划对应节展开；已裁决的持久语义进入 ideas，当前源码事实进入 impls。不得只写“按现有机制接入”而省略跨层责任。

### 已裁决的跨阶段责任

首个拓扑、Backend/provider 接缝、身份/注册协议、平面投影、Directory Record Read、Watch 唤醒承载及 F3d 全失败矩阵见第 8 节。长期契约写入 `notes/ideas/{service,fal,framework}.md`。保留 RegistrationControl sender 的不复用身份作为 instance，依据是内核对象身份与消息 context 的真实契约，不把它当秘密或节点身份。

| 边界 | 唯一 owner / 当前裁决 | 尚待局部设计的内容 |
|---|---|---|
| 发布—发现—调用 | A 承载服务目录，B 发布 FAL2 根，init 经发现调用 B 并装配 route；F4 迁移业务消费者至 test_fal | 无阻塞拓扑选择；具体代码容量由第 8.6 节成本公式核算 |
| 服务到 FAL | Registry 既是注册状态 owner 又是受控 Backend；libfal 的 provider/出口/Watch 通用，绝不依赖 libservice | F3d 实施内部类型/文件拆分不改变责任 |
| FAL 到 Open | A/B 的 MemoryBackend 是真实流后端；F3e 在同一 provider/Runtime 内按已鉴权稳定节点形成 owned Open lease，交给数据及控制任务；Registry 对流操作拒绝 | 第 9 节详细 wire、lease 类型、非快照读并发行为、空闲/结果保留政策和控制状态机 |
| 流客户端到 Copy | 当前 init 通过 libfs 正式流客户端操作 A/B；F4-2 将这些业务断言迁到已启动的 test_fal。Copy 的唯一状态机组合稳定位置、两端 Open/Runnel/最终结果 | 第 10 节 API、两端停止顺序、结果结构；不新建复制服务或阻塞泵 |
| 付款到退出 | 各 provider 的可信授权账户支付自身元数据/派生 grant，PoolBinding 支付将来的流 backing；客户端支付自身缓冲/执行。Runtime 管观察，业务 owner 管状态/退休，init 持根监督 | F3e 的映射/Invitation/Endpoint 成本，F3f 的缓冲及未确认进度；开工前预付，不能留给全局 mapping 延期 |

跨阶段的线性化顺序固定：发现完整 snapshot 取得 → 异步能力派生/回复交付；Open 的稳定节点鉴权/pin → offer 交付 → Attach 确认及 Start → 后端接受/业务结果 → 退休；Copy 的目标独占 Create → 双端连接/搬运 → 两端业务成功 → 自身退休。传输入箱、环进度、业务提交、已知结果与退款互不替代。已接通的 Open 回复 slot 0=StreamControl、slot 1=Invitation；当前 owner/失败行为见 §9 与实现说明。

单线程 World 串行化后端提交、注册和快照取得，没有新用户态锁阶；所有下游调用只带 owned snapshot/操作状态，不跨等待持 Registry/Backend 借用。Runtime 唯一拥有 source token/代次和注销，迟到事件只能路由到原实例/任务。最后引用不承担隐式整链关闭；显式退休 owner 保留失败及 Charge。未来多线程形态不提前进入本闭包。

### 取证与适用边界

本次不引入新的硬件/共享内存契约，已从 `references/CONTRACTS.md` 核对适用面。用户态 IPC、观察与付款以仓库现行正式契约为准；系统参照从 `references/systems/INDEX.md` 选取三种不同模型，不能把其语义直接移植成 Halcyon 的保证。

| 证据 | 已核实事实与本次用途 |
|---|---|
| 本地对象/观察：`os/kernel/src/task/{object,mailbox,lifetime}.rs`，`notes/ideas/message.md`；基线 HEAD `3607f22` | 全局 monotonic Koid；MailboxSender 的观察来源是 queue，LifetimeObserver 不反向保活被观察对象。据此区分 instance、endpoint CLOSED 与 control Lifetime CLOSED |
| 本地 FAL/执行：`libfal::{GrantTable::prepare_derive,NodeStore,StoredValue}`、`libexecution::Requests`、`librpc::{Dispatcher,Outbox}` | 真实 Derive 衰减并继承账户；NodeRef 不冻结 payload；请求 Gate 容量 16；cancel 不撤回已投递请求；回复 Sent 不证明已接收。第 8 节逐项承接其限制 |
| Fuchsia `af123c3ab51340f02c946471fa7781aa2072dc65`：[协议打开过程](https://fuchsia.googlesource.com/fuchsia/+/af123c3ab51340f02c946471fa7781aa2072dc65/docs/concepts/components/v2/capabilities/life_of_a_protocol_open.md) 的打开/路由完成段（77–109、216–234 行）；[directory.fidl](https://fuchsia.googlesource.com/fuchsia/+/af123c3ab51340f02c946471fa7781aa2072dc65/sdk/fidl/fuchsia.io/directory.fidl) 的 permission/Open/Unlink | 客户端创建 channel pair，经 namespace/manifest 路由后直接通信；连接权与修改目录权分开。这里是连接分发，不是本方案的 sender Duplicate；未据此推断路由变更自动保留/撤销既有连接 |
| Genode Foundations 26.05：[Session quotas](https://genode.org/documentation/genode-foundations/26.05/architecture/Resource_trading.html)、[Session routing](https://genode.org/documentation/genode-foundations/26.05/system_configuration/The_init_component.html)；源码另固定 [parent.h @26.02](https://github.com/genodelabs/genode/blob/26.02/repos/base/include/parent/parent.h) 的 announce/session/close | parent 决定路由并交付 session capability，quota 经 parent 转移、close 时返还。证明 session/付款需显式机制，不能把发现复制当作新 session；书与源码版本分别标注，不混称一个快照 |
| L4Re `941f09bbf01ee153c5b66caff87c868deab3c91c`：[namespace](https://github.com/kernkonzept/l4re-core/blob/941f09bbf01ee153c5b66caff87c868deab3c91c/l4re/include/namespace) 的 query/register_obj/unlink；[mem_alloc](https://github.com/kernkonzept/l4re-core/blob/941f09bbf01ee153c5b66caff87c868deab3c91c/l4re/include/mem_alloc) 的 allocator/quota | 名称映射到 capability，注册要求 namespace W，输出权利受注册 flags 与输入权利约束；内存 quota 属独立 allocator。所选资料没有说明 unlink 后已交付 capability 的寿命，不作为该结论的外部证据 |

Halcyon 的摘名/撤权边界由本地 capability/GrantTable 寿命契约独立推出；上述三系统资料都不足以直接证明所有动态重配置行为。未采用 Genode 的 session quota 转移，也不把当前用户态预算说成内核资源隔离。

### 设计完成门

- [x] 亲读实际启动、provider、后端、授权、Record、Watch 与公共执行/运输 owner；定点咨询核验接缝、快照及 Lifetime，未以子代理结论替代主体设计。
- [x] 首个真实拓扑、各阶段共同 owner/authority/付款边界和失败责任已裁决，未发现需重开内核 ABI 的前置；外部固定版本事实与本地契约证据见本节取证表。
- [x] 第 8–10 节分别闭合 F3d/F3e/F3f 的局部设计与施工；F4 的独立消费者及整体组合门仍由 §12 拥有，不以阶段通过代替整体交付。
- [x] 第 7 节残留、第 8.8/12 节验证及 COMPASS/接力断点映射到同一顺序。

该门完成只允许进入 F3d 实施，不表示代码或能力已交付；本轮授权限设计与文档。

## 5. 已实施机制的保留与回归责任

- **授权与路径**：GrantTable 根据内核 sender context 产生不可外部构造的 AccessSnapshot；路径不是 authority。root 为稳定节点，Derive 收窄 FAL ceiling 并继承付款及运输政策；父 grant close 不递归撤销 child。Namespace/Delegate/符号链接共享解析预算和绝对 Deadline，含能力修改先走到最终 provider 再运输 owner。
- **后端事务**：validate/reserve 预付真实存储、Charge 与输出；commit 不分配、不等待、不再失败。节点 pin、目录 link、数据 owner 与退休分账；准备/冲突保留 owner，成功替换的旧值进入明确收束路径。后续抽接缝不得把退款责任下放给随机 handler。
- **F3a Move**：校验收到的真实目标 capability、源 Remove 与目标 Create；同域防循环及稳定位置校验由有界任务完成。跨 provider 明确拒绝，不隐含 Copy/Delete。
- **F3b 值与 Take**：Record/Array 共享总字节、元素、深度及 Handle 预算；role/rights、槽完整性、出口政策均验证。Read 的有效权限是 grant 与节点交集；DirectoryGrant 不走普通 Duplicate。Take 持独占 Busy 预留，以回复成功入箱提交；未投递从 Packet 无分配取回 owner 并恢复，成功后收缩空值 charge。通用含能力属性 Copy 仍不承诺。
- **F3c Watch**：单调 id 仅用于寻址，不是秘密或授权；Query/Unsubscribe 还验证发起 grant context。先登记 owner CLOSED 来源，再复查稳定节点与 WATCH 权限并安装/取代次；安装后、回复前的事件不丢失。显式取消、静默 owner 关闭、回复 abandoned、节点终态和 provider 停止均有撤源与退款路径。
- **执行与退出**：Runtime 唯一管理观察登记/代次/停驻/注销；Dispatcher 拥有下游调用，Outbox 拥有回复与 Delivery。业务提交不等于回复成功，终态不等于资源已退休。provider 停止必须收束入站、下游、授权、Watch、后端、route owner 与全部账户。

现有验证与固定提交审查保留。发现影响这些契约的具体缺陷时在本计划归属闭包登记修复，不以“基础已完成”拒绝修正，也不因后续尚未实施而整体重开前置。

## 6. 各闭包共同约束

- FAL/服务/流 wire 留在用户态；shared 只拥有内核 ABI。RPC request slot 0 是 send-once，业务输入从 1 起；回复独立从 0 编号，Delivery 不占业务槽。逐操作核对实际角色、rights、reserved、长度、版本和能力数量，不能只校验协议标签。
- 稳定位置、对象身份、记录代次、请求身份分别表达不同事实；单调身份不复用，数值不构成 bearer authority。
- 请求已投递后的超时不等于未提交。必须定义查询、条件重试或明确未知结果，不能要求通用 RPC 提供尚未交付的幂等去重服务。
- 业务错误至少区分授权撤销、权限不足、CrossDevice、位置/代次冲突、CursorInvalid、资源/配额不足、Busy、Cancelled、Unsupported 与内部错误；运输 Timeout/ServiceClosed/OutcomeUnknown 属于调用阶段，不能统一降为 FAL Internal。
- 通用 provider 只拥有协议、授权和后端接口；libservice 拥有服务 schema、注册规则及资源分类。接口可以接受行为，但不能用回调隐藏反向依赖。
- 同步门面可为同步调用者服务；事件循环不调用阻塞 Caller 或数据泵。观察/等待/重 arm 复用既有 Runtime/Runnel；不同驱动方式不复制领域状态机。
- 资源上限来自协议、几何、真实成本或显式配置政策；记录谁支付、预留上界与耗尽结果。复制 sender、发现别名、派生 grant 不重新开户扩额；退款跟随真实释放，不跟随摘名或 Cancel。
- 所有锁序、跨线程/跨 hart 影响、最后引用析构与关闭停驻成本均在开工审计中核对。用户态单状态拥有者不能跨下游 RPC/流等待持后端锁；不为当前单线程消费者预建多线程运行体。
- 当前正常清理不能依赖杀进程兜底；失败保留可继续处理的 owner。监督者 ProcessDrain 是异常最终接管，不是正常关闭协议的替代物。

## 7. 唯一残留与接通清单

本表导航本专题尚待核定/接通的责任，不把全部已有实现定性为临时代码。owner 指闭包实施者；详细处置只写在所链接的所属章节，完成后从表删除或转为长期 notes。此次方向检查的正确性项、减法候选及触发条件统一在 §12「结构优先与回顾简化」，不在这里复制。

| 现状与位置 | 目标与责任 owner | 触发、保留期限与完成/删除门 |
|---|---|---|
| Open/StreamControl/provider 数据任务已接通且 F3e 结构/组合门通过；来源拒绝、强制 close 等无稳定入口的注入限制见 §9 | F4-2 将现有 Open 业务断言迁至 test_fal，F4-3 按真实边界补可控故障 | 迁移时删 init 对应断言；若独立故障场景暴露生产 owner 缺陷，回 §9 修复，不恢复 raw 工厂或第二套 WaitSet |
| 无 capability 属性 Copy 与双端流 Copy 均已接通，普通 Copy 消费者已迁入 test_fal；双端最终失败、未知创建与 provider 中途退出仍缺组合证据 | F4-3 证明失败结果与退款，具体续作见 §12 | 按长期调用/组合模型完善操作接缝；复用正式 A/B，不建测试专用泵或服务 |
| 主要业务已迁入 `test_fal`，init 仍有 Open/回读等普通断言与监督剧本混合；route 当前仅一个绑定槽 | F4-2 剩余移交与删除条件见 §12；init 保留正式装配/监督 | 单 route 可保留，扩容由真实拓扑触发；迁移删除同义断言而不扩大测试协议 |
| Record 生产往返、多嵌套能力组合尚缺完整独立验收矩阵 | F4 对 F3b 正式 API 补组合验证 | 第 12 节逐项证明完整快照、槽/权限/失败 owner，不借测试引入第二套实现 |

## 8. 服务注册与发现

### 8.1 首个真实拓扑与启动顺序

选择现有两个 `srv_fs` 进程，不改变 pm 的业务协议，也不新建注册服务 binary：

```text
init（启动、根监督、A/B 的可信付款装配；F4 阶段授予 test_fal 裁剪 GRANT 的业务根）
 ├─ direct grants → A：bootstrap / release / route，配置为目录承载者
 │                    └─ 一个 Runtime + Dispatcher
 │                       ├─ 共享 FAL Mailbox / GrantTable / provider
 │                       │  ├─ 内存根 / MemoryBackend
 │                       │  └─ 发现根 / Registry 后端
 │                       └─ 注册控制 Mailbox / AuthorityTable / 注册任务
 ├─ A bootstrap → init：内存 root、只读发现 root、注册根 authority
 ├─ 注册根 DelegateName("fs.secondary") → exact-name authority
 └─ direct grants → B：bootstrap / release / route / exact-name authority
                      └─ 内存 FAL provider + 异步发布任务
                         Register(自身 FAL2 DirectoryGrant) → PublishReady
init：先订阅 A 发现根 → 读取 fs.secondary → 取得 B 派生 grant
      → 给 A 装配既有 second route；F4 的 test_fal 独立订阅/派生并承担业务断言
```

A 的 FAL 入口和注册控制入口物理独立；FAL 入口内部的内存根与发现根仍是两个授权域，各自绑定独立 NodeRef/授权快照，跨域 Move 返回 CrossDevice。共享入口只有一份接收、GrantTable、Watch、封口及 Runtime 额度，**不承诺两域独立可用性或保底公平份额**；F3d 应以并发预算、交错和权限测试证明共享不放大授权且不丢退休责任，若需独立可用性须另定端点与准入策略。A 不把注册状态塞入 MemoryBackend，route-management 不取得注册权。Registry 是可选的正式后端组合，不以 workload 分支实现另一套 provider。单域采用平面名称集合，名称是非空单个 FAL component，不接受 `/`、`.`、`..`、NUL；多域由独立根 capability/namespace 组合，本闭包不实现注册子树。

A 完成入口、退休来源和三个根控制权的观察登记后，才以严格 bootstrap 包交付上述三项能力。init 在 B 构造前取得并直接 GRANT 名称授权；B 的发布任务在自身 FAL 入口可运行后通过 Dispatcher 调用注册协议，Ready 提交确认后才发送启动 Ready 报告。B bootstrap 不再给 init 交付业务 root，删除该旁路消费者；B 的业务能力只能经此发现链取得。A 启动失败、B 注册失败或期限耗尽都回到 init 现有 supervisor 收束，不降级为无鉴权启动。

需要在 Building 阶段直接交付给正式服务的 A 根 grant 和 B exact-name authority，发行运输政策必须显式包含 GRANT；普通 RPC 运输使用 TRANSIT，不能把二者混用。首个发布 endpoint 是 FAL2 DirectoryGrant，出口为共享根授权的独立派生能力，并非逐客户端 session。F4 的 `test_fal` 仅获得裁剪 `GRANT` 的 A 内存/发现根及阶段 Notification，经 Record 自行派生 B 业务 grant；scoped authority、release 信号、根注册权和 ProcessControl 均留 init，后者负责 provider 退出兜底。测试别名发布仍使用实际 B endpoint，不制造测试专用服务。

### 8.2 后端与任务接口

| 落点 | F3d 必须形成的接口与 owner |
|---|---|
| `libfal::backend` | 通用 Backend 契约：授权后的路径步进/元数据/枚举、Watch 安装复查、完整 `ReadSnapshot`，关联 prepared mutation/Move/Take owner、分步验证和无失败 commit，以及 seal/有界 retire。保留 MemoryBackend 实现；请求层不再解释 `Body` 或直接访问其 NodeStore |
| `libfal::provider` | 按 D4 已落实的边界拥有 State/Grant/Watch/退休算法，按 Backend 参数化；Read/Delegate/Request 等真实服务编排留在宿主，不再按早期全量提取设想制造 DispatchPort。库不依赖 libservice |
| `libservice::Registry` | 同时拥有 AuthorityTable、注册状态/名称索引、投影 NodeStore 和 Publication owner，并实现 Backend；注册入口和 FAL 入口经同一 World 借用该 owner，不做状态镜像或生命周期回调反向注入 |
| `srv_fs` 装配 | 同一 Task::Family 组合一套通用 FAL provider、两个后端域与注册/发布任务；共用 Runtime、Dispatcher、等待点和付款布局。下游完成按任务身份/操作种类路由，删除硬编码只回 `ServiceTask::Delegate` 的分支 |
| `libservice` client | 注册控制和 ServiceRecord 严格 codec、typed control、一次发现、发现失效视图及发布操作；同步调用者门面与 B 的异步任务消费同一操作语义，不在服务循环使用阻塞 Caller |

通用后端不得要求受控目录实现普通写语义。Registry 对 Create/Write/WriteAt/Delete/Move/Take/Link 明确拒绝；读取 root/Ready 属性、枚举、Derive、Watch 是允许面；流操作按节点类型拒绝。拒绝由后端保持，即使错误配置了宽 FAL grant 也不能绕过。服务目录的 root grants 正常只给 TRAVERSE、ENUMERATE、READ_PROPERTY、WATCH、ACQUIRE_CAPABILITY。

投影节点只保有稳定 NodeId、instance 和发布时元数据；endpoint 母本只由注册条目的 Publication 拥有，不在可写 Record 另存一份生命周期。`read_snapshot` 在一次同步借用内复查 Ready/当前绑定，复制全部编码内容并取得各出口的独立 owner，之后才退出借用、做下游 RPC。只有 NodeRef 的 pin 不算内容快照。Drain 摘链并放下注册表自己的节点 pin；旧 Watch pin、旧出口和控制诊断壳分别退休，绝不等待彼此最后引用。

### 8.3 身份、authority 与协议

注册控制 Mailbox 按内核 sender context 查两种表，不相信 payload 的 PID、badge 或数字身份：

- `RegistrationAuthority` 有 root 或 exact-name scope；root 可 DelegateName，exact-name 不再扩大/转委派 scope。Register/QueryName/条件 Withdraw 都限于自身名称范围；目录 FAL rights 不包含注册权。授权 alias 继承既有付款视图，不新开户。上级 authority 自身关闭不递归撤销已交付的名称权/实例控制；Withdraw 只撤出现实例，不取消未来注册权限。
- `RegistrationControl` 只控制一个实例，提供 PublishReady、BeginDrain、Query。`instance: u64` 取该 control **sender** 的不复用 object_id/context_id，不取 LifetimeObserver id，不取节点或 B 的 endpoint id；普通复制保持实例身份。注册表仅持 Lifetime observer，sender 唯一初始 owner 交给回复 Outbox。
- root/名称授权也按“先登记 Lifetime → 安装 → 交付唯一 sender”发行；普通控制表不复用要求 `root: NodeRef` 的 GrantTable，避免无意义节点保活。实例终态壳保留到最后 control/Delivery 消散，以有界注册槽收费。

新用户态注册协议由 libservice 拥有，独立 protocol id、版本 1；RpcPrefix 后为 32 字节 little-endian 头：version:u16、op:u16、status:u32、body_len:u32、reserved:u32、既有 16 字节 Deadline。reserved 必须为零，长度与能力数量逐操作严格相符。实例/代次/被发布协议标识为 u64，被发布协议版本为 u32；不改内核/shared ABI。下表操作按列出顺序编号 1–7，编号仅属于注册 codec，不扩展 FAL 为服务专用指令。

Register 只接受普通 MailboxSender，必须有 WRITE/WAIT/DUPLICATE/TRANSIT，拒绝 send-once；实际权利须覆盖声明出口，Directory ceiling 只含已知 FAL 位，Mailbox 的 FAL ceiling 必须为空。目标目录的真实 FAL 权限由其 Derive 校验，不靠 HandleQuery 猜测。控制 sender 具 WRITE/WAIT/TRANSIT/DUPLICATE，需要启动直授时额外发行 GRANT；不得发行 owner/MANAGE。RPC 回复期限还受服务端有限 outbox 政策约束，不因调用方传 Infinite 而无限保留未发送回复。

| 调用 authority / 操作 | 请求与能力布局 | 成功结果 / 条件 |
|---|---|---|
| root / DelegateName | 名称，无业务 cap | 回复 slot 0 为 exact-name authority，附 scope/identity；错误无新授权可见 |
| root/name / Register | 名称、protocol/version、ExportPolicy、有限 establish_deadline；endpoint 在请求 slot 1，slot 0 是 reply-once | Starting 的 instance/generation/state/deadline；回复 slot 0 是 RegistrationControl |
| root/name / QueryName | 名称，无业务 cap | 当前占用实例/代次/状态或 Absent；私有准备占名时 Busy；不返回 endpoint/control |
| root/name / Withdraw | 名称、expected_instance、expected_generation | 二者均匹配才进入 Drain；不匹配 Conflict，不能误删替代者 |
| instance / PublishReady | 无业务 cap、无可替换内容 | Starting → Ready；仍 Ready 时幂等返回当前状态；Draining/Terminal 拒绝 |
| instance / BeginDrain | 无业务 cap | Starting/Ready → Draining；已 Draining/Terminal 幂等返回当前状态；这是显式放弃及迟到 Ready 的栅栏 |
| instance / Query | 无业务 cap | 当前状态、单调 generation、终因、建立期限；只报告当前事实，不证明旧请求已停止 |

所有 Register 参数在 Starting 前冻结；Ready 不支持原地改 endpoint/协议。ServiceRecord schema v1 固定包含 `schema`、`instance`、`protocol`、`version`、`generation`、`endpoint` 六个字段，未知/重复/遗漏字段按该版本拒绝。当前 FAL 整数是有符号值：u64 instance/protocol/generation 用严格 8 字节 little-endian Blob 表示，不截断为 i64；schema/version 用经范围检查的 Integer。endpoint 为一个 repeatable Handle：首个 FAL2 服务必须标 Directory；普通业务 Mailbox 用 Mailbox，拒绝用 Opaque/Mailbox 伪装目录能力。记录不是健康证明，发布者声明 protocol/version，实际导出和调用仍需验证。

### 8.4 状态、提交点与可见性

| 状态/动作 | 名称与投影 | 提交、重复及退出责任 |
|---|---|---|
| Preparing（私有） | 独占准备占位，不是可发现记录 | 校验 role/rights/scope、期限、所有槽/字节、回复、节点与清理预付；mint control，登记 control Lifetime 与 endpoint CLOSED。失败撤源、释放占位并归还/关闭未发布 owner |
| Starting | 独占名称，无 FAL 成员 | 观察登记后复查占位及期限，安装实例；Register Outbox 持唯一待交付 control。期限取请求给定值与 authority 的有限建立政策上限之较早者，确定后不续租 |
| PublishReady | Ready，新增一个不可变属性节点 | 同一无分配提交更新注册状态、名称投影、记录代次及父目录代次；完成快照取得与 Drain 排序。提交前再次检查建立期限/状态，迟到任务不得发布 |
| BeginDrain / Withdraw | 立即摘名，拒绝新发现，释放名称供争用 | 条件删除仅指向自身的绑定，推进目录代次，产生旧节点 DELETE/TERMINATED；转移 Publication 至显式退休 owner，放下注册表节点 pin，不等待旧出口/已授 grant |
| Draining → Terminal | 保持不可发现；新实例可已占同名 | 撤去 endpoint 观察并关闭母本，完成本实例的注册侧撤出；Query 壳可继续存在，旧导出、Watch pin 和 Delivery 仍按各自寿命退款。Terminal 不表示 B 的业务请求完成或全部资源归零 |
| control Lifetime CLOSED / endpoint CLOSED / 建立到期 | 按上述同一撤出路径处理本实例 | 不创建第二条清理算法；control 消散后不再保留无主诊断壳。Provider stop 先封全部入口/授权，再推进所有任务退出 |

注册 generation checked 增加，单实例只单向推进。新实例使用新 NodeId；Record 的发布代次与该节点的发布版本一致。root 版本按实际 Ready/撤出提交递增；发布前须为全部可见实例的未来撤出保留版本余量，耗尽时拒绝新发布，不能让溢出阻止必需撤出或回绕旧 cursor。Starting 变化不改变可见目录代次。

### 8.5 Directory Record 出口

F3d 接通 **repeatable DirectoryGrant 的普通 Record Read**，MemoryBackend 和 Registry 共用；affine Directory Take 继续明确 Unsupported，通用 capability 属性 Copy 继续不在基本范围。不得只给 ServiceRecord 写特例出口。

1. 在存储域验证 READ_PROPERTY/ACQUIRE_CAPABILITY，完整验证 Record、槽与 policy，并捕获同代次 `ReadSnapshot`；为编码字节、字段 owner、结果槽和退休预付。把已有 grant 的 output_transport 纳入授权快照，定义为仅含 TRANSIT/GRANT 的出口运输上限；字段 policy 中这两位必须是其子集，否则在取得/取走 owner 前返回权限不足，不伪改记录或放宽上限。此检查对普通 Read、Directory Read 和既有 affine Take 一致；字段的 WRITE/WAIT 等业务使用权不与该运输位掩码相交。一般字段取得按出口 policy 收窄的副本；Directory 字段取得供异步调用的母授权使用引用。准备失败关闭已取得副本，不改变原值。
2. 对 Directory 字段通过现有 Dispatcher 顺序执行目标 `Derive { path: "", rights: field.fal_ceiling }`；同一请求最多一个下游调用在途，多字段共享原绝对期限，不为每字段续期。目标 provider 校验真实父授权并继承其账户。发现目录的 FAL rights 不参与这个目标域 ceiling 计算；路径 Delegate 保持原交集规则。
3. 验证 Derive 回复 protocol/version/op、成功 `Response::Node` 的 Directory kind/rights ceiling、完整 payload、恰好一个槽、sender role、实际运输 rights 与上限；子 sender 必须属于母授权的同一 Mailbox 且有不同 context id。不把 Lookup 的 Found tag 用于 Derive 解码。必要的运输权利衰减用正式 Duplicate，未交付宽副本显式退休。错误不交付任何半个 Record；Type 标签不是验证。
4. 所有字段成功后一次组成完整 Outbox。请求挂起期间不得再次按名称读取；Drain 后此前完整快照可继续完成，但 B 已关闭/撤权仍可使派生失败。Snapshot 本身不保证目标可用。
5. 取消/超时/下游退出/回复放弃均由同一出口任务收回已产生的子 grant、普通副本、母授权引用与 Charge；关闭失败保留 Retiring owner。Dispatcher 的已投递未知/迟到回复沿既有 ReplyPort/Delivery 清理，不把取消当作撤回下游请求，也不自动重试。保留 CallError.phase，不把运输错误统一折成 Internal。

### 8.6 Watch、容量与付款

发现者持续订阅服务根后读取/枚举，CursorInvalid 重读而不重建订阅。Ready 产生根 CREATE；Drain/撤销产生根 DELETE 及旧记录 DELETE|TERMINATED；替换是这两次提交，旧节点订阅终止而根订阅继续。ServiceRecord 不原地改写，因而不需要递归 Watch；Starting 的诊断只从控制协议读取。消费者同时观察目录 sender CLOSED，并在通知后实际重读；目录丢失由 launcher/监督政策重获 authority 或报告不可用，不偷偷使用旧发现缓存作为当前事实。

`WatchTable` 的容量成为 provider 配置 W，不是协议常量；存储与待唤醒承载按 W 预付。`publish` 的本地扫描上界为准入 W、effects 上界为一次操作涉及的节点数，返回拥有未发出任务 id 与游标的 `WakeBatch`。每个 advance 按 Requests 剩余容量发出一部分，队列满则保留尾部重新排队，不改变 Runtime 的单步 Gate 上限。pending 已写入后取消原请求不能丢掉 wake debt；provider 整体停止时才可由统一 Watch stop 接管。两域不在一个 advance 中无界拼接唤醒。初始配置允许维持小容量，但须以实际并发订阅数及元数据成本说明；验证必须包含 W 超过单步可容纳唤醒数，证明结构不依赖当前 8/16 的巧合。

| Owner / 成本 | 付款来源与释放条件 |
|---|---|
| A 注册 authority、实例、Publication 编码/表项、两类观察 | A 可信装配的 `AccountView<ServiceResource>`：Authority/Registration/Bytes/WaitSource；不按 PID 收费，不因派生/重发现开户 |
| A 目录节点、发现 grant、Watch、ReadSnapshot/出口 | 相同付款身份的 FalResource 视图；节点 pin/link、grant Lifetime、Watch owner 和出口任务各按实际释放退款 |
| A/B Runtime 与 RPC | 各进程 ExecutionResource 视图预付 Task/InputBytes，Dispatcher admission、Outbox 容器及来源上界在提交前保证；不把执行槽再计成服务实例 |
| B 被发布母 grant、发现所得子 grant | B 的父 grant 账户；A 保有使用引用但不转移付款。新派生授权有独立寿命，不产生新额度 |
| WakeBatch、退休记录及终态壳 | 在允许 Ready 前预付发布及必需撤出的承载；端点关闭/期限触发的清理不得临时申请无法保证的槽。摘名不退款，最后 owner 关闭成功才退款 |
| 后续 Open/Copy | provider 的授权账户与 PoolBinding 支付流及 backing；客户端账户支付自身任务/缓冲。F3e/F3f 补具体成本，不让服务发现承担流账本 |

配置集中声明各域 grants G、名称授权 A、实例 R（含保留诊断壳）、Watch W、请求 Q 与下游并发 D。Task 上界按固定任务 + G + A + R + W + Q + 发布/退休任务逐项相加；Source 上界另外计每个实例 control Lifetime、endpoint CLOSED 和建立回复的最坏同时占用、Watch 建立回复、每个下游调用两源及 Dispatcher 公共源。InputBytes 继续由 `Runtime::input_budget` 和实际 SourceEntry 推导，Bytes 按实际结构/编码/队列容量计算。不得照抄现行 32 tasks/64 sources/48 FAL sources 或用未计费的零槽冒充额度。数值在结构实现后用同一配置及成本断言求得，初始部署不构成不可信域隔离承诺。

### 8.7 失败与未知结果矩阵

| 边界 | 副作用与 owner | 恢复/退出 |
|---|---|---|
| 未发送 / codec、scope、role、额度拒绝 | 未接受的 packet/endpoint 保留在调用方；已接收后由请求 owner 关闭拒绝输入，不创建可见记录 | 原发送阶段明确未投递才可按业务政策重试；错误不丢资源 |
| Register 准备/source 登记失败 | 无 Ready；释放占位、撤源后关闭母本/control，退款跟随实际释放 | 返回失败；稀缺资源恢复后显式新尝试 |
| Register 回复 abandoned | 回收唯一未交付 control，撤出 Starting；无可发现服务 | 不留下永久占名 |
| Register 已入箱但未接收 / 结果未知 | control 由消息/Delivery 保活，但 Starting 仍受有限期限约束 | 不盲重注册；QueryName 仅观察占用，必要时条件 Withdraw 或等待原建立期限后显式重试。未收到 control 不能 PublishReady |
| Ready/Drain 已提交而回复失败 | 注册状态与可见性不回滚，Outbox 仍需退休 | 用原 control Query；决定放弃时 BeginDrain 并确认不可逆状态。Query=Starting 不能单独证明旧 Ready 不会再提交 |
| 同名冲突 / 条件代次失配 | Exists/Conflict，替代者不受影响；传入副本仍由失败路径关闭 | 重新观察后由有权者决定，不自动抢名 |
| 下游 Derive 部分成功、Timeout、取消 | 原属性/服务记录不变；新派生授权可能已存在 | 关闭本地未交付结果，Dispatcher 接管未知/迟到结果；不透明重发 |
| endpoint 关闭 / control 消散 / 建立到期 | 同一实例撤出，新发现拒绝；已授子能力不被追溯收回 | 观察 source 移除后关闭相应 owner；旧 snapshot 按目标现状成功或失败 |
| A 正常停止 / 异常退出 | 正常封全部入口、取消/完成下游、终止 Watch、撤注册/授权及退休队列；不等待外部 control/grant 引用消散才停止，显式撤源并清本地状态、关闭 Mailbox owner。异常时客户端观察 CLOSED | init supervisor 保留 ProcessControl 并 Drain；正常路径必须自行退款，不依赖 Kill 兜底 |
| 注销/关闭失败 | 保留 source/能力/Charge/退休游标，不伪报退款 | Runtime 既有退休重试或明确保留 owner；不阻断其他任务 |

### 8.8 施工顺序与完成门

以下是同一个 F3d 闭包的内聚施工阶段，不分别宣称交付或安排临时服务验收：

单一 F3d 语义闭包按依赖推进；F3a/F3c 已交付的测试只证明当时正常组合，不替代被本轮发现的跨根授权和 owner 复核。各阶段只限制施工注意力，不分别宣称完成：

1. **D0 设计与责任图**：以 sender context→AccessSnapshot 根→后端节点/绑定解析→完整 snapshot/下游 ticket→Outbox/退休为授权与 owner 链；同一后端命名空间拥有路径绑定，不保留 provider 全局 prefix。Registry 独占 Starting 期限、Ready/Drain/Terminal、可见代次与提交 effects；任务只拥有 Source/Outbox。请求 owner 在正式准入后才允许提交副作用，取消终止本地等待而不伪造下游回滚。依赖与残留清单记录在本文，长期契约进入 ideas。
2. **D1 先修授权与事务**：保留当前 A 单一管理 route 槽，但把 owner 从 `World` 移入 `ServiceBackend`，绑定明确附着于 MemoryBackend 的稳定根；`Backend::lookup` 先核对发送授权根正是该内存根、权限与完整路径组件，再返回已复制目标母授权的 typed 委托边界。路由是根上的 namespace 绑定而非伪造 FAL 节点，不另建 mount NodeKind/目录表；重新 Bind 替换唯一 owner，已完成 Lookup 的请求只持此前复制的目标。注册根及任意不包含绑定的内存子根不得取得下游 grant；同域 Move/Registry 域分派保持。同步修 Property commit 对 Take 预留的冲突检验，并迁移实际 init/libfs 消费者。删除 provider 全局 route/prefix；验证 root/subroot/registry root 授权矩阵、合法路由、Move CrossDevice、property/Take 交错与失败 owner。
3. **D2 请求准入与下游取消**：基于 Runtime Gate 返回 owner、Outbox 结果及 Dispatcher 已有 cancel/迟到回复清理，合并分散的 submission 交接状态，保证未准入不做业务提交，正常满载是有界业务拒绝/背压而非 provider fatal。Read Record→B.Derive、A→B.Delegate、B 注册调用是真实消费者。区分未 submit、已 submit 未交付、已交付三阶段，后者取消仅停止本地等待；期限、blocked reply、Gate refusal、late reply 的 owner 以 host/core/stress 和源代码审查作开发证据，混合高水位与最终组合退款留 D5。
4. **D3 Registry 唯一状态与观察**：以 Registry 的 Starting 索引进行条件单实例到期，Ready 在期限内提交即不能被迟调度的观察任务撤出；将 Ready/Drain/期限的投影与 Watch effects 作为提交结果，由一个 owner 发布。Register/Ready/Withdraw 在回复任务获得正式准入后提交；观察任务只持 Lifetime、endpoint CLOSED 和 Source 解除/退休责任，不再凭本地布尔值决定业务终态。A/B/init 真消费者覆盖超时先胜、Ready 先胜、独立关闭、建立回复放弃、提交后阻塞回复及最终服务账户退款；无法由正式入口触发的来源登记拒绝按 owner 链审查，留 D5 复核验证缺口。
5. **D4 深化后再提取 provider**：MemoryBackend 隐藏 Body、NodeStore 和具体 prepared owner，Registry 只实现正式只读/投影与注册状态；双根 ServiceBackend 按 NodeRef 域分派仍保留。通用 provider 只依赖经过两个真实后端验证的窄接口，srv_fs 留组合、注册和 route 配置，libfal 不依赖 libservice。迁移真实调用者并删除旧全局 route、重复期限判断、具体 Body 穿透和旧交接分支；不为搬文件引入临时适配层。
6. **D5 一次组合收口**：核定 Bytes/Task/Source 在运行中高水位与最终归零，覆盖双根混合压力、Watch overflow/重扫、回复/下游阻塞、endpoint/control/process 关闭和静默退出；闭合后再跑 `just check`、七面 lint、当前代码 debug stress、release core 及完整 acceptance。Review 逐项复核上述缺陷与文档，impls 描述最终当前机制，计划留实际运行证据；未到此门不归档。

当前设计负债归属：全局 route 已由后端绑定替代，`establish_active` 已由 Registry 条件到期替代，旧 `dispatch_submission` 已合并为单一取消意图；Runtime Gate 的来源限额拒绝和任务/输入退款在 host 验证，注册任务拒绝路径的未交付 control/endpoint、Outbox 与双观察来源在代码中各有继续 owner，正式服务的基础设施来源拒绝无法不加测试专用入口地稳定触发，列入 D5 验证缺口。D4 的 `Body` 和具体 prepared 类型已从通用请求任务移除，Registry 假写 trait 已删除。route 入口/回复与 root grant 分类归宿主，唯一 `provider::State` 及 Retirement 算法已进库；Read/Delegate/Request 留在 `srv_fs` 正式服务编排，不继续为文件迁移拆分。当前阶段证据不等于 F3d 完成。

### 8.9 D0–D5 收口状态

| 阶段 | 状态 | 证据 / 保留限制 |
|---|---|---|
| D0 设计与责任图 | 已完成 | ideas/impls/唯一计划边界已同步；当前不保留全局 route、第二注册状态机或隐式停止真值 |
| D1 授权与事务 | 已完成 | root/subroot/Registry 路由矩阵、Move/Property/Take owner 与失败退款 host/core/stress 通过；Registry 只读、Memory Body/prepared owner 不穿透 |
| D2 准入与取消 | 已完成 | Gate/Quota/下游取消、迟到带能力回复及 Lifetime CLOSED、混合负载和最终账户归零通过；正常容量拒绝不升级 provider fatal |
| D3 Registry 与观察 | 已完成 | Starting/Ready/Drain/Terminal、timer 先胜、endpoint 独立关闭、建立回复放弃、提交后回复阻塞、Watch effects 与 ServiceResource 归零通过；正式服务来源登记拒绝无稳定外部注入面，已由 Runtime host 契约与 owner 审查覆盖并记录为验证限制 |
| D4 provider 边界 | 已完成（按重评边界） | `libfal::provider` 拥有 State/Retirement/Grant/Watch；srv_fs 保留 Read/Delegate/Request、Dispatcher、route、注册和停止编排；不预建 DispatchPort |
| D5 组合验证 | 已完成 | `just check`、七面 clippy、host 包测试、virt stress/core、sifive_u、virt-nofd、boot-failure、ServiceResource/FAL/Execution 归零通过；fixture 映像已零复制借用 |

D0–D5 与下述结构修复在当前未提交工作树已收口。正式服务来源拒绝的动态注入仍是验证限制；当现有入口出现可控来源拒绝条件或真实消费者触发时，F3d owner 须用正式服务链复核资源返还，不为制造用例新增测试运行体。验收 fixture 生命周期清理另有独立计划；不重新开启已关闭的 D4 Dispatcher facade 方向。

### 8.10 最终结构复核结果

复核按 bug、owner、失败收束、重复真值、性能和旧路径检查；本轮发现与处置如下。可触发 OOM 没有稳定的真实堆故障注入，以下验证为代码路径审查、host 行为与真实负载/账户收束，不能冒称 OOM 注入通过：

| 原问题 | 最终结构与证据 |
|---|---|
| 注册请求复制名称及 `RegistrationReplyTask::new` 无条件预留 `WakeBatch` 可致 provider OOM 终止 | `copy_request_name` 可失败，Outbox 持有收到的上下文并回复 Resource；Register/Withdraw/PublishReady/BeginDrain 在副作用前预付唤醒容量，纯查询使用无分配空 batch，预付失败无能力出版。route 绑定也用可失败复制；FAL Move/Take 的四处请求字符串复制走既有 `V2Failure(Resource)`，已取的 destination/offer owner 随失败释放。 |
| Registry 单条 Record Read 复制 endpoint 后 `vec![snapshot]` 可触发不可恢复分配 | 先 `try_reserve_exact(1)` 再 push，失败沿 `ReadError::Value(ValueError::Allocation)` 返回；snapshot 中复制的 endpoint 随错误析构，提交/账户无残留。 |
| Registry `starting` 期限索引与 Registration 的状态/期限重复，批量 API 无生产调用 | 删索引、`starting_key`、额外 PreparedEntry/Bytes 收费和 host-only 批量 API；`expire_if_starting(instance, now)` 直接核对 Starting 与本实例期限。现有 host 覆盖 Ready 先胜、到期先胜与 seal，真实 provider timer/账户经组合验收。 |
| 三份 `valid_name` 与控制索引中相同 minted id 的 `identity`/`instance` 重复 | `libservice::protocol::valid_name` 为 wire/authority/Registry 单一规则，控制索引只存 instance/task_id；现有无效名称 host 契约及 A/B 注册、Query、失效真实拓扑通过。 |

本轮 `just check`、七面 `just clippy`、`libservice` host 17 项及完整 `just acceptance`（debug stress 16/16、release core、sifive_u、virt-nofd、boot-failure）通过；最终 Move/Take 补修后重跑了七面 clippy 与完整 acceptance，未重复与之无关的 host 测试。无遗留 QEMU/GDB 进程。fixture 的判定收敛由[验收 fixture 清理](todo-2026-09-23-acceptance-fixture-cleanup.md#消费者与覆盖审计)记录。Read/Delegate/Request 和 Dispatcher 仍属 `srv_fs` 的真实编排，不为迁移建立新 facade。


## 9. Open 与流完成

### 既定语义与公共接缝

Open 打开现有稳定流节点；在一次本地操作内解析目标、校验身份/权限并 pin，不信任先前 Lookup。请求声明 Read/Write、offset/范围、RNL2/几何及连接 Deadline；回复交付协商几何、offer_deadline、独立 StreamControl sender 和 affine Invitation。FAL2 op 16–20 的 codec 已与 provider、libfs/init 同批落地，失败与保留门见下文。

客户端依次 Open → Attach → 协议验证 → Start，共享连接 Deadline；provider 还施加独立有限 offer 期限，核验 PEER_ATTACHED 事实且未到期才启动数据任务。Read 时 provider 是 Producer，Write 时 provider 是 Consumer。第一版 Tunnel backing 由 provider PoolBinding 支付，按可信绑定的授权账户预留流成本，不冒充逐连接客户端 MemoryPool 出资。

### 开工前必须补齐

前置核对：`NodeRef` 将节点 pin 与目录链接分账，`MutationBackend` 有单次 `read_stream` 和 `prepare_write`/`commit`，单次推进写入不超过 1024 字节；typed Runnel 提供 producer/consumer、`SourcePlan`、EOF 与关闭失败返还 owner，init↔pm 与本轮 libfs/init 的 FAL Open 均为真实机制消费者。用户态不直接持内核 `PoolBinding` 对象：Tunnel backing 由创建进程绑定的 MemoryPool 在内核支付，FAL 业务配额由可信授权账户预付，不能伪称用户态再转嫁物理 backing。

**已定边界**：`libfal` 的 NodeStore/MemoryBackend 拥有节点 pin、单批数据与账户；`srv_fs::World` 的有界 StreamTable 是每流 `(NodeRef, AccessSnapshot)`、Runnel 单侧角色、待写块、状态、进度、终态和退款的唯一真值。每流一条 Runtime 推进任务仅持表键及本任务的来源登记/注销责任，不复制业务状态；主 FAL Mailbox 以内核 minted sender context 在 GrantTable 与 StreamTable 两个不重叠授权域分流，**不为每流创建第二个控制 Mailbox**。Open 回复由流任务的 Outbox 负责，后续控制调用各由已准入的短任务持自身 Outbox；Finish 的单等待名额存在表项中，Cancel/Query 可由其他已准入任务继续处理。流任务在控制 sender Lifetime CLOSED、有限会话期限或 provider stop 中最早的收束条件后，仍负责推进来源注销、能力关闭及表项退款；不以 sender 永久存活换取永久任务占用。一次不让出执行权的本地操作中，先用现有 `Backend::resolve` 从授权根走路并 pin，再用 `metadata` 取得目标类型、权限交集和长度，先核验 READ/WRITE_STREAM 再使用长度或回复；不预建第二套 `open_stream` trait 或包装 lease，也不按名字重新解析。`libfs` 组合 namespace→Open→typed Attach→RNL2 校验→Start，持客户端角色和本地 EndpointCleanup。只给一个真实 provider 使用，不新建通用 Dispatcher 门面或流服务。Read Open 冻结终点为打开时长度与请求范围末尾的较小值，后续覆盖可影响尚未读取的字节，后续增长不延长本流；当前 Data 只有增长/覆盖，没有 truncate。Read 的 accepted 为已确认对端消费的字节，transported 为环已发布字节；Write 的 accepted 为后端已提交字节，transported 为环已取出字节，均满足 accepted ≤ transported。Write 从环取出的数据由表项内有界待提交缓冲持有，`prepare_write` 与提交成功才增加 accepted；`IoError.completed` 只增加运输计数，不冒充业务接受。

**执行结构取舍**：上一版“每流专属控制 Mailbox，单 Task 同时持所有状态和回复”的草稿未进入公共代码；它会增加每流控制收件来源、控制回复复用及大 enum 的可失败堆分配责任。既有主 Mailbox 已提供不可伪造的 sender context，Runtime 可以通过 `World` 为不同任务串行访问同一有界表。沿用先前 advisor 的 StreamTable 方案，但不预加 `libfal::StreamLease` facade：当前同步 `resolve`/`metadata` 和稳定 `NodeRef` 已能证明首个 Memory/Registry 消费者的身份、权限与范围，真实用例推翻时再引入最小后端方法。表项是业务真值，推进任务只持来源/目标键，控制回复任务只持其调用/Outbox；这是本次避免重复真值的减法。

**已出版的控制与 wire**：沿用 FAL2 `RpcPrefix`/Header/Status 与同一主 Mailbox；主 grant sender context 只接 Open，minted StreamControl sender context 只接 Start/QueryStream/FinishStream/CancelStream，二者以内核身份在 GrantTable/StreamTable 两域分流。op 16–20 的 Open 固定头与 sized 相对路径编码方向、显式范围、有限会话 Deadline、RNL2 协议和请求几何，零几何表示接受 provider 的 `3 * PROCESS_PAGE_SIZE`；checked offset+length 溢出在客户端编码、服务端解码与节点准备层均拒绝，未知协议/几何返回 Unsupported。Read 无上界以打开时 EOF 为终点，Write 无上界仍逐批 checked。控制请求体为空。成功 Offer 包含 minted 控制身份、方向、环字节数、起点、Read 冻结终点及绝对 offer 截止，能力槽 `[0]=StreamControl sender (WRITE|WAIT|TRANSIT), [1]=单次 Invitation (MAP|TRANSIT)`；调用者核对槽数、role、身份及 RNL2 几何。控制回复固定宽报告状态、结果、终态原因、accepted 与 transported；非终态不得伪报成功终局。普通失败只回原协议 Status/空体且不附随能力，reserved、状态和能力槽计数 fail-closed。

**表项状态与控制交接**：`Offered`（Open Outbox 可堵塞，未送达的 sender/Invitation 仍有 owner）→ `Active`（只在确认 Open 已交付、真实 Attach 已观察且 Start 准入后）→ `Terminal`（先冻结结果）→ `Retiring`（按来源注销、Endpoint close、能力回收和退款分批收束）。Open 回复放弃、独立 offer 到期、control CLOSED 或 provider stop 使未激活流终止；激活后 EOF/错误/Cancel/会话期限使业务终止。Active 时只允许一个已准入 Finish 回复任务登记为 waiter，该任务自己持 Outbox/输入收费及超时/abandoned 责任，表项只存 waiter 的 task id；第二个 Finish 即时 Busy，Query/Cancel 经同一表项串行提交而不排在 Finish 的停驻后，Terminal 冻结后唤醒 waiter。Finish 的回复失败只解除 waiter，不回滚流业务；重复 Finish 可从 Terminal 重取同一结果。取消不能覆盖已冻结的成功或失败，部分 Write 的 accepted 单调且与运输计数分列。Terminal 保留最小结果直至 control CLOSED、有限会话期限或 provider stop 中最早者，过期后撤销控制寻址并清理，即使 sender 仍被持有也不无界挂住停止；stop 时未确认的结果只能报告失败/未知。数据资源不依赖该 sender 的最终消散才开始退休，流推进任务未归零时 provider stop 不报告成功。

**期限和预算**：Open 必须提供有限会话期限；Open 回复有效期受 provider 上界约束，offer 截止另取有限会话期限与有效 Open 回复截止加建立宽限的较早值，具体首个消费者参数见下段。Active 会话期限不因控制 RPC 或数据进展反复重置，客户端所有阻塞 WaitMany 传同一绝对期限。StreamTable 的表项上界从 `TASK_LIMIT` 扣除固定服务任务与至少一个可处理 Cancel 的控制任务余量，再受物理 Tunnel backing、`FalResource::Bytes/WaitSource` 与 Runtime Source 容量约束；每个存活表项始终对应一条未退休的流推进任务，不能让结果壳脱离任务另行累积。Tunnel backing 由 provider 进程 PoolBinding 在内核支付，业务长生命周期内存由授权账户承担，不能让失效控制者逃避收费。T2 实施前按实际同时来源与控制回复峰值核对 Task/Source/Bytes 限额，拒绝只影响本流，不能为了多一份流表配额复制状态。

**首个正式消费者的容量与期限**：当前只协商 `3 * PROCESS_PAGE_SIZE`（RNL2 有效容量 12,160 字节），请求非零且不等于该几何即 Unsupported；将来其它几何须按真实客户端与物理成本另行扩展。服务侧 Open 回复最多 5 秒，独立建立宽限 2 秒；offer 截止为 `min(有限会话期限, 有效 Open 回复截止 + 2 秒)`，客户端按绝对截止检查。每流推进任务在 Open 交付期间最多占三个 Runtime 来源，控制调用另占一个；局部 S=15 个表项使最坏时 Tunnel backing 为 45 页、额外 Task 为 2S、Source 为 4S。`TASK_LIMIT`、`SOURCE_LIMIT` 已计入这两项，`ExecutionResource::InputBytes` 按 Source 上限推导，`FalResource::WaitSource` 增加 2S，业务 Bytes 预付表项和 1024 字节待写块；内核 PoolBinding 支付物理页，不重复在用户态计费。S 是本 provider 的有限并发政策，不是 RNL2 协议限制；满载回 Quota。独立 Finish waiter、附着后 Start 丢失到期与来源拒绝仍列为 F3e 未覆盖门，不把静态预算当动态证明。

**出版前责任顺序**：主 Ingress 只接收/复制请求并准备 Outbox，Runtime Gate 正式准入前不解析节点、不建 Tunnel、不铸 sender。准入后先鉴权/resolve、预付表项与配额，以 `OrderedTable::prepare_insert(0, entry)` 取得持有堆节点的 `PreparedEntry`；Open 任务只保存这个小 owner 和 Outbox，不把大表项内联进所有 ServiceTask。后续创建的 NodeRef、sender/Lifetime、Runnel role 与 Invitation 经 `value_mut()` 立即进入预备表项；typed Create 的 `Published`/`Protocol` 错误返回也归其持有，不能通过 `map_err(|_| Resource)` 丢失。Lifetime/Tunnel 来源逐一登记；中途拒绝停止出版、注销已登记来源并保留其依赖 owner。所有登记回执齐备且报价仍有效后，以内核 minted sender object id 调用 `with_key(id)` 安装表项，再将固定槽能力推进 Open Outbox；安装时检查身份未占用，不能以 `insert_prepared` 的 assert 代替拒绝。`Sent` 回执前可撤销，`Abandoned` drain 未交付能力，关闭失败原样持有。数据 Endpoint 只在来源注销回执后显式 close；单次 close 失败不能在 `Step::Runnable` 中无限紧循环，须按 Runtime 停驻/期限交棒模式有界重试，provider 未归零时报告失败而非成功。控制请求的 Outbox 归其各自任务，来源 cookie 及注销回执不重用 Open 任务 owner。本轮 `libexecution` host 新增 `four_sources_retire_before_their_owner_closes`，以 `Runtime::input_budget(4)` 的隔离账户验证四次登记、四次注销回执均先于 owner 关闭且 Runtime 归零；固定 4096 的旧测试账户曾在第一个来源被 Quota 拒绝，并非正式服务预算错误。`libexecution` 全部 host 24 项与七面 clippy 通过；该检查证明 Runtime 合同，不代替真实流任务的取消/关闭失败验收。

1. **类型和 owner**：控制身份、NodeRef、账户 reservation、Runnel 单侧角色、Invitation、观察、待回复 Finish、稳定最终结果及退休记录的唯一 owner；StreamControl Lifetime 观察不自持 sender 母本。
2. **节点及并发语义**：rename/delete 后已 pin 的流继续指向原对象；普通非快照 Read 遇到并发写、长度变化和范围终点时具体如何结束。不能把“不保证快照”当作未定义的内存/数据行为。
3. **进度与存储**：区分应用提交、共享环发布/消费和后端实际接受。文件 offset、范围终点和业务字节计数使用 checked 算术，溢出明确拒绝且不回绕，不能套用 Runnel head/tail 的模计数语义。Write 从环取出后尚未提交后端的数据由谁保留；Prepare 失败、通知失败、后端部分失败如何计数，重试不能重复接受字节。
4. **控制状态机**：重复/迟到 Start、Query、Finish、Cancel 的合法状态、幂等边界和返回结果；竞争由单一业务状态拥有者裁决，不按回复先后猜测提交顺序。
5. **期限与准入**：连接、offer 和每次控制调用期限分别约束什么；本范围用有限会话期限约束运行期，不另造可被伪唤醒重置的空闲时钟。表项、映射、来源、回复、等待者和清理记录必须在允许副作用前预付，有限容量有依据且为控制调用保留准入余量。
6. **终态与保留**：最终结果在控制权仍存活且会话未到期时如何可查询、何时退休；资源释放与结果保留可分开，但不能关闭对端尚需消费的映射或无限保留终态。Finish 待回复名额有界，后续查询不创建无限 waiter。
7. **驱动与关闭**：复用 Runnel poll/SourcePlan、Runtime 登记/注销、Dispatcher/Outbox；服务任务不阻塞。关闭的停驻成本与失败继续 owner 明确，不在任意 Drop 隐式执行整条退休。

### 当前施工断点与依赖顺序

| 阶段 | 当前状态 | 完成证据与进入下一阶段的条件 |
|---|---|---|
| T0 运输建立与退休接缝 | 前置已实施并验证：创建端 Producer/Consumer 均可读取内核 Attach 状态，Consumer 首次可读数据不替代 PEER_ATTACHED 事实且继续订阅该电平；Readable 的建立标志仅指同批内核事件，数据读尽后才来的事件仍会单独报告。typed Create 的已发布异常输出返还 Endpoint/EndpointCleanup/Invitation owner，init root 预备槽接管；`Channel::fail` 只冻结终态，不早于 Runtime 来源注销关闭映射。typed 批量读写提供单一绝对 Deadline，超时不关闭映射、返回已完成进度；原无限期入口保留现有真实消费者语义。rinlib host 9 项与 doc 3 项、librunnel host 20 项、七面 clippy 和完整 `just acceptance`（debug stress 16/16、release core、sifive_u、virt-nofd、boot-failure）在最终观察计划变更后通过；没有模拟内核输出损坏的 target 路线。 | T1/T2 已以真实 Open/Attach 接通来源注销、客户端失败清理和到期取消；第一次 target clippy 因 WaitResult.reason 为 raw u32 失败（日志 `artifacts/f3e-t0-deadline-acceptance.log`），改为 `WaitReason::from_u32` 且未知 reason fail-closed 后完整 acceptance 通过。 |
| T1 一次 Open 准备 | 已接通并验证 | `libfal::backend` 的 `pinned_stream_survives_unlink_until_last_owner_retires` host 证明已 pin 对象在 unlink 后继续读写并最终退款；正式任务准入后 resolve/metadata/权限/checked 范围/NodeRef 均在一次本地操作内完成。A、B 授权读、受限委托 Permission、溢出前置拒绝；真实 Open 后尾部覆盖可见、EOF 外增长不越过冻结终点。真实 Open 后 unlink 的组合由同一 NodeRef host 契约与服务代码路径支撑，不额外制造删除中的测试专用文件。 |
| T2 Offered 与退休 | 当前能力完成；特定故障注入缺口登记于下 | 同一主 Mailbox 的 minted sender、Invitation、固定宽 Offer、Source 回执后安装、Outbox/typed Create owner、5 秒回复加 2 秒建立宽限与会话上界均已接入。正式 init 验证未 Attach 到期的 Terminal Expired、Start 前 EOF 不能绕过门、已收 Offer 随 control CLOSED 退休、堵塞 Open 回复箱关闭后 provider report `abandoned=1`；正常控制关闭与 provider stop 归零。Gate 来源登记拒绝及强制 close 失败缺少稳定的正式服务注入入口，沿 Runtime FakeSet 来源拒绝/退款与 typed Runnel close 失败 host 测试、任务 owner 代码路径证明；触发条件是新增可控注入能力或报告来源退款异常，届时补同一责任链验收。 |
| T3 Active 与双向终态 | 当前能力完成；特定故障注入缺口登记于下 | 1024 字节批推进、待提交缓冲、Read 消费进度与 EOF、Write 后端 accepted 与 Watch MODIFY、Cancel/Query/Finish 固定终态和部分范围失败均在真实流验证。审查发现的 Start 前空 EOF 提前成功、Read 已消费进度在 Cancel 时低报、Terminal Finish 等 close 重试的问题已修；正式 init 覆盖 pre-Start EOF、已消费 4 KiB 后 Cancel、阻塞 Finish 与独立 Cancel、提前关数据端；已提交 Active 的重复 Start 幂等，供丢失回复后 Query 恢复。后端 `prepare_write` 故障与强制 close 失败无稳定服务侧注入入口，由 `libfal` 事务原子性及 `librunnel` affine close host 契约、服务任务的失败 owner/等待唤醒结构证明，新增稳定入口或相关故障报告时补测。 |
| T4 正式消费者与组合门 | 已完成 | `libfs` 的 namespace→Open→typed Attach→Start 及读 EOF 前 Finish、写 Finish 与失败 owner 已接 A/B 的正式 `srv_init`。修复后 `just check`、user host (`libfal` 39、`libfs` 17、`librunnel` 20、`libexecution` 24)、七面 clippy 和 `just acceptance`（debug stress 16/16、release core、sifive_u core、virt-nofd、boot-failure）通过，最终聚合日志 `artifacts/f3e-final-acceptance.log`；结构复核见下节。 |

本轮开发日志：最初 Open Sent 被误判为 abandoned、控制 Outbox 成功后先检查 `is_admitted()` 导致 provider 退出停驻、Write Offer 将仅适用于 Read 的终点校验套用到 Write，均由真实 virt 定位并已修复；失败原始日志位于 `artifacts/failed-acceptance-20260923-161606-21143.log`、`artifacts/failed-acceptance-20260923-161938-24877.log` 与 `artifacts/failed-acceptance-20260923-161801-22979.log`。扩容后 stress 原固定 105 请求未再填满 Task pool，探针增至 150 并延长堵塞回复请求期限，仍要求 Quota、业务恢复和完整退出；`artifacts/f3e-virt-stress-2.log` 通过。后续新增 Watch MODIFY 验证要求 fixture Stream 节点具有 WATCH 权限，`artifacts/f3e-watch-core-2.log` 通过；正式锚点在 `tools/qemu-acceptance.sh`。

### F3e 交付前结构检查

- **范围与消费者**：本阶段只交付现有 Stream 的 Read/Write 单工 Open，RNL2 首个固定几何，不承诺 Copy、append/truncate、持久化、原子替换或跨 provider 快照；`libfal` 公共 op/严格 codec、`srv_fs` A/B、`libfs` 组合与 `srv_init` 正式消费同时接通。F3f Copy 是下一闭包，不能把本阶段延迟接口当 Copy 完成。
- **所有权与状态**：GrantTable 只授权 Open，minted sender context 只授权同一主 Mailbox 的流控制；StreamTable 唯一持 NodeRef/授权快照/role/待写块/业务进度/终态/charge。任务只持准备期 owner、Outbox、来源回执及调度期限；Open 中途失败和响应 Abandoned 先撤来源再关闭 role/Invitation/sender。终态先排 Finish 唤醒，再按来源注销→角色关闭→保留结果壳→control CLOSED/会话到期退休，close 失败不移除表项；未引入锁或跨 hart 共享状态。RNL2 的 tail/head 不作为第二份文件状态。
- **资源、边界与失败**：S=15 的本地并发政策对应最多 45 Tunnel 物理页、30 额外 Task、60 额外 Source；Stream/WaitSource/Bytes、Outbox 回复和 Watch wake 预付。单次 1024 字节进度，Read EOF 需消费确认且 Cancel/到期前刷新消费尾；Write 从环取出到后端 commit 前始终由待提交缓冲持有，成功提交才发布 MODIFY。正常/Quota/Resource、取消、未 Attach、Start 丢失/重试、部分范围失败、数据端关闭、回复堵塞及 provider stop 由现有消费者和协议/host 证据覆盖；无稳定服务注入的来源拒绝、强制 close 和后端准备失败按 T2/T3 的 owner、触发条件与补验标准留唯一记录，不以虚构测试运行体补齐。
- **独立审查与复核**：只读 reviewer 对此工作树指出三项：Start 前空 EOF 提前 Completed（P1），Read Cancel 已消费进度低报与 Finish 唤醒晚于 close 重试（P2）。三者已修，分别以正式 pre-Start EOF、4 KiB 后 Cancel、阻塞 Finish/Cancel 剧本及源码中唤醒位于来源撤销/角色 close 之前复核；重复 Start/Query Active 的回复丢失恢复也加了幂等门。`git diff --check` 无空白错误，`just check`、user host 100 项及修复后完整 `just acceptance` 通过；`cargo fmt --all -- --check` 在原有 shared/boot.rs 等无关文件不通过，未为此重排全仓。未引入旧路/临时第二 Mailbox，也没有新增待删除 adapter。

### 状态与完成责任

| 状态 | 必须持有的责任与约束 |
|---|---|
| Preparing | 已鉴权/pin，预留流/观察/回复/内存；失败零发布并归还 owner |
| Offered | Invitation 与控制能力待交付或已交付，有限建立期限；Start 前不修改文件数据 |
| Active | 有界非阻塞推进，背压只挂起该流；数据端与控制端变化都能唤醒收束 |
| Terminal | 固定业务状态和已确定进度，Query/Finish 按协议取得最终结果；不是资源已释放 |
| Retiring | 先结束访问与注销观察，再关闭 Endpoint/未消费 Invitation、释放 pin/额度；失败继续持 owner |

Read 发布 EOF 后保留端点；成功 Finish 需要确认对端消费到最终 head。普通读 API 返回正常 EOF 前必须确认业务成功。Write 发布 EOF 后还须等待 provider 消费并完成后端工作；Finish 成功只表示接受字节，不默认持久化。PEER_CLOSED 不表示成功，Drop 不等于 Finish。

业务最终状态和后端错误通过 StreamControl 返回，不往 RNL2 共享 header 填文件状态或取消标记；Runnel 继续只拥有字节传输、EOF 和协议终态。

### 失败与完成门

- 未投递回复、入箱未接收、未 Attach、未 Start、Attach 前后失败、Start 已提交但回复丢失、控制权消散、数据端关闭、Cancel、空闲到期和 provider 退出均落到同一业务终态/退休协议。
- 全部 typed failure 保留未消费 Invitation 或已消费后的 Endpoint owner；关闭失败保留资源和 charge，不能删除表项假退款。最终结果未确认前 provider 退出只能报告失败/未知，不能报告成功。
- 真实 provider 与正式流客户端证明 Read/Write、零长度、超过一页且反复跨环、范围边界、部分错误、EOF/Finish、一个阻塞流不妨碍控制与其他流，以及持续运行中的退款。
- F3e 结束前完成其全部调用者与清理路径；F4 只做独立消费者迁移和组合证明，不承接悬空 StreamControl 或未完成的流退休。

## 10. 流 Copy

### 目标与设计门

由 libfs 组合稳定位置、独占 Create、双端 Open、传输和双方最终结果。沿用已交付的无能力属性 Copy，不把两种数据面的进度强行混成一种回滚承诺。F3f 必须在 F3e 接口与失败语义成立后再冻结类型。

**首轮取舍（F3e 完成后的施工决定）**：Open 增加可选的 `expected_identity`，wire 中零表示按当前名字打开、非零要求在同一次授权 resolve/pin 中匹配 NodeRef；不复用 Delete/Move 的 `(identity,version)` 精确 Expected 去发明隐含的 wildcard 版本。`libfs` 从已解析 `Position` 打开时提供该 identity，Copy 目标从独占 Create 成功回复的 `NodeInfo.identity` 取得条件，成功 Open 后继续使用该 pin，不在搬运时二次按名查询。Create 后同名被替换必须在任何数据出版前回 Conflict；名字已移动/删除可返回 NotFound，不承诺按旧名找回原对象。NodeId 仅在同一个实际父 grant 对应的 provider 内比较，不能将 A/B 相同数字判为同物。首个 Copy 入口显式接收源 Position、目标父 DirectoryGrant 和单组件目标名；已有 Create 仅在 grant 根下建单组件，任意深路径的父授权获取不由 Copy 偷加 DERIVE 需求。未来泛路径入口须在真正持有目标父 grant 的消费者出现时另定。

**目标与失败政策**：Create 默认独占，Exists 不覆盖。Create 前以可失败分配保存目标父 grant、名字和错误承载；1024 字节搬运缓冲为有界栈状态，Runtime Task/InputBytes 预算在泵启动前预付，失败后的空目标由回执持有。请求未送出时目标 NotCreated，已送出但结果未确认（含成功回复解码失败）为 CreationUnknown，不能盲目重试。成功回复持 `CreatedTarget(parent_grant,name,NodeInfo)`；部分目标默认保留，不自动 Delete 或 rollback。显式清理方法先收束目标流，再 Lookup 当前名字；identity 不同回 Conflict，相同才用当时 `(identity,version)` 发条件 Delete，由后端提交再次核对以关闭 Lookup→Delete 竞态；NotFound 不证明被移动的原对象已销毁，Delete 回复丢失仍是未知。不增加按 NodeId 全域查找、CreateOpen 或原子跨 provider 事务。

**双端执行责任**：一份 Copy 操作状态持源/目标流、target receipt、固定 1024 字节缓冲、阶段、源已读/向目标环已发布进度以及两端 `NotOpened/Unconfirmed/Terminal(info)`。源 EOF 的 libfs 入口先获源业务 Finish 成功，目标才发布 EOF 并等待目标 Finish；双方结果都成功且字节数一致才是业务成功，本地 close 失败单独标清理待续。每端失效须及时停止搬运并试 Cancel 双端，不能等待一端清理成功才处理另一端；全程只用一个绝对业务 Deadline，不靠每批续期。仅用两个 `read_exact_or_eof_until/write_all_until` 阻塞循环会在背压时遗漏另一端退出/用户取消，正式实现须复用 Runtime/Dispatcher 的来源观察并保持一份 Copy 状态机，不建第二套传输运行体。

| 设计项 | 必须明确的结果 |
|---|---|
| 目标创建 | 目标存在即失败；保存本次创建对象的稳定身份/位置。创建已投递但回复未知时不能假定目标不存在或自动重新创建 |
| 源与目标 | 同一对象/别名的判定范围，不能只比较 provider-local NodeId；源并发变化遵循非快照 Read，不声称复制单时刻映像 |
| 双端状态 | 每阶段谁拥有源流、目标流、搬运缓冲、目标身份与两端控制请求；一端失败时另一端取消/退休有正式推进者 |
| 部分进度 | 分清源读取、传输发布、目标后端确认接受的字节；不能以搬入环的数量冒充复制完成；尚未确认的部分显式报告未知 |
| 最终成功 | 两端业务 Finish 均成功才报告 Copy 成功；源在数据送完后最终失败仍是失败，即使目标已有数据 |
| 取消与期限 | 全操作期限不因阶段切换/重试重置；用户取消、provider 退出、退款失败携带阶段、进度和仍持 owner，不变成阻塞清理循环 |
| 部分目标 | 默认不承诺 rollback；裁决保留与显式清理的 API。清理必须核对本次创建身份，不能误删后来替换的同名对象，也不能无权限承诺一定能删 |
| 执行入口 | 首个真实同步或任务消费者决定门面；如同时需要两种驱动，消费同一状态机，不复制两套算法 |

| 施工项 | 当前状态与证据 | 完成前还需证明 |
|---|---|---|
| F3f-A 条件身份与目标凭据 | Open `expected_identity` 的 zero/非零公共 codec、provider 同次 pin 核验和 libfs Position 消费已接通；B 的 CreatedTarget 以独占 Create 回执条件 Open，替代者不能被旧回执打开或条件 Delete。libfal host 39、正式服务同名替换 Conflict、Open 后 unlink 写完、显式清理 Lookup/当前版本/后端提交核验通过；F4 已补已投递 Create 回复隔离与本地编码失败分类。 | OOM/损坏 Reply 仍无稳定正式注入入口，按 F4 保留限制记录。
| F3f-B 双端数据泵与业务确认 | `libfs::client::copy` 一条 Runtime Task 驱动各端 DATA/仅终态来源及可选取消，五来源上限、1024 字节缓冲、同一绝对期限。源 Finish 成功后目标 EOF/Finish，两端最终进度相等才成功；传输源错误含部分进度时立即失败，不等缓冲被错误唤醒。正式 A→B 大流、同 provider、空源、冲突、预取消及 B 已接受至少 4 KiB 后的另一用户线程取消通过；部分目标保留、带 identity/version 条件 Delete 与退款通过。关闭失败保留 SourceSet/Endpoint 快照与 CallOwner；取消 RPC 错误分别存于 CopyFailure，不丢 owner；F4 已补双端独立 Cancel 和 provider CLOSED 观察。 | 强制 backend/close 失败仍无稳定正式注入入口，保留 typed owner 与代码审查证据。
| F3f-C 消费者与组合收口 | `srv_init` 复用 A/B 正式 provider；`just check`、七面 `just clippy`、受影响 host、完整 `just acceptance` 通过。F4 已完成独立 `test_fal`、普通业务断言迁移、退出观察、Create 不确定性与完整故障组合；结构复核与实现文档已同步。 | 当前基本 FAL 交付已闭合；后续只沿唯一 Review 记录或正式触发的验证限制处理。

本阶段可用能力：真实跨 provider 大流及背压、双方正常 Finish、冲突/替代者保护、预置与中途取消、部分目标及本地 owner 可重试清理。F4 已补齐两端业务消费者、Create Sent-Unknown 分类/回复隔离、完成终态、provider CLOSED 后未投递及退出退款；OOM、强制 close、Gate source refusal 因无稳定正式注入入口保留为验证限制，不影响当前基本 FAL 交付闭合。

交付前结构复核：Copy 没有复用无能力属性 Copy 的字节路径；双端独立 RNL2 环各有唯一 Endpoint 与 control，Runtime 按预算推进且退休来源须有回执；无新锁或内核 ABI。共享 Caller 取消只取消本地等待，不撤销 Sent 副作用，ReplyCleanup 关闭失败保留双 owner；Copy 在清理 Cancel 出错时把源/目标 CallError 留在失败对象，不丢请求。审查曾发现 Attach 失败遗失 Invitation/Endpoint 与 stream/control 关闭失败丢 owner、预取消建错目标、中途取消无法打断 RPC、带部分字节源错误可能停驻、Runtime 构造失败丢 WaitSet，均已沿原路径改正并通过上述分层验证。仍无正式可控的堆 OOM、故障 close、损坏 Create Reply 或 provider 中途退出入口；F4 `test_fal` 应在现有真实进程/后端边界构造必要退出场景，其余注入仅当可定位具体故障并可复现时增加，不建立测试专用服务或运行体。

## 11. Watch 与 provider 接缝的组合责任

本节不另建 Watch 任务；F3c 已有实现，后续调整归 F3d，流修改事件接入归 F3e。

- 原有 Watch 的范围、授权及静默退出契约保持。目录直接成员变化与节点属性修改是不同事件源；服务投影若需要目录级失效，须从其实际可见状态提交点产生，不假定普通属性 MODIFY 会向父目录冒泡。
- 发现客户端采用先订阅后快照。枚举 cursor 冲突重读，记录删除重建重新定位；不能通过销毁再新建订阅引入静默窗口。目录 provider 关闭必须有退出或显式重获 authority 的责任，不只等待自己的 Notification。
- Watch 容量与任务发布按第 8.6 节改为配置准入、预付 WakeBatch 与分批兑现；全部普通 mutation、注册、取消及终态发布者一起迁移，不扩大 Runtime 单次 Gate 上限。
- provider 接缝由 MemoryBackend 与服务目录两个真实语义共同定形。复用授权、运输和任务机制，不要求所有后端具有普通文件写语义，不以回调把服务状态反向灌入 libfal。
- F3e 的 Write/最终接受进度如何推进节点版本和 MODIFY，由实际后端提交语义确定；不能因为传输有正进展就发布虚假的文件修改。

## 12. F4 独立消费者与整体完成门

### 测试所有权

F4 建立独立 `test_fal`，迁移 srv_init 内的业务断言、协议拒绝及失败布景；init 保留启动、capability 装配、root 监督与最终收束。拓扑至少包含两个独立 provider 进程及不同后端/路由域，通过正式启动 grant 组装 namespace，不能共享同一后端对象冒充跨进程。provider 保持正式 Runtime/后端，不建立测试专用服务、延迟 opcode、同步 self-pump 或第二套运行体。F3 施工中已有计划内消费者可保留，但本阶段不得替其补尚未实现的生产失败路径。

### F4 阶段协议与施工

采用 init 装配/监督、`test_fal` 独占业务断言的有限阶段协议：A Ready 后授予 A root、服务发现目录及双向 Notification，交付的两个 sender 均裁剪 `GRANT`，测试进程先装目录 Watch 并报 `DiscoveryArmed`，init 才使用原 scoped authority 正式启动 B；测试进程自行从 ServiceRecord 取得 B grant、核对独立 provider 并报 `SecondaryDiscovered`。init 核对 B bootstrap/注册、准备 A/B 现有 fixture，并等待 `Bind("second", B)` 成功回复后，才发送唯一的 `Continue`；消费者由此可安全解析委托路径。`Complete` 仅在本组业务断言与本地清理成功后报告；随后消费者等待 init 发出的 `OBSERVE_SHUTDOWN`，观察 B grant CLOSED，核对 CLOSED 后新请求未投递并报告 `ProviderClosed`，init 再核验正常退出并收束 ProcessControl，继续 provider 正常停止。阶段等待同时观察报告和子进程 REAPABLE/CLOSED，使用有限绝对期限；测试进程没有 JobControl、ProcessControl、SystemReset、Pool 或 B 直接根能力。此协议归 `test_fal` crate，不进入内核 ABI/FAL 业务协议。

| 闭包 | 当前状态与后续门 |
|---|---|
| F4-1 真实进程与装配 | 已完成：`test_fal`、workspace/initfs/七面 clippy、必选镜像与 A/B grant/阶段顺序已接通；子进程 Watch→Record→B 实际调用、单独成功退出与根监督通过。保留构建目录完整 ELF，仅对 initfs 副本去调试段，payload 从约 44 MiB 降到 8 MiB，128 MiB sifive_u 的 A+B+test_fal 不再 Spawn QuotaExceeded。`just acceptance` 全矩阵通过，随后 F4-2/F4-3 已完成。
| F4-2 业务断言移交 | 已完成 | `test_fal` 承担 Watch/Record/Handle/Move/Open、普通流内容/冻结终点、正常及变体 Copy、回复隔离与客户端清理；init 删除普通 Copy/写入回读断言，仅保留 Cancel/Query/Finish、fixture 装配、ProviderReport、ProcessControl 和监督所需的控制动作。完整矩阵见 `artifacts/fal-f4-acceptance-final.log`。
| F4-3 退出与确定性故障 | 已完成 | `CallOperation` 具备 `Unsent/Sent/Completed` 终态，直接丢弃已发送操作会隔离 ReplyPort；`libfal::ClientOperation` 和 `call_classified` 已接通，Create 本地失败/未投递/已投递未知/明确拒绝分类接入 Copy。正式 A/B 验证已投递 Create 回复隔离、完成后不可推进、provider CLOSED 后新请求未投递、双端 Cancel owner 和有限清理期限；provider 退出报告及 FAL/Execution/ServiceResource 归零通过。`just check`、七面 clippy、完整 acceptance 已通过。未具稳定正式入口的 OOM、强制 close、Gate source refusal 仍作为验证限制保留。

### 结构优先与回顾简化

本轮中途方向检查针对 HEAD `ff73a9e` 及未提交工作树，未重跑代码验收。用户确认：成熟系统的长期结构正确是最终和首要目标；less is more 要消除的是通往正确结构过程中的冗余、机制缠绕与绕路。设计期充分考虑长期目标，允许提前铺路，不以最小机制、最小步进、当前消费者数量或抽象数量限制方案空间。简化主要在实现成形后的审阅中依据完整责任链判断，不能预先把尚未看清的结构裁成局部最小方案。原型期发现根本结构不合适，应整体重构相关责任链，不能为保存错误结构叠加补丁。本轮仅更新约定、计划与相关 ideas/impls，不构成代码修复或 F4 行为验证。

- 保留稳定节点身份、条件操作、未知副作用、EOF/Finish 区分和明确退休责任；它们解决具体可达问题。实现这些契约不要求复制状态机、叠加 facade 或为每种失败建立独立机制。
- RPC→FAL→Create 已作为纵向责任链闭合：单一 `CallOperation` 状态/所有权模型由同步入口复用，`libfal` 暴露分步推进与结果分类，Copy 消费未发送/未知边界。它不是公共设计的能力上限，后续消费者继续复用该模型，不在业务外围补隔离和清理分支。
- `libfal::provider` 当前提取范围有已验证依据，Read/Delegate/Request 留在宿主并非天然欠账。是否进一步提取，按长期 provider 模型与宿主政策边界判断；未来用途本身是有效依据。A 单 route 槽、Copy 局部 Runtime 和验收阶段协议可暂留，但须随长期结构复核其替换条件，既不预先判为必须通用化，也不以只有一个消费者为由拒绝调整。
- 故障验证优先复用正式 A/B、监督能力与公共接缝。新增机制应能以生产系统目标和独立契约解释，不能只有“方便注入某个测试”这一理由；缺稳定注入条件时记录证据边界。未来能力的合理铺路无需等待即时消费者。

下表唯一承载此次方向检查的续作项。尚未完成的接线不直接判为架构失败；当前正确性问题在 F4 交付前闭合。简化项是基于现状提出的审阅假设，不是设计期删除指令：须待相关结构成形，证明替代方案保持长期结构正确且降低理解/修改成本后再决定。发现更好结构时按责任链整体调整，不把每项建议拆成局部补丁任务。

| 性质 / 现状与位置 | owner、目标与触发 / 删除条件 | 验证与顺序 |
|---|---|---|
| 已完成：`librpc/caller.rs` 的 CallOperation 终态与旧 ReplyPort 隔离 | F4-3 已闭合；单一状态/所有权模型由 `Unsent/Sent/Completed`、Drop 隔离及显式 abort 组成，`libfal` 分步入口复用；删除条件已满足 | `test_fal` 证明已发送后放弃、迟到回复、随后调用隔离及完成后不可推进；target 构建和完整 acceptance 通过。
| 已完成：`libfs/client/copy.rs` 本地编码错误误归 CreationUnknown | F4-3 已闭合；准备失败、未投递和已投递未知由 `ClientCallFailure` 单一分类出口提供，Copy 不再维护错误种类猜测表 | 超长名称本地失败、未投递、已发送冲突回复隔离均由 `test_fal` 正式 A/B 验证。
| 已完成：FAL backend/route 路径与边界分配 | Lookup/Link 改用切片迭代；跨等待保存的边界文本和 route 名称显式可失败预留，Resource 错误沿 owner 出口返回 | `just check`、七面 clippy、完整 acceptance 通过；无稳定 OOM 注入，保留代码审查证据边界。
| 已完成：Copy 双端清理串行与业务期限复用 | 两端 control Cancel 各自建立独立 owner；清理使用有限期限且不晚于原业务期限，不把未知结果转为成功 | 正式 A/B Copy 变体和 provider 退出组合通过，双端账户归零。
| 简化候选：Registry `root_version`/`cursor_epoch` 当前同步变化；TargetLocator `provider_id` 尚无仓内消费者；srv_fs 私有 RuntimeBackend 仅一个实现 | 各领域 owner 在设计复核或编辑时说明各自的独立语义、未来用途与变化边界。若两代次确属同一事实则合并；若 provider 身份无独立用途则删字段/syscall；若 RuntimeBackend 仅传播宿主知识则重定边界或具体化。消费者少或当前同值本身不作为删除依据；F4 结构检查记录去留理由 | 比较替代方案是否仍支持长期模型、是否减少机制间牵制，复用枚举失效、条件操作和 provider 组合检查；需结构调整时整体完成，不额外堆 adapter |
| 已完成：init `exercise_open_streams` 普通业务断言与监督动作混合 | 普通流写入/回读/Watch/内容断言已移至 `test_fal`；init 保留 fixture、route、ProviderReport、ProcessControl、Cancel/Query/Finish 及退出监督 | `test_fal` 阶段握手、`ProviderClosed` 观察和完整 acceptance 通过；无新增等待环。

控制 sender 消散导致已停驻 Finish 丢失唤醒的疑点不成立于所述路径：请求 Outbox 持 Delivery 保活授权，不能据此新增流退休状态。fixture 自检深度清理仍由其独立计划拥有；本轮不扩大到全内核或全部库重审。

### 验证矩阵

| 面 | 确定性断言与责任归属 |
|---|---|
| 既有授权/路径 | 真 sender context、同 badge 不同身份、转交后原进程退出仍可用、根逃逸/权限放大拒绝、Delegate 衰减、rename 位置冲突、链接/路由总预算；F1/F2 回归 |
| 后端/Move/属性 Copy | 准备失败零提交、冲突归还 owner、同域 Move 防循环/双权限、CrossDevice、Copy 不覆盖、旧值/摘链有界退休；F1–F3b 回归 |
| Record/能力 | 真实 Record 往返、多层 Array/Record、多能力槽、重复/遗漏/错误 role/rights 拒绝、repeatable/affine 区分、AcquireCapability 交集、Take 成功及投递失败恢复；Directory 出口按实际支持矩阵证明 |
| 注册/发现 | 注册后 Ready 前不可取得 endpoint、完整快照后实际调用、同名争用/条件撤出再独占注册、旧实例清理、新旧快照与 Drain 竞争、已授 sender 不随摘名失效；F3d |
| 发现/Watch | 安装后回复前变化、先订阅后读、普通属性 MODIFY、服务新实例发布/删后重建使正确消费者重读、代次冲突、取消后静默、owner/目录 provider 关闭；F3c/F3d |
| Open 建立 | offer 到期、不 Attach/Start、Attach 失败两类 owner、迟到/重复 Start、回复 abandoned、连接权限与资源不足；F3e |
| 流与最终结果 | Read/Write、零长度、多页多次跨环、并发节点变化、背压、部分后端错误、EOF 与 Finish 区分、Query/Cancel 竞态、数据/控制端消散；F3e |
| 双端 Copy | 双方正常 Finish、一端背压另一端退出、源最终失败但目标有数据、目标部分接受、创建冲突/未知、取消/超时、清理不误删替代者；F3f |
| 公平与资源 | 多客户端/多流与发现/Watch 并行，停驻任务不阻塞控制，预算耗尽是可恢复拒绝；观测每类 owner 实际释放后的退款，不用 Kill/Drain 替代正常清理 |
| 退出组合 | 请求未投递、已投递、业务已提交/回复未投递及最终结果待取阶段的 client/provider 退出；Delivery/Outbox/观察/下游/映射/授权/账户和监督责任全部收束 |

公共 ABI 的确定性回归继续使用既有生产自检/测试，不复制旧夹具：消息原子 move/Receive 回滚及 Delivery/Lifetime 连续寿命、WaitSet 超过单次 WaitMany 上限的观察/代次/移除/Close、期限与迟到回复隔离、有界 ProcessDrain 和真实跨 hart 退出。改变这些边界时按对应档案扩大验证，而不是只看 FAL 成功日志。

### 验证、Review 与归档

- 施工开发检查：受影响 host 纯逻辑测试（显式 `--target aarch64-apple-darwin`）、target 检查与 `git diff --check`；构建统一经 just，阶段结果不构成独立整体验收。
- 当前承诺全闭合后：`just check`、`just clippy`、相关 os/shared/user host 与完整 `just acceptance`，覆盖 debug stress、release core、sifive_u、virt-nofd 和 boot-failure；涉及调度域契约另跑 virt-hetero。新增库/独立 binary 同步纳入 workspace、构建、lint 和 workload，不遗漏 target 分面。
- 长命令完整日志写 artifacts，保留退出码；默认节流、超时按现行 recipe。正常负载成本变化时重校超时，不把截断判为内核故障；退出后确认无残留进程。历史通过或全速诊断不替代本次需要的组合证据。
- 结构收口核对最终类型/authority/owner、重复真值、旧路径、失败退款、锁序、有界性、文档及已有 findings；当前承诺已闭合，剩余仅为明确记录的外部注入验证限制，影响当前正确性的事项不得延期。
- F3d–F3f 的真实消费者、全部清理、旧路径删除及本节组合门已成立，整体基本 FAL 交付完成。同步 impls/COMPASS；本计划保留至提交前摘要与归档手续完成，提交前展示摘要并取得授权，提交后登记固定提交 Review。commit、合并、push 分别取得授权。

## 跨会话接力断点

当前工作树基线 HEAD `ff73a9e`、分支 `task/fal-service-capabilities`，F3d–F3f 与相关验收 fixture 改动尚未提交；保留现存所有未提交改动，提交/合并/push 仍需分别授权。F3d 登记/发现、D1–D5 findings 及验收证据见 §8；F3e 的公共 Open/Stream 与 A/B provider 收口见 §9。

F3f 已在正式 A/B 进程接通条件 Open、`CreatedTarget`、同/跨 provider 流 Copy、单 Runtime Task 双端泵、可选外部取消与 typed 清理 owner。最新入口与剩余注入限制见 §10；core/stress 中 B 已接受至少 4 KiB 后的用户线程取消、预取消、不误删替代者、Unsent/Sent RPC owner、host 122 项、`just check`、七面 clippy 与修复后完整 `just acceptance` 均通过。阶段日志：`artifacts/f3f-final-check.log`、`artifacts/f3f-final-host-tests.log`、`artifacts/f3f-owner-final-acceptance.log`；最新 F4 代码的整体验收见下文。

F4 当前接线已闭合：`srv_init` 的 `launch_fal_consumer`、`launch_secondary_fs` 和 `run` 掌握 A/B 启动、阶段通知、ProviderReport、ProcessControl 与 ProviderClosed 监督；`user/tests/test_fal/src/{lib,main}.rs` 拥有独立 A/B 业务断言、Create 不确定性/回复隔离、普通流内容/冻结终点、完成终态和 provider CLOSED 后未投递观察。`librpc::CallOperation` 以单一状态机提供发送/回复/Abort/Drop 收束；`libfal::ClientOperation` 暴露分步调用和分类结果；`libfs::client::copy` 消费分类并双端独立清理。`test_fal` 的 A/目录 sender 裁掉 `GRANT`，B grant 仅由 ServiceRecord 派生，不将 scoped registration authority 或 release/监督权交给消费者。

当前承诺已完成：F4-3 调用模型与 RPC→FAL→Create、F4-2 业务断言移交、F4 退出与故障组合均已接通并通过结构复核。
最终收口已完成：按 REVIEW 核对契约、owner、失败收束、必要复杂度、临时结构及删除条件；`just check`、`just clippy`、受影响代码构建与完整 `just acceptance` 均通过。剩余 OOM、强制 close、Gate source refusal 仅因没有稳定正式注入入口保留为验证限制，不影响当前已承诺路径的结构闭合。

最终完整代码验收：`artifacts/fal-f4-acceptance-final.log`，包含七面 clippy、virt stress 16/16、virt release core、sifive_u core、virt-nofd 及三类 boot-failure；`artifacts/fal-f4-check.log` 与 `artifacts/fal-f4-clippy.log` 为对应静态证据。

| 保留限制 / owner | 触发与删除条件 |
|---|---|
| F3d 正式服务来源登记拒绝难以稳定注入；owner 为 §8/F4 | 现以 Runtime host Gate/退款、注册 owner 链及真实 stress 覆盖；仅在真实准入拒绝可稳定复现时补独立验收，不能假称已注入。|
| F3f Close/OOM/损坏 Reply 缺稳定入口；owner 为 §10/F4 | 现由 typed 返还与 host/真实取消/结构核对证明本地责任；F4 创建真正双 provider 消费者后构造可控故障，若对应分支仍无法稳定触发，记录具体边界与复核条件，不能将缺口算作已验。|
| F3f provider 中途退出、最终失败及未知创建的系统矩阵 | F4 已在 `test_fal` 正式 A/B 验证已投递 Create 未知、provider CLOSED 后未投递、双端取消与退出退款；稳定入口缺失的强制最终后端失败/close 注入仍由 typed owner 与代码审查覆盖，待未来出现正式触发条件时复核。 |

验收 fixture 的两个映像因独立覆盖暂留，后续内核自检审计由其[独立计划](todo-2026-09-23-acceptance-fixture-cleanup.md)承接。
