# 公共时间与 IPC/运输边界固定提交 Review

状态：已完成只读审查。未修改生产代码、`plans/COMPASS.md`、Review 收口计划或其他 Review 报告。

本报告固定审查三个目标提交，不以当前 HEAD 替代目标树：

- `c6e0a84eeb125a9f7b881354c25309f21e8f8d13`，父 `de981a655b0fb68d7f0f26fe52c9d335a527ae10`：公共 `ClockGeometry`、`Deadline`、ClockState、采样、Wait/Sleep/Send 期限消费者。
- `3060dd8b3facd00b314266791970cd931596ead2`，父 `e583d5ec144e09d459d1cd97037962c495f5c6e6`：typed `MailboxSender`/`SendOnce`、消费式 `Packet` 投递、`Delivery`/失败返还及真实消息消费者。
- `a2aabeda17243f798b17e4337e25f2bfcd625f5c0`，父 `a6ae767d860e38b531c505a169f6a2a0def58bac`：typed `Producer`/`Consumer`、Runnel 创建/接入/结束和 Endpoint owner。

固定对象之后发生了用户态库重排，旧 `user/rinlib`/旧 `librunnel` 现已迁入 `user/libraries/libkernel`、`librpc`、`librunnel` 等路径。本报告用后续代码只核对当前真实消费者和交界，不把后续实现当作固定目标已通过的证据。

## 结论摘要

| 编号 | 严重度 | 性质 | 结论 | 建议归属 |
|---|---|---|---|---|
| F-01 | P1 | 实际反例 | `a2aabed` 的 Runnel 批量读写没有携带绝对 `Deadline`；底层等待固定走零毫秒便利入口，而该入口把零解释为无限等待。对端不启动、无进展或只保留半个流时，正式 `srv_init`/`srv_pm` 数据面可永久阻塞，不能按统一期限、取消或关闭责任收束。 | `librunnel` 与真实流消费者；执行来源/Runtime 只负责后续登记与退休，不复制本 finding |
| E-01 | P2 | 证据缺口 | `3060dd8` 的真实期限返还夹具仍使用 unsafe `send_raw_until`，typed `Packet::try_send`/`try_reply` 在真实 MailboxFull、DeadlineExpired、关闭交界的完整返还没有 guest 证据。 | 消息运输 owner；不转为 Mailbox 内核基础 finding |
| E-02 | P2 | 证据缺口 | `a2aabed` 的真实验证覆盖正常 RNL2 数据面和 host 模拟 close/协议故障，但没有固定树中实际 Endpoint/映射失败、Attach 已消费后协议失败、Unmap/Endpoint close 失败的跨层退款证据。 | 流运输/Runnel owner；映射底层机制转交 HighHolly |
| E-03 | P2 | 证据缺口 | `c6e0a84` 的 ClockGeometry/ClockState 和 `srv_init/time_checks` 覆盖有限期限、At(0)、原期限重试和已提交 Send，但没有把当前流运输、真实 RPC/服务取消迟到组合纳入固定时间消费者证据；RPC/FAL 业务期限明确不在本 Review 内。 | 时间消费者各自 owner；RPC/FAL 交界转交 GoldRidge/MistyFinch |

F-01 是固定目标代码可达的行为缺陷；E-01--E-03 不是已复现代码 bug，不能写成通过，只能由对应 owner 以正式消费者和既有设施补证。除 F-01 外，没有确认新的 Mailbox/Packet owner 泄漏、Deadline 换算反例或 Runnel 协议进展反例。

## F-01：Runnel 批量 I/O 丢失绝对期限，可永久阻塞

**性质：实际反例，固定目标 `a2aabed`。**

**固定位置**

- `a2aabed:user/frameworks/librunnel/src/lib.rs:42-43`：`Transport::wait(&self)` 无 `Deadline` 参数。
- `a2aabed:user/frameworks/librunnel/src/lib.rs:147-153`：`Channel::wait` 只能调用无期限 `transport.wait()`。
- `a2aabed:user/frameworks/librunnel/src/lib.rs:255-267`：`ProducerCore::write_all` 满环后调用无期限 `channel.wait()`。
- `a2aabed:user/frameworks/librunnel/src/lib.rs:361-375`：`ConsumerCore::read_exact_or_eof` 无数据后调用无期限 `channel.wait()`。
- `a2aabed:user/frameworks/librunnel/src/lib.rs:443-454`：guest 实现用 `Endpoint::wait(..., 0)`；`a2aabed` 对应 `rinlib::time::timeout_millis(0)` 的约定是无限等待。
- `a2aabed:user/frameworks/librunnel/src/lib.rs:553-554,602-603`：公开 `Producer::write_all`、`Consumer::read_exact_or_eof` 也没有期限参数。
- `a2aabed:user/services/srv_pm/src/main.rs:86-103`：真实 Producer 消费者逐块写入，调用无期限 `write_all`。
- `a2aabed:user/services/srv_init/src/main.rs:1069-1073`：真实 Consumer 消费者调用无期限 `read_exact_or_eof`。

