# FAL 服务能力与公共 IPC 前置

> 状态：公共前置及 F0–F2 已完成。F2 已交付两个独立 provider、严格 FAL2 wire/client、DirectoryGrant Namespace、真实跨 provider Delegate、独立 route-management endpoint、在途下游调用与 provider 停止/Outbox abandoned 责任，并删除 FAL1、`MemFs`、slot-1 anchor 和同进程泵。F3a 同域 Move、F3b Record/Handle/Take 与属性 Copy、F3c Watch 已到代码实施、审查修复和 core 组合门。库知识/目录/命名重排已经 `96ee03b` 提交并归档；当前从第 8 节恢复 F3d 服务注册/发现的规模审计与设计闭包。整体 FAL 仍未交付，F4 负责独立 `test_fal` 与最终组合验收。ProcessDrain 的管理者职责与 REAPABLE 触发保持现有契约。
>
> 方向参考：`notes/ideas/{object,message,wait,time,rpc,framework,fal,fs,service,tunnel,runnel}.md`。本文件拥有 FAL 业务与总体依赖/交付导航；公共对象/观察/退休由 [公共前置计划](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md) 拥有，时钟/绝对期限由 [期限计划](archived/todo-2026-09-monotonic-time-rpc-deadline.md) 拥有，运输/RPC/服务执行由 [执行前置计划](todo-2026-09-13-service-runtime-prerequisites.md) 拥有。计划审视由实施者负责，代码 reviewer 只审查代码；提交后登记未来代码 Review。

## 开发分支与交接

历史集成基线提交：`d22b9d71ef810145bf4d5bfb3673ffec8640f361`（`chore(fal): 保存公共对象收口后的集成开发基线`），共 131 个文件。它包含公共对象前置交付与其余草稿，不是最终 FAL 合并提交。固定该提交的未来代码复核见 [集成基线 Review](todo-2026-09-13-fal-integration-baseline-review.md)；当前接手以 `dfcf7a3` 及其后续变化为准，不退回该历史基线覆盖后续改动。

- 开发分支：`task/fal-service-capabilities`，由本地 `master` 的 `5d406a4` 分出；F1–F3c 的代码、文档、验收装配与库重排原则已整体固定为 `dfcf7a349fe6d9e2836bb7c8179ff7e96c8ce20a`，父提交为 `84eeed6`（F0 文档重排）。固定提交审查见[基线 Review](todo-2026-09-21-fal-library-baseline-review.md)，该提交不表示整体 FAL 完成交付。
- 交接入口：先读 `plans/COMPASS.md`、本节和对应专题计划；实现现状看 `notes/impls/`，目标契约看 `notes/ideas/`。会话内任务编号只作临时导航，不能写入项目语义，计划文件是跨会话真值。
- 开发方式：F0 形成文档基线；F1–F3c 的连续迁移以 `dfcf7a3` 整体保存，库重排 L0–L5 以 `96ee03b` 提交并归档。当前从 F3d 规模审计开始，随后依次进入 F3e、F3f 与 F4。每项仍须包含真实调用者、失败/取消/退出/退休/退款、旧路径删除和验证，不以 passing fragment 标完成。保留基线之后的全部工作树变化；后续实现提交、合并与 push 仍须取得用户授权。

| 专题 | 交接状态 | 接手入口与剩余责任 |
|---|---|---|
| 公共对象、观察与退休前置 | 完成，已归档 | `notes/impls/ipc.md` 与公共前置档案；保持内核拥有退休、捕获 epoch/预算、来源锁外交接及准入退款，不恢复 Seal/Drain |
| 公共时间与绝对期限前置 | 已完成并归档 | `archived/todo-2026-09-monotonic-time-rpc-deadline.md` 与 `notes/impls/time.md`；完整期限与运行期协作停止已接通，跨硬件 epoch 连续时间按唯一延后项保留 |
| 运输/执行前置 | 消息与流运输、通用执行/准入及 RPC/Outbox 均已提交并验证 | `todo-2026-09-13-service-runtime-prerequisites.md`；保留各闭包证据和固定提交 Review，不再安排新的用户态执行阶段 |
| 公共操作边界收束 | P0–P6 完成，已归档 | `archived/todo-2026-09-14-public-operation-ownership.md`；两组四类公平性、跨 hart/退出/接管/退款和结构残留均已收口 |
| 库知识归属、目录与命名 | 完成，已提交并归档 | [库重排档案](archived/todo-2026-09-21-library-knowledge-ownership.md)；实现提交 `96ee03b`，固定提交审查见[未来 Review](todo-2026-09-21-library-knowledge-ownership-review.md) |
| FAL 业务 | F0–F2、F3a–F3c 已到开发门；当前恢复 F3d | 第 8 节先审计真实发布者、发现消费者、注册权威、投影、Watch、预算与退出责任，再形成设计闭包；F4 负责独立 test_fal 与整体组合验收 |
| 验收可靠性改进 | 首轮已收口，历史墙钟敏感现象只读归档 | `plans/archived/ref-2026-09-acceptance-timing-flake.md`；新现场命中归档触发条件时重新立案，不以重跑直到绿替代证据 |
| workspace 包归属 | 已完成并归档 | `archived/todo-2026-09-13-workspace-package-ownership.md`；跨层 ABI 与 elf/tar/通用算法已统一组织进 shared workspace，算法语义未改 |

代码定位：F3c 当前真值位于 `user/libraries/{libfal,libfs,librpc,libbudget,libexecution}`、`rinlib`、`srv_fs` 和 `srv_init`。Runtime/WaitSet、GrantTable、FAL2 wire/client、MemoryBackend、退休唤醒、route/Delegate、Record/Handle/Take、Move、属性 Copy、Watch、在途退出及旧路径删除已共同闭合；既有运输、RPC 和执行前置的实现事实分别由对应 `notes/impls/` 与固定提交 Review 拥有，本计划不再重新安排其施工。

F1 已收口为单 provider 基础闭包。F2 在两个独立进程上补齐真实 hand-off、Namespace/Delegate 与退出组合门：已提交业务因满回复箱停驻后停止 provider，报告 `abandoned=1`；A 的下游 Derive 实际进入静默 Mailbox 后停止 A，报告 `downstream_abandoned=1`，Cancelled 客户端回复同时以 abandoned 收束。FAL1 与全部旧消费者已删除。

当前验证基线：公共操作 P6 最终快照已通过七面 `just clippy`、os/shared host、`just check` 和完整 `just acceptance`；完整 acceptance 包含 stress 16/16、release、sifive_u、virt-nofd、panic/alloc/fatal 三类 boot-failure。FAL 双 provider/退出组合纳入后，`VIRT_NOFD_TIMEOUT` 重校为 45 秒、`SIFIVE_U_TIMEOUT` 重校为 60 秒；两者均在该门内通过，sifive_u 的 reset `NotSupported` 仍按平台契约收割。确定性内核夹具覆盖请求 start/cancel/epoch、两组四类公平性与 Finalization 交棒；用户态覆盖在途 Drain caller 退出与管理者接管。完整 stress 的历史墙钟敏感现场已完成首轮验收收口，证据见 `plans/archived/ref-2026-09-acceptance-timing-flake.md`。

完整日志与诊断 ELF/SHA256 位于本机 `artifacts/check/public-ipc-final-*`、`public-ipc-exit-*` 及 boot-failure/lint 目录，均被 Git 忽略，不随 clone 交付。异机接手先 `just check`、`just clippy`，显式 host target 的算法/shared 单测，再按变更风险运行 `just virt`、`just virt-release`、`just sifive_u`、`just virt-nofd`、`just virt-boot-failure`；若需复现诊断，用档案指明的生产源码断点重新取证，不依赖旧二进制地址。总体 stress/acceptance 在收尾如实判定，不能用重跑直到绿替代验收可靠性计划。

## 开工审视与任务依赖

所有专题统一遵循 `AGENTS.md`「标准施工流程」：接手与基线 → 任务规模审计 → 拆分/合并与依赖图 → 设计闭包 → 按依赖实施 → 分层验证 → 结构收口 Review → 组合收口与归档。本节只记录 FAL 的依赖、边界和领域特有审计，不重复定义通用流程。

本次任务划分失败的根因是把公共前置和正式服务能力混入一项过大的施工任务，并将文档确认误当成前置已成立。FAL 恢复施工时，必须按上述流程从目标契约反推正常、失败、取消、退出和退款路径，审视前置是否齐备、任务是否按机制闭合；自顶向下设计、自底向上完成完整前置。已有草稿不构成保留理由。

```text
已交付公共时间、对象、运输与服务执行基线
  → 共用算法契约与包归属（既有独立计划）
  → 内核等待/请求/退休结构收束（已完成）
  → F0 重新基线审计（已完成）
  → F1 单 provider 授权—后端—协议—执行闭包
  → F2 独立 provider/client、namespace/Delegate 迁移与旧 v1 删除
  → F3a Move → F3b Record/Handle/Take 与属性 Copy → F3c Watch
  → 用户态库知识/依赖重排与 libraries 目录迁移（独立计划）
  → F3d 注册/发现 → F3e Open → F3f 流 Copy
  → F4 独立 test_fal 与整体组合验收
```

消息与流运输、通用执行/准入和 RPC/Outbox 已分别完成实现、真实消费者迁移、失败/退出/退款验证并提交；[公共操作结构收束](archived/todo-2026-09-14-public-operation-ownership.md) 的 P0–P6 也已完成并归档。[库知识归属、依赖、目录与命名重排](archived/todo-2026-09-21-library-knowledge-ownership.md) 已由 `96ee03b` 闭合，新的知识归属、包分工、资源分类及组合接口成为 F3d 当前基线。每个后续闭包仍必须包含真实消费者、失败/退出及旧路径删除。

本文下面保留的基线与代码连接点是审视材料，不是已完成证据。公共前置章节的旧 Seal/Drain 等候选已经被普通 Close/内核退休替代，旧 ABI 和用户维护编排已删除，不继续照旧施工或恢复兼容。

## 1. 基线、交付范围与自然序

审计前调查基线 `5d406a4`：多页 Tunnel/RNL2 已实现，`606b59d` 完成库存来源与正式自检。该基线材料只用于解释旧计划为何需要重审；当前真值以本计划上方 F0 源码审计结论和 `notes/impls/fal.md` 为准。

