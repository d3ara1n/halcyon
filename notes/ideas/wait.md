# 等待

等待是系统中唯一把线程转入 Waiting 的完成入口。消息接收、隧道门铃、进程终态和 Notification 都先表现为对象状态，再由 `WaitMany` 统一观察；Sleep 也复用同一等待所有权和定时来源。

## ObjectSignals

可等待对象公开一组电平状态。每一位表示当前为真的条件，不是某个等待者私有的事件：Mailbox `READABLE` 表示至少有一条完整消息，Tunnel `DATA` 表示应重新检查共享控制块，ProcessControl `REAPABLE` 表示管理者可以开始有界收束，`CLOSED` 表示不可复活的终态。

WaitMany 只观察状态，不清位、不消费资源。对象语义拥有者显式改变电平：Receive 取走最后一条消息后清 READABLE，Tunnel 协议确认无进展后清 DATA，Notification 由专用 Take 消费待决位。醒来者必须重新执行真实操作，并接受并发消费者可能已先一步改变条件。

不是每个 capability 都可等待。对象 role 必须同时公开合法 signals，Handle 还必须持有 WAIT；一次性 Tunnel Invitation 等纯授权角色不因内部存在关闭状态而自动成为可等待对象。

## WaitMany

每个输入项是 `{handle, signals, cookie}`，结果是 `{cookie, observed, item_index, reason}`，不回显可能已经失效的 Handle。完成原因包括：

- `Signaled`：关心的普通电平命中；
- `Closed`：对象进入 CLOSED；
- `Timeout`：相对超时到达且没有对象完成；
- `Cancelled`：未来公开取消操作的独立结果。

公开 `timeout_ms` 是相对毫秒时长，零表示无限；它不是绝对时钟 Deadline。需要绝对时间的上层协议应先基于公开单调时钟计算剩余时长，或未来使用独立的绝对等待 ABI。超时与取消不能互相伪装。

同一 Handle 可用不同 signals/cookie 重复出现。一次初始检查或同一对象更新同时命中多个项时，输入中最小 `item_index` 获胜；不同对象并发变化由首先取得完成权者决定，不虚构跨对象原子快照。

调用入口解析 Handle 并保留对象引用后，本次等待持有已验证的授权；另一线程关闭或转移原 Handle 不撤销在途操作。对象 owner 关闭仍以 Closed 完成。

## 安装、完成与取消

未立即命中时，dispatcher 只建立等待意图；线程离开 hart 执行点后，调度侧才发布订阅和 timeout registration。对象命中、Timeout、安装错误与终止取消竞争同一个 outcome，唯一赢家取走线程所有权、注销定时项并清理全部订阅。

进程终止使用内部 `Abandoned` 完成，不回到用户态，也不冒充公开 Cancelled。完成后的 timeout registration 必须立即注销，不能继续持有 WaitContext 或阻止系统静默。

Notification 的消费语义由 [`signal.md`](signal.md) 唯一拥有；具体 WaitContext 与 timer queue 实现见 [`../impls/ipc.md`](../impls/ipc.md)。
