# 系统审计批次 E-1：启动/页表/TLB 与 SMP/调度/对象生命周期

> 首审已完成；当前 findings 未闭合。本报告同时作为后续修复与逐条复核计划，修复 agent 不重复首审。

## 审计范围与基线

审计分片 3「启动、页表与 TLB」和分片 4「SMP、调度与对象生命周期」。基线：`e5db4f32a507ca5bc26849b53e64c0a3b73fa82d`（`e5db4f3`）。工作树另有预先的 plans 文档修改；本审计未修改文件、未提交代码，代码证据按当前基线与目标文件读取，不用后续代码替代历史事实。

覆盖 BootPackage/StartupBlock、cold transition、DTB memory/reserved/no-map/status、root/high-half/direct map、satp/SFENCE.VMA、页表 owner、AddressSpace、payload、FramePool/Pool/MemoryObject/Tunnel seam；以及 raw hartid/slot、RuntimeGate、SBI/IPI、Lock Ladder、调度域/ready marker、Thread/Process/Job/Handle/Wait/Timer/Remote Call、ThreadSpawn/teardown/deferred work、owner/permit/charge/authority。

A–D 已知 findings 只作交叉确认：WritePermit/retire/EXECUTE、DT status/FramePool/Drop、bootstrap post-commit、supervisor/raw hart/ThreadControl、RemoteCalls/epoch/UserStack、MappingLease、launcher ELF/payload/token 等不重复编号。

## 执行命令与结果

```text
git status --short --branch
git rev-parse HEAD
git log --oneline -8
rg --files os/kernel/src os/page_table/src os/memory_space/src os/remote_call/src shared/src
rg -n "BootPackage|StartupBlock|satp|sfence|fence.i|RuntimeGate|HartId|HartSlot|send_ipi|TimerQueue|RemoteCalls|ThreadSpawn|retire|rank" os shared
cd os && cargo test -p page_table -p memory_space -p frame_pool -p dtb -p handle_table -p wait_context -p timer_queue -p sched_domain -p remote_call --target aarch64-apple-darwin
cd shared && cargo test --target aarch64-apple-darwin
just check
git diff --check
```

os 相关 host 测试共 **137 项**通过：dtb 23（5+6+6+6）、frame_pool 16、handle_table 17、memory_space 19、page_table 36（1+5+30）、remote_call 5、sched_domain 7、timer_queue 6、wait_context 8。shared 18 项通过，`just check` 和 `git diff --check` 通过。

未运行 `just virt`、`virt-stress`、`virt-release`、`sifive_u`、`acceptance`、`virt-hetero`、`virt-nofd`；未运行目标快照隔离 checkout、QEMU 多 hart TLB/RVWMO litmus、HSM/IPI 失败注入、启动 OOM、DTB malformed 全量注入、跨进程 object rights、Timer/Wait 竞态和 drain 压力。因此 host/`just check` 不等于平台启动或完整验收通过。

## 分片 3：启动、页表与 TLB

### 已证实结构

- BootPackage validator 的 64B LE envelope、checked offset/length、canonical payload、page alignment、zero padding 和窗口边界成立。
- cold transition 的 identity/high-half/transition 4KiB 叶、formal satp 切换和本地 `sfence.vma` 顺序成立。
- TranslationTree `prepare→publish→drain`、表页 owner、AddressSpace transaction 和 Remote shootdown 的数据/控制同步结构成立，但缺多 hart 动态证明。
- payload owner 当前已接入 `BootHeldExtent → BootFundedExtent → BootBorrowed projection → install_bootstrap_funding`；D-2 旧的裸 payload owner finding 已由后续实现修复，不作为当前债务。
- 用户 fault 正常进入进程 fault/termination，不在用户同步异常路径直接 panic。

### M3-1 / P1：启动/镜像构造失败丢弃 Bound AddressSpace，TableTree 未 drain 直接 panic

位置：`os/kernel/src/task/proc.rs:4374-4400`、`os/page_table/src/lib.rs:1373-1390`。

可达前提：`spawn_from_elf` 已完成 `Process::new` 和 `bind_memory_internal` 进入 Bound，随后 `load_elf` 的后续 segment、页表 funding、`map_stack` 任一失败；或未来可信 launcher 复用该入口并传入可失败 ELF。

