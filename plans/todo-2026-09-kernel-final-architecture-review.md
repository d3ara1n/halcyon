# 内核主线完成后的最终架构收口 Review

## 性质

这是一次**内核主线完成后的全局结构审查与重构计划**。A–E 修复复核已经归档，本计划仍等待下列数据面与主要消费者触发条件，不阻塞多页 Tunnel、Runnel v2 或后续用户态服务实现。

当前阶段只登记观察对象，不提前判定其最终去留。原因是只有内核主线、shared ABI、rinlib 与主要用户态消费者共同完成后，才能从全局视角判断某个机制究竟是长期基础、必要的阶段性边界，还是 A→B 迁移中遗留的 C/D/E/F。

## 已登记提交范围

本表是已完成批次的事后审查入口，只登记固定提交与证据，不提前执行 Review。已有实施方案仅供参考；审查以整体需求、正确性和长期结构为依据，允许推翻方案，不以“已按方案完成”免除机制论证。

| 提交 | 批次与范围 | 验证基线 |
|---|---|---|
| `d00604a05d11d22656c02d0270161ba6301657d4` | 多页 Tunnel/RNL2 切片 8/9：几何 ABI、共同 backing、完整 lease、Endpoint owner、RV64 共享访问、真实消费者、失败/退役及旧路径删除；同时重校栈容量与布局派生审计 | host debug/release、七面 clippy、just check、默认与全速完整 acceptance（stress 16/16）、RNL2 14 项模型测试、审计工具 6 项测试和共享访问反汇编均通过 |

该批对应 [`数据面计划`](todo-2026-09-memory-object-data-plane.md)，统一审查时重点重新取证：

- Create/Attach 的结果与 shootdown 是否都来自唯一 lease 几何，所有 Commit 前失败是否保全 Invitation 和 affine owner；close 保活强环是否只在 Commit 后形成并按完成点解除。
- 完整 lease 的无空洞范围、连续对象 offset、页覆盖去重和最终 permit 退役，是否不依赖单 RegionKey/fragment 或当前物理连续性。
- 安全 owner 是否封死直接/间接 raw close 旁路；协议终态后是否停止共享访问，清理失败是否保留责任及可查询诊断。
- 共享访问的 Rust/LLVM/RV64 平台边界是否成立；host 合规模型与独立 guest 非合作改写证据不能互相替代或夸大为语言形式证明。
- 独立物理 cursor、u64 回绕、几何 shadow、EOF、Invited 期发布、ack→重查→wait 和部分完成错误是否组合闭合。
- backing/metadata/work 容量是否覆盖真实最后析构与退款调用链；12KiB guard 派生审计与两平台 256KiB 栈是否有完整布局、代码生成和运行证据，不能把单帧扫描当调用链证明。

切片 10、正式 FAL Open 与 RPC deadline 的独立能力缺口不因本批提交而完成；Review 保持下节统一触发条件。

## 触发条件

满足以下条件后执行：

- MemoryPool、funded frame、ProcessBindMemory、bootstrap、页表 owner 生命周期、deferred retire、匿名 backing、公共 MemoryObject、多页 Tunnel 与 Runnel v2 的内核/ABI 主线均已完成；
- 相关 shared ABI 与 rinlib 封装已同步，主要用户态服务和验收负载已迁移到最终接口；
- 当前自然序没有更高优先级的架构前置缺口；
- 执行前建立该主线最终提交范围与完整验证基线。

## 审查范围

### 结构与所有权

- Process、AddressSpace、MemoryPool、MemoryObject、ObjectBacking、ObjectView、Tunnel Connection/Endpoint/Invitation 的最终类型图；
- frame、quota、charge、metadata permit、Handle、WritePermit、WaitContext、Remote completion 和 work debt 的唯一 owner 与退款终点；
- Lock Ladder、锁外 funding、Commit 后零分配和 deferred retire 的统一协议；
- anonymous、object、Tunnel、boot-held backing 是否共享正确的机制、并保留必要的生命周期差异。

### 迁移残留

全仓搜索并分类：

