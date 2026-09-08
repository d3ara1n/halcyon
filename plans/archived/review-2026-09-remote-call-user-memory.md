# 批次 C-2：Remote Call、AddressSpace epoch/TLB 与用户内存 Review

## 2026-09-08 提交后复核（C-2，通过）

对象 `9ee2791d3e18fdb7857fe41c74bacc7bb0c7c774`；OliveWillow 独立只读审查。

| Finding | 结论与证据 |
|---|---|
| F1 / 跨表 token | 闭合。`os/remote_call/src/lib.rs:55`、`:86` 的 Reservation/FinishToken 携 TableId，错表在定位 slot 前返还 affine owner；`tests/transport.rs:68` 跨表测试通过。不是冻结单实例策略。 |
| F2 / epoch 回绕污染 | 闭合。`os/kernel/src/task/proc.rs:1810` 在同一 AddressSpace 锁下先检验需推进 epoch 未耗尽，拒绝发生在 ledger/PTE 发布前；`:1839` 发布后调用 `:987` 的 checked CAS，不依赖回绕后 panic。 |
| F3 / UserStack cleanup panic | 闭合。`user/rinlib/src/thread.rs:107` 对 Busy 重试，终端错误发布 stack_cleanup_snapshot 并留给 AddressSpace drain；`srv_init/src/main.rs:1075` 检查 abandoned 计数。 |

remote_call/monotonic_id 及相关 host 测试通过；统筹者全速 stress、release、sifive_u、hetero 通过。最大 epoch 的内核注入、UserStack 终端错误注入未现场执行，模型/静态证据不冒充 guest 覆盖。nofd 锚点失败归 E2-7-02。三个 finding 已关闭，本报告归档。

---

以下保留历史首审内容。


> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](todo-2026-09-review-program.md) 为准；正文建议保留首审语境，不作为现行实施顺序。

## 审查范围与基线

本报告对应 Review 统筹计划批次 C-2，范围为 Remote Call 固定槽/token/ABA/IPI/RVWMO/AddressSpace epoch/execution gate/SFENCE/retire；用户 `MemoryMap/Unmap/Protect` 与 Pool、permit、ledger、PTE、HandleClose 守恒；ThreadSpawn 双 guard 栈、JoinHandle、result obligation 及 spawn/kill/末线程/join/Drop 竞态。

目标提交：`1cd6ab2`、`6150d40`、`6199985`、`c82d91a`、`23ec19d`、`6825e19`、`9358963`、`bdc83ef`、`004cae5`。审查基线为 `2ed7e1e`，报告生成时工作树已包含文档 checkpoint `f9b3bda` 之后的未提交 plans 改动；所有代码结论均来自目标提交快照或隔离归档，不把当前树后续变更作为历史提交证据。全程只读，未修改或提交代码。

与 C-1 的边界：C-1 负责线程生命周期、持久 init/pm 和调度域；本报告负责 Remote Call、地址空间同步和用户内存。`004cae5` 的交叉时序由统筹主代理去重。批次 A/B 已知 findings 不在本报告重复：Running rollback 的 WritePermit 泄漏、object owner 重复 retire、post-Commit retire 分配、缺 EXECUTE capability、DT status、FramePool arithmetic、MemoryPool/SystemSupply Drop/query，以及 bootstrap post-commit 可失败窗口。

## 执行命令与结果

执行过：

```text
git log --oneline -12
git show --stat <九个目标提交>
git show <sha>:<path> | nl -ba   # 逐提交读取 remote_call、proc、thread、process、lifecycle、sched、trap、page_table、memory_space、shared、rinlib 等
 git grep -n -E 'expect|unwrap|assert' <目标提交> -- <相关路径>
git diff 004cae5^..004cae5 -- os/frame_pool
cd os && cargo test -p remote_call -p page_table -p memory_space --target aarch64-apple-darwin
git archive 004cae5 | tar -x   # 在隔离快照复跑 remote_call/page_table/memory_space/frame_pool
cd shared && cargo test   # 隔离快照
```

