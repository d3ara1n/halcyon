# 公共对象、观察与退休前置

> 状态：#13 公共对象/消息/WaitSet/内核退休前置已完成并归档，纳入 `task/fal-service-capabilities` 的混合集成基线；提交定位见 FAL 总计划交接节。当前实现真值见 [IPC 实现记录](../../notes/impls/ipc.md)。下列施工期目标、阶段记录和当时的未完成项是历史证据，不再安排当前实施。公共时间 #14、执行基座 #15、FAL 总体仍未完成。

## 完成证据与边界

- ABI/所有权/通知/普通 Close/ProcessDrain/来源与现有消费者共同收口；公开 WaitSet Seal/Drain 和重复维护路径已删除。kernel-owned actor/ticket、预付 mandatory/finish/finalization 根在 Caller 终止后仍完整退休。
- Native 新增真实兄弟撤结果页，停驻 2/8 后仅恢复 6 步、Fault/StoreAccess 离场；同一 waiter 下一轮也在第二 ticket 停驻，旧 epoch 不取消新依赖。隔离启动数据证明预算、轮次、请求/线程/依赖与实际容量退款，不冒充用户指令或远端 shootdown。
- 通知/退休先行历史、多 interest 反位序、终态 Deferred/Complete 摘槽和 64 项非空 actor 三类压力通过。类预算不超过控制面 16；新到类不是同轮保证，deferred 面另有独立 16。
- 用户真实双接收线程先 gated FIFO/确定性 Full，再自由竞争 64 条 4096-byte 独立授权消息，完整 payload/once/Delivery/寿命、不丢不重与 cleanup；跨进程 Close/Native CLOSED 提交后 kill，两个目标精确 Killed 终因及 Pool charge 退款通过。
- WaitSet 显式 GRANT 创建与 into_capability 正向迁移接真实消费者，默认权限不变。Capability 统一非映射 entry；旧 leaf-only/received 契约和辅助函数删除，非空转换直接 Drop 已运行。
- 外部 GDB 捕获生产 Kill(0x131) 前 active=2、三成员、mandatory=1；随后真实 Waiting 取消 epoch1、reusable=false/KernelResult0，识别已安装 Close 回复。日志 `artifacts/check/public-ipc-exit-gdb.log`、完整 core `public-ipc-exit-probe.log`、保留 ELF `public-ipc-exit-kernel.elf` 与 SHA256。本次快照证明该窗口，不宣称每轮两回复都 Waiting。
- 旧阶段三项 continuation findings、Mailbox 三项 P2、最后六项公共测试/Capability 契约 P2 已逐项定点复核关闭；未留下兼容机制或生产 correctness finding。
- 最终 `artifacts/check/public-ipc-final-{core,sifive,release,nofd,boot-failure,clippy,host,shared-host}.log`：core/128MiB/release/nofd 通过，panic/alloc/fatal 三种 Failed 全 hart 停驻通过，七面 lint 与 140+23 host 测试通过。新增组合进入正常 required anchors。
- 先前完整 stress 的 300s Tunnel 截断与概率 15/16 未宣称修复或通过；用户已独立延期到 [验收可靠性任务](../todo-2026-09-13-acceptance-reliability.md)。不执行总体 acceptance/stress 收尾、不豁免新 correctness，不把 #13 完成视为时间/执行/FAL 交付。
- 下一自然序：[公共时间](../archived/todo-2026-09-monotonic-time-rpc-deadline.md) → [运输/RPC/服务执行](../todo-2026-09-13-service-runtime-prerequisites.md) → [FAL](../todo-2026-09-fal-service-capabilities.md)。包归属整理仍独立延期。

## 施工期材料

## 开工审视与范围

目标是让公共 ABI 表达能力，而不是要求用户推进内核对象的内部状态。设计依据是可转移授权、在途交付责任、持久观察和短内核路径；不以现有 syscall 数量、已写代码或文档中的“冻结”为依据。

本任务共同迁移 shared/kernel/rinlib、所有信号来源对象、通知/完成债务、普通 Close、ProcessDrain 及现有真实消费者。它们共享安装/完成/取消与关闭责任，不能按单个 syscall 或 happy path 拆成独立完成项。跨 workspace 包归属仅由 [未来整理计划](../todo-2026-09-13-workspace-package-ownership.md) 承接，本轮不搬包。

## 目标 ABI 与独立理由

