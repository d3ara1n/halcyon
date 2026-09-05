# Token、Generation 与 Epoch 身份边界收口计划

> 当前 Review findings 的机制归并计划。只处理各类 token/generation/epoch 的身份域、回绕与耗尽语义；不重复事务状态机、启动 admission 或对象 capability 的 owner 真值。

## 目标

所有可验证凭据必须同时满足：

- 绑定其产生的实例/容器/地址空间身份；
- 不因槽位、代次或计数回绕而重新解释为仍存凭据；
- 错误实例、错误阶段、错误 owner 的 token 只能失败闭合，不能静默命中其它对象；
- 耗尽时在状态污染前明确拒绝或永久退休；
- token 对外暴露的字段只作诊断，真正校验由不可伪造的内部身份完成。

## 当前问题簇

### Remote Call token

`os/remote_call` 的 `Reservation`/`FinishToken` 只携带 `(target, slot, generation)`；`RemoteCalls::new` 为 public，`entry_mut` 不校验 token 所属表实例。当前内核只有一个全局表，实际跨表误用暂不可达，但纯逻辑 API 没有结构性防误用。

### Handle/Job/Ready reservation token

内核多个容器各自使用全局 `AtomicU64`：Handle transaction、Job member、Ready batch、deferred work。多数只拒绝零值，回绕后可能重新使用仍存 token。容器内部当前依靠锁内短事务和 `expect` 证明错配不可达，但没有统一耗尽策略。

### AddressSpace epoch

`publish_epochs` 使用 `fetch_add` 后再 `checked_add`；达到 `u64::MAX` 时原子已环回为零，随后才 panic。现实中不可达，但违反“状态污染前拒绝”的边界原则。

### Hart raw-id / slot identity

raw HartId 与内部 HartSlot 已基本分离，旧 IPI shift 已修复；剩余 duplicate/sort/admission 问题由 `todo-2026-09-admission-fail-closed.md` 负责。本计划只处理运行期凭据不把 slot/raw/epoch 混作同一身份。

## 最终形态

### Identity domain

为每个 token 容器定义明确 identity domain：

```text
RemoteTableId
HandleTable/transaction domain
Job member/child domain
Ready queue/domain
Work-debt table
AddressSpace identity + epoch
```

推荐使用不可伪造的内部实例 ID 或持有者类型参数；公共诊断字段不能单独构成授权。跨容器提交必须先验证 domain identity，再验证 slot/generation/phase。

### Generation / wrap policy

统一策略：

- generation 达到上限时，槽永久 `Retired`；
- 全局单调 token 达到上限时返回明确 `Exhausted/InternalError`，不得回绕后从 1 重用；
- epoch 达到最大值前拒绝新的 Commit，不能先写零再 panic；
- 身份/代次检查必须在修改 phase/value/epoch 前完成；
- 失败状态不污染旧 owner、Pending 请求或可观察电平。

具体容器可以选择永久退休或实例重建，但必须在同一 owner 机制中保持一致，不能每个 token 自行定义零值特例。

### RemoteCalls

在 `RemoteCalls` 构造时铸造 `TableId`，并把它嵌入 Reservation/FinishToken 的私有校验字段；`cancel/publish/finish` 先验证 TableId，再定位 slot/generation/phase。若长期冻结单实例，也必须把 `new` 收窄为 crate-private 并在实现文档中声明唯一实例不变量；推荐保留实例 identity 护栏，避免测试和未来扩展绕过。

### AddressSpace epoch

Commit 前使用不污染状态的最大值门禁：只有确认旧 epoch 小于最大可发布值时才写入新 epoch。若任一 epoch 耗尽，事务在 Commit 前返回明确错误或使 AddressSpace 进入永久不可变/退休状态；不得继续发布 PTE、Remote 请求或复用旧 epoch。

## 自然实施顺序

1. 盘点所有 token/generation/epoch 的产生点、承载结构与生命周期 owner；
2. 冻结 identity domain 与统一 wrap/exhaustion policy；
3. 先改纯逻辑 `os/remote_call` 和 `os/work_debt`，补跨实例与代次边界测试；
4. 迁移内核 Handle/Job/Ready/member/work token 的耗尽检查，删除零值特例；
5. 修正 AddressSpace epoch 发布门禁；
6. 与 admission 计划联测 raw HartId/HartSlot，确保 slot mask 只在内部解释、SBI 边界只接收 canonical raw identity；
7. 补 host debug/release、最大值模型、槽永久退休、错误实例/阶段/owner 误用和多 hart QEMU 验证；
8. 删除重复 token helper、裸三元组校验和不再拥有真值的 fallback。

## 完成标准

- 跨 RemoteCalls 实例 token、跨表 reservation、错误 phase/owner 的提交均确定性失败且不改变状态；
- 所有 generation/token/epoch 在耗尽前拒绝，或将槽永久退休，不发生 ABA 回绕；
- epoch 发布失败不会先污染原子状态；
- Handle/Job/Ready/Work-debt 的凭据共享同一耗尽语义和错误分类；
- raw HartId、HartSlot、AddressSpace identity、epoch 各自只在所属边界解释；
- host debug/release、完整启动路线和压力/故障注入证明 stale token、代次复用和 IPI/Remote 乱序不误命中；
- 更新 `notes/impls/{call,mm,task,execution-context,internals}.md`，回到 C-2/D-2/E-1 报告逐项复核。

## 依赖与边界

- MemoryChange 阶段类型与 Commit 后 owner 由 `todo-2026-09-memory-transaction-state-machine.md` 负责；本计划只定义其 token identity/exhaustion，不重造事务状态机。
- DT/hart admission 与 canonical raw identity 由 `todo-2026-09-admission-fail-closed.md` 负责；本计划不重复 duplicate/sort admission。
- Capability rights 与 Handle generation 的 ABI 语义若发生 shared 变化，必须与 `todo-2026-09-capability-owner-error-boundary.md` 同步，但 token 本身不成为用户授权凭据。
