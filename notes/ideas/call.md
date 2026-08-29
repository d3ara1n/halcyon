# 内核调用

内核调用分为 System Call（用户线程进入内核）与 Remote Call（一个 hart 请求另一 hart 处理内核短动作）。两者都必须保持内核路径有界，不产生可睡眠的内核执行流。

## System Call

同步调用在一次 trap 内完成；普通操作把结果返回调用线程，终局操作成功后可以不再返回该系统实例。异步调用在入口验证参数和 authority、预留业务事务后，把调用线程转入 Waiting；内核立即调度其他线程，不在调用栈上等待事件。调用名称不决定完成形态：HandleClose 若撤销 object-owned mapping，也必须在远端地址翻译确认完成后才向仍存活的调用者返回。

异步完成由 WaitContext 表达：对象订阅、可选 Timeout 与完成动作竞争唯一 outcome。普通异步结果由完成者写入保存的用户现场并重新入队，线程恢复时从原 ecall 之后继续；终止取消则直接消散线程，不返回用户态。已经越过业务 Commit 的操作不能随 WaitContext 取消：线程只放弃最终回复，业务对象继续推进，且线程 departed/join 必须等待其结果记录解除挂接。等待和 Timeout 的公开语义见 [`wait.md`](wait.md)。

同步输出与异步等待结果具有不同交付边界：会返回的同步调用成功意味着结果已经写回；普通异步等待在 park 后不得依赖可能失效的用户指针。会创建进程资源且可能 park 的 `MemoryMap` 使用内存模型定义的提交前结果承诺：全部可失败工作完成后，以固定宽 UserWriteLease 稳定结果槽，先写 payload、最后 release 发布 cookie 并 Commit 业务；完成阶段只修改保存的返回状态。终局同步调用必须定义成功不返回及所有异常返回的语义。三者都不得因用户地址或参数错误 panic 内核。

## Remote Call

Remote Call 是 hart 间内核短动作的传输层，由 IPI 门铃、固定容量请求槽和可选完成通知组成。请求槽的 Pending 电平是工作真值，IPI 只提示目标 hart 检查槽，不携带业务载荷；门铃可以合并或重复，目标在每个 trap 安全出口都检查本 hart 固定槽。普通调度唤醒只需要门铃，不伪造 Remote Call 请求。

发起方在发布业务状态前预留全部目标槽；容量不足在业务 Commit 前返回忙，不建立无界请求队列。发布后，请求即由其业务所有者负责完成，发起线程终止只消散返回权，不能撤回远端已经可能观察的动作。平台 admission 预先保证 admitted hart 的 IPI 与周期性 trap 路径；Commit 后门铃异常只保留 Pending 等后续安全点补消费，不能改写为业务失败。请求合并只能保持每个原始请求的完成条件。

TLB shootdown 是首个消费者。页表写入方按[内存模型](mm.md)发布 PTE、epoch 与请求并在 IPI 前执行 data fence；目标 hart acquire 请求，执行本地 `SFENCE.VMA` 和可选 `FENCE.I`，再 release 确认。需要等待确认的 System Call 以 WaitContext park 发起线程，最后一个 acquire 确认者推进业务事务并完成等待；内核不在调用栈上等待远端 hart。Remote Call 不拥有地址空间事务、Handle close 或 backing；哪些 hart 必须参与、何时允许复用 backing 由内存模型拥有。

落地形态见 [`../impls/call.md`](../impls/call.md)。
