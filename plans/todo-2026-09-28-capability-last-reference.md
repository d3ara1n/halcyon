# Capability 与资源释放重构

状态：**设计轮次 2：全项目重审下的 capability/Close 分案，尚未实施**。用户已确认消除特殊资源特判，以及 Close 必成、有界释放本地持有而不隐式等待完整退休。总范围由[全项目重构总纲](todo-2026-09-29-system-redesign.md)拥有；维护执行和进程回收分别立案，本篇不再独立决定它们的实现。

## 目标与本轮边界

本案审视 capability、对象持有与通用释放，不以现有对象种类、角色编码、ABI、库封装或消费者状态机为前提。类型专用业务操作可以不同，公共层不得通过识别具体类型补齐释放。

目标：authority、已接受使用、存储保活和退休分开表达；所有合法本地 owner 的释放统一且必成，调用方不因 Drop 等待对端、I/O 或完整退休。需要完成保证的 Unmap/Finish 等显式操作各自拥有其契约。

基线：`master` / `d7d97d4c2947c4acda77bcd50794dd1c6f5b970f`。现状起点见[生命周期收敛归档](archived/todo-2026-09-28-lifecycle-convergence.md)。下列角色与引用行为是源码事实；新的对象模型须从独立需求推导，不能把现状表当作目标保留清单。

本记录不替代 [内核最终架构 Review](todo-2026-09-kernel-final-architecture-review.md)，不宣称已经审计整个内核；FAL 自身的协议耦合与重复任务状态不归因于本轮内核问题。独立 metadata 预算仍由 [预算计划](todo-2026-09-14-kernel-memory-budget.md) 拥有。

## 当前事实与判断

### 1. capability 存活与对象语义没有统一交接

- `os/kernel/src/task/handle.rs::CapabilityRef` 在 new/Clone/Drop 中 downcast `ThreadControl`；计数保存在 `task/thread.rs::ThreadControl::capabilities`，归零调用 `abandon_created`。公共 capability 层依赖具体线程类型，是已确认的知识归属问题。
- 生产路径未发现使用 `Arc::strong_count` 判断最后 capability；相关命中在自检。当前也没有多处完全等价的 capability 计数，不能把重构描述为简单合并重复计数。
- Builder 与 Invitation 是 affine authority，分别在 `close_handle/close_transit` 中 abort/abandon；成功 Start/Attach 则消费其语义后释放 entry。Mailbox/Notification 的唯一 owner 关闭不等待其他 sender/signaler 消散。
- `KernelObject::close_handle(role, owner, exiting)` 与 `close_transit(role)` 把同一释放责任按存放位置拆开；`handle.rs` 又对 Tunnel、RetirementTarget 分流。仅把回调搬入 Drop 不能闭合这些差异。
- `mailbox.rs::send` 成功消费 SendOnce 时可在 HandleTable 锁内析构 entry；现有角色不触发复杂收尾。通用归零后若同步执行跨对象动作，会改变原有锁序。事件产生与动作执行必须有明确的锁外交接。

### 2. Sender 的公开 Lifetime 依赖普通对象保活

`mailbox.rs::MailboxSender` 内嵌 `LifetimeOwner`；`lifetime.rs::LifetimeOwner::drop` 发布 CLOSED。`delivery.rs::Delivery`、`wait.rs::WaitInterest` 等保存 sender 的普通 ObjectRef。因此最后 sender capability 消失时，已接受的消息与等待仍可能使 Lifetime 存活。

已有行为包含必要的使用责任：消息排队、接收和服务处理期间不能因调用者关闭发送权就丢失上下文。问题是这些责任与临时内核 Arc 使用同一表示；直接改成 capability 数量归零即 CLOSED 会破坏 Delivery 语义。

### 3. Tunnel 的合法关闭仍依赖临时可失败资源

