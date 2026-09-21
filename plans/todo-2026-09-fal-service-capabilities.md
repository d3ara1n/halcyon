# FAL 服务能力与公共 IPC 前置

> 状态：公共前置、F0–F2 与用户态库重排已完成；F3a–F3c 已实施并有开发及组合检查证据，整体 FAL 尚未交付。第 4 节跨阶段设计与第 8 节 F3d 详细设计已闭合，下一步为 F3d 实施；F3e/F3f 各自开工前仍需局部详细设计。当前只有文档变更，没有新增运行代码或验收证据。
>
> 本文件唯一拥有基本 FAL 的剩余设计、施工、残留与整体完成门，不另建平行设计计划。统一流程遵循 `AGENTS.md`；方向契约进入 `notes/ideas/`，实现事实进入 `notes/impls/`，本文只记录任务特有的决策、依赖和证据。固定提交 Review 保留原证据，不重复安排本文件拥有的实施。

## 1. 接手基线与证据

### 当前代码与文档基线

- 开发分支：`task/fal-service-capabilities`，从本地 `master` 的 `5d406a4` 分出。
- F1–F3c 连续实现基线：`dfcf7a349fe6d9e2836bb7c8179ff7e96c8ce20a`；库知识、目录与命名重排：`96ee03b0d1641c86ed8ab05951bad6954ea84db4`；本次文档修订接手 HEAD：`3607f22`，接手时工作树干净。
- 当前实现入口：[`FAL`](../notes/impls/fal.md)、[`RPC`](../notes/impls/rpc.md)、[`Runtime`](../notes/impls/runtime.md)、[`Runnel`](../notes/impls/runnel.md)、[`启动`](../notes/impls/startup.md)。组件命名与依赖先读 [`user/README.md`](../user/README.md) 和 [`user/libraries/README.md`](../user/libraries/README.md)。
- 方向入口：`notes/ideas/{fal,fs,service,framework,message,wait,time,rpc,tunnel,runnel}.md`。方向文档不是实现完成证据；当前代码也不自动决定未来边界。

| 基线 | 已成立的责任与证据入口 | 后续不得误读为 |
|---|---|---|
| 公共对象、观察与退休 | [公共前置档案](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md)、`notes/impls/ipc.md` | 需要恢复旧 WaitSet Seal/Drain 或用户态退休编排 |
| 公共时间 | `c6e0a84`；[期限档案](archived/todo-2026-09-monotonic-time-rpc-deadline.md)、`notes/impls/time.md` | FAL 已有完整连接/运行期政策 |
| 消息、流运输、执行与 RPC/Outbox | [执行前置交付导航](todo-2026-09-13-service-runtime-prerequisites.md)；Runtime 是观察与任务寿命 owner，Runnel 已有 poll/SourcePlan 及 init↔pm 消费者 | 正式文件 Open 已接通，或须重建 Runnel 观察层 |
| 公共操作所有权 | `8e0467a`；[公共操作档案](archived/todo-2026-09-14-public-operation-ownership.md) | 业务已拥有注册或流状态机 |
| F0–F2 | 严格 FAL2、GrantTable/稳定节点、双独立 provider、route/Delegate、下游退出与 Outbox abandoned；[固定基线审查](todo-2026-09-21-fal-library-baseline-review.md) | route-management endpoint 已是注册权威 |
| F3a–F3c | 同域 Move、Record/Handle/Take、无 capability 属性 Copy、Watch；同一固定基线与 `notes/impls/fal.md` | 注册/发现、Open、流 Copy 或整体 FAL 已交付 |
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
| 剩余能力设计闭包 | 跨阶段边界与 F3d 详细设计已闭合 | 已核定 A 注册承载、B 发布、init/test_fal 发现及流消费者 | 第 4/8 节产物；F3e/F3f 局部设计仍有独立开工门，不宣称能力交付 |
| F3d | 设计完成，待实施 | F3b Record/出口、F3c Watch、公共 Runtime/记账；A/B/init 正式拓扑 | 第 8 节发布到实际调用、失效、退出及退款；含后端/Directory 出口/WakeBatch 接缝 |
| F3e | 待详细设计与实施 | F3b、typed Tunnel/Runnel、Runtime/Outbox；provider 与正式流客户端 | 第 9 节双方数据/控制/退休闭合，不制造新运输运行体 |
| F3f | 待详细设计与实施 | F3e、稳定位置与独占 Create；libfs 双端复制调用者 | 第 10 节正常及双端部分失败闭合 |
| F4 | 验证矩阵已登记，尚未实施 | F3d–F3f 全部真实消费者与旧路径删除 | 第 12 节独立测试、结构收口、组合验证和归档 |