- 保留 MailboxCreate/MintSender/MakeSendOnce、Send/Peek/Receive/Discard：队列拥有者、独立发送授权和一次性发送是不同能力，消息区并非新加七个调用号。
- 保留 Lifetime 与 Delivery 的保活方向：观察不保活授权；队列、接收预留和已接收请求的交付责任保活授权。单独审计每消息 Delivery 的分配、表项和关闭成本，不因存在寿命需求就默认所有包装结构都必要。
- HandleQuery 仅描述已有能力，不能按身份打开对象或取得额外权限。
- WaitSet 专用操作保留 Create/Register/Rearm/Receive/Remove，删除公开 Seal/Drain、DrainResult 与 WaitSet 专属 REAPABLE/DONE。不单为减少调用号把三个类型不同的注册操作并成通用 Control；五个入口各有明确事务和输出，用户不构造含大量不适用字段的命令。
- WaitSet 非空 Close 不返回 Busy：普通 HandleClose 原子摘除 owner、停止操作/新结果交付、提交内核退休，成功返回表示集合自身责任与资源已经退休。提交失败保留 owner；提交后线程终止只取消回复，不撤销退休、不恢复 handle。
- CLOSED 在关闭提交时发布给既有观察；退休完成使用私有 completion。关闭不等待其他观察者消费事件或运行，否则自观察/交叉观察会形成清理依赖环。
- Remove 立即使 token 失效并撤销尚未消费的记录，物理收束由内核继续；真实退休后才退款。已交付旧记录仍按 token/generation 有效性过滤。
- Receive 非阻塞、有界、失败不消费；阻塞等待复用 WaitMany。每轮 one-shot、消费后 Rearm、来源锁内电平重查和不回绕 generation 保留。
- WaitMany 是临时线程等待，WaitSet 是持久事件集合；共用 ObjectWaitState/Subscription/WaitCore，不用隐藏 WaitSet 重建 WaitMany。
- ProcessDrain 不把零进度 Blocked 暴露成用户推进协议：本批已有进度可以返回 More；零进度且依赖未完成时内核挂起并继续同一批次，保留累计预算。More 继续要求 work_done > 0。

## 必须先成立的内部机制

```text
Open owner / ProcessDrain entry
  └─ 原子提交 ─> 内核 Retirement + 已准入 WorkDebt 强拥有根
                         ├─ 来源注销 / 安装责任 / finish 交回
                         ├─ Runnable 或 Blocked(dependency, generation)
                         └─ 完成 ─> 私有 Completion / RetireTicket
                                      ├─ Close 等待者回复
                                      └─ ProcessDrain 同一游标继续
```

1. **债务可等待依赖**：统一 Step {work_done, Runnable | Blocked | Complete}；Blocked 允许零工作量，保留 payload/槽但不计 runnable、不循环自唤醒。一次性的 WakeToken 在来源登记前取得，Taken 锁存早到唤醒；票据交回前禁止释放/复用槽，避免引入可重放唤醒与额外分配。单一退休执行者用内核 Dependency 持有票据，在其来源同步下采样真实条件，生产者在条件变化后锁外通知；不复制来源电平。先接通未发布进程的生命周期屏障，之后连接集合退休/在途完成和 ProcessDrain 的同一依赖机制。
2. **通用内部结果等待**：线程等待绑定与完成存储分离，Memory/Tunnel/普通对象退休使用同一正式构造机制，各自提供资源资助；不把 WaitSet 塞进 MemoryWaitPermit，不借地址空间事务伪装普通关闭。
3. **稳定退休拥有根**：活体 Close 提交时原子登记进程必成义务，真实完成才释放；调用线程被 kill 或 handle 已摘除都不能令进程提前 REAPABLE。已经 REAPABLE 的 ProcessDrain 则由 pending retirement ticket 拥有责任，不重新建立前置生命周期义务。
4. **预付必成路径**：Create 准入集合、退休债务/完成和唯一 Close 回复存储；Register 准入来源订阅、ready 和可复用 finish。正常 Rearm、Remove、Close 和退休唤醒不分配不可保证的清理存储。
5. **来源锁外交接**：ObjectWaitState 摘出订阅，锁外释放最后引用/执行跨对象工作。集合锁和来源锁不嵌套；WorkDebt 锁内只取出/登记/重排，不执行对象步骤。
6. **安装与重置登记**：Closing 禁止新操作登记；已登记 Register/Rearm 必须交回责任。来源注销、安装结束、finish 归还共同决定单项退休，不能仅凭 entries.is_empty 或 WaitCore::Done 宣布完成。
7. **总安全点预算**：notify/finish/retirement 共用公平轮转和明确总上界；blocked 不产生 runnable pending，idle/远端门铃唤醒不得遗漏。执行在 trap/scheduler 安全点，无内核线程、栈驻留轮询或全表析构。
8. **准入隔离**：区分持久元数据、消息周转与必成清理责任及实际资源成本。长期注册不能消耗清理所需预留；对投递的资源竞争必须有隔离/容量依据和可定位错误，不继续用一个“每项一个额度”解释所有成本。

关闭提交只执行固定工作；建议锁序 HandleTable → WaitSet state → lifecycle，等待绑定、调度与最后引用析构在锁外。具体锁序以实际类型图复核，不能依赖 enum kind 特判不断扩张 Handle 收束路径。

## 内核请求继续的明确前置

代码追踪确认 KernelResult 只有静态返回值，不能恢复 ProcessDrain；WaitContext 在队列归还 finish 槽前 DONE，lifecycle 只存裸 Weak<WaitContext>，Waiting 写出也不能调用假设 Running 的 deliver_output。这些不是加一个 callback 能闭合的问题，必须先与对象退休共同补齐：

