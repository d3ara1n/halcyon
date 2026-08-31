# 系统物理储备未来审查

> 【未来审查计划】审查对象固定为提交 `0a944c7ae86b141207b737afcab240c8e2a9f5c7`（`feat(mm): 隔离系统物理储备`）。只审该提交形成的 system/user 物理隔离、静态容量证明与内核 heap 供血路径；平台供给账本仍归 `todo-2026-09-platform-memory-ledger-review.md`，后续 MemoryPool、funded broker 与 KernelMemoryBudget 不混入本结论。

## 对象概要

该提交新增零分配、固定 workspace 的 `os/memory_supply` planner，在 FramePool 发布前把 managed RAM 原子分类为 permanent、boot-held、system 与 user-free。system supply 以不同 owner 类型持有 FramePool metadata、16 个预清零的 1 MiB heap chunks 与当前为零的 recovery tickets；唯一 FramePool 只接收 user-free，Talc 扩展只 O(1) 消费 heap ticket。raw frame adapter 收窄为 transitional user-inventory 接口，页表、匿名 backing、Tunnel 与库存自检是当前唯一消费者。

## 审查重点

1. 逐个中间集合复算 planner 的区间代数、优先级与失败原子性：managed 输入互斥、permanent 裁剪、boot-held 扣除、system 低地址确定性放置、user-free 补集及全局分类闭包；确认任何错误都发生在 `Plan` 发布前。
2. 独立证明 `MAX_CLASSIFIED_RANGES = M + 2MP + MB + 1 + H + R = 1169` 覆盖 permanent 跨 memory 裁剪、boot-held 被 permanent 切碎、unavailable 合并与最终补集的全部中间峰值；构造接近最坏布局验证不存在静默截断或把保守上界误作输入合法性约束。
3. 复核 FramePool `MAX_ARENAS = 16 × 2 × usize::BITS = 2048` 的 canonical 分解证明，以及 metadata 字节数对 managed frame 数、固定 arena 表、页对齐与实际切片布局的覆盖；检查大 workspace/metadata 均在 BSS 或 system pages，不回到启动栈或 user supply。
4. 追踪 `FramePoolMetadata`、`HeapChunkTicket`、`RecoveryTicket` 与 `SystemSupply` 的 affine 所有权，确认不同用途没有公共可消费转换、system 页从不进入 FramePool、boot-held 回投不会误含 permanent/system，失败与析构也不能制造可重用系统页。
5. 对照 Talc 版本契约复核 `Source::acquire` 的锁内调用语义：所有 heap chunks 必须在 allocator 首次使用前清零，运行期只持 HEAP→SYSTEM_SUPPLY 合法锁序做 O(1) pop/claim，不取得 POOL、不扫描区间、不 memset，也不以 recovery 或 user inventory 回退。
6. 重做 Commit 后 completion、rollback、drain、remote-call 与错误恢复路径分配审计，验证 recovery=0 仍由“无物理页消费者”推出；任何新增消费者必须先有静态并发上界、独立预算和 exhaustion-before-Commit 测试。
7. 搜索全部 frame adapter 调用点，确认旧 `alloc_order`/`alloc_largest` 与 heap permanent-transfer 路径已经消失；transitional `alloc_user_order`/`alloc_user_largest` 只服务已登记的四类消费者，并在 funded broker 接入后按主计划继续收窄。
8. 复核启动事务与锁生命周期：planner、SystemSupply、FramePool 的全局发布顺序不可暴露半初始化状态；所有页数线性清零位于相关自旋锁外；日志与 release 断言都独立证明 `managed = permanent + boot-held + system + user-free` 及 `system = metadata + heap + recovery`。

## 基线证据

- `cd os && cargo test -p memory_supply -p frame_pool --target aarch64-apple-darwin`
- `cd os && cargo test -p memory_supply -p frame_pool --release --target aarch64-apple-darwin`
- `just check`
- `THROTTLE=100 just virt`
- `THROTTLE=100 just virt-release`
- `THROTTLE=100 just sifive_u`

该提交内容在上述验证中均通过：memory_supply 7 项与 FramePool 16 项 host 测试的 debug/release 全绿；三条平台线均到 `race matrix acceptance passed: 16/16`。最终闭包为 virt debug `262144 = 1495 + 9515 + 4236 + 246898`、virt release `262144 = 1308 + 254 + 4236 + 256346`、sifive_u debug `32768 = 1063 + 9515 + 4124 + 18066`；system 子账户分别为 virt `4236 = 140 + 4096 + 0` 与 sifive_u `4124 = 28 + 4096 + 0`。virt 正常 shutdown，sifive_u 按明确 `NotSupported` reset 后端结果收割。

## 完成标准

所有发现按严重度给出文件/行证据、可达输入或并发条件，以及对两层守恒式和用途隔离的影响；容量问题必须给出独立组合上界或反例，Talc 语义必须引用审查时锁定版本的官方 API/源码证据。阻断项修复并重跑对应基线后，本计划转为只读 review 档案；非阻断承接只进入既有唯一计划，不在审查文档中复制 TODO。
