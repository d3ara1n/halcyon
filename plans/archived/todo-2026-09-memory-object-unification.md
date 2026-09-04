# 公共 MemoryObject 与数据面统一（6E 剩余 + 切片 7）

> 【已收口】步骤 1–8 全部完成，见文末「收口记录」。未来审查见
> [`../todo-2026-09-review-program.md`](../todo-2026-09-review-program.md) 的批次 A。
>
> 【原实施计划】合并 6E 剩余项（ObjectBacking、投影统一、对象侧 metadata admission）与切片 7（公共 MemoryObject ABI/Handle/用户面），用最终形态一次设计到位，避免为单独闭合 6E 而造临时层。方向契约由 `notes/ideas/mm.md` 拥有；前序 6E 部分完成状态见 `plans/todo-2026-09-memory-object-data-plane.md` 切片 6E 段落与提交 `2e18c6e`。

## 背景与目标

切片 6E「资金化 backing 与多 extent 数据面基础」提交 `2e18c6e` 已完成：
- ✅ 匿名 `OwnedBacking` 全面资金化（Pool 取得、BackingSlicePermit 保活、多 extent、部分 Unmap 切分）
- ✅ Tunnel Connection 改用创建进程 Pool 付费（删除 raw `FrameTracker`）
- ✅ `alloc_user_*` 生产路径清理（只剩 selftest）
- ✅ Running/Building 共用深层资源原语（backing/table funding）

**但下列关键项未完成**，恰好是切片 7 的前置：
1. `ObjectBacking` 强类型不存在——Tunnel Connection 直接持 `FundedExtent`，未与匿名 `OwnedBacking` 强类型区分；
2. extent → bounded translations 投影未上收为匿名/对象共用：对象侧仍是单页单 PA adapter；
3. metadata admission 缺 ObjectBacking / ObjectView / Connection / Endpoint / Invitation 五类；
4. Running 与 Building 的 plan/complete 是两套平行近同构函数。

切片 7「公共 MemoryObject 与统一 ObjectView」要新增 MemoryObject kind/role、Create/Query/Seal ABI、`EXECUTABLE` ObjectSignals 位、rinlib affine wrapper，并扩展 Running `MemoryMap` 与 Building `ProcessMap` 的来源意图（Anonymous | MemoryObject）。

**合并理由**：6E 的三个缺口正是 7 的地基；拆开做会为 6E 造只服务匿名的临时投影层，再在 7 拆掉——正是要避免的脚手架沉淀。`memory_space` 里切片 7 的纯逻辑地基已存在（`MemoryObjectState` 完整状态机、`ObjectViewAuthorization`、`WritePermit`、`MapBacking::Object` 变体），缺的是对象壳、多 extent backing、共用投影和 ABI。

## 最终类型图与所有权图（冻结）

### 物理 backing storage

```
funded_frame::Funded<C,P,N>      已有：多 extent 物理 claims + 同源 charge
└── FundedBackingStorage         新：直接包住多 extent Funded
    ├── project(range) → bounded PA spans + translations
    ├── OwnedBackingSlice        匿名：可守恒 split/merge
    ├── ObjectBacking            对象：固定长度，不可分解
    └── BootstrapBacking         boot adopt：Commit 前即闭合的唯一 owner
```

- `funded_frame::Funded<C,P,N>` 保持不变（370 行，host 可测，12 项单测通过）；
- kernel 新增 `FundedBackingStorage`，持一个或多个 `Funded<FundedExtent, MemoryCharge, MAX_FUNDED_EXTENTS>`；
- 三个强类型 wrapper 共享 storage core，区别只在「是否可 split」与「谁持 metadata permits」；
- `project(range)` 输入逻辑 offset/length，输出有界 PA spans 与 translation intents；
- 删除当前 `FundedFrames::into_extents` 立即拆包为 `Vec<FundedExtent>` 的路径（`frame.rs:563`）。

### 对象 core 与 view

