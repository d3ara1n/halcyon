# 统一内存事务核与公共 MemoryObject Review

## 2026-09-08 提交后复核（A，通过）

固定对象：`9ee2791d3e18fdb7857fe41c74bacc7bb0c7c774`。WiseHare 独立只读审查，统筹者核对当前代码和组合验证。下列路径均相对仓库根，行号对应固定提交。

| 原 finding | 结论与证据 |
|---|---|
| WritePermit 回滚泄漏 / P1 | 闭合。`os/kernel/src/task/proc.rs:2027`、`:2368` 失败路径交还 reclaimed permits；`:2347` 按 ObjectId 归还来源。`memory_space` rollback 返回 permit，相关 host 测试通过。 |
| 同对象多个 retiring fragment 重复摘 owner / P1 | 闭合。`os/memory_space/src/space.rs:1333` 聚合 ObjectRegionDelta，`proc.rs:2785` 预留 distinct retiring 容量，`:3073` 每对象建立一个退役 owner；`:1270` 只消费唯一槽。 |
| Commit 后 retire 分配 / P1 | 闭合。planner 在 Prepare 前计算容量，`proc.rs:455`、`:2785` 的 Vec 在发布前预留，后置 push 消费既有容量。匿名 backing 增长另由跨在途 reservation 计入。 |
| 缺 EXECUTE / P1 | 闭合。`shared/src/object.rs:59` 定义独立位，`os/kernel/src/task/memory_object.rs:478` 要求 RX 的 MAP/READ/EXECUTE，`srv_init/src/main.rs:713` 起 guest 矩阵覆盖 Mutable、派生裁剪和原 Handle 关闭后合法 RX。 |

验证：reviewer 运行 memory_space/shared host 测试通过；统筹者运行七面 Clippy，通过全速 stress 16/16、release、sifive_u、hetero。两次 50% stress 为既知 15/16 flake；nofd 因旧日志锚点失败，单独归 E2-7-02，不能写本轮 acceptance 聚合通过。

证据限制：未现场重放逐步 OOM/stale 注入、Commit 后禁用 allocator 的整机测试、公共对象跨进程和多 extent/Seal-WaitMany 组合。现有 Seal/RX 用例没有直接 WaitMany(EXECUTABLE)，实现文档已更正；这些限制不伪装成已运行测试。四项正式 finding 已关闭，本报告归档；E-1/E-2 的新增问题不重复编号到 A。

---

以下是原目标提交的首审记录；其中“当前”及最终不通过判定均保留首审语境。


> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](../todo-2026-09-review-program.md) 为准；正文建议保留首审语境，不作为现行实施顺序。

## 审查范围与基线

- 项目：Halcyon / eRhino RV64 微内核系统
- 审查批次：Review 统筹计划批次 A
- 目标基线：`2ed7e1e`
- 主要审查提交：
  - `d2ff81e`：对象与内存事务残留清理
  - `5c0bbb0`：Running / Building / Tunnel 内存事务统一
  - `d6a162c`：公共 MemoryObject ABI 与 `EXECUTABLE` 电平
- 必要前置提交：`6e18b8f`、`16dd3b4`、`51b3742`、`0fad27f`、`310d089`、`0a4eacb`、`81b5b3e`

审查以目标提交内容为准，主要通过 `git show <commit>:<path>` 读取，不把当前工作树中的后续文档状态当作目标提交证据。当前工作树存在 plans 文档迁移相关的未提交改动，但未修改代码；本报告未采用这些后续改动替代目标提交内容。

## 复现环境与执行命令

环境：

- macOS
- Rust nightly，使用仓库固定 toolchain
- `just 1.58.0`
- 当前提交：`2ed7e1e`

执行过的命令：

```text
git status --short
git log --oneline --decorate -12
git show --stat --oneline d2ff81e
git show --stat --oneline 5c0bbb0
git show --stat --oneline d6a162c
git diff --name-status d2ff81e^ d6a162c
git show d6a162c:<path>
git diff --check 2ed7e1e^ 2ed7e1e
cd os && cargo test -p memory_space --target aarch64-apple-darwin
cd shared && cargo test --target aarch64-apple-darwin
cd os && cargo test -p tar -p elf -p page_table -p frame_pool -p dtb -p handle_table -p wait_context -p timer_queue -p stack_layout -p sched_domain --target aarch64-apple-darwin
just check
just virt
```

