# Halcyon

Halcyon 是以 eRhino RV64 微内核为核心、包含用户态系统服务与跨组件契约的完整系统项目。
内核二进制为 `erhino_kernel`，用户态标准库替代品为 `rinlib`。长工作一律在用户态系统服务
（`srv_init` / `srv_fs` / `srv_pm`），内核只做短路径转发。

## 文档

- [notes/](notes/README.md)：设计文档，按 ideas/（系统应该是什么）与 impls/（实际怎么做）
  两个视角分层，附全主题索引；
- [plans/COMPASS.md](plans/COMPASS.md)：方向、位置与活跃计划导航；
- [AGENTS.md](AGENTS.md)：协作约定、仓库结构与构建验证细则。

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
just acceptance    # 阶段收尾聚合：stress + release + sifive_u
```

秒级检查与仅编译内核：

```sh
just check         # 内核 + shared 的 cargo check
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
