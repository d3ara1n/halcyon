# Review 统筹计划：已完成主线的只读审查

> 本计划是当前这轮代码 Review 的统筹入口。本次将现有重叠的 `todo-*-review.md` 整理并合并到批次 A–E；这只是当前积压任务的归并，不废止 `todo-*-review.md` 作为未来提交后的审查计划类型。归档不表示对应审查已完成。每个批次必须先生成正式 `review-<日期>-<主题>.md`，再将本批次标记为已收口。
>
> 审查模型：`moeflux-openai-responses/gpt-5.6-sol`。审查只读，不修改代码；发现问题只写 finding、notes 归属或新的修复 todo。
>
> 当前配额约束：本轮 `reviewer` 子代理账户已耗尽，不再继续委派 reviewer。后续 Review 优先使用用户直接开启的 mesh 主代理会话；`explorer` 仅做代码测绘和事实收集，`researcher` 仅做外部资料取证，二者不能替代 reviewer 或主代理进行 Review 判定和设计决策。该约束是当前工具状态，不改变未来角色定义。

## 当前基线

- 工作树基线：`2ed7e1e`（2026-09-05，统一事务核与公共 MemoryObject 切片收口）。
- Review 纪律：[`REVIEW.md`](REVIEW.md)。
- 代码、测试和实现文档均以提交范围为准，不以当前工作树的后续状态替代目标提交证据。
- Review 报告写入 `plans/review-<日期>-<主题>.md`；报告完成后才将本计划对应批次标为已收口。

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

**状态：待执行；不阻塞主线。**

- 机制泛化：`15c7811`、`9c03251`、`95deea6`，只审 Lock Ladder、per-hart Timeout、MappingLease 三个尚未被吸收的代码轴；公理层和文档自洽不重复审查。
- BootPackage/launcher：`29c6519..1bc83ac`，机制层既有报告不重复，补十切片代码审查。

合并来源：

- `archived/todo-2026-08-27-mechanism-generalization-review.md`
- `archived/todo-2026-08-26-bootstrap-launcher-review.md`

报告：`plans/review-2026-09-mechanism-and-launcher.md`。

### 批次 E：系统审计分片 3–7

**状态：待执行；在基础批次完成后按分片推进。**

`todo-2026-08-system-audit.md` 保留为统筹计划的详细审查规范。分片 1、2 已有归档报告；分片 3–7 分别生成独立 `review-2026-09-audit-<编号>-<主题>.md`。本批次不重复已经完成的分片 1、2。

### 批次 F：触发条件审查

**状态：暂缓。**

- 设备/中断 carryover：设备、中断、DMA 接入设计开始时，按 `todo-2026-08-26-review-carryover.md` 的唯一条目执行。
- 内核最终架构：MemoryObject 主线、多页 Tunnel、Runnel v2、主要用户态消费者全部完成，且 A–E 基础审查收口后执行。对应计划的观察点在触发前只登记，不提前重构。

## 未完成委派记录

| 日期 | 批次 | 模型 | 结果 | 后续 |
|---|---|---|---|---|
| 2026-09-05 | A：统一内存事务核与公共 MemoryObject | `yanproxy-vip/openai/gpt-5.6-sol` | provider `openai_error`，无可用报告 | 已以新模型重试 |
| 2026-09-05 | A：统一内存事务核与公共 MemoryObject | `moeflux-openai-responses/gpt-5.6-sol` | 完成，报告判定不通过，四项 P1 | 后续由该 review 报告承接 |

## 下一轮全体 Review 任务

1. 批次 D：机制泛化与 BootPackage/launcher；
2. 批次 E：系统审计分片 3–7；
3. 批次 F 继续等待触发，不计入当前可执行轮次。

批次 A–C 的未闭合 findings 分别由五份根目录 `review-*` 报告承接，不重复进入下一轮“待执行 Review”列表；修复完成后按报告复核。

## 收口规则

每个批次完成时：

1. 报告逐项给出证据、可达前提、违反的不变量和后续归属；
2. 把设计/实现结论同步到相应 notes；只有 Review 范围之外的独立能力缺口才另建 todo；
3. 在本计划中更新批次状态、报告链接和未决项；
4. findings 未闭合的 `review-*` 留在 `plans/` 根目录并作为唯一行动真值点；全部修复并复核后移入 `plans/archived/`；
5. 不因 Review 通过而修改代码，不把“测试全绿”当作语义审查完成证明。
