# 通用执行与准入固定提交 Review

> 【未来审查计划】固定对象为 `a3891b00c60acc0f91e964c183bb0eea7359404f`（`feat(runtime): 闭合通用执行准入与持久监督`），父提交 `2efbc87d816ad8ddfb061f8b2f04de254970b91f`。提交后登记，供未来独立只读审查使用；本次实现、完整验收和 R1–R16/C1–C7 修复复核已经完成，不阻塞下一机制。

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
