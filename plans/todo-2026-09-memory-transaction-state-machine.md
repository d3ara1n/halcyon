# 内存事务状态机与失败闭包重构计划

> 当前实施的结构性重构。按全局类型图、所有权图与锁阶一次性迁移，不按局部编译反馈逐点打补丁，也不引入半成品兼容层。

## 目标

将 AddressSpace、Bootstrap、ELF 构造与 Tunnel 相关的内存变更统一为类型状态机，确保事务阶段和资源所有权在类型/状态边界上闭合：

```text
Validate → Reserve → Prepare → Commit → Publish → Synchronize → Retire → Complete
```

目标不是逐个修复历史 Review finding，而是消除一整类“提交后仍可失败、rollback owner 可静默丢弃、retire 临时分配、未发布 Bound 直接析构”的机制缺口。

## 当前问题簇

当前代码已经有 `MemoryChangePlan`、`PreparedMemoryChange`、`PublishedSpaceChange` 与 `RetiringSpaceChange` 的名义分层，但仍存在以下结构性缺口：

- `ReclaimedTableFrames` 仍可在含 `WritePermit` 时直接析构，调用点必须自行记住锁外归还责任；
- 同一对象的多个 retiring fragment 由调用点逐项处理，owner 去重不由批次状态表达；
- `RetiringSpaceChange` 的 `retiring_views` 在 Commit 后仍可能 `Vec::push`；
- `spawn_from_elf` 在 Bind 后失败时没有未发布 Bound 的类型化 rollback owner；
- `launch_bootstrap` 在 HandleTable commit 后仍执行可失败的 Attach、Job member、staged 与 execution 准备；
- Commit 后的发布/收束接口仍混合可恢复 `Result`、普通分配和最终不变量断言。

这些问题属于同一事务失败闭包，不按 A/B review 报告逐条打补丁。

## 最终方案（方案 B）

### 类型状态机

为每个阶段定义不可互换的状态类型；状态转换消费旧 owner 并返回新 owner。Commit 后的类型不暴露可恢复失败接口：

```text
ValidatedChange
  └─reserve→ ReservedChange
      └─prepare→ PreparedChange
          └─commit→ PublishedChange
              └─synchronize→ SynchronizedChange
                  └─retire_step→ RetiringChange
                      └─complete→ CompletedChange
```

失败只存在于 `Validate/Reserve/Prepare`，或在 Commit 前的明确复检点。Commit 之后的操作只能返回固定进度/状态，不返回业务错误。

### 资源 owner

每个 affine 资源必须在状态类型中有唯一字段归属：

- 页表 funding owner；
- anonymous/object backing；
- `WritePermit` 多重集；
- object view owner；
- UserWriteLease/result obligation；
- Handle/Job/lifecycle reservation；
- Bootstrap 的未发布 BoundAddressSpace 与 payload owner。

rollback 必须消费专用失败状态并在锁外完成所有来源归还。任何含有未消费 permit、reservation 或 capability 的状态不得存在“普通 Drop 即表示成功”的隐含路径；必要时 `Drop` 只允许在已证明为空的终态执行断言。

### Retire 批次

Retire 批次在 Commit 前根据 validated fragments 计算对象 owner 去重结果和固定容量，完成所有容器预留。Commit 后只消费已经存在的固定容量槽位或有界游标：

- 每个 `AddressSpace × ObjectId` 在一个 retire batch 至多产生一个 retiring owner；
- permit 归还只查询 batch-local owner，不回到 AddressSpace 重新猜测 owner；
- 不允许 Commit 后 `Vec::push`、`try_reserve` 或其它普通可恢复分配。

最终容器可采用固定容量数组或等价的类型化有界容器；容量必须由事务上限静态推导并有断言/测试。

### Bootstrap / ELF

Bootstrap 和普通 launcher 使用同一组状态转换，不保留第二套后置提交协议：

1. 所有线程对象、Job member、Handle、staged、execution domain、ready capacity 与 payload/backing 在 Commit 前准备完成；
2. Commit 一次性消费全部 reservation 并发布 Building/Running 所需状态；
3. Commit 后只执行不可失败的固定发布序列；
4. ELF 构造阶段使用 `UnpublishedBound` owner，成功时显式转移为进程持有，失败时经有界 rollback/drain 收束；
5. 已发布进程只能走普通 ProcessDrain，不能由 `Drop` 旁路销毁地址空间。

