# 外部契约索引

本文件只负责定位，不复述规范。引用 finding 时使用目标文件的小节标题。

## SBI

根目录：`normative/riscv-sbi-v3.0/src/`

| 概念 | 文件 |
|---|---|
| ecall 寄存器、错误码、返回结构、hart list | `binary-encoding.adoc` |
| 规范版本编码、扩展探测 | `ext-base.adoc` |
| 时钟编程 | `ext-time.adoc` |
| IPI hart mask/base | `ext-ipi.adoc` |
| hart 启停与状态 | `ext-hsm.adoc` |
| Debug Console 地址与部分读写 | `ext-debug-console.adoc` |
| 系统重置与停机 | `ext-sys-reset.adoc` |
| legacy 调用 | `ext-legacy.adoc` |
| 版本差异 | `changelog.adoc` |

## RISC-V ISA 与特权架构

根目录：`normative/riscv-isa-v20250508/src/`

| 概念 | 文件 |
|---|---|
| S 态 CSR、trap、interrupt、`sstatus`、`stvec`、`sscratch`、`sepc`、`scause`、`satp`、`SFENCE.VMA` | `supervisor.adoc`，地址翻译同步见「Supervisor Memory-Management Fence Instruction」与「ASID Usage」 |
| `SRET`、`MRET`、`WFI` 等通用特权指令 | `priv-insns.adoc` |
| 特权架构版本与模块状态 | `priv-preface.adoc`、`priv-history.adoc` |
| RV64I 寄存器与基础指令 | `rv32.adoc`、`rv64.adoc` |
| `FENCE.I` 与指令流同步 | `zifencei.adoc`「FENCE.I Instruction」 |
| 扩展上下文的 FS/Initial/Clean/Dirty 状态 | `machine.adoc`「Extension Context Status」 |
| 浮点指令、寄存器与 fcsr | `f-st-ext.adoc`、`d-st-ext.adoc` |
| RISC-V 弱内存模型、`FENCE` 与 acquire/release | `rvwmo.adoc`；解释见 `mm-eplan.adoc`「Fences」「Explicit Synchronization」 |
| CSR 与内存先后、time 采样的 R/I/W fence | `zicsr.adoc`「CSR Access Ordering」；`rv32.adoc`「Memory Ordering Instructions」，CSR read 分类为 device input |
| 原子指令与 LR/SC | `a-st-ext.adoc` |
| time 计数器、跨 hart 同步、频率与回绕 | `counters.adoc`「"Zicntr" Extension for Base Counters and Timers」；`machine.adoc`「Machine Timer (mtime and mtimecmp) Registers」 |

## psABI、调用约定与 ELF

根目录：`normative/riscv-psabi-v1.0/`

| 概念 | 文件 |
|---|---|
| 整数/浮点寄存器约定、栈、参数、返回值、系统调用 | `riscv-cc.adoc` |
| RISC-V ELF header、relocation、section、program header | `riscv-elf.adoc` |
| 汇总入口 | `riscv-abi.adoc` |
| DWARF 寄存器编号 | `riscv-dwarf.adoc` |

## Devicetree

根目录：`normative/devicetree-v0.4/source/`

| 概念 | 文件 |
|---|---|
| node/property、`compatible`、`reg`、address/size cells | `chapter2-devicetree-basics.rst` |
| `/chosen`、`/cpus`、`/memory` | `chapter3-devicenodes.rst` |
| 设备绑定 | `chapter4-device-bindings.rst` |
| Flattened Devicetree 二进制格式 | `chapter5-flattened-format.rst` |
| DTS 源语言 | `chapter6-source-language.rst` |
| 通用 CPU 属性与 `cpu-map` 拓扑 | `normative/devicetree-schema-v2026.06/cpu.yaml`、`cpus.yaml`、`cpu-map.yaml` |
| NUMA distance map | `normative/devicetree-schema-v2026.06/numa-distance-map-v1.yaml` |
| RISC-V CPU 节点、现代 `riscv,isa-base`/`riscv,isa-extensions` | `normative/riscv-dt-bindings-linux-818bebeb/cpus.yaml`、`extensions.yaml` |

## Rust 语言与共享内存访问

本轮取证版本：rustc `c54751567b19c4ceb08b0412d83529c2568cba8b`（1.100.0-nightly，LLVM 23.1.0）。仓库当前使用浮动 nightly，实施时须记录实际版本并核对代码生成；证据边界见 [`共享访问取证`](../plans/ref-2026-09-shared-memory-access.md)。

| 概念 | 固定源码 / 规范入口 |
|---|---|
| Rust 原子、混合尺寸访问、from_ptr 契约 | [core/sync/atomic.rs](https://github.com/rust-lang/rust/blob/c54751567b19c4ceb08b0412d83529c2568cba8b/library/core/src/sync/atomic.rs)「Memory model for atomic accesses」及整数原子 from_ptr |
| 指针、allocation、volatile 与普通拷贝 | [core/ptr/mod.rs](https://github.com/rust-lang/rust/blob/c54751567b19c4ceb08b0412d83529c2568cba8b/library/core/src/ptr/mod.rs)「Safety」「Allocated object」、read_volatile/write_volatile |
| inline asm 编译器义务 | [Rust Reference](https://doc.rust-lang.org/reference/inline-assembly.html#rules-for-inline-assembly)「Rules for inline assembly」；在线入口非固定快照，实际平台序列仍以本表 rustc 版本与固定 RISC-V ISA 验证 |

## 实现参照

| 概念 | 文件 |
|---|---|
| OpenSBI 平台要求 | `implementations/opensbi-v1.9/docs/platform_requirements.md` |
| OpenSBI generic/QEMU virt 平台 | `implementations/opensbi-v1.9/docs/platform/generic.md`、`platform/qemu_virt.md` |
| OpenSBI HiFive Unleashed 平台 | `implementations/opensbi-v1.9/docs/platform/sifive_fu540.md` |
| QEMU `virt` 机器 | `implementations/qemu-v11.1.0/riscv/virt.rst` |
| QEMU `sifive_u` 机器 | `implementations/qemu-v11.1.0/riscv/sifive_u.rst` |
| Linux RISC-V trap 指令用法检索（非设计依据） | `implementations/linux-riscv-818bebeb/arch/riscv/kernel/entry.S` |
| Linux secondary/CSR 指令用法检索（非设计依据） | `implementations/linux-riscv-818bebeb/arch/riscv/kernel/head.S`、`cpu_ops_sbi.c`、`smpboot.c` |
| Linux 局部 FPU 汇编及 ABI 构建用法检索（非设计依据） | `implementations/linux-riscv-818bebeb/arch/riscv/Makefile`、`kernel/fpu.S`、`include/asm/switch_to.h` |
| Linux ISA 属性解析用法检索（非设计依据） | `implementations/linux-riscv-818bebeb/arch/riscv/kernel/cpufeature.c`、`include/asm/cpufeature.h` |