`task/tunnel.rs::Endpoint::new` 预付 LeaseRetire，但 `close_handle` 仍执行 `plan_object_unmap`、`fund_table_preflights`、`prepare_memory_completion` 和 `prepare_shootdown`；内存、完成容量、远端槽及执行快照竞争都可能使关闭在提交前失败。`user/libraries/libkernel/src/ipc/tunnel.rs` 因而需要返还 owner 重试，Drop 失败则留下 entry/映射供进程 Drain。

当前 Endpoint 与本地映射 lease 的绑定同时提供安全语义：显式 Close 成功覆盖映射撤销与同步完成；`LeaseRetire::finish` 在完整 lease 退役后才发布对端通知。进程退出的 detached close 则在 REAPABLE、用户已不能继续访问后逻辑关闭，再由地址空间 Drain 释放存储。这两种物理回收路径的差异有依据。

### 4. 不可逆终止后的完整回收仍依赖监督者继续调用

`task/process.rs::run_termination_todo` 自动推进到 REAPABLE；`task/proc.rs::publish_reapable` 只通知依赖和 Control。HandleTable、地址空间与 backing 的回收由后续 ProcessDrain 批次驱动；Finalization 强根在 `drain_batch` 的 PublishDead 阶段才发布。

因此停止调用 Drain 会停住此前的回收。Job 成员表保活及 JobDerive 允许另一个管理者接管，但不保证接管发生。当前文档明确承诺“管理者驱动”，所以这不是实现漏接，而是本轮建议重新选择的公开责任边界。

### 5. 现有区分背后的需求，不冻结其表示

| 区分 | 当前独立理由 |
|---|---|
| role 与 rights | 普通 sender 去掉 DUPLICATE 仍可重复发送；SendOnce 还包含成功消费语义，不能由权限位推导 |
| TRANSIT 与 GRANT | 有缓冲容器滞留/丢弃与 Building 直接交付具有不同对象图；允许 owner 直接交付不意味着允许 owner 进入消息队列 |
| capability 与内核 Arc | 临时内核引用保内存安全，不能自动算作可行使 authority |
| 最后 capability 与最后使用 | Delivery/已接受操作可能仍需完成；对象业务寿命不必随最后外部授权结束 |
| Handle 关闭与 mapping 释放 | MemoryObject 的既有 view 独立保活 backing，关闭 Handle 不撤销其他合法使用 |
| Control 关闭与进程终止 | 释放观察/管理能力不是 Kill；终态壳也可以在资源回收后继续存在 |

这些事实只说明权限、寿命、运输和观察存在需要表达的差异，不证明当前 role、对象拆分或计数方案就是正确目标。本轮没有完成其余领域的架构审查；具体表示由后续设计重新选择。

## 外部参照与证据边界

本轮辅助研究核实 XNU 与 seL4 官方固定版本源码；决策依据仍是上述本项目责任链，不照搬其线程、抢占或内存管理模型。

