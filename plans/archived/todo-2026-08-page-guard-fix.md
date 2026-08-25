# todo：栈窗口 guard 机制缺陷修复

来源：[page guard review](todo-2026-08-page-guard-review.md) 的必须修项。
修复完成后结论回写 `notes/impls/mm.md`「栈窗口」，两份文档一并归档。

## 缺陷

1. **单页 guard 可被合法大帧跨过**（高）：`mm.rs` `GUARD_SIZE = 0x1000`，
   `audit_elf.py` 允许单帧 `0x1800`（当前最大实帧 `0x1620`）。进入函数前
   sp 距 guard 顶不足 `0x620` 时，一次 sp 下调整体越过 guard，落入前一
   hart 已映射栈页——无 fault、静默跨槽污染，「溢出即时可见」不成立。
2. **直映射满配与栈窗口共占 root slot 511**（中）：`DIRECT_VPN2_LIMIT
   = 256` 使直映射覆盖 [256, 512) 全部顶层槽，`STACK_WINDOW_SLOT = 511`
   随后覆写之；PA [255GiB, 256GiB) 直映射静默消失而 `phys_to_virt` 仍
   声称线性可转。当前平台不触发，启动校验必须显式拒绝该配置。
3. **emergency 栈无独立 guard**（中）：4KiB emergency 下方是仍映射的
   正式栈，溢出先破坏正式栈再 fault；fatal 路径（FatalFrame 304B +
   Rust 诊断）无结构性预算保证。
4. **audit_elf.py 文档失真**（低）：头注释 0xc00 ≠ 实际 0x1800，注释引
   用已归档 todo 旧名。

## 修复方向（机制收编，不打补丁）

- **guard 跨度与审计阈值共享同一布局真值**：guard ≥ 允许最大单帧
  （构建期强制 `MAX_FRAME <= guard_span`），映射循环仍按 4KiB 建 PTE；
  首选引入 `StackWindowLayout` 单一布局对象（VA 基址/guard/stride/
  物理基址/槽数经链接符号构造并自校验），消除四处散布的布局知识。
- 直映射上限收紧至 255 或由布局对象显式拒绝与栈窗口槽重叠。
- emergency 独立 guard（或纳入布局对象的统一槽模型），并建立 fatal
  路径栈预算断言。
- 布局纯逻辑抽 host 测试：跨 guard 最大帧、满配槽重叠、sifive_u 单
  叶表向量。

## 验证

- host 布局测试全绿；`just check`、`just virt`、`just sifive_u` 通过；
- audit_elf.py 文档与阈值一致，构建期新增的 `MAX_FRAME <= guard_span`
  断言生效；
- `notes/impls/mm.md` 栈窗口篇反映新布局。

## 完成条件

四缺陷修复、验证通过、文档同步后，本文件与 page guard review 计划
一并归档；值得重构清单中未随本修复落地的项（TableTree borrowed-root
所有权、统一 Switch 边界切 satp、Boot Reservations）转交后续里程碑
或 notes 立项。
