# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。

## 矫正区

以下区域处于待矫正状态：其现有写法、结构与模式不作为新工作的参考。进入或引用这些区域前先读对应条目，按条目指定的依据工作；条目随承接任务完成而删除。

全项目重审范围表示目标结构需要重新确定，不等于其中每个实现已被证明错误。代码与 impls 继续如实描述现状，不能依据目标 ideas 把未实施行为回填成事实。

| 范围 | 问题 | 改用的依据 | 承接 |
|---|---|---|---|
| 设备资源与用户态接线：`user/libraries/libdevice`、`user/libraries/libdriver`、`user/services/srv_device`、`user/services/srv_driver`、`user/drivers/`、`srv_init`、`board/io`、`board/irq`、`shared/erhino_shared::platform`、`boot/bootstrap`、`boot/watch`、`resource_tree`、`resource/mmio`、`resource/irq`、`resource/dma`、线程请求与 VM/Backing 退休接点 | 第二轮设备控制链、持续监督、分步装载及生命周期收口已统一提交并完成所选组合验证，旧独立监督线程已删除。DMA＋IPI 组合证据缺口及全 hart idle／真实 IRQ 门均已关闭；独立时钟异常按用户决定延期，复合 USB 接口未覆盖。本次单项调查未重新核定首批总体交付 | [第二轮设备目标](plans/todo-2026-09-device-system-design.md#第二轮收敛设计2026-10-27)、[持续监督目标](plans/todo-2026-10-07-service-assembly.md#第二轮持续监督设计2026-10-27)、[生命周期收口](plans/archived/review-2026-10-27-device-lifecycle.md) | 后续按[设备交付接力](plans/todo-2026-09-device-system-design.md#第二轮提交与调查接力)及唯一收口表推进；[固定交付独立 Review](plans/todo-2026-10-27-device-convergence-review.md)单独登记，不重复已完成的第二轮迁移与组合取证 |
| 全项目代码与当前实现说明：`os/`、`shared/`、`user/`、`notes/impls/` | 已授权无兼容约束整体重审；现有组件、接口和消费者不能作为必须保留某项机制的理由 | [系统重构总纲](plans/todo-2026-09-29-system-redesign.md) 的已确认原则及独立分案；只读盘点保留为现状事实 | 各分案设计闭合后精确登记实施范围；未完成前不作为新结构范本 |
| `notes/ideas/` 全体 | 施工和重构期间的未审定线稿；清理、精简或局部目标改写均不代表机制已重新设计 | [系统重构总纲的审定登记](plans/todo-2026-09-29-system-redesign.md#重构设计依据与-ideas-审定登记)及所属 todo 的详细设计；未登记的 ideas 范围不作目标依据 | 各分案在计划中完成详细设计后登记可参考范围；文档清理、设计审定与代码重构分别记录 |
