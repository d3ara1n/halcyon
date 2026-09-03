# 公共 MemoryObject 与数据面统一（6E 剩余 + 切片 7）

> 【当前实施计划】合并 6E 剩余项（ObjectBacking、投影统一、对象侧 metadata admission）与切片 7（公共 MemoryObject ABI/Handle/用户面），用最终形态一次设计到位，避免为单独闭合 6E 而造临时层。方向契约由 `notes/ideas/mm.md` 拥有；前序 6E 部分完成状态见 `plans/todo-2026-09-memory-object-data-plane.md` 切片 6E 段落与提交 `2e18c6e`。

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
- `memory_space` 纯逻辑层带泛型 owner 参数，内核实例化为 `Arc<MemoryObjectCore>`，host 测试用轻量 stub。

## 阻断项与删除条件（reviewer 核对结果）

必须在本片完成（按依赖顺序）：

### A. 冻结对象基础设施

**A1. metadata admission 补五类**（`os/kernel/src/task/resources.rs`）
**A1. metadata admission 补五类**（`os/kernel/src/task/resources.rs`）—— ✅ 已完成（`6e18b8f`）
- 新增 `ObjectBackingPermit`、`ObjectViewPermit`、`ConnectionPermit`、`EndpointPermit`、`InvitationPermit`，各有全局/sponsor 上限、唯一 owner、退款终点。
- 实际上限：ObjectBacking 2048/64、ObjectView 8192/256、Connection 512/16、Endpoint 1024/32、Invitation 512/16。
- 启动 selftest 已扩到穿过五类的 acquire → 本地耗尽 → drop 重取退款。
- 剩余：`ObjectViewPermit` 尚无消费者，待 C2 引入强 `ObjectView` owner 时接入。

**A2. 引入 `FundedBackingStorage` 与 `ObjectBacking`**（`os/kernel/src/frame.rs`）—— ✅ 已完成（`6e18b8f`）
- `FundedBackingStorage` 已建：多 extent `Funded` 的唯一持有者，提供 `project(offset, length)` 输出有界物理 span 序列、`split_off`/`merge_from` 守恒变换。
- `ObjectBacking` 已建：包装同一 storage 但不暂露 split/merge，落实三分中缺失的一分。
- 已删：`fund_user_extent`（Tunnel 迁移后失去唯一调用者）。
- **未删（转入 C1）**：`FundedFrames::into_extents` 仍服务 `OwnedBacking`；`BackingExtentOwner::BootBorrowed` 与 `install_bootstrap_funding` 仍在。它们的删除条件是 `OwnedBacking` 迁到 `FundedBackingStorage`，属于 C1。

**A3. 建立 `MemoryObjectCore`**（`os/kernel/src/task/memory_object.rs`）—— ✅ 已完成（`6e18b8f` + `16dd3b4`）
- core 统一持：ObjectId（全局单调铸造）、`ObjectBacking`、`MemoryObjectState`、sponsor 强引用 + `ObjectBackingPermit`。
- Tunnel `Connection` 已改持 `Arc<MemoryObjectCore>`；Endpoint/Invitation 各持自己的 permit，attach 端由附着进程支付。
- 已删：tunnel.rs 的 `NEXT_MEMORY_OBJECT`/`mint_memory_object`、`fund_tunnel_backing`。
- **设计修正**：等待面（`ObjectWaitState`）**不入 core**——Tunnel 的等待面在 Endpoint、公共 MemoryObject 的在其公共 shell，二者各自拥有。因此 `MemoryObjectState::seal_waiter` 的删除与 `EXECUTABLE` 电平位接入属于 E（公共 ABI），不属于 A3。
- **栈窗口代价已付**：构造 core 的路径单帧 0x3540（`MAX_FUNDED_EXTENTS` 槽按值经事务返回，栈上展开一份）。STACK_GUARD 0x3000→0x4000、审计上限 0x2800→0x3800、sifive_u formal 栈 0x9000→0xF000。盒化 storage 实测反而抬到 0x3a00（按值返回的事务结果仍先落栈再拷入堆），已放弃并将结论记入类型文档。