- raw frame allocation 与 transitional adapter；
- 单页、单 PA、单 translation 特化路径；
- `BackingPlanFailure`、重复 Map validation、分散的 backing permit 传递等迁移接线；
- 仅为某个阶段或验收 workload 存在的字段、常量和分支；
- 重复的 owner/permit/charge 真值；
- 兼容层、旧 ABI、旧注释、旧实现文档和计划中的过时事实；
- 只验证“资源下降”而未证明精确守恒的验收逻辑。

### 阶段回溯

逐批回看 1、2、3、4、5、6A、6B、6C、6D、6E 以及其后的 MemoryObject/Tunnel/Runnel 提交，回答：

- 阶段引入的机制在最终架构中是否仍存在；
- 若仍存在，它是否已被统一到最终 owner/transaction seam；
- 若不再存在，哪些 C/D/E/F 仍依赖它；
- 是否有阶段性错误分类、容量限制或测试政策泄漏到长期接口。

## 输出分类

每个发现必须归入且只能归入以下一类：

### 长期保留

已经属于最终架构的机制。记录最终职责、唯一 owner、禁止的旁路和对应验证。

### 统一重构

语义正确但实现分叉、命名不统一或仍停留在过渡接口。记录目标 B、迁移顺序、受影响调用点和完成后的结构不变量。

### 明确删除

只为旧 A 或阶段性路径存在的 C/D/E/F。记录删除前置、删除范围、全仓搜索模式和删除后的验证。

### 另案立项

不是残留，而是独立能力缺口或后续设计问题。转入唯一的 plans/ 计划，不在本 review 中顺手扩大范围。

## 执行顺序

1. 冻结最终设计：类型图、所有权图、锁阶、失败边界和 ABI 归属；
2. 建立全仓符号与调用图，标记每个阶段引入的机制；
3. 生成残留矩阵，明确 A、B、C/D/E/F 关系；
4. 先删除重复真值和旧旁路，再统一最终 owner/transaction seam；
5. 迁移内核、shared、rinlib、服务和测试调用点；
6. 删除旧类型、字段、adapter、兼容分支与过时文档；
7. 做第二次全仓残留搜索，确认 C/D/E/F 不再被引用；
8. 运行 host debug/release、clippy、`just check`、virt core/stress/release、异构路线、sifive_u 与完整 acceptance；
9. 更新 notes/impls、plans/COMPASS 和长期规范；
10. 由独立 reviewer 做最终只读复核，确认代码已从 A 收敛到 B，而不是保留 A+B 双轨。

## 完成标准

- 最终 owner/permit/charge/retire 图可以从代码和实现文档独立复原；
- 所有生产路径不存在未登记的 raw allocation、单页特化或旧 ABI 旁路；
- 每个保留的阶段机制都有最终职责，不再靠“过渡”“以后删除”解释存在；
- 每个删除项已从代码、测试、注释、notes 和 plans 清除，或已转入唯一的另案计划；
- 内核、shared、rinlib、服务和验收负载均使用统一最终接口；
- 完整验证矩阵通过，且资源守恒、错误分类、锁阶和 Commit 后零分配均有可观察证据；
- Review 结论写入归档文档，后续不再重复把已删除的 C/D/E/F 引回主线。

## 当前观察登记

以下不是当前阶段的定论，只是未来 review 必须重新取证的候选观察点：

- `os/kernel/src/task/tunnel.rs` 已贯通多页 Tunnel、MemoryObjectCore/ObjectBacking 多 extent 投影与完整 lease 退役，rinlib owner/RNL2/现有消费者已同步；未来按最终提交范围复核是否还存在阶段性特化或重复 authority；
- `os/kernel/src/task/proc.rs` 当前存在 `BackingPlanFailure`、重复 Map validation 与 `backing_permits` 多层传递；需在最终 MemoryObject/backing planner 完成后判断哪些应统一；
- `frame.rs` 当前仍保留库存 selftest 的 raw `alloc_user_order` adapter；需在所有生产路径迁移后判断是否删除；
- 固定 split metadata 上界、验收 workload 分支和 Pool 守恒观测方式需结合最终并发/碎片模型重新证明；
- 1–5 与 6A–6D 的既有 review 计划只验证对应提交，不替代本次全局架构收口。

本计划不得提前执行局部重构；触发条件满足前，只能在相关实现计划中登记新的候选观察点和明确的语义缺口。
