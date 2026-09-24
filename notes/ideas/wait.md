# 等待与持久观察

等待是系统中把线程转入 Waiting 的统一完成入口。消息、Tunnel、进程终态、Lifetime 与 Notification 先表现为对象状态，再由 WaitMany 或持久 WaitSet 观察；Sleep 复用同一等待所有权和定时来源。

## ObjectSignals

可等待对象公开电平条件。Mailbox READABLE 表示当前可以尝试接收消息；队头被另一接收事务占有时暂不成立，Tunnel DATA 提示重查共享控制块，PEER_ATTACHED 表示对端映射已经建立且尚未关闭，ProcessControl REAPABLE 表示可以收束，CLOSED 表示不可复活的终态，MemoryObject EXECUTABLE 表示可执行发布完成。

观察不清位、不消费业务资源。Receive、Tunnel acknowledge 和 NotificationTake 才改变各自条件。醒来者必须重试实际操作，并接受并发消费者已改变条件。

已登记观察的命中候选须在对象发布时保留；后续清位不抹去候选。异步交付不能只重读最新电平，也不能把不同更新 OR 成从未同时成立的快照。

role 必须公开合法 signals，Handle 还必须有 WAIT。Invitation、Delivery 等纯授权或交付角色不因内部有寿命而自动可等待。等待解析区分“本次已验证的使用引用”和“真正的电平来源”；多个发送授权可以观察同一 Mailbox，不能复制队列电平真值。

## WaitMany

每项包含 Handle、signals 和 cookie，结果包含 cookie、observed、item_index 和 reason，不回显可能失效的 Handle。完成原因包括 Signaled、Closed、Timeout，以及未来公开取消的独立结果 Cancelled。

内核接受 [绝对 Deadline](time.md)。用户库可以提供相对时长便利入口，但只在入口转换一次。有限零时点不是无限：初始观察已有条件可立即命中，否则返回 Timeout；由此可以表达非阻塞观察。有限 RPC 仍在接受回复时独立核验其 Deadline。

同一 Handle 可按不同条件重复出现。同一次初始检查或同一对象更新命中多项时，最小 item_index 获胜；不同对象变化由首先取得完成权者决定，不承诺跨对象原子快照。

解析 Handle 后，等待持有已验证的观察来源直至注销或完成。另一线程关闭或转移原 Handle 不撤销在途观察，对象 owner 关闭则发布 Closed。Waiting 线程的执行责任另有稳定拥有根，订阅不能成为线程唯一的保活来源。

## WaitSet

WaitSet 是持久观察集合，供长期服务把大量对象状态汇入有界接收批次。它不替代对象状态、不接收业务消息，也不执行用户回调。

拥有者可以逐项 Register、Rearm、Remove，并批量 Receive 就绪记录。每个 registration 有不复用的 token、用户 cookie、观察条件和预付的结果槽。Register 不批量遍历全部来源；集合规模由显式资源预算决定，不受单次 WaitMany 输入项数量限制。

注册周期为：

```text
Installing → Armed → Queued → Disarmed
                        ↑         |
                        └─ Rearm ─┘
任一存活状态 → Removing → Dead
```

一轮 arm 最多交付一条记录。Queued 之后保留第一次获选的完整快照，不累积无界事件；Receive 消费记录并进入 Disarmed，只有显式 Rearm 才开始下一轮。Rearm 必须原子观察当前电平，已经成立的条件不能因没有新边沿而丢失。

WaitSet 的 READABLE 只表示其就绪队列非空。Receive 以有界批次原子交付，输出失败不消费记录。Remove 使该 token 不再产生可接收的新记录，并收束源订阅及在途完成责任；已经被用户取走的旧记录仍需由用户按 token 的有效状态过滤。

持久注册保留已验证的观察来源。原 Handle 关闭后，来源对象的终态仍可到达，不依赖一个已经退休的表槽。内核注册不授予数据访问或映射关闭权；用户态协议可以借出观察能力而继续独占自身数据 owner。

## WaitSet 的收束

WaitSet 是容量可增长的内核容器，其来源订阅和结果存储由内核拥有并 [有界退休](object.md)。用户停止服务业务与内核维护集合是不同责任；用户只提交普通关闭，不编排内部维护与退休步骤。

- 普通关闭原子停止新操作和结果交付，转交稳定的内核退休责任；非空不是关闭失败理由。
- 每次执行只推进有界工作；依赖在途安装、重置或完成时停驻，完成后继续，不热循环也不申请无法保证的清理资源。
- 关闭提交即发布 CLOSED，退休完成使用内部完成责任；关闭成功返回时集合自身的订阅、安装和完成资源已收束。
- 调用线程终止不撤销已提交关闭；进程退出与正常关闭复用同一退休机制，不遗漏已经从表中摘除的对象。
- 关闭不等待其他观察者运行或消费事件，避免自观察和互相观察形成清理依赖环。

owner 唯一、不可 TRANSIT；可以在 Building 期直接 GRANT。显式 close 和正常析构使用普通关闭，允许通过线程 Waiting 等待内核退休，而不是在内核栈上自旋。用户服务仍须独立停止准入、完成/取消业务任务和释放运输责任，集合关闭不替代这些业务契约。

## 安装、完成与取消

WaitMany 与 WaitSet 共用来源订阅、发布快照与有界通知机制。订阅的完成目标可以是一次线程等待或一个持久注册周期；不复制对象电平算法，不让来源锁内执行跨对象清理。

WaitMany 未立即命中时，dispatcher 建立等待意图；线程离开 hart 后才发布订阅和定时项。对象、Timeout、安装错误与终止取消竞争唯一 outcome。WaitSet 的 Installing 同样隔离尚未发布的注册与提前命中；失败必须撤销整个注册，不能留下幽灵结果。

命中候选、唯一 outcome 与最终交付是不同责任。结果槽、通知及完成清理责任均在接受观察前准备；发布者不分配，也不同步遍历所有订阅。批量清理按实际工作推进，最终析构不能隐藏另一次全表遍历。

进程终止使用内部 Abandoned，不返回用户态，不冒充公开 Cancelled。完成后的定时项和来源订阅必须及时注销。Notification 消费由 [signal](signal.md) 拥有；业务超时与重试由各协议拥有。
