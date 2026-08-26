# 对象信号与 Notification

事件面分为两层：[ObjectSignals](wait.md) 是附着在对象上的非消费式电平状态；**Notification** 是用于 OR 合并且必须显式消费的独立对象。两者都经 Handle、lifecycle role 与 rights 使用，等待统一由 `WaitMany` 完成。

## ObjectSignals

可等待对象把当前条件公开为位掩码。典型位义如下：

| 对象 | 状态 | 含义 |
|------|------|------|
| 邮箱 | `READABLE` | 至少有一条完整消息可 Receive（owner 观察） |
| 邮箱 | `WRITABLE` | 占用低于容量，Send 可推进（sender 观察） |
| 隧道端点 | `DATA` | 对端提交了状态改变提示 |
| 隧道端点 | `PEER_CLOSED` | 对端已关闭，页内内容不再有对端保证 |
| 任意对象 | `CLOSED` | 对象已进入不可复活的终态 |

状态是条件，不是等待者私有的交付记录：多个等待者可同时观察同一位，WaitMany 返回绝不清除它。拥有语义的一方显式置位或清除；每种对象都必须定义清除前提。`CLOSED` 和 `PEER_CLOSED` 是持续可见的终态位。

WaitMany 以 `{ handle, signals, cookie }` 项观察组合对象，结果以 cookie、观察状态、输入项索引和原因辨认来源；它不接受 ObjectKind、对象 id 或「自身对象」等平行寻址，也没有按结果隐式清位规则。

## Notification

Notification 显式创建、可等待。创建者获得唯一 owner Handle：不可复制或 TRANSIT，可在 ProcessStart 中直接 GRANT；owner 关闭即对象关闭。持有 `SIGNAL` 的 signaler 可以按授权 duplicate、TRANSIT 或 GRANT，并向对象提交位掩码；待决位按 OR 累积，至少一位待决时 `READABLE` 为真。

持有读取权者通过专门取走操作原子取得并清除所选位，未选择的位保留。这把「通知发生」和「由谁消费」分开：等待只负责唤醒，取走才改变 Notification 内容。Notification 无负载、无顺序、无计数；需要身份、次数或数据使用消息，需要高频共享状态使用隧道协议。

## 终止请求与运行时分发

进程终止请求可以由进程对象协议的状态位表达；它是协商请求而非强制回收。强制终结直接进入进程回收和 Handle drain。内核不向任意线程注入 handler 或改写用户执行现场。

用户态运行时若需回调，可由专门线程 WaitMany 于相关 Handle，在该线程普通调用栈上分发；程序也可显式检查对象状态。