结果：`memory_space` 19 项、shared 18 项及相关 host 测试通过；`just check` 和 `just virt` 通过。未执行 `just virt-release`、`just virt-stress`、`just acceptance`，也未执行本报告涉及的失败注入、多对象批量 Unmap、Seal/RX、跨进程 Handle 或 OOM 注入测试。

## 逐项结论

### 1. WritePermit 守恒

前半段路径大体正确，但统一 Running 事务尾段存在已证实的 permit 泄漏。`memory_object::map_view`、`fund_and_complete_running`、Tunnel `abandon_mapping`、planner rollback 和 `RetiringSpaceChange::advance` 的主要路径都有显式处理；但 `start_running_memory_change` 的 rollback 宏只析构 `ReclaimedTableFrames`，没有提取并归还其中的 `WritePermit`。

### 2. view owner 与 permit 析构顺序

单个 object fragment 的正常顺序设计正确：AddressSpace 锁内移出 owner，以 `RetiringObjectView` 保活，先完成 permit 归还，最后清理 retiring owner。但 `release_view_region` 只检查 live ledger，不检查同一 retire batch 中尚未处理的 fragment。同一对象多个 region 在一次 Unmap 中完全移除时，第二个 fragment 会再次查找已移除的 owner 并触发 `expect`。

### 3. 锁阶

已检查的主要路径基本遵守锁阶，没有发现新的确定性逆序。对象授权、permit 取得、permit 退役、view owner 安装和 drain 路径均未发现 AddressSpace → MemoryObject 的锁逆序。批量 retire 的 owner 判断缺陷属于 owner 语义问题，并非新的锁阶问题。

### 4. Commit 后零分配

`commit_inner` 的主要容量预留基本完整，`install_view_owner` 的 insert 有前置容量基础。但 `RetiringSpaceChange::advance` 在 Commit 后第一次处理 object fragment 时执行 `retiring_views.push(...)`；`begin_retire_published_change` 仅构造空 `Vec`，没有看到按本批对象数前置 `try_reserve`。该不可逆阶段仍可能分配。

### 5. AddressSpace-owned 与 object-owned view 撤销闭包

正常单 view 路径的 authority 分离基本正确。`MapAuthority::AddressSpace` 写入普通 ledger，`MapAuthority::ObjectLease` 使用 `RegionOwner::Lease(LeaseKey)`，Tunnel close 复用同一页表事务、shootdown 与 retire 路径。未发现正常路径中的确定性越权证据；多页 lease 的完整闭包仍未被本批验收覆盖。

### 6. 容量自洽

`MAX_WRITE_VIEWS_PER_OBJECT = 64`、Tunnel 写 view 上限 2、ObjectView admission 全局 8192/每 sponsor 256、ObjectBacking admission 全局 2048/每 sponsor 64、backing 页数上限 512 之间没有仅凭数值即可证明的直接矛盾。但这些关系缺少正式容量推导、静态断言和压力测试，属于低风险设计/文档缺口。

### 7. 多段对象投影

`ObjectBacking` 多 extent 表达、`project` 有界物理 span、offset/length checked arithmetic、逐 span preflight、锁外表页 funding、失败回滚以及单页假设清理均已成立。完整闭包仍受批量 owner 问题和验收缺口影响；多 extent 公共对象的跨进程、部分撤销和 frame 级守恒尚未被 QEMU 验收。

### 8. seal、EXECUTABLE、Query、跨进程与 rights

状态机的 Mutable → Sealing → Executable 单向性、writable permit 竞争、最后 permit 退役后的 EXECUTABLE 电平以及 Query 的基本实现方向成立。但公共 ABI 缺少独立 `EXECUTE` capability：`shared::object::Rights` 没有 `EXECUTE` 位，MemoryObject rights 上限没有该位，RX map 只要求 `MAP | READ`。这与 `notes/ideas/object.md` 和 `notes/impls/memory-object.md` 的“执行 view 要求 `MAP | READ | EXECUTE`”不一致。

