# Remote Call 与地址空间 epoch Review 计划

> 【未来审查计划】对象是用户内存映射切片 4–5 的提交 `619998517f5293d13e48d91f3980d4f4d389b90a`；Review 纪律见 [`REVIEW.md`](REVIEW.md)。方向契约见 [`notes/ideas/call.md`](../notes/ideas/call.md) 与 [`notes/ideas/mm.md`](../notes/ideas/mm.md)，实现现状见 [`notes/impls/call.md`](../notes/impls/call.md)「Remote Call」、[`notes/impls/mm.md`](../notes/impls/mm.md)「用户地址空间」，实施上下文见 [`archived/todo-2026-09-user-memory-mapping.md`](archived/todo-2026-09-user-memory-mapping.md)。

## 提交对照

| 提交 | 内容 |
|---|---|
| `6199985` | reservation-aware TableTree 与不可失败 Publish；固定容量 `os/remote_call`；kernel Remote Call adapter、SBI IPI mask/base、SSIP/调度安全点；AddressSpace 稳定 identity/epoch；lifecycle execution sequence 与 enter/leave gate；运输及 active snapshot epoch 启动探针；实现文档与主计划同步 |

## Review 轴（代码为主）

### 固定槽状态机与 affine token

- `Empty → Reserved → Pending → Taken → Empty/Retired` 是否覆盖每个消散点；Reserve 批量失败、Prepared drop、Publish 后 Doorbell 消散和动作 panic 各自留下的唯一所有者是否可从结构推出。
- generation 在 Finish 后、复用前推进及 `u32::MAX` 永久退休是否足以拒绝 ABA；跨 `RemoteCalls` 实例误用 reservation/finish token 的安全 API 边界是否需要显式 table identity，而不能只依赖 kernel 当前只有一个全局实例。
- 每 hart 4 槽是否与 MemoryChange、HandleClose、seal 等未来最大并发事务数相容；完成回调在 ack 后且槽归还后执行是否避免嵌套提交产生虚假 Busy，同时不允许无界回调链留在内核短路径。
- `take` 的最低槽序选择与乱序目标确认是否只影响性能、不形成饥饿；单安全点最多 4 项的硬界是否在持续生产下仍给调度/用户返回留下进展。

### RVWMO、PTE 与确认链

- Commit 的 PTE store、AddressSpace epoch Release、Remote slot Publish、锁外 `FENCE RW,RW`、SBI IPI、目标 acquire/`SFENCE.VMA`/可选 `FENCE.I`、ack AcqRel RMW、最后回调之间逐边重建 happens-before，确认没有把 spinlock 实现细节当未入档的隐含屏障。
- `BatchCompletion.remaining.fetch_and(AcqRel)` 的 release sequence 是否让最后确认者 acquire 全部目标动作；目标在 ack 后先归还 slot、再调用 sink 是否保持 Retire 前置而不丢前序可见性。
- 当前 ASID 恒 0 的全量 `SFENCE.VMA` 与 identity 切换时 `FENCE.I` 是否保守完整；请求中的范围何时值得转为按 VA 失效，instruction epoch 是否可按真实可执行页发布替代调度器遗留的每次 `FENCE.I`。
- TableTree 的 Prepare/Publish 与 shootdown Commit 闭包组合后，任何页表 store 是否仍只发生在 epoch 发布之前；Protect/Unmap 的 mega leaf split 与表帧 reservation 是否存在 Publish 后才暴露的失败。

### IPI、hart admission 与安全点

- registry 的 slot 位图到 `(hart_mask=1, hart_mask_base=raw_hartid)` 转换是否完整遵循 SBI v0.2+；稀疏或大于 XLEN 的 raw hartid、invalid/unavailable hart 和部分 IPI 失败是否都只影响门铃、不伪造确认。
- admitted mask 在 Remote Call 启用前是否已冻结且所有目标都能进入安全点；未来 hart stop/start、CPU hotplug 或 admission 变化出现时，请求槽生命周期和 active snapshot 应在哪里重新线性化。
- SSIP trap 入口/出口、scheduler loop/leave 三类消费点是否足以覆盖 WFI、用户态长运行、非 Resume 和 IPI 丢失；普通调度/终止门铃是否可能清 SSIP 后让 Pending 长期无人消费。
- 启动运输探针的 `records_epoch=false` 是否是清晰的动作语义而非 sentinel 特例；probe 与 primordial active snapshot 探针的先后竞态、失败终态和日志完成锚是否值得移入可组合 action 类型。

### execution gate 与生命周期

- `(execution_sequence, active)` 快照是否拒绝 active 离开后恢复同值的 ABA；enter 的“gate 外同步、gate 内复检”和 leave 的“锁外排空、gate 内复检后清位”与 Commit 的 `ADDRESS_SPACE → LIFECYCLE → REMOTE_CALL` 锁序逐窗口证明。
- 不同 AddressSpace 请求在同一 hart 交错时，本地单项 `(identity, translation, instruction)` cache 是否可能被无待办的其它 identity 覆盖而让 leave 永久重试；active 集合的互斥事实是否结构性排除该窗口。
- `synchronize_local` 在尚未切换到目标 user satp 时执行全量 fence、随后 trap 汇编切 satp 再 fence 的正确性与冗余成本；未来 ASID 引入后，cache 形态和切换顺序是否必须整体替换。
- Commit 先于 Running→Terminating 时，后续统一 AddressSpace transaction 是否真正由终止路径接管直到 ack/Retire；Terminating 先行时 Prepared 槽、页表 reservation、backing 与结果 lease 是否全部零业务副作用回滚。

### 与后续 MemoryChange 的组合

- 本提交只提供 completion seam，不含真实 waiter/Retire。待主计划切片 6–7 落地后，对照检查发起线程在 Commit 前/后消散、WaitContext 单 outcome、Handle close park、ProcessDrain 接管与最后 ack 回调是否共同只拥有一份业务事务。
- completion sink 是否只做固定上界的 Retire/offer，较长 frame/object/drain 工作是否转入既有 Waiting/管理者驱动状态机；任何路径都不得在 Remote Call 安全点等待另一 hart。
- OwnedExtents、ObjectView、WritePermit 与 retiring backing 是否在全部 acquire ack 前持续计数和持有；Executable seal、Unmap 后物理帧复用及 Tunnel lease close 分别补真实多 hart 竞态验证。

## 验证复核

- host debug/release 重跑 Remote Call 5 项状态机测试及 page_table 25 项 tree + 1 项 Drop ledger；补充跨表 token、槽退休边界和 completion 立即再 Reserve 的 adapter 级测试。
- RISC-V 负载加入至少两个 hart 同一 AddressSpace 的真实 Unmap/Protect、乱序确认、IPI 失败后安全点补消费、instruction epoch 与 backing 复用 litmus；不得只依赖启动空 Publish 探针。
- `virt`、`virt-release`、`virt-hetero`、`virt-nofd`、`sifive_u` 检查 Remote/epoch 完成锚、10/10 竞态矩阵、正常 reset 或既定平台失败终态，并核对无 Lock Ladder 违规与槽泄漏。
