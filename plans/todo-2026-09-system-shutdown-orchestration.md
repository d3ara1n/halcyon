# 系统关机编排设计

状态：**设备首批路线的停止与故障终局已确定，通用持久化服务关机尚未设计完成，未实施**。设备范围来自[设备方案](todo-2026-09-device-system-design.md)，本篇唯一拥有最终 reset、返回错误与根监督者死亡的政策；不新增独立电源管理进程或内核 reaper。

## 问题边界

`SystemReset` 是受 capability 授权的最终整机动作，不枚举服务、不等待设备、不证明 DMA 安全。现有 init 固定验收剧本与 RootSupervisor 提供实际监督基础，不能直接称为通用关机编排。首批设备路线没有持久化写入承诺；文件系统 flush、网络会话和更多长期服务的停止仍须按各自领域接入。

## 设备管理故障的接点

采用 init 作为首批系统停止 owner。init 持平台资源根、管理来源封口权、受托 Job 控制和 SystemReset。状态为 `Running → Stopping → Resetting`，任一不可恢复错误转 `FailureTerminal`，不回到 Running。停止意图通过实际管理入口提交；同一意图重复返回当前阶段，不重启停止流程。启动/发布/驱动管理使用同一个装配准入 gate，Stopping 提交即封闭新系统工作。

### 正常停止

1. init 封新装配与 srv_driver 自动选择，封相应 Claim 来源；设备侧停止新 Get/功能准入与子登记。已提交责任仍可使用清理接口。
2. srv_driver 请求各驱动停止激活；srv_device/Provision 按真实父子和供应顺序停止绑定。子会话完成前，父 provider 保留其清理服务，不能先杀父驱动再期待叶驱动协作。
3. 对有响应的控制器完成硬件 halt、在途收尾、DMA 显式退休、IRQ/映射撤销；只有 Provision Closed 才允许 Binding Released。停止期限由装配配置和平台规范共同约束，期限到只触发升级。
4. 全部必需绑定安全 Released 后，init 收束受托驱动/管理 Job，保留最终控制根至最后动作。Registry 撤名不充当设备安全完成，也不阻塞硬件停止。
5. 正常 Shutdown 或完整 Cold reboot 按请求提交 `SystemReset(Requested)`。成功不返回；失败进入下述终局，不能重新开放系统。

首批所需拓扑由实际登记/Binding/激活关系和受托 Job 集合枚举，不在 init 手写每个设备的关闭序列。各 owner 已准入的清理任务推进到终态，init 观察结果，不负责逐条 pump。

### 无法证明安全的系统故障

srv_device 失效、受信 DMA 主控死亡且无隔离证据、必要停止失败或管理期限耗尽，均进入 FailureTerminal。init 先封新装配、停止受影响执行、封资源新使用；保留可能仍被设备访问的 mapping/backing/charge，不假装完成 Released 或退款。srv_device 的用户态表不可用时，不重建表后继续分配。

随后允许提交 `SystemReset(ColdReboot,SystemFailure)`，这是故障升级，不宣称优雅停机已完成。选择 Cold 而非 Warm，依据 [SBI 3.0](../references/normative/riscv-sbi-v3.0/src/ext-sys-reset.adoc)的完整复位语义；首条 QEMU 路线须实际验证该叶动作。Reset 返回错误或不支持时，init 保留错误诊断，并终止自身以进入下述根终局；不循环无限重试，也不提前释放隔离资源。

### 根 init 死亡

目标增加 **boot owner 的预付终态观察**，不是公共 Process 的 PID1 特判。初始进程发布前备好观察节点、Work 与启动账户；Start 成功后由 boot 后继 owner 继续持初始 ProcessShell 的观察来源，等待实际 `DONE`。现有 Shell 只公开 DONE，本方案不声称已有 Frozen 通知。

观察期间 Work 停驻，不占 runnable 队列、不轮询。hint 只唤醒，在锁外 Work::step 核对终态后调用既有 `runtime_stop::request/check`，不在 hint 内停机；不遍历用户进程、不回收设备、不重建 init。这个 owner 不持初始 Process 的 live capability 或 Body 借用，不得因观察本身阻止其 DONE；持的是允许终态观察的稳定来源/壳。费用按系统启动寿命保留。

这一目标复用[Shell 来源](../os/kernel/src/task/process/shell.rs)、[Bootstrap Work/HintTarget](../os/kernel/src/boot/bootstrap.rs)、[等待源约束](../os/kernel/src/task/wait_source.rs)和[runtime_stop](../os/kernel/src/runtime_stop.rs)，**现有代码尚未接上初始进程运行期终态观察**。现有 runtime_stop 在锁外安全点停驻并记录 PARKED，不是已经完成的全 hart 屏障；IPI 失败亦不能据此宣称所有 hart 已停。

安全不依赖观察到 DONE 的速度：设备方案要求活跃 DMA mapping 的稳定 owner 保活 backing/原 charge，控制 handle 全部 Close 只封准入并形成静态有账保留，不能触发 unpin。其他 hart 在停驻传播窗口推进 init 或驱动回收也不能复用这些页。该保留无可推进工作，不伪造一项永远 runnable 的维护债务。IRQ 控制孤立则由内核按完整 claim/complete/disable 顺序封源。

根终局不再运行自动恢复或内部 reset；设备可能继续访问已 pin 的内存，但不得复用这些页。结束保留需要外部复位/平台看门狗。首批受信物理 DMA 不保证抵抗恶意主控写任意内存，故不能把这里称作 IOMMU 级隔离。

## 施工接点与验证

设备资源实现必须先提供最后 Close 不退休活跃 DMA 的不变量，再接根观察；不能以“先停住所有 hart”替代 pin owner。boot 观察和资源机制一起纳入设备首批闭包，不另造恢复任务。实际 Job/进程停止、Reset capability 与错误保持复用已有 owner。

验收至少覆盖正常有在途 I/O 的停止、叶/控制器死亡、srv_driver/srv_device 死亡、init DONE、Reset 失败和 IPI 传播失败。核对页/费用不提前复用、无新准入、旧对象不恢复；不能要求故障保持路径全部退款。QEMU 只证明所选模拟平台，真实 DMA halt/隔离另行取证。

## 通用关机的剩余范围

正式持久化服务接入前，继续设计其 drain/flush、依赖、拒绝/失败、期限和关机许可，验证实际数据守恒与恢复行为。独立电源服务只有明确需求才由[电源计划](todo-2026-09-power-management-service.md)立案，不是本案默认前置。一般服务热重启、睡眠/唤醒、分设备域恢复不由本次设备终局自动获得。

本次仅设计和源码核对，未修改内核或运行 reset。后续按设备计划的统一施工顺序实施所选闭包；本篇继续保持活跃，不能因设备故障终局已定而把全系统优雅关机登记完成。