公共接缝调整由首个真实使用它的闭包承担，不按文件或 opcode 拆成独立交付。若发现可独立证明且确实缺失的公共前置，回到规模审计后立案并链接，不预建泛化框架或平行方案。

## 4. 剩余能力设计闭包

### 目标与责任

本任务拥有跨 F3d–F3f 的责任审计与设计，不重审全部已交付公共机制，也不代替各闭包开工前的详细设计。设计实施者负责决策；普通内部取舍自行裁决，改变已确认外部语义、无法闭合或与 notes 目标冲突时再交用户确认。

起始代码落点：

- `srv_init/src/main.rs`：双 provider 启动授权、bootstrap root grant、route 装配与验收消费者；监督 owner 在 `supervisor.rs`。
- `srv_fs/src/server.rs`：唯一 World/Runtime，直接持有 MemoryBackend；Ingress、RequestTask、Outbox、GrantTask、DelegateTask 与 WatchTask。
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
| 流客户端到 Copy | init（以后 test_fal）通过 libfs 正式流客户端操作 A/B；Copy 的唯一状态机组合稳定位置、两端 Open/Runnel/最终结果 | 第 10 节 API、两端停止顺序、结果结构；不新建复制服务或阻塞泵 |
| 付款到退出 | 各 provider 的可信授权账户支付自身元数据/派生 grant，PoolBinding 支付将来的流 backing；客户端支付自身缓冲/执行。Runtime 管观察，业务 owner 管状态/退休，init 持根监督 | F3e 的映射/Invitation/Endpoint 成本，F3f 的缓冲及未确认进度；开工前预付，不能留给全局 mapping 延期 |

跨阶段的线性化顺序固定：发现完整 snapshot 取得 → 异步能力派生/回复交付；Open 的稳定节点鉴权/pin → offer 交付 → Attach 确认及 Start → 后端接受/业务结果 → 退休；Copy 的目标独占 Create → 双端连接/搬运 → 两端业务成功 → 自身退休。传输入箱、环进度、业务提交、已知结果与退款互不替代。Open 回复 slot 0=StreamControl、slot 1=Invitation 保留为 F3e 首选候选，需按第 9 节完成 owner/失败设计后才成为 wire。

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
- [x] 第 8 节给出 F3d 可开工设计；第 9/10 节保留 F3e/F3f 的局部设计门，不预建无消费者类型或将候选当已发布 wire。
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

本表是本专题尚待核定/接通的责任，不把全部已有实现定性为临时代码。owner 指闭包实施者；每项在对应闭包开工时补齐具体类型、路径和清理证据，完成后从表删除或转为长期 notes。新增临时结构必须同时登记替代物、期限、删除条件与验证。

