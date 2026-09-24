# 构建与验证

运行构建、测试、QEMU 或调整验收基础设施前读取本篇。任务是否完成由 [AGENTS「任务适用范围与完成条件」](../AGENTS.md#任务适用范围与完成条件)判断；同一路线在施工期间可作开发检查，路线通过本身不宣告机制完成。具体命令和默认参数以 [Justfile](../Justfile) 为执行真值。

## 工具链与构建入口

- 开发机为 macOS，系统工具为 Homebrew 的 `just`、`dtc`、`qemu`、`riscv64-elf-binutils`、`riscv64-elf-gdb`；系统级安装由用户执行。
- Rust edition 2024，工具链由根目录 `rust-toolchain` 选择；当前内容是浮动 `nightly`，并未固定日期。取证或比较代码生成时记录实际 rustc 版本。
- 内核、共享库、用户态分别属于 `os/`、`shared/`、`user/` 三个 workspace，以 path 依赖连接。内核 target 为 `os/riscv64imac-unknown-erhino.json`，用户态默认 target 为 `user/rinlib/riscv64imac-unknown-erhino-elf.json`，均使用 build-std；浮点验收 `test_fp` 使用独立 gc target。
- **统一走 `just`，不裸跑 `cargo build`**。内核链接器 `riscv64-elf-ld` 和链接脚本由构建入口配置，用户态由自定义 target 与 build-std 提供运行环境。
- 内核 package、默认 binary target 和产物名统一为 `kernel`；本文命令与构建入口以此为准。
- initfs 使用 tar 时避免 bsdtar 的 `._` AppleDouble 文件混入归档。

```sh
just build_kernel
just check
(cd shared && cargo check)
just clippy
```

`just check` 对 os workspace 执行带 build-std 的 cargo check，包含它的实际依赖，不等于检查所有 workspace。`just clippy` 覆盖 shared/os/user host、kernel/user RISC-V、stress feature 与独立 gc target 七个分面，全部使用 `-D warnings`；完整日志写入 `artifacts/lint/`。

## Host 测试

纯逻辑 crate 的 host 测试必须显式指定 host target，不能继承 RISC-V 默认 target 去链接 std。当前开发机命令：

```sh
(cd os && cargo test --workspace --exclude kernel --target aarch64-apple-darwin)
(cd shared && cargo test --workspace --target aarch64-apple-darwin)
```

根据改动先选择受影响的 package 或现有 testcase。新增测试说明其独立契约和目标错误；无关测试或新增 harness 不作为默认交付物。

## QEMU 路线与选择

workload 由用户态 `srv_init` 编译期控制，内核不感知测试政策。服务、驱动、验收进程分别位于 `user/services/`、`user/drivers/`、`user/tests/`；验收配置中的进程也可承载正式机制，成熟度由对应 impls 与任务记录分别说明。

| 命令 | 覆盖与用途 |
|---|---|
| `just virt` | 日常 debug core：确定性内存、IPC、Tunnel、Job、监督与 reset |
| `just virt-stress` | core 加 control/Tunnel 重复压力、`max_work=1` Drain 与完整 16/16 竞态矩阵 |
| `just virt-release` | release core，覆盖优化代码生成和 trap 寄存器保持 |
| `just sifive_u` | 128MiB 与非零 boot hart 等板级差异，运行 core |
| `just virt-nofd` | 无 F/D、无兼容浮点域的 core 路线 |
| `just virt-hetero` | 异构调度域契约；涉及调度域时额外运行 |
| `just virt-boot-failure` | 外部 GDB 对正式 debug binary 注入 Ready 前 panic/alloc/fatal，确认所有 hart 在 Failed 后停驻 |
| `just acceptance` | 闭包收尾聚合：先 clippy，再 debug stress、release core、sifive_u core、nofd、boot-failure |

施工中可以复用上述路线查找集成错误，结果只作为开发证据。机制闭包收尾按实际影响执行 `just check`、`just clippy` 与必要路线；跨机制组合收尾使用 `just acceptance`，涉及调度域再加 hetero。文档等不改变执行行为的工作按影响核验，不运行无关的代码验收。

## 节流、超时与判定

常规路线经 `tools/qemu-throttle.sh` 和 `tools/qemu-acceptance.sh`。默认 `THROTTLE=50`，stress 同样节流，不作为全速性能基准；GDB 与全速专项诊断用 `THROTTLE=100`。节流只限制宿主资源占用，全速也仍经过路线的日志、锚点与超时判定。

各路线有独立超时，可用 `VIRT_TIMEOUT`、`VIRT_STRESS_TIMEOUT`、`VIRT_RELEASE_TIMEOUT`、`VIRT_HETERO_TIMEOUT`、`VIRT_NOFD_TIMEOUT`、`SIFIVE_U_TIMEOUT`、`VIRT_BOOT_FAILURE_TIMEOUT` 覆盖。默认值只在 Justfile 维护；virt stress 默认 420 秒，因为竞态矩阵和 FAL 资源/退出组合持续增加；sifive_u 默认 120 秒，因为 128MiB/五 hart 路线在完整 FAL core workload 下需要更长的 QEMU 仿真预算。超时从 QEMU 运行阶段计，不含冷编译；聚合命令不另设跨路线总时限。

超时负责收割异常停滞，不是性能目标。默认值依据对应 workload 在相同节流条件下的近期正常耗时留出宽裕余量；验收面或运行成本变化时重校。正常运行经常接近上限时，先确认运行身份和进展，再调整 recipe 与文档，不能把基础设施截断当成内核挂死，也不能用放宽超时掩盖缺失业务锚点。调查已定位的早期卡死时可临时收紧。

判定依据 workload 身份、完整业务收束和显式 reset 锚点，不能用矩阵中途超时宣称内核失败。`virt-boot-failure` 同样节流，由 `tools/check-boot-failure.py` 独立判定预期故障、限制 GDB 时间并回收进程组，完整日志在 `artifacts/boot-failure/`。

所有路线通过、失败、中断或超时后，都要确认本次启动的进程已清理。手动启动 QEMU/GDB 也由启动者负责清理，不干扰其他任务的进程。

## sifive_u 平台边界

该模型对应 HiFive Unleashed：hart 0 无 MMU，可运行 hart 为 1–4，DRAM 128MiB，timebase 1MHz，boot hart 不固定。内核继续运行时探测现代 SBI，不因板型历史调用 v0.1 核心 ABI，也不引入专用内核政策。

模型没有可用 shutdown 后端。`just sifive_u` 遇到明确 reset 失败或 panic 终态时主动收割，再检查完整 core 锚点；终态日志触发收割不代表验收自动通过。

## 日志与调查

长时间构建、测试和验收保存完整输出，终端只展示末尾摘要；失败必须保留完整日志、打印路径并传递真实退出码，不用 `tail` 截断后丢失错误上下文。摘要长度使用对应脚本环境变量调整，需要诊断时按日志定位原始错误。

通过规定检查后停止；只有新改动、失败或具体未解决风险触发补测。挂起与损坏按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 调查，工具、GDB 和汇编细节见 [TOOLING-PITFALLS](TOOLING-PITFALLS.md)。
