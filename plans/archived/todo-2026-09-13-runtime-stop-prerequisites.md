# 运行期协作停止前置

> 状态：已完成并归档。它是公共时间任务中新识别的前置，用户确认保持单 epoch 最大区间，仅承诺观测到时钟事实失效后的协作收束；不归入独立延期的验收可靠性任务。

## 目标与边界

启动 RuntimeGate 保持 Preparing→Ready/Failed，不增加 Ready→Failed。运行中观测到平台时钟失效或已无完整调度量子可表达后，使用独立静态停止锁存、现代 SBI IPI 门铃、各 hart 锁外安全点停驻；发布成本受 admitted hart 数限制，不随等待或 timer 数增长。非法期限/用户 duration 越界仍拒绝请求，不 poison 时钟。

停止不是 syscall 成功、对象退休或资源退款，不逐项取消 timer、不伪造 Timeout，不引入内核线程或堆上清理。持锁 panic/SBI fatal 的泛化停止不属于当前承诺；公共时钟只锁存错误并释放 guard，随后在统一锁外点收束。

固定证据：RISC-V counters/Zicsr/machine timer 与 SBI TIME/IPI/binary encoding。mtime 比较 unsigned、pending/CSR 反映不保证一 tick 处理时限。用户确认保持最大单 epoch 区间，只承诺**观测到事实失效后的协作收束**，不保证任意回退/停钟/回绕在所有 hart 睡眠时实时被探测，不减经验安全余量。实现与提交见 `notes/impls/time.md`；本前置纳入 `task/fal-service-capabilities` 的时间提交。

## 最终结构与自然序

1. registry 在 HSM start 前 release 发布 immutable admitted raw IDs 与 slot mask；IPI 读不再取 registry 锁，同一快照服务现有门铃与停止。
2. 静态不可逆运行停止状态，第一次发布发门铃，重复发布幂等；门铃失败不撤销真值、不递归 require。诊断与 park 在锁外。
3. scheduler 循环/重试、trap 入口/直返出口、idle 睡前醒后及启动等待消费停止状态，检查早于普通债务。所有真实调用者接通，不能只增加一个未消费 bool。
4. 时间任务接 validated tick/snapshot、固定 epoch guard、健康尾部量子截取；所有 raw 消费者迁移，平台事实错误在锁外触发停止。
5. 正式 debug binary 外部 GDB 在 Ready 后注入锁存的时钟失效，捕获全 hart 停驻与无用户继续执行；保留正常 core/platform/release与启动失败门，源码 reviewer 复核锁阶/发布/直返路径。

## 完成证据

- `runtime_stop.rs` 使用独立静态 REQUESTED/STOPPED/PARKED；Ready gate 不回退。首个观察者读取已发布 admitted raw ID 快照并发送现代 SBI IPI，门铃失败仅记录 mask；发送前后不持 registry/object/timer 锁，不清理 timer、不伪造 Timeout/资源退款。
- scheduler、用户 trap 入口/出口、idle 睡前/醒后、启动等待和 Online→Ready 交界均消费停止状态；停止前清本 hart SSIP，避免自 IPI 让 WFI 忙等。调度量子无法表达时经 request→安全点 park，不在 timer lock 内停驻。
- `registry.rs` 在 Release 发布 admitted mask 前写入 ID，调用方 Acquire mask 后读取槽；slot/index 与命中 ID 有 debug assert。raw `read_time` 仅留初始化与 ClockState sampler，其余期限/到期消费者统一走 validated ticks。
- 正常 `virt` core、virt-release、virt-nofd、128MiB `sifive_u` core、七面 clippy、os/shared host 通过。外部 GDB probe 在 Ready 后注入 debug stop request，观察 `debug_runtime_stop_parked_mask()` 为 `15`，4/4 hart 位于 `hart::park`；probe 因主动收割 guest 返回 1，不能按普通验收成功解释。日志 `artifacts/check/time-runtime-stop-{probe,gdb}.log`。
- 本闭包不证明任意硬件回绕/停钟会被实时发现，不恢复 Ready→Failed，不把在途 timer 逐项转成 Timeout；平台事实失效被观察后才保证协作停止。持锁 fatal 的更强语义另案。

## 完成门

无新 ABI、无临时 adapter、无持锁停止点；无锁 raw ID 发布与所有安全点齐备、时钟真实调用者组合、停止与正常运行证据、已有 findings 复核同时满足。本任务已完成并可归档；它不完成运输/RPC/服务执行或 FAL，后续持锁 panic 等能力若需要扩展应另行完整设计。