- 等待身份携带不可回绕的 cycle epoch，offer/arm/abandon 的验证与状态转换属于同一轮次协议；旧 lifecycle 取消不能触达新请求。WaitCore 用原子字中的代次/状态防 ABA，owned 请求不存进 Copy outcome。
- 目标 Process Core 出生时预付一个可复用 Drain 完成存储、finish 槽和固定请求状态；captured DrainRequest 持真实 target/control、输出、一次截断的预算、累计工作与 affine 批次许可。恢复不重新解析 Handle，不重置预算，不保存 spinlock guard。
- finish 执行请求 continuation，贯通 StepResult<owned dependency>，Blocked 停驻已有 finish 债务；业务 work_done 与安全点登记/取消成本分开计算，不把阻塞变成 More+0。
- finish 槽归还、旧请求/依赖退役后才发布可复用 DONE，并在锁外交付线程；来源持久性与完成容量复用是两个属性。
- Process 从 Job 成员表摘除前，终段传播必须已转交独立的预付 Finalization 债务拥有根，不依赖 Control.drain_owner 或当前 DrainRequest。与 Native 批次共享目标游标且逐步仲裁；批次许可占有期间 Finalization 停驻，释放许可后正常 Wake，不轮询。Done 后才退休债务拥有根。
- Installing/FINISHING 的 abandon(epoch) 独立于 outcome；Blocked 时主动摘下匹配来源登记并正常 Wake，使执行者释放请求/批次许可。未安装 WaitPlan 被 scheduler 丢弃也归还本轮许可，已提交对象 actor 继续退休。
- 单一 actor 独占对象退休推进，只监听进度依赖；ProcessDrain 等 actor 的独立完成依赖，普通 Close 结果由 actor 直接提交预付等待。两种条件各自单一消费者，不把 actor 与 drainer 塞进同一个单 waiter 槽；后台退休成本归安全点预算，本次 Drain 只累计直接启动/游标/结果工作。
- 有提交副作用的内核请求结果异步写出使用间接访问；失败按 Waiting completion 冻结 caller Fault/StoreAccess、锁外发布终止并完成 departure，不伪装 Running member。普通 WaitMany 仅观察、不消费业务资源，输出失败仍返回 MemoryNotAccessible，不把观察错误无理由升级为 fault；两者以是否已提交资源责任区分。

共同 scope：wait_context、WaitContext/WaitPlan/finish、lifecycle/scheduler 凭据与取消、ProcessCore/DrainRequest/批次许可、间接输出策略、对象 actor/ticket/pending retirement 与资源容量。先实现这些最终结构的组成部分，再一次接通 WS/ProcessDrain 并删除旧协议；允许前置构件尚无后续 caller，不以此标记闭合任务完成。

## 自底向上施工顺序（同一闭合任务）

消息接收电平与失败重试是既有机制的独立正确性缺口，不依赖新增退休类型，可先收紧完整预留/commit/rollback 路径；这不是单独任务完成门。其余机制按下列依赖顺序推进，特别是不可先换 WaitSet ABI 再补退休拥有根。

1. 定义 WorkDebt blocked/wake、通用结果等待、退休拥有根与资助类型，接通现有 Memory/Tunnel/进程清理真实消费者并删除零进度轮询。先做这些前置，不先换 WaitSet 调用号。
2. 接通对象通知/来源注销与锁外析构，修正消息接收预留电平及有限期限重试，审计 Sender/Lifetime/Delivery/运输回滚和准入隔离。
3. 以同一退休机制接通 WaitSet 安装/rearm/remove/close、进程必成义务、ProcessDrain pending ticket，删除公开维护状态与旧关闭特判；同步 shared/kernel/rinlib。
4. 迁移 rinlib WaitSet/Close/ProcessDrain、Runnel 注册门面、RPC dispatcher、libsrv runtime、grant 及已有监督/验收消费者。用户服务停止业务仍独立成立，不把 WaitSet 关闭等同于任务成功或业务取消。
5. 完整连接后执行 host/静态/内核及已有真实消费者组合验证；源间竞态、调用者退出、self/cross observation、Blocked 唤醒、公平预算和最后退款一起检查。不能把一个入口能编译当成任务完成。

## 当前施工与检查证据

最新施工记录（本任务仍未完成）：

