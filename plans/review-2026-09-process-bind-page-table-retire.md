# 批次 B-2：ProcessBind、页表 owner 与 deferred retire Review

## 审查范围与方法

基线：`2ed7e1e`。目标提交：

- `7c76097`：ProcessCreate/ProcessBindMemory/root bootstrap
- `c522e50`：funded owner 与页表生命周期
- `7225673`：deferred retire/work debt
- `cfad6cf`：页表资金化事务
- `addb4a5`、`b4bfb20`：切片 6D 收口

代码证据全部来自目标提交快照，不采用当前工作树后续改动。审查覆盖 ProcessCreate/bootstrap、ProcessBindMemory、funded root/PoolBinding、TableTree owner 生命周期、页表资金化事务、deferred retire/work debt、Running/Building/Tunnel transaction gate、错误映射和失败回滚，重点核对 Commit 前全部可失败工作、Commit 后零分配/必完成、唯一 owner、generation、锁序和锁外析构。

执行命令与证据：

```text
git status --short --branch
git show / git grep / git diff-tree <目标提交>
git archive b4bfb20   # 提取到 /tmp/halcyon-review-b4bfb20
cd /tmp/halcyon-review-b4bfb20/os && cargo test -p funded_frame -p memory_pool -p metadata_admission -p page_table -p memory_space -p work_debt --target aarch64-apple-darwin
just check
just virt
just virt-stress
```

`b4bfb20` 隔离快照中的 funded_frame 11、memory_pool 14、metadata_admission 8、page_table 1+5+30、memory_space 19、work_debt 5 项测试全部通过。当前工作树 `just check`、`just virt` 通过，只作环境参考；`just virt-stress` 在已知 flake `last-thread-exit-vs-kill` 失败并超时，不据此新增 finding。未在目标提交隔离环境运行完整 acceptance/release/sifive_u。

## 逐提交结论

### `7c76097`

ProcessCreate 已收窄为 Unbound shell，Job member reservation 与两项输出 Handle 在 Commit 前预留；Bind 通过 BuildingLease、bind_in_progress、Builder/Pool pin、锁外 PoolBinding/root 准备，`HANDLE_TABLE → ADDRESS_SPACE` 双锁提交后再锁外 close。bootstrap 复用普通 binding seam，root Pool Handle 与内部 binding 指向同一 core 的设计成立。

但该提交的 bootstrap launcher 在 Handle 不可逆提交后仍执行可失败的 Attach/Job/Start 准备，最终代码仍未闭合，见 F-1。

### `c522e50`

`FundedRootFrame` 单页 owner+charge、TableTree root/intermediate owner、PreparedTranslation/PublishOutcome 和 `prepare → publish → drain` 分离清晰；host 测试覆盖额度守恒、错误回滚、owner 返回和 drop chain。未发现独立阻断 finding。

### `7225673`

固定槽 work debt、owner/generation/FIFO、Pending 电平和安全点消费路径已接通；`Process::drain_batch` 与 `AddressSpaceState::drain` 将 owner 从锁内摘下、锁外析构，符合 `MEMORY_POOL → ADDRESS_SPACE` 与有界工作原则。bootstrap 后置提交窗口仍会绕过正常 drain 入口。

### `cfad6cf`

匿名/对象 map/unmap/protect 的 plan/fund/complete/commit 分层，funded table owner 在 AddressSpace 锁外取得，Prepared 以 generation 门控，失败返回 `ReclaimedTableFrames`；Running completion 通过 Remote ack 后进入 Retiring/work debt。未发现独立阻断 finding。

### `addb4a5`

普通 MemoryPool syscall、Tunnel/object map/unmap/protect、ProcessStart/Thread 路径进一步切到 funded owner/事务接口；Handle/Job/生命周期接线与页表测试覆盖面较完整。未发现独立阻断 finding。

### `b4bfb20`

补齐异常完成路径观测和实现文档，隔离快照 host 测试通过。bootstrap launcher 的 Commit 后可失败窗口仍存在。

## Finding

### F-1 / P1：bootstrap 在不可逆提交后仍执行可失败 Attach/Job/Start 准备

位置：`os/kernel/src/task/proc.rs:3954-4002`（`b4bfb20`）。关键点：

- `3954-3958`：`table.commit(reservation, handles)`；
- `3961-3972`：随后调用 `process.attach_thread`，Err 直接 return；
- `3977-3981`：随后 `job.reserve_member` 仍返回 Result；
- `3992-3999`：随后 `staged.try_reserve_exact(1)` 仍可失败，再以 `begin_running(...).expect` 提交。

可达前提：bootstrap 已完成 ELF/stack/StartupBlock map、Handle reservation 已 commit；随后线程对象/ThreadDeparture/Arc 分配 OOM，root Job member Vec 扩容 OOM，或 staged Vec reserve OOM。这些是代码显式承认的正常 `OutOfMemory` 路径。

直接证据与后果：

