# 设备整体审查修复交付的独立 Review

状态：**已登记，尚未执行。** 本次审查固定设备整体 Review 修复交付，用新上下文复核最终责任链；不重复首批设计与第二轮整体审查，也不把交付前检查算作独立 Review 已通过。

## 固定对象

- 分支：`refactor/system-redesign`。
- 基线：`d19fdfe4d9f655d20cc8f46d8d0cb570b879f05f`。
- 目标：`e23afdbfe0924058ce498a9b8994d70b10f451e3`（`fix(device): 闭合停止与退休责任并分步启动驱动`）。
- 范围：该提交的产品实现、测试、设计依据与说明文档及其实际依赖。后续纯登记提交不扩展产品审查对象；执行时如 HEAD 已变化，仍以固定对象取证。

[原整体报告](archived/review-2026-10-28-device-driver-overall.md)拥有 G1–G5、C1、D1 原发现、修复依据、交付前检查和验证事实；[设备主计划](todo-2026-09-device-system-design.md#整体审查修复后的接续2026-10-28)拥有当前条件覆盖及候选出生观察。[服务装配设计](todo-2026-10-07-service-assembly.md#运行期驱动出生的分步接线2026-10-28)拥有 C1 输入与出生合同。归档中的“尚未提交”保留当时事实；本次固定目标已本地提交，未 push。

## 审查范围与重点

1. **停止取代原操作**：Work 的 Frozen 候选与 Registry 就绪提交顺序，来源封口/维护扫描交错；Manager 停止先于旧发现调用消费，取消/迟到结果与 Scope Watch 退场；合法 Closed 与运输/帧错误不混淆。
2. **原轮终态与付款**：认证 Control/Use 的 Shared Released 快照和原 Funding 创建 Watch；活动与终态出生分别保有正确引用，不保活已退休供应，不以裸 BindingRef 铸造终态；Ack、别名退出和费用收束。
3. **实际硬件与页退休**：原命令/TD 决定 StopEndpoint/SetDequeue/Disable，枚举间隙不从描述符推断已配置；DMA DONE→CPU Unmap→Closed，错误保留映射 owner，待领取结果先转独立回执，构造期失败交进程终局。
4. **公共输入与分步装载**：ELF Validation 的输入所有权、范围游标、移动安全和首错；ImageView 独立映射寿命；借用/拥有输入共用 Preparation，OwnedGrant 在实际 ProcessGrant 成功点移交；完成或取消后输入关闭，失败保留 owner。同步调用者、root 与 srv_driver 的实际接线都须核对。
5. **原 ProcessTask 出生与监督**：Preparing/Prepared/Supervised/Empty 的单一关闭权；取消 Collector、PID/Control 记录、原 Pool 和 Run 关联；DONE 实际附着后 Start，来源注销先于最终收集。bootstrap 入口提交立即唤醒关联，清理来源不承担补发就绪。
6. **期限与消费者**：Claim→功能安装一次绝对 40s，worker 出生仍 10s，单次 RPC/停止/硬件期限不变；正常 Stop 不产生错误候选；Released 后首次 Watch 的真实 function-session 消费者与新增 host 回归。

按 REVIEW 检查最终结构、权限/付款、失败/取消/超时/释放、可达错误、停驻唤醒及旧路径清除。主旨是闭合上述责任链并收敛重复机制，不为覆盖不足添加猜测性防御、第二个 loader 或供应状态镜像。无本轮证据时不重开已关闭的 DMA＋IPI、全 hart idle、真实 IRQ 门；IOMMU、非一致 DMA、复合 USB、通用 HID、安装仓库及持久化关机不属于本次交付。

## 已有证据及限制

- `just check`、十分面 Clippy、203 项 host 测试及 2 项 doctest 通过。
- QEMU reuse、candidates、function-session、Driver loading 故障路线通过；最终 function-session 包含 core 聚合与两轮 Released→Watch→Ack。
- 逐路线源码版本、原始失败及最终交付指纹见原整体报告。提交整理核 18 个产品文件与交付指纹一致；本次不因提交重跑已通过验证。
- G1/G2 精确交错、G4 两个指定枚举中断窗口、G5 Unmap 分配失败、C1 每个取消窗口未逐窗动态注入。当前使用源码/owner 论证、纯逻辑测试和真实组合分层取证，覆盖缺口由设备主计划按触发条件维护。
- 一次候选路线 HID 出生 Unavailable 缺少原首因，收集与退款完成；补诊断后同一路线通过，根因仍未定。只读核诊断和收束责任可纳入本次审查，不默认触发重复压力调查，不把后一次通过称为修复。

## 执行与后续入口

执行时先核固定提交、工作树及上述依据，再用新上下文审查实际消费者和共同变化的契约。发现必须提供可达路径、具体位置及违反的合同；结合已有证据决定必要验证，不机械重跑全部路线。形成报告后将 findings 的修复与复核统一转到该报告，本登记链接报告并归档；无 findings 也须保留检查范围与证据限制。

本登记尚无审查结果，下一步是执行固定交付独立 Review。条件覆盖和未归因观察保持原唯一 owner，不复制到本登记成为另一套任务清单。