- Mailbox 已补 `mailbox/selftest.rs` 并与 WaitSet continuation 共用 `task/selftest.rs` 的真实 Bound space/域准入/用户复制/终止退休夹具；接收尾段 `finish_receive` 由生产与自检共用，不新增测试 syscall。独立 sender/duplicate/迁移 once 的身份、badge、role/裁剪 rights、成功消费与禁止二次 send 运行通过；queued/received Delivery 保 sender，最后引用关闭时 Lifetime 弱授权根消散且已安装 CLOSED 订阅实际被唤醒。
- 真实接收预留后 READABLE 隐藏 queued followers，Peek/Discard/第二预留 Busy；范围初检后生产 Unmap 撤销 payload 页、header 已写而复制失败，生产尾段退款目标表槽/队头、锁外唤醒已装 WSet。重试完整 header/payload/业务 KOID/Delivery KOID 不变，新能力未关闭时全部旧编号为 StaleHandle。满箱 receiving 占位不腾容量，failed Send 同时保留 once 和 move 源；ownerclose 后在途仍保 life，再真实 Unmap+生产尾段 closed rollback 拒收并关闭最后 transit，队列不复活。整组 Pool/准入/control/deferred 退款，不把末尾库存代替交付断言。
- 定点 reviewer 未见生产 bug，三项 P2 测试缺口（旧号跨复用、once 实际消费、Lifetime 通知）已修补并经第二轮定点复核关闭。最新 debug virt core/reset、128MiB sifive_u core/预期 reset NotSupported 收割、virt-release core/reset 与七面 clippy 通过，见 `artifacts/check/public-ipc-mailbox-{virt,sifive,release,clippy}.log`；正常路线新增 Mailbox ownership required anchor。本组不执行用户指令、不证明真实多 hart 或远端 shootdown，下一自然序 Native 完成坏输出/新轮 parked 旧取消、真实多 hart Waiting/active Caller 退出与接收竞争、通知历史/Deferred 小回归及更宽压力门。公共任务保持未完成。
- 验收概率覆盖误失败与 Tunnel 静默截断已按用户安排统一转入 [验收可靠性 todo](../todo-2026-09-13-acceptance-reliability.md)，KNOWN_ISSUES/COMPASS 同步。本任务继续公共正确性施工，不反复重跑直到绿、不降低验收标准或把暂缓等同已修复。
- notification 的 publish/Pending 增、实槽 rearm/旧 Pending 减已统一 DEBTS 临界区，来源 complete_notification/Held/Release/Reschedule 与最后引用仍锁外。current owner/current hart 协作执行使旧分段计数不直接构成已证竞态；这是控制协议一致性收口，非 Tunnel 截断归因。定点 reviewer 确认槽、根、来源状态/跨 owner重排和锁阶闭合，未见确定问题。
- `control_pressure` 建 64 来源订阅 backlog、实际 offer 后预排旧 finish 和空 actor Close；进入安全点前三类均 pending，同一安全点通知有 outcome/仍有未处理订阅、旧 finish Done、actor finished、总步骤不超过正式 MAX_STEPS_PER_SAFE_POINT（16）。排水后全 backlog Done/Signaled/READABLE，再关闭并与 baseline 比较全部槽及准入退款，不能仅以退款当成功交付。最终 virt core/reset 日志 `artifacts/check/public-ipc-control-pressure-virt.log` 通过；七面 clippy 日志 `public-ipc-control-pressure-clippy.log` 通过。
- 本压力组证明隔离启动中进入安全点前已 pending 三类的保底进度，不证明真实多 hart 并发、执行中才入队类的同轮保证或非空 actor 多步吞吐；控制面 16 与 deferred_work 的独立 16 不合称单一总预算。消息顺序竞争/回滚与 Sender/Lifetime/Delivery 已见上述最新运行证据，真实多 hart 与其它组合门保持未完成。

