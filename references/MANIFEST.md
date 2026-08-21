# 上游语料清单

获取日期：2026-08-21。

| 目录 | 上游版本 | 固定 commit | 官方来源 | 许可证 | 入库范围 |
|---|---|---|---|---|---|
| `normative/riscv-isa-v20250508/` | `riscv-isa-release-ed0aaa2-2025-05-08` | `ed0aaa296203a68f7b1f4a231c3971cd3ed28973` | <https://github.com/riscv/riscv-isa-manual> | CC BY 4.0 | `src/`，移除仅用于渲染的图片；保留 `LICENSE` |
| `normative/riscv-sbi-v3.0/` | `v3.0`（Ratified） | `c33ad9f414505806f084e8677e04d2744f76c8df` | <https://github.com/riscv-non-isa/riscv-sbi-doc> | CC BY 4.0 | `src/`，移除仅用于渲染的 ditaa 图；保留 `LICENSE` |
| `normative/riscv-psabi-v1.0/` | `v1.0` | `74936a9e25dec3c5b297b19e4c37e97d14868b22` | <https://github.com/riscv-non-isa/riscv-elf-psabi-doc> | CC BY 4.0 | ABI、calling convention、ELF、DWARF AsciiDoc；保留 `LICENSE` |
| `normative/devicetree-v0.4/` | `v0.4` | `112f53cc57e5931f1503dfcaa1644caf15362c30` | <https://github.com/devicetree-org/devicetree-specification> | Apache-2.0 | `source/`；保留 `LICENSE`、`NOTICE` |
| `normative/devicetree-schema-v2026.06/` | `v2026.06` | `0d16008e39254b487564e171dcd2269d978550cf` | <https://github.com/devicetree-org/dt-schema/tree/v2026.06/dtschema/schemas> | BSD-2-Clause | 通用 CPU、CPU topology、NUMA distance schemas；保留 `LICENSE.txt` |
| `normative/riscv-dt-bindings-linux-818bebeb/` | Linux commit 快照 | `818bebeb63dd6bf5f4e07e145f6cdbace520a34c` | <https://github.com/torvalds/linux/tree/818bebeb63dd6bf5f4e07e145f6cdbace520a34c/Documentation/devicetree/bindings/riscv> | GPL-2.0 OR MIT | RISC-V CPU 与 ISA extension bindings；保留两种许可证文本 |
| `implementations/opensbi-v1.9/` | `v1.9` | `cbf9f6734dd85a982c63e3cb5db7ffe09da839ca` | <https://github.com/riscv-software-src/opensbi> | BSD-2-Clause | `docs/`，移除图片；保留 `COPYING.BSD` |
| `implementations/qemu-v11.1.0/` | `v11.1.0` | `84f07211cc5b4fc6a371559bf8a5de4fb068e648` | <https://github.com/qemu/qemu> | GPL-2.0 | `virt.rst`、`sifive_u.rst`；保留 `COPYING` |
| `implementations/linux-riscv-818bebeb/` | Linux commit 快照 | `818bebeb63dd6bf5f4e07e145f6cdbace520a34c` | <https://github.com/torvalds/linux/tree/818bebeb63dd6bf5f4e07e145f6cdbace520a34c/arch/riscv> | GPL-2.0 | trap、启动、FPU、CPU capability、SMP 相关源码；保留 `COPYING` |

上述副本均未修改正文。删去的渲染资产不承载本项目 review 所需的规范文字；需要恢复时从对应 commit 重新导入。
