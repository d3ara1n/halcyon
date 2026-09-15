# Runtime 闭包修复与复核（2026-09-15）

> 当前状态：HighHolly 接手重构及集中修复已完成；R1–R16 与后续 C1–C7 均已复核关闭，最终完整验收通过；未提交。基线 HEAD 为 `2efbc87d816ad8ddfb061f8b2f04de254970b91f`。本报告记录 R1–R16 与后续 C1–C7 的修复与复核，不另建重复 todo。接手前的 passing 日志和修复候选不构成当前代码通过证据。

## 范围与接手

用户授权 HighHolly 接替 SilverSeal 实施必要重构。原工作树含其他协作者改动，未回退或覆盖无关工作；消息/流运输已提交的前置保持，RPC/Outbox、FAL 业务和内核公共操作重构不进入本批。

首审发现 Runtime 退休饥饿、停止破坏 FIFO、Collector 失败后状态残缺、失败包装不可恢复分配、观察阻塞/期限不闭合，以及 init 丢失原机器或停止监督。第一轮统一复核只确认 R1/R2/R7，其他候选仍存在对应可达问题。接手后从责任归属、观察协议和公平工作面统一实施；不延续字符串报错、retained 日志或临时 fallback 代替真实 owner 的路径。

## 接手后的最终结构

- **Runtime**：期限/维护/任务轮转，维护内继续独立轮转接收、来源退休、任务退休。任务输入为来源记录内的 FIFO，按预算弹出、未消费回存；无每次 advance 全来源扫描。SourcePlan 是普通 WaitSet 观察意图的不透明值，不需要动态 Box 或任意注册回调。
- **准入/退休**：任务、来源、Gate、scratch 和接收存储有预留；input_budget 推导装配额度。来源独立预付 TimerQueue 槽，任务另有执行失败退避槽；重试和等待不全表扫描，不以另一个来源成功抹掉失败项。shared/timer_queue 只补 value_mut 支持预付后绑定外部 token。
- **观察协议**：Observation 持有不复用身份、control、signals 和绝对 deadline，恢复区分 Ready、TimedOut、SourceError。恢复只改变状态，下一事件通过 step 取得。ObservationSlot 消费登记与注销回执，实际撤销后才交给机器推进关闭。
- **Collector**：从等待到 Drain/Verify/Close 一直保存原机器；Close 成功前保留快照，失败/停止按值交付机器。Job 单栈推进、派生前 fallible 预留，ProcessOperations/JobOperations 驱动正式实现和替换测试。同步门面共用状态机，执行任务内部无阻塞 wait。
- **RootSupervisor**：main 长期持有，脚本借用；services 的兜底机器、运行体与任务在启动子服务前准备为待命责任。Job/Batch 各两槽对应原操作和 services 兜底，Read 一槽，连同空闲集合共六个 WaitMany 目标。批次保存已/未准入任务、世界和全部结果；Job root control 保留到显式关闭；流建立和关闭失败也有预备清理槽。可恢复项退避、永久错误保留原类型，其他运行体继续。所有责任结束后才提交失败 reset，平台拒绝时管理根保留。
- **真实消费者**：pm 删除重复 Domain 编排，使用 JobDriver；实际 max_work=1，并用 seal/shutdown_turn 结束长期邮箱任务。init 同一期限覆盖 done→腾位→TAIL；root 用真实已启动服务检验部分准入/结果恢复，用独立 Job Runtime 检验跨运行体故障隔离。

## Finding 对照与完成门

R1–R16 均已复核关闭；集中复核新增的 C1–C7 及其关闭证据见下文。