整体目标是以通用对象寿命、消息交付、持久观察和绝对期限支撑正式服务，再一次接通 FAL。交付包含：

- Mailbox 队列与发送授权分离、Lifetime、Delivery、HandleQuery；
- 持久 WaitSet 及有界收束，复用现有对象信号与通知债务；
- 期限计划拥有的 MonotonicNow、绝对 Send/Wait 与完整 RPC deadline；
- typed 运输 owner、异步 RPC、独立 `libexecution` / `libbudget` 和 Runnel 安全观察；公共机制迁移已由库重排计划闭合；
- 稳定节点、DirectoryGrant、真实跨 provider 路由；
- Record/Handle 属性、注册/发现、正式 Open、Watch、同域 Move 与普通 Copy；
- 独立 provider/client 进程、全部失败路径、旧机制删除及整体组合验证。

不实现 CPU 预约、KernelMemoryBudget 公共 ABI、设备/中断/DMA、BufferQueue、系统关机政策或通用异步语言运行时。FAL 超出基本操作面的能力唯一承接见 [`扩展操作计划`](todo-2026-09-fal-extended-operations.md)。

自然序以「开工审视与任务依赖」为准：已交付前置与 F0–F3c → 库知识/依赖重排及目录迁移 → F3d–F4。后续 BufferQueue、设备/中断/DMA 与异构各依实际能力前置推进；开放不可信创建域前须完成 [KernelMemoryBudget](todo-2026-09-14-kernel-memory-budget.md)，不把当前有界准入当作完整资源隔离。最终全局架构 Review 等本专题主要消费者完成。

## 当前施工位置

公共对象、时间、运输/RPC/服务执行、共享包归属、公共操作 P0–P6、库知识/目录/命名重排及 F0–F2 的机制基线保持。F3a 同域 Move、F3b Record/Handle/Take 与属性 Copy、F3c Watch 已到代码实施、定点审查修复和 core 组合门；当前按新的 `libbudget` / `libexecution` / `libservice` 知识边界恢复 F3d 规模审计。F2 的类型图、责任图、启动能力图与删除结果作为迁移基线；F3 后续每项仍按责任链闭合。

### F3 规模审计与重排裁决

源码审计显示，F3 的五类原始能力并不共享同一完成边界：

- `MemoryBackend::prepare_move`、`validate_move_step`、`commit` 和 `GrantTable::validate_received` 已有正式积木，但没有 wire、Ingress 能力准入、客户端 API 或生产消费者。
- Record/Handle 属性和 affine Take 已由 `value.rs` 写入，属于基本 FAL 能力而非扩展操作；当前 `Ingress` 拒绝带能力请求，`Read` 拒绝带 handle 的值，`Write` 丢弃输入 owner，必须显式承接。
- Watch 只有 `FalRights::WATCH`、Notification 出口政策和资源分类铺路，缺少订阅状态、generation、取消和真实事件源。
- 注册/发现缺少 `ServiceRecord` schema、RegistrationControl、目录后端和发现消费者。
- Open/Attach/Start/EOF/Finish 还缺少协议、offer/StreamControl、StreamTable、provider Tunnel 出资和 Runtime 驱动；它是 F3 中最大的闭包。
- Copy 必须拆成属性复制和流复制：前者依赖 Record/Handle，后者依赖 Open。普通 Create 已经是冲突即失败的独占创建，不另造没有独立语义的 `CreateExclusive` opcode。

F3 不要求每个铺路组件在写入时就有运行时消费者，但要求唯一计划登记其未来消费者、接通条件和清理边界。按此原则，原 F3 重排为以下串行闭包：

```text
F3a 同域 Move
  后端已有事务 + wire/Ingress 能力准入 + libfs 客户端 + 计划内 srv_init 剧本
  → F3b Record/Handle/Take 与属性 Copy
  → F3c Watch
  → 公共记账/执行与领域依赖重排、libraries 目录迁移（独立前置）
  → F3d 服务注册/发现
  → F3e Open/Attach/Start/EOF/Finish
  → F3f 流 Copy
  → F4 独立 test_fal 与整体组合验收
```

| 闭包 | 主要新增责任 | 估计规模 | 依赖 |
|---|---|---:|---|
| F3a Move | 目标 grant/context 验证、同域事务、CrossDevice、能力携带请求、客户端 rename/move、计划内双 provider 剧本 | 中 | F2 |
| F3b Record/Handle/Take + 属性 Copy | Handle/Record 编解码、出入口政策、affine Take 线性化、属性复制与 owner 退款 | 中 | F3a 的 handle 准入 |
| F3c Watch | Subscription 状态、generation、Notification signaler、取消/静默退出、修改事件 | 中高 | F3b 的能力和值语义 |
| F3d 注册/发现 | ServiceRecord、RegistrationControl、目录后端、Ready/Draining 生命周期、发现快照 | 中高 | 库知识/依赖重排完成；F3c 可选；必须登记计划内消费者 |
| F3e Open | offer、StreamControl、Attach/Start、Runnel/Runtime 任务、EOF/Finish、流资源退休 | 高 | F3b；需要正式 Tunnel 出资 |
| F3f 流 Copy | 双端 Open、部分进度、取消/期限、目标退休和不覆盖策略 | 中高 | F3e |

F3a 的实施顺序固定为：先补协议和请求 handle 预算，再接 provider 的授权/事务边界，随后接 `libfs` 客户端和 `srv_init` 计划内调用；只有这条链形成后才进入 F3b。F4 不拥有 F3 的铺路代码删除责任，只负责独立测试消费者和整体组合门。

### F3a 实施记录

F3a 已完成代码接线：`libfal::protocol` 增加严格 `Move` request、`CrossDevice` status 和空回复布局；`libfal::client::Client::move_entry` 以复制的目标 sender capability 发送业务 slot 1；`srv_fs::Ingress` 按 opcode 准入一个业务 handle，并通过 `GrantTable::validate_received` 区分同 provider 与跨 provider；`MoveOperation` 在 `RequestTask` 内按 Runtime budget 分步执行循环检查，成功后调用后端无分配 Commit，失败原样走状态回复并释放预备事务。`srv_init` 已加入每个 provider 的同域 Move 剧本和双 provider 的 CrossDevice 拒绝剧本。`MemoryBackend` 与 protocol host 回归覆盖已补齐。

当前仅记录开发检查：`just check`、`git diff --check` 和 `cd user && cargo test -p libfal --target aarch64-apple-darwin` 通过。F3a 的 Review、QEMU/acceptance 组合门和提交收口按专题整体完成后统一执行，不在此阶段提前宣称最终验收。

### F3b 实施记录

F3b 已把 `value.rs` 的 Record/Handle 铺路接入正式生产链。Create/Write 按值编码声明的业务槽位消费全部 capability owner，`StoredValue::prepare` 统一校验 role、运输 rights、ExportMode 和 Record/Array 总预算；普通 Read 要求 `AcquireCapability`，repeatable Handle 按保存的 ExportPolicy duplicate 并把请求槽位重写为回复槽位。DirectoryGrant 明确拒绝走普通 Duplicate，仍必须经目标 provider 的 Derive 产生收窄授权。

`Op::Take` 由独立 `TakeOperation` 与 backend `PreparedTake` 驱动。准备阶段预付空值存储并设置节点 Busy 门，Read/Write/Enumerate/Lookup 元数据在预留期间拒绝；Outbox 回复成功入箱才无失败地提交空值和版本，关闭/期限/发送失败则从未投递 Packet 逐项取回 capability 并恢复原属性。RPC `PreparedResponse/Outbox` 新增逐项 drain 出口，只转移未投递业务 capability，不分配第二份 owner 集合。计划内剧本覆盖 repeatable 两次读取、affine Take、关闭回复邮箱后的恢复再取、同 provider 与跨 provider 无 capability 属性 Copy；Copy 由客户端 Read→Create 编排，不覆盖已有目标、不承诺跨 provider 原子性，也拒绝携带 capability 的源值。

F3b 阶段在库迁移前的证据为 `libfal` host 26 项、`libfs` host 17 项、原通用运行时 host 27 项、`metadata_admission` host 8 项，以及 target check、七面 `just clippy`、`git diff --check` 和 core QEMU；一次错误使用“只关 sender、不关 mailbox owner”构造不可投递现场导致预期 Busy，失败日志保留在 `artifacts/failed-acceptance-20260920-095111-74662.log`，不作为机制缺陷。迁移后的现行证据由库重排计划拥有：`libfal` 27 项、`libexecution` 22 项、`libbudget` 6 项 host 测试及完整 acceptance 均通过。

定点代码审查提出的五项问题已同次关闭：Read 的 `AcquireCapability` 改取 grant 与节点 rights 交集；Directory Handle 的 Take 在接入正式 Derive 出口前返回 `Unsupported`；Move 最终提交复查目标 Take 预留；成功 Take 把 StoredValue 的 Bytes charge 收缩为空值实际占用；Take 在取出 owner 前预留政策/恢复容器，未投递 Packet 通过无分配逐项 drain 回滚。审查同时指出的 Record 生产往返与多嵌套 Handle 矩阵保留给 F4 独立 `test_fal` 整体验收，不改变 F3b 当前协议与 owner 闭包。

### F3c 开工设计闭包

源码复核确认 Notification signaler 自身可用 `WAIT` 直接观察 owner 的 `CLOSED`，不需要为 Watch 再造 Lifetime；provider 当前 Runtime 的长期任务、来源登记和停止框架足以承载订阅。F3c 因此采用以下最终责任图：