直接证据：`spawn_from_elf` 在 Bind 后直接执行 `process.space.load_elf(...)?`、`process.space.map_stack()?`；`BoundAddressSpace` 无自定义 Drop；`Process` 局部 Arc 消散时没有 ProcessDrain。`TableTree::Drop` 对仍有 `owned_root/owners` 的未 drain 树执行断言。前置 segment 已成功安装而后置 segment/stack 失败时，root/branch owner、metadata、backing 和 ledger 仍在树中。

违反契约：`notes/ideas/mm.md`/`bootstrap.md` 要求失败保持 owner 闭包并经有界 drain；`notes/impls/mm.md` 的 TableTree Drop 仅接受 drain 后终态；用户可达 OOM/坏 ELF 不应升级内核 panic。

影响：坏 initial ELF 或普通 OOM 可触发未 drain TableTree panic，无法证明 Pool/frame/metadata 守恒。该 finding 不重复 D-2 F-01（payload 裸 owner，已修复），也不重复 B-2 post-commit Attach/Job/staged 窗口（不可逆点不同）。

建议：在 `spawn_from_elf` 引入未发布 Bound rollback guard，撤销已安装 ledger/PTE，锁外释放 backing/table/root/binding owner；或使用与 ProcessDrain 等价的有界失败收束。补多段 ELF 后段失败、stack/table OOM 和 partial load drop 的 debug/release 负向测试。

### M3-2 / P1：HartRegistry 未拒绝重复 raw hartid

位置：`os/kernel/src/registry.rs:144-166`、`os/kernel/src/main.rs:180-185`。

可达前提：病态/恶意 DTB 提供两个 admitted CPU 节点且 `reg` 相同。

直接证据：`HartRegistry::admit` 只检查容量，直接写入下一 slot；board/main admission 没有 duplicate check。结果可产生两个同 raw hart 的 BootRecord/slot，`bring_up_runtime` 对同 raw hart 发两次 HSM start，第二次可能 `ALREADY_AVAILABLE` fatal，或另一 record 永远停在 Starting。

违反契约：DT CPU `reg` 唯一性；项目 raw HartId 与内部 HartSlot 必须一一映射。

影响：破坏启动 record、slot→HartLocal、active mask、HSM/IPI 的身份闭包，导致启动 fatal 或永久等待。

建议：`admit` 返回 `Result<HartSlot, DuplicateHartId>` 并在写入前拒绝；补重复 raw id、边界 raw id 和未排序 raw id 的 DTB admission 测试。

### M3-3 / P1：HSM start 错误未发布 RuntimeGate::Failed

位置：`os/kernel/src/rt.rs:114-151`、`os/kernel/src/registry.rs:263-305`。

可达前提：任一 secondary `sbi::hart_start` 返回错误。

直接证据：boot hart 先发布所有 secondary 为 Starting，随后用 `sbi::require(sbi::hart_start(...), "HSM.hart_start")`；错误直接进入 fatal/park，没有先 `registry::publish_failed()`。只有等待 Online 超时分支才发布 Failed。其他 hart观察到的 Gate 可能永久停留 Preparing，而 boot hart已不再是发布者。

违反契约：RuntimeGate 的 Failed 是启动整体失败广播；SBI HSM 错误必须按错误边界处理，不能让其他 hart永久等待。

影响：HSM failure 不能形成统一可观测终态，secondary/晚到 hart 可能永远自旋 Preparing，诊断/停驻闭包不成立。

建议：HSM 错误分支先 Release 发布 `RuntimeGate::Failed`，记录 raw hart/error，再进入 fatal/park；补 HSM failure injection 和 Gate 模型测试。

### M3-4 / P2：CPU slot 顺序依赖 FDT child 顺序，未证明 raw-id 升序

位置：`os/dtb/src/board.rs:414-438`、`os/kernel/src/main.rs:180-185`、`registry.rs:144`。

可达前提：合法 DTB 的 CPU child 顺序为 hart 3、hart 1。

直接证据：board 按 FDT children 顺序写入 CPU 数组，main 按数组顺序 admission；registry 注释却声称 slot 按 raw hart 升序。DT 规范要求唯一 reg，不要求 child 遍历数值升序。

影响：当前显式 record.raw 路径通常仍能运行，但 slot 排序、拓扑快照和未来 affinity/调度诊断不再有文档保证。

建议：admission 前按 raw hart 排序并拒绝重复；或者删除升序契约并同步 notes/impls。长期建议排序。

### 既有交叉确认

