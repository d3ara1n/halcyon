# Page Guard 改动 Review 计划

栈窗口 guard 页机制已落地（设计见 `notes/impls/mm.md`「栈窗口」、
`notes/impls/execution-context.md`「地址空间归属纪律」），横跨三个提交。
实现经过双平台集成验证，但属于启动路径 + 页表 + 生命周期三处交叉的
敏感改动，需在 IPC 主线间隙做一轮系统性 review。

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
