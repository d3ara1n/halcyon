# Capability 与 Affine Owner 错误边界收口计划

> 状态：rights、用户态 owner、RPC 接收与 ticket 查询均已按各自完整调用链收口，本实施计划现已归档；提交后复核由 [`Review program`](todo-2026-09-review-program.md) 统筹。它们共享错误原则，但不共用一个事务类型。

## 目标

所有可转移/消费式资源必须有闭合的 authority 与错误策略：

- capability rights 能表达对象允许的每种操作，且 shared/kernel/rinlib 纵向一致；
- Handle/Transit/Grant/ReplyPort 的失败路径不泄漏、不误安装、不污染下一次操作；
- affine owner 的显式 close 与自然 Drop 不采用相互矛盾的错误语义；
- 已消费 ticket、已拒绝消息、已撤销资源都有可观察且不 panic 的终态；
- 只有具有结构性“不可失败”证明的析构路径才能使用断言，普通用户可达错误不能升级为 kernel/user panic。

## 当前问题簇

### Capability / ABI（已完成）

`Rights::EXECUTE = 1 << 10` 已进入 shared KNOWN mask，MemoryObject 完整 rights 与 RX required rights 统一为独立授权；ThreadControl 只公开真实可达的 DONE。通用 Duplicate/Transit/Grant 继续使用 role allowed-rights 与源 rights 双重子集校验，不为 EXECUTE 建旁路。shared 固定位测试与 QEMU capability 矩阵覆盖 Mutable 拒绝 RX、Seal 后缺 EXECUTE 的派生副本 `RightsDenied`、具 `MAP|READ|EXECUTE` 的副本在原 Handle 关闭后成功 Map/Unmap。

### Affine owner Drop（已完成）

MemoryPool/MemoryObject typed owner 的 unsafe 构造契约现要求当前进程已安装的对应 leaf role 与唯一 raw owner；安全创建/派生保持该不变量。它们的显式 close 和 Drop 共用不可失败 leaf-close，不再暴露虚假的可重试分支。UserStack 仍按异步 mapping 分类：Busy 按 tick 重试，终端错误把仍由 AddressSpace 账本拥有的 mapping 留给 ProcessDrain，并以饱和 count/last-error 快照记录，不在 Drop 中 panic 或无限等待。

### 消费式 ticket / query（已完成）

`SystemSupply` 已分离不可变 Range 快照与 `Option<Ticket>` owner；`heap_ranges`/`recovery_ranges` 在任意合法消费顺序后仍返回原规划，`consumed_*`/`remaining_*` 分别观察状态，查询不能重新取得或伪造 affine ticket。

### RPC reject / ReplyPort（已完成）

`validate_response` 是 host 可测的 framing 分类入口；Caller 对 ServiceClosed、wait/receive error、timeout 和所有 framing reject 均废弃当前 ReplyPort。已接收但未接受的 response 先逐项关闭 Handle 再 discard；可 TRANSIT role 不含 Tunnel Endpoint，cleanup 是固定上界叶 close。`srv_init` 双调用验证 malformed response 携带 Handle 被关闭，下一调用在新端口正常成功。

## 最终形态

### Rights 与 signal 的单一真值

1. 在 shared ABI 增加独立 `EXECUTE` 位，更新 KNOWN mask、布局/版本断言；
2. kernel MemoryObject allowed rights、RX required rights、Seal/Executable 状态校验统一使用 `MAP|READ|EXECUTE`；
3. rinlib/public map API 与 Handle derive/grant 保持 rights 子集语义；
4. ThreadControl 只暴露真实可达的终态 signal（首选移除 CLOSED，保留 DONE）；若未来需要 CLOSED，必须先定义独立状态机和发布顺序；
5. 以 capability 矩阵测试覆盖 Create/Derive/Grant/Transit/Map/Protect/Seal/Wait/Unmap。

### Affine owner 错误策略（已实现）

按 owner 类型冻结策略，不以统一 `Drop` 掩盖不同语义：