| 现状与位置 | 目标与责任 owner | 触发、保留期限与完成/删除门 |
|---|---|---|
| srv_fs World/serve_v2 直接依赖 MemoryBackend，Dispatcher 完成路由硬编码 Delegate | F3d 按 §8.2 提取 libfal Backend/provider 与通用任务完成路由；F3e 续用 | MemoryBackend 和 Registry 两个真实后端接通，删除请求层 Body 解释和 Delegate 专用路由；不复制授权/Outbox/观察/退休 |
| 无 ServiceRecord/RegistrationControl/发现调用者，无 libservice crate | F3d 按 §8.1–8.4 接 A Registry、B 发布、init 发现后调用 | 同闭包闭合所有退出责任；删除 B bootstrap 直接交付 root 的业务旁路；不留空库或全局 registry 占位 |
| DirectoryGrant 普通 Record Read/Take 返回 Unsupported | F3d 按 §8.5 通用异步派生出口，MemoryBackend/Registry 同时消费 | repeatable Read 必须接通；affine Directory Take 保持明确 Unsupported，capability 属性 Copy 不扩大范围；不改标 Mailbox 绕过 ceiling |
| GrantTable 保存并继承 output_transport，但 AccessSnapshot/Record Read/Take 未消费，output_rights 无调用者 | F3d 在共同出口接通实际运输上限，不保留影子政策 | 明确该上限仅为 TRANSIT/GRANT 位；快照携带它，Read/Take 在副作用前验证字段转授位是其子集；业务 rights 仍由字段 policy/目标协议决定。移除未使用 getter，补双后端与 Take 拒绝/退款验证 |
| Watch 发布表固定 8 项，WakeSet 与 Runtime 请求容量耦合 | F3d 按 §8.6 配置准入 W、预付 WakeBatch 并分批兑现唤醒 | 迁移全部变更/取消/终态发布者；验证 W 超单步容量且 wake debt/Charge 无丢失，不能只扩大数组 |
| Open/StreamControl/provider 数据任务尚不存在；Runnel 观察已有真实消费者 | F3e 组合既有机制形成正式文件流 | F3e 全部 owner 与消费者共同接入；不恢复 raw 工厂、旧观察草稿或第二套 WaitSet |
| 仅无 capability 属性 Copy 已接通 | F3f 建立双端流 Copy 与结构化部分结果 | F3e 闭合后施工；F3f 不留下临时循环或无 owner 的失败目标 |
| FAL 业务断言仍在 srv_init；启动/授权/监督是正式装配，route 当前仅一个绑定槽 | F4 将业务验收迁至 test_fal；init 保留正式装配/监督 | F4 前剧本保留；迁移后删除重复断言和测试专用裸包布景旧调用。固定消息/容量是验收政策，单 route 不冒充通用注册服务；扩容仅由真实拓扑触发 |
| Record 生产往返、多嵌套能力组合尚缺完整独立验收矩阵 | F4 对 F3b 正式 API 补组合验证 | 第 12 节逐项证明完整快照、槽/权限/失败 owner，不借测试引入第二套实现 |

## 8. 服务注册与发现

### 8.1 首个真实拓扑与启动顺序

选择现有两个 `srv_fs` 进程，不改变 pm 的业务协议，也不新建注册服务 binary：

```text
init（启动、根监督、A/B 的可信付款装配）
 ├─ direct grants → A：bootstrap / release / route，配置为目录承载者
 │                    └─ 一个 Runtime + Dispatcher
 │                       ├─ 内存 FAL Mailbox / GrantTable / MemoryBackend
 │                       ├─ 发现 FAL Mailbox / GrantTable / Registry 后端
 │                       └─ 注册控制 Mailbox / AuthorityTable / 注册任务
 ├─ A bootstrap → init：内存 root、只读发现 root、注册根 authority
 ├─ 注册根 DelegateName("fs.secondary") → exact-name authority
 └─ direct grants → B：bootstrap / release / route / exact-name authority
                      └─ 内存 FAL provider + 异步发布任务
                         Register(自身 FAL2 DirectoryGrant) → PublishReady
init：先订阅 A 发现根 → 读取 fs.secondary → 取得 B 派生 grant
      → 正式 FAL 创建/读写/枚举 → 给 A 装配既有 second route
```

A 的三个入口属于不同 authority；内存域与发现域即使共进程，Move 仍为 CrossDevice。A 不把注册状态塞入 MemoryBackend，route-management 不取得注册权。Registry 是可选的正式后端组合，不以 workload 分支实现另一套 provider。单域采用平面名称集合，名称是非空单个 FAL component，不接受 `/`、`.`、`..`、NUL；多域由独立根 capability/namespace 组合，本闭包不实现注册子树。

A 完成入口、退休来源和三个根控制权的观察登记后，才以严格 bootstrap 包交付上述三项能力。init 在 B 构造前取得并直接 GRANT 名称授权；B 的发布任务在自身 FAL 入口可运行后通过 Dispatcher 调用注册协议，Ready 提交确认后才发送启动 Ready 报告。B bootstrap 不再给 init 交付业务 root，删除该旁路消费者；B 的业务能力只能经此发现链取得。A 启动失败、B 注册失败或期限耗尽都回到 init 现有 supervisor 收束，不降级为无鉴权启动。

