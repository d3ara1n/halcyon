# 切片 6D 页表资金化收口未来审查

> 【未来审查计划】审查对象固定为提交 `addb4a5` 与后续文档/异常路径修正提交 `b4bfb20`。只审切片 6D 的 root 与全部用户页表资金化、锁外 funded owner 生命周期、失败回滚、错误分类、异常可观测性和实现文档一致性；匿名 backing 全面资金化、公共 MemoryObject、多页 Tunnel 与 Runnel v2 不混入本结论。

## 审查重点

1. **Funded owner 唯一归属**：确认 root、中间表、mega split、replacement、Published/Retiring 结果和失败回收包均只有一个 owner；PoolBinding 不重复持有 root。
2. **锁序闭包**：逐入口核对 `MEMORY_POOL < ADDRESS_SPACE`。任何 funded owner、PreparedTranslation、失败 owners、backing 与 published table changes 都不得在 AddressSpace 锁内触发归还。
3. **跨锁事务**：核对 Running、Building、ELF、bootstrap、Tunnel Map/Unmap 的 plan → 锁外 funding → 锁内 complete/commit，确认 transaction gate、generation、ledger 和 backing 在竞争及 stale 场景下保持一致。
4. **失败原子性**：覆盖 quota、物理库存、结构上限、metadata、prepared allocation、页表 prepare、shootdown、结果写回和 detached close 失败，确认 ledger、Pool、FramePool、backing、permit 与 gate 完整恢复。
5. **错误分类**：确认 `QuotaExceeded`、`OutOfMemory`、`ReachLimit`、`ObjectBusy` 不因跨层映射或回滚适配被压平。
6. **Building 语义**：确认 `image_end` 只由已提交映像区推进，固定主栈不推进；Unbound 异常路径可观测且不会丢失已取得资源。
7. **验证与文档**：核对 host debug/release、`just check`、`just virt`、`just virt-stress`、`just virt-release`、完整 acceptance，以及 `notes/impls/mm.md`、本计划和 `plans/COMPASS.md` 的状态一致。

## 当前证据

- `just check` 通过。
- `page_table`、`frame_pool`、`memory_pool` host 测试通过。
- `just virt-stress` 通过，thread memory suite 与竞态矩阵均为 `16/16`。
- `just virt-release` 通过。
- `just acceptance` 通过，包含 debug stress、release core 与适用平台收束。
- 页表 transitional raw API 已从内核任务映射路径删除；匿名 backing、Tunnel 单页 backing 与库存自检 raw 路径仍分别归属 6E、8、10。

## 完成标准

审查必须按严重度给出文件/符号证据、可达交错、owner/ledger 方程影响和验证复现。锁内退款、root 双重归还、funding 失败泄漏、stale Prepared 发布、错误分类丢失、`image_end` 越界或异常路径不可观测均为阻断项。若仅剩匿名 backing 或后续 IPC 数据面能力，必须回指唯一主计划，不在本审查复制实施方案。