### B. 投影统一

**B1. 上收 extent → bounded translations 投影为匿名/对象共用**（`os/kernel/src/task/proc.rs`）
当前匿名侧 `OwnedBacking::preflight_install`（proc.rs:641）自己按 `Vec<BackingExtent>` 迭代生成 preflight；对象侧 `prepare_object_mapping`（proc.rs:2908）硬编码单页单 PA：`permits.len() == 1`、`object_bytes: PAGE_SIZE`、`offset: 0`、单个 `TranslationPreflight`。Tunnel 侧已改调 `core.backing.project(0, 1)`，但仍断言长度为一后只取 base 塑回单 PA（tunnel.rs `prepare_mapping`）。

目标是两侧都经同一投影得到有界 preflight 序列，单页退化为长度为一。具体到哪一层取决于与 C1 的合并程度，见下方「下一任务」。

待删：`prepare_object_mapping`/`complete_object_mapping`/`rollback_object_mapping*`/`commit_object_mapping` 的单 PA 假设与 `assert_eq!(permits.len(), 1)`；`MapBacking::Object` 的 `object_bytes` 参数（长度应从经认证的 view 取，而非调用方独立传入，reviewer B3）；tunnel.rs `prepare_mapping` 的单页断言。

### C. owner-aware ledger 事务

**C1. Publish 时切分 anonymous backing owner**（`os/kernel/src/task/proc.rs`、`os/memory_space/src/space.rs`）
- 当前：`BoundAddressSpace.backings` 按 `BackingId` 保存可出现空洞的 `OwnedBacking`；切割后物理 owner 仍留原位，Remote ack 后按 offset 查找删除。
- 目标：prepare 阶段预留切分 metadata；Publish 原子产生 live 左右 slice owner 与 retiring slice owner；retiring owner 随 `RetiringChange` 保活。
- 删除：`retire_backing_one` 的运行时 offset 查找。
- 删除条件：ledger fragment 与物理 owner 结构性绑定，不靠 BackingId/offset 再关联。

**C2. object region 持强 `ObjectView` owner**（`os/memory_space/src/space.rs`）
- 当前：纯 planner 只存 `ObjectId/offset`，kernel 按 id 回查 owner。
- 目标：ledger 带泛型 owner 参数（内核实例化为 `Arc<MemoryObjectCore>`），object region 直接持 `ObjectView`。
- 删除：kernel 侧 object backing map（若引入过）；`BackingView::Object` 只存 id 的现状。
- 删除条件：`memory_space` 全面泛型化（space.rs:1819 行），31 项 planner 测试引入 stub owner 通过。

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
- **机会型**：沿路撞见的 `FundedRootFrame`/单变体 `TableFrameToken::Root`/`holds_write_permit` 死字段/`start_user_memory_change` 布尔参数/命名错位顺手改，改不到不阻塞。

## 实施顺序（8 步）

1. **补 metadata admission 五类**（resources.rs）—— ✅ `6e18b8f`
2. **建立 `FundedBackingStorage` 与 `ObjectBacking`**（frame.rs）—— ✅ `6e18b8f`
3. **建立 `MemoryObjectCore` 并迁移 Tunnel**（memory_object.rs、tunnel.rs）—— ✅ `6e18b8f` + `16dd3b4`
4. **上收 bounded projection + owner-aware backing**（proc.rs、frame.rs）—— ⏸ 下一任务，**与步骤 5 合并为一次最终形态重构**，见下节
5. ~~owner-aware ledger~~ —— 已并入步骤 4（同一批 backing 代码，不拆中间态）
6. **统一事务核**（proc.rs）→ 删四套 plan/complete 类型与函数。
7. **迁移 Running/Building/Tunnel 调用点** → 全走统一事务核。
8. **开放公共 MemoryObject ABI**（shared、syscall、rinlib）→ Create/Query/Seal、`EXECUTABLE` 信号、用户态 affine owner。