需要在 Building 阶段直接交付的 A 根 grant 和 exact-name authority，发行运输政策必须显式包含 GRANT；普通 RPC 运输使用 TRANSIT，不能把二者混用。首个发布 endpoint 是 FAL2 DirectoryGrant，出口为共享根授权的独立派生能力，并非逐客户端 session。F4 的 test_fal 得到 A 内存/发现根、测试名称的 scoped authority 及必要 release 信号权；init 保留根注册权、ProcessControl 与退出兜底。测试别名发布仍使用实际 B endpoint，不制造测试专用服务。

### 8.2 后端与任务接口

| 落点 | F3d 必须形成的接口与 owner |
|---|---|
| `libfal::backend` | 通用 Backend 契约：授权后的路径步进/元数据/枚举、Watch 安装复查、完整 `ReadSnapshot`，关联 prepared mutation/Move/Take owner、分步验证和无失败 commit，以及 seal/有界 retire。保留 MemoryBackend 实现；请求层不再解释 `Body` 或直接访问其 NodeStore |
| `libfal::provider` | 提取现有 provider 状态、Ingress/请求/Grant/Watch/出口/退休算法，按 Backend 参数化；变更结果携带通用 effects。进程配置与测试政策不进入库，库不依赖 libservice |
| `libservice::Registry` | 同时拥有 AuthorityTable、注册状态/名称索引、投影 NodeStore 和 Publication owner，并实现 Backend；注册入口和 FAL 入口经同一 World 借用该 owner，不做状态镜像或生命周期回调反向注入 |
| `srv_fs` 装配 | 同一 Task::Family 组合两个 FAL provider 与注册/发布任务；共用 Runtime、Dispatcher、等待点和付款布局。下游完成按任务身份/操作种类路由，删除硬编码只回 `ServiceTask::Delegate` 的分支 |
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

1. 接通 Backend/通用 provider 与完成路由，迁移 MemoryBackend 所有调用者和既有 Watch/退出；同时建立完整 snapshot/异步 Directory Record 出口及 WakeBatch，删除具体 Body 访问与 Delegate 专用完成路由。
2. 建立 libservice 的严格协议、authority/实例状态、投影和显式退休；以 Registry 作为第二个真实 Backend，接入 A 同一 Runtime。原有 `StoredValue` 的非 Directory/affine Take 责任保持，未支持类别明确拒绝。
3. 迁移 A/B bootstrap、B 发布任务、init 先订阅—发现—真实调用及 route 装配；删除 B root 直交旁路。覆盖同名测试别名、旧 control/旧快照、自动撤出与静默退出，不等待 F4 才接清理。
4. host 测试严格 schema、authority/状态/条件代次、准备失败 owner、完整快照、版本耗尽及唤醒分批；target 检查全部装配。真实组合断言覆盖注册/发现/调用、Directory ceiling 与独立身份/共同付款、Watch 重读、双方退出、来源/任务/账户退款；与 F3e/F3f/F4 的最终组合门保持区分。

F3d 完成必须证明：低权限发现目录能按被授权出口取得可写 B grant，但不能扩大 B 母授权；旧实例不能删除替代者；pin-before-drain 而 snapshot-after-drain 被拒绝，完整 snapshot-before-drain 可继续；Register 入箱未取有期限；迟到 Ready 无法越过已确认 Drain；控制/endpoint 的两种 CLOSED 正确分工；两域 Move 拒绝；多字段出口部分失败、W 超单步容量和正常停止均无 owner/退款残留。

## 9. Open 与流完成

### 既定语义与公共接缝

Open 打开现有稳定流节点；在一次本地操作内解析目标、校验身份/权限并 pin，不信任先前 Lookup。请求声明 Read/Write、offset/范围、RNL2/几何及连接 Deadline；回复交付协商几何、offer_deadline、独立 StreamControl sender 和 affine Invitation。具体 wire 在本节设计门裁决后分配。

客户端依次 Open → Attach → 协议验证 → Start，共享连接 Deadline；provider 还施加独立有限 offer 期限，核验 PEER_ATTACHED 事实且未到期才启动数据任务。Read 时 provider 是 Producer，Write 时 provider 是 Consumer。第一版 Tunnel backing 由 provider PoolBinding 支付，按可信绑定的授权账户预留流成本，不冒充逐连接客户端 MemoryPool 出资。

### 开工前必须补齐

