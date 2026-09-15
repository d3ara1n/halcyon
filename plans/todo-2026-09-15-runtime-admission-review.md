# 通用执行与准入固定提交 Review

> 【未来审查计划】固定对象为 `a3891b00c60acc0f91e964c183bb0eea7359404f`（`feat(runtime): 闭合通用执行准入与持久监督`），父提交 `2efbc87d816ad8ddfb061f8b2f04de254970b91f`。提交后登记，供未来独立只读审查使用；该提交的实现与完整验收已经完成；原 R1–R16/C1–C7 复核记录保留于档案。2026-09-15 用户要求审视多轮修补后的最终结构，本文件同时记录该固定快照的设计反馈；其中 PM 真实停止的旧完成声明曾被代码证据推翻；下述已授权清理批次现已补齐并通过集中复核。

## 范围

- libsrv 的泛型准入分类、任务/来源稳定身份、Gate、输入 FIFO、期限、失败退避、停止和退休退款；FAL 分类及既有调用点的必要迁移。
- Runnel 类型化观察与 SourcePlan 值接缝；登记、注销、generation 和终态的真实 Runtime 消费。
- libprocess 的 Observation、Process/Job 原机器恢复、同步门面和 JobDriver；错误阶段、进度与 authority 一并保留。
- init 的持久 RootSupervisor、组合等待、失败隔离与预备能力账本；pm 正式 JobDriver、最小预算执行与停止；相关验收与文档。
- shared/timer_queue 只增加预付载荷绑定接口；内核与 shared ABI 没有改动。RPC/Outbox、FAL 业务及内核公共操作重构不在本提交范围内。

## 复核重点

1. 任务、来源、期限和退休在最小预算及持续就绪下均前进；输入未消费、失败和停止的路径不丢失责任或重复退款。
2. Gate 正常/失败/Complete 共用任务归属校验；业务期限独立于 Hold/Retry，任务执行及 Gate 回调前兑现已到期义务，同一期限不会重复投递。
3. 已裁决 Ready 不因晚恢复变成超时；终态到期的一次非阻塞 probe 符合 ready 优先契约；实际注销确认先于 Drain/Close。
4. Process/Job 失败携带原机器、阶段、快照、进度和控制能力，恢复不重新枚举来替代旧 owner，也不重复执行已提交阶段。
5. 根监督在部分准入、运行、启动、发送和关闭失败时保留全部相关 owner，其他责任继续；Grant/Send 成功才兑现移交，活动 Job 借用的 root 不提前关闭。
6. 真实消费者使用最终机制，旧重复编排已删除；typed owner、资源分类边界、文档与固定提交一致。

## 已有证据

- Host：libsrv 26，libprocess 8 个单测及 4 个 Runtime 集成测试，librunnel 17，shared workspace 全部通过。
- `just check`、七面 clippy、core 与完整 `just acceptance` 通过。最终日志 `artifacts/acceptance-takeover-20260915-121051.log` exit 0，含 stress 16/16、release、sifive_u、nofd 及 panic/alloc/fatal 启动失败三线。
- `.sources.json` 的 31 个改动源码哈希已核对一致；完成时无 QEMU/GDB 残留。本机 artifacts 不随 clone 交付。
- 完整修复与复核历史见 [已归档报告](archived/review-2026-09-15-runtime-closure.md)，实现事实见 `notes/impls/runtime.md`。已关闭 findings 作为回归检查依据，不据此重复立案。

## 完成门

未来 reviewer 使用新上下文只读核对该固定提交，finding 必须给出位置、可达路径、影响与违反的契约。无 finding 后归档本计划；有实际 finding 时由唯一报告承载修复与复核，不复制已有问题。后续 RPC/Outbox 仍由[执行前置计划](todo-2026-09-13-service-runtime-prerequisites.md)拥有。

## 提交后的结构审视（2026-09-15，基线 d2d83de）

用户边界已澄清：当前服务进程全部属于内核和框架/标准库验收负载，没有正式服务实现；测试消息数量、阶段顺序、固定槽和直接装配均可按验收需要编写，不为其设计生产级服务架构。审视目标限于正式库中同一责任的平行实现、不存在的状态和不必要分配；服务改动只用于库接口适配和有效验收。既有 passing 测试约束应保留的行为，不决定当前库结构必须保留。以下记录审视时的代码事实，由直接读码与两份独立 advisor 核对；后续实施结果见已授权批次，不把本表的旧基线当作当前代码。后续选择与验证统一在本文件承接，不另建平行修复清单。

