# sifive_u 平台缺陷与差异记录

QEMU `sifive_u` 模拟 SiFive FU740（U74 核心簇）。该模型年代早、实现残缺，
与 `virt` 同为集成验证负载的第二块板。本文件记录实测的平台行为差异与缺陷，
作为「代码问题还是平台问题」的第一判据——新疑点先对照此处，再决定是否
归因到内核。

实测基线：QEMU 11.1.0 自带 OpenSBI（fw_dynamic），2026-08 压测取证。

## 板级差异（非缺陷，内核已按契约收口）

- hart 0 是无 MMU 的 E51 监视核，可运行 hart 为 1–4；
- boot hart 不固定（实测同命令两次运行分别为不同 hartid）；
- DRAM 仅 128 MiB，initfs 装载地址 0x86000000（见 memory.x / Justfile）；
- timebase 为 1 MHz；
- 无平台 shutdown 设备：SRST 可能存在但 shutdown 不保证 QEMU 退出，
  运行阶段以 timeout 收束（Justfile `run_qemu_timed`）。

## 平台缺陷（实测复现）

### S 态 `time` CSR 由固件模拟（非缺陷，行为差异）

本模型未向 CPU 注册 rdtime 回调（QEMU `hw/riscv/sifive_u.c` 传
`provide_rdtime=false`，virt 为 true）：S 态 `csrr time` 硬件上触发
illegal instruction，medeleg 不委托该异常，由 OpenSBI 在 M 态模拟
（读 mtime 写入目标寄存器、`mepc += 4`）。因此：每轮可见数十次非法
trap 日志但内核无感，时间值正确。含义：① 内核不能靠「会不会 trap」
探测 time 可用性；② 每次 rdtime 是一次 M 态往返，热路径开销显著；
③ 若需真实 trap 语义，可由内核清 MCOUNTEREN.TM 后经 redirect 接收。
对照：`senvcfg` 自 privileged 1.12 才存在，本模型不实现，读写均非法；
OpenSBI 无法模拟，redirect 回 S 态，由内核 csr_try 探测机制消化
（不可用即放弃，无需平台补丁）。

## 历史（pre-ai 时期踩过，重构后未必仍适用）

- SiFive core 曾被观察到要求 PTE 预置 A/D 位否则 page fault
  （pre-ai commit 370d882）；当前 Sv39 实现已恒置 A/D。
- OpenSBI 唤醒的 secondary 无法接收 IPI 曾导致放弃 IPI 调度方案
  （pre-ai commit ca400da）。

## 准入纪律

平台缺陷一律先记录于此再谈兼容；兼容必须以机制形态存在（能力探测 +
声明式 fallback，如 csr_try / DT ISA 准入），禁止散布平台特判补丁。
无法机制化的缺陷即声明不支持，宁缺毋滥。