每步先跑对应 host debug/release、`just check`；涉及启动后跑 `just virt`；涉及 Remote/drain 补 `just virt-stress`；收尾跑 `just acceptance`。

### 已完成

- **步骤 1–3 ✅**（见下方「已完成提交索引」）
- 步骤 4+5 合并为一次最终形态重构，见「下一任务」节。

## 下一任务：最终形态设计（已按用户指示改为一次到位）

用户指示：操作同一批代码的，按最终视图一次实施，不做「先 A 再 B」的中间态。本计划据此从上一会话的 A/B/C 三选项收敛为单一最终视图，一次重构到位。

### 最终视图：匿名 backing 与对象 backing 同构

**（一）匿名 backing 直接持 `FundedBackingStorage`（不再持 `Vec<BackingExtent>`）**

- `OwnedBacking` 从 `{ identity, pages, extents: Vec<BackingExtent> }` 改为 `{ identity: BackingId, storage: FundedBackingStorage }`。
- 消灭：`BackingExtentOwner`、`BackingExtent`、`PreparedBacking`（退化为 funding 结果）、`OwnedBacking::extents`、`preflight_install` 的手工遍历、`write_from_start` 的手工遍历、`release_one` 的手工 split+Vec 重排、`install_bootstrap_funding` 的 `BootBorrowed` 替换。
- `FundedBackingStorage` 已有能力：`pages()`、`extents()`、`project(offset,length)`（输出有界 span 序列）、`split_off`（逻辑前缀/后缀切分，物理+charge 同步）。
- **结构约束（关键设计决定）**：一个 `OwnedBacking` = 一个 ledger fragment 的 backing。切分时用 `storage.split_off(left_pages)` 一次切出「左 live 半 + 右（retiring/live）半」两个 storage，不再保留中间状态。
- **为什么这就满足 C1 的删除条件**：ledger 的每个 `RetiringFragment` 现在在 Reserve/Publish 阶段携带它对应的那个 storage（连同物理 owner 与 charge），retire 路径直接析构它，不再靠 `BackingId + offset` 在 `backings` 里运行时查找（`retire_backing_one` 的 `binary_search_by_key` + `release_one` 的线性 `position` 全部消失）。

**（二）对象 backing 复用同一 storage，view 只持授权快照不持物理 owner**

- `ObjectBacking` 已存在（frame.rs），持 `FundedBackingStorage`、不暴露 split/merge，对象自身唯一拥有 backing。
- 对象 region 在 ledger 里继续只存 `ObjectId + offset`（现状不变，view 的切割不切数据 backing——ideas/mm.md L84 已冻结）。
- 与 (一) 的对称性：匿名用 `OwnedBacking{identity, storage}`，对象把 storage 收在 `MemoryObjectCore` 的 `ObjectBacking` 里。两种来源都经 `FundedBackingStorage`，但投影（`project`）只发生在 map 的 preflight 阶段（对象 view 建立时从对象 backing 投影、匿名从匿名 backing 投影），切分只发生在匿名（对象 backing 从不切）。

**（三）projection 上收：单一有界 preflight 组装路径**

- `FundedBackingStorage::project(offset, length)` 是**唯一**的几何→物理展开入口（已存在）。匿名 `preflight_install` 与对象 map 的 preflight 组装都调用它。
- 删除：`MapBacking::Object` 的 `object_bytes` 字段（view 越界在 validate 由 authorized 快照与对象几何核对，不靠调用方独立传入）；对象 map 的 `PAGE_SIZE` 硬编码（多页 view 直接经 authorized 长度投影）；`prepare_object_mapping` 的单 PA 假设与 `assert_eq!(permits.len(), 1)`。

**（四）匿名/对象在 ledger 中的切分差异只体现在 fragment 携带物上**

