# 中期设计审查：notes 全量与机制收敛

> 【待实施计划】审查对象是当前 `notes/` 全量（ideas + impls + README），以提交 `b6b05d1` 的快照为基线冻结。主线（切片 6D 收尾与 6E）由并行会话实施中，因此本审查 **docs-only**：代码仅作取证参考且可能过时，一切 finding 必须锚定文档；代码级结论待 6D/6E 落地后由对应代码审查验证。Review 纪律见 [`REVIEW.md`](REVIEW.md)。

## 背景与动机

切片 1–6 建立的资源模型（供给账本、MemoryPool、funded broker、进程绑定、页表事务、deferred retire）是切片 7–9（MemoryObject、多页 Tunnel、Runnel v2）的全部地基。现有审查队列全部是单提交窄范围复审，没有横切视角检查 ideas/impls 作为整体是否合理、哪些机制该收敛合并。在地基上继续盖楼之前插入本次审查，重构成本最低。

## 吸收关系

`todo-2026-08-27-mechanism-generalization-review.md` 的五个轴中，**公理层**与**文档自洽**两轴为设计/文档级，吸收进本审查（真值点转移，原计划标注并保留代码级三轴待 6D/6E 落地后执行）：

- 公理层 → 本审查「公理层一致性」轴：新增 role/对象按 `ideas/object.md`「收束分层」公理分类；跨 hart 完成确认为正交维度。
- 文档自洽 → 本审查「文档纪律与自洽」轴：三篇以上 impls 对同一机制无重复描述（README 归属纪律）。

## 审查轴

1. **机制收敛与合并**：结构同构的 reserve/commit/publish 事务骨架（Pool Derive、ProcessBindMemory、Building Map/Write/Grant/Attach/Start、Running Map/Unmap/Protect、Tunnel create/attach/close、MemoryObject create/seal）是否收敛于统一 seam，还是调用点仍在自建协议；admission permit 分类是否收敛于未来 KernelMemoryBudget 形状还是特例化膨胀；wait/remote 槽、retire 路径是否有可合并的平行机制。
2. **公理层一致性**（吸收轴）：object.md 收束分层公理是否覆盖全部新增对象；是否出现对单一对象的特判/特殊处理。
3. **文档纪律与自洽**（吸收轴）：ideas 不下沉结构字段与代码引用、impls 随代码演进不过期；同一机制单一归属（无跨篇重复描述）。
4. **前向收敛**：过渡形状（admission 门、剩余 raw 路径：匿名 backing→6E、Tunnel→切片 8、selftest→切片 10）是否收敛向终态，有无需要返工的死胡同。
5. **ideas 粒度与边界**：文档切分是否仍成立（如围绕数据搬运的 message/ipc/call/rpc/runnel/tunnel/shared-memory/buffer-queue 群），有无该合并或该拆的。

## 戒律

- 「核对新 seam 是否真正删除调用点协议，而不再重议是否抽象」（继承自 mechanism-generalization review）：收敛 finding 只认「已有统一 seam 但调用点仍自建协议」的证据。
- 范式级推翻只在范式被证明错误时发生；性能问题在现有范式内逐点优化。
- 已知过渡状态（6E 在途、Tunnel raw 路径待切片 8、selftest 待切片 10）列为非 finding。
- 审查者保留完整项目上下文，只报告不修改；findings 带证据（文档路径 + 章节/行），集中呈报。

## 完成标准

- 产出 `review-2026-09-midterm-design-review.md`：每条 finding 有证据、影响面与建议动作分类（立即修 / 并入切片 7–10 / 立新案 / 驳回说明）。
- 用户 triage 完成；行动项分别落入对应 todo/主线计划或 ideas 修订。
- 原 mechanism-generalization review 计划完成吸收标注；本计划随审查收口归档。