```
MemoryObjectCore                 新：统一拥有
├── ObjectId                     全局单调铸造（删除 Tunnel 私有 NEXT_MEMORY_OBJECT）
├── ObjectBacking                固定长度多 extent funded storage
├── MemoryObjectState            Mutable → Sealing → Executable 状态机
├── ObjectWaitState              EXECUTABLE 电平位 + 订阅队列
├── metadata permits             壳/backing/view permits + sponsor 强引用
└── 使用方
    ├── 公共 MemoryObject shell  Handle kind/role, Create/Query/Seal ABI
    └── Tunnel Connection        内部复用同一 core（本片统一）

ObjectView {                     affine，非 Copy
    object: Arc<MemoryObjectCore>,
    offset: usize,
    length: usize,
    permit: Option<WritePermit>
}

ViewLeaseOwner                   取代 Copy 的 ObjectMappingLease
├── LeaseKey                     稳定 ledger 身份
├── ObjectView                   强 owner
└── lifecycle state              Mapped | Retiring | Closed
```

- `MemoryObjectCore` 是 Tunnel Connection 与公共 MemoryObject 的统一基座；
- `memory_space::MemoryObjectState` 的 `seal_waiter: Option<u64>` 改为 kernel `ObjectWaitState` 电平位（删除专用槽）；
- `ObjectView` 强引用保活对象，Handle 先关闭不影响 view 存续；
- `ViewLeaseOwner` 不可复制，Endpoint 唯一持有；撤销时暂时移出 owner，Commit 后消费，失败返还。

### 统一事务核

一个 `AddressSpaceTransaction` core，五个维度参数化：

| 维度 | 变体 | 实际差异 |
|---|---|---|
| **source** | Anonymous \| Object | backing 来源：Pool funded / ObjectView 强引用 |
| **authority** | Process \| Builder \| Lease | execution gate / BuildingLease / lease owner |
| **lifecycle** | Running \| Building \| Reapable | shootdown / 尚未运行 / 已终止 |
| **completion** | Shootdown \| Immediate \| Detached | WaitContext / 同步完成 / ProcessDrain 接管 |
| **output** | Cookie \| HandlePublish \| None | pinned result / Handle 槽 / 无输出 |

当前 Running/Building/Object/Tunnel 四套近同构事务编排收敛到它。保留差异包括：authority 检查、Building `image_end` 推进、result cookie 原子发布、lease lifecycle 状态、shootdown vs immediate complete。

### 锁阶与分层

```
MEMORY_POOL (230) < MEMORY_OBJECT (250) < ADDRESS_SPACE (300)
```

- AddressSpace 锁内不得进入 Pool/MemoryObject；
- Pool charge reservation / ObjectView authorization 在 Reserve 阶段锁外取得；
- `memory_space` 纯逻辑层**不带**泛型 owner（最终形态已推翻该方向，见「已推翻的方案与推导」）；对象 view 授权在 `MapBacking::Object` 里、`Region.permit` 是写凭据、region 只存 id+offset。

## 阻断项与删除条件（reviewer 核对结果）

必须在本片完成（按依赖顺序）：

### A. 冻结对象基础设施

**A1. metadata admission 补五类**（`os/kernel/src/task/resources.rs`）
**A1. metadata admission 补五类**（`os/kernel/src/task/resources.rs`）—— ✅ 已完成（`6e18b8f`）
- 新增 `ObjectBackingPermit`、`ObjectViewPermit`、`ConnectionPermit`、`EndpointPermit`、`InvitationPermit`，各有全局/sponsor 上限、唯一 owner、退款终点。
- 实际上限：ObjectBacking 2048/64、ObjectView 8192/256、Connection 512/16、Endpoint 1024/32、Invitation 512/16。
- 启动 selftest 已扩到穿过五类的 acquire → 本地耗尽 → drop 重取退款。
- 剩余：`ObjectViewPermit` 尚无消费者。原计划待 C2 引入强 `ObjectView` owner 时接入，C2 已降为可延后项——改由步骤 8 公共对象 view 建立时接入（每个独立 view 一枚 permit），这是它的正当消费者。

**A2. 引入 `ObjectBacking`**（`os/kernel/src/frame.rs`）—— ✅ 已完成（`6e18b8f`，形态于 `51b3742` 修正）
- `ObjectBacking` 已建：固定长度、不可分解的对象数据 backing，不暴露 split/merge，落实三分中缺失的一分。
- `project(offset, length, &mut spans)` 输出有界物理 span 序列到调用方缓冲，是对象 view 组装 translation 的唯一几何入口。
- **形态修正**（`51b3742`）：中途引入的 `FundedBackingStorage`（内联定长 `MAX_FUNDED_EXTENTS` 槽）已删除——它把定长事务容器嵌进常驻对象，直接导致构造路径单帧 0x3540。`ObjectBacking` 改为堆化持 `Vec<FundedExtent>`，与匿名侧同构；`split_off`/`merge_from` 随之失去消费者一并删除。
- 已删：`fund_user_extent`（Tunnel 迁移后失去唯一调用者）、`FundedBackingStorage`。
- **保留（已重新归类）**：`FundedFrames::into_extents` 服务匿名与对象**两侧**的堆化转换——它**不是**过渡物，而是从定长 funding 结果转成堆化常驻 owner 的正当机制；`BackingExtentOwner::BootBorrowed` 与 `install_bootstrap_funding` 随匿名 backing 重构一并延后。