- **匿名**：切割消费原 storage 的一个子段 → 产出一个 retiring `FundedBackingStorage` 交给 retire 路径析构（物理+charge 守恒由 `split_off` 保证）。
- **对象**：切割只改 `BackingView::offset` 与 RegionKey，不产生物理 owner（数据 backing 留在对象）。
- 两者共享同一 region 切割 / `RegionKey` 铸造 / `WritePermit` 流转 / commit-publish-synchronize-retire 阶段机；差异只在「retiring fragment 是否携带一个待析构的 storage」。

**（五）Boot 路径**

- 去掉 `BootBorrowed` 投影后，`map_bootstrap_block` 在 Planning/Commit 前把 payload 物理并入 backing 的 storage：prefix 用匿名 funding 路径出资，payload 借外层 `BootFundedExtent` 几何先做可失败 map，成功后在无分配点上把 payload 并入同 storage（**合并而非替换**，因为此时 backing 已经是完整 storage 而不是 Vec 了）。
- 删除：`install_bootstrap_funding`（其职责并入 map_bootstrap_block 的收尾——不需要在 `backings.last_mut()` 里找 extent 替换了，因为 backing 在 map 时就已带着完整 storage）。
- 若要保留 BootBorrowed 的「投影后安装」模式则 storage 需能表示「只读借用子段」，而 `FundedBackingStorage` 内固定数组装不下借用视图——所以改为把 payload 在 map 时就并入 backing storage 的合并式，boot 物理在 `launch_bootstrap` 外层继续持 `BootFundedExtent` 直到并入。

**（六）消灭 `FundedExtent` / `FundedFrames` / `into_extents` / `fund_user_frames` 中间层**

- 现状：`fund_owned_backing` 拿 `FundedFrames` 后 `into_extents` 摊平成 `Vec<FundedExtent>`，每个再包 `BackingExtentOwner::Funded`。
- 最终：`fund_owned_backing` 直接返回 `FundedBackingStorage`（funding 结果不用拆散）；`FundedExtent`、`FundedFrames`、`into_extents`、`fund_user_frames` 全部删除（funded_selftest 改走 `fund_backing_storage`）。

### 删除条件（完成后全绿才是收口）

1. `BackingExtentOwner`、`BackingExtent`、`FundedFrames`、`FundedExtent`、`into_extents`、`fund_user_frames`、`BackingRetireCursor`、`install_bootstrap_funding`、`preflight_install`、`release_one`、`BackingPlanFailure::Owned` 不再存在。
2. 匿名 retire 不再按 `BackingId+offset` 运行时查找；retiring storage 随 fragment 结构性携带。
3. `preflight` 组装只有一条经 `project()` 的路径。
4. 单一 `OwnedBacking{identity, storage}`，ledger 每 fragment 与 storage 一一对应。

### 风险与待验证

- `project()` 目前返回惰性迭代器（借 self）；切分后多段投影需要收集成 Vec 或其它形式——preflight 路径本来就是 Vec，问题不大；但**惰性迭代器持有 storage 借用**会阻碍「切出 retiring storage 后立即析构」的移动语义，可能要先把投影收集成 owned Vec 或让 `project` 输出 owned `(base,pages)` 序列。
- `merge_from` 用于 boot payload 并入，但失败会保持双方——并入点需要可失败处理。
- `split_off` 失败（非法切分）——release_one 的切分总是内部合法（范围已由 ledger 保证），用 expect 即可。
- **帧预算**：把 `FundedBackingStorage`（64 槽）直接放进 `BoundAddressSpace.backings` 的 `OwnedBacking`，每个 active 匿名映射都内联一份 64 槽数组——已确认单 mapping 只持一份 storage（commit 时把整份放进去），不放大。但地址空间固定预算：`REGION_SLOTS_PER_ADDRESS_SPACE = 4096`，每 backing 上限 64 物理 extent；若每 fragment 持一份 storage，则「fragment 数 × 每 fragment 物理 extent 数」需要受同一 region_slot 预算约束——匿名映射每 fragment 一个 backing，这个不变式必须守住。
- **sifive_u 栈**：`map_bootstrap_block` 现在要把 payload 并入 backing storage（多一步 split/merge 在栈上），单帧可能逼近 `new_tunnel_connection` 的 0x3540，需重估 sifive_u formal 栈（现 0x10000 有 36KB→60KB 余量，够）。

