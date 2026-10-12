# 平台保留内存生命周期设计 Review

## 审查状态与范围

本报告是第四批独立设计 Review 的只读产物。未修改生产代码、平台保留内存计划、`plans/COMPASS.md`、Review 收口计划或其他报告；本文件是本批指定范围内唯一新增文件。

审查对象：

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md` 当前完整设计；
- `plans/REVIEW.md` 的设计 Review 纪律；
- `plans/todo-2026-09-device-system-design.md` 的 D4/DMA 交界；
- `plans/todo-2026-10-01-memory-redesign.md` 当前 Sv39/MemoryPool 事实；
- `notes/impls/mm.md`、`notes/impls/internals.md`、`notes/impls/startup.md`；
- `references/CONTRACTS.md` 指向的 Devicetree Specification v0.4 及相关固定规范章节。

明确排除：具体设备/驱动 ABI、完整设备 D1-D6 重审、服务装配政策、全系统关机状态机、对象链已完成部分、前三批报告 findings，以及尚不存在的保留内存生产实现代码验收。

## 结论摘要

当前计划的方向与既有内存责任链一致：动态 reservation 必须在普通供给发布前整体放置，静态保留区不能压平成匿名排除区，`reusable` 不能伪装成普通 `MemoryPool` grant，且 DMA/翻译收束不能由进程退出或普通 VM retire 冒充。当前实现对动态 reservation 和 `reusable` 明确 fail closed，这一事实符合现阶段边界，不是本 Review 的代码缺陷。

但是，计划仍不能作为 D4/DMA 施工依据。关键问题不是缺少实现细节，而是若干会改变身份、owner、提交点和支持边界的决策仍留在“实施切片”或“决策门”中。尤其是动态放置的排序/放置策略、region identity 粒度、布局事务发布点、primordial authority、reusable reclaim 状态机、IOMMU/无 IOMMU 支持矩阵以及两个 DTS fixture 的可判定拒绝条件尚未闭合。

| 编号 | 严重度 | 性质 | 结论 | 建议归属 |
|---|---|---|---|---|
| RM-01 | P1 | 设计缺口 | 动态 reservation 的确定性排序、候选区间选择和容量上界没有被选定或推导，planner 尚不能据此形成稳定布局合同 | 本计划，动态布局 planner |
| RM-02 | P1 | 设计缺口 | DT region 输入模型没有闭合多 `reg`、动态 node、`compatible`、phandle 与 `memory-region(-names)` 的身份/引用粒度和未知属性政策 | 本计划，DT 纯逻辑模型；交接设备 D4 |
| RM-03 | P1 | 设计缺口 | 启动期整体布局事务没有明确唯一提交点、发布对象集合和所有失败/重入边界，无法证明 direct-map、FramePool、system supply、root Pool 不会看到部分结果 | 本计划，启动 admission 与 `memory_supply` 接点 |
| RM-04 | P1 | 设计缺口 | 普通永久区、系统储备、设备专用区、`no-map` 和 `reusable` 的 owner/capability 图及物理真值投影尚未落地 | 本计划，region authority 与内存账本 |
| RM-05 | P1 | 设计缺口 | `reusable` loan/reclaim 只有目标叙述，没有可执行的状态机、线性化点、既有 backing 收束和失败保留规则 | 本计划，reusable 机制；交接设备 D4/DMA |
| RM-06 | P1 | 设计缺口 | IOMMU 与无 IOMMU 的 DMA、缓存、翻译和设备移除边界没有形成支持/拒绝矩阵；owner 消散与超时也无唯一收束政策 | 本计划与设备 D4/D5 |
| RM-07 | P2 | 设计与证据缺口 | 实施切片、两个 DTS fixture、拒绝条件和完成标准没有客观验收谓词，无法判断何时允许进入设备 D4/DMA 施工 | 本计划，施工前置与验证门 |

这些 findings 都是设计尚未闭合的问题，不是对不存在的生产实现提出的代码 bug。

## Findings

### RM-01：动态放置策略和有界容量没有形成可推导合同

**性质：设计缺口。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「目标模型」；
- 同文件「实施切片」第 1、2 项；
- 同文件「决策门」中的“动态请求的确定性排序与放置策略”。

**可达或可推导路径**

1. `/reserved-memory` 子节点可以用 `size` 请求动态区间，并可同时指定 `alignment` 和多个 `alloc-ranges`；规范还允许 `reg` 属性包含任意数量的地址/长度对。
2. 启动 admission 需要把静态 `reg`、FDT reservation block、内核/启动 owner 和全部动态请求合并到同一物理布局。
3. 当前计划只要求“确定性结果”和“确定性排序”，但没有选择按 DTS 出现顺序、phandle、节点路径、请求大小/对齐，还是其他稳定键排序；也没有说明候选范围内部采用低地址优先、高地址优先、最佳适配或其他策略。
4. 没有给出动态 node、`alloc-ranges` tuple、静态 region 数和输出 extent 数的固定容量推导，无法判断 planner 的存储与工作上界，也无法判断“容量不足”是输入描述超限、候选空间不足还是 planner workspace 不足。

**违反的契约**

- 计划要求动态放置是启动期物理布局事务，结果必须稳定、失败整体原子；
- 项目施工规则要求进入实施前解决架构性选择，不能把“排序或回滚”保留为未选择候选；
- 完成条件要求每次推进有结构性上界，且供给容量有单一真值来源。

**影响**

同一 DTB 在不同遍历实现、节点重排或固定数组容量下可能得到不同物理布局；更严重时，planner 可能在一部分候选已经写入后才发现后续请求或 workspace 无法满足。设备引用看到的 region 几何、direct-map 洞、FramePool 供给和 root Pool 总额度将无法从输入唯一推导，后续测试也无法区分策略差异与实现错误。

**建议归属**

由本计划在编码前冻结：

- 动态请求的稳定排序键及同键的完整 tie-break；
- 每个请求在 `alloc-ranges` 中的候选生成、页对齐与 checked arithmetic 规则；
- 候选选择策略及“请求不可放置”的拒绝条件；
- 输入/中间/输出数组的容量公式，公式同时覆盖静态多 tuple、动态请求和 region 引用表；
- planner 单步工作界和失败时 workspace 的可复用语义。

该结论不要求某一种排序或放置策略，但要求选择结果能从 DT 输入、固定常量和已接纳平台边界直接推导。

### RM-02：region 输入模型与引用粒度没有闭合

**性质：设计缺口；外部规范事实见证据边界。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「外部契约」「目标模型」；
- 同文件「实施切片」第 1、3 项；
- Devicetree v0.4 `chapter3-devicenodes.rst`「/reserved-memory/ child nodes」和「Device node references to reserved memory」；
- Devicetree v0.4 `chapter2-devicetree-basics.rst`「phandle」。

**可达或可推导路径**

1. 一个 reserved-memory child 可能以多组 `reg` 描述静态区间；若同时有 `reg` 和 `size`，规范规定 `reg` 优先，`size` 被忽略。
2. 动态 node 的 `size`、`alignment`、`alloc-ranges` 使用 parent node 的 cell 宽度；`alloc-ranges` 本身也是多个 address/length tuple。
3. 设备通过 `memory-region` 的 phandle/specifier 数组引用 reserved-memory child，`memory-region-names` 按条目对应，但规范不替本项目定义设备服务如何将引用转成 capability。
4. 当前计划说“保留节点身份、phandle、compatible”和“为每个节点产生稳定 region identity”，却没有决定 identity 是按 node、按 `reg` tuple、按动态分配出的 extent，还是 node 下的一组 extent；同样没有决定一个 capability 是否覆盖整个 node、某个 named entry 或单个物理 extent。
5. `compatible` 的未知 vendor 字符串、额外标准属性、`memory-region-names` 数量不匹配、phandle 指向非 `/reserved-memory` 子节点或重复/悬空引用的处理也尚未写成拒绝或保留策略。

**违反的契约**

- 设备计划要求每个事实有唯一 owner，设备引用不能越过受托资源范围；
- 本计划要求保留 region identity，不把不同生命周期压平成无名区间；
- Devicetree 规范要求 phandle 在 DT 内唯一，并定义 `memory-region` 为指向 reserved-memory 子节点的引用，但不提供本项目的 capability 粒度。

**影响**

解析层即使成功，也可能无法判断一个设备引用取得的是整块 region、一个命名资源还是某个可分配 extent。多 `reg` region 的部分交付、共享引用、设备移除和 reclaim 将因此需要在 D4 临时决定身份和范围。若未知 `compatible` 或附加属性被静默接受，系统可能把不支持的 cache/DMA 约束误当成普通可映射区；若全部未知属性都拒绝，又没有记录支持边界，无法判断平台描述是否属于明确不支持。

**建议归属**

由本计划的 DT 纯逻辑层冻结：

- node identity、region identity、extent identity 和 capability identity 的关系；
- 多 `reg` node 的区域集合语义、是否允许跨 tuple 合并以及共享引用的稳定顺序；
- `reg` 优先于 `size` 的保留信息（包括被忽略 `size` 是否保留诊断）；
- `memory-region` 与 `memory-region-names` 的 cardinality、命名重复、悬空/错域 phandle 拒绝条件；
- `compatible` 的支持集合、未知 vendor 值的 opaque 保留或拒绝政策；
- 解析层只保存事实，还是在 admission 层拒绝无法证明映射/缓存/DMA 语义的 node。

设备 D4 只应消费已冻结的 region/capability 形状，不再决定 DT 引用的身份粒度。

### RM-03：启动期整体布局事务缺少唯一发布边界

**性质：设计缺口。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「目标模型」和「完成标准」；
- `notes/impls/mm.md`「启动物理供给与系统储备」；
- `notes/impls/startup.md`「唯一 init bootstrap」；
- `os/dtb/src/memory.rs` 当前静态解析边界；
- `os/memory_supply/src/lib.rs` `Plan` / `InventoryPlan` / `SystemSupply` 原子规划边界。

**可达或可推导路径**

1. 当前 `memory_supply` 已有固定 workspace、`Plan` 成功后才拆出 `InventoryPlan` 和 `SystemSupply` 的 fail-closed 形状；当前 `os/dtb` 也只返回完整的静态 `PlatformMemory`。
2. 未来动态 reservation 必须在 `FramePool`、direct-map、system tickets、root Pool 和 bootstrap boot-held owner 发布前落位。
3. 计划没有规定“布局事务”究竟包含哪些对象：DT 解析快照、region table、dynamic placement、静态排除区、direct-map admitted ranges、FramePool user-free、system ticket、root Pool seed、设备引用索引是否同属一次提交。
4. 也没有规定唯一提交点之前哪些输入可被覆盖重规划、提交后哪些发布只能不可失败完成，以及 direct-map 建表失败、系统储备不足、root Pool 铸造失败或启动 owner 与 region 重叠时由哪个 owner 保留未发布结果。

**违反的契约**

- 计划明确要求“不发布部分布局”；
- 当前内存实现的 `Plan` 语义要求完整成功后才发布可消费对象；
- 项目完成条件要求失败、取消、退出和释放路径守恒，且已提交责任不能依赖调用者继续推进。

**影响**

若 planner 成功但后续 direct-map 或 system ticket 接线失败，系统可能已经把某些 region 从普通供给扣除，而另一些发布对象仍反映旧布局。相反，若 direct-map 先发布、region capability 后发布，设备或启动 owner 可能看到未授权的映射。没有统一提交边界，也无法证明 `managed = permanent + boot-held + system + user-free` 与 region/extent 账本在同一个静止点成立。

**建议归属**

由本计划和 `memory_supply` 接点补出一个明确的 admission result：

- 输入快照、临时 planner workspace、region identity/capability 表和所有分类结果；
- 唯一 commit 点及 commit 前允许失败的操作；
- commit 后必须使用已预付对象完成的固定发布序列；
- direct-map admitted ranges、FramePool unavailable/user-free、system tickets、root Pool seed、boot-held owner 和 region owner 的发布顺序；
- 任一失败在 commit 前的原状恢复，以及 commit 后不可恢复失败的系统级 fail-stop/停驻政策；
- 任何重规划只能发生在未发布 workspace，不得读取已消费 ticket 反推新布局。

这项 finding 不否定现有 `memory_supply` 的局部原子性；它指出平台 region 引入后尚未定义跨 parser、planner、VM 启动和供给发布的整体事务。

### RM-04：分类、owner 和 capability 图没有证明单一物理真值

**性质：设计缺口。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「目标模型」；
- 同文件「实施切片」第 3、5 项；
- 同文件「决策门」中的“region capability 由哪个 primordial authority 接收”“设备服务如何取得引用”；
- `notes/impls/mm.md`「启动物理供给与系统储备」「MemoryPool 额度与 metadata 准入」「页表模式选择」；
- `notes/ideas/mm.md`「物理内存」「MemoryPool 与 backing charge」。

**可达或可推导路径**

1. 当前实现把 permanent、boot-held、system、user-free 分成不同 ticket/owner，并让普通 `MemoryPool` 额度来自 user supply；静态 reserved-memory 被排除普通供给，`no-map` 还从标准 direct-map admitted ranges 扣除。
2. 计划提出普通永久区、`no-map` 区、可复用区使用不同类型，但没有给出这些类型与 `PlatformMemory` region、FramePool unavailable、direct-map hole、system reserve、设备 capability 和 MemoryPool charge 的一一映射。
3. 也没有明确普通永久区是否允许标准内核 direct-map、设备专用区是否可以被 kernel 通过专用映射访问、`no-map` owner 如何建立映射、静态专用区由谁持有到设备移除或系统终局。
4. “region capability 由 primordial authority 接收”与“设备服务如何取得引用”仍是决策门，因此 capability 交付、设备引用和物理 owner 之间没有可达的责任转换点。

**违反的契约**

- 项目要求同一物理事实只有一个 owner 和一个真值来源；
- 普通 `MemoryPool` 是不可撤销 grant；需要强制收回的资源必须使用独立的可撤回 lease/resource domain；
- Devicetree v0.4 只要求 reserved memory 排除普通使用，`no-map` 进一步禁止标准映射和推测访问；它不替本项目定义 authority 或 charge。

**影响**

同一物理页可能在 region 表中仍属于设备、在 direct-map 中可被内核标准访问、在 FramePool 中被标为 unavailable，同时又被错误计入 MemoryPool 总额度或通过 raw frame 路径交付。即使这些路径暂时没有代码，设计缺少一张类型和 owner 矩阵会迫使实施者在接入点各自解释“保留”“映射”和“可借用”的含义。

**建议归属**

由本计划建立单一物理事实矩阵，至少逐类列出：

- 物理状态、唯一 owner、是否进入普通供给、是否进入 direct-map、是否可建立专用映射；
- region capability 的持有者、派生者、设备引用接收者、撤销/移除者和系统终局 owner；
- 普通永久区、系统储备、设备专用区、`no-map`、reusable region 的 charge 是否存在、由谁付款、何时可退款；
- `MemoryPool` grant 与 reusable loan 的类型级不可互换边界；
- raw frame、FramePool、MemoryPool、direct-map、IOMMU mapping 任何旁路被拒绝的入口。

这张矩阵应成为 D4 的输入，而不是由首个设备实现补充。

### RM-05：reusable loan/reclaim 没有可执行状态机和失败收束

**性质：设计缺口。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「目标模型」；
- 同文件「实施切片」第 4 项；
- 同文件「决策门」中的“reusable 借用允许哪些 backing 类别”“reclaim 的超时、失败与设备移除政策”；
- `notes/impls/mm.md`「动态 reservation 与 `reusable` fail closed」及用户地址空间 retire 语义；
- `notes/ideas/mm.md`「平台保留内存」与「Derive 是不可撤销的资源 grant」。

**可达或可推导路径**

1. region owner 把 reusable extent 借给允许的 backing 使用者。
2. region owner 请求 reclaim；此时可能有新的 loan 请求、既有 View、对象 backing、用户地址空间 PTE、正在执行的 CPU 访问或设备 DMA。
3. 计划只描述“停止新借用、收束或迁移既有内容、完成地址翻译与 DMA 同步、归还完整 extent”，没有定义这些动作的状态、线性化点、观察者、每步上界或 owner 转移。
4. 没有规定内容不可丢弃时如何判定“可迁移”、迁移失败时是否保留旧 backing、部分 extent 已收回时如何阻止再次借用，也没有规定 reclaim 与普通 VM `Unmap/Retire` 的关系。

**违反的契约**

- Devicetree v0.4 的 `reusable` 前提是设备 owner 能够收回，并通常只允许 volatile、可重建或可迁移内容；
- 当前 VM 的普通 MemoryPool grant 不可撤销，提交后的资源责任不因调用者消散而丢失；
- 项目要求取消、超时、失败和 owner 消散都有稳定执行根，不能由 Drop 或调用者重试隐式完成。

**影响**

实施者可能把 loan 当成普通 `MemoryCharge`，导致 reclaim 无法阻止新 use；也可能在迁移完成前撤销 View、释放物理页，或在超时后把 region 标记为可用而 DMA/旧翻译仍可达。另一种错误是无限等待调用者或设备报告，region owner 永不恢复完整 extent，造成平台储备泄漏。

**建议归属**

由本计划选定并写出 reusable 状态机，至少包含：

- `Available/Loaned/Revoking/Draining/Returned/Quarantined` 等状态的真实必要集合；
- 停止新借用的唯一线性化点；
- 每个 loan 的 backing 类别、可丢弃/可迁移证明和稳定 owner；
- CPU View/PTE、对象 backing、缓存和 DMA pin 的分层收束顺序；
- 正常 reclaim、取消、迁移失败、超时、设备移除和 owner 消散的责任接管；
- 完整 extent 归还前不得重新进入 `Available` 的不变量；
- reclaim 观察结果如何与普通 MemoryPool 的不可撤销语义隔离。

若某类 backing 或某个平台无法满足这些条件，应在 admission 时明确拒绝，而不是进入半支持状态。

### RM-06：IOMMU、无 IOMMU 和设备移除边界没有形成支持矩阵

**性质：设计缺口；不是当前 DMA 代码缺陷。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「目标模型」「决策门」「完成标准」；
- `plans/todo-2026-09-device-system-design.md` D4/DMA 接点和“保留内存与 DMA”；
- `notes/impls/mm.md` 整空间退出屏障中“设备/DMA use 为独立屏障”；
- Devicetree v0.4 `chapter2-devicetree-basics.rst`「dma-ranges」及 `chapter3-devicenodes.rst` reserved-memory 条款。

**可达或可推导路径**

1. 设备引用一个静态或 reusable region，驱动将其交给 DMA。
2. reclaim、设备移除或管理者消散发生时，系统需要停止设备、撤销或失效 DMA 地址域、处理缓存可见性，并确认设备不能再次写入旧 extent。
3. Devicetree 的 `dma-ranges` 只描述总线 DMA 地址空间到父地址空间的映射；`reserved-memory` 的 `no-map`/`reusable` 不证明 IOMMU 存在，也不证明设备已停止。
4. 当前计划只要求“有 IOMMU 与无 IOMMU 平台各自的 DMA 收束边界”，没有列出哪些能力在无 IOMMU 平台一律拒绝，哪些设备隔离/复位证据可以替代，缓存维护由谁执行，IOMMU invalidation 和设备完成 ack 如何进入 region reclaim。

**违反的契约**

- D4 要求固定平台规范证明停止或隔离何时成立，不能用维护 Work、VM fence 或进程 DONE 冒充硬件安全；
- 当前 VM 已明确 CPU fence 不替 DMA use 屏障；
- 外部 DT 规范提供地址描述，不提供 DMA 停止、IOMMU 失效、缓存一致性或热移除完成证明。

**影响**

无 IOMMU 时，系统若仅撤销 CPU mapping 就把页返还 FramePool，仍可能留下设备按物理地址访问旧页的路径；有 IOMMU 时，若只清软件表而未完成硬件翻译失效，旧 DMA mapping 仍可能有效。设备移除、管理者死亡或超时场景若没有保守隔离终态，系统会在“region 已归还”和“设备仍可访问”之间出现两个互斥真值。

**建议归属**

由本计划与设备 D4 共同形成逐平台/逐设备能力矩阵：

- DMA 地址域来源、`dma-ranges` 解释、IOMMU 存在性与隔离粒度；
- map/unmap、IOMMU invalidation、设备停止/复位、缓存 clean/invalidate 的可信完成证据；
- 无 IOMMU 时支持的最小静态专用区能力，以及 reusable loan 是否一律 `Unsupported`；
- 有 IOMMU 但设备不能证明停止时的 quarantine 规则；
- 设备移除、owner 消散、超时和复位失败时保留哪些 region、mapping、charge 和维护责任。

该矩阵完成前，不得把通用 `MemoryPool` 或 VM retire 的成功当作 DMA region 可回收证明。

### RM-07：fixture、拒绝条件和完成门不可客观判定

**性质：设计与证据缺口。**

**位置 / 章节**

- `plans/todo-2026-09-platform-reserved-memory-lifecycle.md`「实施切片」第 6 项；
- 同文件「决策门」；
- 同文件「完成标准」；
- `plans/REVIEW.md`「设计检查」和项目施工准入条件。

**可达或可推导路径**

1. 计划要求至少两个自备平台 DTS fixture，并列出静态/动态、多 `alloc-ranges`、对齐、容量不足、引用错误、`no-map`/`reusable` 互斥和 reclaim 竞态。
2. 但没有给出两个 fixture 的结构差异、固定 cell 宽度、FDT reservation block、静态/动态 region 数量、phandle 引用图或预期布局。
3. “验证通过”没有逐项谓词：没有说明动态布局的精确结果、tie-break、失败后哪些发布对象必须保持未发布、错误是 parse reject 还是 admission reject，也没有给出 reclaim 竞态的最终 owner 和可观察状态。
4. 完成标准使用“单一真值”“所有失败和收束路径守恒”等正确目标，但没有绑定到可核对的日志/host 断言/平台证据，因此不能作为进入 D4/DMA 的闸门。

**违反的契约**

- 独立 Review 要求 finding 可由路径和证据边界推导；
- 项目完成条件要求验证结果与未覆盖范围明确，延期能力必须有触发、接通和验证标准；
- 设备施工准入要求选定能力没有未决正确性前提，且首批平台事实和拒绝条件有依据。

**影响**

实现者可以选择两个结构近似的 DTS、只验证成功布局，或把 reclaim 超时直接当成失败通过，从而在没有证明设备安全的情况下宣称专题完成。之后 D4 只能从测试输出反推 DT identity、region owner 和 DMA 边界，无法区分平台不支持、解析拒绝、布局不足和实现回归。

**建议归属**

由本计划在进入施工前固定：

- 两个结构实质不同的 fixture（例如不同 cell 宽度/多 RAM gap、静态与动态混合、共享 phandle 与多 `alloc-ranges`）及其预期规范化布局；
- 每个 fixture 的成功/拒绝 oracle、布局 hash 或完整区间快照、账本等式和 region identity；
- overflow、zero/invalid alignment、empty/out-of-range `alloc-ranges`、重叠、悬空 phandle、名称 cardinality、互斥属性和容量不足的分类；
- reclaim 竞态中的终态谓词：新借用被拒、旧内容已丢弃/迁移、DMA barrier 已证实或 region 进入 quarantine，且完整 extent 未提前返还；
- host planner、DT parser、启动组合和真实平台验证分别覆盖什么，哪些能力明确未覆盖。

在这些 oracle 固定前，报告不能把“有两个 fixture”视为完成证据。

## 当前事实与证据边界

### 外部规范事实

- Devicetree v0.4 规定 `/reserved-memory` 子节点可用 `reg` 指定静态区间，或用 `size` 请求动态区间；两者并存时 `reg` 优先，动态区间可由 `alignment` 和 `alloc-ranges` 约束。
- `no-map` 要求系统不得把该区纳入标准系统内存映射或允许无设备驱动控制的推测访问；`reusable` 允许 OS 临时使用，但设备 owner 必须能够收回，通常只适合可丢弃、可重建或可迁移内容；二者互斥。
- `memory-region` 是指向 `/reserved-memory` 子节点的 phandle/specifier 数组，`memory-region-names` 与其条目对应。phandle 在整个 DT 内唯一；这些规范事实不定义本项目的 capability 派生、DMA 停止或 reclaim 状态机。
- `dma-ranges` 描述总线 DMA 地址空间与父地址空间的映射，不等于 IOMMU 存在性、设备停止证明或缓存同步协议。

### 当前内存实现事实

- `os/dtb::memory::parse` 当前接纳页对齐、checked 的 `/memory`、FDT reservation block 和静态 `/reserved-memory/reg`；对 `no-map` 做普通供给排除与 direct-map 禁止映射的子集记录。
- 当前 `os/dtb` 对动态 `size` reservation 返回 `UnsupportedDynamicReservation`，对 `reusable` 返回 `UnsupportedReusable`；没有现成的 dynamic placement、region capability、reusable loan/reclaim 或 DMA 收束生产实现。
- `os/memory_supply` 的 `Plan` 只有完整规划成功后才拆出 `InventoryPlan` 与 `SystemSupply`，失败 workspace 可被覆盖重规划；这证明现有供给 planner 的局部 fail-closed 形状，不证明平台 region 跨组件整体事务已经存在。
- `notes/impls/mm.md` 当前 Sv39/FramePool/MemoryPool/VM 责任链不包含 DMA use、IOMMU mapping 或可撤销 region loan。`MemoryPool` 的普通 grant 仍是不可撤销额度。
- `notes/impls/startup.md` 的 bootstrap 保留 boot-held/package owner，并按已有 owner 机制收编或回投；它没有接入 reserved-memory dynamic/reusable region authority。

以上事实不能被写成“现有代码遗漏了已承诺能力”。在当前专题尚无生产实现的前提下，动态/reusable fail closed 是明确边界；本 Review 的 P1 来自目标设计尚未提供施工所需的唯一决策和责任链。

## 进入 D4/DMA 施工前的唯一决策门

本报告建议把以下内容作为本专题进入设备 D4/DMA 施工前的单一门，而不是分散在多个实施切片中：

1. 冻结 DT 输入模型、region/extent/capability identity 粒度、未知属性和引用拒绝政策。
2. 冻结动态 placement 排序、候选选择、容量公式、checked arithmetic 和完整失败原子。
3. 冻结整体 layout admission 的 commit 点及 direct-map、FramePool、system supply、root Pool、boot-held 和 region owner 的发布顺序。
4. 冻结五类以上物理身份（普通永久、system reserve、设备专用、`no-map`、reusable）与 direct-map/FramePool/MemoryPool 的唯一真值矩阵。
5. 冻结 primordial authority、设备 capability 交付、普通不可撤销 grant 与 reusable loan 的不可互换类型边界。
6. 冻结 reusable reclaim 状态机、迁移/丢弃条件、既有 View 与 backing 收束、owner 消散/设备移除/超时失败的 quarantine 政策。
7. 冻结 IOMMU 与无 IOMMU 的 DMA、缓存、翻译失效和硬件停止证据矩阵，并明确不支持能力。
8. 固定两个结构不同的 DTS fixture 及成功/拒绝 oracle、账本断言和 reclaim 竞态终态。

在上述门闭合前，允许继续做纯逻辑取证和 fail-closed parser/planner 设计，但不应开始设备 DMA ABI、region capability 交付或 reusable loan 的生产接线。

## 未覆盖边界

本报告没有重审设备 D1-D6 的完整身份、IRQ、供应、服务发布或关机状态机；只指出它们消费 region/DMA 决策的位置。没有审查具体驱动是否正确编程寄存器、具体 IOMMU 硬件的实现细节、具体缓存一致性平台、热插拔协议或外部设备 ABI。没有运行构建、host 测试、QEMU 或硬件验收，因为审查对象是尚未进入生产实现的完整设计；fixture 和验证门本身的缺失已在 RM-07 登记。
