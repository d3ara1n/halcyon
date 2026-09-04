# 未来审查计划：统一内存事务核与公共 MemoryObject

审查对象是提交，不是当前工作树。本篇记录本次任务对应的提交与改动概要，供日后 Review 对照。

## 提交范围

| 提交 | 性质 | 概要 |
|------|------|------|
| `d2ff81e` | 清场 | 删死类型/重复真值，统一对象身份铸造与错误边界 |
| `5c0bbb0` | 重构 | 四套 AddressSpace 事务收敛为统一核，对象权限真值上收 |
| `d6a162c` | 实现 | 公共 MemoryObject ABI、EXECUTABLE 电平、view owner 与两段式 Unmap/Protect |

前置基座见 `6e18b8f`（admission 五类 + ObjectBacking + MemoryObjectCore）、`16dd3b4`（Tunnel 迁 core）、`51b3742`+`0fad27f`（对象映射多段投影）。

## 改动概要

**清场（`d2ff81e`）**
- `shared/src/mem.rs` 删 `PageNumber`/`MemoryRegionAttribute`/`MemoryOperation`（零引用），移除 shared 与 rinlib 的 `flagset` 依赖
- `memory_space` 删 `RegionKindView::Mapping::holds_write_permit`（只写不读）与 `WritePermit::serial`（零消费者）
- `FundedRootFrame` → `FundedTableFrame`，删单变体 `TableFrameToken`
- `ObjectId` 改由 `object::try_mint_koid` 铸造，删第二个全局对象身份计数器
- `PublishedTableChanges` 单变体收敛为批次；`SpaceError` → `SystemCallError` 收为单一 `From`

**统一事务核（`5c0bbb0`）**
- `MemoryChangePlan`/`PreparedMemoryChange` 以字段表达 source/authority/output/image_end/view 维度；删四套 plan/prepared 类型与平行 complete/rollback/commit（proc.rs 净减约 700 行）
- 对象 view 权限真值从 `ObjectViewAuthorization` 流出，删 `prepare_object_mapping` 三处 `ReadWrite` 硬编码；`ObjectMappingLease` 增 `protection`，退役 fragment 复核按 lease 权限比对
- 请求解析收敛为锁外一次 `MapIntent::parse` / `building_intent`，Validate 预检与 Reserve 复检共用
- `start_user_memory_change` 布尔档位改 typed `ChangeOutput`
- Tunnel Create/Attach 12 个提交前失败点收敛为 `abandon_mapping`/`abandon_unmap`

**公共 MemoryObject（`d6a162c`）**
- shared：`MemoryObjectCreate/Query/Seal(0x55-0x57)`、`ObjectSignals::EXECUTABLE(1<<5)`、新 `memory_object` 模块、`MemoryMapRequest` 增 `source`/`source_offset`
- `MemoryObjectCore` 收编等待面（EXECUTABLE 与状态机共用对象锁）；公共 shell 提供 Handle kind/role 与 rights 上限
- AddressSpace 持 per-object view owner（强引用 + `ObjectViewPermit`）；「是否仍有引用」查账本，不记计数
- `WritePermit` 所有权统一归 AddressSpace，`MemoryRetireSink` 退化为通知面
- 两段式 Unmap/Protect：Validate 报告 permit 多重集，permit 在 AddressSpace 锁外取得后重入 Reserve
- `memory_space`：`seal` 删单槽 waiter，只报告是否发生 Executable 转换
- rinlib `memory_object` affine owner + `MappedRegion::map_object`

## 待审查重点

按风险排序。前四项属正确性，未经独立 review（本轮 reviewer 因 provider 限额中断，仅完成自查）。

1. **WritePermit 守恒**：permit 被静默 drop 会使对象计数永久泄漏、seal 永不完成。需逐条核对每个提交前失败路径：`complete_memory_change` 的 `fail!` 宏、`rollback_memory_change`、`rollback_memory_change_plan`、`acquire_view_permits`/`release_view_permits`、`fund_and_complete_running` 的错误元组、tunnel 的 `abandon_mapping`。自查未发现裸 drop（`proc.rs:3078` 的 `drop(permit)` 是 `ObjectViewPermit` 而非 `WritePermit`），但缺独立复核。
2. **view owner 与 permit 的析构顺序**：`advance()` 先 pop fragment 再 pop permit，`retiring_views` 在末尾 `clear()`。需确认 owner 不会在本批 permit 归还前析构（最后一个 `Arc` 消散会使 permit 失去归还目标）。`RetiringObjectView::_owner` 的保活意图靠注释表达，无类型级强制。
3. **锁阶**：已修两处（`PreparedObjectView::new` 锁外取 identity、`reserve_mapping_resources` 上移锁外）。需全仓复核是否仍有 AddressSpace 锁内进入 `MemoryObjectCore` 的路径，特别是 `commit_inner`/`install_view_owner`（运行在 `ADDRESS_SPACE → LIFECYCLE` 下）与 drain 路径。debug 秩栈断言已覆盖实际执行到的路径。
4. **Commit 零分配**：`install_view_owner` 依赖 plan 阶段 `views.try_reserve(1)`；「对象已有 view」分支走 drop 而非 insert。需确认 `commit_inner` 全路径无分配。
5. **`MapAuthority::AddressSpace` view 的撤销闭包**：进程自有 view 不产生 lease，靠账本区间撤销。已验收部分 Unmap 与 charge 守恒，但需核对与 object-owned lease 路径的语义对称性。
6. **容量自洽**：`MAX_WRITE_VIEWS_PER_OBJECT`(64) 与 ObjectView admission（全局 8192 / 每 sponsor 256）的关系未成文；公共对象 `permit_limit` 用 64、Tunnel 用 2，理由只在代码注释。

## 验证覆盖缺口

已验收（`srv_init` core `test_memory_mapping` 尾段）：创建 → 快照 → 同对象 RW/RO 双 view → Handle 先关仍可访问 → 部分撤销 → Pool charge 守恒。

未覆盖，需在后续切片补验收负载：

- **seal 与 `EXECUTABLE` 电平完全未验收**：无 RX view 建立、无 WaitMany 观察该电平、无「seal 与在途 writable view 竞态」路径。这是本次唯一「实现了但未被任何负载执行」的 ABI 面。
- 跨进程 view（同对象映入两个进程、Handle 经 TRANSIT/GRANT 转移后建立 view）
- rights 拒绝矩阵（缺 `MAP`/`WRITE`/`EXECUTE` 时的失败分类）
- `MemoryObjectQuery` 的 `state` 字段只在 Mutable 下断言，Sealing/Executable 未观察
- 对象 backing 的 frame 级守恒（当前只断言 Pool charge）

## 验证状态

host 全绿；`just virt`、`just virt-release`、`just sifive_u` 通过；`just virt-stress` 16/16（其间一次 `last-thread-exit-vs-kill` 偶发 flake 复跑即过，与 KNOWN_ISSUES 记录一致）。最大 debug 栈帧 `0x2390`，guard `0x3000` / audit `0x2800` 未变。