### 本次会话的探察结论（已并入上述最终视图）

1. **`OwnedBacking` 的 release_one 是「物理+charge 守恒切分」的唯一真值**：一次 unmap 中段切出「左 live / 中 retiring / 右 live」三段，左右留在 `backings` 的 Vec 里、中间返回给 retire 路径。
2. **匿名与对象在 backing 上的差异已收敛为「切不切」**：匿名切物理（unmap 中段归库存），对象永不切（backing 在对象）。
3. **ledger 已为「fragment 携带 retiring storage」铺好路**：`PreparedPlan.retiring: Vec<RetiringFragment>`、`PreparedChange`、`CommittedChange/Published/Synchronized/RetiringChange` 全程持 `RetiringFragment`；`RetireBatch` 在 `begin_retire` 从 Synchronized payload `mem::take` 出来给调用方逐批 pop。匿名 retire 只要把 fragment 关联的物理 owner（storage）放进 `RetiringFragment` 即可——当前匿名 fragment 不带物理 owner，是因为物理 owner 在 `backings` 里按 id 查。
4. **boot 的 `BootBorrowed` 是非真 owner 的临时投影**：物理所有权由 `launch_bootstrap` 外层 `payload_funded: BootFundedExtent` 覆盖，map 后由 `install_bootstrap_funding` 原位替换成真 `Boot` owner。改成「并入 backing storage」是合并式，物理在 `launch_bootstrap` 外层直到并入。
5. **对象侧不用泛型**：对象 view 的授权（ObjectViewAuthorization）在 `MapBacking::Object` 里、`Region.permit: Option<WritePermit>` 是可写对象映射的 affine 凭据；region 只存 id+offset 不存物理。要让匿名/对象共用一套 backing 切分，对象侧不需要 ledger 泛型——差异在「匿名 fragment 多携带一个 storage，对象不」。

### 设计验证（收口时全绿）

- host：`memory_space` 19 项 + `funded_frame` 12 项 + 新增的匿名 backing 切分 host 用例（如果 storage 切分逻辑抽到可 host 测的 crate）。
- 内核启动自检：funded/Pool/AddressSpace 守恒断言。
- `just check` + `just virt`（core 锚点）+ `just virt-stress`（Remote/drain/竞态）+ `just virt-release`（优化代码生成）+ `just acceptance`（含 sifive_u，验证新栈帧）。

### 已定决策：retiring storage 走内核事务 token，**不**泛型化 ledger

原计划 C2 写的「ledger 带泛型 owner 参数」本次**不做**。理由从需求独立推导：

- `memory_space` 是 `no_std` + `forbid(unsafe_code)` 的纯 planner（space.rs 1819 行、无任何现有泛型），`RegionTemplate`/`BackingView` 是 `Copy` 并按值穿过 `templates_compatible`/`normalize_templates`；引入非-Copy owner 会迫使整条模板合并链改写。付这个代价应该换来真实收益，而它换不来——见下条。
- 真正要消灭的不是「ledger 不持 owner」，而是「**退役时才去找** owner」：`retire_backing_one` 的 `binary_search_by_key(identity)` + `release_one` 的线性 offset 重叠查找。只要 owner 在**切分发生的那一刻**就跟事务走，查找就不存在了——而事务 token 是内核侧的（`PublishedSpaceChange`/`RetiringSpaceChange`），它本就能持内核类型。
- `ideas/mm.md` L72 的原文是「解除的中段**转入事务 retire 所有权**」——归事务，不是归 planner。本方案正面满足该句；C2 的泛型化反而是过度读解。