**A3. 建立 `MemoryObjectCore`**（`os/kernel/src/task/memory_object.rs`）—— ✅ 已完成（`6e18b8f` + `16dd3b4`）
- core 统一持：ObjectId（全局单调铸造）、`ObjectBacking`、`MemoryObjectState`、sponsor 强引用 + `ObjectBackingPermit`。
- Tunnel `Connection` 已改持 `Arc<MemoryObjectCore>`；Endpoint/Invitation 各持自己的 permit，attach 端由附着进程支付。
- 已删：tunnel.rs 的 `NEXT_MEMORY_OBJECT`/`mint_memory_object`、`fund_tunnel_backing`。
- **设计修正**：等待面（`ObjectWaitState`）**不入 core**——Tunnel 的等待面在 Endpoint、公共 MemoryObject 的在其公共 shell，二者各自拥有。因此 `MemoryObjectState::seal_waiter` 的删除与 `EXECUTABLE` 电平位接入属于 E（公共 ABI），不属于 A3。
- ~~栈窗口代价已付~~ **已收回**（`0fad27f`）：把定长 `MAX_FUNDED_EXTENTS` 容器嵌进常驻对象 core 才导致单帧 0x3540；改为堆化持 extent 列表后回落到 0x2390，STACK_GUARD 与审计上限随之退回 0x3000/0x2800。盒化整份 storage 曾实测更差（0x3a00，按值返回的事务结果先落栈再拷堆），正确解法是**不把定长容器放进常驻对象**而非盒化它。sifive_u formal 栈保持 0xF000——它约束调用链总和而非单帧，实测降回会 guard hit。

### B. 投影统一

**B1. 上收对象侧投影**（`os/kernel/src/task/proc.rs`）—— ✅ 已完成（`51b3742`），详见「步骤 4 已完成」节
原始现状记录：当前匿名侧 `OwnedBacking::preflight_install`（proc.rs:641）自己按 `Vec<BackingExtent>` 迭代生成 preflight；对象侧 `prepare_object_mapping`（proc.rs:2908）硬编码单页单 PA：`permits.len() == 1`、`object_bytes: PAGE_SIZE`、`offset: 0`、单个 `TranslationPreflight`。Tunnel 侧已改调 `core.backing.project(0, 1)`，但仍断言长度为一后只取 base 塑回单 PA（tunnel.rs `prepare_mapping`）。

目标是对象侧经 `project()` 得到有界 preflight 序列，单页退化为长度为一。匿名侧的 `preflight_install` 保持现状（其 `Vec<BackingExtent>` 遍历是堆化常驻表示的正当形态，见「已推翻的方案与推导」）。

待删：`prepare_object_mapping`/`complete_object_mapping`/`rollback_object_mapping*`/`commit_object_mapping` 的单 PA 假设与 `assert_eq!(permits.len(), 1)`；`MapBacking::Object` 的 `object_bytes` 参数（长度应从经认证的 view 取，而非调用方独立传入，reviewer B3）；tunnel.rs `prepare_mapping` 的单页断言。

### C. owner-aware ledger 事务 —— ⚠️ **整节已降为可延后项**（不服务切片 7，推导见「已推翻的方案与推导」）

**C1. Publish 时切分 anonymous backing owner**（`os/kernel/src/task/proc.rs`、`os/memory_space/src/space.rs`）
- 当前：`BoundAddressSpace.backings` 按 `BackingId` 保存可出现空洞的 `OwnedBacking`；切割后物理 owner 仍留原位，Remote ack 后按 offset 查找删除。
- 目标：prepare 阶段预留切分 metadata；Publish 原子产生 live 左右 slice owner 与 retiring slice owner；retiring owner 随 `RetiringChange` 保活。
- 删除：`retire_backing_one` 的运行时 offset 查找。
- 删除条件：ledger fragment 与物理 owner 结构性绑定，不靠 BackingId/offset 再关联。