- 已补启动期 `wait_set/selftest.rs`：旧完成快照跨新 Armed 轮次、CLOSED 已 seen 的通知摘槽、关闭后迟到来源安装（含最终 Closed ready）、未交回安装操作的退休门、Rearm 在来源 reset 前/后 × Remove/Close 的 4/4 排列。直接调用生产 core/callback/Operation/source/actor 边界，夹具准备 registration 状态；这是机制级顺序排列，不声称实际多 hart 或 syscall 端到端。
- CLOSED/Rearm 滞留 P1 已定点 reviewer 确认关闭。复核续发现 `retire_closed_step` 跳过未见历史激活快照的 P2：`select_snapshot` 统一普通通知与退休的未 seen 最小 serial/无候选 CLOSED fallback，退休仍无条件摘槽；Readable→Closed→退休先行自检能够捕获原回归，定点复核确认 P2 关闭。
- `retirement/selftest.rs` 持正式队列 Taken token 到业务 Complete，在队列交回前/后 × 新注册并 Remove/Close 的 4 个窗口安排工作，再调用生产 `return_slot`/backend finish，验证同一槽重发布和收束。另有仅队列保留一个强引用的关闭用例，完成后 Weak 不可升级，证明最后根释放。
- 未安装 committed Close reply 自检使用真实 Lifecycle attach/BeginRunning/termination/departure；保留 reply identity 在等待类型边界直接断言该轮 Abandoned/Done/Abandoned outcome，再确认 outstanding operation 仍使进程不可 REAPABLE，actor 结束才退款 mandatory。定点 reviewer 未见实现漏洞，指出的取消断言缺口已补并运行通过；未 dispatch 线程，也未提交 termination todo，故不冒充正式 Waiting/active hart 退出链。
- `PendingClose::Retirement` 真实 ticket 在独立一槽 WorkDebts 上以 work=0 停驻，注册生产 FinishDependency，分别在 park 前/后完成 actor，验证早到锁存与晚到重新 runnable、完成 ticket 只收一个 cursor work、槽与 WakeToken 一并交回。不是 Native syscall/scheduler suspension 的直接证据。
- 夹具结束分别比较 20 类 admission、notification/finish/retirement 固定槽池 Empty 库存与当前 hart Pending。构造压力另耗尽 actor 槽、Kernel finish 槽及 Object 准入，穿过正式 WaitSet ctor，证明早期/后期失败的预付槽与许可退款；Object 压力复用批量 SponsoredPermit，不保存巨量逐项 permit。隔离启动自检通过，不声称并发跨表一致库存。
- 独立 test_target 的 `retirement` workload 保留 256 项 WaitSet，Ready signal 后由 srv_init kill 并逐批 `max_work=1` Drain：4165 批完成，More 必须 work=1、所有结果 work<=1。正式句柄退休/用户预算消费者组合已运行，不据此推断具体调度窗口已暂停。
- stress 重跑完整 16/16/reset：`artifacts/check/public-ipc-retirement-gates-stress-retry.log`（最后根/构造压力补项前）；含所有补项的最终 virt core/reset：`public-ipc-retirement-gates-final-virt.log`、128MiB sifive_u core/预期 reset NotSupported 收割：`public-ipc-retirement-gates-sifive.log` 均通过。最终全仓七面 clippy：`public-ipc-retirement-gates-clippy.log`，just check 无警告：`public-ipc-prerequisites.log`，六公共包 host 含 integration 共 74 项：`public-ipc-host-tests.log`，全部通过。QEMU 工具把自检和 256 项退休纳入对应路线 required anchors。
- 本轮失败记录：`artifacts/failed-acceptance-20260913-190359-98990.log` 是 Ready signaler 未授 GRANT 导致 Spawn RightsDenied，修正夹具；`failed-acceptance-20260913-190640-99659.log` 是已知 last-thread-exit-vs-kill 15/16，256 项 Drain 已先通过；`failed-acceptance-20260913-192209-9842.log` 是 Object 逐项 permit 数组大块分配失败，替换为正式批量资助 guard。它们不替更早未分类超时归因。
- `wait_set/selftest/continuation.rs` 已接启动 Ready 前组合：真实 Bound space/stack、执行域 ReadyBatch/AdmittedThread、WSet Create/Register/HandleClose、clear_active/生产 wait::install、生产 termination debt/ThreadDeparture。已安装 Close 回复被 kill 后成员归零但 mandatory 仍等 operation/actor，真实完成才 REAPABLE；不再仅直接调用 lifecycle 摘成员。
- Native 使用正式 ProcessDrain/control/request/FINISH_DEBTS，停驻时核 epoch、dependency/request/thread、finish 存储以及已累计 budget/work=(8,2)，pending ticket、batch active 和 runnable=0，结果 sentinel 未写。恢复前仅推进 actor，确认 control pending=(0,1)/actor 无 runnable，生产 finish 安全点恰花剩余 6 步；More/work=8、ticket 被消费。另一用例在停驻时生产 kill，主动取消原依赖、返还 batch/finish 并确认成员离场，不写半结果；已提交目标 actor 继续。
- 同一 prepaid waiter 第二轮 budget=1 复用，核同 context/恰增一 epoch/旧身份 abandon=Lost，新轮请求与结果正常。第二轮允许合法 More/Complete，地址空间有逐 root 槽退休，不强制 Complete。unpublished 正式债务在 pending ticket 上零工作 park，runnable=0/重复安全点无进度，归还 operation 后唤醒至 Dead。末尾 Pool/20 类 metadata/control/deferred/actor 槽库存完整退款。
- 上述组合是单 hart 主动推进、Ready 前其他 hart 未 dispatch 的生产路径夹具，**不执行用户指令、不证明真实多 hart Waiting/active 退出时序或新轮已停驻时旧取消并发**。Running uaccess 采用自检专用 Activated SATP guard，按固定 supervisor.adoc ASID Usage 每写 ASID0 satp 后完整 SFENCE.VMA、锁外恢复内核页表后才 pump/park/collect，不改生产 uaccess。
- 定点 review 新增 second-round 完成过强 P1、预算仅看输出/身份 helper 未核 epoch 两个 P2，均已修订并经第二轮定点复核关闭；隔离生产 finish 的 6 步断言能捕获累计值 reset 后多执行 2 步。失败现场 `failed-acceptance-20260913-194924-13440.log`（fixture 未激活 Running 页表）、`195206-14603.log`（错误游标预期）、`195738-15925.log`（错误第二轮 Complete 预期）保留。
- 最新组合在 debug stress 自检、128MiB sifive_u core 和 virt-release core 通过，见 `artifacts/check/public-ipc-waiting-continuation-{stress,sifive,release}.log`，新 Waiting continuation 锚点纳入所有正常 QEMU 路线。但本轮整体 stress 未通过：`failed-acceptance-20260913-201112-17670.log` 在 300s、THROTTLE=100 截在 close round7；诊断 360s 配方总 223.446s 走完 Tunnel 24 项后因已知 15/16 主动失败收割（`201738-25206.log`），普通复跑总 166.765s 同样 15/16（`202204-31065.log`）。不拿后续通过局部矩阵替原超时归因。
- round7 无输出窗口已用外部 GDB 三次采样：`public-ipc-stall-gdb-{1,2,3}.log`/`public-ipc-stall-timing.json`；分别为等待投递/dispatch、MemoryMap preflight、Tunnel unmap 发布并伴另一 hart 等 lifecycle 锁，诊断轮后来走完矩阵。未见确定等待环，但无内部 round 前进计数，不能证明原 300s 轮同因。路径假设为页表每层 512 槽准备/发布成本及 yield 改 execution sequence 导致 ObjectBusy 重试，后续需计数证实，不更改 execution gate 或直接放宽时限；原 wrapper 明确 THROTTLE=100，不采纳默认 50% 推测。
- 下一自然序：三类同时压力/总预算公平与消息 Sender/Lifetime/Delivery/Receive 回滚组合；真实多 hart Waiting/active 退出、Native 完成输出失效和新轮 parked dependency 旧取消继续补证。用户明确将概率验收判定与 Tunnel 截断/静默机制暂缓，唯一实施点为 [验收可靠性计划](../todo-2026-09-13-acceptance-reliability.md)，本任务继续施工；不把未绿组合记为通过，剩余公共正确性门仍须闭合。通知先行/多 interest 反向位序/终态 Deferred 小回归同专题补齐。FAL 业务仍暂停。

