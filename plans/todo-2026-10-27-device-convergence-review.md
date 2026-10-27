# 设备第二轮完整交付的独立 Review

状态：待独立审查。代码交付已固定，本登记与导航更新不改变实现；后续主线按[设备调查接力](todo-2026-09-device-system-design.md#第二轮提交与调查接力)推进 DMA＋IPI 组合及独立时钟异常，本审查单独安排。

## 固定对象

- 分支：`refactor/system-redesign`。
- 基线：`a5a1a7893872e73d410eecb38b7ea21aed7d1616`。
- 目标：`c5d005dc7e6c64a74f0bb7e5874494c4532db4b0`，`refactor(device): 收敛逐轮控制与持续监督并闭合生命周期`。
- 范围：基线到目标的完整交付，包含第二轮机制替代、交付后整理、生命周期收口及相关文档/验收工具。后续登记提交不属于新的产品实现，不另为它登记 Review。

本次不同于[首轮固定检查点审查](archived/todo-2026-10-27-device-implementation-review.md)：该审查已由[原发现及第二轮处置档案](archived/review-2026-10-27-device-implementation.md)结束；此次用新上下文检查最终替代结构，不把旧 C 编号原样当作当前缺陷。新增 L1–L7 的修复与交付前复核由[生命周期档案](archived/review-2026-10-27-device-lifecycle.md)保存。

## 目标与审查范围

审查判断最终实现是否兑现已确认的权限、所有权、活性、失败与费用合同，以及是否还存在可通过共同机制消除的分支和重复事实。以代码为真值，[设备第二轮目标](todo-2026-09-device-system-design.md#第二轮收敛设计2026-10-27)、[生命周期收口目标](archived/review-2026-10-27-device-lifecycle.md#目标设计与责任链)及[持续监督目标](todo-2026-10-07-service-assembly.md#第二轮持续监督设计2026-10-27)为设计依据；当前实现说明见 [device](../notes/impls/device.md)、[rpc](../notes/impls/rpc.md)、[startup](../notes/impls/startup.md)、[runtime](../notes/impls/runtime.md)。

| 切面 | 实际范围与重点 |
|---|---|
| 中央设备控制链 | libdevice、srv_device：原轮固定 Use/Registrar、完整 Watch、受限静止报告、不可逆暴露、单调 Provision Stop、供应通知、Released 与入口尾部；实际资源和供应责任不能因 RPC 失败或别名关闭丢失 |
| 驱动及管理 | libdriver、srv_driver、PCI/xHCI/HID、libusb：Preparing 的唯一提交资格、只读结果、IO 准入与执行身份、Source 拒绝、主动运输退场、Stop/Get/登记/Master 交错、Managed 单调阶段/期限、多绑定与旧入口隔离 |
| RPC 订阅 | 初始/Ack 完整快照、序号确认、后续调度与实际注销；同时核 init 的 FS/service 消费者，不只检查设备路径。Dispatcher 完成合同与同步 Caller 私有回复口清理区分 |
| root 监督与装配 | srv_init 的单一终局 owner、Start 前观察、实际业务等待与注册探针、SourceClosed/DONE 两种顺序、原 Collector；旧线程、握手和重复能力账本的删除 |
| 分步前置 | shared/elf、libprocess：同一验证器/ISA 解析、Preparation/ImageCopy 的工作界、首错和取消、GrantOutcome、失败 Unmap owner、同步与 root 消费者；不是第二套 loader |
| DMA 构造失败 | os/kernel/resource/dma：仅未发布零成员根的局部退款，不扩大为已暴露映射可析构或改变稳定维护责任 |
| 工具与说明 | Justfile、RPC/设备/平台资金 GDB 工具、check-device、impls/plans：判据未因机制替换放宽、原失败保留、版本证据准确；历史导航不恢复已删除机制 |

保留 Seat、Provision、Activation、Process 的不同完成条件；静止证明、Closed、Released、CPU DONE 和最后引用退款不能合并。无兼容层要求；不为减少类型/文件数删除必要 owner，也不引入通用恢复账本、驱动运行体或多方事务。

## 已有验证与边界

最终源码版本由 `artifacts/lifecycle/source-version.json` 记录，提交整理核对零漂移。可复用的最近证据为：

- `just check`、十分面 `just clippy`、137项相关 host 测试和 user 格式检查通过。新增 Lifecycle host 用例核退休后的执行身份/IO 权及准备拒绝后禁止复活。
- `THROTTLE=100` 的 core、device-reuse、device-candidates、unknown QueryUse/GetResource/Master 六条 QEMU 路线均退出0；具体入口、退款/故障覆盖和进程清理见[最终组合验证](archived/review-2026-10-27-device-lifecycle.md#最终组合验证与交付边界)。
- 第二轮较早的单 root 最早 Start/业务等待/装载/停止故障、reset 拒绝、IRQ idle、平台及未发布根退款证据见[原交付档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)。保留它们各自的版本限制，不把它们描述为最终提交逐项重跑。
- 终态结果查询、Released 早于报告回复、停止先于迟到安装成功、异步 Source 准入拒绝的精确窗口未逐项动态注入；当前依据为源码责任链、生命周期 host 和实际消费者组合，不外推全部交错。

本机 `artifacts/` 不提交，正文保留命令、结果及证据边界；缺少本机原始证据时明确说明，不将缺文件改写成已通过或产品错误。DMA＋IPI 组合及独立时钟异常继续由[唯一收口表](todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)拥有，不因本次代码交付或 Review 自动关闭，也不另复制调查记录。

## 接手与报告

按 [REVIEW](REVIEW.md) 的固定对象与只读审查纪律执行。新的 findings 须给出目标提交中的具体位置、可达路径和违反的合同；先区分代码缺陷、机制改进候选与已登记的证据缺口。必要复核仅运行针对具体风险的现有检查，不机械重跑所有设备路线。

审查开始后以 `review-2026-10-27-device-convergence.md` 统一承载发现、修复和复核，并将本登记转为报告入口；目前没有该报告，也没有宣告独立 Review 完成。
