# 设备设计基线独立审查与处置

状态：**独立审查已完成，四项发现已由主代理修订并针对原反例复核关闭。** 本报告审查固定设计基线及其后续减法修订；不表示设备代码已实现或运行验收通过。最终施工范围及设计准入由[设备主计划](../todo-2026-09-device-system-design.md#当前进度与接力)拥有。

## 固定范围与证据

- 固定提交：`9ef1858e96c3eef2bed9eb4c57384223ed279cbd`，对比父提交 `efd29c3`；后续合同修订核对至 `e6d11a1`。
- 独立 reviewer 使用新上下文，只读检查设备主方案、服务装配、系统终局、内存 DMA 接点、平台保留内存边界、中断承接、导航和固定外部证据记录；生产代码在该提交区间没有变化。
- reviewer 核对 VM 发布/退休、BackingUse、MemoryObject 写许可、等待源与维护执行、PLIC/idle 目标接点、boot Shell/runtime_stop 事实，未修改文件。
- 审查结束前主代理已开始并行设计修订。独立报告只将这些草案视为针对性处置，不声称完整复审最终工作树；本篇下方的修复复核由主代理承担，没有再做一轮独立首审。
- 本地 SBI、QEMU virt 资料和仓库固定记录已核读；PLIC/xHCI/USB/HID 的全部远端资料没有在这次独立审查中重新获取。硬件追加核实由[平台取证](../ref-2026-10-23-device-platform-contracts.md)拥有。
- 两个提交区间的 `git diff --check` 通过。未运行状态模型、编译、测试或 QEMU。

## D01 · P1 · MMIO 在途发布与 Revoke 顺序未闭合

**原始发现。** 固定基线设备计划第 211、245 行（e6d11a1 第 210、244 行）把资源 gate 与 VM gate 分开，但未规定尚未发布 PTE 的 Map 何时进入资源账本。反例为 Map 通过资源准入后准备 VM，Revoke 封口并撤完已登记成员后报告完成，原 Map 随后发布。现有 `vm/operation.rs` 的结果发布后必成提交及 `vm/change.rs` 分步发布不能自动补足这个接缝。基线及 e6d11a1 均存在该缺口。

**处置。** [VM 来源、发布与强制撤销](../todo-2026-09-device-system-design.md#vm-来源发布与强制撤销)规定准备责任先登记、资源门内取得 PublishGranted、VM 锁外接续原事务。Revoke 完成必须无仍可发布事务、无存续片段且相关翻译失效；部分 Unmap/空洞重映射以 MappingId 区分。强制撤销承载在 VM/Map 出生时预付，整空间退出明确接管。

**复核。** Revoke 先于许可则原 Map 回滚；许可先行则 Revoke 账本中已有该记录，只能等待其回滚或发布后撤销。发布中退出不丢 CommittedChange，弱 VM 定位失效不代替退休回执。原漏记窗口被关闭。状态：**设计修复并复核关闭**；真实跨 hart/Quota/退出验证属于未来实现门。

## D02 · P2 · 空闲取消后的合法 Provision 跳轮未定义

**原始发现。** 基线第 102、114、118、122、127 行：`StopClaim(1)` 先到，关闭空闲轮而不创建 Binding；随后 `Claim(2)` 首次向 provider 发 `Prepare(2)`。基线同时要求不跳轮，却未确定 provider 如何认识从未收到的第 1 轮及发行权限。基线和 e6d11a1 均有缺口。

**处置。** [Provision 合同](../todo-2026-09-device-system-design.md#provision-与冻结资源表)共用原 claim_epoch；本实例的 srv_device context 是唯一发行者。无 active 时允许 e>closed_epoch 的合法前向跳过；有 active 时只接受相同轮，其他新轮 Busy。空闲 StopClaim 仍不制造无意义的远端工作。

**复核。** 上述 Prepare(2) 可接纳并封住未曾准备的旧轮；迟到 Prepare(1) 返回 Closed。active(2) 未完成时 Prepare/Stop(3) 不能越轮；公开未来 Claim 仍由 srv_device 拒绝，普通驱动不能自授 Provision 发行权。状态：**设计修复并复核关闭**。

## D03 · P2 · 超时换轮重试越过匹配 owner

**原始发现。** e6d11a1 第 114 行新增“srv_device 超时 Stop 后以新 epoch 重试”，与第 93、108、146 行的旧轮 Released、srv_driver 选择权及禁止隐式无限重试冲突；固定基线没有此句。provider 已准备但回复丢失时，srv_device 不能自行创造新申请主体/候选或跳过本地 Claim。

**处置。** 超时只提交原 Binding.Stop；确认 Closed、收束并 Released 后，由匹配者按原候选政策决定下一次 Claim。保留删除 Provision.Query 的减法，不恢复轮询接口。

**复核。** 旧回复丢失后仅原 epoch 进入停止；来源封口/管理员 Stop 后没有绕开准入的新 Claim；新轮只能由持权主体正常提交。状态：**设计修复并复核关闭**。

## D04 · P2 · Provision backing 持权禁令与资源组合职责冲突

**原始发现。** e6d11a1 第 89 行新增“backing 不可 Claim 或持其他能力”；这把调用角色不附带权限误写成实现 owner 不能持权。PCI provider 必须持窗口、DMA/IRQ/MMIO 控制以准备和确认 Closed，USB provider 必须持 session/transport；按禁令实施无法完成同案分配的职责。固定基线没有该禁令。

**处置。** [角色表](../todo-2026-09-device-system-design.md#claim轮次与角色)只规定 Provision 接口不附带 Claim、其他 BindingControl 或来源管理权；backing 允许并要求持有实际供应、资源控制及清理所需能力。设备占用仍由 srv_device 提交。

**复核。** PCI/USB backing 能按原授权组合资源；取得 Provision 调用接口不因此取得另一设备的 Claim 权，角色权限与 owner 持有分开。状态：**设计修复并复核关闭**。

## 覆盖结论与边界

原审查未发现其他可证实阻塞项：占用、选择、总线效果和 OAL 投影有独立 owner；DMA 安全不由进程 DONE/VM fence/timeout 替代；PLIC complete-before-disable 与 idle 安全点已有明确目标。未实施和延期能力本身不作为缺陷。

四项原 finding 的设计责任已关闭，记录归档后保持原发现及处置事实。主代理后续补齐的启动交付、功能取得权限、资源树、Pin 暴露边界、装配与验证门属于本轮设计增量，由主计划的交付前结构检查负责；未来提交后按 REVIEW 固定本轮增量登记独立审查，不能把本报告冒充对所有后续变更的独立通过结论。