- 公开协议为 `Subscribe`、`QuerySubscription`、`Unsubscribe`。Subscribe 的业务槽 1 是 `NotificationSignaler`，必须具备 `SIGNAL | WAIT | TRANSIT`；回复返回不可猜测的单调 `subscription_id`、观察节点代次和有效 mask。客户端 `Subscription` 独占 Notification owner，并持一份 grant 使用引用，等待时同时观察事件 owner 与 provider grant 的 `CLOSED`。
- 每项订阅是 Runtime 长期任务。任务先登记 signaler 的 `CLOSED` 来源，再把记录安装进 provider 的有界订阅表，安装后才允许提交 Subscribe 回复；因此安装后的修改可早于回复但不会丢失。订阅表按发起 grant 的内核 sender context 验证 Query/Unsubscribe，数字 id 本身不构成取消权。
- 修改事件只从后端成功提交点发布。目录的直接成员创建/删除/移入/移出更新目录版本并发布 `CREATE`、`DELETE`、`RENAME`；属性、流和 Take 成功提交更新目标版本并发布 `MODIFY`；被观察节点删除发布 `DELETE | TERMINATED` 并保留终因查询，后续不再接收事件。位按 OR 合并，不承诺次数、顺序或名称负载。
- 发布表上限固定为 8，并由 `FalResource::Watch` 与 `WaitSource` 双重计费；一次提交至多唤醒每个订阅一次，保持在 Runtime 单 Gate 的 16 项请求上界内。信号 syscall 只由对应 Watch task 执行，普通请求任务不持 signaler owner，也不在同步 handler 中等待。
- Unsubscribe 在同一 provider 状态拥有者中先摘除发布记录并清空尚未提交给 Notification 的 pending 位，再唤醒任务撤销来源；回复确认后不再产生新 signal，客户端 Notification 中已经 pending 的位保持真实。Notification owner 静默关闭走同一摘除和退款路径。provider 停止时先尝试发布 `TERMINATED`，随后撤销来源并关闭 signaler；客户端仍以 provider `CLOSED` 作为不可替代的终态。

首个真实消费者仍放在 `srv_init` 的双 provider 业务剧本：分别证明根目录成员事件、节点 MODIFY、先安装后修改、Query 代次推进、显式取消后静默、Notification owner 静默关闭清理及最终 Watch/WaitSource 退款。跨 provider Watch 不在本闭包内；通过哪个 provider 的 grant 订阅，就只覆盖该 provider 中已授权节点或目录直接成员。

### F3c 实施记录

F3c 已发布严格 `Subscribe`、`QuerySubscription`、`Unsubscribe` wire 与 `libfal::client::Subscription`/`libfs::client::Transport` 入口。`srv_fs` 新增固定 8 项的 provider-local Watch 表和长期 `WatchTask`：订阅准备阶段预付 Watch/WaitSource charge 并接收 signaler，source 登记回调重新按原 AccessSnapshot 解析同一路径，确认稳定 NodeId 与当前 WATCH 权限后在同一推进点安装记录、取得节点版本并开放回复。安装后 Create/Link、Property/Write、WriteAt、Move、Delete 与成功 Take 的 commit 点发布有界事件；Move/Delete 同时推进目标节点版本，节点 Delete 保留 `NodeDeleted` 终因。取消先摘表再确认，Notification owner 消散、Subscribe reply abandoned 和 provider stop 均由同一任务撤源、关 signaler 并退款。

独立代码审查提出的三项问题已关闭：安装回调重新取代次并拒绝已删除/换代路径，消除准备—安装窗口；Subscribe Outbox abandoned 主动撤销已安装订阅，不把未知 id 的清理押给客户端；`SubscriptionInfo` 解码拒绝 `pending` 超出 effective mask 及 Active 携带 TERMINATED。`srv_init` 在两个独立 provider 上覆盖根目录 CREATE、节点 MODIFY、Query 代次、外来 grant context 拒绝、显式取消后静默、owner 静默关闭、节点删除 `DELETE | TERMINATED`、provider stop `TERMINATED` 和最终 Watch/WaitSource 退款。

当前 F3c 开发与 core 组合证据：`libfal` host 27 项、`libfs` host 17 项、RISC-V 全用户程序 ELF 构建、七面 `just clippy`、`git diff --check`、`THROTTLE=100 just virt` 与 `THROTTLE=100 just virt-release` 通过。首轮 core 曾因验收错误地把空 Notification `take` 当作返回零而短路，已改为零期限 wait 验证静默；该日志仅是测试假设错误，不构成机制缺陷。完整 stress、平台和 boot-failure 仍按 F3/F4 整体收尾统一执行。

### 跨会话接力断点

接手分支为 `task/fal-service-capabilities`，代码与方向/计划基线为 `dfcf7a349fe6d9e2836bb7c8179ff7e96c8ce20a`。F1–F3c 的全部新增、修改和删除共同构成该连续迁移快照，提交后审查登记与导航更新另以文档提交保存。接手时先运行 `git status --short --branch` 与 `git diff --check`，保留该基线之后的全部工作树变化，不回退到 `84eeed6`、`d22b9d7` 或只挑 Watch 文件继续。

F3c 已完成的最近证据是：`just check`、RISC-V 全用户程序构建、`libfal` host 27 项、`libfs` host 17 项、七面 `just clippy`、`THROTTLE=100 just virt`、`THROTTLE=100 just virt-release` 与 `git diff --check`。未执行的是 F3/F4 收尾才要求的完整 stress、`sifive_u`、`virt-nofd`、boot-failure 和 `just acceptance`；下一会话不得把这些未跑门写成 F3c 缺陷，也不得把 core/release 通过冒充整体 FAL 验收。

用户态库知识归属、目录迁移与组件命名已经由 `96ee03b` 完成并归档；未来服务领域库的正式名称为 `libservice`，但 F3d 不预建空 crate。当前从规模审计恢复。第 8 节仍是方向草稿：读取 `notes/ideas/service.md`、`user/README.md`、`user/libraries/README.md`、`notes/impls/fal.md` 和当前 `srv_init`/`srv_fs` 启动与 route 装配，盘点首个真实发布者、发现消费者、ServiceRecord capability 槽、RegistrationControl owner、Ready/Draining/撤销状态、旧实例竞态、Watch 组合、预算与退出退款；确认这些责任链后再更新本计划的 F3d 设计闭包并实施。不得预设 `libservice` 已有注册表、不得把 route-management endpoint 直接改名充当注册权威，也不得提前建立无真实消费者的全局 registry。

### F0 重新基线审计结论（已完成）

#### 1. F0 历史生产调用图（F2 已删除）

- F0 当时唯一真实 FAL 生产消费者是 `user/services/srv_fs`：`srv_init` 以空 grants 启动它；`srv_fs::Fs` 同时拥有 provider sender、同步 `Caller` 和 worker，在同一进程中把自有 MailboxSender 当作 `PrefixTable` 根 anchor。
- 客户端路径为 `libfs::resolve` → `Fs::call` → `librpc::Caller` → Mailbox/Delivery → `srv_fs::Ingress`/`RequestTask` → `Outbox` → `libfal::provider::serve` → `MemFs`，真实经过内核运输，但业务状态不跨进程、不经过 FAL authority。
- v1 provider 只校验 slot 1 有一个 handle，随后完全不解释 anchor；`MemFs` 从自身 root 行走，权限来自可由请求指定的节点属性，`property_write` 忽略授权上下文。`Move/Copy/Open` 仍返回 `Unsupported`，客户端没有 Delete/长生命周期业务消费者。
- `libfs` 的 `PrefixTable` 持裸 `Handle`，`Delegate` 只有 host mock 产生；`srv_init` 没有 namespace/FAL 装配，启动 grant 没有交付 FAL endpoint。

#### 2. v2 类型图与实际接线

- `NodeStore`/`MemoryBackend`/`Data`/`StoredValue`/`PreparedMutation` 构成自洽的 v2 后端积木：NodeRef 的 pin 与目录 link 分账，准备事务预付 slot/Charge，commit 无分配，冲突原样返还 mutation，旧属性/流块/目录项由结果或退休路径承担，最后引用通过 `Wake` 发布退休工作。
- `AccessSnapshot` 的唯一构造点是 `GrantTable::snapshot`；`MemoryBackend` 的 lookup、create/delete/write/property/move 全部要求该 snapshot。由此授权不是 F1 之后可随意接入的旁路，而是后端类型闭包的一部分。
- `GrantTable` 已由 `srv_fs` provider 与 Runtime 共同拥有：Runtime 登记 Lifetime CLOSED source，source 成功后才 install 并发布根 sender；Ingress 按内核填入的 sender context 获取 `AccessSnapshot`。
- F0 时 `protocol.rs` 已提供基础 FAL2 request/response codec，旧 `srv_fs::Fs::call_v2`、Ingress、RequestTask 和 Outbox 构成首个生产 client/provider 链；F2 已将 client 迁至 init 并删除 `Fs`/worker。

#### 3. owner/authority 与失败责任

- 未提交阶段：PreparedNode、PreparedMutation、PreparedWrite、StoredValue、GrantState 持有全部节点 pin、目录/块 slot、Account Charge、能力 owner；准备失败或冲突必须原样返还事务/输入 owner，不能只返回错误码。
- 已提交阶段：目录替换、属性替换、流块替换和节点摘链分别产生旧值/退休责任；回复失败只能形成 `OutboxResult::Abandoned`，不能伪造业务回滚。后端退休由显式任务推进，`Wake` 只负责唤醒，不能依赖下一次业务请求。
- 服务退出现已由 `srv_fs::server::run` 按停止准入 → 根 grant source → backend retire → retire source → Runtime 的顺序驱动；`NotificationWake` 为真实 backend 构造点。剩余 F1 收口责任是扩大失败/取消/期限/退款覆盖，而不是恢复旧 WaitSet owner。
- F1 已有 `libfal` host 45 项测试、`just check`/`just clippy` 以及 `THROTTLE=100 just virt` 的真实 provider/client 证据；FAL1 兼容路径和最后消费者迁移仍明确属于 F2，不能据此宣称整体 FAL 完成。

#### 4. 依赖裁决与新自然序

F0 裁决不再把“后端”与“授权/执行/协议”拆成可独立验收的文件任务：

```text
F0 重新基线审计（已完成）
  → F1 单 provider 闭包：服务状态拥有者 + Runtime/WaitSet + GrantTable + FAL2 response/client/provider 接缝 + MemoryBackend/NodeStore/Data/Value
  → F2 两个独立 provider/client：namespace/Delegate、真实启动能力图、跨 provider 路由与旧 v1 删除
  → F3 长生命周期业务：Open/Attach/Start/EOF/Finish、Watch、服务注册/发现、同域 Move、普通 Copy
  → F4 独立 test_fal 与组合验收
```