| 编号 | 原问题 | 当前实现与验证落点 | 当前状态 |
|---|---|---|---|
| R1 P1 | Finalizing 经 stop 重入 ready，摘除后 panic | WorkQueue 限制生命周期调度、摘除前 unschedule；`finalizing_task_is_not_requeued_by_shutdown` | 原问题已复核关闭；当前回归通过 |
| R2 P1 | scan_visible 的首个不匹配阻断退休 | next_after 有界游标；可退休计数与来源账本维护；持续来源组合验证独立完成任务退款 | 原问题已复核关闭；当前回归通过 |
| R3 P1 | 接收/退休依赖剩余预算，max_work=1 饥饿；全表扫描未计费 | 独立持久轮转、输入 FIFO、预付期限堆；`persistent_source_gate_retirement_and_stop_refund_with_one_work_unit` | 已实施并复核关闭 |
| R4 P2 | removing RetryAt 忙循环或被别的成功项清除 | 每来源独立期限槽、O(1) 最近期限；Busy 实际 wait 与双来源退避保留测试 | 已实施并复核关闭 |
| R5 P1 | Escalate/Close 失败丢 Process 阶段或 snapshot | CollectorFailure 一直持原 Collector；ProcessOperations Close Busy/恢复；真实 Runtime 集成断言不重复 Drain/登记 | 已实施并复核关闭 |
| R6 P1 | 分页/扩栈/错误包装依赖不可失败分配 | Job frame/page fallible reserve；失败按值返回；Runnel 观察计划改值类型；无入口/错误 Box::new | 已实施并复核关闭 |
| R7 P2 | child 准备失败后错误进入 CloseChild | frame 和栈位先准备再派生；只从 child Done 回退关闭，nested Close Busy 恢复 | 原问题已复核关闭；当前回归通过 |
| R8 P1 | 持久电平绕过 RetryAt，timeout 输入反复调度 | 机器保留绝对重试，执行退避不伪造业务 timeout；连续 Drain Busy 真实 Runtime 测试 | 已实施并复核关闭 |
| R9 P1 | step 内阻塞；Observe 缺期限、身份、错误/恢复结果处理 | 统一 Observation/ObservationSlot；登记与注销回执；单次等待、错误身份、超时、登记错误、注销 Busy 的测试 | 已实施并复核关闭 |
| R10 P1 | helper/race 丢原 Job 机器，新建机器不能接回已摘除 control | 所有相关调用借用 RootSupervisor 的原 Job duty，Job root 留到账本显式 close；旧 bool/字符串仅传诊断 | 已实施并复核关闭 |
| R11 P1 | partial spawn/run/首错丢在队任务或结果 | Batch 保存全部 owner，准入失败仍推进已入队任务；真实容量拒绝+失败/健康成员+原机器恢复验收 | 已实施并复核关闭 |
| R12 P1 | init 退出或转入无关等待；首个失败阻挡其他责任 | 预备 services 待命机器、组合等待、独立任务/运行体失败处置；真实跨 Runtime 隔离锚点 | 已实施并复核关闭 |
| R13 P2 | close 报错但丢返回 owner，或仅依赖 Drop | Runtime/Read/Job root 关闭失败放回原槽；流 setup Endpoint/Invitation 有预备清理槽；pm 失败在 owner 存活时非零退出 | 已实施并复核关闭 |
| R14 P2 | done 有限、TAIL 仍无限；pm spawn 失败成功退出 | done 和逐条接收共用绝对 deadline；pm 初始准入/执行/关闭失败均非零终态 | 已实施并复核关闭 |
| R15 P2 | 只有单项 Gate 测试，无真实 stop/组合退款 | 持续来源+Gate+独立退休+停止+Busy+额度归零 host 组合；pm 实际 max_work=1/shutdown；root 两个必检故障隔离锚点 | 已补组合证据并复核关闭 |
| R16 P3 | 公共操作专题状态及实现文档矛盾 | 更新 COMPASS、执行前置计划、ideas/framework、impls/runtime；公共操作仍待实施 | 已修正文档并复核关闭 |

## 验证证据

接手阶段已执行：

- `artifacts/takeover-host.log`：libsrv 24 项；libprocess 6 项单测与 3 项 Runtime 集成测试；librunnel 17 项（最终次数与输出以当前日志为准）。
- `artifacts/takeover-virt-isolation.log`：core 完成两个 root 故障隔离锚点、pm 实际停止、Pool 退款与 Requested reset。其后若有生产改动，以最终聚合日志覆盖。
- `artifacts/takeover-clippy.log` 及 `artifacts/lint/`：分面静态检查；最后聚合将再次执行统一门。
- 本轮初次故障隔离测试把准入失败预期写成表限，但实际账户先返回 QuotaExceeded，失败日志 `artifacts/failed-acceptance-20260915-095727-85870.log` 保留。修正为检验真实账户拒绝后，故障隔离 core 已通过；该失败不作为通过证据。

首轮接手后聚合 `artifacts/acceptance-takeover-20260915-103842.log` 通过七面 lint、stress 16/16、release 与 sifive_u，随后 nofd 在既有 `time_checks::delivery_deadline` 的 30ms 成功提交假设上收到合法 DeadlineExpired 并 panic；完整现场 `artifacts/failed-acceptance-20260915-104212-99798.log`，聚合退出 124，源码哈希与启动时一致。该路线不算通过。时间验收改为给成功探针两个 QEMU 节流周期的窗口，并在有限的新尝试中逐次核验提交前过期不消费 once/move；不修改内核期限语义。后续聚合采用修正后的快照。

集中复核前的聚合 `just acceptance` 已通过：`artifacts/acceptance-takeover-20260915-104931.log` 末尾 `[acceptance exit code: 0]`，包含七面 lint、stress 16/16、release、sifive_u、nofd、panic/alloc/fatal 启动失败注入。对应 `.sources.json` 记录改动源码 SHA-256，结束后核对无变化；无残留 QEMU。`takeover-host.log` 的 libsrv 24、libprocess 6+3、librunnel 17 项与 `takeover-shared-host.log`、`takeover-check.log` 均通过。该快照用于接手后的集中独立复核，后续修复与最终证据见下文。

