# 第四批独立 Review：Notification N6

## 范围与结论

本审查针对 `plans/todo-2026-10-10-notification-observer-roles.md` 当前 N6 注入前置，以及已实施的 Notification Owner/Signaler/Observer 代码、消费者和验证边界。读取了 `plans/REVIEW.md`、FAL/service 主计划中的 Watch 接点、`notes/impls/{ipc,service,framework,runtime}.md`、Notification/handle/metadata/进程寿命源码和现有运行日志。

范围不包括 FAL/fs Watch/Copy/Bind 业务协议、对象链基础 owner、Runnel/Dispatcher 执行语义、设备/DMA 或平台保留内存。本报告不重新设计三角色，也不复制前几批报告的 findings。

**结论：未发现有证据支持的固定代码反例；N6 仍未闭合，存在三组精确注入/逐项费用证据缺口。** 当前实现的失败 owner 与寿命结构在源码上自洽，正常 core/stress/release/完整 acceptance 只能证明已覆盖的正常角色、运输、退出和邻接行为，不能替代 N6 的资源阶段注入。

## 当前实现事实

`NotificationCore` 唯一持有 pending、closed、WaitQueue、维护 Work 和 core metadata；Owner 的 header 投影该 core，Owner 最后 capability 执行 `shut`，清 pending 并发布 CLOSED。Signaler 与 Observer 各自拥有独立 header/metadata，但只强持同一 core；Observer 仅允许 `WAIT|DUPLICATE|TRANSIT|GRANT`，可观察 `READABLE|CLOSED`，没有 Take/Signal/Mint 权限（`os/kernel/src/task/notification.rs:21-31,173-188,206-265`）。

`mint_observer` 先以 `Authorization::accept` 接受源 handle 的 `MANAGE`，再核对 Owner 类型和输出 rights；它只 clone core，不增加 Owner holders 或业务 use。Observer Arc 从调用线程的 `funding_pool` 支付 metadata，KOID 和 Arc 构造失败会在局部 owner 退出时释放付款；成功的 cap 经过 `handle::install` 发布（`os/kernel/src/task/notification.rs:267-302`，`os/kernel/src/task/handle.rs:23-69,176-185`）。

`handle::install` 的责任顺序为：持 BodyLease，预留 handle metadata/槽，锁外写回，执行 publish，再 commit；写回失败 rollback reservation 并 `release_all`，reserve 失败同样释放 cap，commit 失败或 Frozen/shut 路径由表的预留释放事件收束（`os/kernel/src/task/handle.rs:209-264`；`os/capability/src/table.rs:333-422`）。Observer 的 metadata charge 由 `MetadataCharge` 持有实际 Pool 和 `AllocatedCredit`，Drop 才归还额度，不按逻辑对象状态提前退款（`os/kernel/src/task/resources.rs:37-53,82-87`）。

源 core 已关闭并不重新打开 Notification：Owner Close 在 Notification 锁下设置 `closed=true`、清 pending 并置 CLOSED；已接受的 Observer/Signaler/WaitSet source loan 仍可观察 CLOSED，不能触发 Owner hook，也不能恢复 pending。Observer/Signaler/WaitSet 登记引用与 core metadata 的存活是独立费用责任（`os/kernel/src/task/notification.rs:62-129,132-169`；`notes/impls/ipc.md:39-45,49-57`）。

进程 Frozen 由 Shell ingress 计数与 BodyLease 共同保护。`lease_body` 成功后登记 ingress；Frozen 只在 ingress 清零后继续回收。因此 `mint_observer` 的接受、写回和 handle commit 期间不会被正常 Frozen 路径提前拆 Body（`os/kernel/src/task/process/shell.rs:181-209,282-318,724-731,842-859`）。输出 fault 会由 uaccess 记录到当前线程，随后由 trap 路径冻结进程；这不是当前已有的 Notification 专用注入证据（`os/kernel/src/uaccess.rs:157-199`）。

## Findings

### N6-E01：mint 资源阶段没有精确注入和逐项退款证据

**类型：证据缺口，不是固定 bug。**