F1 的唯一真实消费者是一个正式 provider 运行体及其最小 client；不得以 `srv_fs` v1 `MemFs`、同进程 self-pump、slot-1 anchor 或仅 host mock 充当 F1 消费者。F1 必须同时证明：授权快照可取得且不能伪造、后端五类 mutation 的准备/取消/冲突/提交/旧值退休、服务任务的公平推进与显式 wake、期限/调用者退出/服务退出、Delivery/Outbox 交付以及 Account/metadata 退款。

#### 5. F1 开工设计裁决

F1 保持一个语义闭包，内部施工顺序不形成独立交付。编码前的最终类型与范围边界如下：

- `Runtime` 是 provider `WaitSet` 的唯一登记、接收和关闭 owner。`GrantTable` 不再借用 `WaitSet`、保存原始 token 或直接消费 `ReadyRecord`；每个 grant 的 Lifetime 观察由正式 grant task 通过 `Requests::{add_source,remove}` 登记，Runtime 回执的 `SourceId` 和未注销 owner 留在该任务状态中。登记成功前 sender 不得发布；CLOSED、停止和服务退出均由同一任务先撤销来源，再移除授权状态、关闭 observer 并退款。
- backend retire 使用预先创建的 Notification。owner 作为 Runtime retire task 的来源，signaler 由 `NotificationWake` 独占并注入 `NodeStore`；任务每次显式 `take` 电平后按预算调用 `retire_step`，仍有工作时保持 Runnable，无工作才 rearm。公开首个 grant 前必须完成 retire source 与根 grant Lifetime source 的登记；最后先停止授权准入并排空 backend，再关闭 Notification owner 和 Runtime。
- provider 以同一个 `libbudget::Account` 建立 `AccountView<ExecutionResource>` 与 `AccountView<FalResource>`：前者只支付 Runtime 的 Task/InputBytes，后者只支付 Node/Bytes/Grant/Watch/WaitSource 等 FAL 领域资源。`Request`、`Outbox` 不与机械执行槽重复记账；实际未消费的领域槽在本轮不预留假额度。
- F1 包含 provider-local 根 DirectoryGrant 的正式 sender 身份、Lifetime 观察、`AccessSnapshot` 和最小 client，但不包含进程 namespace、Delegate 或跨 provider 路由。F2 将该已成立的 grant 契约迁入独立 provider/client 启动能力图并接通 namespace/Delegate。
- F2 已接通严格 wire、sub-grant、独立 hand-off、两个真实 provider 和 provider route/Delegate：init 为两个 `srv_fs` 分别装配 bootstrap/release/route 与监督 control，取得不同 sender object identity 的 root grant；A 经独立管理 endpoint 持有 B 的母 grant与权限 ceiling，Lookup 命中后由 Runtime 内 `Dispatcher` 非阻塞调用 B 的 Derive，再向正式 `libfs::client::Transport` 返回独立衰减 grant、consumed 和 remaining。Namespace 只挂 A，`/second` 与 `/second/f2-dir/leaf` 已真实跨 provider 行走；两类在途退出与 ProviderReport 已闭合。
- F1 的生产协议只发布首个真实消费者所需的基础操作；未实现的 F2/F3 opcode 不进入已发布枚举。`MemoryBackend` 的 Move 事务在 F1 作为后端状态机与 owner 回归覆盖，公开 Move 请求、跨 grant 目标验证和客户端语义仍由 F3 共同交付。
- F2 迁移现有 `srv_fs` 真实消费者和启动装配后，最后一个 v1 使用点消失即删除 FAL1 临时 anchor、无鉴权 `MemFs`、裸 `PrefixTable` Handle 和同进程旧泵，不将双轨保留到 F4。F4 只建立独立 `test_fal`、执行跨机制组合门并归档。

#### 6. F2 完成断点

本轮开发门：删除 FAL1 后 `libfal` host 22 项、`libfs` host 17 项、`just check`、七面 `just clippy`、`git diff --check` 和 `THROTTLE=100 just virt` 通过。QEMU 已证明 pid 4/5 两套独立 Runtime/GrantTable/MemoryBackend、不同 root sender identity、独立 route-management endpoint、A→B 下游 Derive、Delegate 空/非空 remaining、权限 ceiling、全部 10 个 FAL2 操作、sub-grant 退休、提交后 Outbox abandoned、已投递下游调用退出以及两套 provider 的最终退款和监督回收。

F2 无未完成责任。FAL1、`MemFs`、slot-1 anchor 和同进程旧泵已删除；在途窗口完全由正式 mailbox 背压、静默下游与进程 release 生命周期形成，没有增加延迟 opcode。固定业务剧本属于 init 的验收政策；双 provider hand-off、route 管理端、Delegate、release source 与 ProviderReport 属于正式装配接缝。下一自然位置是 F3。

F0 完成标准已满足：当前 v1/v2 调用图、authority/owner 图、状态与责任边界均有源码落点；旧计划中“先单独实现后端、再接授权”的错误前提已删除；未知项均转为 F1 的明确接缝或后续唯一阻塞项。完成 F0 不代表 F1 已开工，也不代表 FAL 业务已交付。

下面条目是 F0 审计前混合工作树的定位材料；与上面结论冲突时以上面的当前源码审计为准。后续 F1 只取实际代码事实，不把这些历史盘点或局部源码存在当作完成证据：

- `shared/erhino_shared/src/time.rs` 已写入 Deadline、ClockSnapshot、ClockGeometry 和换算测试；`kernel/clock.rs` 接入单一时钟来源，调度量子与启动期限不再截断频率。
- shared/kernel/rinlib 的绝对 WaitMany/Sleep/Send 已开始纵向迁移；同步 Caller 与异步 Dispatcher 已写入同一 deadline、Unsent/Sent 错误阶段和未发送 Request/能力归还，尚未形成编译与组合证据。
- entry badge 已迁至独立 MailboxSender；Lifetime、Delivery、HandleQuery、ReceiveResult 和对应 syscall/封装已写入。Capability/HandleSet、ReceivedMessage、ReceiveBuffer/MessageStorage、Packet 和 RequestContext 已承担运输 owner；原始移动 Send 已改为显式 unsafe，既有验收消费端开始清除重复关闭，正式 srv_fs 旧泵仍待整体替换。
- ObserverSink 已将 WaitContext 与 WaitSet arm 接入同一来源/完成债务。WaitSet 的注册、ready、rearm、remove、seal/drain 和 ProcessDrain 接管代码已写入，组合竞态与预算证据尚未补齐。
- 通知入队不再同步访问 registry，用户 trap 尾部开始有界推进；剩余 pending 的完整安全点/门铃验证属于未完成责任。
- Tunnel PEER_ATTACHED 与 Runnel 的安全 WaitSet 注册/等待准备已开始接线；AttachFailure 的真实 typed owner、清理及两类角色的统一驱动尚未完成。
- `libbudget` 的 Budget/Account/AccountView/Charge 共用 `metadata_admission`，`libexecution` 的任务与输入缓冲先准入；WorkQueue 持有稳定任务记录、公平 ready FIFO 和预付期限槽，Runtime 轮转期限/输入/任务并保留 max_work=1 的轮转位置。Seal 后分步停止任务、真实退休后清空；外部 WaitSet 最后收束。正式任务与 FAL 状态机已由 `srv_fs` 真实接入。

施工中识别的预付缺口已进入最终结构：WaitSet 注册持久保留来源订阅与 finish debt，消费后在来源锁内重置静止 arm，并用不回绕代次隔离旧记录；正常 Rearm 不分配。任务期限也不能在业务提交后重新申请完成槽，TimerQueue 已增加保留 token/generation 的 park/reschedule，WorkQueue 每任务预付一个期限槽，停用不占活动堆、恢复不分配；时间机制的具体实施仍由期限计划拥有。这些代码都待整体竞态、预算及组合复核，不使用轮询/额外重试 adapter 掩盖缺口。

- `libfal/store.rs` 已写入 NodeId/NodeRef、PreparedNode、NodeStore 和 RetireContext。目录链接与 pin 分账；准备期独占 store 容量与账户额度；未入表节点取消不排入退休链；最后链接/pin 消散只排队，具体 payload 逐步退休。F1 `srv_fs` provider 已实际消费该结构；正式跨 provider 目录与能力派生仍属 F2。
- `libfal/authority.rs` 的 FalRights 与 `GrantTable::snapshot` 已形成不可伪造的请求授权；`GrantTable::prepare_derive` 已继承父账户/运输 ceiling、拒绝 rights 放大并产生独立 sender/Lifetime。`srv_fs` grant task 同时协调 Lifetime 与回复 Outbox source，回复阶段完成后继续存活到 grant 退休；第二 provider 已通过 A 的异步下游 Derive 生产真实 Delegate。
- OrderedTable 已支持有序名字键、借用查询/游标/删除及准备后确定 key，整数键继续走同一个 AVL 实现。名字事务不得退回不能 fallible reserve 的 BTreeMap 路径。

- `backend.rs` 已写入目录/属性/流/链接的非递归 payload、准备/最终校验/无分配 Commit，以及 Create/Delete/属性替换/定位写/Move；F1 `srv_fs` 已调用基础接口，并新增 Enumerate/Link 的最小处理。跨 provider 路由已由 F2 接通；公开 Move 的跨 grant 目标验证仍属 F3。
- `value.rs` 已写入正式长度化属性编码、Array/Record、非递归总预算验证、唯一完整槽引用、实际 capability 描述与出口政策验证、canonical slot 重写和存储 owner。Directory 出口包含 FAL ceiling，必须由正式导出任务向目标 provider Derive，不能直接 duplicate 母本。旧 property/memfs/provider v1 路径仍待整体删除。
- `data.rs` 已写入稀疏分块数据、预付写块与无分配替换、逐块退休。正常定位写范围受 payload 上限约束，Open 数据任务以有界块推进。
- `protocol.rs` 已写入 v2 基础操作、`Enumerate`/`Link`/`Derive`、严格 Lookup 三态、稳定 cursor/entry 编码、能力槽数量和完整 Header Deadline；`srv_fs` 已真实消费 sub-grant、LinkBoundary、枚举、独立进程 hand-off 与跨 provider Delegate walking。
- NodeStore 的退休发布以 `libexecution::Wake` 和预先配置的 Notification 唤醒；正式域必须先注册该来源再公开 grant，最后才关闭 Notification owner，禁止等待下一次业务请求偶然退休。
- `rinlib/ipc/invitation.rs` 与 EndpointCleanup 已写入邀请未消费/已消费失败 owner；原始 Tunnel/Runnel Attach 已改为 unsafe，五处既有调用点已标明原始责任。Runnel Channel/ProducerCore/ConsumerCore 构造失败以 InitFailure 原样返还 Transport，安全 Producer/Consumer create/attach 接入 typed Invitation，协议初始化失败返还 Endpoint，不自动丢弃承载。pm 接收侧已迁入 typed Invitation/Producer::attach，init 创建侧与正式 Open 仍待迁入安全工厂；host 用例只已写入，未运行。
- 代码 reviewer 的静态追踪发现 OrderedTable 内部 scan cursor 仍为 u64，已统一为 K；未运行构建或测试。其他 IPC 并发观察不构成运行安全性证据。