接手后统一复核确认 Runtime 公平维护、输入 FIFO、原机器恢复、RootSupervisor 故障隔离及真实 stop 已成立；新增的明确缺口集中为下列一组，已在同一闭包修复并完成定点复核：

| 编号 | 复核问题 | 修复与新增证据 |
|---|---|---|
| C1 P1 | 失败 Gate 用 None 覆盖业务期限 | 业务 timer 独立更新，Gate 结束采样任务当前 deadline；Hold/Retry 仍取得已到期业务输入的回归 |
| C2 P2 | Rearm/Remove 没有校验请求任务 | 三类 Gate 统一拒绝其他任务来源，双任务共享 SourceId 的负路径回归 |
| C3 P1 | Ready 在晚恢复时被改判 timeout；记录尚未接收就注销 | 保留已裁决 Ready；持久 Process/Job 信号到期时一次非阻塞 probe；等于/晚于 deadline 与 max_work=1 排队记录+注销 Busy 回归 |
| C4 P2 | 错误 stage 与机器真实阶段不一致 | Process/Job 以 stage/fail_current 生成错误；Verify 阶段基础设施失败仍报告 Verify |
| C5 P2 | 等待次数恒零、Job 失败遗漏当前成员工作 | 新 Observation 计一次等待；Job 失败快照叠加当前 Process 工作，不提前并入累计；部分 Drain 失败/恢复计数回归 |
| C6 P2 | pm sender、辅助 Job controls 和 startup/mailbox 早期 owner 未入根 | 工厂先预备能力槽再 create/duplicate，typed Sender 长期借用，Grant/Send receipt 后移交；真实 startup duplicate 拒绝、最终辅助能力基线锚点 |
| C7 P3 | 文档对完整 owner 声明超前 | 根账本覆盖补齐后同步实现说明，不以 reset/Drop 掩盖常驻 entry |

修复后的 host 记录见 `artifacts/takeover-final-fixes-host.log` 与 `takeover-process-final-fixes.log`，当前 libsrv 26、libprocess 8+4；`takeover-final-fixes-clippy.log` 七面通过，`takeover-final-fixes-virt.log` core 与新增辅助能力收口锚点通过。第二次聚合 `artifacts/acceptance-takeover-20260915-114827.log` 已以 exit 0 完成：七面 lint、debug stress 16/16、release core、sifive_u core、nofd、panic/alloc/fatal 启动失败注入全部通过。`.sources.json` 的 31 个改动源码文件 SHA-256 与验收结束后工作树一致；无残留 QEMU/GDB。C1/C2、C3–C5、C6/C7 分别由 sub-12、sub-13、sub-14 定点复核，不重复已关闭结构。

定点复核中 sub-12 确认 C2、sub-13 确认 C3–C5；sub-12 另指出 C1 的 `Retry < deadline < now` 次序仍可能先执行再抹掉期限，sub-14 指出 C6/C7 尚漏 seal/create 临时 JobControl 副本的发送失败分支。两项集中补齐：

- `mature_deadline` 记录未交付的期限义务，在任务执行和 Gate 回调前定点兑现，只操作该任务，不要求先清空全局到期堆。原回归新增 `10 < 50 < 100` 次序，并在取得 timeout 后改为 INFINITE，下一轮断言没有重复 timeout。
- `race_seal_create` 两类 duplicate 进入 Root 工厂，发送成功才 transferred；失败直接返回，原副本与 child 保留在账本。真实 startup fixture 新增向已关闭 mailbox 发送 JobControl 的失败，然后显式成功关闭原 control，证明内核与根均保留 owner。

补齐后的 `takeover-final-ordering-host.log`（libsrv 26、libprocess 8+4）、`takeover-final-ordering-clippy.log` 七面、`takeover-final-ordering-virt.log` core 全部通过。最终聚合 `artifacts/acceptance-takeover-20260915-121051.log` 已以 exit 0 完成七面 lint、stress 16/16、release、sifive_u、nofd、panic/alloc/fatal 启动失败三线；对应 `.sources.json` 的 31 个改动源码哈希核对一致，QEMU/GDB 无残留。`takeover-final-check.log` 的 just check 退出码为 0。

sub-15 已定点复核并确认 C1、C6/C7 关闭：到期通知不会被较早的 Retry 绕过，不重复 arm 或投递；两个竞态副本发送失败仍在根账本，成功时只移交一次。结合 sub-12 的 C2、sub-13 的 C3–C5 和此前对 R1–R16 的复核，本报告没有开放 finding。复核只检查指定修复，没有重开首审。

未执行 commit、push 或合并。

## 保留边界

同步 collect/job 门面用于顺序上下文，内部共用正式机器；刻意 raw 的内核契约验收保持顺序。异常 Drop 只兜底进程放弃，不作为正常清理证明。永久失败允许管理根保留完整 authority 和错误状态，不承诺无授权地修复权限或内核故障，但不得阻断其他责任。公共操作、RPC/Outbox 与 FAL 后续能力继续由各自唯一计划拥有。