- XNU `xnu-12377.121.6`：[`ipc_port.h`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/osfmk/ipc/ipc_port.h) / [`ipc_object.h`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/osfmk/ipc/ipc_object.h) 分开对象引用、send/send-once rights 和 make-send 计数。它证明授权事件与存储寿命可以明确分开，不意味着 Halcyon 应复制这些具体计数。
- 同版本 [`ipc_port.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/osfmk/ipc/ipc_port.c) 的 `ipc_port_release_send_and_unlock` 在锁内准备 no-senders 通知，解锁后发送；[`ipc_kmsg.c`](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/osfmk/ipc/ipc_kmsg.c) 通过 delayed destruction queue 限制消息销毁级联递归，reap 要求不持锁。该证据支持“归零交接”和“执行收尾”分开，不把它的每线程队列照搬到协作式内核。
- seL4 `16.0.0`：[`cnode.c`](https://github.com/seL4/seL4/blob/16.0.0/src/object/cnode.c) 用 MDB 关系判断最后 capability，Delete/Revoke 经 finalise/zombie 分步消解；[`untyped.c`](https://github.com/seL4/seL4/blob/16.0.0/src/object/untyped.c) 在无子对象时重置 untyped 的分配位置。显式资源管理属于完整模型选择，不能单独借来证明已有自动准入/收费内核必须让监督者反复执行 Drain。
- Zircon 进程终止沿用仓库[固定版本取证](ref-2026-08-task-termination-research.md#五zircon-源码级终止路径)，commit `7dedc3f2bbbba618ff0f1cda6d9e67cbf3e6f98a`：最后线程移出成员后进入 `FinishDeadTransition`，销毁地址空间，再发布终态并移出 Job。官方 [task_kill](https://fuchsia.dev/reference/syscalls/task_kill) / [Process](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/process) 契约允许持有终态 Handle 查询结果。本轮 googlesource 不可达，未重新核实 Dispatcher 双计数、Channel 内运输和 Process HandleTable 清空位置，不把搜索摘录当源码证据。

这些参照支持的是责任分层及回收归属的选择空间，不构成本项目新设计的正确性证明。

## 推荐目标结构

### A. capability 层拥有引用机制，类型拥有归零语义

- 铸造、受权复制、移动、pin、安装、消费、关闭和失败退回使用同一条 owner 责任链。移动/退回/pin 不制造减一再加一的零窗口；复制新增一份引用；最终放弃只释放一次。
- 通用层不认识 ThreadControl 等具体类型；类型根据自身语义决定零事件是否放弃 Created、结束授权或无动作。affine owner 的关闭不能等待不相关 sender/signaler 全部消失。
- 明确未发布准备、成功消费和放弃的区别，避免构造回滚误作已发布对象关闭，也避免成功 Attach 后重复 abandon Invitation。
- 归零事实在线性化点确定，跨对象动作在锁外交接；需要延后执行时持稳定 owner、预付容量及事件身份，不把任意析构点当可出游位置。
- 零引用不统一等同永久终态。ProcessControl 等可由另一合法创建/派生 authority 重新取得；铸造资格由对象操作决定，不能从内部 Arc 随意复活 authority。
- 具体计数承载类型、事件 token 与类型回调签名在实现设计中闭合；不预先要求所有对象增加同形计数或一套通用对象状态机。

### B. 将业务使用责任从普通存储保活中分离

从现有 Sender → 排队消息 → Delivery → 服务处理链提取独立需求，再设计授权持有、已接受使用、存储保活及终态观察的关系。需要保留的是在途责任不会因发送者消失而丢失；不预设必须保留同名的 Sender/Delivery/Lifetime 对象组合。

等待需逐入口规定哪些属于必须延长使用的操作、哪些只是终态观察；不能由一个 `_authority: ObjectRef` 偶然决定公开 Lifetime。ThreadControl 的 WAIT-only capability 是否应维持 Created 寿命、启动权与观察权是否独立，是待重设计项，不能以旧计数已通过验收为理由直接保留。

不把 mapping、线程执行容器和所有对象都塞进一套使用计数；各自已有 owner 可保持独立，只需与公共引用事件形成准确边界。

### C. Close 的已确认目标

- 合法 Close 只消费这份本地持有，返回前完成失效与责任交接；没有待后续调用才能成立的半关闭。类型若因此结束新操作准入，在对应线性化点完成。
- 普通 Drop 可使用 Close，不等待对端、完整退休或资源退款，不因清理容量不足/暂时繁忙返还 owner 重试。原始非法 Handle 仍按 ABI 拒绝，不把调用错误混作合法释放失败。
- 扩大资源或清理负担时先准入实际承载；队列忙只影响后续推进，不能使已成立释放责任无人接管。内部执行由[维护计划](todo-2026-09-29-kernel-maintenance-execution.md)拥有。
- Close 不证明共享对象销毁、地址翻译已撤销或业务成功。明确需要访问撤销屏障的 Unmap、业务 Finish/Flush 与关闭分开；资源在安全退休前不复用、不退款。
- 不为每次 Close 铸造新的用户可见完成对象。已有对象的完成状态或确有独立用途的操作结果承担观察；观察者存在与否不影响退休活性。
- 原 Endpoint 同时拥有通信与映射、公共 HandleRole、Lifetime/Delivery 的对象边界均须重新论证；不能仅给旧 Close 补足等待资源然后宣称完成新语义。

## 迁移范围与删除门

1. 固定目标对象/authority/使用关系，再确定类型、入口、计数和消费动作；不是先设计一个万能 hook 覆盖全部旧行为。
2. 公共层统一铸造、复制、移动、消费、释放及回滚，删除 ThreadControl downcast 与按对象种类分流的 Close。
3. 与维护执行、映射及等待分案一起接通必成且不隐式停驻的释放；删除用户态 EndpointCleanup/Close 失败 owner 重试及仅为机械注销存在的重试。
4. 迁移 shared ABI、libkernel、线程/进程封装、IPC/RPC/Runnel 与实际服务消费者。用户态真实的业务取消、结果收集、共享引用和账目不得作为“清理代码”一并误删。
5. [进程自动回收](todo-2026-09-29-process-automatic-reclamation.md)拥有 ProcessDrain 删除与监督消费者迁移；本案提供它消费的同一释放契约。
6. 回填当前 impls 并删除旧路径；不保留为了兼容旧 owner/Close 语义的双轨。

## 实施前设计门

- role 是否仍为独立表项维度、哪些关系应成为独立对象、哪些仅属于运输/消费语义；逐项独立论证。
- 零引用、显式 Close、成功消费、未发布回滚的语义矩阵；重新铸造资格、并发复制/关闭/Start 的线性化点和锁序。
- 业务使用与终态观察的持有规则；Sender/Delivery/Lifetime 只是现状起点，不要求同名保留。
- 内存映射 owner 的本地释放与显式 Unmap 屏障、可复用地址和 backing 的归还关系；完成之前不能允许不安全复用。
- 所有消费者的返回类型、raw/safe 边界和取消行为；只有上述关系形成成型方案后才开始实施。

## 验证门

验证依据是目标契约和以下独立反例，优先复用现有 handle_table、IPC/retirement/生命周期自检及 QEMU 消费者，不另造测试运行体。

- capability 经受权复制、移动、运输丢弃、接收回滚、直接交付、pin 和进程回收只释放一次；移动不出现伪零窗口。Created 的启动、放弃与观察依据新设计的独立语义验证，不把旧 WAIT-only 计数写成先验预期。
- 最后 sender capability 释放而 Delivery 尚在时 Lifetime 不提前 CLOSED；临时内核 Arc 不改变业务终点；等待取消、来源注销及观察者关闭不留下自保活环。
- SendOnce/Invitation 成功消费与放弃分别执行正确语义；通用归零不从表锁内触发未经允许的跨对象清理。
- 真实内存/metadata/远端执行容量紧张时，合法关闭仍被内核接管；关闭回复取消或调用者被杀后继续完成；映射、TLB 确认、对端信号与退款顺序正确。
- 目标不可逆终止后全部监督者停止调用或先退出，目标仍释放自己的全部资源；另一个进程仍持有的共享对象不被误收；资源归还原账户，多个回收目标均取得进度。
- 对外完成信号、终态 Query 和 Job 完成具有一致边界；未发布构造失败与正常退出复用同一收束机制。

施工验证遵循 [构建手册](BUILD-AND-TEST.md)：相关 host 自检显式 host target；责任链接通后 `just check`、`just clippy`、必要跨 hart/平台/退出组合及 `just acceptance`。当前文档轮不运行代码测试，不提供实现验收证据。

## 当前接力

已确认消除公共资源特判、Close 必成且不等待完整退休。上一轮保留同步 Close/Tunnel 原组合的建议已被全项目重审取代；源码盘点和外部事实继续作为现状证据。接下来闭合目标对象关系及上述设计门，与维护执行和进程自动回收分案共同施工。本案没有代码实现或验收完成宣告。
