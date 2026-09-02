# IPC 对象实现

当前 IPC 控制面由进程本地 HandleTable、内核对象、统一等待、Mailbox 与 Notification 组成。共享 ABI 位于 `shared/src/{object,message,wait,startup,call}.rs`，用户封装位于 `user/rinlib/src/ipc/`；Tunnel 与 Runnel 的实现分别见 [`tunnel.md`](tunnel.md) 与 [`runnel.md`](runnel.md)。

## Handle entry 与运输

`os/handle_table` 是纯逻辑 generation slot 表。entry 保存 object、role、rights 与 immutable badge；duplicate、rights 裁剪、TRANSIT 和 GRANT 都保持 badge。

- `extract_moves` 要求 TRANSIT，供 Mailbox message；
- 普通 direct grant 要求 GRANT；
- ProcessGrant direct grant 复用同一 entry/rights 模型，受保护 builder 与 transfer entries 的 pin/提交事务见 [`startup.md`](startup.md)。

`task/handle.rs` 校验对象最大 rights，并在表锁外执行 close callback。Mailbox/Notification owner 可直接 GRANT，不可 TRANSIT；sender、signaler、send-once 与 Tunnel Invitation 可按 role TRANSIT/GRANT；Tunnel Endpoint 与本地 VM lease 绑定，不可运输。

## 生命周期 control roles

ProcessControl 与 JobControl 的 Handle close/transit 都只消散 authority，不触发 kill、seal 或递归收束。ProcessBuilder 是 affine 构造 authority：最后一个 builder 消散时，Building process 以 Abandoned 进入终止路径。

ProcessControl 的电平分两阶段：REAPABLE 表示线程和 active hart 已离场，可执行 ProcessDrain；CLOSED 表示 HandleTable 与 AddressSpace 已完成收束、终态快照已冻结。JobControl CLOSED 表示本 Job 已 sealed 且直接成员/child Jobs 全部 Dead。

所属 Job 强持未 Dead Process core。若全部 ProcessControl shells 消散，JobDerive 可从成员 core 重新铸造单一 shell，并重放 REAPABLE 或 CLOSED，使管理者仍能完成 drain。

## Mailbox 与 badge

`task/mailbox.rs` 实现唯一 owner、多 sender、16 条 FIFO、READABLE/WRITABLE/CLOSED 与 transit entries。

MailboxCreate 原子返回 owner 与 badge-0 sender。MailboxMintSender 要求 owner MANAGE，铸造同对象 sender role、调用者指定 badge 和收窄 rights；MakeSendOnce 保持源 badge。

Send 在 `HandleTable → Mailbox` 锁序下，以当前 pid 和目标 sender badge 构造 MessageHeader。send-once target 与 moves alias 在任何摘除前拒绝。Receive 先预留目标 slots，再以 token 独占队头，完整写回后 commit；失败 rollback 且不出队。Discard 和 owner close 在对象锁外关闭 transit entries。

事务窗口内 READABLE/WRITABLE 是乐观电平；并发者可能得到一次 ObjectBusy 或操作条件已变化，回环重查不会丢事件。

## Notification

`task/notification.rs` 保存 OR pending bits。READABLE 只表示 pending 非零，NotificationTake 是唯一消费入口。owner 可直接 GRANT、不可 TRANSIT；signaler 可按 rights 委托。方向契约见 [`../ideas/signal.md`](../ideas/signal.md)。

## WaitContext 与 Timeout

`os/wait_context` 提供 `Installing → Armed → Finishing → Done` 和单 outcome 仲裁。`task/wait.rs` 解析 Handle/WAIT/allowed signals，保存对象引用，在线程离开执行点后安装订阅，并负责结果交付和取消清理。

公开 ABI 参数 `timeout_ms` 是相对毫秒，零表示无限；完成原因 `WaitReason::Timeout` 的 wire 判别值为 3。内核安装时换算为单调时钟 `expires_at`。

每 hart 的 `TimerQueue` 位于独立 `os/timer_queue` 纯逻辑 crate：arena + 索引最小堆，稳定 token 含 owner slot、arena slot 与 generation；注册、取消、到期弹出 O(log n)，peek O(1)。WaitContext 的原子 `TimeoutRegistration` 在 Unregistered/Token/Closed 间仲裁：对象命中、错误、终止 Abandoned 或 Timeout 只有完成赢家负责退休 token。对象提前完成会立即从 owner queue 注销，不再强持 Context。

跨 hart 完成可以锁 owner queue 删除 token，但不远程重编程 owner timer；至多产生一次提前中断，owner 在下一装填点按新堆顶恢复。timer queue 锁外才析构被移除的 Context Arc。

`clear_active` 与 `park_waiting` 之间若进程已 Terminating，安装者以 Abandoned 完成仍处于 Installing 的 Context，并确认线程离场；不会遗留 lifecycle 成员。用户显式 Cancelled ABI尚未接入，终止 Abandoned 不回用户态。

异步 WaitMany 写回与同步 syscall 输出不同：结果页复检失败经 MemoryNotAccessible 返回等待线程，不杀进程。ThreadSpawn 已接入，同进程另一线程可在等待在途时解除结果页映射，复检失败经该错误通道交付。

## Handle close callbacks

ProcessDrain 的阶段、预算与 pending close 由 [`task.md`](task.md) 唯一记录。本篇只拥有 Handle 摘出后的控制面对象收束语义。Mailbox 队列上限为 16 条、每条最多 8 个 transit entries，因此 owner close 的运输 fanout 至多 128；对象订阅上限为 1024，每个 WaitContext 最多 64 项，完成方在对象锁外清理。具体语义：

- Mailbox owner：关闭邮箱、完成等待者并关闭有界队列中的 transit entries；
- sender/signaler、ProcessControl、JobControl：叶子消散；
- Tunnel Endpoint：关闭事务与 detached retire 见 [`tunnel.md`](tunnel.md) 和 [`mm.md`](mm.md)，本篇只负责 Handle close 的分派边界；
- Tunnel Invitation：放弃未 attach 一侧并通知创建端，具体连接状态见 [`tunnel.md`](tunnel.md)；
- WaitContext 不在 HandleTable，由终止路径单独取消。

## 数据面索引

Tunnel 的 Connection、Endpoint、Invitation、映射与关闭见 [`tunnel.md`](tunnel.md)。Runnel 的页内布局、角色视图、游标、内存序与门铃封装见 [`runnel.md`](runnel.md)。本篇不重复定义数据面对象的 backing、view 或协议不变量。

## 验证入口

- handle_table host：generation、reservation、badge、运输、consume/transfer pin 与 rights 回滚；
- wait_context/timer_queue host：安装窗口、唯一赢家、token generation/owner、堆删除与 cancel/expiry 竞争；
- init core acceptance：badge、send-once、满箱/WRITABLE、Notification 与 WaitMany 确定性通路；Tunnel/Runnel 相关验收分别由 [`tunnel.md`](tunnel.md) 与 [`runnel.md`](runnel.md) 记录；stress workload 追加重复 control/Tunnel 生命周期、ProcessDrain `max_work=1` 和完整竞态矩阵；
- QEMU：`virt`/release/hetero/nofd/sifive_u 分别验证 core 与平台差异，`virt-stress` 验证 16/16 竞态、公开 MemoryMap/Protect/Unmap、异 hart kill、guard fault 局部收束及压力回收；全部路线由 workload、服务监督、资源收束与 reset 终态的 fail-closed 锚点判定。
