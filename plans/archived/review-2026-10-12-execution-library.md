# 执行准入、RPC/Outbox 与用户态库边界 Review

状态：已完成只读审查。未修改生产代码、`plans/COMPASS.md`、Review 收口计划或其他 Review 报告。本文件为本批唯一报告。

## 固定对象与结论摘要

本次按用户指定的固定树审查，不以当前 HEAD 替代：

| 对象 | 固定提交 | 父提交 / 顺序 |
|---|---|---|
| Runtime 通用准入与监督 | `a3891b00c60acc0f91e964c183bb0eea7359404f` | 父 `2efbc87d816ad8ddfb061f8b2f04de254970b91f` |
| Runtime 请求复用与 Job 分页 | `5de2780fe4802f2bb31ddeeafccf87737ba80cc4` | 父 `d2d83def973461c992fcb41c62916fc4dc9de437`；按既定序列接续审查 |
| RPC/Outbox Runtime 接线 | `e0b5c45914d5c9b25f228b1dfc7f4a384ae77375` | 父 `6045e6a7b2c6f48d0a95a7fe752dd1ef2cf852ab`；按既定序列接续审查 |
| Runtime/Outbox 服务闭包 | `4e18e5e396870198eda22623c349e0b7e4cc8033` | 父 `e0b5c45914d5c9b25f228b1dfc7f4a384ae77375` |
| 用户态库知识归属、依赖、目录及命名 | `96ee03b0d1641c86ed8ab05951bad6954ea84db4` | 父 `2124413710bdb8a3bc8a0fa5e73d06f2839af176` |

| 编号 | 严重度 | 性质 | 结论 | 归属 |
|---|---|---|---|---|
| E-01 | P2 | 证据缺口 | Dispatcher 的取消、已发送未知结果、迟到/未知回复及 Wake 拒绝收束路径，缺少固定目标下可核实的组合故障证据；代码路径本身有显式 owner 转换，未确认实际丢失或泄漏。 | `librpc` 与 Runtime 组合验证 |
| E-02 | P2 | 证据缺口 | Outbox 的 Commit 前准入、停止/期限失败、实际来源注销与最终退休退款的组合，缺少可追溯的跨层故障证据；未确认退款或 reply-once 所有权的代码反例。 | `librpc::Outbox`、`libexecution` 及正式消费者 |
| — | — | 实际反例 | 未确认本 Review 固定范围内的新实际代码缺陷。 | — |

E-01/E-02 是验证证据缺口，不应表述为已发现 bug。除这些缺口外，未发现固定实现中可直接证明的准入身份错配、Gate 越权、请求 FIFO 丢失、错误退避/重复退款、RPC 请求结果无人持有或库依赖反向的反例。

## E-01：Dispatcher 取消、迟到结果及 Wake 拒绝缺少组合证据

**性质：证据缺口，不是已确认代码缺陷。**

**固定位置与可达路径**

- `4e18e5e:user/frameworks/librpc/src/dispatcher.rs:388-406`：`wake_completion` 对已完成调用投递 Runtime Wake；请求缓冲拒绝时保留 `wake_pending` 重试责任。
- `4e18e5e:user/frameworks/librpc/src/dispatcher.rs:409-474,475-545`：统一完成路径回收计时器/回复存储并收束来源；收到回复后根据活动 txid 匹配请求。
- `4e18e5e:user/frameworks/librpc/src/dispatcher.rs:688-754,777-818`：来源事件、取消、完成领取及已封存的无人领取结果退休路径。
- 可达边界为：调用者取消已发送请求或 deadline 到期 → 结果为 sent/unknown → 对端回复迟到或出现未知 txid → 回复被丢弃；或者完成后 Wake 因容量/目标状态拒绝 → Dispatcher 保留或清理 waiter 并继续推进完成队列。

**契约与影响**

固定计划要求 Wake 暂满、目标退休、调用者退出及拒绝回执不丢失完成责任，也不保留无界 owner。静态代码显示 Dispatcher 有 `wake_pending`、已完成队列、发送状态区分、来源清理后入队及封存时退休无人领取结果等机制；因此本次没有证据证明这些路径违反契约。但提交记录列出的 `librpc` host 测试集中在库级/出站辅助接口，固定源码中没有 Dispatcher 状态机测试；既有 `just virt` 通过记录也未给出上述取消、Wake 拒绝和迟到回复组合的可定位断言。缺少证据使得“未知结果仍由谁持有、何时退休”无法从验证材料确认。

**建议归属**

由 `librpc` 与 Runtime 的真实组合验证补证：至少覆盖一个已发送请求取消/超时后迟到回复，以及完成 Wake 暂不可投递或目标已退出时结果的可领取/退休闭合。沿用正式 Dispatcher/Runtime 和现有验收消费者，不另造测试专用运行体。若底层来源取消迟到注册回执触发 HighHolly F1，只引用其 finding，不在此复制或扩大为新的 SourceRegistration finding。

## E-02：Outbox 失败与真实注销/退休的组合证据不足

**性质：证据缺口，不是已确认代码缺陷。**

**固定位置与可达路径**