| 项 | 审视时证据 | 裁决与处理边界 |
|---|---|---|
| S1 生命周期与验收耦合／验证声明纠正 | `srv_pm/main.rs` 的 MailboxTask 在 `dispatched == 2` 后 Complete；Flow 成功发送 TAIL 后派生 Domain 并 Complete；Domain 设 domain_done 时也 Complete，main 此后才 seal/shutdown。init 先完成读取再发 Flow 请求，故正常路径没有 Active 任务消费 stop。 | 只补框架真实停止证据：“邮箱仍 Active 且有登记→消费 stop→注销→退款”。可继续使用现有测试消息和 Flow→Domain 阶段关系，不要求另建正式协调层或服务生命周期协议。旧报告中 R15 的 Runtime host 证据仍成立，PM 实际停止部分重新打开。 |
| S2 失败操作协议分叉 | `SuperviseTask` 将原失败 Collector 交 Sink 并 Complete；JobDriver 留机器于原任务，Root 按 task id 恢复。Root 因而维护结果扫描/重新准入和原地恢复两套编排，JobWorld 又复制 driver.failure。 | 仅按 libprocess 自身契约判断驱动与错误交付是否存在可消除的重复；保留原机器与显式移交能力。不以统一 Root 测试编排为理由强行改变 Process/Job 的失败语义，不预建统一操作框架。测试侧错误副本或恢复分支不是独立施工目标。 |
| S3 执行核心的残留表示 | Slot.task 只构造 Some，从不置 None；advance 的空任务重排分支不可达。Add/Arm 最终都是 handle/signals 登记但应用路径分开；每次 advance 都重新分配和记账固定 Requests。 | 存在的 Slot 直接持 T；登记内部统一为值计划；每 Runtime 预备并复用请求承载，Gate 持有期间不复用。删除空任务回退、平行登记与每轮缓冲分配/退款路径。保留任务 ID 查询失败、领域类型和失败 owner 返还。 |
| S4 Job 分页承载重复且随宽度增长 | JobFrame 有 members/children 两套 Vec 与游标/stalls；当前逐页累积全量列表后才处理，成员结束后不再读取其列表。父层只须跨子 Job 保留当前 child 页及关闭责任。 | 每 frame 一份预付页，处理完一页再取下一页，现有阶段决定成员类别。保留页内 position、枚举 next_cursor、more 与 stalls 的不同含义。删除完整列表累积、双份承载和增长分配；内存随深度及页容量而非各层全部成员数增长。 |
| S5 Root 装配与操作边界不清 | 按 Job/Batch/Read 硬编码两/两/一槽、分支和 cookie；verify_* 直接修改 Duty 私有状态。多 Runtime 的隔离能力存在，但每种任务必须单独成域没有独立证据。 | 取消对测试装配的架构化要求，不单列 Root 托管结构、动态执行域表或槽位泛化重构。固定槽、分支和直接状态操作可以保留，只核验其确实驱动正式框架并证明所声称的故障隔离与 owner 契约。 |
| S6 流操作交付与观察租约分散 | init StreamReadTask 与 pm StreamTask 重复登记失败、来源、注销与停止状态；流角色和结果经 Task/World/Root 的 Option 交接；InitTask 只有单个变体并纯转发。 | 库级观察登记寿命若能以现有真实消费者证明共性，可做必要收敛；不为消除测试代码中的 Option、单变体转发或可变访问，新增正式流操作框架。保留 Process 终态观察与可变流条件的语义区别；服务仅随库接口做必要适配。 |

### 依赖与验证边界

- S3 可独立收敛，保留预算 1、公平性、未消费输入、失败 Gate、业务到期顺序、归属校验和最终退款回归。
- S4 是独立分页闭包。ABI `shared/erhino_shared/src/proc.rs` 的游标为最后返回的单调 ID；`ordered_table::scan_visible` 按 key > cursor 并在未决占位前停住；Job 先 Seal。这为逐页收束提供依据。下一页错误时前页可能已收束，是执行顺序变化，必须保留原机器及真实进度。验证多页、嵌套、子关闭失败、条目并发消失、零进展占位、下一页错误恢复和派生前预付失败。
- 本批主范围为 S3 核心清理、S4 有界分页及 S1 框架停止补证，按此顺序推进，最后集中验证与 Review。S2/S6 仅纳入有独立库契约理由且可与本批闭合的机械消重，不把统一服务装配或预建操作框架作为完成门。S5 的测试架构重构要求撤销，不作为延期任务保留。
- 不删除必要状态：业务完成/退休完成、准备中/已准入 owner、观察结果/实际注销、业务期限/执行退避、根兜底/工作域隔离，均须有各自责任落点。
- 不重写 Runnel 数据协议或 ProcessDrain，不引入 RPC/Outbox、内核 ABI 改动、通用事务框架或测试专用替代运行体。本节保留设计审视及用户收窄范围的记录，实际实施结果见下节。