- **固定位置与可达路径：** `os/kernel/src/task/notification.rs:284-294` 的 Observer metadata/KOID/Arc 构造；`os/kernel/src/task/handle.rs:231-262` 的 handle 表 reserve/commit；路径为 `NotificationMintObserver` syscall → `Authorization::accept` → Observer metadata/Arc → `handle::mint` → `handle::install`。
- **契约：** quota/metadata 额度、allocator/Arc 分配、KOID 铸造和 handle reserve 任一失败，都必须保留源 Owner、pending/closed 事实，释放未公开 Observer、预约和已取得的额度；失败不能消费输入 Owner。
- **当前结构：** `MetadataCharge` 是局部 RAII owner；`ObjectHeader::try_new` 和 `Arc::try_new` 的错误会在返回时释放已取得 charge；`handle::reserve` 预留失败不会留下部分 reservation，`install` 失败调用 `release_all`。源码没有发现丢失 Owner 或清 pending 的路径。
- **缺失证据：** 现有 `test_pool_budget` 验证的是 Pool 派生/Invitation 创建的双维 quota，不是 Notification Observer metadata 的实际付款；现有负向角色测试只验证权限错误后 Owner/pending 仍可用。没有真实注入 `QuotaExceeded`、allocator `OutOfMemory`、KOID `ReachLimit` 或 handle metadata reserve 失败，并逐项核对 Observer Arc、core、handle metadata、WaitSet source 和已登记维护引用的额度变化。
- **影响：** 不能把源码责任链提升为实际 N6 失败/退款通过；若未来修改 metadata 布局、Pool 付款或 cap table reserve，现有正常运行不会捕获付款归属/回滚回归。
- **归属与完成门：** Notification 前置计划 N6，owner RubyRaven，协调 MellowPond。需要现有隔离目标或正式 Pool 饱和设施在真实 mint 路径施加单阶段失败，并在失败后继续使用原 Owner/pending，再在所有物理引用退出后核对额度恢复。不得以 init 自身耗尽资源或普通 Pool/Invitation 失败替代。

### N6-E02：输出写回、安装 Frozen 边界没有真实故障证据

**类型：证据缺口，不是固定 bug。**

- **固定位置与可达路径：** `os/kernel/src/task/handle.rs:231-262` 的 reserve→uaccess write→commit；`os/kernel/src/uaccess.rs:188-199` 的输出 fault 记录；路径为 Observer cap 已构造且 handle reservation 已取得后，用户输出地址 fault，或进程在安装交界进入 Frozen。
- **契约：** 输出写回失败必须 rollback reservation、释放未公开 Observer 和对应 metadata，源 Owner/pending 不变；进程 Frozen 时不得公开半安装 handle，也不能让 commit 释放路径把已写出的 stale 数值误当成成功能力。
- **当前结构：** BodyLease/ingress 把有效 syscall 安装窗口纳入 Frozen 屏障；写回错误显式 rollback/release；表 `commit` 对 shut 后 reservation 有预付释放事件，避免在 commit 阶段重新分配。未发现固定的半安装或 pending 清除代码反例。
- **缺失证据：** 没有在 Notification mint 的已接受点注入输出 fault、handle table shut/Frozen 交错或 reservation commit 边界；现有 Boot/IPI/VM fault 工具不定位 Observer metadata、handle reserve/install 或该 syscall 的输出地址。完整 acceptance 中的 Boot/VM 故障和正常 reset 不是这个窗口的证据。
- **影响：** install 的源码回滚规则已审计，但真实 fault 后的表代次、输出值、Observer metadata、源 Owner/pending 与进程退出接管之间没有现场证据。
- **归属与完成门：** Notification 前置 N6，继续由 RubyRaven/MellowPond 承接。应沿既有隔离子进程/正式 fault 设施命中 mint 输出或安装阶段；进程被真实 fault 终止时，须按进程退出合同核对 Owner/core、Observer 壳、handle metadata、WaitSet source 和维护 Work 的最终释放，而不是把 fault 本身记为退款通过。

### N6-E03：接受后源 Close 与逐壳物理退款没有精确交错证据

