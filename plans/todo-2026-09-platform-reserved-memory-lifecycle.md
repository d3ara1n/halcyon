# 平台保留内存完整生命周期

> 【未来实施计划】承接 Devicetree Specification v0.4 的动态 `/reserved-memory`、`reusable` 与设备 `memory-region` 引用。静态 `reg`、FDT reservation block 和 `no-map` 由当前 MemoryPool 主线的供给账本切片先行闭合；本计划必须在正式设备/DMA 资源接入前完成，不把未知语义降级成永久排除区。

## 外部契约

依据 `references/normative/devicetree-v0.4/source/chapter3-devicenodes.rst`「`/reserved-memory` Node」：

- 子节点以 `reg` 声明静态区间，或以 `size` 请求动态分配；两者并存时 `reg` 优先；
- 动态请求可以用 `alignment` 约束地址边界，并用 `alloc-ranges` 限制可选物理范围；
- `no-map` 禁止标准系统内存映射和无驱动控制的推测访问；
- `reusable` 允许 OS 临时使用，但设备所有者必须能够收回；它只适合可丢弃、可重建或可迁移内容；
- `no-map` 与 `reusable` 互斥；设备以 `memory-region` / `memory-region-names` 引用保留区。

## 目标模型

解析层保留节点身份、phandle、compatible、静态或动态几何、映射政策及设备引用，不把不同生命周期压平成无名区间。平台 admission 在发布 FramePool、系统储备或 root MemoryPool 前，把全部静态排除和动态放置结果归一为唯一物理布局；任一请求无法满足、引用悬空或政策矛盾都使启动失败。

动态放置是启动期物理布局事务，不是普通运行期 frame claim。它必须同时考虑全部静态 reservation、内核/启动 owner、请求的 size/alignment/alloc-ranges、页边界与确定性放置顺序；提交后为每个节点产生稳定 region identity，失败时不得发布部分布局。

`reusable` 不是“先放进普通空闲池”。它需要显式平台 region owner、可撤回 loan、允许的 backing 类别和有界 reclaim：借用者只能存放可丢弃、可重建或可迁移内容；设备 owner 请求收回后，系统停止新借用、收束或迁移既有内容、完成地址翻译与 DMA 同步，最后把完整 extent 归还 region。普通 MemoryPool grant 不可撤销，不能承担该语义；是否复用通用 MemoryLease/资源域须在实施前单独决策。

## 实施切片

1. 扩展 DT 纯逻辑模型，完整校验 `size`、`alignment`、`alloc-ranges`、`compatible`、phandle 与 `memory-region` 引用，保留静态优先规则及未知属性边界。
2. 建立 host 可测的启动期动态放置 planner：固定容量输入输出、checked arithmetic、确定性结果、整体失败原子和区间供给闭包。
3. 定义平台 region capability、设备引用交付与静态专用区所有权；普通永久区、`no-map` 区和可复用区使用不同类型。
4. 设计并实现 reusable loan/reclaim 状态机，闭合 backing 可迁移性、映射失效、在途 DMA、owner 消散和系统收束。
5. 把动态放置结果接入 direct-map admitted ranges、system/user supply 分类和启动日志；禁止 raw frame 路径绕过 region owner。
6. 在至少两个自备平台 DTS fixture 上验证静态/动态、多 alloc-ranges、对齐、容量不足、引用错误、no-map/reusable 互斥以及 reclaim 竞态。

## 决策门

实施前必须确定：动态请求的确定性排序与放置策略；region capability 由哪个 primordial authority 接收；设备服务如何取得引用；reusable 借用允许哪些 backing 类别；reclaim 的超时、失败与设备移除政策；有 IOMMU 与无 IOMMU 平台各自的 DMA 收束边界。上述结论进入 `notes/ideas/` 后再编码。

## 完成标准

支持的 Devicetree 描述不依赖平台名称或模拟器特例；动态 reservation 在普通供给发布前整体落位；reusable 的每一页始终属于 region、loan 或 reclaim 中的唯一 owner；direct map、FramePool、MemoryPool、设备能力与 DMA 映射对同一物理事实只有一份真值；所有失败和收束路径均能恢复各自守恒方程。
