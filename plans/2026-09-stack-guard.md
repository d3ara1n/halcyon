# 内核栈溢出防护（设计任务）

背景：sifive_u 野跳转根因为 `free_subtree` 在 debug 构建下生成
0x40d0 栈帧，超过每 hart 0x4000 栈区，向下越界踩踏相邻 hart 内核栈
（案例见 `plans/debug-playbook.md`）。实例已修，本任务是建立系统性
防护：溢出必须即时可见，不允许静默污染。非急迫，IPC 主线间隙做。

## 业界参照

- Linux `CONFIG_VMAP_STACK`：线程栈经 vmalloc 独立映射，栈底下方
  guard 页不映射，溢出即 page fault（riscv 支持）；
- Zephyr：MPU/MMU stack guard region、`CONFIG_STACK_SENTINEL`
  （栈底哨兵字，上下文切换时检查）、thread analyzer 水位测量；
- FreeRTOS `configCHECK_FOR_STACK_OVERFLOW`：哨兵 pattern 与
  high-water-mark 两档。

## 方案选项

### A. guard page（推荐的目标形态）

每 hart 栈区之间留一页 unmap 的 VA 洞，溢出立即 store page fault →
fatal 转储，第一现场直接定位。零运行时开销。

设计要点：
- 现状栈区连续排布（slot i 自高地址向低地址紧挨），需改为隔洞排布；
- 直映射纪律：栈区从直映射连续区间中挖洞，或栈区改为独立映射——
  两者对 phys_to_virt 契约的影响要先成文；
- fatal 路径自身不能再溢出：sp 已坏时切 emergency 栈的时序要复核；
- bootstrap 过渡环境的栈不受此布局管辖，边界情况要写明。

### B. canary 哨兵

栈底最低地址放 magic word，调度切换/时钟 tick 检查。实现半天，
但检测延迟、跨栈穿透只能报「有人越界」不能定位写者。不单独做，
仅在 A 设计期间需要临时保险时作为探针存在。

### C. 构建期栈帧审计（先行落地的小任务）

Rust 无 `-Wframe-larger-than`；扩展 `os/tools/audit_elf.py`：扫描
全部函数 prologue 的 sp 减量合计，超过阈值即构建失败。阈值按
MODE/MODEL 区分（debug 帧显著大于 release），初值建议正常栈预算
（0x3000）的 1/4。零运行时成本，防本案复发；局限是抓不到深调用链
总和超限，由 A 兜底。

## 实施顺序

C 先行（独立小提交）；A 成文设计（直映射契约影响先行确认）后实施；
B 不立项。