现有验收只覆盖创建、Mutable Query、RW/RO view、Handle close 后 view 保活、部分 Unmap 和 Pool charge；未覆盖 Seal/EXECUTABLE WaitMany、Sealing/Executable Query、RX view、seal 竞态、rights 矩阵、跨进程 Handle、frame 级守恒和多 extent 跨地址空间映射。

### 9. Rust 代码健康

多数 `expect` 位于内部 token 状态机、已验证几何、提交阶段或启动自检路径，unsafe 布局读写注释与 const layout 检查基本成立，未发现新的明确 unsafe SAFETY 论证错误。但以下两个问题会使用户可达状态组合触发内核级故障：`os/kernel/src/task/proc.rs:3100` 的重复 owner 释放 `expect`，以及 `os/kernel/src/task/proc.rs:1874` 一带的 permit 静默丢弃。

## Findings

### P1：统一 Running 事务失败回滚静默泄漏 WritePermit

位置：

- `os/kernel/src/task/proc.rs:1869-1875`
- `os/kernel/src/task/proc.rs:2763-2788`
- `os/kernel/src/task/memory_object.rs:503-505`

可达前提：公共 MemoryObject RW mapping 或 Unmap/Protect replacement 已取得 `WritePermit`，随后 `prepare_memory_completion`、shootdown、stale commit 等 Commit 前步骤失败。

直接证据：`start_running_memory_change` 的 rollback 宏只 drop `ReclaimedTableFrames`；`rollback_memory_change` 返回的 permit 保存在 `ReclaimedTableFrames` 中，但没有 `take_permits()`/`cancel_write()`，而 `WritePermit` 没有 Drop 归还语义。

违反契约：`notes/ideas/mm.md` 的事务失败零副作用与 permit 覆盖 reserved/published/retiring 三阶段；`notes/impls/memory-object.md` 的未提交 permit 必须取消；原批次 Review 清单的 WritePermit 守恒要求。

后果：对象 `MemoryObjectState::permits` 永久高于真实 writable view 数；后续 Seal 可能永久停留在 Sealing，`EXECUTABLE` 不发布，Query 的 `write_views` 错误。

建议：统一 rollback 尾段在 AddressSpace 锁外按 permit 对应的对象 core 调用 `cancel_write`；让 completion、shootdown 和 stale commit 三类失败共用该 helper；补 map/unmap/protect 失败注入与 Seal 守恒测试。

### P1：同一对象多个 retiring fragment 重复移除 owner 并触发内核 panic

位置：

- `os/kernel/src/task/proc.rs:1118-1147`
- `os/kernel/src/task/proc.rs:3096-3122`

可达前提：同一 MemoryObject 在同一 AddressSpace 建立两个或多个 RO 或 RW view，且一次严格覆盖的 MemoryUnmap 同时解除多个 region。

直接证据：退休路径逐 fragment 调用 `release_view_region(object)`；该函数只在 live `views` 表中查找唯一 owner，并在找不到时 `expect("retiring object fragment lost its view owner")`。同一批次第二个同对象 fragment 到达时 owner 已被第一个 fragment 移除。

违反契约：每个 AddressSpace 对对象的强 owner 必须与 ledger 引用闭合；用户可达 fault 不得升级为内核 panic；原批次 Review 的 owner 保活和批量 retire 约束。

后果：合法的一次跨 region Unmap 会在异步 retire 阶段 panic 内核。

建议：创建 `RetiringSpaceChange` 时按 ObjectId 去重，或维护 batch-local `released_objects`，每个对象一个 `RetiringObjectView`；加入同对象多 RO、多 RW、混合 fragment 和部分撤销测试。

### P1：post-Commit retire 执行未预留的 Vec::push

位置：

- `os/kernel/src/task/proc.rs:417`
- `os/kernel/src/task/proc.rs:3050-3062`
- `os/kernel/src/task/proc.rs:1146-1147`

可达前提：object view 成功 Commit，shootdown 完成进入 Retiring，且 retire 需要释放 AddressSpace object view owner，`retiring_views` 容量不足。

