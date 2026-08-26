# IPC 对象 / Handle 统一 review 结论

> 审查范围：`4c06a99..836e501`（IPC 对象重建）加后续 `f5a80e9`（一次性投递权与
> 发送侧流控电平），即 IPC 对象层现行全部实现。本文档只读，承载
> `todo-2026-08-ipc-object-review.md` 预定审查节点的结论。

## 总体结论

**未发现当前可达缺陷。** 七个审查面（ABI / Handle / 等待 / 消息 / Tunnel /
Runnel / 启动授权过渡）逐项核过，核心不变量成立；回归验证全绿（见文末）。
产出为两项**多线程前置债务**（已记入 KNOWN_ISSUES）与若干观察项。

## 1. ABI 面 — 通过

- 结构全部 `repr(C, align(8))`、固定宽度整数，size/align 编译期断言
  （`shared/src/{object,message,wait,startup}.rs` 尾部 const block）。
- reserved 纪律：`SendHeader.reserved` 内核校验非零拒绝；`WaitItem.reserved`
  非零拒绝；`MessageHeader`/`WaitResult` 的 reserved 由内核写零，不透传用户位。
- 调用号分区无冲突（0x30–0x3a 对象/等待、0x40–0x45 消息、0x60–0x64 tunnel）；
  `StartupMailbox = 0x18` 已注释为过渡。
- `a0–a5` 布局与 rinlib `raw_call` 核对一致（Send/Receive 六参数两组均对）。
- 输出可见性：异步路径（Sleep/WaitMany）不前进 sepc，由完成方 deliver 写回
  结果后置 `a0=0` 再 `+=4`——成功/失败对用户一次可见，无中间态。
- 用户提供的 `Rights::from_raw` 未知位一律拒绝（`is_known`），不截剪不放大。

## 2. Handle 面 — 通过

- generation 回绕：`advance_generation` 在 `u32::MAX` 时置 Retired，槽位永不
  复用；generation 0 恒无效（`Handle::is_valid`）。host 测试覆盖。
- 失败守恒：`extract_moves` 先全量验证（重复项、rights 子集）再统一摘除，
  任何失败表不变；Send 的满箱判断先于摘除——满箱不 move（集成负载验证）。
- reservation/commit/rollback 全有或全无；rollback 同步前进 generation，
  失败输出中的暂存 Handle 数值永不复活。
- 一次性投递权：`MailboxMakeSendOnce` 与源 role/kind/rights 同判同临界区；
  消费与投递在同一表锁内原子化；经 transit move 转移后消费顺延到接收方；
  满箱失败不消费。集成负载六个组合用例全覆盖。

**观察（性能，非缺陷）**：`HandleTable::reserve_slot`/`insert` 对空槽线性
扫描，单进程 65 536 槽上限下，close/duplicate 循环可把每次操作放大到
O(n)。正确性无损；free-list 是自然的后续优化，接入 pm 后再议。

## 3. 等待面 — 通过

- Installing/Armed 竞态由 `WaitCore` 状态机闭合（offer / finish_installing /
  arm 三方交接，唯一完成者 CAS 仲裁）；host 并发测试含 2 000 轮 offer-vs-arm
  交错，每轮恰有一方取得完成权。
- 重复 Handle 在同一 WaitMany 中不去重：语义良定义（同对象多订阅，第一个
  匹配者完成，其余在 cleanup 时 unsubscribe），无缺陷。
- 关闭语义：`ObjectWaitState::matches` 令 CLOSED 匹配一切兴趣——对象关闭
  必唤醒全部等待者；终态冻结（`update` 拒绝 CLOSED 后的任何迁移）由共用
  结构保证，各对象无需逐点防御。
- 清理时序：finish 先切断 `WaitContext → Thread`，再逐对象 unsubscribe
  （锁外串行，无嵌套），后 deliver + enqueue——对象锁内永不触碰 space/表锁。
- 中断安全：内核态全程 SIE=0（`_ret_to_user` 前显式清零、U 态由 sie 控制），
  timer 到期只从用户态 trap 或 idle 显式路径进入——`expire → unsubscribe`
  触碰对象锁不会嵌套进持锁上下文，无 Spinlock 自锁。

**观察（占位怪异，无实害）**：deliver 中 `(WaitMany, Deadline|Cancelled)`
分支返回 FunctionNotAvailable。当前不可达（WaitMany 无期限参数）；未来
WaitMany 带期限时需给出正式语义（Cancel reason 或专门错误），接入时改。

## 4. 消息面（Mailbox）— 通过

- 锁序 `HandleTable → Mailbox` 全程一致（enqueue_with / begin_receive 均在
  表锁内取邮箱锁）；`finish_waiters` 恒在两锁之外。
- Receive 事务：token 独占队头 + 表侧 reservation；任一步失败整体回滚
  （队头 push_front、reservation 回滚）；用户写回失败同样回滚，失败不出队。