下一连接点以本计划顶部的新依赖图为准；以上旧施工盘点只用于定位草稿，不据其中过时的完成或 Seal/Drain 描述续写。执行基座成立后，先把后端的准备失败、取消与替换旧值清理收回完整后端 owner，再接授权/provider/client；不把后端退休逐项交给 handler，也不把 FAL 资源分类继续加进执行核心。

实现细节收口：WaitSet 的 ready 双向链接存在正式 OrderedTable 注册记录中；入队/摘除使用固定数量的有界 AVL 查找，避免新增 unsafe intrusive 指针或无界 tombstone 扫描。收束政策上限统一从 shared `DRAIN_WORK_MAX` 取得，不能用用户传入的巨大预算把短内核路径放大成全表操作。

## 总体推进视图

这不是一次“给文件库加几个操作”的改动，而是一次正式用户态服务栈交付。工作量来自三条同时需要闭合的链：授权链（sender/Lifetime → grant → 稳定节点 → 出口衰减）、执行链（WaitSet → 公平任务 → 下游 RPC/背压 → 业务完成）、责任链（Packet/Delivery → 请求/outbox/流 → terminal → retire → 精确退款）。缺少任一条，演示路径可能工作，但正式服务契约不成立。

总体目标保留，设计与任务边界按证据持续修正。下面五段是导航；前两段由独立前置计划安排并包含其闭合完成门，后三段在前置完成后恢复。A–G 旧落点只作为代码盘点参考，不构成继续按文件分片的任务划分。

| 施工段 | 直接形成的最终结构 | 后续连接与段末检查对象 |
|---|---|---|
| 1 公共资源基座 | 内核身份/Delivery/WaitSet/时间，rinlib 运输与映射 owner，Runnel 构造失败 owner | 正式服务能无分配地恢复观察、按阶段归还资源；全部 raw 消费入口标明责任，不能让安全 API 消费另一 owner 的借用值。 |
| 2 服务任务运行体 | `libexecution` 任务、`libbudget` 账户、显式债务唤醒、RPC dispatcher、Outbox 与有界 retire | 一个运行体实际驱动请求、下游调用和退役；所有完成/取消路径保留 Delivery，源码存在不等于已接通。 |
| 3 授权目录与值 | NodeStore/PreparedMutation、GrantTable、Namespace/走路、Record/Handle/Take、跨 provider Derive | 替换旧路径模型和无鉴权 anchor；稳定位置、真实权限衰减与运输槽布局同步迁移。 |
| 4 长生命周期业务 | Open offer/Attach/Start/EOF/Finish、Watch、注册/发现、同域 Move 与客户端 Copy | 全部 terminal/retire、部分进度、取消、静默退出与旧实例竞态闭合；不在 handler 中等待。 |
| 5 整体组合收口 | 两个 provider、独立 test_fal 与 init 能力图的组合验证 | 各机制的真实装配、消费者迁移与旧路径删除须在各自闭包完成；本段集中执行第 14 节跨机制组合门并核对完整交付。 |

以上是总体能力导航；当前实施顺序由 F0 代码审计后的依赖图及各唯一 todo 拥有。F1 必须把 Runtime/WaitSet、provider-local 根 grant、FAL2 codec/provider/client 和后端 owner 作为一个单 provider 纵向闭包共同迁移；F2 再接独立 provider/client、namespace/Delegate 和真实启动能力图，并随最后一个真实消费者迁移删除旧 v1；F3 才进入 Open、Watch、注册/发现、Move/Copy 等长生命周期业务。store/backend/value/grant 仍是源码位置，不直接作为交付任务。每轮登记真实责任链、前置证据、失败/退出/退款路径和旧 v1 删除门，发现新前置先修订本计划。

## 2. 公共对象类型与所有权参考

```text
HandleTable Entry = object + role + rights

MailboxOwner ──> Mailbox(queue, receive reservation, signals)
Sender / SenderOnce ──> MailboxSender(identity, badge, queue, LifetimeOwner)
                               |
                               └── strong queue reference
LifetimeObserver ──> LifetimeState(observed identity, CLOSED, subscriptions)
                         ^
                         └── LifetimeOwner（仅内核，observer 不反向持 Sender）

queued Message ──> Delivery ──> MailboxSender
Receive ──原子移交──> Delivery Handle ──> 用户 RequestContext

WaitSetOwner ──> WaitSet ──> Registrations ──> validated source references
                               └── 一个预付 ready slot / arm completion
```

### 2.1 Mailbox 与 Lifetime

- `Mailbox` 只拥有队列/接收/容量电平，receiver-owner 唯一且不可 TRANSIT。
- `MailboxSender` 是独立 KernelObject，拥有不可变 badge、自己的 koid、目标 Mailbox 强引用和一个 affine `LifetimeOwner`。SenderOnce 与普通 sender 引用同一对象，只变 role。
- `LifetimeState` 是可等待 KernelObject，只拥有观察状态、被观察对象 koid 和订阅，不持 Sender/业务对象强引用。公开 observer 可 WAIT、DUPLICATE、TRANSIT、GRANT，不提供提前终止权。
- `LifetimeOwner` 是通用内核内部基元，最终释放只发布一次 CLOSED。不暴露用户态“寿命 owner”，不引入外部续租或原进程保活依赖。
- 使用对象强引用本身保持 sender、运输、临时 syscall 使用与 Delivery 的连续所有权；最终析构发布 Lifetime。不读 `Arc::strong_count`，不在 HandleTable 外另算一份 sender 数。
- `Entry` 删除 badge 字段及 `entry_with_badge`；badge 的唯一真值迁到 MailboxSender。所有非 Mailbox 对象不增加无意义的授权计数或标签。
- MailboxCreate 只交付 owner；MailboxMintSender 原子交付 `{sender, lifetime_observer}`。通用便利工厂可以组合这两个调用，失败关闭尚未发布的 owner，不保留内核默认 badge-0 sender 路径。原先只请求 READ|WAIT 的 ReplyPort 创建者需显式取得 MANAGE 完成 Mint；只接收不铸造的服务可以由 launcher 预先 Mint，再通过 GRANT 收窄其 owner。
- 重新 Mint 是新对象，即使 badge 相同也有新 koid；Duplicate/MakeSendOnce 不新建寿命实例。服务登记以发送授权 koid 为键，badge 作为不可变标签。
- 队列 CLOSED 与发送授权 Lifetime CLOSED 不等同。sender 的可写/队列关闭等待绑定实际 Mailbox 电平源；本次等待另保留原 sender 使用引用。按真实电平源合并订阅，不复制 WRITABLE 状态。
- provider 政策撤销在用户态拒绝新的授权准入；已准入 RequestContext、独立派生 grant 和已建立流按各自契约收束。普通 close 不实现递归 revoke。

### 2.2 Delivery 与 Receive

- 每条成功 Send 有一个独立 `Delivery` KernelObject，强持被调用 MailboxSender。Delivery role affine，可 TRANSIT/GRANT，不可 DUPLICATE、不可 Send、不可 WAIT；关闭仅释放一个交付责任。
- Message 在排队和接收预留中拥有 Delivery；Receive 原子将其安装为接收方 Handle。业务 Handle 上限仍为 8，Delivery 额外占一项真实表槽，不占业务槽编号。
- 接收 ABI 使用结构化请求/结果：`ReceiveResult` 包含 MessageHeader 与本地 Delivery Handle；Peek 只返回 MessageHeader，不能伪造一个尚未安装的 Delivery Handle。业务 `handle_count` 不包含 Delivery。
- MessageHeader 增加 `sender_context_id`，等于被调用 MailboxSender 的 koid；保留内核填入的 sender_pid、sender_badge。发送方没有填写这三项的入口。
- Send 在发布前完成 Delivery/队列/运输存储预留；成功入队与 moves/send-once 消费原子化。投递期限的检查位置由期限计划定义。
- Receive 预留 `业务 handles + 1`，完整写回后一次提交；输出失败恢复同一个 Message/Delivery，owner 同时关闭则统一清理，不复制责任。
- `OwnedMessage`/`RequestContext` 保留 Delivery，直到处理和回复/拒绝责任终结。即使请求派生异步任务或 outbox，移动整个上下文，不留下裸 sender_context_id 与已释放的寿命。
- 消息中的 transit 集合统一由 `TransitEntries` 叶收束 owner 管理，覆盖 Discard、owner close、回滚与异常路径。不能只 drop `Vec<Entry>` 而漏掉 Invitation 等 close callback。
- 不把所有 Entry 的 Drop 泛化成 close：容器 owner、ProcessBuilder、映射 owner 仍走其既有显式协议。删除无生产调用且会静默丢 Pinned entry 的 `HandleTable::drain/into_entries` 辅助路径，测试改走正式事务/有界摘除。

### 2.3 HandleQuery

新增只读查询，输入必须是调用者真实持有的 Handle；不要求新管理权，不允许以对象 ID 打开对象。固定宽结果包含 `object_id, related_object_id, kind, role, rights, badge` 及零 reserved。不暴露内核地址，不在此绕过 WAIT 提供任意动态电平读取。

- MailboxSender 的 object_id 是发送授权身份，related_object_id 是目标 Mailbox；
- Lifetime 的 related_object_id 是被观察对象；
- Delivery 的 related_object_id 是被调用发送授权；
- 不适用的关联身份/标签置零。

