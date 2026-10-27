# Notification

Notification 表达按位 OR 合并的通知状态。需要事件身份、次数、顺序或负载时，使用消息或相应数据协议；就绪通知本身不承诺逐事件保存。

## 三种角色

- Owner 持有消费与关闭责任，并可凭管理权签发只读 Observer。
- Signaler 提交通知位；获得等待权时可以观察接收端关闭。
- Observer 只观察 READABLE/CLOSED，不取得 Signal、Take 或关闭业务的权力。

这些能力指向同一通知状态，但各自的能力寿命不同。最后 Owner 关闭结束业务，观察者、发送者和已有等待登记可以继续保留观察存储；存储仍在不代表业务仍开放。

## 观察与消费

WaitOne 观察一个对象，持久 WaitSet 汇集多个来源。观察不清除待决位；Owner 的 Take 才消费所选通知位。通知只提示业务重新读取自己的权威状态，不能替代业务查询或确认。

需要每订阅者独立确认时，由 provider 为订阅关系组合相应通知与业务控制。Observer 的消散不自动等同于业务订阅取消，具体关系由所属协议定义。

角色、签发、付款、等待与关闭合同由 [Notification 计划](../../plans/todo-2026-10-10-notification-observer-roles.md)拥有，通用观察关系见 [wait](wait.md)。