当前树相关 host 命令编译通过；目标 `004cae5` 隔离快照的 remote_call/page_table/memory_space/frame_pool 与 shared host 测试编译通过。未运行 `just check`、QEMU virt/virt-stress/virt-release/hetero/nofd/acceptance/sifive_u；host 测试输出未逐条核验断言，因此只作为编译/基础验证证据，不作为完整历史验收证明。

## 九个提交逐项结论

1. `1cd6ab2`：有界 FramePool/order 树和 affine FrameTracker 结构成立；`004cae5` 的 `release_blocks` 兄弟节点物化修复补足整块分配归还语义。未发现新增 finding。
2. `6150d40`：MemorySpace 区域/事务/lease 状态机、validate→reserve→commit→publish→synchronize→retire token 校验闭合；footprint×lease 双向冲突检查成立。未发现新增 finding。
3. `6199985`：固定槽、BatchCompletion、RVWMO/IPI/ack、enter/leave gate 结构正确，但发现 F1、F2。
4. `c82d91a`：published/result obligation 分离以及 Complete→retire→mandatory→WaitContext 顺序成立。未发现新增 finding。
5. `23ec19d`：Tunnel ObjectView/WritePermit/HandleClose/Drain lease 的目标路径未发现本范围新增可达问题。
6. `6825e19`：公开 Map/Unmap/Protect ABI、UserWriteLease、commit cookie 和几何拆分基本成立；发现 F3。
7. `9358963`：8A 单线程公开面和 guard/termination/platform matrix 无新增 owner seam。
8. `bdc83ef`：Spawning 中间态、ThreadDeparture/result obligation、JoinHandle、双 guard 和主要锁序成立；未发现本范围新增锁逆序。
9. `004cae5`：spawn/kill、末线程/join/Drop 竞态和 FramePool extent 归还修复未发现新增可达 P1；交叉面如与 C-1 重合，遵从统筹去重。

## Findings

### F1 / P2：RemoteCalls token 未绑定实例，跨表同形 token 可误命中

位置：`os/remote_call/src/lib.rs`（目标 `6199985`），`cancel` L136-147、`publish` L150-163、`finish` L189-204、`entry_mut` L220-223。

可达前提：存在两个 `RemoteCalls` 实例，或内部 token 被错误传递到另一实例；另一实例恰有相同 `(slot, generation)` 且处于相容 phase。当前内核只有一个全局 `CALLS` 且类型为 `pub(crate)`，因此当前主线不可达，但 `RemoteCalls::new` 为 `pub`，token 具可复制形态，API 边界没有结构性实例证明。

直接证据：`entry_mut(target, slot, generation)` 仅据三元组定位，`publish/finish` 只校验 phase 和 generation，不校验 token 来源于当前表实例。

违反契约：`notes/impls/call.md` 的 generation/ABA 保护要求；Remote Call 计划要求跨实例 token 误用必须被拒绝。

后果：跨表 token 可 bump 另一实例 generation、使 reserve 失败或把他人 Taken 槽归还为 Empty，丢失 payload。

建议：token 内嵌不可伪造 `TableId` 并在 entry lookup 前验证；或限制 `RemoteCalls::new` 为 crate 私有，仅暴露单实例封装。补跨表 token host 测试。若长期明确单实例政策，可将其记录为显式不变量并保留防误用护栏。

### F2 / P3：epoch `fetch_add` 在溢出时先污染状态再 panic

位置：`os/kernel/src/task/proc.rs` `publish_epochs` L863-877，目标 `004cae5`（自 `6199985` 引入）。

直接证据：`fetch_add(1, Release).checked_add(1)` 检查的是旧值加一；旧值为 `u64::MAX` 时原子已写入 0，随后才因旧值加一溢出 panic。`FenceRequest` 又断言 epoch 非零。

可达前提：同一 AddressSpace 完成约 `2^64-1` 次相关 Commit，现实中不可达；这是边界硬化问题。