复用 `ObjectHeader` 和 `monotonic_id` 的单一 koid 来源，不建立第二个全局对象注册表。公开 kind/role 使用固定 wire 判别，不直接拷贝 Rust enum 内存布局。查询持有期间防止 entry 被并发移走；输出失败无副作用。

## 3. WaitSet：公共持久观察

### 3.1 类型与接口

`WaitSet` 为唯一 owner 的可增长容器。公开接口：Create、Register、Rearm、Receive、Remove、Seal、Drain；最终通过 HandleClose 关闭空集合。owner rights 为 READ/WAIT/MANAGE/GRANT 的相应子集，不可 DUPLICATE/TRANSIT。

`Registration` 包含：不复用 token、cookie、interest、原已验证使用引用、真实信号源、源订阅 token、arm 代次、仲裁状态和一个预付 ready slot。状态为 Installing → Armed → Queued → Disarmed；Remove 进入 Removing → Dead；Rearm 产生新 arm 代次。

- 每轮最多一条 ready 记录；第一次获选的完整 observed 快照冻结，后续变化不拼成另一快照。
- Receive 事务交付固定上界批次，失败不消费；成功后该轮 Disarmed。
- 每个 registration 持久占有来源订阅和预付完成槽，Armed/Queued/Disarmed 不撤销再安装；正常 Rearm 不分配、不重新申请通知/完成债务。只在 Remove/Seal/退役时注销来源。
- Rearm 在源锁内重置已经结束的 arm、更新 source epoch 基线并观察当前电平，已为真时立即进入完成路径。ready 记录带 arm_generation，返回新代次用于过滤旧记录；溢出显式拒绝，不能回绕。
- Remove 使 token 不再进入可接收队列，清理已排队记录及在途完成责任；用户已经取得的旧记录须按 token 有效性过滤。
- ready 队列以 registration 内联链接实现唯一入队；摘除只做固定数量的有界注册表查找，不留大量 tombstone 让一次 Receive 无界扫描。
- 每个 Register 只处理一个来源；规模由 metadata/队列预算准入，不能把 WaitMany 的 64 项变成整个服务的连接上限。

### 3.2 与既有等待机制合并

把来源订阅的完成目标抽象为 `ObserverSink`：一次性 WaitContext 或 WaitSet registration 的一个 arm 周期。两者复用 `ObjectWaitState` 的电平、代次、发布快照、预付通知槽和 `notify_work` 排水。

源锁内只进行有界快照与原子 offer，不锁住另一个 WaitSet、不执行用户回调。完成引用在源锁外转交已预付的 finish 路径；周期完成后先归还该 registration 的 finish 槽，再发布 ready，使立即消费/rearm 的另一 hart 也能复用该责任。持久完成不注销来源订阅，来源锁内的 Lost/Complete 分支均保留持久订阅，普通线程等待仍按原语义移除。安装、提前命中、取消、Rearm、输出失败竞争同一轮状态，迟到完成不能命中新 arm。

通知发布统一拆成两个阶段：锁内/析构路径只把预付 debt 放入当前 hart 队列并设置 pending，不执行 IPI 或获取 registry；用户 trap 统一尾部、调度安全点及入 idle 前，在全部业务锁释放后有界推进通知，并为剩余 pending 发布门铃。现有 `notify_work::publish` 会经 `try_ipi_slots` 获取 rank 150 的 REGISTRY 锁，不能从高秩锁内的 Lifetime 析构直接调用。所有通知消费者迁到同一发布/安全点协议，不新增 Lifetime 专用队列，也不靠未来偶然 timer 唤醒。

WaitMany 保留单次 64 项的输入上限及最小 item_index 规则；它是有界的一次等待，与逐项增长的 WaitSet 共用来源机制。禁止新增一套独立电平实现、用户态轮流等待子集、每连接阻塞线程或计时轮询适配器。

### 3.3 收束与预算

WaitSet 为增长型容器，状态为 Active → Sealed → Draining → Done → Closed。Seal 停止 Register/Rearm 和新 ready 交付，发布 REAPABLE；全部退役完成发布 DONE；最终 owner close 才发布 CLOSED。

Drain 每个 work unit 推进一个真实注册/完成责任/存储块的收束，游标属于集合；并发 Drain 使用单一 gate 仲裁。未完成的源注销与 finish 责任仍在注册账本中。存储分块释放，最终 Drop 不再遍历全部空槽。

非空 HandleClose 返回 ObjectBusy 并保留 entry，不隐含先 seal 的部分副作用。rinlib 显式 close 使用 Seal/Drain/Close；预算耗尽或期限到达返还完整 owner 与阶段。异常 Drop 单次尝试并记录诊断，交 ProcessDrain 接管。

ProcessDrain 的 pending close 扩展为“原 entry 或该对象的 RetireState”，复用同一 WaitSet drain 内核；外层 max_work 约束真实内层工作，不能用一个外层 unit 隐藏全表注销。运行中和进程退出不建立两份清理算法，不增加内核线程。

## 4. 时间、期限与信号扩展连接点

时间具体契约、换算、回绕边界及施工由期限计划唯一拥有。这里冻结连接点：

- MonotonicNow 返回同一系统时间域的 u64 纳秒；Deadline 为显式 Infinite/At，不复用零表示无限；
- WaitMany 直接接受绝对期限；相对便利入口只在用户库转换一次；
- Send 在全部预留之后、入队线性化点核验同一绝对期限；过期不消费能力；
- RPC 的最终回复接受再次检查 Deadline；同步与异步共享投递阶段语义；
- `libexecution` 以用户态 deadline heap 和 WaitSet READABLE 的一次绝对等待组合定时，不增加服务专用内核 timer；
- Tunnel Endpoint 增加 PEER_ATTACHED：当且仅当对端状态为 Alive 时成立；Invited 不成立，peer close 清除并发布 PEER_CLOSED。这是 Connection 事实，不是 FAL Start 或 Runnel 验证结论。

信号位和 epoch 数量由一个规范列表派生，不能在 `SIGNAL_BITS`、KNOWN 和各数组长度中留下互不关联常数。

## 5. 用户态类型与执行框架

### 5.1 rinlib / librpc

- 叶能力、Invitation、Delivery、Lifetime observer 均有不可伪造 owner；消息结果先拥有全部资源，再通过 take 提取。构造 owner 只来自正式 syscall/Receive/StartupBlock 授权入口。
- 通用出站 packet 拥有 move 集合：成功投递消费，失败返还。不能在错误枚举中只返回错误码而让调用者猜 Handle 是否还在本地。
- AttachFailure 区分未消费 Invitation 与已消费后持有 Endpoint 的协议初始化失败；不得对可能已消费的 raw Handle 重复 close。
- Runnel Producer/Consumer 继续独占 Endpoint，提供非阻塞推进、等待准备、WaitSet registration 和 peer 状态查询；不导出 raw Handle、共享 slice 或可复制数据角色。
- Runnel 的阻塞门面与事件循环共用同一推进/acknowledge/重查逻辑。原错误后不可逆终态、已完成字节与清理 owner 规则保留。
- RPC PendingCall：Unsent(packet) → Sent(reply routing) → Completed；过期/失败按阶段返回 owner 或 OutcomeUnknown。send-once 请求 slot 0 使用 WRITE|WAIT|TRANSIT。
- 同步 Caller 私有端口，失败可整端废弃；多 in-flight dispatcher 共享端口，单个超时只移除对应 txid，迟到回复连资源一起丢弃。二者共享 framing、deadline 和运输状态，不共享错误的端口失效范围。
- 期待回复但 framing 违约时，只有在 txid 和 reply role 已可验证的情况下发送协议拒绝；否则释放输入，由调用期限结束对端等待。

### 5.2 服务运行体装配

一个服务状态拥有者组合 `libexecution::Runtime` 与领域表：FAL provider 拥有 GrantTable、NodeStore、SubscriptionTable 和 StreamTable；未来 `libservice` 拥有 RegistrationTable 与服务状态机，而不接管 FAL 内部真值。控制循环由 `libexecution` 收集有界 ready 批次、按稳定任务 token 分发、每任务有限工作、未完成任务排队尾，并在无立即工作时以最近绝对期限等待。

请求、下游 PendingCall、outbox、流和用户态 retire 都是正式任务状态，不在 handler 中同步等待。服务端不调用无限 send_blocking、write_all，也不持后端状态锁等待客户端/下游。未来设备 I/O 通过异步完成接回同一调度结构。

资源账户由启动/授权政策通过 `libbudget` 分配并随派生继承来源，不以 PID 生成 authority。预算覆盖节点及数据、grant、等待注册、请求、outbox、service record、Watch、Open offer 与活动流。达到预算在发布前拒绝；错误/退出精确退款。

第一版 Tunnel backing 由 provider 的 PoolBinding 支付，按授权账户预留连接配额；不假装已经支持逐连接使用客户端 MemoryPool。账户出资身份与当前持有进程/Job 正交。

独立授权域可使用独立 Mailbox，域内共享 badged sender。所有入口汇入 WaitSet 和公平任务循环；不能宣称一个共享 16 项 FIFO 已提供对恶意 sender 的强公平。

## 6. 稳定节点、授权和路径

### 6.1 数据与类型

- `NodeStore` 集中拥有稳定节点记录，目录项保存稳定 NodeId；`NodeRef` 保留被 grant/流引用的对象，不用字符串路径充当 root。
- GrantState = `{sender_context_id, root: NodeRef, rights, output_transport_rights, account, policy_state, lifetime_registration}`。存储 Lifetime observer，不存 sender 母本。
- `Namespace` 拥有 `prefix → DirectoryGrant`；替换/卸载返还 owner；解析持有自己的引用。
- `Position` 是稳定父 DirectoryGrant + 最终名字 + 可选 expected NodeId/version；Found 的元数据是快照，不是之后操作的授权证明。
- `RequestContext` 持 Delivery、reply_once、输入 owner、授权快照和操作状态；异步路径整体移动。
- 节点 pin、目录链接、打开流与账户 charge 分账。最后引用释放把节点加入用户态 retire 队列，有界释放，不在控制循环的 Drop 中递归遍历子树。
- 后端修改通过正式 `PreparedMutation` 拥有节点/目录项变更、capability 所有权差量、数据存储预留和输出预算。Validate/Reserve 可失败，Commit 不分配、不等待、不再失败；Create、属性替换、WriteAt、Move 共用这个事务边界，不各自维护回滚矩阵。
- 配额检查不能代替真实存储预留。采用 fallible allocation/预付槽和数据块；不能让用户触发的 Commit 调用没有 fallible reserve 的 BTreeMap 插入、无预算 Vec 扩容或递归复制，再以 allocator panic 处理正常资源不足。