上一轮 actor 迁移证据：

- WaitSet actor 已与普通 Close/ProcessDrain 共同接通：Create 预付同一维护/退休槽及单次 Close 回复；Remove 立即失效并加入 intrusive retire 队列，关闭由单一 actor 按 entries 游标继续，operation=0/Done/source_id=0 才摘项。来源完成只保存/发布快照，不参与物理退休。
- Register/Rearm 在目标锁内登记 operations，跨来源锁外工作通过 Operation Drop 归还；closing 禁止新操作。普通 Close 在 HandleTable→WaitSet→Lifecycle 同一提交内发布 CLOSED/mandatory，随后摘 Handle、锁外发布 actor；已提交回复 plan 被 Drop 会启动无线程取消，actor 不撤销。pending ticket 持 ObjectRef，Native 与 unpublished 都等独立 completion，actor 只占 progress waiter；旧内部 WaitSet drain 推进路径已删除。
- ABI 同步：shared 调用号与 DrainResult、kernel Seal/Drain 分派、rinlib raw/safe Seal/Drain 已删除；集合只允许 READABLE/CLOSED。KernelObject 提供 retirement backend，不在 Handle 按 WaitSet kind 继续扩大维护分支。静态队列身份 7，runtime 从 8 起。控制面 notification/finish/retirement 同一安全点最多 16 步，有 pending 的后类保留最低份额；idle 包含 actor runnable。
- 来源 CLOSED 不再安装持久槽；终态扫描无未 seen epoch 仍用 CLOSED 当前快照 offer 并摘 full Subscription。WaitSet actor 也有界 pop 来源槽，不等待观察者消费/执行。复核发现旧 Done 后回调读取新 Armed outcome 的 P1，现完成方 Done 前捕获(epoch,outcome)，目标登记缓存且代次校验；Operation Drop 只发匹配缓存，定点复核确认关闭。来源迟到安装 P1 已修；复核续发现 CLOSED/Rearm 已 seen 后 selected=None 绕过退休，闭合分支及后续复核/确定性证据见本节最新记录。
- 真实验收新增 64 注册非空关闭、Remove 立即失效、自观察、交叉观察与 16 轮 Rearm。actor stress 日志 artifacts/check/public-ipc-actor-stress.log 完整 16/16/reset；最终 CLOSED 无候选修复后 core 日志 public-ipc-actor-final-virt.log 通过。普通重构检查不等于所有依赖/容量/退出竞态覆盖。
- 剩余验证真值以本节「下一自然序」为准；上述 actor 迁移最初的待验项已有本轮机制与运行证据，未完成项不由 passing fragment 或 lint 替代。未通过整体组合前不完成 #13，也不启动后续 FAL 业务。

上一轮 Native 请求施工证据：