- owner close 与在途回滚的交错由 `rollback_receive` 锁内 closed 检查闭合：
  关闭后不重新入队、由接收方关闭 transit；关闭前排空循环稍后取走。无消息
  泄漏、无双重关闭。
- 电平是状态的纯函数（`MailboxState::publish` 单点派生），无增量转移。

**观察（良性窗口）**：`begin_receive` 不重发布电平——接收事务期间队头已
出队但 READABLE 仍置位，并发观察者可得 Signaled 后撞 `ObjectBusy`。
单接收者模型下这是良性的自愈路径；用户态多线程落地后它会成为可观察的
虚假唤醒面（仍正确，只是需要重试），届时可考虑事务内降级电平。

## 5. Tunnel 面 — 通过

- Invitation 单次消费：attach 在 Connection 锁内验证（closed 标志 + sides
  身份 as_ptr 比较 + 对端 Alive）后原子替换 sides、remove 源项（消费不触发
  lifecycle）；与 creator close / invitation abandon 的并发全部由 Connection
  锁线性化，四向交错核过。
- Endpoint / Invitation 的 allowed_rights 均不含 DUPLICATE——每对象恰一个
  Handle，close 即最后引用，无需引用计数。设计干净。
- 帧守恒：`Connection.frame`（FrameTracker）RAII；`external_mappings` 登记
  使 AddressSpace 回收只清 PTE 不还帧；`unmap_external` 是唯一解除入口；
  Process::drop 先摘表项执行 close（space 仍活），再自然回收。virt 收尾
  31 346 帧自由 + quiescent 佐证无泄漏。
- `Endpoint::close` 的 peer 侧 replace-then-restore 写法是借用检查的迂回，
  行为正确（peer 侧 Alive 不变，仅取出通知目标），风格问题不记录。

## 6. Runnel 面 — 通过

- RVWMO 配对逐对核对成立：init 全零 + release magic ↔ attach acquire magic；
  数据普通写 + release head ↔ acquire head + 数据读；数据读 + release tail
  ↔ acquire tail + 覆写；set_eof release ↔ 先 acquire eof 再 acquire head。
  与 `references/normative/riscv-isa-v20250508` 的 PPO acquire/release 规则
  一致，不依赖 volatile 或偶然顺序。
- EOF 顺序：观察到 eof=1 后冻结 `eof_head`，head 再变即 Broken（host 测试
  `head_cannot_advance_after_eof_publication` 覆盖）。
- shadow cursor 双向校验（推进量 ≤ 未决量、used ≤ CAP），违反即永久 Broken
  （host 测试覆盖）；写入与读取的环形分割拷贝跨页边界正确（host 回绕测试）。
- 门铃闭环：acknowledge（清 DATA 电平）→ 重查 → wait 的顺序消除虚假唤醒；
  DATA 为显式消费电平，ring 后持续置位，无丢失唤醒；跨进程流控负载中 pm
  侧 spin 位检测证明唤醒只能由腾位引起。

## 7. 启动授权过渡实现 — 安全性通过（最终模型另行处置）

- `StartupMailbox` 查询只返回本进程 bootstrap owner，无越权面。
- 装载器在进程 runnable 前投递（邮箱不可并发访问，enqueue_startup 的 assert
  保证前置）；未消费的 sender grant 与未认领的 pm sender 均 close_transit，
  无泄漏。
- 本面按计划只审安全性；最终契约由 `todo-2026-08-process-startup-resources`
  承接，不得把 StartupMailbox 当冻结方向（该 todo 原文已声明）。

## 前置债务（已转 KNOWN_ISSUES）

**多线程写回 panic 面**：`MailboxCreate`/`HandleDuplicate`/`MakeSendOnce`/
`Receive` 等写回路径的 `expect("validated ... must remain writable")` 依赖
「预校验到写回之间同进程无映射变更」。当前单线程进程下成立；`ThreadSpawn`
落地后，同进程异 hart 线程可在两次 space 锁之间经 `unmap_external`（close
tunnel endpoint）解除输出页映射，写回校验失败 → expect panic → 内核崩溃，
违反「用户可触发的 fault 杀进程绝不 panic 内核」戒律。接入用户态多线程前
必须改为优雅错误路径（或写回前在锁内复检并以进程终止处置）。

## 验证证据（回归基线）

- host 单测：handle_table 10 项、wait_context 4 项（含 2 000 轮交错）、
  librunnel 8 项（含双线程 257 轮回绕）全部通过；`just check` 绿。
- `virt` 四核：IPC 集成负载全过（128 轮控制面、64 轮 tunnel 生命周期、
  send-once 全组合、WRITABLE 电平与跨进程流控唤醒、8 192 字节 Runnel、
  peer closed 观察）、四服务全员回收、静默停机。
- `sifive_u`：同负载全过、quiescent（SRST 不可用为平台已知，按日志关键行
  判定通过）。