直接证据：`RetiringSpaceChange.retiring_views` 是普通 `Vec`，`begin_retire_published_change` 构造空 Vec，后续 `advance` 在已越过 Commit 后执行 `push`，未发现前置 reserve 或固定容量容器。

违反契约：Commit 后事务不可失败、retire 工作由固定预算推进、Commit 路径必须覆盖所有容量预留。

后果：不可回滚阶段可能发生 allocator failure，只能 panic/abort。

建议：从 validated retire fragments 计算 distinct object owner 数，在 Commit 前 `try_reserve`，或改用固定容量按对象去重容器；补极限容量和 allocator failure 测试。

### P1：MemoryObject 缺少独立 EXECUTE capability

位置：

- `notes/ideas/object.md:28-45`
- `shared/src/object.rs:46-59`
- `os/kernel/src/task/memory_object.rs:282-295`
- `os/kernel/src/task/memory_object.rs:450-455`

可达前提：对象已 Executable，调用方持有 `MAP | READ` Handle，并请求 ReadExecute view。

直接证据：`Rights` 没有 `EXECUTE` 位，MemoryObject rights 上限也没有该位，ReadExecute 只检查 `MAP | READ`。

违反契约：设计要求 EXECUTE 独立于 READ/MAP/修改权，执行 view 要求 `MAP | READ | EXECUTE`。

后果：RX mapping 无法通过 capability 独立授予或撤销，跨进程 rights 裁剪无法表达可读不可执行。

建议：新增 `Rights::EXECUTE`，更新 KNOWN mask、MemoryObject rights 上限与 RX required rights，补三状态和跨进程 rights 矩阵测试。

## 已证实成立的不变量

1. MemoryObject 状态机单向性以及 seal/writable view 的状态锁内线性化成立。
2. 对象 backing 由 `MemoryObjectCore` 唯一持有，AddressSpace view owner 与 backing 生命周期分离，Handle close 不直接撤销 view。
3. ObjectViewAuthorization 提供固定长度、多 extent projection 和 checked offset/length 几何。
4. 表页 funding 与 PTE publish 分离，主要对象授权/permit 路径没有确定性锁逆序。
5. 公共匿名 Map/Unmap/Protect 的 planner Validate/Reserve 失败原子性在现有路径中成立。

## 低风险设计与验证缺口

- `MAX_WRITE_VIEWS_PER_OBJECT`、ObjectView admission、Tunnel limit 的容量推导尚未进入实现文档或静态测试。
- Seal/Sealing/Executable Query、EXECUTABLE WaitMany、RX view 与 seal 竞态没有验收。
- 跨进程 object Handle transport、rights 裁剪和 Handle close 后 view 保活没有验收。
- 多 extent object 跨进程/跨地址空间 map、部分 Unmap/Protect 和 frame/charge/metadata 三账本守恒没有验收。
- post-Commit allocation audit 尚未覆盖 retire 编排的全部容器。

## 后续行动与复核条件

以下为首审建议与复核条件；当前前三项事务问题由内存事务计划统一实施，EXECUTE 由 capability 计划实施，归属见统筹导航：

1. 先修复统一 rollback 的 WritePermit 归还、retire batch 的 object owner 去重和 post-Commit 容量预留；
2. 再补齐独立 `EXECUTE` capability、RX rights 矩阵及 shared/kernel/rinlib 纵向 ABI；
3. 增加 map/unmap/protect 失败注入、批量 retire、Seal/WaitMany、跨进程 Handle、多 extent 和 frame/charge/metadata 三账本验收；
4. 同步更新 `notes/ideas/object.md`、`notes/impls/{mm,memory-object,tunnel}.md`；
5. 修复提交完成后以本报告逐项复核，全部 findings 闭合后移入 `plans/archived/`。

## 最终判定

**不通过代码 Review。** 统一事务核、对象 backing 多段投影、对象状态机和主要锁阶方向总体合理，但四项 P1 问题分别涉及 permit 守恒、合法用户请求导致内核 panic、Commit 后不可恢复分配和 capability authority 不完整。当前 host、`just check` 与 `just virt` 通过不足以证明该批次语义收口。
