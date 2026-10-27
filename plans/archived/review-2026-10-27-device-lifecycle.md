# 设备第二轮收敛后的生命周期与观察审查

状态：本组发现已修复并复核，所选组合验证及交付前结构检查完成，归档。用户已确认本轮审查结论并授权改造；基线为 `a5a1a7893872e73d410eecb38b7ea21aed7d1616` 及其上未提交的第二轮实现。本篇保存本组新发现、设计处置、修复与验证；上一轮[收敛档案](review-2026-10-27-device-implementation.md)保持历史。设备总体目标及原开放验收门仍由[设备主计划](../todo-2026-09-device-system-design.md)拥有。当前改动尚未提交或 push。

## 范围与原始发现

本轮覆盖 libdriver 逐轮生命周期、PCI/xHCI/HID 接线、srv_driver Managed、librpc 订阅及 Dispatcher 消费者的失效清理分支。保留 Seat/Provision/Activation/Process 的独立事实、原 Use 授权、DMA 安全证明和单 root 监督；不新增运行体、通用事务、恢复账本或硬件能力，不修改硬件停止协议。已有未提交代码和其他文档整理均保留，不提交或 push。

| 编号 | 原始证据与可达路径 | 影响与处置目标 |
|---|---|---|
| L1 | 三个 driver Request 把 QueryActivationResult 与 Activate 共走接纳并 Wake；BindingTask Complete 后只置 lifecycle_done，旧 ID 仍存 | 终态或迟到结果查询可 Wake 已退休任务并触发 fatal。查询只读，首次接纳才唤醒；退休撤销可唤醒身份 |
| L2 | libdriver/binding.rs 在收到 Released 后进入 closing，仍把主动 cancel 的 Shutdown 当普通业务错误 | 正常结束被升级为 worker 故障。明确运输退场阶段，只领取/丢弃原调用完成并退观察，不再采用业务结果 |
| L3 | xHCI Get 的 Closed 透过 Inputs::adopt，外层以当前 Activation 已 Stopping 为由 fatal | 正常 Stop/Get 顺序决定是否故障。封口拒绝交原停止 owner，真实硬件/协议错误仍单独处理 |
| L4 | Managed 先进入 Stopping，迟到 InstallFunction 成功又写 Ready/无限期限 | 状态回退、停止期限重新起算。操作完成只提交事实，统一转换优先兑现单调停止 |
| L5 | Binding 观察同步失败可拒绝 Preparing，实际 Source 准入失败却经宿主 refused 直接 fatal | 接通原 ConsumerTask 的拒绝回执，按是否取得 IO 许可决定实例拒绝或故障 |
| L6 | ConsumerTask::new 丢初始快照，仅存 pending；Ack 回复丢业务快照，再 Query | 直接消费完整初始/更新快照，保留序号确认、终态与有界推进 |
| L7 | Dispatcher 只交已注销来源的完成，错误为 Unsent(Request) 或 Sent(None)；Managed/Binding Work 仍保存 ReplyCleanup 重试状态 | 删除不可达 cleanup/retry_at。同步 Caller 的清理合同保留，不为简化丢真实取消/来源退休责任 |

以上为本会话静态审查结论，尚未运行复现；具体实现与新证据在下节登记，不把历史验收当本轮验证。

## 目标设计与责任链

1. **逐轮提交边界**：ActivationSlot 私有保存 IO 准入和生命周期执行身份，通过有限方法提交核验成功、准备拒绝和生命周期退休。只读结果请求只核管理身份、BindingRef 并读取回执，不调用接纳、不分配实例、不唤醒后台任务。Activate 冻结输入的幂等校验保留，只有首次接纳需要推进准备。硬件事实仍由原 Engine 的明确提交点产生。
2. **运输退场**：BindingTask 只有业务推进和不可逆退场两种执行模式。Preparing 拒绝或真实 Released 转入退场后，原在途 txid 由同一任务 cancel/take，迟到成功与取消错误均只销毁其运输结果；观察 stop 后等待实际 Complete。最后才释放准备输入、撤销生命周期执行身份并通知 Engine；不以取消或 Closed 合成 Released。任何已提交来源的实际退休仍由 Runtime/Dispatcher 承担。
3. **失败归属**：BindingTask 接收宿主针对自身的 Source/Wake 拒绝，转交已有 ConsumerTask；准备尚未允许 IO 时走同一拒绝收尾，已允许 IO 的观察故障仍升级。xHCI 对 Get 被封口的合法 Closed 按原停止路径推进，严格校验错误帧，不把权限/格式/硬件故障视作停止成功；Master/登记的停止交错按各自实际效果核对。
4. **Managed 转换**：远端快照、停止意图及已完成操作先进入原 Run/Managed 事实；生命周期转换集中决定阶段与下一动作。首次停止记录绝对期限，此后成功回复只能增加原操作的完成事实，不能重开 Ready 或移动期限。Released 仍必须来自原 Binding 观察；待在途调用和两个观察真实脱离后才解除进程关联。
5. **订阅快照**：初始快照进入 Reading；消费者存入自己的事实缓存后确认该序号。Ack 返回仍 pending 的完整更新快照直接进入 Reading；没有更新才重新监听，已确认终态转退休。每次 advance 仍只推进一个动作，不递归追更新；Scope 的枚举用途继续由实际消费者解释。迟到旧 Ack 不清新变化，错误/取消不创造终态事实。
6. **运输类型和费用**：Dispatcher 的完成保证不交未注销的观察来源，消费者只释放返还的未发送请求；删除专属于同步 Caller 私有回复口的清理分支。沿用现有任务/来源/实例预算，不新增公共能力或第二套 owner。终态记录仍按实际别名与请求寿命计费。

