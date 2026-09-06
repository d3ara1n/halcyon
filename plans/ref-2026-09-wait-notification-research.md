# 等待命中与通知交付：固定版本参照

本篇为只读取证资料，不拥有 Halcyon 的设计或实施任务。检索范围来自 `references/systems/INDEX.md`；下列结论仅限列出的固定文件，不推断整个系统都具有或缺少某项性质。

## Zircon：置位与观察者命中必须共同线性化

固定提交：`ee347841701cd7d51148a7cae48d16b51e321e03`。

[dispatcher.h](https://fuchsia.googlesource.com/fuchsia/+/ee347841701cd7d51148a7cae48d16b51e321e03/zircon/kernel/object/include/object/dispatcher.h) 的 `get_lock` 注释给出不变量：

> When not held, there must be no observers_ matching any of the active signals_.

同文件在 `signals_` 注释中区分清位与置位：清位不触发观察者；置位和通知命中者必须相对于添加、移除、取消观察者表现为原子操作，并在整个期间持有 `get_lock()`。`NotifyObserversLocked` 的签名也要求该锁。它支持的是电平观察，不代表只要同一信号稍后变回假，就能抹去已经登记的命中。

本文件同时明确 `AddObserver` 的 handle 销毁取消规则。这与 Halcyon「已解析并保留授权的等待不因原 Handle 关闭而撤销」不同，不能直接复制取消和引用关系。

`SoloDispatcher` 的锁说明承认最坏临界区可能较长，使用 CriticalMutex 保持响应性；此证据不是 Halcyon 协作式短路径的工作预算证明。

## seL4：单目标 signal 与批量取消不是同一复杂度

固定提交：`6e7c3b733d296cfd88d5fbf635c96e447a882374`（16.0.0）。

[notification.c](https://raw.githubusercontent.com/seL4/seL4/6e7c3b733d296cfd88d5fbf635c96e447a882374/src/object/notification.c)：

- `sendSignal` 在 Waiting 分支摘一个队头 TCB；Active 分支按 OR 累积 badge。
- `receiveSignal` 在 Active 分支交付 badge 并转为 Idle；这是消费式 Notification，不是非消费式 WaitMany 广播。
- 队列操作使用 TCB 节点，不在这些函数里临时分配观察者节点。
- `cancelAllSignals` 在 MCS 与非 MCS 分支均遍历等待队列。因此不能从单目标 signal 推断批量取消也是常数工作，更不能把线程的 MCS 执行预算等同于内核通知预算。

## managarm：延续队列分离上下文，但排水仍需另证上界

固定提交：`9438b5362b3f0fed86584a64e61ecbba2dd0fb48`。

[work-queue.cpp](https://raw.githubusercontent.com/managarm/managarm/9438b5362b3f0fed86584a64e61ecbba2dd0fb48/kernel/thor/generic/work-queue.cpp)：

- `WorkQueue::post` 接收已存在的 `Worklet*`，根据 executor context 与 IPL 选择本地、IRQ 本地或锁保护的远端队列；由空转非空时调用 `wakeup()`。
- 同上下文且 IPL 允许时 `post` 可以直接 `run()`；运行中的 worklet 再 post 时使用前插。
- `run()` 转接队列、提高 IPL 后，以 `while (!_pending.empty())` 调用 worklet；本函数没有步数或时间预算参数。

这证明延续责任可以与发布者调用栈分离，但不能证明「使用 WorkQueue」本身就满足一次安全点的固定工作预算。

## 使用边界

可用于设计检查的问题是：命中事实在哪一刻保留、谁保活等待对象、完成节点何时付费、实际回调在哪个上下文，以及每层循环是否有独立计费。三个参照都不能替 Halcyon 证明其所有权、失败出口与有界完成闭包。
