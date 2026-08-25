# review：IPC 三面（message/signal/tunnel+Runnel）

本次审查确认旧实现的对象模型、等待所有权、消息事务、隧道寻址与 Runnel 发布契约均不能作为后续基础；修复由 [IPC 对象 / Handle 重建](todo-2026-08-ipc-object-foundation.md) 承接。本文是只读审查档案，记录风险与由风险推导出的验证要求，不作为待办状态载体。

审查对象：`c21839e`（message/signal）、`4f70b5b`（tunnel/Runnel 契约）、`29b48b3`（tunnel/Runnel 数据面）。新契约基准为 `notes/ideas/{object,wait,message,signal,tunnel,runnel}.md`；RVWMO 取证入口为 `references/INDEX.md`。

## 审查结论与承接

| 项 | 结论 | 新计划承接 |
|---|---|---|
| A1 | 旧的 mailbox/signals 到线程 space 的锁序与对象生命周期无法满足新事务边界。 | Handle、邮箱与消息事务 |
| A2 | 惰性死条目可滞留线程资源，不接受。 | WaitContext |
| A3 | 旧 NONEMPTY 是隐式邮箱与旧信号模型，不保留。 | MailboxCreate + READABLE |
| A4 | 用户区校验与复制必须在同一 space 锁保护，旧逐页路径不再是模型。 | Handle、邮箱与消息事务 |
| A5 | 静默论证须随设备接入重审；新等待不让对象订阅保有线程。 | WaitContext / 集成验收 |
| B1 | 旧登记表双端拆除不再是目标模型。 | 隧道与 Runnel |
| B2 | lookup 消失静默跳过会永久等待，不接受。 | WaitContext + Connection 终态 |
| B3 | 帧生命周期改由 Connection 与两端 lease 定义。 | 隧道与 Runnel |
| B4 | host/TCG 不能证明弱内存序。 | Runnel + RVWMO 审查 |
| B5 | id attach 竞争由 invitation 单次消费和 Connection 线性化替代。 | 隧道与 Runnel |
| C1 | 随机 id 及其熵声明随 registry 删除。 | 隧道与 Runnel |

## 由风险推导的验证要求

### 消息、信号与等待

- 新锁序无反向获取者；用户空间锁覆盖校验和复制。
- 完成或取消后订阅不保有线程、进程或地址空间。
- Mailbox `READABLE` 覆盖入队、Receive、Discard 和 owner close。
- 跨页、未对齐的最大 payload 输入/输出正确；失败输出不可解释。
- 设备接入时重审静默与外部完成源。

### 隧道与 Runnel

- 双端 close、进程退出与未 attach invitation 交错下帧计数守恒。
- wait 安装期间 Endpoint 消失以关闭结果完成，不永久睡眠。
- attach 与 close、双 attach 竞争只产生一个合法 B 端。
- host 双线程、RISC-V 双 hart 压测覆盖 Acquire/Release 对与 Runnel 门铃闭环。
- Runnel 锚点、EOF、游标回绕和 Broken 分流符合新规格。

### 压力验证

- 多进程消息风暴覆盖 MailboxFull、ObjectBusy 与回滚，资源守恒。
- 隧道 create/attach/write/close 与退出风暴保持帧守恒。
- librunnel 运行真并发双线程测试。
- sifive_u 五核既有集成负载连续至少十轮。

可执行状态与完成证据统一记录在重建计划及其最终实现 review 中。