**C2. object region 持强 `ObjectView` owner**（`os/memory_space/src/space.rs`）
- 当前：纯 planner 只存 `ObjectId/offset`，kernel 按 id 回查 owner。
- ~~目标~~（已推翻）：ledger 带泛型 owner、object region 直接持强 owner。推导见「已推翻的方案与推导」——公共对象不需要 ledger 泛型化。
- 删除：kernel 侧 object backing map（若引入过）；`BackingView::Object` 只存 id 的现状。
- ~~删除条件~~（已推翻）：31 项 planner 测试引入 stub owner。

### D. 统一事务核

**D1. 建立 `AddressSpaceTransaction` core**（`os/kernel/src/task/proc.rs`）
- 合并当前四套事务编排（Running Map、Building Map、Object Map/Unmap、Tunnel create/attach/close）为一个 core。
- 五个维度参数化：source / authority / lifecycle / completion / output。
- 删除：`UserMemoryPlan/PreparedUserMemory`、`OwnedMappingPlan`、`ObjectMappingPlan/PreparedObjectMapping`、`ObjectUnmapPlan/PreparedObjectUnmap` 四套类型；`plan_user_map/complete_user_memory` 与 `plan_owned_mapping/complete_owned_mapping` 重复函数。
- 删除条件：Running/Building/Object/Tunnel 都调同一 transaction core，plan/complete 只剩一套。

## 可延后项（不阻断本片）

按 reviewer 评级与触发条件：

- **切片 8**：Tunnel 外部多页 ABI、动态 create/attach 几何；但内部 ObjectBacking/projection 本片就是多 extent。
- **切片 8**：Tunnel close 当前"一 lease fragment/一 permit"外围状态可暂留，但公共 object retire core 本片不能继续单 permit。
- **切片 9**：RNL2 与动态 ring capacity。
- **切片 10**：raw FramePool inventory selftest adapter、启动 selftest 走内部 seam。
- **可延后**：匿名 backing 内部重构（`BackingExtentOwner` 三变体、手工 split+Vec 重排下沉到纯 crate、`identity` 二分 + offset 线性查找）。**不服务切片 7**，收益仅内部对称性。触发条件：匿名 backing 出现新能力需求（COW、部分 discard、pager），或结构收口 review 判定该重复真值已造成实际缺陷。完整推导见「已推翻的方案与推导」。
- **机会型**：沿路撞见的 `FundedRootFrame`/单变体 `TableFrameToken::Root`/`holds_write_permit` 死字段/`start_user_memory_change` 布尔参数/命名错位顺手改，改不到不阻塞。

## 实施顺序（8 步）

1. **补 metadata admission 五类**（resources.rs）—— ✅ `6e18b8f`
2. **建立 `ObjectBacking`**（frame.rs）—— ✅ `6e18b8f`（`FundedBackingStorage` 已于 `51b3742` 删除，对象 backing 改堆化持 extent 列表）
3. **建立 `MemoryObjectCore` 并迁移 Tunnel**（memory_object.rs、tunnel.rs）—— ✅ `6e18b8f` + `16dd3b4`
4. **对象侧投影上收**（proc.rs、frame.rs）—— ✅ `51b3742` + `0fad27f`，见下节
5. ~~owner-aware ledger / 匿名 backing 重构~~ —— **已降为可延后项**（不服务切片 7，推导见下节「已推翻的方案与推导」）
6. **统一事务核**（proc.rs）→ 删四套 plan/complete 类型与函数。
7. **迁移 Running/Building/Tunnel 调用点** → 全走统一事务核。
8. **开放公共 MemoryObject ABI**（shared、syscall、rinlib）→ Create/Query/Seal、`EXECUTABLE` 信号、用户态 affine owner。

每步先跑对应 host debug/release、`just check`；涉及启动后跑 `just virt`；涉及 Remote/drain 补 `just virt-stress`；收尾跑 `just acceptance`。

### 已完成

- **步骤 1–4 ✅**（见下方「已完成提交索引」与「步骤 4 已完成」节）
- 步骤 5 已降为可延后项；**下一步是步骤 6–8**，其中步骤 8 可独立先行。