- 内核请求继续已纵向接通：Process Core 出生预付 reusable drain_waiter；request.rs DrainRequest 捕获真实 Process/Control、输出、一次截断预算和累计 work_done，drain_active 许可跨暂停，Drop 释放许可后通知。ProcessDrain 首轮与恢复同走 finish step，不再通过用户重试或重新解析 Handle；无 Core 的 Dead 快速路径保留。
- finish StepResult 贯通 Native 请求，Blocked 保存原 request/thread 后 arm/register/park；通知和取消按 context+epoch WaitKey 取同一 affine WakeAction，早到取消由注册复检吸收。finish 的 publish/park/wake/finish/rearm 与 Pending 计数同队列临界区，门铃锁外；Native 不复用来源 persistent 属性，容量复用以 reuses_finish 单独表达。
- WaitPlan 未安装即被 scheduler 丢弃时，reusable 轮次登记 abandonment 并用 arm 仲裁启动无线程完成尾段，归还批次许可；单次 Memory 等待不改变原未安装资源归还语义。正常完成先返回 finish 槽、清请求/依赖、DONE，再释放 Native 批次和交付线程；请求 step 在所有 Context 锁外执行。
- Native 结果通过 Waiting 间接访问写回；失败按 Fault/StoreAccess/None-member 终止 Caller，取消保留已完成游标及 pending entry。普通 WaitMany 无业务资源副作用，保留 MemoryNotAccessible 返回，不混淆两种提交契约。
- 复核发现 PublishDead 后 Job 完成传播可能随 Caller/Control 消散而丢失：Process 出生预付 Finalization 槽，Job 摘除前发布独立 Arc<Process> 债务根，后台与 Native 以 drain_gate 仲裁同一终段游标；Native 许可 active 时停驻，Drop 清 active 后 notify。定点 reviewer 确认原 P1 及首次通知时机漏唤醒的新 P1 均关闭。Static TableID 6/runtime >=7 已共同迁移。
- 最新检查：just check 无警告、virt core 通过；Native 继续路径初次 default stress 16/16 通过。加终段 owner 后有一轮 150s 在 Tunnel close round7 超时；随后诊断 300s 完整 stress 16/16/reset，QEMU 实测 162.657s（总 167.232s），日志 artifacts/check/public-ipc-native-final-diagnostic.log 与 timing.json。按实际正常运行成本将 VIRT_STRESS_TIMEOUT 默认重校为 300s，不把基础设施截断当成内核故障；早先超时仍未逐次归因。
- 该轮后续连接已见最新 actor 记录；未覆盖竞态/退款及总体组合门仍不得宣布公共对象前置闭合。

此前施工与边界记录：

- 来源订阅立即命中/拒绝安装、取消、扫描落选、正常完成与 unsubscribe 均返回完整 retired Subscription，来源包装锁外释放；WaitAdvance::finish 统一交接。不是仅在一个调用点延迟 Drop。
- IpcClass 将对象壳、Delivery、Registration、KernelWait 分开；reserve_kernel_wait 提供独立非内存资助。计数仍是前期政策，真实 heap 分配 fallible；对象/交付上限延续 65536 count 政策，注册/等待由完成容量限制，不称为硬件约束或等量字节驻留保证。
- finish 表划为 Thread 8192、Kernel 8320（memory-wait 128 + IPC Wait 8192）、Persistent 8192，共 24704 槽；仍同一 FIFO/预算，不加执行队列。出生 self_test 实际占满 Persistent 物理槽并同时取得 Thread/Kernel 等待，验证退款；不能只占计数宣称物理隔离。新增容量占用是静态固定存储，组合验证继续覆盖 128MiB 板型。
- WaitCore 的 phase/outcome 用原子字携带 epoch，独立 abandon 位可在 FINISHING 登记取消；轮次范围由标记位布局推导，越界不回绕。WaitIdentity/WeakWaitIdentity 捕获同一 epoch，lifecycle、timer、source sink、prepared plan、Memory/Tunnel 完成均保留捕获身份，裸 Arc 回调重新定位入口已删。ReadyRecord 代次直接使用 core epoch，旧 ArmCycle generation 字段已删。
- 修正拒绝 park 与 OUTCOME_WRITING 竞争：abandon 后 arm 仲裁，不强求 READY、不自旋。timeout token 使用低 63 位，关闭位与待注销 token 同原子字，take/expiry CAS 独占责任；旧两原子 pending_cancellation 已删。定点 reviewer 确认这两项及物理槽隔离 finding 关闭。
- Thread finish 分成执行结束与完成交付：队列先释放/归还槽及 Pending，complete_finish 再 DONE、锁外交付 AdmittedThread 或 departure；引用拥有根连续。当前仍单次使用，reviewer 不将此等同于 Native/reusable 已接通。
- 最新 just check 无警告，6 个公共逻辑包 59 项 host 测试通过；默认 150s 的 THROTTLE=100 virt-stress 完整 16/16/reset 通过，实测 QEMU 92.856s（总 97.356s），日志 public-ipc-stress-default.log 与 timing.json。没有因总耗时含冷编译而修改 recipe 超时。此前两次 15/16 命中 KNOWN_ISSUES 的概率场景；一次 150s 在 thread memory suite 后超时未归因，后续若同锚点复现须按原日志/GDB 定位，不能口头认定已修复。失败日志均保留。
- 下一责任：预付 reusable 请求存储、DrainRequest/批次许可、FINISHING 主动依赖取消、finish StepResult/park、Waiting 间接输出故障，以及 object actor/ticket/ProcessDrain 全链。旧 WaitSet Seal/Drain ABI 尚未删除，FAL 业务继续暂停。

以下为此前施工盘点与证据，不代替最新状态：