1. `table.commit` 已使启动 handles 对进程可见；之后 `attach_thread` Err 直接返回，没有 table rollback、没有撤销 entries，也没有解除映射或释放 funded payload owner。
2. 此时 process 尚未加入 Job。局部 `Arc<Process>` drop 会触及仍有 root_owner/owned root/owners 的未 drain TableTree；`os/page_table/src/lib.rs:1373-1382` 的 Drop assert 会把 OOM 失败升级为内核 panic。
3. Attach 成功而 `job.reserve_member` OOM 时，lifecycle 已插入 Staging member，但 Process 没有 Job member；直接 return 后 Staging Thread 持有 `Arc<Process>`，形成无法由正常 Job drain 到达的孤岛。
4. Job reserve 成功而 staged reserve OOM 时，Job member 已 commit，lifecycle 仍 Building 且线程仍 Staging，留下 Job 中不可运行的半初始化成员。

违反契约：

- `notes/ideas/kernel.md`：Commit 是唯一不可逆点；Commit 后不得再有可恢复失败，事务必须归内核并必然完成。
- `notes/ideas/bootstrap.md`：bootstrap 必须复用 Building Bind/Map/Write/Grant/Attach/Start 的原子组装，发布前完成全部条件；失败不得留下半初始化 Job member。
- `notes/ideas/mm.md`：Bound 地址空间释放必须经可恢复 drain；最终析构只能验证已清空。
- Rust Review 纪律：资源可达 OOM 必须错误返回并保持闭包，不得由 expect/drop assert 定义失败语义。

建议修复方向：把 bootstrap launcher 收敛为单一事务。在任何 Handle/Job/lifecycle 不可逆提交前，完成所有可能失败的资源准备，包括线程对象、Job member Vec 容量、staged buffer、execution domain/ready admission；或引入专用 bootstrap transaction guard，统一拥有 Handle reservation、lifecycle staging、Job reservation、BoundAddressSpace/payload owner 并在 Commit 前 rollback。Commit 后只允许无分配、不可失败的固定发布序列。若选择不可逆点后转入终止接管，必须提供与普通 Start/mandatory completion 同等的固定槽接管路径，不能直接 return。

## 已证实不变量

- ProcessCreate 输出 Handle 与 Job member 在正常路径上先写用户输出、再 commit member、再公开 capability；HandleTable reservation rollback/temporary generation 逻辑有 host tests 支持。
- Bind 仅允许 Unbound → Bound 一次；Pool Handle pin 与 PoolBinding/root funded owner 的同源关系、失败时 unpin/owner drop 路径成立。
- `FundedRootFrame` 为单页 owner+charge；TableTree 只持 root/intermediate owner，Prepared/Publish/Outcome 明确转移所有权；未消费 owner经 failure/outcome 返回。
- Running/Tunnel map/unmap/protect 的 Commit 前 fund/prepare 与 generation gate、Commit 后 Remote ack/Retiring/work debt 路径在正常路径和 host model 中闭合。
- deferred retire 使用固定槽、generation、owner FIFO 与 Pending 电平；安全点和 idle 双检覆盖门铃丢失窗口。
- AddressSpace drain 的 ledger/backing/table/root 分阶段游标推进，owner 在 AddressSpace 锁外析构；TableTree final Drop assert 能暴露未 drain 状态，但 F-1 正是把该 assert 暴露给可达 OOM。

## 验证缺口与复核条件

- 分别在 `Thread::new_thread`/`Arc::try_new`、Job member `try_reserve`、staged `try_reserve_exact`、Handle commit 后步骤注入 OOM，断言返回错误而非 panic，并检查 Job members、lifecycle members、HandleTable、ledger/PTE、backing、root Pool allocated 与 boot-held/payload owner 全部守恒。
- 建立 bootstrap Attach/Job/Start 的“每个不可逆点后禁止 Result/分配”静态或单元检查；普通 `thread::spawn` 的 rollback 可作为机制对照，但不应继续手写平行流程。
- 修复后从干净提交快照运行完整 acceptance、release、sifive_u，并重新 Review。

## 后续行动与文档归属

本报告在 finding 未闭合期间即为行动计划，不另建重复 todo：

- 机制/事务边界修订进入 `notes/ideas/bootstrap.md` 与 `notes/ideas/mm.md`；
- 实现机制同步 `notes/impls/mm.md`、`notes/impls/task.md`；
- 修复批次须覆盖 F-1 所列 rollback、终止接管与故障注入；
- 修复提交完成后以本报告为清单执行复核，全部闭合后把报告移入 `plans/archived/`。

## 最终判定

**不通过（P1 blocker）。** 六个提交的正常 owner、资金化和 deferred-retire 设计大体成立，但 bootstrap 后置提交窗口违反核心 Commit/失败闭包不变量，可由资源耗尽触发内核 panic、孤立 Job member和 backing 泄漏。需修复、补齐故障注入和干净快照 acceptance 后重新 Review。