违反契约：`notes/impls/mm.md` 要求 epoch 单调且不复用；checked arithmetic 应在状态污染前拒绝。

建议：用 load 门禁或依据 `fetch_add` 返回的旧值在达到 `u64::MAX` 时先拒绝/永久退休，不让原子环回；补边界模型测试。该 finding 与 B-1 arithmetic 属同族，但位置和字段独立，保留为新增证据。

### F3 / P3：rinlib UserStack 清理对非 Busy 错误直接 panic

位置：`user/rinlib/src/thread.rs:58-71`，目标 `004cae5`（与 `bdc83ef` 同源）。

可达前提：UserStack affine owner 在 Drop/join 清理时调用 Unmap，返回除 `ObjectBusy` 外的错误；当前首版正常 ownership 下基本不可达，但未来 Unmap 错误扩展或多线程异常窗口可达。

直接证据：只有 `ObjectBusy` 进入 sleep 重试，其余错误执行 `panic!("UserStack cleanup failed: …")`。

违反契约：用户态资源收束应如实暴露错误；与 B-1 的 MemoryPool Drop `expect` 同属错误策略族，但不是同一函数。

建议：明确 affine owner 的 Unmap 错误集合为结构性不变量并在接口中证明，或改为记录错误并进入显式泄漏/终止政策；与 MemoryPool owner Drop 策略统一，不要让新错误类型意外变成 panic。

## 已证实不变量

- Remote 槽 `Empty→Reserved→Pending→Taken→Empty/Retired` 单向，finish 只接受 Taken，generation 在复用前推进。
- 批量 reserve 失败可取消全部预留；publish 后不可取消，Pending 是工作真值。
- BatchCompletion 使用 AcqRel 位清除和 release sequence 归并 ack。
- Commit PTE/epoch release → publish slot → fence/IPI → 目标 acquire + `sfence.vma`/`fence.i` → release ack 的 RVWMO 链不依赖 spinlock 隐式屏障。
- enter/leave gate 在 dispatch 前同步并于 gate 内复检 epoch；active 快照只减不增。
- Map 事务的 completion、shootdown、backing、permit、result obligation 在 Commit 前准备，正常 commit 闭包无普通可恢复分配。
- Spawning/ThreadDeparture/result obligation/JoinHandle 的正常所有权转移和 `HANDLE_TABLE→ADDRESS_SPACE→LIFECYCLE` 锁序成立。

## 验证缺口

- 未在目标快照运行完整 `just check`、QEMU virt/virt-stress/virt-release/hetero/nofd/acceptance/sifive_u；无法现场证明多 hart stale translation、IPI 失败、ack 乱序、1024 spawn/join 竞态。
- 未测试跨 `RemoteCalls` 实例 token、epoch 环绕和 UserStack 非 Busy 清理。
- 多 extent object、对象 side sanitize 和 Seal/EXECUTABLE 属批次 A/C-1 边界，不在本报告重复。
- host 测试命令仅确认编译/通过返回，未逐条审查断言输出。

## 后续行动与复核条件

以下为首审建议与复核条件；当前 F1/F2 由 identity 计划拥有策略并与事务接线同步，F3 由 capability/owner 计划实施：

1. 为 Remote Call token 补实例身份，或在实现文档中冻结并证明单实例 API 边界；
2. 修正 epoch 溢出检查，统一 arithmetic hardening；
3. 与 MemoryPool Drop finding 合并审视 rinlib 所有 affine owner 清理错误策略；
4. 增补跨表 token、epoch 边界、Unmap 错误和多 hart QEMU 证据；
5. 全部闭合后再移入 `plans/archived/`。

## 最终判定

**有条件通过 / 非阻断。** 本范围未发现可达 P0/P1。Remote Call、epoch/execution gate、RVWMO 确认链、Map/Unmap/Protect 守恒、ThreadSpawn/join 所有权和末线程竞态的核心结构成立；F1 为 P2 API 防误用边界，F2/F3 为 P3 防御与工程收口缺口。完成上述收口或书面冻结单实例/错误策略后，本批次可复核通过。