## 步骤 4 已完成：对象侧投影上收（`51b3742` + `0fad27f`）

**本节替换了上一版冻结的「匿名 backing 迁 FundedBackingStorage + 消灭 offset 查找」方案。**
该方案在实施前的可行性验证中被自身推翻，推导与结论见下方「已推翻的方案与推导」。
方案不是真理，计划与文档中的结论同样允许推翻——记录推导过程比保留错误结论更有价值。

### 已交付

- `prepare_object_mapping` 接收 view 的对象内偏移与投影出的物理 span 序列，逐段
  `preflight_map`；`ObjectMappingPlan` 持 `Vec<TranslationPreflight>`，
  `complete_object_mapping` 循环 `prepare` 并经 `publish_batch` 发布，与匿名侧
  `complete_owned_mapping` 同形。表页改走 `fund_table_preflights`（每段独立预算）。
- 删 `MapBacking::Object` 的 `object_bytes`：长度真值随 `ObjectViewAuthorization`
  从对象状态机流出（`MemoryObjectState` 持 `object_bytes`），调用方不再另传一份。
- 删 `assert_eq!(permits.len(), 1)`（prepare/rollback 三处）与 `PAGE_SIZE` 硬编码；
  `ObjectMappingLease` 记 `object_offset`，`prepare_object_unmap` 据此复核而非假定 0。
- `ObjectBacking` 改为堆化持 `Vec<FundedExtent>`；删只服务对象的 `FundedBackingStorage`
  （`project` 逻辑内联进 `ObjectBacking`，`split_off`/`merge_from` 无消费者）。
- `MemoryObjectCore` 不再另存 `identity`——身份与长度同归 `MemoryObjectState`，单一真值点。
- 栈窗口回落：STACK_GUARD 0x4000→0x3000、审计上限 0x3800→0x2800、虚拟跨度 2.25→2.19MiB。
  sifive_u `STACK_SIZE` 保持 0x10000（约束调用链总和，非单帧，实测降回 0x9000 会 guard hit）。

### 删除条件核对（全部满足）

1. ✅ `prepare_object_mapping` 及四个 wrapper 不含 `PAGE_SIZE`/单 PA/单 permit 假设。
2. ✅ `MapBacking::Object` 无 `object_bytes`（全仓 grep 只剩 object.rs/space.rs 的正当处）。
3. ✅ Tunnel 单页作为长度为一的普通消费者，无专用断言。
4. ✅ 公共多页对象可在不改 `prepare_object_mapping` 的前提下映入——步骤 8 只加 ABI 与 Handle。

零 `warning`、零 `error`；host 全绿；virt core / virt-release / sifive_u / virt-stress 16/16 通过。

### 已推翻的方案与推导

上一版冻结了「`OwnedBacking` 从 `Vec<BackingExtent>` 迁到 `FundedBackingStorage`，并把 backing
切分移到 Commit 以消灭退役期查找」。实施前逐条验证代码，三处前提均不成立：

**（一）定长 64 槽不适合作常驻表示——对匿名与对象都不适合。**
`FundedBackingStorage` 内联 `[Option<ClaimedUserExtent>; MAX_FUNDED_EXTENTS=64]`（约 1.5KB）。
匿名侧现状 `Vec<BackingExtent>` 每个物理 extent 是堆上单槽 `Funded<...,1>` owner，切分在预留
容量的 Vec 里零分配重排——**已经是堆化形态**，`into_extents` 正是这个转换的机制，不是历史包袱。
更强的证据来自本次实测：把定长容器嵌进**对象** core（步骤 2 的做法）直接导致构造路径单帧
0x3540、上一提交不得不抬高 guard 与审计上限；改回堆化后最大帧回落到 0x2390，容量随之收回。
定长容器只适合做一次性事务结果（栈上短暂存在），常驻对象一律堆化。

**（二）「一 backing 一 storage」与中段 unmap 留洞矛盾。**
中段 unmap 后同一 `BackingId` 下 live 的是左右两段、中间是洞，而 `Funded` 逻辑连续、表达不了
洞。要消灭 `identity` 二分 + offset 线性查找，物理 owner 必须绑到 **ledger fragment**（洞由「无
fragment」天然表达），这需要 `reserve` 出口报告每个 replacement 新铸的 `RegionKey`（当前只暴露
map 的 `mapped_region_key()`），即改造纯 planner 的事务出口语义。