| owner 类别 | 显式 close | Drop | 失败后策略 |
|---|---|---|---|
| 固定容量、内核 close 可证明无错的叶 owner | 不可失败 close | 同一 leaf-close | 仅 unsafe 构造或内核不变量破坏 |
| 用户可达、close 可能 Busy/Closed 的 owner | 返回 `(owner,error)` | 不执行可能失败 syscall；记录/留给上层 owner | 显式 retry 或 supervisor 接管 |
| 已接收但尚未接受的 capability | 不进入业务 owner | 必须 discard/close | reject helper 统一收束 |
| 需要异步确认的 mapping/stack owner | 显式 wait/retry | 不在 Drop 中无限等待 | Join/reaper policy |

最终选择：MemoryPool/MemoryObject role 在内核中是非 Tunnel 叶 close，typed owner 的 unsafe 构造契约保证表项有效且唯一；显式 close 与 Drop 统一走不可失败 leaf primitive。UserStack 属异步 mapping owner，Busy 重试，终端清理失败记录后留给进程 AddressSpace drain，不沿用 leaf 策略。

### SystemSupply consumed-state（已实现）

- ticket 的不可变几何与消费状态分离：保留 `Range` snapshot，查询只读快照不访问已消费 owner；
- `remaining_*`、`*_ranges` 对任意合法消费顺序均不 panic；
- ticket owner 仍只能单向消费，不能通过 query 重新取得或伪造资源；
- recovery/heap 的容量与用途隔离继续由类型保证。

### RPC reject guard（已实现）

统一 `reject_reply` 机制：

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

## 纵向子单元与依赖

以下按消费边界组织；每个子单元同时迁移生产者、消费者、失败路径、测试和旧入口。精确 owner 错误政策需在对应子单元开始前确认，不把本计划的候选建议当成已经批准的 ABI 改动。

1. **已完成**：冻结 Rights/signal ABI 变更与 capability 矩阵；
2. **已完成**：增加 `EXECUTE` 并迁移 shared/kernel/rinlib 调用链；
3. **已完成**：收口 ThreadControl signal 集合；
4. **已完成**：冻结 affine owner 错误策略，迁移 MemoryPool/MemoryObject/UserStack；
5. **已完成**：重构 SystemSupply 几何查询与 consumed-state；
6. **已完成**：引入 librpc reject helper，统一 Handle/ReplyPort discard；
7. **已完成**：补 host、shared ABI、跨进程 rights、Seal/RX、reject 携带 Handle、迟到响应隔离和 owner 正常收束测试；
8. **已完成**：与 Mapping/retire/Join 语义联测，core guest 强制检查 RPC cleanup 与零 abandoned stack 锚点。

## 完成标准

- RX capability 必须独立持有 EXECUTE，缺任一 MAP/READ/EXECUTE 均拒绝；跨进程 rights 裁剪不放大；
- ThreadControl allowed signals 与实际发布集合完全一致；
- 任一 affine owner 的显式 close、Drop、abandon 路径都有文档化错误策略，无用户可达 panic 作为正常失败语义；
- ticket 消费后 query/range accessor 不 panic，剩余/已消费状态可观察；
- RPC 任一 reject 都关闭收到的 Handle、废弃旧 ReplyPort，下一次调用不会复用污染端口；
- shared/kernel/rinlib ABI、notes 与测试一致，旧 rights/fallback/helper 全部删除；
- 运行 host debug/release、`just check`、virt/release/stress/acceptance 及专项故障注入。

## 依赖与边界

- MemoryChange/retire 的最终 owner 类型由 `todo-2026-09-memory-transaction-state-machine.md` 冻结；本计划不得提前引入另一套 mapping owner。EXECUTE 的 shared/kernel/rinlib 纵向子单元是完整 RX authority 验收的前置，按接口需要先行，不必等待本计划的 RPC/Drop 等无关子单元。
- Admission 的未知输入拒绝由 `todo-2026-09-admission-fail-closed.md` 负责；本计划只处理对象 capability 与消费路径。
- 监督失败接管由 `todo-2026-09-supervision-authority-policy.md` 负责；owner 错误只提供可接管的显式状态，不自行实现 supervisor。
