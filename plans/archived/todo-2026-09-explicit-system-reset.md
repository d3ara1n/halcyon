# 显式系统复位

> 【已归档】capability 授权的显式系统复位已替代调度器从 idle 推断关机。方向契约见 `../../notes/ideas/system-reset.md`，实现现状见 `../../notes/impls/internals.md` 与 `../../notes/impls/startup.md`。

## 目标

删除 `idle -> is_quiescent -> SBI shutdown` 正常路径。idle 只维护调度唤醒所需状态并进入 WFI；系统 shutdown/reboot 只能由持有 `SystemReset` authority 的用户态进程显式提交。

本轮由 init 在现有验收和服务收束完成后直接提交 `Shutdown + Requested`。不新增 power 服务；未来服务另见 [`todo-2026-09-power-management-service.md`](../todo-2026-09-power-management-service.md)，该任务不承载本轮设计结论。

## ABI 与 authority

shared 定义独立于 SBI 的固定宽 ABI：

- `ResetAction::{Shutdown, Reboot}`；
- `ResetReason::{Requested, SystemFailure}`；
- `SystemReset(handle, action, reason)`，成功不返回。

内核显式映射到平台后端；用户态不观察 SBI 数值、cold/warm 分类或 vendor 扩展。非法编码拒绝，不向固件透传。

bootstrap 铸造 `SystemReset` 叶子对象并作为 primordial capability 交付 init。调用要求对象类型、role 与 `MANAGE`；初始 entry 可持 `MANAGE | DUPLICATE | TRANSIT | GRANT`，便于用户态按政策裁剪和转授。没有 PID、进程名或 Job 身份特例。

对象以无等待的原子 in-flight 门串行化平台提交：赢家调用后端，竞争者立即得到 `ObjectBusy`；失败释放提交权，成功按契约不返回。关闭和 transit 丢弃都是固定上界叶子操作。

## 实施批次

1. [x] **shared 与内核对象**：加入 reset ABI、调用号、对象 kind/role、bootstrap capability 与 rinlib 安全封装；补齐固定宽解析和 rights 负路径。
2. [x] **平台映射与调度**：把 eRhino action/reason 显式映射到 SBI SRST；错误返回用户态；删除 `sbi::shutdown`、`is_quiescent` 和 idle 内停机分支，保留 idle mask 的唤醒路由职责。
3. [x] **init 与验收**：init 在全部既有终态事实成立后提交 shutdown；virt 以“内核已接受请求 + QEMU 退出”证明成功，sifive_u 在平台返回失败时记录结果并由 wrapper 收割；删除 quiescent 锚点。
4. [x] **文档收口**：同步 impls、KNOWN_ISSUES、AGENTS、COMPASS，并归档提前 quiescent 调查。

## 错误语义

- ABI 编码非法：`IllegalArgument`；
- Handle role/kind 不符：`WrongObjectType`；
- 缺少 `MANAGE`：`RightsDenied`；
- 已有请求进入后端：`ObjectBusy`；
- 平台不支持：`NotSupported`；
- 后端失败、已校验参数仍被拒绝或成功后异常返回：`InternalError`。

任何失败都返回调用者，不进入 `hart::park()`。内核不以全局 idle、Job 状态或资源收束作为调用前置条件。

## 完成标准

- 无 authority、错误 role、缺少 rights 和非法 ABI 均稳定拒绝；
- 标准 shutdown 在 virt 上不返回且使 QEMU 退出；
- 不支持 shutdown 的平台得到明确返回，内核不永久停放调用 hart；
- idle 路径不包含任何系统生命周期推断；
- 原有业务与资源锚点全部保留，新的 reset 提交锚点只能出现在它们之后；
- debug/release、hetero/nofd、sifive_u 与 host 验证全绿；
- 提前 quiescent 停机路径由结构删除。

## 验证结果

- `just check`、用户态完整构建、shared 与 OS/user host 测试全过；
- `virt` debug/release、hetero、nofd 均在完整业务锚点后由 `Shutdown + Requested` 使 QEMU 退出；
- `sifive_u` 明确返回 `NotSupported`，init 保持 root supervisor，wrapper 按失败终态锚点收割；
- capability 负路径覆盖错误对象与缺少 `MANAGE`，ABI host 测试覆盖未知值和高位非零。
