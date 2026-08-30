# 平台物理供给账本未来审查

> 【未来审查计划】审查对象固定为提交 `198e665`（`feat(mm): 闭合平台物理供给账本`）。只审该提交形成的平台供给闭包，不把后续系统储备、MemoryPool 状态机或动态 reserved-memory 生命周期混入结论。

## 对象概要

该提交解析全部 `/memory` tuples、FDT memory reservation block 与静态 `/reserved-memory/reg`，将 `no-map` 同时排除于普通供给和标准直映射；启动期把平台永久区、内核永久区、DTB、cold-bootstrap、BootPackage、FramePool metadata 与 free inventory 分类闭合。正式直映射由固定静态预算的 eager mapper 按 1GiB/2MiB/4KiB 建立；transition 表只精确映射内核与实际 DTB 页，并在相应 boot-held 页回投前撤销临时叶。动态 `size` 与 `reusable` 在独立生命周期机制完成前 fail closed。

## 审查重点

1. 逐条对照 Devicetree Specification v0.4，复核 cell 继承、`status`、多 tuple、页边界、reservation block、静态 `reg`、`no-map`、`reusable` 与 `reg`/`size` 优先级；确认拒绝面没有误收合法描述或吞掉 malformed 描述。
2. 对每类交叠画出区间所有权：平台 reservation、SBI/内核、bootstrap、DTB、BootPackage、metadata 与 RAM 外 reservation；复算 `total = permanent + boot-held + metadata + free`，检查回投路径不存在双重释放、遗漏或把 RAM 外地址交给 FramePool。
3. 审查 cold transition 与正式 direct map 的 PTE 几何、identity/high-half 共用、跨 vpn2 槽、静态 middle/leaf 上界、页表发布顺序及临时叶撤销同步点；特别检查未来 HSM 重启是否只依赖永久映射。
4. 审查 `EagerMapper` 的最大叶选择、冲突预检、幂等粗叶、预算耗尽后的未发布树语义，以及用户页表只继承有效 kernel root 槽的 teardown 所有权。
5. 复核 QEMU 自备 DTS、`-m`、BootPackage window 与运行时 DTB 放置的板级闭包；virt 与 sifive_u 的差异不得沉淀为内核平台名特例。
6. 检查固定容量常量是否均有独立上界证明，debug-only 断言是否被误当作 release 正确性机制，启动栈与 ELF frame audit 是否覆盖最坏调用链。

## 基线证据

- `cd os && cargo test -p tar -p elf -p page_table -p frame_pool -p dtb -p handle_table -p wait_context -p timer_queue -p stack_layout -p sched_domain --target aarch64-apple-darwin`
- `cd os && cargo test -p dtb -p page_table --release --target aarch64-apple-darwin`
- `cd shared && cargo test --target aarch64-apple-darwin`
- `just check`
- `THROTTLE=100 just virt`
- `THROTTLE=100 just virt-release`
- `THROTTLE=100 just sifive_u`

该提交内容在上述验证中均通过；virt debug/release 均到 `race matrix acceptance passed: 16/16` 与 SBI shutdown，sifive_u 到同一矩阵终态并按明确 `NotSupported` reset 后端结果收割。

## 完成标准

所有发现按严重度给出文件/行证据、触发条件和守恒式影响；规范问题引用固定章节，页表问题引用 RISC-V privileged architecture/RVWMO 条款。阻断项修复并重跑对应基线后，本计划转为只读 review 档案；非阻断承接只进入既有唯一计划，不在审查文档中复制 TODO。
