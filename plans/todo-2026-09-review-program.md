# Review 统筹计划：已完成主线的只读审查

> 当前首审阶段已结束。本计划保留为 Review findings 的状态导航与修复交接入口；后续修复 agent 应以各 `plans/review-*.md` 的未闭合 findings 为唯一行动真值点，不再重复发起首审。
>
> 本计划是当前这轮代码 Review 的统筹入口。本次将现有重叠的 `todo-*-review.md` 整理并合并到批次 A–E；这只是当前积压任务的归并，不废止 `todo-*-review.md` 作为未来提交后的审查计划类型。归档不表示对应审查已完成。每个批次必须先生成正式 `review-<日期>-<主题>.md`，再将本批次标记为已收口。
>
> 审查模型：`moeflux-openai-responses/gpt-5.6-sol`。审查只读，不修改代码；发现问题只写 finding、notes 归属或新的修复 todo。
>
> 当前配额约束：本轮 `reviewer` 子代理账户已耗尽，不再继续委派 reviewer。后续 Review 优先使用用户直接开启的 mesh 主代理会话；`explorer` 仅做代码测绘和事实收集，`researcher` 仅做外部资料取证，二者不能替代 reviewer 或主代理进行 Review 判定和设计决策。该约束是当前工具状态，不改变未来角色定义。

## 当前基线

- 当前状态复核基线：`11fde56`（2026-09-05，系统审计首审文档收口）。各历史 Review 报告仍以各自目标提交为证据，不以本基线替代历史范围。
- Review 纪律：[`REVIEW.md`](REVIEW.md)。
- 代码、测试和实现文档均以提交范围为准，不以当前工作树的后续状态替代目标提交证据。
- Review 报告写入 `plans/review-<日期>-<主题>.md`；报告完成后才将本计划对应批次标为已收口。当前 A–E 均已有首审报告并保留在根目录承载未闭合 findings；系统审计规范详见 `todo-2026-08-system-audit.md`。

## 统筹原则

1. 先审会影响后续主线的基础机制，再审可独立收口的历史批次。
2. 同一提交或同一 owner/事务 seam 只保留一个审查真值点，重叠计划不重复全量审查。
3. 设计结论进入 `notes/ideas/`，实现事实进入 `notes/impls/`，可执行修复进入唯一的新 todo；Review 报告不直接改代码。
4. 发现高风险 finding 时暂停依赖该机制的后续 Review，先形成修复计划；普通 finding 可集中到后续修复批次。
5. `kernel-final-architecture-review` 与设备/中断 carryover 仍受触发条件约束，不提前伪造完成。
6. Reviewer 失败、超时或额度中断均记为“未完成”，不得据部分输出生成正式报告；失败批次进入下轮待审集合并保留原优先级。
7. 每次 reviewer 委派必须自包含：明确项目架构、审查纪律、提交边界、历史清单、目标输出、禁止事项和验证权限；不得依赖会话继承。

## 审查批次与顺序

### 批次 A：统一内存事务核与公共 MemoryObject（P0）

**状态：已完成，不通过。** 报告：[`review-2026-09-memory-transaction-unification.md`](review-2026-09-memory-transaction-unification.md)。报告本身承载四项 P1 的修复与复核计划。批次 B–E 需避开这些共享事务/对象 seam 的修复依赖，或在 findings 修复后重新取证。

目标提交：`d2ff81e`、`5c0bbb0`、`d6a162c`；前置对象投影提交 `51b3742`、`0fad27f`、`310d089`、`0a4eacb`、`81b5b3e` 只在相关 seam 被引用时取证。

合并来源：

- `archived/todo-2026-09-memory-transaction-unification-review.md`
- `archived/todo-2026-09-object-projection-review.md`

重点：WritePermit 守恒、view owner/permit 析构顺序、锁阶、Commit 零分配、AddressSpace-owned 与 object-owned view 撤销闭包、容量自洽、seal/EXECUTABLE 与跨进程 view 的验证缺口。

报告：`plans/review-2026-09-memory-transaction-unification.md`。

### 批次 B：内存供给、Pool、funded owner 与页表生命周期

**状态：已完成，不通过。** B-1 报告 [`review-2026-09-memory-supply-and-pool.md`](review-2026-09-memory-supply-and-pool.md) 发现 1 项 P1、3 项 P2；B-2 报告 [`review-2026-09-process-bind-page-table-retire.md`](review-2026-09-process-bind-page-table-retire.md) 发现 1 项 P1。两份报告本身承载后续修复与复核计划。B-1 由 `SlowJuniper`、B-2 由 `CosmicPussy` 通过 mesh 独立只读完成。

按提交依赖顺序分为同一统筹批次下的窄报告，避免把九份重叠清单重复审查：

