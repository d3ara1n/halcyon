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
- 当前缺：ObjectBacking、ObjectView、Connection、Endpoint、Invitation。
- 新增：`ObjectBackingPermit`、`ObjectViewPermit`、`ConnectionPermit`、`EndpointPermit`、`InvitationPermit`，各有全局/sponsor 上限、唯一 owner、退款终点。
- Object core/backing permit 随 creator 消散后继续存活；view permit 随独立 view 存活；Tunnel 三类随各自对象存活。
- 删除条件：公共 Create/TunnelCreate 进入 Commit 前全部预留成功，Commit 后零分配。

**A2. 引入 `FundedBackingStorage` 与 `ObjectBacking`**（`os/kernel/src/frame.rs`、新 `os/kernel/src/task/memory_object.rs`）
- 当前：`FundedFrames::into_extents` 立即拆包，`OwnedBacking` 持 `Vec<BackingExtent>`，Tunnel 持单 `FundedExtent`。
- 新增：`FundedBackingStorage` 包住多 extent `Funded`，提供 `project(range)`；`ObjectBacking` 包装它且不可分解。
- 删除：`into_extents` 路径；`BackingExtentOwner` 的 `BootBorrowed` variant（非真正 owner）；`install_bootstrap_funding` 提交后补 owner 路径。
- 删除条件：`OwnedBackingSlice` 与 `ObjectBacking` 成为 storage 的唯一消费者。

**A3. 建立 `MemoryObjectCore`**（新 `os/kernel/src/task/memory_object.rs`）
- 统一：ObjectId 铸造、ObjectBacking、`MemoryObjectState` 改接 `ObjectWaitState`（删除 `seal_waiter: Option<u64>`）、metadata permits + sponsor 强引用。
- Tunnel Connection 改持 `Arc<MemoryObjectCore>`；公共 shell 复用同一 core。
- 删除：`os/kernel/src/task/tunnel.rs:97` 的 `NEXT_MEMORY_OBJECT` 私有铸造。
- 删除条件：Tunnel 与公共对象走同一 core 构造路径，object identity 无双来源。

### B. 投影统一

**B1. 上收 extent → bounded translations 投影为匿名/对象共用**（`os/kernel/src/task/proc.rs`）
- 当前：匿名侧 `OwnedBacking::preflight_install`（proc.rs:641）按 extent 迭代；对象侧 `prepare_object_mapping`（proc.rs:2908）硬编码单页单 PA。
- 目标：`FundedBackingStorage::project(range)` 输出有界 PA spans；匿名与对象都调它。
- 删除：`prepare_object_mapping` 的单 PA adapter、`MapBacking::Object` 的 `object_bytes` 参数、Tunnel 的单页 `PAGE_SIZE` 硬编码。
- 删除条件：单页 Tunnel 作为「集合长度为一」走共用投影，`prepare_object_mapping` 四个类型全删。

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

1. **补 metadata admission 五类**（resources.rs）→ 启动 selftest 穿过全局/sponsor exhaustion、Commit 后零分配。
2. **建立 `FundedBackingStorage` 与 `ObjectBacking`**（frame.rs、新 memory_object.rs）→ 删 `into_extents`、`BootBorrowed`。
3. **建立 `MemoryObjectCore`**（memory_object.rs、tunnel.rs）→ Tunnel 改持 core、删私有 ObjectId 铸造。
4. **上收 bounded projection**（proc.rs）→ 删单 PA adapter、对象/Tunnel 走共用投影。
5. **owner-aware ledger**（proc.rs、memory_space/space.rs 泛型化）→ Publish 切 live/retiring slice、object region 持强 owner。
6. **统一事务核**（proc.rs）→ 删四套 plan/complete 类型与函数。
7. **迁移 Running/Building/Tunnel 调用点**→ 全走统一事务核。
8. **开放公共 MemoryObject ABI**（shared、syscall、rinlib）→ Create/Query/Seal、`EXECUTABLE` 信号、用户态 affine owner。

每步先跑对应 host debug/release、`just check`；涉及启动后跑 `just virt`；涉及 Remote/drain 补 `just virt-stress`；收尾跑 `just acceptance`。

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
