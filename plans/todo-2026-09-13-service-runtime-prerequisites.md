# 用户态运输、RPC 与服务执行前置

> 状态：[公共对象/观察/退休 #13](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md) 已完成；[时间/绝对期限 #14](archived/todo-2026-09-monotonic-time-rpc-deadline.md) 已完成；本任务进入施工前审视。当前 rinlib/Runnel/RPC/libsrv 源码均需按目标重新核对，不视为已完成框架。本文件拥有用户态运输与通用执行机制施工，FAL 业务由 [总计划](todo-2026-09-fal-service-capabilities.md) 在本任务完成后恢复。

## 闭合目标与任务边界

公共内核保证短路径、持久观察和必成对象退休；用户态执行基座保证业务准入、任务驱动、背压、取消、下游调用及正常清理。业务状态不能倒灌入内核，内核来源状态也不能由用户手工维护。

本任务把 Packet/Delivery → Request/Task/Outbox → terminal → retire → refund 接成完整责任链。运输初始化、运行时状态、RPC 阶段和服务公平调度互相约束，必须共同迁移；不先写 FAL handler，再遇到失败时补基础 adapter。

## 规模、阶段与提交策略

#15 是当前主线中继 #14 之后最大的机制任务。现有代码已经有若干类型草稿，但不能按源码行数或“已有框架”计入完成度；真正工作在于把所有权、阶段错误、取消、背压、期限、退休和退款接成闭合责任链，并迁移真实调用者。

预计按以下五个机制闭包推进，每个闭包可独立构建、验证和提交，但前一闭包未完成时不宣称 #15 完成：

1. **Typed transport**：审计 `Capability/HandleSet/Packet/ReceiveBuffer/Delivery`，完成 Runnel Producer/Consumer 的构造、Attach、部分进度、EOF/Broken、初始化失败和 Endpoint cleanup；迁移 pm/init 的实际工厂，保留 raw ABI 仅作明确 unsafe 验收边界。
2. **RPC context**：统一同步/异步 RequestContext、预付回复、txid/Deadline、`Unsent`/`Sent`/`OutcomeUnknown`、关闭和取消；验证协议拒绝、迟到回复、附带能力退休与请求 owner 原样返还。
3. **Outbox 与服务运行体**：在业务提交前准备回复存储、Delivery 和发送额度；接通 `libsrv` Budget/Account/Charge、WorkQueue、Runtime、Wake，验证公平轮转、`max_work=1`、任务期限、背压和显式退休。
4. **真实消费者迁移**：迁移 `srv_init`、`srv_pm`、`srv_fs`、`test_hammer` 及现有 Runnel/RPC 调用点；删除旧阻塞泵、重复 close、相对期限重试和没有真实 owner 的 adapter。
5. **组合收口**：host/static/clippy、core/release/platform、初始化失败、取消、调用者退出、Sent 后超时、迟到回复、Endpoint 关闭、资源退款和旧路径删除一起验证；完成后才解除 FAL 业务前置。

提交纪律：不按文件机械拆分强耦合 ABI/owner/通知/退休迁移；每个提交说明所闭合的机制、真实调用者、尚未覆盖的失败面和验证证据。跨阶段暂存的类型只能是最终结构的一部分，不能用兼容层或测试专用路径替代未完成责任。最终 #15 交付提交前，再登记固定 hash 的提交后 Review；不自动合并或 push。

## 从最终调用反推的前置

- 安全发送失败返还完整未消费 Packet；成功消费 moves/send-once，借用的 owner 不被 raw helper 静默消费。
- 接收后 prepaid storage 接管业务能力和 Delivery；协议失败、容量失败、过期与取消均由同一 owning 上下文收束。
- Runnel Invitation 失败区分未消费邀请与已消费 endpoint；协议初始化失败返还未建立角色的 transport。运行期 Broken 是另一 terminal，保留部分进度和清理责任。
- 一个 actor 公平处理输入、到期政策、业务 step、下游 RPC 与退休。max_work=1 仍保留轮转位置，不用业务请求偶然唤醒退休。
- RPC Unsent 返回请求；Sent 超时/关闭报告结果未知，不能自动重试。所有阶段使用同一 Deadline，最终接受回复再次核验；晚到能力统一退休。
- Outbox 在业务 commit 前准备回复存储/队列额度；背压和发送失败保留 reply-once/Delivery。业务成功不等于回复成功，运输完成与业务结果明确分开。
- 来源 token/generation 对应稳定任务 ID；任务完成后的批次旧记录被丢弃，不复活已退休任务。
- 准入按服务/授权域资助并计实际责任；正常 release 唤醒 actor，长清理有硬工作界限，真实退休后退款。
- 普通对象 close 可挂起；运行体必须先停止业务准入、完成/取消任务、退役 RPC/outbox/运输，再关闭 WaitSet 与唤醒源。用户业务停止不能用 WaitSet 的内核 CLOSED 代替。

## 自底向上施工顺序（同一闭合任务）

1. 审计 rinlib Capability/HandleSet/Packet/ReceiveBuffer/Delivery 所有权、初始化失败与 raw unsafe 边界；完成已有真实消费者迁移，无双重 close 或“Drop 后伪造成功”。
2. 完成 Runnel Producer/Consumer 构造、非阻塞运行、观察准备、部分进度、EOF/Broken 和 endpoint cleanup；迁移 pm/init 及生产工厂，原始接口仅保留明确的 ABI 验收用途。
3. 统一同步/异步 RPC 的 RequestContext、预付响应、Unsent/Sent、截止接受和 shutdown；实现 Outbox 作为正式任务驱动而非另一个阻塞泵。
4. 接通 libsrv WorkQueue/Runtime 的公平 step、预付任务/事件/期限、Wake 与显式退休；利用既有真实服务请求/数据责任连接运行体，不引入只用于制造前置通过的临时服务或兼容泵。
5. 迁移实际调用者，删除旧运输/阻塞推进/维护编排；验证原有服务正常行为及满箱、取消、迟到回复、初始化失败、静默退出、最后退款。随后才准许 FAL provider/domain/业务施工。

## 代码与删除连接点

rinlib ipc/{capability,message,packet,invitation,tunnel,wait_set}.rs、librunnel、librpc/{caller,exchange,dispatcher}.rs、libsrv/{budget,work_queue,runtime,wake}.rs 及正式 Outbox；真实消费者包括 srv_pm、srv_init、srv_fs 既有运输责任与 test_hammer。既有 FAL grant 只迁移其基础 API 使用，不据此扩展业务；store/backend/value/protocol 等草稿待本任务完成后重新论证。

公共对象已删除 WaitSet Seal/Drain 编排；执行任务须确认消费者不恢复它。必须删除：安全 API 中的 raw 消费捷径、重复 close、初始化失败自动执行运行时 Broken、业务提交后才分配回复或 timer、轮询退休/偶然业务唤醒、无真实调用者的重复 runtime/adapter。不能删除具有独立业务语义的 Task/Runtime seal、授权准入停止或协议终因。

## 完成门

全部运输/RPC/Outbox/Runtime 的正常、失败、取消和退役路径实际连接，拥有者与阶段错误完整，公平与 deadline 政策可验证；当前真实消费者使用最终路径，旧机制删除，host/静态/服务组合验证证据可定位。该任务完成不意味着 FAL 交付，但为后续 provider/domain 施工提供已成立的能力，而不是待业务补救的结构草稿。