- `4e18e5e:user/frameworks/librpc/src/outbox.rs:87-125`：终态设置及来源注册/重试；Outbox 保留响应与 Delivery 的实际 owner。
- `4e18e5e:user/frameworks/librpc/src/outbox.rs:159-206`：期限/来源失败、准入和进入发送步骤的门槛。
- `4e18e5e:user/frameworks/librpc/src/outbox.rs:206-283`：发送、完成和 `stop`；`Abandoned` 明确不是业务回滚，`is_complete` 要求清理完成。
- 可达边界为：准备响应后提交 Outbox → 来源注册或发送失败/超时/停止 → 向实际 Runtime 请求来源注销 → 收到注销确认后任务退休并释放未提交资源；若发送已成功，则不能把业务提交伪作退款或撤回。

**契约与影响**

固定计划要求 Outbox 在业务副作用前预备任务、来源及回复资源；`Sent`/`Abandoned`、期限、关闭和停止必须分别保有正确的 Delivery/reply-once/额度终态，且来源实际注销先于退休。代码检查未发现 `complete` 直接伪造回滚：其 `Abandoned` 明确表示放弃发送而非逆转业务；并且 `is_complete` 与来源清理相联系。但可查的固定目标验证记录只汇总 target 检查、clippy、host 测试和 `just virt`，未定位到覆盖“提交前失败、发送后停止、注销失败重试、最后退款/终态”的单项证据。故这里只登记证据缺口，不声称该路径已经失败。

**建议归属**

由 Outbox、Runtime 和真实消费者 owner 通过正式服务路径补证，重点证明提交前失败完整返还、已发送后停止不假造回滚、注销拒绝/重试不提前退休，以及最终结果只由正确 owner 领取或退休。有关 Packet/Producer/Consumer/Endpoint 运输语义转交 MossyFalcon；对象链底层 owner 与其迟到回执转交 HighHolly，不在此重审。

## 代码与结构覆盖

### Runtime / 准入

按 `a3891b00…`、`5de2780…` 与最终固定树审查了任务/来源身份、任务族分类、Gate 请求归属校验、输入 FIFO、deadline 与执行退避分离、到期输入、失败任务隔离/重试、停止调度、来源移除与退休退款，以及 Job 分页/失败恢复的 owner。检查了最小工作预算下的轮转与有界推进，不把纯 Deadline 换算、旧来源相位或已删除的 public-operation ownership、DebtLedger、ProcessDrain 当作本次新 finding。未发现超出 E-01/E-02 的实际反例。

### Dispatcher / Outbox / 正式消费者

检查了 Dispatcher 的请求发送状态、txid 匹配、结果领取、取消与超时完成、唤醒待处理、未知/迟到回复、无人领取结果退休；检查了 Outbox 的提交前准入、PreparedResponse/reply-once、Delivery、发送/放弃、停止和注销后退休。按固定树核对 `srv_init`、长期 `srv_fs` 的 Runtime 接线和旧阻塞泵迁移。没有把业务协议解释为 RPC 的领域知识，也未发现调用者必须继续运行才能推动已提交 Dispatcher/Outbox 责任的具体反例；组合边界仍有 E-01/E-02 所述证据限制。

### 用户态库知识归属、依赖和命名

在 `96ee03b0…` 固定树检查了 framework → libraries 移动、`libexecution`/`libprocess`/`librpc` 的包依赖、公共类型与真实消费者迁移、旧空 `libdrv` 和共享服务类型删除，以及路径、Cargo package、target/import 和文档引用。固定树的执行由 `libexecution` 拥有，`libprocess` 消费该能力；RPC、流、预算和领域库位于组合侧，没有发现反向依赖或重复 facade 的固定树反例。

当前文档/树中 `libkernel`、`libruntime` 等后续分层只用于核对现行知识归属与命名边界，不倒填为 `96ee03b0…` 已包含的结构，也不替代对其后续固定提交的独立审查。现行规则要求目录叶名、Cargo package、默认 target、crate identifier 一致，并禁止 alias/重复 facade；当前 `user/libraries/README.md` 与 `user/README.md` 明确由 `libkernel`、`libruntime`、`libprocess`、`libexecution` 各自承担不同知识。未发现本批指定改名闭包留下的实际重复 owner；后续分层自身的其他提交不在本报告固定对象内。

## 排除项与报告交界

- **HighHolly / `plans/review-2026-10-12-substrate-object-chain.md`**：不重审对象链基础 owner、VM/funding/Work/Job，不复制 SourceRegistration 迟到回执 F1 或 IPI/runtime_stop F2。E-01/E-02 若依赖 F1，直接引用其处置，不另立相同 finding。
- **MistyFinch / `plans/review-2026-10-12-fal-service-delivery.md`**：不审 FAL/fs/libservice 的业务语义、权限、服务目录或文件交付；只检查它们作为执行机制实际消费者的接线，不复制其 findings/evidence gaps。
- **MossyFalcon / `plans/review-2026-10-12-transport-time.md`**：不审时间/Deadline 纯换算或 Packet/Producer/Consumer/Endpoint 运输细节。Outbox/RPC 到运输层的返还和取消只描述交界；若发现运输 owner 问题转交其报告，不重复立项。

## 验证与残余风险

本轮为固定提交只读审查，没有修改代码，也没有重新运行构建、host 测试或 QEMU。固定提交树、父提交、既有任务计划中的验证记录及相关实现均已核对；不能把已有整体通过记录视为 E-01/E-02 指定组合路径的证明。未确认实际代码缺陷；残余风险是 Dispatcher 的迟到/未知结果与 Wake 拒绝收束，以及 Outbox 在失败、停止、注销和退款交界缺少可定位的组合证据。此结论不表示相关路径已通过，也不复制三个相邻 Review 的发现。