1. B-1（`SlowJuniper`）：平台供给与系统储备 `198e665`、`0a944c7`；MemoryPool 与 funded frame broker `4715f3a`、`48227c8`；
2. B-2（`CosmicPussy`）：ProcessBind/bootstrap 与 funded owner 页表生命周期 `7c76097`、`c522e50`；deferred retire 与页表资金化事务 `7225673`、`cfad6cf`、`addb4a5`、`b4bfb20`。

合并来源：

- `archived/todo-2026-09-platform-memory-ledger-review.md`
- `archived/todo-2026-09-system-supply-reserve-review.md`
- `archived/todo-2026-09-memory-pool-review.md`
- `archived/todo-2026-09-funded-frame-broker-review.md`
- `archived/todo-2026-09-process-memory-binding-bootstrap-review.md`
- `archived/todo-2026-09-funded-owner-page-table-lifecycle-review.md`
- `archived/todo-2026-09-deferred-retire-review.md`
- `archived/todo-2026-09-page-table-funding-transaction-review.md`
- `archived/todo-2026-09-02-memory-page-table-6d-review.md`

报告：

- `plans/review-2026-09-memory-supply-and-pool.md`
- `plans/review-2026-09-process-bind-page-table-retire.md`

### 批次 C：线程生命周期、Remote Call 与用户内存联合审查

**状态：已完成，不通过。** C-1 报告发现 3 项当前有效 P1、3 项 P2；两项历史 P1 已由后续 `98d2449` 修复。C-2 报告发现 1 项 P2、2 项 P3。报告本身承载 findings 的修复与复核计划；交叉范围和已知 A/B findings 已去重。

合并以下重叠范围：

- `d741880`、`bdc83ef`、`004cae5`：线程成员表、teardown barrier、ThreadSpawn/join、末线程终局；
- `6199985`、`004cae5`：Remote Call 固定槽、RVWMO、epoch、stale translation 与 retire；
- 用户内存切片 `1cd6ab2` 至 `9358963`、`bdc83ef`、`004cae5`；
- `fcbd5b6`、`b161163`：持久 init/pm 委托域；
- `1d7dc92`：调度域 eligibility 与 D64。

合并来源：

- `archived/todo-2026-08-28-thread-teardown-review.md`
- `archived/todo-2026-08-30-remote-call-review.md`
- `archived/todo-2026-08-30-user-memory-mapping-review.md`
- `archived/todo-2026-08-28-persistent-init-review.md`
- `archived/todo-2026-08-28-domain-eligibility-review.md`

报告：

- C-1：`plans/review-2026-09-lifecycle-and-scheduling.md`
- C-2：`plans/review-2026-09-remote-call-user-memory.md`

同一提交 `004cae5` 的线程、地址空间和 Remote Call 交错已由统筹会话去重；两个主代理无需直接沟通。

### 批次 D：机制泛化与 BootPackage/launcher

**状态：已完成，不通过。** D-1 报告发现 2 项 P1、1 项 P2；D-2 报告发现 6 项 P1、1 项 P2。两份报告本身承载 findings 的修复与复核计划；D-1 由 `PaleBear`、D-2 由 `StormyPine` 通过 mesh 独立只读完成。

- 机制泛化：`15c7811`、`9c03251`、`95deea6`，只审 Lock Ladder、per-hart Timeout、MappingLease 三个尚未被吸收的代码轴；公理层和文档自洽不重复审查。
- BootPackage/launcher：`29c6519..1bc83ac`，机制层既有报告不重复，补十切片代码审查。

合并来源：

- `archived/todo-2026-08-27-mechanism-generalization-review.md`
- `archived/todo-2026-08-26-bootstrap-launcher-review.md`

报告：

- D-1：`plans/review-2026-09-mechanism-generalization.md`
- D-2：`plans/review-2026-09-bootstrap-launcher.md`

### 批次 E：系统审计分片 3–7

**状态：已完成首审，不通过。** E-1 报告发现 3 项 P1、1 项 P2；E-2 报告发现 1 项 P1、1 项 P2。两份报告本身承载 findings 的修复与复核计划；E-1 由 `BrightDick`、E-2 由 `IndigoMagpie` 通过 mesh 独立只读完成，交叉 findings 与 A–D 已知问题已去重。

`todo-2026-08-system-audit.md` 保留为详细审查规范。分片 1、2 已有归档报告；分片 3–4 与 5–7 分别由 `review-2026-09-system-audit-03-04.md`、`review-2026-09-system-audit-05-07.md` 承载首审结论与后续复核。本批次不重复已经完成的分片 1、2。

### 批次 F：触发条件审查

**状态：暂缓。**