### 6.2 权限与撤销

FAL rights 按 ideas/fal.md 的十项业务权利实现。检查入口、每个中间目录及最终对象，结果取 grant ceiling、节点能力和政策交集；不得沿用 memfs 的 X/R/W 属性作为唯一鉴权。

AcquireCapability 专门约束属性/记录中的 capability 出口，不重复取代 Traverse 所授权的受限目录派生、路由和 ReadStream/WriteStream 所授权的建流操作。FAL 操作可以明确铸造新对象，内核 DUPLICATE 仅约束同对象 entry 复制，不被描述成禁止一切用户态再委派。每个 GrantState 的输出运输 rights 由发行政策保存和收窄，不能根据客户端报送的数字放大。

DeriveGrant 创建独立 sender/Lifetime、稳定 root 与收窄 rights，先登记状态和观察再交付 sender；发布失败关闭 sender，Lifetime/Delivery 统一完成回收。父 grant 关闭不撤销 child。

政策撤销在线性化点停止该 context 的新准入；已准入上下文继续持授权快照。删除 registry 项后，旧 sender 的新请求返回 GrantRevoked，不需要保留无界 tombstone，因为 context koid 不复用。已打开流与订阅有自己的管理/取消入口，不把普通 grant close 偷换成强撤销。

### 6.3 解析与 Delegate

客户端顺序解释符号链接和 `..`，回退仅使用已持有的逻辑父帧；孤立 grant 不能越根。更具体的 namespace 路由按组件覆盖，路由前缀的逻辑父层只用于导航，不伪造远端父目录。绝对链接使用显式 namespace，未提供者返回明确边界错误。

每次 Delegate/Link 必须恰好覆盖请求且有合法推进；修复 `verify_cover` 的非等长接受和提前 normalize `link/..`。所有重试共享解析预算与连接 Deadline。

真实路由绑定持有目标 provider 的根 grant。A 对来访 grant 计算 `来访 ceiling ∩ 绑定政策 ∩ B 母本上限`，向 B 非阻塞 DeriveGrant 后才交给客户端。请求 B 的本地根派生不再次递归路由；反向/循环 Delegate 由客户端全调用预算拒绝。

创建/删除/移动使用稳定父目录和最终名字。Open 本地确认目标、鉴权、pin 和准入不可拆成信任客户端 Lookup 的两步。携带能力的写操作先解析到最终 provider，再运输能力；冲突返回明确未提交结果，普通超时不自动重试。

## 7. Wire、属性与其他操作

升级 FAL wire 版本，全部消费者同步迁移，不保留 v1 分支。内核 envelope 留 shared，FAL wire 留 libfal。具体 opcode 从共享的唯一枚举分配，不预留未实现业务。

请求业务 slot 0 是 send-once，额外输入从 1 起；回复从 slot 0 起独立编号。Delivery 不属于业务槽。所有 kind 明确准确 Handle 数、槽用途、允许的 kind/role/rights；全局验证未引用项、重复槽引用、错误 role、未知 flags、reserved、长度和嵌套预算。

| 能力 | 交付契约 |
|---|---|
| Record | 异构具名字段；一次读写覆盖全部元数据与 Handle；编码按总 payload/Handle 容量预算，不使用旧 VALUE_MAX 魔数兜底。 |
| Repeatable Handle 属性 | 存储完整 owner 和 ExportPolicy，每次读取 duplicate/衰减；要求 AcquireCapability。 |
| affine Take | PreparedTake 独占预留值，出站成功入箱后不可失败地置空；发送失败恢复值，预留期间并发写/取返回 Busy。 |
| Handle 属性 Write | 先校验与预留，再原子替换，最后释放旧值。未提交拒绝尽可能按明确响应槽返还新值；回复不可交付时由服务关闭，调用者收到 Sent/OutcomeUnknown。 |
| 同域 Move | 源 target grant 与收到的目标目录 sender 都通过真实 HandleQuery/context 登记验证；同存储事务域一次完成源 Remove、目标 Create、防目录循环、名称更新与事件代次。跨域 CrossDevice。 |
| Copy | 普通 Stream 经客户端 CreateExclusive/Open/传输/双方 Finish；不携带能力的数据属性整值复制。失败返回部分目标身份/进度，不 copy+delete，不默认覆盖已有目标。 |
| Delete | 删除最终目录项，不追踪链接 target；非空目录返回 NotEmpty。已经 pin 的节点不改身份。 |

Directory 类型 capability 的导出必须远端实际派生。其他协议的 endpoint 按发布者显式出口政策交付；没有通用业务衰减协议时不能根据 FAL rights 猜其 badge 权限。

协议错误至少区分 GrantRevoked、权限不足、CrossDevice、Conflict/StalePosition、CursorInvalid、资源/配额不足、Busy、Cancelled、Unsupported 与内部错误；运输 Timeout/ServiceClosed/OutcomeUnknown 留在调用错误层，不混成一个 FAL Internal。

## 8. 服务注册与发现

库重排前本节仅保留方向；现在公共账户/额度已归 `libbudget`，执行已归 `libexecution`，F3d 从真实消费者规模审计开始。未来 `libservice` 拥有 ServiceRecord schema 和注册控制协议，消费 `libfal` 的通用 Record 与 provider 接口；`libfal` 不预定义服务记录、服务状态或 ServiceRecord 预算槽。首个承载者可以是 srv_fs 的服务目录后端，不创建所有进程必须经过的全局注册权威。

状态：Absent → Starting → Ready → Draining → Absent。Register 创建独立 RegistrationControl sender/Lifetime，并捕获已预先限制权限的 endpoint；instance 使用该注册控制对象的不复用身份，记录/目录代次描述同一实例的状态变更。

- 注册上级 capability 限定名称/子树和资源账户；普通目录写权不能绕过状态机。
- Starting 不交付 endpoint，并有有限建立期限；PublishReady 一次发布完整 Record。
- Ready 的读取取得一个完整快照及其 capability；不逐字段读取。快照后 endpoint 可以关闭，发现不承诺即时可用。
- BeginDrain 撤出新发现；endpoint CLOSED 或 RegistrationControl Lifetime CLOSED 触发条件撤销。
- 所有撤销带 instance/generation，旧完成不能删除新实例；已授出的 sender 不因名称撤销失效。
- 记录关闭、导出中的副本、outbox 与观察都有明确 owner，挂起的导出任务持完整记录快照。
- 初始队列 owner、启动控制 sender/Lifetime 由 launcher 显式交付；普通启动控制不充当无鉴权目录 anchor。provider ready 后向 init 交付正式根 grant，再由 init 组装客户端 namespace。

## 9. Open 的完整状态机

### 9.1 接口与数据承诺

Open 请求声明方向 Read/Write、现有节点位置/预期身份、offset、范围约束、RNL2 版本、几何请求和连接 Deadline。回复包含实际几何、offer_deadline，业务槽 0 为 StreamControl sender、槽 1 为 affine Invitation。StreamControl 是独立 sender/Lifetime，不靠猜测 stream ID 授权。

客户端 connect：Open → Attach → 协议验证 → Start，全部消费原连接 Deadline。provider 验证 PEER_ATTACHED 且 offer 未到期才接受 Start；客户端自报附着不构成证据。Start 回复失败可能已进入 Active，按普通 Sent/OutcomeUnknown 和放弃路径收束。

Read 时 provider Producer；Write 时 provider Consumer。基本协议不含创建、append、truncate、双工、快照保证或持久化保证。范围上限 checked 验证，计数不回绕；实际复制每轮受资源/工作预算约束。

### 9.2 类型与阶段

`OpenStream` 持 NodeRef、账户 reservation、方向/范围、Runnel 单侧 owner、StreamControl Lifetime observer、等待注册、计数、终态及清理 owner。

| 状态 | owner 与推进 |
|---|---|
| Preparing | 已鉴权/pin，预留 stream/观察/outbox/内存；任何失败零发布并释放。 |
| Offered | Invitation 与控制 sender 待交付或已交付；保留有限 offer_deadline，禁止执行文件修改。 |
| Active | Start 已接受；以非阻塞任务推进，协商的空闲政策不因伪唤醒或无进展重置。 |
| Terminal | 固定状态与字节计数，服务可回答 Query/Finish；不能提前撤销对端仍需读取的映射关系。 |
| Retiring | 注销观察、关闭 Endpoint/未消费 Invitation、释放节点和额度；失败保留 owner，完整结束才删除账项。 |

Query 立即返回状态；Finish 允许一个有界的待回复请求等待稳定结果，后续查询不分配无限 waiter；Cancel 幂等地发起停止并返回已确定进度。相互竞争由单一服务状态拥有者线性化。

Read 在生产结束发布 EOF，保留端点；Finish 成功需已验证对端 tail 追上最终 head。客户端正常 EOF 在收到业务成功后才向上层返回。Write 的共享环发布进度不是后端进度；客户端发布 EOF 后等待 provider 消费、完成后端并返回 Finish。后端失败保留实际接受字节数，允许部分结果，不声称 rollback。

### 9.3 失败与退款

- 回复发送失败：关闭未交付的 sender/Invitation 和本地 Endpoint。
- 回复入箱但未接收：消息清理释放 Invitation/控制 sender，Lifetime 和 PEER_CLOSED 驱动同一退役。
- 长期未 Attach/Start：offer 到期；即使客户端仍活着也关闭创建端，拒绝迟到 Start。
- Attach 失败：typed failure 返还仍持资源，客户端释放控制权；已消费后的协议失败关闭本地 Endpoint。
- 数据端、控制权消散、取消、空闲政策到期：进入统一 terminal/retire，记录部分字节。
- provider 退出：客户端从 sender CLOSED 和 PEER_CLOSED 收束；最终结果未确认前不能报告成功。
- close 失败：保留真实 Endpoint/清理记录及配额，不通过删除 StreamTable 项伪造退款。重试/升级受服务政策约束，最终由监督者 Drain 进程兜底。

