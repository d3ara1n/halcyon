# 内核调用实现

方向见 [`../ideas/call.md`](../ideas/call.md)。系统调用 ABI 为 a7 调用号、a0–a5 参数，返回 a0 错误码与 a1 值。

## 同步调用

同步 handler 在一次 trap 内完成：结果写入 UserContext、sepc 前进 4，dispatcher 返回 Resume。输出地址先校验，副作用后的最终写回统一使用 `uaccess::deliver_output`；若同进程另一线程在两次 AddressSpace 临界区之间拆除输出映射，复检失败冻结调用进程 `(Fault, StoreAccess)`，不把已发生副作用与错误返回混合。

handler 返回 Completed 后，dispatcher 再检查 lifecycle。若 syscall 期间另一 hart 已冻结终止，或 deliver_output 触发 Fault，出口改为 Killed，不再返回用户态。

`SystemReset` 是终局同步调用：dispatcher 完成固定宽参数与 capability 校验后进入单次平台调用；成功按契约不返回，后端拒绝或异常返回才写入 syscall 错误并恢复调用线程。对象、SBI 映射与错误语义见 [`internals.md`](internals.md)「idle 与系统复位」。

## 异步调用

dispatcher 只登记 `WaitPlan` 到 HartLocal park 槽并返回 Wait。调度循环在当前线程离开执行点、清 active 后调用 `park_publish` 安装 `WaitContext`。

WaitPlan、WaitContext、TimeoutRegistration、订阅清理与 rejected-park 竞态由 [`ipc.md`](ipc.md) 唯一记录。调用层只区分两种交付：WaitMany 把结果或 MemoryNotAccessible 写回保存现场，Sleep 在相对超时到达时写回成功。终止 Abandoned 不交付结果。

dispatcher 的 Wait 出口不提前改为 Killed，终止竞态由等待安装路径吸收。

## Remote Call

`os/remote_call` 是 `no_std`、禁止 unsafe 的固定槽纯逻辑核心。内核为每个 admitted hart 配置 4 个槽；槽状态按 `Empty → Reserved → Pending → Taken → Empty/Retired` 单向转换，reservation 与 finish token 均不可复制，generation 在复用前前进，耗尽时永久退休以拒绝 ABA。Reserve 容量不足不发布请求；Commit 前取消精确归还；Pending 电平是工作真值，门铃合并、重复或单次失败都不丢请求。host debug/release 各 5 项测试覆盖容量、回滚、目标隔离、无门铃消费、乱序完成、generation 和调用者预算。

内核 `remote_call` adapter 用 `REMOTE_CALL` 锁秩（650）包装全局固定表。`ReservedBatch` 在业务 Commit 前一次取得完整目标集合；`publish` 只在 `ADDRESS_SPACE → LIFECYCLE → REMOTE_CALL` 正序内发布请求并返回 affine `Doorbell`，调用者释放业务锁后才执行 `FENCE RW,RW` 和 SBI IPI。slot 位图只在 registry 边界转换为 raw hartid；Remote Call 门铃失败返回失败位图并保留 Pending，不把已 Commit 业务改写成错误。目标在用户 trap 入口/出口和调度循环安全点每次最多处理 4 项，动作及最后完成回调均在槽锁外执行。

当前首个动作是地址翻译同步。请求携带稳定 AddressSpace identity、translation/instruction epoch 与失效范围；ASID 恒 0 的第一版保守执行本 hart 全量 `SFENCE.VMA`，instruction epoch 非零时再执行 `FENCE.I`，随后以 release RMW 确认。最后确认者通过同一原子 release sequence 的 acquire 侧调用业务 `Completion`；槽在动作与确认之后才复用。每 hart 另记最近完成的 identity/epoch，它不是 active 集合，只服务 execution gate 的 enter/leave 复检。

Process lifecycle 的 active 位图仍是执行成员唯一真值，并以单调 execution sequence 拒绝 active 离开后恢复同值的 ABA。dispatch 在 gate 外同步 epoch、gate 内复检后才登记 active；Requeue/Park/Killed 先在锁外消费 Pending，再在 gate 内确认已达当前 epoch 后清 active。AddressSpace 稳定外壳已提供 `prepare_shootdown → commit_shootdown → ShootdownSynchronization::start`：Reserve 快照 active 并预留全部槽，Commit 以 `ADDRESS_SPACE → LIFECYCLE` 复检、发布 PTE 闭包与新 epoch，锁外才 ring；空目标集锁外直接完成。启动探针分别验证全部 admitted hart 的运输/fence/ack，以及 primordial process active snapshot 下的真实 epoch shootdown；virt debug/release 与 sifive_u 均出现完成锚点并通过 10/10 竞态矩阵。

尚未接入的部分是具体 MemoryChange/HandleClose 对 `Completion` 的所有权、WaitContext park、发起线程消散、终止接管与 ack 后 Retire。它们需要统一 RegionLedger/backing 事务作为业务真值，将随 AddressSpace 调用者迁移闭合；当前基础设施不把空探针解释为公开 MemoryMap 完成语义。