## 前置设计工作

实施前必须先冻结并写入 notes：

1. 完整类型图：每个阶段的输入/输出、可失败点、消费关系；
2. 完整所有权图：Pool、FramePool、AddressSpace、MemoryObject、Job、HandleTable、lifecycle 的跨锁转移；
3. 锁阶：所有 rollback、permit 归还、owner 析构的锁外边界；
4. Commit 后固定工作预算和容量推导；
5. Bootstrap 与普通 ProcessStart 的共同提交协议；
6. 未发布 Bound rollback 与已发布 ProcessDrain 的分界；
7. 失败注入模型及每个阶段的守恒断言。

不得在前置设计未完成时引入 adapter、兼容分支或第二套事务类型。

## 自然实施顺序

1. 冻结 `MemoryChange` 类型图、owner 图和 Commit 后预算；
2. 重构 `os/memory_space` 的阶段类型与 RetireBatch；
3. 重构内核 `AddressSpace`/页表 funding/permit rollback；
4. 将 Tunnel、MemoryObject、匿名 Map/Unmap/Protect 全部迁移到新状态机；
5. 引入 `UnpublishedBound` 并迁移 ELF/Bootstrap 构造；
6. 将 Bootstrap 与 ProcessStart 收敛到同一提交协议；
7. 删除旧的 `MemoryChangePlan`/`PreparedMemoryChange` 兼容入口和调用点旁路；
8. 补齐失败注入、OOM、批量 retire、owner/permit/Pool/frame/ledger 守恒验证；
9. 更新 `notes/ideas/{mm,bootstrap}.md` 与 `notes/impls/{mm,task,startup,tunnel}.md`，回到 A/B-2/E-1 Review 报告逐项复核。

## 完成标准

- Commit 后所有接口无可恢复业务错误、无普通分配、无未登记 owner；
- 任一 Prepare/Reserve/Commit 前失败注入均保持账本、PTE、Pool、FramePool、permit、Handle、Job/lifecycle 守恒；
- 同对象多 fragment、混合 RO/RW、部分 Unmap/Protect 的 retire 不重复移除 owner；
- 未发布 Bound 的 ELF/Bootstrap 失败不触发 `TableTree::Drop` assert 或内核 panic；
- Bootstrap Attach/Job/Start 的不可逆提交后不再存在 `Result` 或分配路径；
- host debug/release、`just check`、`virt`、`virt-release`、`virt-stress`、`acceptance`、`sifive_u` 及相关故障注入全部通过；
- 全仓删除旧阶段 API、单用途 adapter、重复 owner 真值和过渡注释；
- 原 Review 报告中的事务失败闭包 findings 全部标记为“机制重构闭合”并完成复核。

## 实施状态

本计划已由 Review 修复交接提升为当前主线。切片 8/9 暂不推进，先完成本计划的纵向迁移与完整复核；期间不得以“先让单个 crate 编译”为目标拆出局部兼容层。每个阶段只在整体类型/所有权/锁阶模型保持闭合的前提下推进，代码验证用于确认整体设计而非驱动设计漂移。

当前已接入的第一批结构性收口包括：

- `ReclaimedTableFrames` 的提交前失败路径统一显式消费 `WritePermit`，不再依赖调用点记忆或普通 Drop；
- 账本在 Reserve 阶段记录 retiring object owner 的去重容量，内核在 Commit 前预留 owner 容器，Retire 阶段只消费既有槽位；
- `spawn_from_elf` 以 `UnpublishedBound` 持有 Bind 后、启动发布前的地址空间，失败经有界 drain 收束，不再由 `TableTree::Drop` 旁路；
- Bootstrap 的 domain、staged 容量、Attach、Job member 与 Building operation 前移到 Handle commit 之前，提交后仅保留固定发布序列。

这些接入仍需与完整状态类型、Bootstrap/ProcessStart 共用提交协议、故障注入和全验证矩阵一起收口，未完成前不得将本计划标记为完成。
