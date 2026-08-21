# SBI 边界与地址纪律

- 基线：`c449f00186a8533c4ca8fb04c93dc8e735602b3a`
- 状态：首审与独立核验完成；DBCN/平台基线已收口，启动与 CSR 问题纳入执行环境重构
- 外部规范：`references/normative/riscv-sbi-v3.0/`
- 项目设计：`notes/internals.md`、`notes/execution-context.md`
- 重构导航：`plans/reviews/system-audit/context-redesign.md`

## 代码表面

- 统一封装：`os/kernel/src/sbi.rs`
- 启动与 HSM：`os/kernel/src/main.rs`、`rt.rs`、`external.rs`、`assembly.asm`
- TIME/IPI/SRST 调用者：`os/kernel/src/sched.rs`、`trap.rs`
- DBCN/legacy 调用者：`os/kernel/src/console.rs`、`syscall.rs`
- 地址转换：`os/kernel/src/mm.rs`

## 契约表

| ID | 契约 | 来源 | 实现点 | 结论 |
|---|---|---|---|---|
| ABI-1 | `a7=EID`、`a6=FID`、`a0..a5` 参数、`a0/a1` 返回 | `binary-encoding.adoc`「Binary Encoding」 | `sbi_call` | 当前调用正确 |
| ABI-2 | 标准错误码为 0、-1..-14；错误时 `a1` 通常未定义 | 同上「Standard SBI Errors」 | `SbiError`、`sbi_call` | 映射正确，错误时未消费 `a1` |
| BASE-1 | version 为 7 位 major + 24 位 minor；probe 任意非零表示可用 | `ext-base.adoc` 对应函数小节 | `init` | 解码与 probe 判断正确；项目基线为 SBI 2.0+ |
| TIME-1 | `stime_value` 是绝对 `uint64_t`；最大值可卸载 | `ext-time.adoc`「Set Timer」 | `set_timer`、`DISARM` | RV64 参数与卸载值正确 |
| IPI-1 | hart mask 是标量，另传 base | `binary-encoding.adoc`「Hart list parameter」、`ext-ipi.adoc` | `send_ipi` | 在 hartid 0..7 前提下正确 |
| HSM-1 | start address 是 PA；opaque 原样进入 `a1` | `ext-hsm.adoc`「Hart start」 | `hart_start`、`awaken_pa`、`_awaken` | 地址域与参数正确 |
| HSM-2 | secondary 只获得规范列出的确定状态 | 同上「Hart Start Register State」 | `HART_SETUP`、`_resume_user` | 违反，见 SBI-002 |
| HSM-3 | hart start 异步 | 同上「Hart start」 | `wake_secondary_harts` | 启动集合发布不闭合，见 SBI-003 |
| DBCN-1 | write 参数为 length、PA low、PA high | `ext-debug-console.adoc`「Console Write」 | `debug_console_write_bytes` | 当前调用者均为直映射内核内存，参数正确 |
| DBCN-2 | 非阻塞 write 允许 partial/no writes | 同上 | `debug_console_write_best_effort` | 单次尽力写，符合项目策略 |
| SRST-1 | shutdown type=0、reason=0；成功不返回 | `ext-sys-reset.adoc`「System reset」 | `system_reset`、`shutdown` | 参数与返回处理正确 |
| LEGACY-1 | putchar 为 EID 1、字符在 `a0`，无 `a1` 返回 | `ext-legacy.adoc`「Console Putchar」 | `legacy_console_putchar` | 调用约定正确，仅可作为 best-effort |

## Findings

## SBI-001：DBCN 合法零进度被判为致命错误

- 分类：缺陷
- 严重度：P1
- 置信度：已确认
- 状态：已收口

### 契约

