# 系统关机编排设计

状态：**设备首批 DMA 保留、boot 的 init DONE 观察及 reset 拒绝后的自终止已接；第二轮停止接线与root唯一监督已完成本轮实现及所选验证，通用持久化关机未完成**。设备范围及原验证来自[设备方案](todo-2026-09-device-system-design.md)，控制面目标见其第二轮设计，持续监督目标见[服务装配](todo-2026-10-07-service-assembly.md#第二轮持续监督设计2026-10-27)。本篇拥有最终 reset、返回错误与根监督者死亡的政策；不新增独立电源管理进程或内核 reaper。

## 问题边界

`SystemReset` 是受 capability 授权的最终整机动作，不枚举服务、不等待设备、不证明 DMA 安全。现有 init 固定验收剧本与 RootSupervisor 提供实际监督基础，不能直接称为通用关机编排。首批设备路线没有持久化写入承诺；文件系统 flush、网络会话和更多长期服务的停止仍须按各自领域接入。

## 设备管理故障的接点

采用 init 作为首批系统停止 owner。init 持平台资源根、管理来源封口权、受托 Job 控制和 SystemReset。状态为 `Running → Stopping → Resetting`，任一不可恢复错误转 `FailureTerminal`，不回到 Running。停止意图通过实际管理入口提交；同一意图重复返回当前阶段，不重启停止流程。启动/发布/驱动管理使用同一个装配准入 gate，Stopping 提交即封闭新系统工作。

### 正常停止

1. init 封新装配与 srv_driver 自动选择，封相应 Claim 来源；设备侧停止新 Get/功能准入与子登记。已提交责任仍可使用清理接口。
2. srv_driver 提交原 Binding 停止，驱动从原轮观察停止新业务；srv_device/Provision 按真实供应责任推进。子会话完成前父 provider 保留清理服务，不能先杀父驱动再期待叶驱动协作；不再另发 StopActivation。
3. 对有响应的控制器完成硬件 halt、在途收尾、DMA 显式退休、IRQ/映射撤销；只有 Provision Closed 才允许 Binding Released。停止期限由装配配置和平台规范共同约束，期限到只触发升级。
4. 全部必需绑定安全 Released 后，init 收束受托驱动/管理 Job，保留最终控制根至最后动作。Registry 撤名不充当设备安全完成，也不阻塞硬件停止。
5. 正常 Shutdown 或完整 Cold reboot 按请求提交 `SystemReset(Requested)`。成功不返回；失败进入下述终局，不能重新开放系统。

首批所需拓扑由实际登记/Binding/激活关系和受托 Job 集合枚举，不在 init 手写每个设备的关闭序列。各 owner 已准入的清理任务推进到终态，init 观察结果，不负责逐条 pump。

消费者侧确认按设备第二轮 StopRequirement 与原 Use 窄权限持有：已 Stopping、Get 门已封且资源能力从未交付的轮次，由原 Work 证明未产生访问；需要消费者配合且已可能交付时，等待认证的原轮 ReportQuiescent。PCI 最后 ECAM/配置访问和仍需父资源的子清理先于报告，父资源 Revoke 晚于报告，worker 不等自己 Released 才报告。Binding 将该事实并入单调 StopProvision，不再另发 ConfirmStopped；它不能替代 provider 的 Prepare 效果、后代 DMA 或实际资源完成，缺证路径仍进入故障终局。

停止入口只交给 init 的受权管理 context，普通键盘客户端或驱动不持 SystemReset。正常重复请求返回当前阶段；停止中收到故障升级只能进入 FailureTerminal，不能另启动一条并行关机流程。装配 gate 封闭后，srv_driver 停止选择、srv_device 封来源并阻止新 Claim/子登记；正在启动/准备的记录已经属于原监督/Binding 集合，迟到成功只进入停止，不逃出收束范围。这里不要求跨进程原子广播：每个接收者的封口回执与其封口前已提交责任一起纳入等待，未回执者按失联/期限策略升级。

观察已封来源的完整收束不能只取一次设备树快照。srv_device 的源停止任务持既有成员游标并计入在途 Preparing，终态只在全部绑定 Released、子来源封口和迟到准备责任闭合后给出；仅观察控制壳尚未退款不阻止硬件停止完成。init 先取得这些领域完成事实，再关闭驱动 Job，避免先杀父 provider 后等待它完成子会话的循环依赖。

### 无法证明安全的系统故障

srv_device 失效、受信 DMA 主控死亡且无隔离证据、必要停止失败或管理期限耗尽，均进入 FailureTerminal。init 先封新装配、停止受影响执行、封资源新使用；保留可能仍被设备访问的 mapping/backing/charge，不假装完成 Released 或退款。srv_device 的用户态表不可用时，不重建表后继续分配。

RootSupervisor 独占不可逆故障终局和唯一动作 authority；实际装配等待使用原 Assembly，观察线程及跨线程终局争用已删除。实现与所选验证见[收敛交付档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)。原 serve_forever 的 Shutdown/无限清理循环不充当此路径。真实 JobSeal 封迟到 Start，来源有界封口、平台根 Seal 及原 JobCollector 停止均不伪造全系统 DONE；不等所有退款/Job 完成才允许升级。

随后允许提交 `SystemReset(ColdReboot,SystemFailure)`，这是故障升级，不宣称优雅停机已完成。选择 Cold 而非 Warm，依据 [SBI 3.0](../references/normative/riscv-sbi-v3.0/src/ext-sys-reset.adoc)的完整复位语义；首条 QEMU 路线须实际验证该叶动作。Reset 返回错误或不支持时，init 保留错误诊断，并终止自身以进入下述根终局；不循环无限重试，也不提前释放隔离资源。

### 根 init 死亡

**boot owner 的预付终态观察**由 `boot/watch.rs::InitialWatch` 承担，公共 Process 无 PID1 特判。初始进程发布前备好观察节点、Work 与启动账户；Start 前挂入独立来源，Start 成功后由该 boot owner 继续等待实际 `DONE`。Shell 只公开 DONE，不声明已有 Frozen 通知。

观察期间 Work 停驻，不占 runnable 队列、不轮询。hint 只唤醒，在锁外 Work::step 核对终态后调用既有 `runtime_stop::request/check`，不在 hint 内停机；不遍历用户进程、不回收设备、不重建 init。这个 owner 不持初始 Process 的 live capability 或 Body 借用，不得因观察本身阻止其 DONE；持的是允许终态观察的稳定来源/壳。费用按系统启动寿命保留。

观察记录在初始进程 Start 前准入，并先 attach 再检查 Shell 终态，不能等用户已开始运行后才准备节点。Bootstrap 启动完成把既有 boot 责任移交该观察记录，释放装载临时存储；Start 成功与初始进程立即 DONE 的交错不会丢终态。来源绑定已 terminal 时直接推进，同一 hint 重复只唤醒原 Work。记录计费和来源引用随 boot 生命周期，不拿初始 Process 的 ingress/receipt lease 保活它。

这一接线复用[Shell 来源](../os/kernel/src/task/process/shell.rs)、[Bootstrap Work/HintTarget](../os/kernel/src/boot/bootstrap.rs)、[等待源约束](../os/kernel/src/task/wait_source.rs)和[runtime_stop](../os/kernel/src/runtime_stop.rs)。runtime_stop 在锁外安全点停驻并记录 PARKED，不是先验完成的全 hart 屏障；IPI 失败亦不能据此宣称所有 hart 已停。固定 QEMU 中实际收齐四个 park 的结果只作为对应路线证据。

安全不依赖观察到 DONE 的速度：设备方案要求活跃 DMA mapping 的稳定 owner 保活 backing/原 charge，控制 handle 全部 Close 只封准入并形成静态有账保留，不能触发 unpin。其他 hart 在停驻传播窗口推进 init 或驱动回收也不能复用这些页。该保留无可推进工作，不伪造一项永远 runnable 的维护债务。IRQ 控制孤立则由内核按完整 claim/complete/disable 顺序封源。

根终局不再运行自动恢复或内部 reset；设备可能继续访问已 pin 的内存，但不得复用这些页。结束保留需要外部复位/平台看门狗。首批受信物理 DMA 不保证抵抗恶意主控写任意内存，故不能把这里称作 IOMMU 级隔离。

## 施工接点与验证

设备资源实现必须先提供最后 Close 不退休活跃 DMA 的不变量，再接根观察；不能以“先停住所有 hart”替代 pin owner。boot 观察和资源机制一起纳入设备首批闭包，不另造恢复任务。实际 Job/进程停止、Reset capability 与错误保持复用已有 owner。

验收至少覆盖正常有在途 I/O 的停止、叶/控制器死亡、srv_driver/srv_device 死亡、init DONE、Reset 失败和 IPI 传播失败。核对页/费用不提前复用、无新准入、旧对象不恢复；不能要求故障保持路径全部退款。QEMU 只证明所选模拟平台，真实 DMA halt/隔离另行取证。

Cold reboot 路线由外部验收观察真实 QMP reset/重新启动边界后结束本次实例，不沿用 Shutdown 的进程退出判据；Reset 返回错误才走返回错误分支。init DONE/全控制 Close 路线要观察 MayBeExposed mapping 仍由稳定 owner 持有、无可运行的孤立回收债务；尚未交付地址的 Pin 准备可以安全取消，不应为了故障保留而制造无必要泄漏。具体运行入口和证据由设备主计划维护。

## 通用关机的剩余范围

正式持久化服务接入前，继续设计其 drain/flush、依赖、拒绝/失败、期限和关机许可，验证实际数据守恒与恢复行为。独立电源服务只有明确需求才由[电源计划](todo-2026-09-power-management-service.md)立案，不是本案默认前置。一般服务热重启、睡眠/唤醒、分设备域恢复不由本次设备终局自动获得。

设备第二轮停止接线已完成，首批剩余验证门继续由[设备唯一收口表](todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)承接。本篇保持活跃，下一步在持久化服务或新的停止需求明确后闭合其接点；根终态与设备停止接通不表示全系统优雅关机完成。
