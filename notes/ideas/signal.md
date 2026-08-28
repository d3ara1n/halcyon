# Notification

Notification 是按位 OR 合并、由消费者显式取走的独立对象。它适合无负载、允许合并的通知；需要身份、次数、顺序或数据时使用消息，需要高频共享状态时使用 Tunnel 协议。

## 角色与待决位

创建者获得唯一 owner Handle。owner 不可 duplicate 或 TRANSIT，可通过 ProcessStart 直接 GRANT；owner 关闭使对象进入 CLOSED。获授权的 signaler 可按 rights duplicate、TRANSIT 或 GRANT，并提交位掩码。

待决位按 OR 累积，重复提交同一位不会计数。存在任意待决位时对象的 READABLE 电平为真。

## 消费

消费者先用 WaitMany 观察 READABLE，再调用 NotificationTake 原子取得并清除所选位；未选择的位继续保留。WaitMany 从不改变待决位，因此多个等待者可以同时被唤醒，最终由 Take 协调消费竞争。

Notification 不是广播队列：一个消费者取走某位后，其他消费者不再看到该位。需要每订阅者可靠通知时，provider 必须为每个订阅关系持有独立 Notification signaler，或使用可重放日志。

ObjectSignals、WaitMany、Timeout 与取消的通用契约见 [`wait.md`](wait.md)，本篇不重复定义。