## 已授权清理批次（2026-09-15，已提交 5de2780）

用户确认按 Runtime 清理 → Job 分页 → 必要适配和停止补证推进；服务均为测试夹具，不实施 S5 或服务正式化。基线 `d2d83de`，保留既有 AGENTS.md 工作树改动。具体范围：

1. Runtime：现存 Slot 直接持任务；Add/Arm 公共入口共用同一登记操作；去掉仅为计划动态登记遗留的 SourceRegistrar；Requests 与其额度随 Runtime 预付并复用，唯一 Gate 只持结算状态。失败 Gate 的完整请求 owner、到期输入、来源归属和退款语义不变。
2. Job：每 frame 预付一份最大 ABI 页，直接枚举进该页。页内位置仅在该成员/子 Job 完成后推进；页耗尽且 more 才继续枚举；成员全部处理后复用该页处理 children。子 frame 页与栈位在派生前预备。错误、零进展及下一页失败不覆盖原枚举游标和未兑现 owner；更换政策时允许恢复耗尽的枚举尝试预算。
3. 验收：保留现有 pm 的测试阶段关系，仅令已登记 Mailbox 保持 Active 到明确 stop，检查 stop 被消费、运行体全部注销与关闭、账户退款。库变更只做必要测试服务适配，不统一服务失败政策，不引入新操作框架。
4. 完成门：受影响 host 回归先行，Job 新增多页/嵌套/消失/下一页失败与零进展/关闭失败/派生前 OOM 组合；just check、七面 lint、core 与完整 acceptance。全部实现和验证完成后做一次集中 Review，发现的问题集中修复。提交需另行授权。

### 本批完成证据

- Runtime 的直接任务存储、统一登记和预付 Requests 已完成；SourceRegistrar 及旧空槽回退已删除，正式 host 26 项通过。
- JobPage 已接单页消费和嵌套恢复，取消 members/children 全量列表及增长分配；验证枚举 ABI，replenish 恢复零进展预算。libprocess 原 8 单测、4 观察集成与新增 4 个执行恢复集成通过，librunnel 17 项通过。
- 新增 `execution_recovery.rs` 使用线程局部分配失败注入测试正式库，不替换 Runtime：拒绝全部新分配仍可执行 Gate、返还失败派生 owner、退休并退款；栈位与子页分别 OOM 都没有派生新 authority。多页成员/children、并发消失、嵌套子关闭失败、后续两类页失败和零进展续作均有可追溯断言。
- pm 仅保留 Active 邮箱到显式 stop，实际登记/注销回执与最终账户归零共同作为新 QEMU 必检锚点。`artifacts/cleanup-paging-virt.log` core 通过；`cleanup-paging-check.log` 和 `cleanup-paging-clippy.log` 分别为 just check 与七面 lint 通过。
- S2/S6 没有足以支持本批扩大正式接口的独立需求，保持当前库契约；S5 已撤销，均不以服务夹具的整齐为理由添加框架。

- 完整 `just acceptance` 已通过：`artifacts/acceptance-cleanup-paging-20260915-131912.log` exit 0，stress 16/16、release、sifive_u、nofd、panic/alloc/fatal 启动失败三线均通过。对应 `.sources.json` 的 7 个本批改动源码哈希一致，无 QEMU/GDB 残留；sub-4/sub-5 已完成一次集中只读复核，两者均无 finding；分别确认 Runtime 预付/Gate/owner 与 Job 分页/派生前预付/恢复/Active 停止契约。

本批 S1 停止补证、S3 核心清理与 S4 有界分页均完成，原 PM 停止证据缺口关闭；没有开放修复项。S2/S5/S6 按用户确认的测试夹具边界不构成本批待办，不另挂延期。正式实现四个文件合计净减少 86 行，新增测试不计入该数。生产源码与最终聚合哈希保持一致，文档完成状态同步更新。本批已提交 `5de2780fe4802f2bb31ddeeafccf87737ba80cc4`，未 push；固定差异复核见 [未来 Review](todo-2026-09-15-runtime-cleanup-paging-review.md)，本文件保留原始审视与本批证据。