**（三）切分时机不能前移到 Commit。**
物理 owner 切分必须发生在不可能再 rollback 之后——若在 Reserve/Commit 期切，回滚需无损还原，而
`merge_from` 可失败，会把不可失败的回滚路径污染成可失败。现状把切分放在 **retire 阶段**（Commit
之后、确认之后），配合 Commit 前预留的 slice permit 与预留容量的 Vec 实现零分配重排——这是正确
设计，不是欠账。上一版「切分移到 Commit」的推导忽略了 rollback 无损性要求。

**（四）与「延迟即立案」的关系。**
匿名 backing 重构的收益是内部对称性（`BackingExtentOwner` 三变体、手工 split+Vec 重排下沉到纯
crate），**不服务任何已确认的外部语义**，也不解锁切片 7。按「大投入零需求不进主线」，它降级为可
延后项，触发条件是：匿名 backing 出现新的能力需求（如 COW、部分 discard、pager），或 reviewer
在结构收口 review 中判定该重复真值已实际造成缺陷。

### 下一步：步骤 6–8

对象侧真前置已就位，剩余为原定的统一事务核（步骤 6–7）与公共 ABI（步骤 8）。步骤 8 可独立于
6–7 进行——公共对象只需 Handle/ABI 与已有的多段投影路径，不依赖事务核统一。

## ABI 与用户态改动

必须同步的文件（explorer 测绘结果）：

**shared 侧**：
- `shared/src/object.rs`：`ObjectSignals::EXECUTABLE = 1<<5`，更新 `KNOWN` mask。
- `shared/src/call.rs`：新增 `MemoryObjectCreate = 0x55`、`MemoryObjectQuery = 0x56`、`MemoryObjectSeal = 0x57`。
- 新 `shared/src/memory_object.rs`：`MemoryObjectCreateRequest`、`MemoryObjectSnapshot`（64B，仿 `MemoryPoolSnapshot`）、`MemoryObjectSealRequest`。

**os 侧**：
- 新 `os/kernel/src/task/memory_object.rs`：`MemoryObjectCore`、公共对象 shell、Handle kind/role。
- `os/kernel/src/task/object.rs`：`ObjectKind::MemoryObject`、`HandleRole::MemoryObject`。
- `os/kernel/src/task/proc.rs`：`memory_map`/Building `map` 增 MemoryObject 来源意图分支。
- `os/kernel/src/syscall.rs`：3 个新 dispatch 分支。
- `os/memory_space/src/object.rs`：`MemoryObjectState::seal` 改接 `ObjectWaitState`，删除 `seal_waiter` 字段。

**user 侧**：
- 新 `user/rinlib/src/memory_object.rs`：affine owner wrapper（from_handle/into_handle/query/seal/Drop）。
- `user/rinlib/src/call.rs`：3 个 `sys_memory_object_*` 裸封装。
- `user/rinlib/src/mm.rs`：`map_object` 变体或 `MappedObjectRegion`。
- `user/frameworks/libprocess/src/lib.rs`：Building `ProcessMap` 的 MemoryObject 来源意图。

## 验证策略

按两闭包分批（6E 承诺）：

**第一闭包（匿名多 extent）**：
- host：funded_frame broker 已有 12 项。
- 内核启动自检：funded/Pool/AddressSpace 守恒断言。
- srv_init：扩展 `test_memory_mapping`，补跨 extent 部分 Unmap 左右 live 与 retiring middle 的 frame/charge/permit 守恒、调用线程消散、ProcessDrain 接管。
- 注：原列的「memory_space 新增 owner 泛型 stub」随 ledger 泛型化降为可延后项一并取消。

**第二闭包（Tunnel/对象付费）**：
- 当前 srv_init 已有 Tunnel close 后 Pool charge 下降断言（仅 core）。
- 补：Attach 失败零消费基线、ack 前 backing 不退款、Connection 最后引用消散后 Pool **与 FramePool 双基线**恢复。

**公共对象验证**：
- host：memory_space/object.rs 已有状态机/WritePermit 测试，补多 extent projection。
- srv_init 或 test_target：多 extent object 在不同 VA/权限映入多进程、Handle 先关仍可访问、部分 object Unmap offset 保持且 backing 不切分、rights 拒绝、seal 与 writable permit/remote retire 竞态、对象最终 frame/charge 守恒。