**可达路径**

1. Producer 写满共享环，或 Consumer 已消费当前数据但尚未取得下一批数据。
2. `write_all`/`read_exact_or_eof` 先 `acknowledge` 并复查仍无进展，进入 `Channel::wait`。
3. `Channel::wait` 没有保存、比较或传递调用方期限；guest 最终通过 `Endpoint::wait(..., 0)` 进入零毫秒便利入口。
4. 固定时间封装将 `timeout_millis(0)` 保留为 `Deadline::INFINITE`，因此对端不启动、停在半流、无 EOF 或无后续通知时，调用线程没有期限到达分支。
5. 由于 `Producer`/`Consumer` 明确为非 `Sync`，同一 owner 没有并行关闭入口可作为可靠取消机制；调用者也无法从该阻塞调用中取得累计进度后按 deadline 退出。服务停止、Endpoint 退休和映射退款依赖这次调用返回，责任链可永久停驻。

**违反的契约**

`c6e0a84` 建立的公共契约是绝对 `Deadline` 在入口一次转换，Wait/Sleep/Send 的重试保留同一时点；有限期限不是相对等待重启或零值哨兵。Runnel 是当前真实的共享流运输消费者，却在固定目标中没有期限形态，且把等待硬编码为无限。`notes/impls/runnel.md` 的最终契约也要求批量读写把同一绝对期限传递给每次等待；该说明不能替固定提交补上缺失实现。

**影响**

无响应的流对端可令 `srv_init` 或 `srv_pm` 永久占住执行线程、Endpoint、共享映射和相关上层收束责任。关闭、Finish、Runtime 停止和最后引用不会按声明的期限产生结果；这不是 FAL 业务语义，也不是 GoldRidge 所属的 Dispatcher 执行准入问题，而是 Runnel 与公共时间/运输边界未接通。

**归属与建议方向**

由 `librunnel` 和真实流消费者 owner 修复：公开批量读写须接收绝对 `Deadline`，每次 acknowledge 后复查并以同一 deadline 等待；超时必须携带累计进度，且不伪造 EOF/成功。相对超时便利入口只能在用户态入口一次转换，不能在 `Transport` 内重新计算或以零代表 Infinite。修复后须用正式 `srv_pm`/`srv_init` 或 libfs 流消费者验证对端不启动、满环、半流、EOF、取消/关闭和映射退休，不能新增测试专用流运行体。

对象链基础 Endpoint/映射最终 close 的底层 owner 不在此复制；若修复需要处理 Endpoint close/unmap 的拒绝或迟到回执，转交 HighHolly 的基础 owner 报告。

## E-01：typed Packet 失败返还缺少真实 guest 证据

**性质：证据缺口，不是已确认代码 bug。**

`3060dd8:user/rinlib/src/ipc/packet.rs:106-143` 定义了消费式 `try_send`/`try_reply`，失败应返还完整 `Packet`、transit capability 和 send-once。`user/frameworks/librpc/src/exchange.rs` 的 `Request::try_send`、`PreparedResponse::try_send` 也按 take/restore 实现。固定提交的 host 用例只覆盖构造、裁剪和空 Packet；`3060dd8:user/services/srv_init/src/time_checks.rs:91-128` 的期限/满箱检查仍直接调用 unsafe `send_raw_until`，不是 typed Packet 失败路径。现有提交说明中的 guest 正常流控、fs 泵和 RPC 证据能证明成功路径，但不能证明 typed wrapper 在 `MailboxFull`、已过期 Deadline、关闭目标和回复失败时的双重返还。

未发现由静态代码单独证明的泄漏窗口：`Packet::publish` 的 syscall 失败分支保留 `self`，`try_reply` 保留 `SendOnce`，上层再决定重试或关闭。当前缺口是正式 guest 组合证据，尤其不能用 raw 夹具的 stale/rights 结果替代 typed owner 的责任证明。补证归消息运输 owner；内核 `MailboxCore::enqueue` 的基础线性化和 Delivery 退款转交对象链 owner。

## E-02：Runnel 跨层失败清理证据不足

**性质：证据缺口，不是已确认代码 bug。**

固定 `a2aabed` 的 `Producer/Consumer::create/attach` 已删除旧 raw 工厂，协议初始化失败返回 Endpoint/Invitation owner；`srv_init` 的 `CreateFailure::Protocol` 分支显式关闭两项 owner。host 测试覆盖协议损坏、通知失败、显式 close 失败和累计进度，例如 `a2aabed:user/frameworks/librunnel/src/lib.rs:854-868,930-952`。但这些 host Transport 不是真实 Endpoint/普通映射，固定提交的 virt 验证主要是正常 RNL2 数据面和 typed Invitation 转移。