`references/normative/riscv-sbi-v3.0/src/ext-debug-console.adoc`「Function: Console Write (FID #0)」规定调用非阻塞；控制台暂时无法接收时允许 partial/no writes，成功返回值是实际写入数。

### 实现

基线中的 `debug_console_write_all` 把 `written == 0` 视作无效长度并进入 `fatal()`。常规日志路径此时仍持有 `CONSOLE` 锁。

### 影响

合规 SBI 返回 `SBI_SUCCESS + 0` 即可永久停放当前 hart；若路径持有控制台锁，其它 hart 的日志路径还会永久自旋。启动日志和用户 Debug syscall 均可达。

### 处理方向

已选择单次 best-effort：`debug_console_write_best_effort` 只调用一次 DBCN，部分写、零写和错误均丢弃；日志失败不再进入功能路径。

### 核验

两次独立 review 均确认原缺陷；修复 diff 已通过独立 spec review。

## SBI-002：HSM secondary 未建立确定的 CSR 基线

- 分类：缺陷
- 严重度：P0
- 置信度：已确认
- 状态：已核验，纳入执行环境重构

### 契约

`references/normative/riscv-sbi-v3.0/src/ext-hsm.adoc`「Hart Start Register State」只保证 `satp=0`、`sstatus.SIE=0`、`a0=hartid`、`a1=opaque`；调用方不能依赖其它状态。`references/normative/riscv-isa-v20250508/src/supervisor.adoc` 的 `sstatus`/SRET 规则规定 SPP 决定返回特权级。

### 实现

`HART_SETUP` 只清 SIE、设置 FS；没有确定 SPP/SPIE。`sched::run` 以 `csrs sie` 在未知初值上追加 SSIE/STIE。`_resume_user` 未确定 SPP 就执行 `sret`。

### 影响

若 SPP 为 1，首次 `sret` 会返回 S 态而非 U 态；遗留的 `sie` 位还可能使未接入的中断进入只处理 SSIP/STIP 的 trap 路径。当前 OpenSBI 恰好给出安全初值不能构成契约。

### 处理方向

在 secondary 入口建立完整 CSR 基线：精确设置 `sie`，并在用户返回边界显式建立 SPP/SPIE 等 sret 前置状态。字段级操作优先于覆盖整个 `sstatus`。

### 核验

独立 reviewer 复核 HSM 与特权规范后确认；SPIE 单独为 0 并不屏蔽 U 态 S 级中断，finding 只保留已证实的 SPP 与额外 `sie` 风险。

## SBI-003：异步启动前未完整发布预期 hart 集合

- 分类：缺陷
- 严重度：P1
- 置信度：高
- 状态：已核验，纳入执行环境重构

### 契约

`ext-hsm.adoc`「Hart start」明确 `sbi_hart_start` 可在目标 hart 真正开始执行前返回，因此调用者与已启动 secondary 可以并发。

### 实现

`wake_secondary_harts` 在遍历 CPU 时逐项执行 `expect_hart`，随后立即 `hart_start`。secondary 可直接进入调度器，而静默判定只比较当时的 `EXPECTED_MASK`。

### 影响

在 boot hart 不是首个可运行 DT CPU 的平台上，较早启动的 secondary 可能在 boot 尚未进入 expected 集合时耗尽 ready/timer，形成前缀集合上的“全员 idle”并调用 shutdown。当前服务负载降低触发概率，但没有提供时序保证。

### 处理方向

HSM 启动改为两阶段：先验证全部 CPU 并一次性发布完整 expected 集合，再启动任何 secondary；或以独立 startup-complete 状态门控静默判断。

### 核验

独立 reviewer 考虑 initfs 已先入队后仍确认该竞态为条件可达；触发依赖 CPU 顺序和执行时序。

## SBI-004：SBI 能力基线与控制台降级策略未成文

- 分类：设计缺口
- 严重度：P2
- 置信度：高
- 状态：已收口

### 契约

DBCN 的函数表标明其自 SBI 2.0 出现；BASE 允许通过 probe 查询扩展。legacy 扩展已弃用，不能作为现代 SBI 的必有能力。

### 实现

基线中的 `init` 只拒绝早于 0.2 的版本，却强制要求 DBCN；`console.rs` 又描述“DBCN 优先、legacy 回退”。实际上 DBCN 缺失会在初始化阶段停放，正常运行期不存在能力回退。

### 影响

代码无法回答项目最低 SBI 版本、DBCN 是否平台硬要求、无控制台是否允许运行。后续改动容易分别依据不同假设。

### 处理方向

已选择并写入 `notes/internals.md`：SBI 2.0+；TIME/IPI/HSM/DBCN 必需，SRST 可选。DBCN 仅承载 best-effort 观测输出；legacy putchar 只用于 DBCN 尚未就绪时的早期诊断。

## SBI-005：boot 到 secondary 的共享状态发布尚无内存模型证明

- 分类：未证明不变量
- 严重度：P2
- 置信度：高
- 状态：已核验，纳入执行环境重构

### 契约

HSM 只定义启动状态和异步性，没有把 `hart_start` 定义成共享内存屏障。

### 实现

secondary 会读取 boot 写入的跳板页表、`KERNEL_SATP` 和 DBCN 支持位；其中存在 Release store 配普通 `ld` 或 Relaxed load 的组合，没有成文的 publish/consume 协议。

### 影响

代码依赖“先写再 hart_start 即可见”，但 SBI 契约不足以证明该结论。具体正确性需结合 RISC-V 内存模型、cache coherence 和入口 fence 审查。

### 处理方向

在启动/SMP 分片建立统一发布协议，而不是逐个原子补丁；覆盖页表、正式 satp、HartLocal/全局初始化和 online acknowledgement。

## 正确实现与边界

- EID/FID、`a0/a1` 返回、标准错误码映射正确。
- RV64 TIME 参数与 `u64::MAX` 卸载值正确。
- IPI 使用标量 mask、base=0；当前 hartid 上限 8 可被一个 RV64 mask 覆盖。
- HSM `start_addr` 使用 `_awaken` 的 PA；高半区 `secondary_entry` 作为 opaque 合法。
- DBCN 参数顺序及 PA low/high 正确；当前调用者没有把用户 VA 直接交给 SBI。
- SRST shutdown/reboot 常量和失败后停放语义符合规范。
- legacy putchar 调用约定正确，但只能作为早期 best-effort 诊断。

## 跨片登记

- 启动/MMU：boot hartid 在进入 Rust 前直接索引固定数组和栈，需结合支持平台的 hartid 契约审查；当前不能把 SBI 当作稠密 hartid 保证。
- 启动/SMP：SBI-005 的共享状态发布协议。
- trap/CSR：SBI-002 的修复应与完整 sret/trap CSR 不变量一起收口。
- syscall/调度：用户 sleep 的 `ms * ticks_per_ms + now` 存在溢出路径。
- 平台：直接读取 `time` CSR 及其与 DT `timebase-frequency`、SBI TIME 的关系不属于 SBI 契约，需从特权规范和平台契约证明。

## 已决策

- DBCN：单次非阻塞 best-effort，任何输出失败不影响其它模块。
- SBI 基线：2.0+；TIME/IPI/HSM/DBCN 必需，SRST 可选。

SBI-002、SBI-003、SBI-005 不在旧启动路径逐项补丁；统一由 `notes/execution-context.md` 的 BootstrapContext、HartBootRecord、formal entry 与 Release/Acquire RuntimeGate 收口。当前代码在整体重构完成前仍不满足这些契约。