RNL2 EOF 不能承载后端错误；不修改共享 header 去塞文件状态，不把 PEER_CLOSED 当正常 EOF。Drop 是放弃，不是 Finish。

## 10. Watch

客户端持唯一 Notification owner，Subscribe 输入业务槽 1 为 SIGNAL|WAIT|TRANSIT signaler。服务在一次状态修改中鉴权、安装订阅并取 generation，回复 subscription_id/generation/effective_mask；订阅安装后的事件可早于回复并保持 pending。

SubscriptionState 持 NodeRef、授权账户、发起 context 身份及 signaler；Unsubscribe 验证所属 grant，不能靠猜 subscription_id 取消别人的订阅。客户端 Subscription 保留其 DirectoryGrant 使用引用。

create/delete/modify/rename 按位合并；另有 TERMINATED 位，原因通过查询。取消确认后不再 signal，但不伪造清空客户端已 pending 位。客户端同时等待服务 CLOSED；provider 通过 signaler CLOSED 发现 owner 消散，无需等下一次业务事件才清理。

范围只覆盖本 provider 的节点或目录直接成员。客户端采用先订阅后快照，枚举代次失配重读；不承诺可重放、递归或跨 provider Watch。

## 11. 锁阶、失败边界与清理规则

- 内核保持 HandleTable → Mailbox 的投递/接收事务顺序；用户输出校验/预留先于公开 entry，Receive 回滚保存完整 owner。
- MailboxSender 的 badge/目标/identity 不可变，不增加发送热路径对象锁。
- 新 Lifetime 状态锁位于既有 lifecycle 之后、work-debt 之前（命名秩 LIFETIME = 620）；它只发布自身终态，不取 Mailbox、HandleTable、WaitSet 或业务锁。
- WaitSet 状态锁使用命名秩 WAIT_SET = 550。来源锁与目标 WaitSet 锁绝不嵌套；Installing/offer/finish 协议跨越两段临界区。
- 所有 source offer 在来源锁内只做原子仲裁；ready 入队、源注销、关闭其他 owner 和析构在相应锁外推进。
- MEMORY_COMPLETION、WORK_DEBT、REMOTE_CALL、HEAP 等高于或等于 LIFETIME 的基础设施锁内不得析构可能最后释放 MailboxSender 的任务/引用；先取出任务，解锁再释放。不能靠升高 Lifetime 秩掩盖回调锁反转。
- Lifetime、Delivery、WaitSet registration 和 finish 责任都在公开前完成 metadata admission。发布信号及 Close/Drain 不再申请不可保证的清理存储。
- 用户态单一状态拥有者串行提交本地后端；存储/节点/grant 锁不跨 RPC、共享流等待或下游 I/O。后续并行后端必须维护这一边界。
- 失败分为未提交 owner 原样归还、已提交待完成、业务结果未知、终态待清理。不得把“回复失败”统一当业务回滚，也不得把“结果已完成”统一当全部资源已退休。

## 12. 施工图与连接点

任务按开工审视后的前置依赖推进，强耦合机制共同迁移并验证，不引入过渡 adapter 或双轨。完整前置具有自己的完成门，局部编译/测试或单个提交不构成 FAL 总体交付；下表是代码连接点盘点，不是独立验收分片。

| 分片 | 主要落点 | 必须接上的后续责任 |
|---|---|---|
| A 时间 | 期限计划；shared、sched/clock、rinlib | 绝对 Wait/Send、同步/异步 RPC、offer/服务政策，不能只做 Now 读数。 |
| B 身份/交付 | os/handle_table、kernel task/{object,handle,mailbox}，新增 lifetime/delivery；shared、rinlib | 全部 Mailbox 创建、duplicate、send-once、transit、ProcessGrant、Receive/Discard/Drain 迁移。 |
| C 持久观察 | kernel task/{wait,object,notify_work}，WaitSet；trap/sched 安全点、ProcessDrain、rinlib | WaitMany 共用来源机制、通用通知/完成发布、普通 Close 内核退休与 ProcessDrain/异常退出；旧 Seal/Drain ABI 删除。 |
| D 执行/协议基座 | `librpc`、`libexecution`、`libbudget`、rinlib Tunnel、`librunnel` | 真正可组合等待、PEER_ATTACHED、outbox、账户、全部 owner/期限失败路径。 |
| E/F1 授权—后端—协议—执行 | `libfal`、`libfs`、`libexecution`、`libbudget`、`librpc`、`rinlib` | 一个真实 provider 状态拥有者共同消费 GrantTable、AccessSnapshot、FAL2 wire、NodeStore/PreparedMutation、payload retire、显式 wake 与账户退款；不能拆成无消费者的后端或授权阶段。 |
| F/F2–F3 业务与跨 provider | 未来 `libservice`、`libfal`、`libfs` | DirectoryGrant、namespace/Delegate、Record/Handle、注册/发现、Open、Watch、Move、Copy 及所有 terminal/retire；铺路阶段登记计划内消费者与接通条件，闭包阶段闭合对应真实 client/provider 责任。 |
| G/F2–F4 真实装配 | `srv_init`、`srv_fs`、`test_fal`、现有服务/驱动/验收消费者 | F2 完成启动能力图、独立进程、现有消费者迁移和旧泵/anchor 删除；F4 建立独立 test_fal 并执行两个 provider 的组合验证。 |

方向文档已经描述最终契约；实施中只把真实落地事实同步到 impls，不能把本表状态提前写成已实现。代码提交前按项目要求展示摘要并取得 commit 授权；本计划不包含 commit 或 push 授权。

## 13. 唯一残留/删除门

| 现状 | 目标与位置 | 删除触发与验证 |
|---|---|---|
| Entry.badge、entry_with_badge、默认 badge-0 sender | MailboxSender 单一身份；handle_table/kernel mailbox/rinlib | B 全调用者迁移，grep 无旧字段/默认根旁路，duplicate/transfer/rollback 保持 sender identity。 |
| Receive 后只剩整数 envelope | 独立 Delivery owner；kernel Message/shared/rinlib/librpc | B/D 请求、回复、拒绝、outbox、退出均持/移交 owner；无未归属 receipt。 |
| 手动 transit close、测试专用不安全 drain | TransitEntries 与正式有界表事务 | B 全失败路径退款，删除无生产调用的丢 Pinned 辅助接口。 |
| 相对内核等待、截断 ticks/ms、无限发送背压 | 期限计划唯一真值 | A/D/G 全部消费者迁移；旧内核路径删除，便利 wrapper 共用绝对核心。 |
| WaitMany-only 服务、无执行体 | WaitSet + `libexecution` 公平事件循环 | C/D/F 完整工作/退役，无固定 64 项服务上限或用户态轮询补偿。 |
| 裸 anchor/Position/同进程 fs 泵、无鉴权 memfs | 稳定 grant/节点及独立客户端 | F1 建立 provider-local 正式根 grant；F2 迁移最后一个真实消费者时删除 srv_fs 自泵与 slot-1 anchor，并验证越权和竞态路径。 |
| Open/Move/Copy Unsupported、无 Record/Watch/发现 | 本计划声明的正式能力 | F/G 所有真实消费者和失败门完成；超出范围只由扩展操作计划承接。 |

若在这些路径发现新的前置缺口，先修订对应唯一计划和自然序，再继续；不得留下实现中的兼容字段或口头延期。上述旧路径是待删除的当前实现，不是授权引入新的过渡机制。

## 14. 整体验证与完成门

基线以上所有承诺接通后，集中完成验证。测试必须验证真实状态机和失败不变量，不以模型测试代替实际跨进程业务。

- host：身份/运输/回滚；来源发布快照；WaitMany/WaitSet 的安装、rearm、remove、关闭竞态与预算；完整期限；FAL codec、节点身份、权限、路由/链接、Record/Handle、Open/Watch 与部分结果。
- 确定性内核面：最后 sender/在途 Send/queued Delivery/已 Receive Delivery 的关闭顺序，接收写回失败、Discard、队列 owner 退出、跨表运输；Lifetime observer 本身不保活目标；额度恢复。
- WaitSet：超过 64 个真实来源，部分持续就绪不饿死其他任务，输出失败不消费、Remove 对迟到完成有效、非空 Close 保留 owner、max_work=1 Drain、源与集合两侧退出，最终库存退款。通知补证覆盖高秩锁内最后 sender 消散、纯 Resume syscall 返回、idle 前新发布和预算耗尽后的 pending 门铃，不依赖下一次偶然 timer。
- 时间：非整千 timebase、跨 hart 非倒退、checked overflow、raw 回绕边界、计算 deadline 后的抢占窗口、满箱直到到期、已入箱超时/迟到回复、下一调用成功。
- 业务拓扑：两个独立 srv_fs 实例（不同后端/路由域）及独立 test_fal；通过真实启动 grant 组装 namespace，不用客户端泵或共享同一 MemFs 实例。
- 授权：转交后原进程退出仍可用；最后引用与 Delivery 收束后退款；根逃逸、权限放大、伪造对象身份、错误 role、同 badge 不同 sender、Delegate 衰减、路径重命名竞态。
- 流：超过一页且多次跨环；Read/Write、零长度、背压、EOF/最终状态、部分写错误；不 Attach、不 Start、Start 前后关闭、投递失败、客户端/服务退出；一个阻塞流期间控制面与其他流继续推进。
- 注册/Watch：Ready 快照一致、旧实例清理不能删新实例、已授 sender 不因撤销名称失效；订阅先于快照、安装后回复前变化、独立消费者、取消和静默退出。
- 组合门：完整 host/静态检查、`just clippy`、`just acceptance`；涉及调度域契约时追加 virt-hetero。按项目脚本保存日志、保留退出码、收割 QEMU 残留；已知竞态矩阵 flake 按 KNOWN_ISSUES 判读。

只有公共前置、全部真实消费者、失败/退役路径、旧机制删除、组合验证和文档现状同时满足，才标记整体完成。提交后登记对应固定提交的未来代码 Review；方案本身不送 reviewer。
