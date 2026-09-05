# Capability 与 Affine Owner 错误边界收口计划

> 当前 Review findings 的机制归并计划。目标是统一 capability 语义、消费式 owner 的生命周期和错误处理边界；不把 EXECUTE、Drop、RPC reject、ticket query 拆成孤立补丁。

## 目标

所有可转移/消费式资源必须有闭合的 authority 与错误策略：

- capability rights 能表达对象允许的每种操作，且 shared/kernel/rinlib 纵向一致；
- Handle/Transit/Grant/ReplyPort 的失败路径不泄漏、不误安装、不污染下一次操作；
- affine owner 的显式 close 与自然 Drop 不采用相互矛盾的错误语义；
- 已消费 ticket、已拒绝消息、已撤销资源都有可观察且不 panic 的终态；
- 只有具有结构性“不可失败”证明的析构路径才能使用断言，普通用户可达错误不能升级为 kernel/user panic。

## 当前问题簇

### Capability / ABI

- `shared::object::Rights` 缺少独立 `EXECUTE` 位；MemoryObject allowed rights 与 RX map required rights 也缺少该位。
- `ThreadControl::allowed_signals` 暴露 `CLOSED`，但 ThreadControl 只发布持续 `DONE`；允许等待集合与可达状态不闭合。
- MemoryObject Seal/EXECUTABLE、RX view、跨进程 rights 裁剪、Transit/Grant 组合尚未有完整纵向矩阵。

### Affine owner Drop

- `user/rinlib::{memory_pool,memory_object}.rs` 的 Drop 对 close 失败直接 `expect`；显式 `close(self)` 却返回可重试 owner，错误边界矛盾。
- `user/rinlib/thread.rs::UserStack::release` 对非 `ObjectBusy` 错误直接 panic；没有统一的用户态终止/泄漏报告政策。
- owner close、HandleTable close callback、AddressSpace retire、对象 backing 归还之间没有统一“显式 close / Drop / abandoned”策略表。

### 消费式 ticket / query

- `SystemSupply::take_heap_chunk` 与 `take_recovery_ticket` 消费 slot 后，`heap_ranges`/`recovery_ranges` 仍对已消费 slot `expect`。
- affine ticket 的只读几何、剩余状态和已消费状态没有独立快照，公共 accessor 对合法生命周期顺序不安全。

### RPC reject / ReplyPort

- `librpc::Caller::call` 对 ServiceClosed、ProtocolMismatch、UnknownVersion、NotResponse、TxidMismatch 直接返回。
- 已收到但未接受的 Handle 没有逐项 close；当前 ReplyPort 没有统一 discard，迟到响应可能污染下一次 call。
- 正常 timeout/wait/receive error 已有 discard，但 framing reject 没有复用同一策略。

## 最终形态

### Rights 与 signal 的单一真值

1. 在 shared ABI 增加独立 `EXECUTE` 位，更新 KNOWN mask、布局/版本断言；
2. kernel MemoryObject allowed rights、RX required rights、Seal/Executable 状态校验统一使用 `MAP|READ|EXECUTE`；
3. rinlib/public map API 与 Handle derive/grant 保持 rights 子集语义；
4. ThreadControl 只暴露真实可达的终态 signal（首选移除 CLOSED，保留 DONE）；若未来需要 CLOSED，必须先定义独立状态机和发布顺序；
5. 以 capability 矩阵测试覆盖 Create/Derive/Grant/Transit/Map/Protect/Seal/Wait/Unmap。

### Affine owner 错误策略

先按 owner 类型冻结策略，不以统一 `Drop` 掩盖不同语义：

| owner 类别 | 显式 close | Drop | 失败后策略 |
|---|---|---|---|
| 固定容量、内核 close 可证明无错的叶 owner | 可返回错误但应建立不变量证明 | 允许断言式 close，证明写入 API 契约 | 仅内部不变量破坏 |
| 用户可达、close 可能 Busy/Closed 的 owner | 返回 `(owner,error)` | 不执行可能失败 syscall；记录/泄漏政策由上层接管 | 显式 retry 或 supervisor 接管 |
| 已接收但尚未接受的 capability | 不进入业务 owner | 必须 discard/close | reject helper 统一收束 |
| 需要异步确认的 mapping/stack owner | 显式 wait/retry | 不在 Drop 中无限等待 | Join/reaper policy |

首版建议：MemoryPool/MemoryObject 的 typed owner 不再在 Drop 中调用可失败 close；引入显式 `close`/`discard` 责任转移或可证明的 infallible kernel close primitive，二者必须择一并贯穿所有调用点。UserStack 与其它 affine region 遵守同一策略，不允许按错误类型散落 panic。

### SystemSupply consumed-state

- ticket 的不可变几何与消费状态分离：保留 `Range` snapshot，查询只读快照不访问已消费 owner；
- `remaining_*`、`*_ranges` 对任意合法消费顺序均不 panic；
- ticket owner 仍只能单向消费，不能通过 query 重新取得或伪造资源；
- recovery/heap 的容量与用途隔离继续由类型保证。

### RPC reject guard

引入统一 `RejectedReply`/`reject_reply` 机制：

```text
receive
  → validate framing
  → accept Reply OR reject_reply

reject_reply:
  close/discard every received Handle
  discard current ReplyPort
  return typed CallError
```

以下路径必须统一进入 helper：ServiceClosed、所有 framing/kind/txid mismatch、Receive 后 payload decode/size 错误以及 future response validation。Helper 只做固定上界的 Handle close 与 port discard，不阻塞、不重试、不依赖服务端。

## 自然实施顺序

1. 冻结 Rights/signal ABI 变更与 capability 矩阵；
2. 增加 `EXECUTE` 并迁移 shared/kernel/rinlib 调用链；
3. 收口 ThreadControl signal 集合；
4. 冻结 affine owner 错误策略，迁移 MemoryPool/MemoryObject/UserStack；
5. 重构 SystemSupply 几何查询与 consumed-state；
6. 引入 librpc reject helper，统一 Handle/ReplyPort discard；
7. 补 host、shared ABI、跨进程 rights、Seal/RX、reject 携带 Handle、ServiceClosed/timeout/迟到响应和 owner failure 测试；
8. 与事务状态机完成后的 Mapping/retire/Join 语义联测。

## 完成标准

- RX capability 必须独立持有 EXECUTE，缺任一 MAP/READ/EXECUTE 均拒绝；跨进程 rights 裁剪不放大；
- ThreadControl allowed signals 与实际发布集合完全一致；
- 任一 affine owner 的显式 close、Drop、abandon 路径都有文档化错误策略，无用户可达 panic 作为正常失败语义；
- ticket 消费后 query/range accessor 不 panic，剩余/已消费状态可观察；
- RPC 任一 reject 都关闭收到的 Handle、废弃旧 ReplyPort，下一次调用不会复用污染端口；
- shared/kernel/rinlib ABI、notes 与测试一致，旧 rights/fallback/helper 全部删除；
- 运行 host debug/release、`just check`、virt/release/stress/acceptance 及专项故障注入。

## 依赖与边界

- MemoryChange/retire 的最终 owner 类型由 `todo-2026-09-memory-transaction-state-machine.md` 冻结；本计划不得提前引入另一套 mapping owner。
- Admission 的未知输入拒绝由 `todo-2026-09-admission-fail-closed.md` 负责；本计划只处理对象 capability 与消费路径。
- 监督失败接管由 `todo-2026-09-supervision-authority-policy.md` 负责；owner 错误只提供可接管的显式状态，不自行实现 supervisor。