实施顺序按上述公共合同→驱动/管理消费者→验证与说明回填安排；中间编辑不要求独立验收。使用 actor 单一所有权，无新增跨 hart 共享状态或锁阶；内核 ABI、硬件/USB 规范和关机终局不变，外部契约不新增。

## 验证门与接力

新增验证必须捕获上述具体交错，预期来自终态查询无副作用、停止单调、真实 Released 优先于运输回复及序号确认合同。优先复用已有纯逻辑测试、真实驱动及 device-reuse/unknown 路线；不另造测试服务或模拟运行体。执行受影响 host 测试、`just check`、`just clippy`，并按最终修改运行必要的设备热插/复用/准备拒绝和回复缺失组合。精确交错若不能用现有设施稳定命中，分别说明静态证明与运行覆盖边界，不冒称已经注入。

公共生命周期、三个驱动、Managed、订阅和无效清理分支已迁移；impls 已按当前代码回填，组合验证及交付前检查已完成。本轮没有剩余施工步骤；原 DMA＋IPI 和时钟异常仍由主计划拥有。

## 实施与开发证据

- Lifecycle 用 Pending/Preparing/Active/Retired 同时约束 IO 准入和执行身份，替换公开的 task/done/io_ready 组合；Activation 仅提供只读借用，Engine 经 Slot 具名方法提交事实和通知。QueryActivationResult 三个实际入口均只读取原记录，HID 外层请求收尾也不对结果查询发 Wake。
- BindingTask 先判运输退场再处理业务结果；拒绝或 Released 后仍等原 call/watch 退休，不解释迟到结果。宿主将 Binding 的异步 Source/Wake 拒绝交回原观察任务。初始快照直接交付后，BindingTask 在确认读取时显式保持 Runnable，确保 Ack 不依赖偶然事件。
- Managed 在领取结果前后统一协调已观察状态和停止意图；迟到安装成功不直接修改公开阶段/期限。附带修正同一责任链的 Claim/Stop 交错：取得 Control 后先建 Binding Watch，再提交停止，避免跳过唯一 Released 观察出口。
- PCI/xHCI 资源/登记及 Master 的合法 Closed 沿已有停止 owner 收束；xHCI 资源拒绝严格核零正文/零能力。明确拒绝的 xHCI 未发布登记关闭自己的 provider，Controller 继续持硬件清理。
- 删除 Managed、中央 Binding Work、xHCI 以及 srv_driver Retry 消费者中的 Dispatcher 回复口清理分支。只收窄实际调用者假设，未改变同步 Caller 的错误类型及清理合同。
- 编译工具链：`rustc 1.101.0-nightly (19833af72 2026-10-20)`。目标开发检查通过，日志 `artifacts/lifecycle/development-check.log`。新增两项 host 用例使用实际 Lifecycle，分别核退休后无任务/IO 权及准备拒绝后迟到许可不能复活，依据 L1/准备不可逆合同，不伪造 kernel handle。
- `libdriver/libdevice/libusb/librpc/libfs/libservice` 共137项 host 测试通过，`artifacts/lifecycle/host.log`；`just check` 通过，`artifacts/lifecycle/check.log`。首轮 Clippy 报两个已失去消费者的 World::changed，已删除；原失败日志保留为 `clippy-first.log` 与 `clippy-user-target-failed.log`。随后十分面 `just clippy` 全通过，`artifacts/lifecycle/clippy.log`。
- 首轮完整 core：`THROTTLE=100 just acceptance-virt debug` 退出0，入口 `artifacts/run/20261027-183418-58869-acceptance-virt-debug.log`。原 init 的 FS Watch 用例已包含读取后更新、旧 Ack 对新快照及终态消费，服务目录用例覆盖实际注册/撤出，均继续复用原设施。其余路线原始退出码由 `artifacts/lifecycle/validation-status.json` 记录，未结束前不计通过。

本轮不新增产品测试接口或新的模拟执行器。WorkerControl 只由 srv_driver 持有，现有 test_device 无权直接查询旧 Activation 结果；终态纯查询及 ReportQuiescent/Released 的精确回包交错先以真实代码责任链核对，不把通常的复用/停止通过当作已指定注入这些窗口。异步观察准入拒绝同样区分代码分派检查与实际故障注入覆盖。