因此仍缺少以下固定树证据：Map 失败后 Endpoint/Invitation 双 owner 的真实清理；Attach 已消费 Invitation 后输出几何/协议失败；Unmap 成功而 Endpoint close 失败、或二者失败/重试时的跨层责任；失败期间对端观察、最后引用和 Pool 退款的组合。报告不把缺证据升级为泄漏 finding，也不把当前 HEAD 的独立 `MappedEndpoint` 设计倒填到 `a2aabed`。该缺口由流运输 owner 补证；实际 VM/backing/映射退休基础归 HighHolly。

## E-03：公共时间与运输/迟到组合未形成固定消费者证据

**性质：证据缺口，不是已确认换算反例。**

`c6e0a84:shared/src/time.rs:7-130` 的 `Deadline` 编码、u128 换算、可读/可编程尾部和 Infinite/At(0) 语义自洽；`c6e0a84:os/kernel/src/clock.rs:43-146` 以 ClockState 高水位、不可逆失败和最终期限检查作为内核唯一时间入口。固定真实自检覆盖 malformed Deadline、At(0)、绝对 Sleep、满箱后保留同一 deadline、已提交交付不撤回和跨线程因果采样，见 `c6e0a84:user/services/srv_init/src/time_checks.rs:22-181`。

固定证据没有覆盖：

- Runnel 批量等待与 Deadline 的组合（并且 F-01 证明固定流 API 根本未传期限）。
- 真实 RPC/服务取消、迟到回复、ReplyObserver/Delivery 与 Deadline 的组合；这些执行准入和 FAL 业务责任分别由 GoldRidge/MistyFinch 拥有。
- 平台实际高水位/epoch 边界上的多 hart 采样与期限消费者组合。Clock selftest/host 证据可证明编排状态机，但不等于完整弱序硬件证据。

这不否定 `ClockGeometry`/Deadline 静态实现，也不重新审查已排除的 `runtime_stop` F2。补证必须使用正式时间消费者和既有验收设施；不能把 RPC/FAL 业务测试或后续 HEAD 的 Runnel API 当作本固定目标的通过证据。

## 覆盖范围

本次按固定提交相对父提交检查了：

- shared `Deadline`/`ClockSnapshot`/`ClockGeometry` ABI、内核 ClockState、采样和当前 Wait/Sleep/Send 消费者；
- `MailboxSender`、`SendOnce`、`Packet`、`Delivery`、Lifetime observer、typed 转换、raw/兼容出口和真实 init/pm/fs 运输路径；
- Runnel `Producer`/`Consumer`、Channel 协议进展、Start/EOF、acknowledge→重查→wait、Invitation/Endpoint、Create/Attach/Finish/关闭和跨层映射 owner；
- 固定目标之后的当前消费者迁移，只用于定位旧目标仍存在的责任和避免重复审查。

没有审查或宣告通过：

- 对象链基础 owner、VM/funding/Work/Job、SourceRegistration 取消迟到回执 F1、IPI/runtime_stop F2；
- FAL、fs/libservice 业务协议和授权语义；
- RPC Dispatcher/Outbox 执行准入、来源登记/注销和任务退休；
- 设备/DMA、磁盘持久化和验收脚本整体可靠性。

## 与其他报告的交界

- **HighHolly / `plans/review-2026-10-12-substrate-object-chain.md`**：本报告不复制其 SourceRegistration 迟到回执 F1 或 IPI/runtime_stop F2。E-02 若需要真实 VM/backing、Endpoint close 或映射退休底层证据，转交 HighHolly；本报告只保留 Runnel 跨层 owner 的直接缺口。
- **MistyFinch / `plans/review-2026-10-12-fal-service-delivery.md`**：本报告不审 FAL/fs 业务协议、Copy/Watch/Bind 或服务目录交付，不复制其五项证据缺口。只指出 `srv_init`/`srv_pm` 作为运输/时间消费者触发 F-01 的调用位置；业务收束归其既有 owner。
- **GoldRidge 的执行与库边界 Review**：F-01 不属于 Dispatcher/Outbox 执行准入；E-03 中 RPC 取消/迟到组合只作为未覆盖边界转交，不复制执行层 finding。

## 验证与残余风险

本轮未修改代码，也未重新运行构建、host 测试或 QEMU；结论来自固定提交相对父提交的源码、既有提交验证记录和当前消费者路径。F-01 是由固定 API 形态和调用路径直接构成的可达反例，不因既有正常流测试降级为证据缺口。E-01--E-03 明确保留为证据缺口，直到对应 owner 使用正式消费者补证或记录不破坏契约的延期去向。