**类型：证据缺口，不是固定 bug。**

- **固定位置与可达路径：** `os/kernel/src/task/notification.rs:267-302` 的接受后 clone core/Observer 发布；`NotificationCore::shut` 位于 `:113-129`；Owner 最后 capability hook 位于 `:232-234`；Wait/WaitSet source loan 与成员释放见 `notes/impls/ipc.md:39-57`。
- **契约：** Close 先于接受时，源 handle 解析失败；接受一旦完成，Owner 随后 Close 可以成功，Observer 必须看到已 CLOSED 状态而不能重新打开 core。Observer 壳、Signaler、WaitSet source loan、core Work 和维护引用分别由各自 owner 持有至物理释放；普通 Observer handle Close、stale handle 或 Observer 数量归零不能冒充 core/Watch 终态退款。
- **当前结构：** `Authorization` 的 BodyLease 只保护接受和安装过程，不增加 Owner capability holders；clone core 后 Owner Close 会清 pending/置 CLOSED，观察角色仍保留 source storage。WaitSet 成员由集合 owner detach，Notification Work 每步发送一个 hint，所有引用和 metadata 按实际 Drop/退休释放。正常 `public_ipc::notification_roles` 已验证 Owner Close 后 Observer/Signaler 看到 CLOSED，Observer 消散不清 pending，且普通能力 Close 不需要资源分配。
- **缺失证据：** 当前夹具是接受完成后再 Close 的正常顺序，未在“接受前/安装中/提交后”分别强制 Close；没有逐项账本证明 Observer 壳、源 core、handle metadata、WaitSet source loan、维护 Work 和已交付 Watch 责任在不同释放点分别归还。既有 `test_target`/IPC kill 证明进程退出观察语义，但不证明 mint 交界窗口；普通 Close/重复 Close/伪造 stale handle 已明确不是 N6 资源故障。
- **影响：** 不能证明源已关闭或安装交界时的 accepted Observer 一定既可观察 CLOSED 又不提前释放任何仍被登记/维护引用使用的存储；也不能证明逐壳费用只在真实 owner/物理退出后退款。
- **归属与完成门：** 通用 Notification N6 由前置计划承接；Watch 记录、控制 Lifetime、sequence/pending、业务终态和业务退款仍归 FAL/fs 主计划，不在此报告扩展。补证需使用真实 Owner/Observer/WaitSet/维护引用，逐点记录 Close 顺序、CLOSED 结果和最终 Pool/metadata 余额；不得以 Watch Query/Ack 或普通 provider 退出代替。

## 已覆盖与明确排除的证据

现有 `public_ipc::notification_roles` 已覆盖角色权限、Observer 不消费 pending、部分 Take 的 READABLE、WaitSet 持续电平、运输与 typed 采用、Owner Close 后 CLOSED，以及正常 Observer/WaitSet 清理（`user/services/srv_init/src/public_ipc.rs:465-729`）。日志中相应锚点为 `Notification observer roles passed`；完整 acceptance/P4 还覆盖相邻 IPC、Startup、监督、Watch 正常业务与平台故障。它们支持 N1/N2/N3/N4/N5/N7 及主计划已有业务边界，不支持上述三组 N6 注入。

当前 `notes/impls/ipc.md`、`service.md`、`framework.md`、`runtime.md` 对角色、source loan、typed owner、Runtime 维护和付款分层的描述与代码一致；没有发现需要在本报告范围内修正的实现说明。FAL 主计划中的 Watch 协议、Copy/Bind、Dispatcher 和业务控制寿命仅作接点引用；其业务窗口不升级为 Notification N6 finding。

## Review 判定

本次没有固定代码 finding 可交给修复；N6-E01—E03 是必须保留的证据缺口。Notification 前置在这些窗口补齐前不应归档，也不应把普通 Close、stale handle、init 自身 OOM、Boot/IPI/VM 故障或完整 acceptance 通过写成 N6 完成。报告只记录审查事实，不修改生产代码、Notification 计划、COMPASS、FAL 主计划或其它报告。
