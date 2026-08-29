# 显式系统复位与 power 服务

> 【当前待实施计划】ThreadSpawn 前的独立收口项。本篇只编排工作；先完成
> `notes/ideas/` 方向设计并经确认，再进入 ABI、内核和用户态实现。当前不以
> 本计划内容冒充已经落地的 ideas/impls。

## 问题与目标

当前调度器把「全系统暂时没有可枚举 waker」直接解释为关机意图：idle 路径
满足 quiescent 谓词后调用 SBI SRST。该机制同时承担运行时政策和 QEMU 验收
收束，已经出现负载尚存活时提前停机的低频现场；调查记录见
[`todo-2026-08-29-early-quiescent-shutdown.md`](todo-2026-08-29-early-quiescent-shutdown.md)。

目标是删除这项推断：idle 只负责等待可到达的唤醒源，系统 shutdown/reboot
必须由持有明确 authority 的用户态政策显式提交。验收也从「测试结束后碰巧
进入全局 idle」改为「init 证明验收和资源收束完成后命令 power 服务提交
reset」。

## 已确认方向

1. **仅显式复位**：删除 quiescent → SRST 正常路径；不保留 boot flag、测试
   特例或两阶段 quiescent 通知。
2. **capability 授权**：普通进程不能仅凭 syscall 号复位系统。内核依据平台
   事实铸造系统复位 authority，经 initial capability 图交付 init，再由 init
   显式授予 power 服务。
3. **政策与机制分离**：init 拥有监督拓扑、服务收束与最终时机政策；独立、
   单线程 power 服务只持最终复位 authority 并执行平台动作，不取得 root
   JobControl。
4. **本阶段范围完整而窄**：覆盖 shutdown、cold reboot、warm reboot 及失败
   返回；设备 runtime PM、suspend、thermal、电池和物理电源键留到设备/中断
   设计，不在本机制里预留半实现入口。
5. **失败不伪装成功**：SBI 成功按规范不返回；`INVALID_PARAM`、
   `NOT_SUPPORTED`、`FAILED` 或成功后异常返回均转为明确失败，不以永久
   `hart::park()` 冒充已关机。

外部契约以 `references/normative/riscv-sbi-v3.0/src/ext-sys-reset.adoc`
「System Reset Extension」为准：标准 reset type 为 shutdown/cold reboot/
warm reboot，标准 reason 为 no reason/system failure；成功同步调用不返回。

## 设计阶段（先确认，后编码）

### 1. authority 与 ABI

形成 `notes/ideas/power.md`，并同步 `notes/ideas/{kernel,bootstrap,object}.md` 中
可由该设计推出的边界。设计至少回答：

- 系统复位对象的 core/role/rights、是否可 duplicate/transit/grant；
- initial capability 图如何向 init 表达该 authority；
- reset type/reason 的 shared 固定宽枚举、保留值和错误映射；
- 成功不返回、失败返回的 syscall 契约；
- 对象关闭为何是固定上界叶子操作。

### 2. 用户态拓扑与协议

power 服务必须活到最终 reset 提交点，不能先被「全部工作负载已收束」阶段
一起 Drain。设计需明确其持久控制域、init 的监督/重启 authority、请求协议及
reset 失败后的稳定状态。不得借 PID、进程名或隐藏 syscall 白名单授权。

### 3. idle 终态

删除全局 quiescent 关机后，各 hart 无工作时只按 timer/IPI/设备唤醒所有权
进入 WFI。纯 IPC 等待无发送者时可以永久等待；这是用户态政策停滞，不再被
内核解释为关机请求。

## 实施批次（设计确认后）

1. **shared 与内核机制**：固定宽 reset ABI、系统复位对象与 Handle role、
   bootstrap authority、syscall 分发、SBI 错误映射及 host 可测纯逻辑面。
2. **用户态政策**：rinlib 封装、独立 power 服务、init capability grant 与
   最终请求协议；失败路径保持可诊断、可继续监督。
3. **调度与验收迁移**：删除 `is_quiescent` 和 idle 内 SRST；迁移 QEMU 终态
   锚点与平台收割规则；同步 notes/impls、KNOWN_ISSUES 与 COMPASS。

每批独立验证、独立提交；不得在第一批留下只能靠后续批次才能退出的中间
验收状态。

## 验收语义迁移

现有 `tools/qemu-acceptance.sh` 同时要求业务/资源锚点与
`system quiescent (no waker)`。迁移保持前者不变，只替换最后的终态证明：

- `drain minimum-budget acceptance passed`；
- `race matrix acceptance passed: 10/10 scenarios passed`；
- `acceptance domain collected`；
- `all services supervised to completion`；
- `peer closed observed`；
- `pm delegated domain confirmed Dead`；
- 新的「显式 reset 已提交」终态锚点。

终态锚点只能在 init 已观察上述验收事实并完成应收束对象后，由最终 power
路径发布。脚本仍在 QEMU 退出或主动收割后逐项检查完整集合，因此过早请求
reset 会缺失前序锚点并 fail-closed，不会把早退当成功。

平台判定：

- **virt**：显式 SRST 必须使 QEMU 正常退出；不退出或返回错误即失败；
- **sifive_u**：若平台无实际 shutdown 后端，power 路径记录明确的
  `NotSupported/Failed` 终态，acceptance wrapper 只在终态锚点后主动收割，
  随后仍检查完整必需锚点；
- reset 提交点保留可 grep 的空闲帧数观测，使资源守恒判据不因删除
  quiescent 日志而消失。

## 完成标准

- 方向设计已进入 notes/ideas 并经确认，实际机制进入对应 notes/impls；
- 用户态无 authority 调用得到权限错误，持 authority 的标准 type/reason
  行为符合 SBI 固定规范；
- 内核正常运行路径不存在 quiescent → SRST，也不存在 reset 失败后 park；
- power 服务与 init 的监督/授权关系完全由 capability 图和用户态协议表达；
- 原有业务锚点全部保留，virt/virt-release/hetero/nofd/sifive_u 验收迁移全绿；
- 提前停机由结构删除而不可达，连续压力轮次不存在业务锚点前的 reset；
- `todo-2026-08-29-early-quiescent-shutdown.md` 与对应 KNOWN_ISSUES 条目收口；
- COMPASS 转向用户内存映射前置任务。