- 设备/中断 carryover：设备、中断、DMA 接入设计开始时，按 `todo-2026-08-26-review-carryover.md` 的唯一条目执行。
- 内核最终架构：MemoryObject 主线、多页 Tunnel、Runnel v2、主要用户态消费者全部完成，且 A–E 基础审查收口后执行。对应计划的观察点在触发前只登记，不提前重构。

## 未完成委派记录

| 日期 | 批次 | 模型 | 结果 | 后续 |
|---|---|---|---|---|
| 2026-09-05 | A：统一内存事务核与公共 MemoryObject | `yanproxy-vip/openai/gpt-5.6-sol` | provider `openai_error`，无可用报告 | 已以新模型重试 |
| 2026-09-05 | A：统一内存事务核与公共 MemoryObject | `moeflux-openai-responses/gpt-5.6-sol` | 完成，报告判定不通过，四项 P1 | 后续由该 review 报告承接 |

## 当前 HEAD findings 状态复核（基线 `11fde56`）

本轮先尝试委托两个 explorer 做只读状态测绘，但均因账户周配额耗尽失败；没有可采纳的子代理输出。以下状态由统筹主代理依据当前代码、git history 和现有报告手工核对，属于修复前的整理，不修改代码。

| 报告/ finding | 当前状态 | 依据与后续归属 |
|---|---|---|
| A：WritePermit rollback 泄漏 | 仍存在 | `proc.rs` rollback 仍未统一 `take_permits/cancel_write`；留在 A 报告 |
| A：同对象 retiring owner 重复移除 | 仍存在 | `release_view_region`/batch 去重未见收口；留在 A 报告 |
| A：post-Commit retire `Vec::push` | 仍存在 | retire 容器仍需核对前置容量；留在 A 报告 |
| A：MemoryObject 缺 EXECUTE | 仍存在 | shared Rights 与 RX required rights 仍缺独立位；留在 A 报告 |
| B-1 F-1：DT status unknown | 仍存在 | `dtb/memory.rs` 仍同时接受 `ok/okay`；留在 B-1 报告 |
| B-1 F-2：FramePool checked arithmetic | 仍存在 | metadata 加法边界仍待 checked hardening；留在 B-1 报告 |
| B-1 F-3/F-4：Drop/query panic | 仍存在 | MemoryPool Drop 与 SystemSupply ticket query 仍需统一错误边界；留在 B-1 报告 |
| B-2 F-1：bootstrap post-commit 可失败窗口 | 仍存在 | 当前 `launch_bootstrap` 在 `handles.commit` 后仍 `attach_thread`；留在 B-2 报告 |
| C-1 P1-01/P1-02：监督 authority/静默降级 | 仍存在 | `srv_init` 仍在失败时移除 control/继续启动缺失服务；留在 C-1 报告 |
| C-1 P1-03：一次性 steady_state | 已修复 | 后续 `98d2449` 永久循环；仅保留历史证据 |
| C-1 P1-04：隐式 quiescent shutdown | 已修复 | 后续 `98d2449` 显式 SystemReset；仅保留历史证据 |
| C-1 P1-05：旧 raw-hart shift | 已修复/合并 | 当前 `send_ipi(1, raw)` 已取代旧 shift；不再作为独立当前债务 |
| C-1 P2：q-only DT、ThreadControl CLOSED、无限监督等待 | 仍存在 | 当前 parser/signal/supervisor 仍需修复；留在 C-1 报告 |
| C-2 F1/F2/F3：Remote token/epoch/UserStack | 仍存在/待核验 | 当前接口仍缺 table identity、epoch 边界和统一 Drop 策略；留在 C-2 报告 |
| D-1 P1-01/P1-02：旧 WaitContext/期限注销 | 已修复 | 当前 `TimeoutRegistration`、`finish_installing`、TimerQueue cancel 已接入；留历史证据 |
| D-1 P2：MappingLease 验证边界 | 仍为验证缺口 | 未发现已证实新死锁/泄漏；留在 D-1 报告 |
| D-2 F-01：payload owner | 已修复 | 当前 `BootFundedExtent/BootBorrowed/install_bootstrap_funding` 已接入 |
| D-2 F-02：reservation holes/overlap | 仍存在/待核验 | `frame.rs` reservation normalize 仍需独立修复核对；留在 D-2 报告 |
| D-2 F-03：sifive_u memory 参数 | 已修复 | 当前 Justfile 按 MODEL 选择 128M/1024M |
| D-2 F-04：ProcessCreate Control/Job membership | 已修复 | 当前 `process::create` 已预构造 Control 并写入结果；旧 finding 需标历史修复 |
| D-2 F-05/F-06：ELF entry/PT_INTERP/flags | 仍存在/待核验 | 当前 audit/parser 仍需补 byte-range 与 unsupported header 核对；留在 D-2 报告 |
| D-2 F-07：reservation token wrap | 仍存在/待核验 | 当前 token 耗尽/代数策略仍需独立确认；留在 D-2 报告 |
| E-1 M3-1：Bound image 失败未 drain | 仍存在 | 当前 `spawn_from_elf` Bind 后 `?` 仍无显式失败收束；留在 E-1 报告 |
| E-1 M3-2/M3-3/M3-4：raw admission/Gate/slot order | 仍存在 | registry/rt/board 当前仍需 duplicate、Failed 广播和排序闭包；留在 E-1 报告 |
| E-2 E2-5-01：librpc reject Handle/ReplyPort | 仍存在 | 当前 framing reject 仍未 close handles/discard port；留在 E-2 报告 |
| E-2 E2-7-01：clippy/lint gate | 仍存在 | clippy 仍非 Justfile 静态门；留在 E-2 报告 |