- 2026-09-13：已修订 Mailbox receiving 预留/commit/rollback 的完整电平发布，写回回滚后锁外通知；rinlib 失败重试独立核验原 Deadline。该路径尚待实际并发/写回失败组合验证。
- `just check` 退出 0，7 条未满足 lint expectation 警告，日志 `artifacts/check/public-ipc-prerequisites.log`。这是内核检查，不是用户态编译或无警告 clippy。
- ordered_table/timer_queue/work_debt/wait_context/metadata_admission/monotonic_id 显式 aarch64-apple-darwin host 测试共 42 项通过，日志 `artifacts/check/public-ipc-host-tests.log`；未覆盖内核接收竞态、ABI 或新退休机制。
- 初始基线追踪（相应入口已在下列本轮记录中迁移）：`task/wait.rs::prepare_memory` 已能预构造 Installing context，`install` 才绑定线程，但资助类型仍固定为 MemoryWaitPermit；`lifecycle.rs::commit_if_current` 仍要求 Running execution snapshot，不能作为普通关闭的通用准入入口。`ArmCycle::return_finish` 归还槽后 mark_done/finish_cycle 是在途退休依赖的真实完成点；`deferred_work.rs` unpublished 目前仍轮询 REAPABLE。这四处必须在同一机制内接通，不能用 Memory 资助伪装、零进度计数或继续自 IPI 掩盖。
- 本轮接通：Memory/Tunnel 使用 KernelWaitPermit/prepare_kernel，资助仍出自各自内存准入；Lifecycle::commit_running 可选择验证执行快照，现有内存调用仍传 Some(snapshot)，普通对象未来使用同一个状态/必成提交门，不伪造快照。
- WorkDebt 增加 affine WakeToken、arm/cancel/park/wake 与 StepResult；早到 Wake 在 Taken 锁存，未交回票据不能 Finish/Rearm/Requeue。内核 Dependency 登记时同步采样真实条件，不缓存第二份电平。Process 预先拥有 reapable_dependency，成员/Building/必成责任的全部实际完成生产者转入 Process::publish_reapable；未发布进程债务已改为停驻及原 owner 队尾唤醒，不再轮询 REAPABLE。
- 静态复核发现 Wake 入队与 Pending 增量锁外分离的跨 hart 交错，已将未发布队列的 Publish/Wake/Park/Finish 和对应计数更新统一放入同一队列临界区；门铃与 Process 最后引用释放仍锁外。不能以当前 bootstrap 同 owner 为理由保留公共并发协议缺陷。定点 reviewer 静态复核确认原 finding 已关闭，未发现新锁阶/票据/计数交错；不替代实际跨 hart 分支验证。
- 检查更新：内核 just check 退出 0 且无警告；6 个公共逻辑包 49 项 host 测试通过（新增 7 项票据/竞态顺序/容量/公平/失败返还用例）；内核检查通过，原 7 条失效 expect(dead_code) 已删除。THROTTLE=100 just virt 构建全部现有用户态消费者并通过 core QEMU 服务监督/reset，日志 artifacts/check/public-ipc-virt.log；仍有既有 dtc/linker 和用户态 deprecated 警告，不是无警告全仓 clippy。
- 构建暴露的 Capability 路径及 prepare_create Position 消费闭包错误已修正，不新增业务功能或兼容导出。
- 连接未完：Dependency/票据还需接 WaitSet 安装/完成退休和 ProcessDrain，同一退休拥有根/普通 Close、专用资助分类及 ABI 删除尚未施工。core 不覆盖新停驻分支的全部竞态或接收写回回滚；本任务保持未完成。

## 已识别缺口与删除门

| 现状 | 目标及位置 | 删除/验证条件 |
|---|---|---|
| 接收电平/原期限重试已收紧，顺序竞争/真实撤页复制失败与回滚唤醒已验证，实际并发组合不足 | mailbox.rs 完整事务与 rinlib message.rs 原期限；真实同进程线程接收/发送干扰 | 运行证明竞争无队列/能力损失及准入泄漏，原期限不被 Busy/NotAvailable 重试延长 |
| actor/ticket、operations 与 ABI 已共同接通，组合覆盖不足 | 最新施工记录三组确定性竞态/退出/容量证据 | 未覆盖实测前不标记对象退休完成；旧路径不恢复作兼容 |
| stress 静默截断与概率覆盖误失败，后续验收完善 | [验收可靠性计划](../todo-2026-09-13-acceptance-reliability.md) 唯一安排调查与改进；本文件仅保留当前证据 | 暂缓不宣称已修复；未绿组合如实报告，不能豁免新发现的公共正确性缺陷 |

## 完成门

普通关闭、ProcessDrain、通知/finish/退休和现有真实调用者均使用同一最终机制；旧维护 ABI/重复清理路径删除；提交前失败保留资源、提交后必成义务不中断、在途安装与完成不泄漏；完整验证证据可定位。此前不得把源码存在视为前置完成，也不得继续扩展 FAL 业务来寻找基础缺口。