1. **类型和 owner**：控制身份、NodeRef、账户 reservation、Runnel 单侧角色、Invitation、观察、待回复 Finish、稳定最终结果及退休记录的唯一 owner；StreamControl Lifetime 观察不自持 sender 母本。
2. **节点及并发语义**：rename/delete 后已 pin 的流继续指向原对象；普通非快照 Read 遇到并发写、长度变化和范围终点时具体如何结束。不能把“不保证快照”当作未定义的内存/数据行为。
3. **进度与存储**：区分应用提交、共享环发布/消费和后端实际接受。文件 offset、范围终点和业务字节计数使用 checked 算术，溢出明确拒绝且不回绕，不能套用 Runnel head/tail 的模计数语义。Write 从环取出后尚未提交后端的数据由谁保留；Prepare 失败、通知失败、后端部分失败如何计数，重试不能重复接受字节。
4. **控制状态机**：重复/迟到 Start、Query、Finish、Cancel 的合法状态、幂等边界和返回结果；竞争由单一业务状态拥有者裁决，不按回复先后猜测提交顺序。
5. **期限与准入**：连接、offer、运行期空闲和每次控制调用期限分别约束什么；伪唤醒不重置空闲期限。流、映射、来源、回复、等待者和清理记录必须在允许副作用前预付，有限容量有依据。
6. **终态与保留**：最终结果在控制权仍存活时如何可查询、何时退休；资源释放与结果保留可分开，但不能关闭对端尚需消费的映射或无限保留无主终态。Finish 待回复名额有界，后续查询不创建无限 waiter。
7. **驱动与关闭**：复用 Runnel poll/SourcePlan、Runtime 登记/注销、Dispatcher/Outbox；服务任务不阻塞。关闭的停驻成本与失败继续 owner 明确，不在任意 Drop 隐式执行整条退休。

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

完成门：真实跨 provider 大流、双端背压/部分错误、两端最终失败、创建冲突/未知、取消/超时、双方退出、目标重命名/替换竞态及持续运行退款。所有失败有可解释的目标状态，不留只有错误码而无人持有的流/清理责任。

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
- 结构收口核对最终类型/authority/owner、重复真值、旧路径、失败退款、锁序、有界性、文档及已有 findings；未关问题只有一个延期条目，影响当前正确性者不得延期。
- 只有 F3d–F3f 的真实消费者、全部清理、旧路径删除及本节组合门成立，才能标整体完成。同步 impls/COMPASS、归档本计划；提交前展示摘要并取得授权，提交后登记固定提交 Review。commit、合并、push 分别取得授权。

## 跨会话接力断点

当前已完成第 4 节跨阶段设计及第 8 节 F3d 详细设计：真实拓扑、Backend/provider 接缝、authority/控制协议、状态/投影/快照、Directory 出口、Watch/付款和失败退出均已裁决。新增发现的 output_transport 影子政策也已归入 F3d 接通清单。没有新增运行代码或运行验证；F0–F3c 的历史证据不作为 F3d 证据。

本轮仅修改原有六份设计/实现基线/计划文档；6 份 Markdown 的 80 个本地链接、3 个指向本计划的入向锚点、代码围栏与 `git diff --check` 均通过，已清理过时的“设计未立案/拓扑未定”导航。未运行编译、host 测试或 QEMU；实现门仍未执行，提交状态与当前 HEAD 以 Git 为准。

下一会话顺序：

1. 核对分支/HEAD/工作树、本文件与 COMPASS；设计接手基线为 `3607f22`，六份文档已保存完整裁决，不覆盖后续变化。
2. 在取得运行代码实施授权后，按第 8.8 节从 Backend/provider、完整快照/Directory 出口、output_transport 和 WakeBatch 开始，共同迁移既有消费者与失败 owner；不要重新立案或另选拓扑。
3. 接通 Registry、A 同 Runtime 双域、B StartupBlock 名称授权及异步发布、init 发现后调用；删除 B bootstrap root 业务旁路，再检查 F3d 全失败/退出/退款责任。
4. F3e/F3f 在各自开工前完成第 9/10 节局部详细设计；最后 F4 汇合独立 test_fal 与完整组合验收，不把任何施工阶段包装为整体交付。
5. 会话中断时写回已裁决/未裁决、代码与文档真值、阻塞与验证状态，不另起平行设计任务，不将局部检查写成整体完成。