**切分时机：Commit**。Commit 是不可逆线化点、零分配、无可恢复失败——`FundedBackingStorage::split_off` 正好零分配（定长数组内重排），且在边界已由 ledger 预先验证、slice permit 已在 Commit 前预留的前提下不可失败。因此：

- **Commit 前**：storage 完整留在 `backings`，rollback 零接触——不需要「把切过的合回去」的逆操作（若在 Reserve 阶段切，rollback 就得调 `merge_from`，而它可失败，会把不可失败的回滚路径污染成可失败）。
- **Commit 时**：一次原子切出三段——`let mut rest = storage.split_off(mid_start)`（self 成 live 左），`let right = rest.split_off(mid_len)`（rest 成 retiring 中段）。边界退化（mid_start==0 或 mid 到尾）则相应跳过一次 split。live 左/右各自成为一个新 ledger fragment 的 backing（回 `backings`），retiring 中段进 `PublishedSpaceChange`。
- **Commit 后**：`RetiringSpaceChange` 直接析构手上的 retiring storage，没有任何查找；`BackingRetireCursor` 整个删除。

**附带的预算简化**：当前 `MAX_BACKING_SPLITS_PER_CHANGE = MAX_FUNDED_EXTENTS * 4`（按每 extent 可能被切估）。改为 storage 级切分后，每个被覆盖 fragment 最多两次 `split_off`、每次最多把一个跨界 extent 切成两个，故预算为 `2 × 被覆盖匿名 fragment 数`，与物理 extent 数无关。预算常量应同步重推并写成可读公式，不照搬旧值。

### 实施顺序（自底向上，每步产物都是最终形态的一部分，不产生待拆的中间物）

1. **frame.rs 补齐 storage 能力**：`project()` 的惰性迭代器改为可在不持借用下消费的形式（写入调用方提供的 `&mut Vec<(FrameNumber, usize)>`，或返回定长 `ArrayVec` 风格的 owned 序列），使「先投影再切分/移动」成立；`split_off` 保持现状。
2. **`OwnedBacking` 换表示**：`{ identity, storage: FundedBackingStorage }`，`write_from_start` 与 preflight 组装全部改走 `project()`。`fund_owned_backing` 直返 storage；同步删 `FundedExtent`/`FundedFrames`/`into_extents`/`fund_user_frames`（`funded_selftest` 改走 `fund_backing_storage`，顺手消灭风险登记的旧入口项）。
3. **Commit 切分**：`commit_*` 路径原子产出 live 左/右 storage 与 retiring storage；`PublishedSpaceChange`/`RetiringSpaceChange` 持 retiring storage；删 `retire_backing_one`、`release_one`、`BackingRetireCursor`。同步重推 split permit 预算常量。
4. **对象侧上收**：`prepare_object_mapping` 接 `&ObjectBacking` + view 几何，内部 `project()` 出多段 preflight；`ObjectMappingPlan.preflight` 改 `Vec`；删 `object_bytes` 参数、`PAGE_SIZE` 硬编码、`assert_eq!(permits.len(), 1)`；tunnel.rs 去掉单页断言。
5. **Boot 路径**：`map_bootstrap_block` 改为「prefix storage + payload 并入」；删 `BootBorrowed` 与 `install_bootstrap_funding`。
6. **残留审计**：全仓搜 `BackingExtent`、`BootBorrowed`、`into_extents`、`FundedExtent`、`retire_backing_one`、`object_bytes`；确认删除条件四条全部满足。

每步跑 `just check` + host 单测；步 3–5 各自跑 `just virt`；步 6 后跑全量验收。允许分多次提交（中间态不完整可接受），但最终成品必须干净完整。

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
- host：funded_frame broker 已有 12 项；memory_space 新增 owner 泛型 stub。
- 内核启动自检：funded/Pool/AddressSpace 守恒断言。
- srv_init：扩展 `test_memory_mapping`，补跨 extent 部分 Unmap 左右 live 与 retiring middle 的 frame/charge/permit 守恒、调用线程消散、ProcessDrain 接管。

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
