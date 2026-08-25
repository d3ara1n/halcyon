# Page Guard 改动 Review 计划

状态：**review 已完成，发现必须修缺陷，修复归 [todo-2026-08-page-guard-fix](todo-2026-08-page-guard-fix.md)**；修复落地后本文档归档。

## Review 结论（2026-08-25，七项逐项）

| 项 | 结论 |
|---|---|
| ① 建表正确性 | 几何通过：virt 双叶表跨 2MiB 单元、slot 打包与 `virt_to_phys` 互逆、sifive_u 单叶表路径正确；但 guard 跨度缺口见必须修 |
| ② 地址算术 | 通过：仅 `mm.rs` 保留受控 `va - KERNEL_VA_BASE`；DBCN/HSM 全走 `virt_to_phys` |
| ③ teardown | 当前阶段通过：单线程 + 本 hart 持 Arc 链，AddressSpace::drop 先剥共享顶项再递归；残留竞争窗口维持 M3 接受位（execution-context.md） |
| ④ 死亡路径 | 现有路径通过：ecall exit 与用户异常均经 `report_exit`；Park/Requeue 进程存活前提无反例；未来 kill 必须走同一终止/切表边界 |
| ⑤ fatal 判定 | 通过：trap 入口切 emergency 前只碰 HartLocal，不在洞上取指；cause 12 命中 guard 属控制流损坏，另给诊断提示即可 |
| ⑥ 帧记账 | 当前平台通过：`.bss.*` 收编、栈区计入 `__kernel_pa_end`、帧池双剔除、bootstrap 只回投自有区间，物理边界无重叠 |
| ⑦ 栈预算 | 有问题：审计阈值 0x1800 > guard 0x1000（跳洞根因）；sifive_u 正式栈 0x3000 只剩约半给调用链 |

### 必须修（详见 fix todo）

1. 单页 guard 可被合法大帧跨过：相邻槽仅隔 0x1000，审计允许单帧 0x1800、当前最大 0x1620——进入函数前距 guard 顶不足 0x620 时一次 sp 下调越过整个 guard，落入前一 hart 已映射栈页，不产生 fault，「溢出即时可见」核心保证不成立。
2. 直映射满配与栈窗口共占 root slot 511：`DIRECT_VPN2_LIMIT = 256` 时直映射含 slot 511，随后被栈窗口覆盖，`phys_to_virt` 对 PA [255GiB,256GiB) 谎报可线性转换；当前平台不触发，但启动断言接受错误配置。
3. emergency 栈（4KiB）无独立 guard：下方是仍映射的正式栈，溢出先破坏正式栈而非 fault；`_fatal_entry` 先放 FatalFrame（304B）再跑 Rust 诊断，无结构性预算保证。
4. `audit_elf.py` 头文档（0xc00）与实际阈值（0x1800）不一致，且引用已归档旧 todo 名。

### 值得重构（机制层，随 fix 或后续里程碑）

- `StackWindowLayout` 单一布局对象：VA 基址/guard/stride/物理基址/槽数目前散在链接脚本、external.rs、mm.rs、main.rs，应集中为经符号构造并自校验的布局真值。
- `TableTree` 显式建模 borrowed root entries：现状靠 `proc.rs::Drop` 手工 `clear_slots` 配对，是所有权模型缺口；记录 root 槽所有权后可移除通用逃生口。
- 非 Resume trap 出口统一切 kernel satp：取代 `report_exit` 与调度循环顶部两处补丁式 `normalize_satp`，Killed/Park/Requeue 天然满足「调度循环恒内核页表」。
- 启动物理占用改 Boot Reservations 集合：帧池两洞 + bootstrap `free_range` 统一为排序合并、重叠校验的 reservation 表达。
- 栈窗口页 NX（当前 `KERNEL_DIRECT` 含 X）。

### 保留现状

- 栈窗口映射留 `mm.rs`（内核布局归属正确）；`page_table` crate 不引入 hart/guard 语义；SBI 地址统一经 `virt_to_phys`；cause 12 不标 stack overflow。

## 待 review 提交

- `eca758f` feat(kernel): 栈窗口 guard 页——内核栈溢出即时可见（主体）
- `4c3c3a3` chore(os): 运行时与构建输出口统一正式英文（含 clear_slots
  用例帧记账断言修正，review 时勿被英文化 diff 干扰）
- `9f003fb` docs(os): 注释内 notes 引用路径对齐 impls/ 新组织（纯注释）

`f5b9146` / `7791e73`（QEMU 节流与超时）不在本次范围。

## 重点核查项

1. **建表正确性**（mm.rs `map_stack_window`）：跨 2MiB 单元的叶表分配、
   guard 洞偏移计算、物理打包基址公式与 `virt_to_phys` 互逆性。virt 平台
   栈跨度恰好跨两个 2MiB 单元，是天然测试向量；sifive_u（0x4000 槽，
   单叶表）路径也要过一眼。
2. **地址算术假设审计**：全仓搜索对栈 VA 做 `va - KERNEL_VA_BASE` 类
   直映射算术的残留调用点；任何新代码经 SBI 传 PA 是否都走了
   `mm::virt_to_phys` 全函数。
3. **teardown 交互**：`AddressSpace::drop` 剥离 + `free_subtree` 递归的
   边界；已知残留竞争窗口——exit 与他 hart reap 并发时、normalize 生效
   前的短暂暴露（M3 接受，成文于 execution-context.md），评估何时需要
   升级为确定性方案（root 引用计数或调度循环恒内核页表）。
4. **死亡路径覆盖**：`report_exit` 内 normalize 是否覆盖全部终止来源
   （ecall exit、用户异常 abort、未来信号 kill）；Park/Requeue 出口不
   normalize 的前提（进程仍存活）是否有反例。
5. **fatal 判定完备性**：`is_guard_fault` 仅匹配 cause 13/15；
   KERNEL_DIRECT 含 X 位，guard 洞取指故障（cause 12）按理不可达但未
   防御——确认 emergency/fatal 自身不会在洞上取指。
6. **帧记账**：`.bss` 通配符收编后 `_bss_end` 外延扩大，确认帧池剔除、
   bootstrap 回收区间、initfs 三者的物理边界无新的重叠或遗漏。
7. **栈预算**：sifive_u 每 hart 0x4000 中 emergency 占 1/4，正式栈仅
   12KiB——对照 audit_elf 阈值复核余量。

## 完成标准

逐项给出结论并回填本文档；发现缺陷走正常 fix 流程，结论性事实回写
notes/impls 对应篇目。全部完成后本文件归档至 `plans/archived/`。
