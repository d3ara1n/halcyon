# todo：IPC 三面（message/signal/tunnel+Runnel）实现 review

状态：**待实施**。完成后本文件移 `archived/`，结论落
`plans/review-2026-08-ipc.md`（只读档案）；发现问题按性质分流：
缺陷当场修、潜伏缺陷入 KNOWN_ISSUES、新任务开新 todo。

## 审查对象

| 提交 | 内容 |
|---|---|
| `c21839e` | message/signal 契约落地（邮箱 + 对象切面信号状态机） |
| `4f70b5b` | tunnel/Runnel 契约与规格文档（审查时作对照基准） |
| `29b48b3` | tunnel/Runnel 数据面落地（登记表 + librunnel） |

契约基准：`notes/ideas/{message,signal,tunnel,runnel}.md`；
调研背景：`plans/ref-2026-08-ipc-contract-research.md`；
外部规范取证入口：`references/INDEX.md`（RISC-V 内存模型相关条款）。

## 核查清单

### A. message/signal

- [ ] **A1 锁序论证**：全系统锁序「mailbox/signals → 目标线程 space →
  就绪队列」是否确无反向获取者。重点排查 deliver 移交路径（持目标
  邮箱锁再取等待者 space 锁）与 exit/reap 路径的交错。
- [ ] **A2 死条目内存驻留**：多源 SignalWait 命中一处后，其余对象队列
  里的同代条目是死条目，仅在队列被触碰（submit/enroll/扫描）时惰性
  消失。推演极端场景：低频对象的队列长期无人触碰 → Arc<Thread> 滞留
  → 线程与其地址空间延迟释放。量化最坏滞留，判定是否可接受或需
  主动清扫（如 reap 时反向摘除）。
- [ ] **A3 NONEMPTY 不变量覆盖性**：枚举邮箱队列的全部变更点
  （deliver 入队 / take / discard），核对 sync_nonempty 是否无遗漏、
  无多余唤醒。
- [ ] **A4 put_user_indirect**：逐页 translate 拷贝的页界处理、
  check_range 校验区间与实际拷贝区间的一致性；跨页消息（负载接近
  PAYLOAD_MAX 且缓冲不对齐页界）的边界用例。
- [ ] **A5 quiescence 语义**：「IPC 等待者无主人，不阻静默」的论证
  在当前无设备中断前提下成立；标注设备接入后必须重审（已写进
  is_quiescent 注释——验证注释与代码是否会被后续改动无声破坏）。

### B. tunnel/Runnel

- [ ] **B1 双端并发拆除**：两个 hart 同时 Dispose 同一条隧道的两端，
  或一端 Dispose 与另一端 process_died 交错。推演
  notify_survivor_or_release 的双路径（通知幸存端 / 摘除条目）在
  entry 锁下是否完备；`ends[idx].take()` 的幂等性依赖是否有洞。
- [ ] **B2 SignalWait 隧道目标的消失窗口**：publish 登记时 lookup
  失败静默跳过（条目永不命中）。语义上该端点已消失，等待者应否
  以错误完成而非永久睡眠？构造时序：wait 注册期间对端 Dispose。
- [ ] **B3 帧生命周期不变量**：process_died（不触达死者空间）与
  dispose（unmap_external 自己空间）的分工在「attach 后未 dispose
  直接退出」「create 后 peer 从未 attach 即退出」等边角下的帧归还
  与 PEER_CLOSED 正确性。可用帧计数断言（free_frames 前后对比）。
- [ ] **B4 Runnel 内存序的实证缺口**：host 测试是单线程交错，
  fence 配对未经真 SMP 弱序实证（QEMU TCG 也未必暴露）。评估是否
  需要：双 hart 压测负载（生产者/消费者分驻两核高频往返）+
  现有 fence 层次的逐条比对 RISC-V 规范条款。
- [ ] **B5 attach 竞态**：两进程同时凭同一 id attach（第二端点槽位
  竞争）；attach 与对方 dispose 同时发生。

### C. rand

- [ ] **C1 兜底质量声明复核**：sifive_u 路径的熵估计是否如实入档；
  id 撞车循环在登记表满时的行为。

### D. 压力验证（建议新增负载）

- [ ] **D1** 多进程消息风暴：N 个进程互发 + 满箱 MailboxFull 重试，
  观察错误码路径与无泄漏（帧计数守恒）。
- [ ] **D2** 隧道建立/拆除风暴：反复 create/attach/write/dispose 与
  进程退出交错，帧计数守恒断言。
- [ ] **D3** librunnel host 多线程测试：std::thread 双线程真并发跑
  协议两端（现有 host 测试为单线程交错，这是 B4 的部分补偿）。
- [ ] **D4** sifive_u 5 核重复跑既有集成负载 ≥10 轮（时序差异覆盖）。

## 已知疑点（实现时注意到但未深究的）

- A2 死条目滞留是最可能需要真实修复的点；
- publish_signal_wait 的 Tunnel 分支 lookup 失败静默 None（见 B2）；
- MailboxFull 的上层流控（请求-应答配对限流）尚无消费者验证过。

## 完成标准

1. 清单全部勾销：每项给出「确认无误」或「问题 + 分流去向」；
2. D 组压测全绿且帧计数守恒；
3. 结论入档 `plans/review-2026-08-ipc.md`，本文件移 archived/。