## 交付前结构检查

主代理亲核最终实现、三组实际驱动接线、Runtime 的注册/注销回执、Dispatcher 完成出口和 init 的已有订阅消费者；限定辅助检查 sub-3 对新实现做独立上下文检查，无新增可达 finding。该辅助检查不是提交后的固定版本独立 Review。

| 发现 | 代码处置与复核 |
|---|---|
| L1 | QueryActivationResult 三个分支只调用 query_result/receipt，HID 外层不 Wake；Lifecycle 退休后 task 返回 None，host 核退休身份/IO 权不可恢复 |
| L2 | drive 先判 closing，retire 不解码迟到业务结果；cancel 后仍 take 原 txid，watch 真实 Complete 后才退休 Slot，不丢来源或输入 owner |
| L3 | 合法 Closed 在取得/登记/启 BME 的操作消费点处理，沿原 Engine stop/硬件清理；畸形错误、权限错误及清理失败不被吞掉，不合成静止 |
| L4 | consume 不再写 Ready/无限期限；reconcile 为 Ready/Stopping 及首次停止期限唯一转换点，Retire 只写 Terminal；Released 后等原调用和观察退场 |
| L5 | 三个 Family::Binding 的 refused 转到原 ConsumerTask；源码核准备阶段拒绝与已准入 IO 的故障边界，未伪造系统级 Source 分配成功 |
| L6 | 初始/Ack 更新都交 Reading；BindingTask 和 Subscription 确认后 Runnable，init 显式确认后 wake；订阅未准入取消接受初始 Reading，不新造 Query |
| L7 | 核 Dispatcher 只在 clean 后交完成，未发送请求随错误值释放；删除的 retry_cleanup 没有实际 ReplyCleanup 来源，同步 Caller 继续持原合同 |

任务、capability 与付款者保持原有归属；没有新运行体、跨线程状态或锁序。业务终态与运输尾部继续独立，实际请求及观察退出后才释放原存储；旧接口、原可变生命周期字段、两个 World::changed 转发和不可达清理状态已删除。`git diff --check` 与用户 workspace 格式检查通过。全部本轮改动源码的当前指纹保存在 `artifacts/lifecycle/source-version.json`，后续验证结束核对没有源码漂移。

## 最终组合验证与交付边界

下列路线均使用 `THROTTLE=100`，原始退出码均为0；没有为本次验收修改产品协议、扩大预算、增加测试专用角色或放宽锚点。

| 路线 | 本轮覆盖 | 原始入口日志（仓库根路径） |
|---|---|---|
| acceptance-virt debug | PCI/xHCI 装配/停止、FS/service 真实订阅及完整 core | `artifacts/run/20261027-183418-58869-acceptance-virt-debug.log` |
| acceptance-device-reuse | A/B/C 多 Binding、旧入口尾部、真实 HID fault、原轮释放及 Pool 退款 | `artifacts/run/20261027-183601-62226-acceptance-device-reuse.log` |
| acceptance-device-candidates | 原负候选、准备期间取消、显式选择及复用闭包 | `artifacts/run/20261027-183817-66639-acceptance-device-candidates.log` |
| acceptance-device-unknown QueryUse | 首次权限核验回包缺失，未 IO 准备拒绝并退原轮 | `artifacts/run/20261027-184054-71430-acceptance-device-unknown-QueryUse.log` |
| acceptance-device-unknown GetResource | 实际取得回包缺失，暴露事实不撤回、原 Use 真实报告后收束 | `artifacts/run/20261027-184240-75036-acceptance-device-unknown-GetResource.log` |
| acceptance-device-unknown Master | 原 function/context/绝对期限恢复，真实控制器及 core 收束 | `artifacts/run/20261027-184430-78662-acceptance-device-unknown-Master.log` |

汇总为 `artifacts/lifecycle/validation-status.json`；host、check/clippy 与失败的首次 lint 证据按上节保留。核对源码指纹无漂移；`process-cleanup.log` 确认验收 runner、QEMU、GDB 和设备操作者均未遗留。没有新增锁或内核工作、硬件/ABI 变更，本轮不重复平台、DMA＋IPI 和时钟调查路线。

本次承诺的生命周期收口及观察简化已接通实际消费者，L1–L7 没有留下兼容分支、半迁移或待接通 owner。终态查询、Released 早于报告回包、停止早于迟到安装回包、异步 Source 准入失败的精确调度窗口以静态责任链和实际生命周期 host 用例复核，未宣称逐窗做过动态注入；实际设备组合覆盖范围以上表为准。旧硬件安全条件及原两项独立开放问题没有因此关闭。

提交需要单独授权；取得固定提交后，按 REVIEW 登记包含本次收口的完整代码交付独立审查，不以这次辅助检查替代。