- B-1 DT status unknown、FramePool arithmetic、SystemSupply query 仍有效，不重编。
- B-2 bootstrap post-commit Attach/Job/staged 窗口当前仍在 `proc.rs`，与 M3-1 不同阶段，不重编。
- A 的 permit/owner/retire/EXECUTE findings 仍在。
- C-2 epoch overflow、D-2 ELF/PT_INTERP/token 等按既有报告承接。
- D-1 的旧 INSTALLING WaitContext/期限不注销已被后续 `TimeoutRegistration` 和完成闭包修复，不算当前债务。

## 分片 4：SMP、调度与对象生命周期

Lock Ladder rank 集中、同秩链 key 和锁内不出游未发现新的确定性逆序；raw id/slot 分层清晰；RuntimeGate 正常 Prepared→Starting→Online、Release/Acquire 顺序成立；sched_domain eligibility、D64、ready marker FIFO host model 成立；Thread/Process/Job/Handle/Wait/Timer/RemoteCall 正常 owner graph、ThreadDeparture/result obligation、ProcessDrain/deferred work 固定槽方向成立。

本轮未发现可独立于分片 3的新 P0/P1 生命周期/调度 finding。C-1 的 ThreadControl CLOSED、无限监督等待、监督 authority，C-2 RemoteCalls/epoch/UserStack，A/B/D 的 owner/permit/ELF findings 继续交叉承接。当前 raw IPI 的旧 shift 问题已由 `send_ipi(1, raw)` 改善，不机械重复；M3-2 duplicate admission 和 M3-3 Gate error 广播覆盖当前身份/启动错误闭包。

分片 4 的验证缺口：未做多 hart Remote ack/Timer cancel/Wait install、HSM/IPI failure、最大 Job/Thread/Handle/token、ThreadSpawn/kill/join/Drop 与 ProcessDrain 交错 QEMU；不能以 host 纯逻辑测试替代并发证据。

## 已证实闭包

1. transition→high-half→formal satp 和本地 `sfence.vma` 结构存在；非 Resume 出口先切 KERNEL_SATP/本地同步。
2. user tree 共享 kernel high-half root slots，TableTree owner ledger 与 FundedTableFrame 结构连接。
3. AddressSpace MemoryChange Validate/Reserve/Commit/Publish/Synchronize/Retire、Remote ack、work debt 和 ProcessDrain 游标结构统一，但 M3-1 与既有 findings 表明失败收口不完整。
4. raw hart/slot/HartLocal/per-hart timer/active 位图索引层次清晰，SBI 调用使用 raw id。
5. Lock Ladder、调度域、Thread/Process/Job/Wait/Timer/RemoteCall 正常所有权和锁序在代码表面成立。

## 验证缺口

- 未运行本批基线 QEMU virt/virt-stress/virt-release/sifive_u/acceptance/hetero/nofd。
- 未做启动 OOM/partial ELF、HSM failure、duplicate/unsorted raw hart DTB、no-map/BootPackage overlap、payload teardown 注入。
- 未做多 hart Remote ack、Timer cancel/expiry/Abandoned 与 Wait install、最大 token/Job/Thread/Handle 和 ThreadSpawn/ProcessDrain 交错测试。
- 未做跨进程 MemoryObject rights/Seal/EXECUTE/RX、多 extent object/Tunnel 和三账本整机验收。

## 后续行动与复核条件

本报告在 findings 未闭合期间同时作为唯一行动计划，不另建重复 todo：

1. 修复 M3-1 Bound image 构造失败 rollback/drain；
2. 修复 M3-2 duplicate raw hart admission；
3. 修复 M3-3 HSM error 前发布 RuntimeGate Failed；
4. 修复或明确 M3-4 slot 排序契约；
5. 补启动/SMP/页表/TLB/并发故障注入与 QEMU 证据；
6. 同步 `notes/ideas/{mm,bootstrap,execution-context}.md` 与 `notes/impls/{mm,startup,execution-context,internals}.md`；
7. findings 修复并复核后移入 `plans/archived/`。

## 最终判定

**不通过（P1 blocker）。** 分片 3 的 transition、Sv39、PTE owner、AddressSpace/Remote shootdown 和当前 payload owner 正常结构总体成立；但 M3-1 未 drain 失败、M3-2 duplicate raw hart admission、M3-3 HSM error 未广播 Gate Failed 三项 P1 破坏启动失败闭包和 SMP 身份/活性。分片 4 未发现独立新 P1，但未运行 QEMU/故障注入且 A–D findings 仍阻断整体收口。