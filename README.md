# eRhino

操作系统学习：RV64

## 设计

参阅 `./notes/{doc}.md`

## 快速开始

### 先决条件

- Rust nightly（由仓库根的 `rust-toolchain` 指定，首次构建自动安装）
- RISC-V 目标与组件：`rustup target add riscv64gc-unknown-none-elf && rustup component add rust-src llvm-tools`
- 系统工具（macOS / Homebrew）：`brew install just dtc qemu riscv64-elf-binutils riscv64-elf-gdb`
  - `riscv64-elf-binutils` 提供 kernel 链接器 `riscv64-elf-ld`
  - `riscv64-elf-gdb` 用于调试
- 拉取子模块：`git submodule update --init`（`dtb_parser` 是内核硬依赖）

> QEMU 默认以内置 OpenSBI 固件作为 `-bios`，无需自行编译 OpenSBI。

## 进度

- [ ] IPC
  - [x] 信号
  - [ ] 消息
  - [ ] 隧道
    - [x] syscall
    - [ ] Runnel
- [ ] 设备租借
  - [ ] 中断转发
- [ ] 文件系统
  - [ ] FAL/syscall
    - [x] access
    - [x] inspect
    - [x] read
    - [x] write
    - [ ] create
    - [ ] delete
    - [ ] open
  - [ ] FAL/ipc
  - [ ] 内核文件系统
    - [ ] rootfs
    - [ ] procfs
    - [ ] sysfs
  - [ ] 具体文件系统
    - [ ] FAT32

## (将)受支持的平台

- [x] qemu-virt: 4 cores 128MB ram with MMU
- [x] qemu-sifive_u: 5 cores(#0 disabled) 128MB ram with MMU

## 标准库

~~Porting std is huge work, I wont do it at the current stage.~~

仅提供 `rinlib`

## 源码使用

构建系统用 [Just](https://just.systems)，可执行名为 `just`。

运行（自动编译内核与 initfs 并启动 QEMU）：

```sh
just virt       # qemu virt: 4 核
just sifive_u   # qemu sifive_u: 5 核（#0 禁用）
```

仅编译内核：

```sh
just build_kernel
```

调试：让 QEMU 以 `-s -S` 启动并暂停等待，再用 `riscv64-elf-gdb` 连接：

```sh
# 终端 1
just PLATFORM=qemu MODEL=virt run_qemu -smp cores=4 -s -S
# 终端 2
riscv64-elf-gdb artifacts/erhino_kernel -ex 'target remote :1234'
```

导出 QEMU 生成的设备树：

```sh
just run_qemu_dump_dtb
```
