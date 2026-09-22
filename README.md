# Halcyon

Halcyon 是以 eRhino RV64 微内核为核心、包含用户态系统服务与跨组件契约的完整系统项目。
用户态基础运行库为 `rinlib`。内核采用协作式执行，每次推进有结构性工作上界；业务政策与可由用户态承担的长工作在用户态服务中完成，已提交的特权维护由内核分批收束。
内核当前构建名为 `erhino_kernel`，按全仓组件身份规则统一为 `kernel` 的后续工作见[命名计划](plans/todo-2026-09-22-kernel-identity.md)。

## 文档

- [notes/](notes/README.md)：设计文档，按 ideas/（系统应该是什么）与 impls/（实际怎么做）
  两个视角分层，附全主题索引；
- [plans/COMPASS.md](plans/COMPASS.md)：方向、位置与活跃计划导航；
- [AGENTS.md](AGENTS.md)：项目契约、协作边界、施工流程与按需阅读入口；
- [构建与验证手册](plans/BUILD-AND-TEST.md)：工具链、检查命令、QEMU 路线与证据边界。

## 快速开始

### 先决条件

- Rust nightly（由仓库根的 `rust-toolchain` 指定，首次构建自动安装）
- build-std 源码：`rustup component add rust-src`（内核与用户态均使用仓库内自定义
  JSON target + build-std 编译，无需 rustup 安装预编译 target）
- 系统工具（macOS / Homebrew）：`brew install just dtc qemu riscv64-elf-binutils riscv64-elf-gdb`
  - `riscv64-elf-binutils` 提供内核链接器 `riscv64-elf-ld` 与 `riscv64-elf-objcopy`
  - `riscv64-elf-gdb` 用于调试

> QEMU 默认以内置 OpenSBI 固件作为 `-bios`，无需自行编译 OpenSBI。

## 运行与验收

构建系统用 [Just](https://just.systems)，可执行名为 `just`。所有 QEMU 路线默认经
CPU 节流与路线级超时保护（`THROTTLE=100` 全速；各超时可经同名环境变量覆盖）：

```sh
just virt          # qemu virt：日常 core 快速验收
just virt-stress   # qemu virt：完整压力与 16/16 竞态矩阵
just virt-release  # release core 验收
just virt-hetero   # 多调度域（无 F/D 的 Base64 域 + D64 域）集成验证
just virt-nofd     # 无兼容域验证（全 Base64 拓扑）
just sifive_u      # qemu sifive_u：板级 core 验收（hart 0 禁用）
just acceptance    # 闭包收尾：clippy + stress + release + sifive_u + nofd + boot-failure
```

秒级检查与仅编译内核：

```sh
just check         # os workspace 及其依赖的 cargo check
just build_kernel  # 仅编译内核（alias: just b）
```

调试：让 QEMU 以 `-s -S` 启动并暂停等待，再用 `riscv64-elf-gdb` 连接：

```sh
# 终端 1（THROTTLE=100 关闭节流，便于断点单步）
THROTTLE=100 just PLATFORM=qemu MODEL=virt run_qemu -smp cores=4 -s -S
# 终端 2
riscv64-elf-gdb artifacts/qemu/virt/erhino_kernel -ex 'target remote :1234'
```

导出 QEMU 生成的设备树：

```sh
just run_qemu_dump_dtb
```

## (将)受支持的平台

- [x] qemu-virt: 4 cores 1GiB ram with MMU
- [x] qemu-sifive_u: 5 cores(#0 disabled) 128MB ram with MMU