该表是当前修复排序真值点；报告正文保留历史目标提交证据，不在本轮伪造“原提交已被当前代码修复”。

## 首审阶段收口与修复交接

- A–E 首审已完成；本轮不再启动新的首审 reviewer。
- 当前复核确认：历史 findings 不逐条打补丁。事务失败闭包相关 findings 统一延期至 [`todo-2026-09-memory-transaction-state-machine.md`](todo-2026-09-memory-transaction-state-machine.md)，按最终类型状态机整体重构；在触发条件满足前不引入半成品兼容修复。
- 启动/平台 admission 相关 findings 统一由 [`todo-2026-09-admission-fail-closed.md`](todo-2026-09-admission-fail-closed.md) 承接，按 canonical admission 与 fail-closed 机制整体收口，不拆成孤立修复。
- 生命周期监督相关 findings 统一由 [`todo-2026-09-supervision-authority-policy.md`](todo-2026-09-supervision-authority-policy.md) 承接，按 authority 保留、有限等待和失败升级状态机整体重构，不拆成孤立超时/日志修复。
- Capability/affine owner/error boundary findings 统一由 [`todo-2026-09-capability-owner-error-boundary.md`](todo-2026-09-capability-owner-error-boundary.md) 承接，按 ABI、消费式 owner 与 reject/discard 机制整体收口，不拆成零散 Drop 或权限补丁。
- Token/generation/epoch findings 统一由 [`todo-2026-09-identity-generation-boundaries.md`](todo-2026-09-identity-generation-boundaries.md) 承接，按身份域与统一耗尽策略整体收口；与事务、admission、capability 计划交叉处只保留各自 owner，不重复造凭据机制。
- 所有未闭合 findings 继续留在九份根目录 `review-*.md`，报告同时作为修复计划、验证清单和后续 diff review 规范。
- 后续修复 agent 应先以当前 HEAD 重新核对报告中的 findings 状态，再按依赖顺序修复；已被后续提交修复的历史 finding 只保留证据，不重复实施。
- 修复后必须回到原报告逐条复核，完成 host/debug/release/QEMU/故障注入等对应验证；全部闭合后才将报告移入 `plans/archived/`。
- 批次 F（设备/中断/DMA carryover、内核最终架构 review）继续等待各自触发条件，不属于当前修复交接范围。

### 建议修复顺序

1. 内核失败闭包：M3-1、B-2 bootstrap post-commit、A 的 WritePermit rollback/retire owner/capacity；
2. 启动与 SMP 准入：M3-2、M3-3、M3-4；
3. 用户态 authority 与协议：C-1 监督 findings、E2-5-01、A 的 EXECUTE capability；
4. 供给/错误边界/工程化：B-1 findings、C-2/D-1/D-2 仍有效 findings、E2-7-01 及专项验证缺口。

### 委派失败记录

本轮 explorer 状态复核委派因账户周配额耗尽失败，无可采纳输出；状态矩阵由主代理依据当前代码和 git history 手工核对。该失败不影响首审完成状态，也不构成新的 Review 结论。

批次 A–E 的未闭合 findings 分别由九份根目录 `review-*` 报告承接，不重复进入下一轮首审列表；修复完成后按报告复核。

## 收口规则

每个批次完成时：

1. 报告逐项给出证据、可达前提、违反的不变量和后续归属；
2. 把设计/实现结论同步到相应 notes；只有 Review 范围之外的独立能力缺口才另建 todo；
3. 在本计划中更新批次状态、报告链接和未决项；
4. findings 未闭合的 `review-*` 留在 `plans/` 根目录并作为唯一行动真值点；全部修复并复核后移入 `plans/archived/`；
5. 不因 Review 通过而修改代码，不把“测试全绿”当作语义审查完成证明。