## 完成标准

- 所有 6E 阻断项已删除（`ObjectBacking` 存在、投影统一、admission 补齐、事务核收敛）；
- 公共 MemoryObject ABI/Handle/rinlib 接入，Create/Query/Seal 可达；
- `EXECUTABLE` 信号可经 WaitMany 等待；
- Running `MemoryMap` 与 Building `ProcessMap` 支持 Anonymous | MemoryObject 来源意图；
- Tunnel 内部持 `MemoryObjectCore` 与多 extent `ObjectBacking`（外部 ABI 仍单页）；
- 两个验证闭包通过；
- host debug/release、`just check`、`just virt`、`just virt-stress`、`just acceptance` 全绿；
- impls/mm.md、impls/memory-object.md、impls/tunnel.md 同步更新为实际状态（删除提前描述统一 seam 的不实叙述）；
- COMPASS 更新位置段、活跃计划表删除本项、生成带实际提交哈希的未来 review 计划。

## 收口记录（2026-09）

步骤 1–8 全部完成。步骤 5（owner-aware ledger / 匿名 backing 重构）在可行性验证中被推翻并降为可延后项，推导见上方「已推翻的方案与推导」。

| 步骤 | 提交 | 结果 |
|---|---|---|
| 1–4 | `6e18b8f`、`16dd3b4`、`51b3742`、`0fad27f`、`310d089` | admission 五类、ObjectBacking、MemoryObjectCore + Tunnel 迁移、对象映射多段投影 |
| 清场 | `d2ff81e` | 死类型/重复真值/身份铸造/错误边界 |
| 6–7 | `5c0bbb0` | 统一事务核，四套 plan/complete 与两份 Tunnel 回滚矩阵删除 |
| 8 | `d6a162c` | 公共 MemoryObject ABI、EXECUTABLE 电平、view owner、两段式 Unmap/Protect |

**与原计划的偏离**：

1. **步骤 8 不能独立先行**（原文档判断有误）。`prepare_object_mapping` 当时把 `current`/`maximum`/PTE flags 硬编码为 `ReadWrite`，且只支持 `FixedEmpty`、无结果槽；而公共对象 Map 需要 result cookie + placement + 对象来源三者同时成立，这三样分散在两条平行事务里。先做步骤 8 必然要在两套即将合并的事务上各补一遍对象来源，等 6–7 合并时全部返工。因此实际顺序是清场 → 6–7 → 8。
2. **D1 的五维参数化落地为字段而非类型参数**。source/authority/output/image_end/view 都是 `MemoryChangePlan` 的字段（`Option` 或小 enum），不是泛型维度——组合数少、失败路径需要统一处理，泛型只会把同一段回滚逻辑复制到每个实例。
3. **C2「object region 持强 ObjectView owner」以另一种形态成立**：owner 不进纯逻辑 planner（该方向已推翻），而是 AddressSpace 持 per-object view owner 表，「是否仍有区域引用」查账本。planner 保持无泛型 owner。
4. **新发现并修正一个真实前置缺口**（原计划未列）：含 W 的 object view 被部分 Unmap 或降权时，存活片段是新铸造区域、各需一枚新 `WritePermit`，而 permit 只能在 AddressSpace 锁外向对象取得。Unmap/Protect 因此改为两段式。这是公共对象接入后才可达的路径——Tunnel view 是整段撤销，从不切割。

**删除条件核对**：`UserMemoryPlan`/`PreparedUserMemory`、`OwnedMappingPlan`/`PreparedOwnedMapping`、`ObjectMappingPlan`/`PreparedObjectMapping`、`ObjectUnmapPlan`/`PreparedObjectUnmap` 八个类型与对应 plan/complete/rollback/commit 全部删除；全仓无 `prepare_object_mapping` 单 PA 假设、无 `ReadWrite` 硬编码、无 `seal_waiter` 单槽 waiter、无 `MemoryRetireSink::retire_permit`。`ObjectViewPermit` 的空缺消费者已由 view owner 补上。

**可延后项现状**：匿名 backing 内部重构仍未触发（触发条件不变：COW / 部分 discard / pager，或结构收口 review 判定重复真值已致缺陷）。切片 8（多页 Tunnel ABI）、9（RNL2）、10（raw FramePool selftest adapter）仍在 `todo-2026-09-memory-object-data-plane.md` 名下。
