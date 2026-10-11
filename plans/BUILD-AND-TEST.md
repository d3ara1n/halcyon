# 构建与验证

运行构建、测试、QEMU 或调整验收基础设施前读取本篇。任务是否完成由 [AGENTS「任务适用范围与完成条件」](../AGENTS.md#任务适用范围与完成条件)判断；同一路线在施工期间可作开发检查，路线通过本身不宣告机制完成。具体命令和默认参数以 [Justfile](../Justfile) 为执行真值。

## 工具链与构建入口

- 开发机为 macOS，系统工具为 Homebrew 的 `just`、`dtc`、`qemu`、`riscv64-elf-binutils`、`riscv64-elf-gdb`；系统级安装由用户执行。
- Rust edition 2024，工具链由根目录 `rust-toolchain` 选择；当前内容是浮动 `nightly`，并未固定日期。取证或比较代码生成时记录实际 rustc 版本。
- 内核、共享库、用户态分别属于 `os/`、`shared/`、`user/` 三个 workspace，以 path 依赖连接。内核 target 为 `os/riscv64imac-unknown-erhino.json`，用户态默认 target 为 `user/platforms/riscv64imac-unknown-erhino-elf.json`，均使用 build-std；浮点验收 `test_fp` 使用独立 gc target。
- **统一走 `just`，不裸跑 `cargo build`**。内核链接器 `riscv64-elf-ld` 和链接脚本由构建入口配置，用户态由自定义 target 与 build-std 提供运行环境。
- 内核 package、默认 binary target 和产物名统一为 `kernel`；本文命令与构建入口以此为准。
- initfs 使用 tar 时避免 bsdtar 的 `._` AppleDouble 文件混入归档。

```sh
just build_kernel
just check
(cd shared && cargo check)
just clippy
```

`just check` 对 os workspace 执行带 build-std 的 cargo check，包含它的实际依赖，不等于检查所有 workspace。`just clippy` 覆盖 shared/os/user host、kernel/user RISC-V、stress/memory feature 与独立 gc target 八个分面，全部使用 `-D warnings`；完整日志写入 `artifacts/lint/`。

## Host 测试

纯逻辑 crate 的 host 测试必须显式指定 host target，不能继承 RISC-V 默认 target 去链接 std。当前开发机命令：

```sh
(cd os && cargo test --workspace --exclude kernel --target aarch64-apple-darwin)
(cd shared && cargo test --workspace --target aarch64-apple-darwin)
(cd user && cargo test -p libkernel -p libthread -p libruntime -p libbudget -p libexecution -p librpc -p librunnel -p liboal -p libservice -p libprocess -p libfs --target aarch64-apple-darwin)
```

user 的 host 命令只选择公共库，不能把 RV64 服务二进制纳入 host `--workspace` 测试。根据改动先选择受影响的 package 或现有 testcase。新增测试说明其独立契约和目标错误；无关测试或新增 harness 不作为默认交付物。clippy 编译测试目标不等于执行 host 测试，QEMU 聚合同样不替代它们。

## QEMU 路线与选择

workload 由用户态 `srv_init` 编译期控制，内核不感知测试政策。服务、驱动、验收进程分别位于 `user/services/`、`user/drivers/`、`user/tests/`；验收配置中的进程也可承载正式机制，成熟度由对应 impls 与任务记录分别说明。

| 命令 | 覆盖与用途 |
|---|---|
| `just virt` | 日常 debug core：确定性内存、IPC、Tunnel、Job、监督与 reset |
| `just virt-stress` | core 加 control/Tunnel 重复压力、非空 WaitSet 进程自动退休与完整 16/16 竞态矩阵 |
| `just virt-release` | release core，覆盖优化代码生成和 trap 寄存器保持 |
| `just sifive_u [mode]` | 128MiB 与非零 boot hart 等板级差异，完整 core；默认 release，可指定 debug |
| `just virt-nofd [mode]` | 无 F/D、无兼容浮点域的完整 core；默认 release，可指定 debug |
| `just virt-hetero [mode]` | 异构调度域及完整 core；默认 release，可指定 debug，涉及调度域时额外运行 |
| `just virt-boot-failure` | 外部 GDB 对正式 debug binary 注入 Ready 前 panic/alloc/fatal，确认所有 hart 在 Failed 后停驻 |
| `just virt-ipi-failure` | 外部GDB跳过真实已发布责任的一次SBI IPI并返回FAILED；核无timer WFI目标在后续真实广播唤醒后先停驻，确认稳定停止事实 |
| `just virt-vm-fault` | debug memory：复用 stress 中的线程/映射消费者，旁观真实 Map 发布窗口 fault→RetryFault；detach 后核目标 Dead/Completed、Job 收束、辅助能力关闭与显式 reset |
| `just virt-vm-repair` | 外部GDB保留真实PTE与用户现场，分别注入R/W/X入口故障及重复故障；核实际SFENCE/FENCE.I、原寄存器与pc重试、无进展终止界 |
| `just acceptance-core` | 常规跨机制回归：clippy、无调试器 debug stress、release core |
| `just acceptance-platforms` | 平台组合：release sifive_u core、release nofd core |
| `just acceptance-faults` | 特权失败与发布窗口：boot-failure、IPI failure、VM fault、VM repair |
| `just acceptance` | 完整收尾：依次执行上述三组，各路线仅一次 |

日常局部开发优先受影响 host 测试及 `virt-release`；需要 debug 现场用 `virt`，涉及竞争与退休用 `virt-stress`。常规跨机制回归用 `acceptance-core`；平台、DT、hart 启动/能力、资源布局变化补 `acceptance-platforms`，调度域变化再加 hetero；启动失败、IPI/停驻、VM 发布/修复、uaccess 变化补相关故障路线，跨这些机制的改动用 `acceptance-faults`。广泛系统变更及完整收尾保留 `acceptance`，不把所有组强加给每次局部修改。

施工中路线结果只作为开发证据。机制闭包收尾按实际影响执行 `just check`、`just clippy` 与必要路线；已由聚合覆盖的检查不另行重复。平台完整业务保留，不用“板级差异”理由删去可能暴露资源/调度交错的真实消费者。文档等不改变执行行为的工作按影响核验，不运行无关的代码验收。

## 节流、超时与判定

常规路线经 `tools/qemu-throttle.sh` 和 `tools/qemu-acceptance.sh`。默认 `THROTTLE=50`，stress 和自动 GDB 路线同样节流，不作为全速性能基准；需要全速的专项诊断显式设 `THROTTLE=100`。节流限制宿主执行占空比，不能由 50% 推导整条路线耗时恰好翻倍；全速也仍经过路线的日志、锚点与超时判定。

各路线有独立超时，可用 `VIRT_TIMEOUT`、`VIRT_STRESS_TIMEOUT`、`VIRT_RELEASE_TIMEOUT`、`VIRT_HETERO_TIMEOUT`、`VIRT_NOFD_TIMEOUT`、`SIFIVE_U_TIMEOUT`、`VIRT_BOOT_FAILURE_TIMEOUT`、`VIRT_IPI_FAILURE_TIMEOUT`、`VIRT_VM_FAULT_TIMEOUT` 覆盖。默认值只在 Justfile 维护；virt stress 为竞态矩阵和文件资源/退出组合预留较长预算。VM fault 的 memory 负载有独立预算，旁观与 detach 后收束共用总期限；聚焦负载后仍保留原预算，不把提速当作收紧防挂死期限的依据。平台路线预算同时允许显式 debug，异构域的 Base64 系统负载固定在单 hart。超时从 QEMU 运行阶段计，不含冷编译；聚合命令不另设跨路线总时限。

路线超时是可扩大的运行预算，采用**撞限翻倍**政策：实际触及路线期限时，将该路线在 Justfile 中的默认值翻倍，并重跑核对完整业务收束与 reset；不为保留旧期限删减正式场景。默认数值只由 Justfile 拥有，手册和验收脚本不另设副本。超时从 QEMU 运行阶段计，不是性能目标；panic或明确错误触发的主动收割不算撞限，不能仅凭退出码误调期限。挂起或失败的具体原因仍按调查手册定位，翻倍不替代业务锚点与正确性验证。

判定依据 workload 身份、该 workload 的完整收束和显式 reset 锚点，不能用矩阵中途超时宣称内核失败。`virt-boot-failure` 同样节流，由 `tools/check-boot-failure.py` 独立判定预期故障、限制 GDB 时间并回收进程组，完整日志在 `artifacts/boot-failure/`。

GDB专项路线在各自artifacts目录下使用时间戳与进程号创建独立日志，保留ELF hash、注入位置和原始观察。共同工具[qemu_gdb.py](../tools/qemu_gdb.py)按ELF PT_LOAD换算物理地址，并用QEMU的物理内存远程包读取跨hart全局事实，读取后恢复原模式；不依赖所选hart当前satp。VM repair中的异常原因/地址由诊断器注入，证明修复分支及现场合同，不宣称复现真实硬件TLB缓存陈旧；VM fault则要求正式消费者产生真实发布窗口故障。GDB停顿不作为计时、WCET或调度公平性证据。

M累计栈工具从固定ELF重建CFI与动态目标，源码语义前提须人工核实后由[固定证书](../tools/maintenance-stack-certificate.json)绑定；没有证书、未知site/CFI/业务环或源码变化均不签发上界。本轮推导、命令和拒签负向证据由[对象链修复档案](archived/review-2026-10-07-object-chain.md#新elf累计栈与拒签证据)拥有；证书不是对未来构建的自动许可。

验收脚本只要求当前实际消费者与仍在调用的自检锚点；已删除机制的自检、日志与容量路线同步删除，不保留兼容锚点或为使旧门通过重建机制。路线通过证明该 workload 的当前可观察行为；内部交错、失败退款和付款保证的验证缺口由所属工作记录承载，不以旧锚点缺失代替风险判断。

所有路线通过、失败、中断或超时后，都要确认本次启动的进程已清理。手动启动 QEMU/GDB 也由启动者负责清理，不干扰其他任务的进程。

## sifive_u 平台边界

该模型对应 HiFive Unleashed：hart 0 无 MMU，可运行 hart 为 1–4，DRAM 128MiB，timebase 1MHz，boot hart 不固定。内核继续运行时探测现代 SBI，不因板型历史调用 v0.1 核心 ABI，也不引入专用内核政策。

模型没有可用 shutdown 后端。`just sifive_u` 遇到明确 reset 失败或 panic 终态时主动收割，再检查完整 core 锚点；终态日志触发收割不代表验收自动通过。

## 日志与调查

长时间构建、测试和验收保存完整输出，终端只展示末尾摘要；失败必须保留完整日志、打印路径并传递真实退出码，不用 `tail` 截断后丢失错误上下文。摘要长度使用对应脚本环境变量调整，需要诊断时按日志定位原始错误。

通过规定检查后停止；只有新改动、失败或具体未解决风险触发补测。挂起与损坏按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 调查，工具、GDB 和汇编细节见 [TOOLING-PITFALLS](TOOLING-PITFALLS.md)。
