# 统一等待源组件

状态：**轮次5定稿及实际消费者已交付，后续内部完成通知协议按下方审查修正**。轮次2–4和最初方案保留推导，不作当前依据。本案属于[全项目重构](todo-2026-09-29-system-redesign.md)，首次实施/验证由[对象链档案](archived/todo-2026-10-01-object-chain-kernel-integration.md)承载，后续修复见[审查修复档案](archived/review-2026-10-07-object-chain.md#通知与引用归还的最终协议设计轮次4)。ProcessShell终态、ThreadControl、Mailbox各类型、Notification、ReplyObservation及用户fault依赖均已接入。用户裁决仍为纯电平、删除WaitMany、禁止嵌套WaitSet。

当前没有待接通的本链来源或清理责任；未来新来源按轮次5的稳定节点、owner自摘和付款合同接入。硬件故障/全安装交错穷举仍是验证边界，不能由已过正常路线推成全面故障证明。当前面貌由[task实现](../notes/impls/task.md)拥有。

## 审查修正：内部完成来源与维护wake

轮次5的普通Thread/WaitSet提示仍在全部业务门外交接。SignalSource内部完成是一个明确的短边：DONE/internal_done提交与maintenance::wake状态捕获同来源门发生，避免唯一内部等待者先完成并把最后根递归回发布者。它只改记录状态；潜在最后根及IPI由调用方带出全部领域锁后处理。QUEUE内非last保证、Work.record纯投影和来源→QUEUE锁序由[维护计划](todo-2026-09-29-kernel-maintenance-execution.md#状态与唤醒)拥有，不把这条边推广为锁内执行Hint/业务回调或最后析构。

用户fault等待复用事务Completion的独立SignalSource，无每PTE actor；其捕获前释放空间锁/短loan，Frozen走原detach，DONE晚于退款/access归还，详见[内存分案fault](todo-2026-10-01-memory-redesign.md#fault)。原方案中“所有唤醒都必须在来源锁外”的绝对表述仅对普通提示适用，不覆盖上述内部记录仲裁。

## 范围与非目标

- 拥有：来源电平、等待轮次与唯一完成、等待节点存储寿命、准入与付款、有界扫描、取消/超时/Frozen 的摘除 owner、WaitSet 成员与 arm。
- 不拥有：消息语义、Delivery/Lifetime、TRANSFER 环（IPC 分案）；调度队列（sched）；Work 执行机制（维护执行分案，本案只提出所需的记录状态）。
- 替换：`task/object.rs::ObjectWaitState`、`task/wait.rs` 的 `ObserverSink`/`FinishClass`/`WaitContext` 完成仲裁、`notify_work.rs`、`KernelObject::rearm_observer` 等转发入口、直接 WaitMany。旧路径只作现状事实；删除时逐项接住其仍必要的责任（轮次仲裁、唯一完成）。

## 用户裁决（2026-10）

1. **纯电平就绪**：唤醒只是重试提示，醒来重读当前电平；信号在被扫描到之前被他人消费（如 Notification 待决位被取走）则继续等待，不承诺捕获登记期间短暂成立的事实。扫描协议仍须保证**持续成立**的电平最终覆盖全部既有等待者。绝对 deadline 在重试中不重置。
2. **删除直接 WaitMany**：多项等待一律使用显式 WaitSet。每线程只有一个单对象等待节点；ABI `wait.rs` 多项接口与其消费者同步迁移，不留隐式旧路径。

## 设计

### 1. 三层

- **来源层**（`os/capability::signal` 纯逻辑 + 内核 `task/wait_source.rs`）：电平、侵入式等待链、扫描游标。来源状态与对象业务状态同一同步域。
- **等待层**：每轮等待的 epoch、安装交接、唯一结果、节点何时可复用。
- **目标层**：线程 Ready ingress（进程回收分案的原子 gate）、WaitSet 完成槽、退出回执。

### 2. 来源入口与锁

- 来源归共享 core 所有（Mailbox、Notification 在 core，不按每个 Sender 复制）；授权对象只提供受限观察视图。
- `KernelObject` 不返回裸 `&WaitSource`，而提供受同步域保护的操作（`with_source(|guard| …)` 一类），由对象证明持有含业务状态的锁。
- “锁外”指**释放全部相关业务锁之后**（如 ReplyObservation 从 Mailbox core 锁内发布，唤醒须在 core 锁也释放后）。来源锁内只更新状态、摘候选；唤醒、Ready 发布、timer 注销与最终 drop 都在锁外。

### 3. 节点存储与所有权

- 节点使用稳定地址：线程节点内嵌于 Thread；WaitSet 成员节点为成员各自的稳定分配，不放入会搬迁的集合元素。
- 四段所有权：`安装中 → 链上 → 已摘出未交接（持完成凭证）→ 可复用/可释放`。摘链者取得有限期完成凭证，锁外交接结束才归还。
- 退出、Frozen、WaitSet 关闭必须覆盖在途凭证：以“凭证已归还”而非“链上已无条目”判定节点可释放。
- 强 owner：Waiting 线程由进程成员容器强持，退出时主动拆除；来源只持链接，不强持线程或 WaitSet。节点在链上期间持来源观察存储（保活存储，不计入 Holders、无 mint 资格）。

### 4. 轮次与唯一完成

- 每轮等待有独立 epoch（线程节点/WaitSet arm 各自递增）；来源候选、timer、取消都绑定该轮。旧轮迟到通知 epoch 不符即丢弃。
- 状态：`Idle → Installing → Parkable → Completing → Idle`；唯一完成原因（Signaled/Closed/Timeout/Cancelled/Frozen）由第一个 CAS 胜者写入。
- 安装期间允许记录完成（例如 timer 先到期），但现场交付推迟到安装结束；安装者结束时发现已完成则不停驻。timer 先登记、对象后挂链，迟到 timer 由 epoch 拒绝。
- 旧凭证未归还前不得重置同一节点进入下一轮。
- 停驻前在成员锁内检查完成态；完成方也取同一成员锁转 Ready，因而闭合。

### 5. 唯一锁外完成入口（与 Frozen 协调）

1. 持完成凭证，在成员锁内核对成员身份与 epoch。
2. 只有唯一完成者消费预付 Ready admission。
3. 同一成员锁内对 ingress 字 CAS `!Frozen`：成功才转 Ready，入队后释放计数；见 Frozen 则转离场责任，不入队。
4. SweepMembers 与迟到完成者争同一完成/离场 owner；完成凭证收束前不释放节点。
- 结果仲裁与执行准入分开：信号先赢得结果，不阻止之后的 Kill/Frozen 终止线程。

### 6. 有界扫描

- 预算按**检查过的节点数** qW 扣减；用尽未完成一轮直接续调，不为判断剩余匹配额外遍历。
- 游标在删除、重挂、并发新增下有效；新增挂在游标之后的位置，频繁更新不把游标重置到头部，避免尾部饥饿。
- `update` 锁内只改固定规模状态、标记待扫描并按需激活来源扫描 Work；首批可在调用者处有界推进。
- WaitSet 被唤醒后发布自身电平只提交工作，不递归同步展开。
- 扫描 CPU 计入来源对象的出生 Pool 账户。

### 7. 维护记录：可重复活动的承载

现有 `maintenance::Record` 的 Done 不可复活，与反复扫描不相容。向维护分案提出：增加 **Quiescent**（当前无责任、释放执行强根）状态，重新激活与并发更新在维护队列锁下仲裁；Done 仍永久终结。来源对象退休时由业务关闭发布最终扫描后转 Done。需与维护分案同步收敛后实施。

### 8. 关闭与终态分开

- 来源有通用**关闭状态**（停止服务）：关闭使所有等待返回 Closed，不论业务 interest；何时关闭由对象决定，不由通用最后持有 hook 推导。
- 业务完成信号由对象在自身终态屏障成立后发布。最后 capability 消失不意味着终态（ProcessControl 归零进程照常运行；ReplyObservation 关闭只放弃观察）。

| 来源 | 业务信号 | 关闭 |
|---|---|---|
| MailboxReceiver 视图 | READABLE、NO_SENDERS（可清除） | 接收端 shut |
| MailboxSender/OnceSender 视图 | WRITABLE | 接收端 shut → PEER_CLOSED |
| NotificationOwner | PENDING | Owner shut；Signaler 不可等待 |
| ReplyObservation | SENT / ABANDONED（终态不可变） | 无（关闭观察权不改结果） |
| ProcessShell | TERMINATED（终态屏障后） | — |
| ThreadControl | DONE（执行完成后） | — |

### 9. WaitSet

- 区分持久 **membership** 与单次 **arm**：membership 持来源观察存储与预付完成槽，arm 只决定是否挂链；Rearm 不重新解析 Handle。
- Add 从 WaitSet 出生 Pool 追加实际 charge，可失败回滚；Remove/Close 先关闭新 arm 准入，再逐项收束链上和在途凭证，有界。
- WaitSet 互相观察可能成环：最后 capability 触发主动拆成员，不依赖 Arc 归零。

### 10. 准入清单（提交后必成）

登记提交前完成全部可失败准备：稳定节点、timer 容量、Ready admission、每成员完成槽、维护承载。`timer_queue` 登记若会扩容，改为线程出生时预留一个 timer 槽。提交后不得因资源不足失去完成路径。

### 11. 计数

- 分开来源版本、等待 epoch、成员身份 generation；纯电平下来源不保留全局代次（锁内挂链与脏标记替代）。
- epoch 每轮只由该节点 owner 推进，在新 arm 准入时处理耗尽（拒绝新等待），不让已提交完成、Close、退出失败。

## 验证门

- `os/capability::signal` host 单测：登记-复检无丢失；大量不匹配条目下单步检查数 ≤ qW；扫描中并发新增/删除游标有效。
- 等待层 host 单测：旧轮迟到通知被拒；安装中超时不丢；完成凭证未归还不复用。
- 整链 QEMU：摘链后 Frozen；超过 qW 后清电平（持续成立者全部覆盖）；嵌套 WaitSet 通知；同一来源反复扫描；进程退出/Kill 后 ProcessWait 醒来；Mailbox 关闭唤醒接收者。

## 设计轮次 3：advisor 复审轮次 2 的七处补齐（待复审）

1. **扫描公平**：来源维护“本轮未扫描链”与“已扫描/新增链”。本轮只消耗前者；新增节点在来源锁内先复检电平，再挂入后者，不得插到本轮未扫描节点之前。游标所指节点被删除时，游标在同一来源锁内 O(1) 前移到其后继。脏标记只请求下一轮，下一轮开始时两链交换。
2. **结果写入者与收束执行者分离**：唯一结果由 CAS 决定（来源、timer、取消、Frozen 均可赢）；收束执行者另行确定，负责来源摘链、timer 撤销、全部在途访问归还，完成后才发布 Ready 或可 Rearm 的结果。来源摘链者输掉结果 CAS 仍须交还凭证。Installing 阶段早到的结果只记录，由安装者跳过或撤销挂链。Frozen 改变执行去向，不跳过收束。
3. **timer 在途访问**：timer 状态 `Queued → Extracted → Returned` 纳入节点访问寿命；取消区分“成功移除”与“回调已取得访问权”，后者计入在途访问。节点可释放条件覆盖安装者、来源凭证、timer 回调与其他已取得访问权的完成者。
4. **Quiescent 转换（与维护分案同步）**：`activate(Idle|Quiescent) → Queued` 并接管执行强根；`activate(Running) → Running{notified}`；`finish(Quiescent)` 遇 notified 回 Queued，否则释放执行强根；Done 永久拒绝激活，进入 Done 前先关闭产生扫描责任的入口。Quiescent 退根与 Done 清理是不同动作，旧一轮退根不触及新一轮状态。
5. **timer 承载**：选“线程持稳定 timer 节点”路线：timer 队列改为侵入式、登记不扩容的索引结构，节点随线程存储出生准入；迁移时在新 owner 队列挂接，旧回调收束后才复用。去掉 token 的独立 generation，失效由等待 epoch 与节点在途访问负责。
6. **WaitSet 完成槽**：Rearm 前置条件为旧结果已消费或显式丢弃、旧安装与凭证全部收束、成员未 removing。Receive、Remove、Rearm 在 WaitSet 锁下仲裁；Remove 后迟到完成只归还责任，不发布成员结果。每成员一个完成槽。
7. **ThreadControl DONE**：沿用进程回收分案的 join 屏障：执行离场，且可能访问线程栈或结果页的已提交义务与访问租约全部收束后才发布。

验证门补入交错：扫描插队、结果胜者与摘链者分离、timer 出队后 Frozen、Running→Quiescent 并发激活、未消费结果时 Rearm、异步写回未结束时禁止 DONE。

## 设计轮次 4：以删减收敛（取代轮次 3 的补丁，用户要求“删减大于新增”）

轮次 2/3 的复杂度都来自同一个根：**来源替等待者完成并摘链**，于是节点在来源锁外仍被他人访问，才需要四段所有权、完成凭证、结果写入者与收束者分离、timer 在途访问协议。本轮删掉这个根。

### 核心规则：来源只唤醒，等待者自摘

- 来源扫描时**不摘链、不写结果**：对匹配节点在来源锁内 clone 一份目标强根（`Arc<Thread>` 或 `Arc<WaitSet>`，有界），锁外发唤醒提示。
- 唤醒只是重试提示（纯电平，用户已选 A），重复唤醒无害。
- 节点的挂链、摘链、复用只由**节点 owner 自己**做：线程醒来后在来源锁内自摘再返回；WaitSet 成员由 WaitSet 自己摘。来源从不在锁外持有节点指针，故节点寿命 = owner 寿命，无需凭证。
- timer 到期同样只是唤醒提示：回调持 `Arc<Thread>`，不碰节点。线程醒来自己注销 timer、自摘来源节点。
- Frozen/Kill：Sweep 只把线程唤醒并标 Exiting；线程在自己的离场路径上自摘节点、注销 timer，计入既有离场回执与 WaitShell。没有第二个清理 owner。

### 删除项（相对轮次 2/3）

| 删除 | 原因 |
|---|---|
| 四段节点所有权、摘链完成凭证 | 来源不再在锁外持节点 |
| 结果写入者/收束者分离、结果 CAS | 无结果交付，醒来重读电平 |
| timer Queued→Extracted→Returned 在途访问协议 | 回调只持线程强根，不碰节点 |
| 未扫描链/新增链双链公平 | 扫描不改链；新节点挂尾后自复检电平；扫描中再次 update 只置一位“本轮后重扫”，单遍游标必到尾 |
| WaitSet arm/Rearm、每成员完成槽、ReadyRecord 队列 | 见下 |
| 来源侧 Frozen 摘除路径 | 由线程自摘 |

### WaitSet 简化为“可等待的电平聚合”

- membership 持久挂链（纯电平下无需单次 arm）；成员被唤醒提示时，在 WaitSet 锁内置成员的 maybe-ready 位并更新 WaitSet 自身 READABLE 电平。
- Receive：有界遍历 maybe-ready 成员，在各来源同步域重读电平，返回当前满足者并清位；不满足者清位即可。无 Rearm、无结果槽、无覆盖问题。
- Add 时付款（节点 + 位），Remove/Close 由 WaitSet 自摘。WaitSet 观察 WaitSet 时，强根只在唤醒提示期间短暂存在，不形成持久环。

### 保留项（无法再删）

- 等待 epoch：仅用于线程判断“本次醒来是否属于当前等待”，耗尽在新等待准入时拒绝。
- 有界扫描 Work：每次 update 只置脏位并 publish；扫描按检查节点数扣 qW。维护记录需要“完成后回到可再激活”的一个转换（finish 遇 notified 回 Queued，否则回 Idle 并释放执行强根）；Done 仍永久。须与维护分案同步，这是唯一新增的维护契约。
- timer 无 OOM：线程出生内嵌 timer 节点，`timer_queue` 改为侵入式结构，登记不扩容、删 token generation。这是替换现有 Vec 结构，不是新增机制。
- ThreadControl DONE 沿用进程回收分案 join 屏障（轮次 3 第 7 条保留）。

### 代价

- 被唤醒后电平已被他人消费的线程会醒一次再睡，属纯电平语义的预期。
- 等待者返回前需再进一次来源锁自摘，每次等待多一次短临界区。

## 设计轮次 5：轮次 4 定向修正（用户裁决：禁止嵌套 WaitSet）

保留轮次 4 的“来源只提示、不摘链、不写结果”，补回被误删的必需责任；不恢复完成凭证、结果 CAS、arm/结果槽。

1. **唯一清理入口**：等待侧拥有唯一的 `detach(entry)`（来源锁内自摘 + 注销 timer）。正常路径由等待续体在 syscall 返回用户前调用（等待 syscall 以“重读电平→满足则 detach 返回 / 否则停驻”的内核续体执行，唤醒后从续体恢复）。退出路径由仍持有线程执行 owner 的退出入口（`sched::reap` 等）在释放 Thread 之前调用同一 `detach`；Waiting 的 owner 在成员锁下取走，锁外清理。顺序固定：**detach → 释放线程执行 owner → 请求 Departure**。ThreadDeparture 只负责结果义务、成员离场与 DONE，不持 Thread、不做 detach。
2. **持有图**：来源节点存目标 Weak，来源锁内 upgrade 得临时 Arc 带出锁，锁外 drop；节点（经其 owner）强持来源观察存储直到 detach 结束。来源业务关闭不释放观察存储，关闭后仍可查询关闭事实与注销。“节点已摘”不等于 Thread 已退款，退款仍按进程回收分案的实际退款屏障。
3. **安装—提示—停驻握手**：每轮等待在成员锁下有 pending 提示位；提示方在成员锁内置位（epoch 匹配才置）。每次调度推进只做一次复检（电平、关闭、绝对 deadline），复检前消费 pending；停驻判定在同锁内检查复检后新到的 pending，置位则经既有 Ready ingress/Frozen 仲裁 `Requeue`，下次调度再进续体，**不在内核原地循环重试**。超时到期走同一握手，绝对 deadline 不变。禁止锁嵌套：Members(610)/WaitSet(550) 锁内只取引用与身份，放锁后逐个进入来源锁，再回目标锁提交。
4. **扫描游标与轮次边界**：游标归来源锁；owner detach 游标所指节点时同锁 O(1) 前移游标。来源内嵌稳定的轮次边界标记，一轮扫描至边界止，新增节点挂在边界之后，边界在本轮结束时移到尾部。update 在扫描中只置“本轮后重扫”位。
5. **WaitSet（扁平）**：成员只能是真实来源，WaitSet 不得作为成员（Add 时按 ObjectKind 拒绝），故无存储环、无反馈环；最后 capability 仍主动拆成员。Receive 串行化：WaitSet 锁内取走本次候选位并记成员身份 → 放锁逐个查询来源 → 回锁：仍满足且成员身份未变则重新置位并返回，Remove 后的迟到结果丢弃。满足项轮转返回，防永久就绪成员独占。
6. **维护重复激活**（同一队列锁内）：Idle→activate 入队接管强根；Queued→合并；Running→置 notified；finish 本轮无工作：notified 则重新入队，否则转 Idle 并带走本轮强根，锁外只 drop 该 Arc；Done 拒绝激活，进入 Done 前先关闭产生扫描责任的入口。无 Quiescent 状态。
7. **timer**：线程出生内嵌 timer 节点，`timer_queue` 改侵入式、登记不扩容、删 token generation。到期在 timer 队列锁内彻底解除链接并复制 `{Arc<Thread>, epoch}`，解锁后不再访问节点；取消按所属队列与轮次定位，不误删复用后的登记。
8. ThreadControl DONE 沿用进程回收分案 join 屏障。

代价：纯电平下的一次空醒；嵌套 WaitSet 能力取消（用户确认）。

**轮次 5 结论**：advisor 复审后余两项阻断（Departure 不持 Thread 却承担 detach；pending 驱动的原地重试无上界），已按其删减型修正闭合（上文第 1、3 条）：退出清理归释放 Thread 前的执行 owner，重试改为 Requeue。advisor 判定“采用两处局部修正即可收口”。**设计定稿，可施工**；维护重复激活转换（第 6 条）须同步写入维护分案。

### 施工中的真实消费者接缝

全仓直接 WaitMany/便利封装有 47 处调用，42 处固定单对象、5 处真实多项（srv_init supervisor/main、libfal client、librpc caller）。supervisor 的多项包含若干 Runtime 导出的 WaitSet READABLE 观察句柄，不能把这些集合再登记成一个新 WaitSet 来机械替代 WaitMany。用户态执行领域须闭合扁平来源装配和各 Runtime 的登记/事件分流责任；此接缝不恢复嵌套能力，不重复询问既定裁决。消费者位置、实施顺序与待接线范围记录在对象链入口。

当前来源契约入口：ProcessShell 的 TERMINATED 用 DONE 位表示，Body/VM 退休退款完成后发布，Control 可查询稳定 Dead，不置来源通用 closed。Job 完成用 CLOSED。Notification 的三角色、Owner[MANAGE] 派生 READABLE/CLOSED Observer、Signaler 可获 WAIT(CLOSED) 及公开 LifetimeObserver/ReplyObserver 命名由 [Notification 前置](todo-2026-10-10-notification-observer-roles.md) 接管；本篇历史轮次的旧角色写法不再定义当前 ABI。ReplyObserver 的 Sent/Abandoned 仍由 DONE/PEER_CLOSED 表示，MakeSendOnce 同步交付完整 pair；通用扁平等待、来源只提示和唯一 detach 契约保持。对象链既有证据从原记录查阅，新增角色/消费者证据从 Notification 前置和其链接的主计划查阅。
