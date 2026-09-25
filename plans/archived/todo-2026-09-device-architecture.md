# 设备体系设计工作记录

> 状态：模块与责任边界设计稿已完成；本任务只形成设计文档和模块图，不实现内核 ABI、用户态服务或驱动。后续实施需以选定平台与真实设备建立独立工作记录。

## 目标与范围

从嵌入式 capability 微内核的系统目标出发，补全设备声明与描述、发现与 PnP、资源授权、接管与租借、驱动运行、功能服务和故障恢复的组合。长期设计唯一归属为 [device](../../notes/ideas/device.md)，本记录保存设计依据、当前实现差距、验证边界与后续实施入口。

已确认的系统方向：不以 POSIX 兼容为目标，不据此补建兼容接口；多用户不是当前需求，未来认证、用户和 ACL 政策位于用户态，内核只强制 capability 与隔离机制。

## 当前事实与基线

- 设计开始时工作树干净，基线提交 `bb38eee`。
- `notes/ideas/device.md` 原有平台资源、驱动授权和 IRQ 边界，尚无完整模块与生命周期设计。
- `shared/erhino_shared/src/object.rs`、`call.rs` 尚无 MMIO、IRQ、DMA 对象或操作。
- `os/kernel/src/trap.rs` 只接入 SSIP/STIP；外部设备中断尚未接通。
- `user/drivers/drv_spi_sifive/src/main.rs` 仅输出日志。
- 现有 ProcessBuilder、ProcessGrant、Job/Process 监督、Runtime、RPC、WaitSet、MemoryObject、Tunnel、Runnel、FAL 和服务 Registry 是可复用基础；BufferQueue 只有方向文档。
- 设备设计与 [平台保留内存生命周期](../todo-2026-09-platform-reserved-memory-lifecycle.md)、[IRQ 审查承接](../todo-2026-08-26-review-carryover.md)、[关机编排](../todo-2026-09-system-shutdown-orchestration.md) 联动，不另建重复实施计划。

## 设计责任与取舍

- 描述、授权、占用和客户端会话分开；设备记录和路径不授予硬件访问权。
- 静态声明与动态发现共用设备目录和生命周期；PnP 是可选自动化政策，静态嵌入式配置无需运行自动匹配循环。
- 设备拓扑、供应依赖与隔离/复位约束分别建模；可独立租借的范围受真实硬件约束。
- 设备租约位于用户态；内核分别拥有 MMIO、IRQ、DMA 的可撤销使用与特权同步责任，不提前引入万能 Device 内核对象。
- 资源根与管理权由上级保留，使用权交给驱动；释放回复丢失、驱动死亡或设备管理服务失败均不释放仍可能被设备访问的 backing。
- 驱动通用生命周期由 `libdriver` 拥有；设备领域由 `libdevice` 拥有；具体总线与设备类别各自定义协议。逻辑模块不强制一模块一进程。

## 设计产物

1. 扩展设备方向文档，形成三类声明、四类概念、三种关系、逻辑模块表、模块依赖图和绑定生命周期图。
2. 定义首次接管、驱动替换、三类租借、正常退出、突然移除、驱动死亡、管理者失效与固件交接的责任边界。
3. 明确 MMIO/IRQ/DMA 需要的内核机制；IRQ 采用直接可等待 binding 的目标形态，控制器寄存器流程留待固定规范取证。
4. 同步 kernel 的嵌入式/非 POSIX/用户态多用户方向、framework 的库分工与 notes/COMPASS 导航。
5. 设备运行实现没有改变，故未新增或修改 impls，也没有新增空 crate。

## 外部取证

以下为本次设计的事实参照；不将其他系统 API 或平台保证移植为 Halcyon 的既有能力。

| 来源与版本 | 核实事实 | 取证边界 |
|---|---|---|
| [Devicetree v0.4：基础](../../references/normative/devicetree-v0.4/source/chapter2-devicetree-basics.rst)，`compatible`、`status`、`ranges`、`dma-ranges`、`Interrupts and Interrupt Mapping` | compatible 用于编程模型匹配；reserved 可表示固件占用；CPU/bus 地址与 DMA 地址转换分别描述；中断路由不等同设备父子树 | 固定本地规范；具体设备绑定和控制器协议不由通用 DT 语义代替 |
| [Genode Foundations 25.05：Common session interfaces](https://www.genode.org/documentation/genode-foundations/25.05/components/Common_session_interfaces.html)、[Platform session 25.05 源码](https://github.com/genodelabs/genode/blob/25.05/repos/os/include/platform_session/platform_session.h) | Platform session 访问已经分配给客户端的设备；描述 ROM、acquire/release、设备内 MMIO/IRQ 接口与 DMA buffer/address 操作分别存在 | 固定 25.05；release 注释只证明服务内部资源释放接口，不证明任意设备 DMA 停止 |
| [Fuchsia：Drivers and nodes](https://fuchsia.dev/fuchsia-src/concepts/drivers/drivers_and_nodes) | node properties 用于匹配，capabilities 在绑定时路由；复合节点聚合多个父节点能力；移除先停止子节点/子驱动 | 滚动官方概念页，本次只引用该页事实；未固定源码，不作为硬件或 ABI 依据，也不从 node 移除推断 DMA 安全 |
| [seL4 Microkit manual v2.3.0](https://docs.sel4.systems/projects/microkit/manual/2.3.0/) | 静态描述安排设备内存与 IRQ；单 IRQ 至多对应一个 PD；内核 mask 到 ack；设备地址隔离依赖 IOMMU/SMMU | 固定手册版本；其中 IO address space 支持有平台范围，不能推断当前 RV64 平台已有 IOMMU 或 fault 后 DMA 收束 |
| [Linux v6.15：VFIO](https://docs.kernel.org/6.15/driver-api/vfio.html)，`Groups, Devices, and IOMMUs`、`VFIO Device cdev` | 可隔离 group 是所有权粒度；同 group 只能有一个 DMA owner，设备名不同不保证隔离 | 固定文档版本；其 ioctl、设备文件及 legacy container 不作为本项目接口；EEH 示例的停止事务顺序只适用于其平台上下文 |

Genode、Fuchsia、Microkit 和 VFIO 的以上具体片段均核对官方页面或源码。没有采信未核实的“关闭驱动即硬件安全”结论，也未把新版发布说明与旧版接口混成同一实现基线。

## 交付前结构检查

- 主代理核对描述/授权/占用/会话的状态与 owner，区分 PnP 期望政策、租约实际状态和 Registry 发布状态；无第二份可写占用真值。
- 检查依赖方向：驱动生命周期使用租约协议，不依赖设备管理进程；功能与供应节点是协议，FAL/Runtime 不反向认识设备业务。
- 核对正常、失败、取消、退出、超时和退款路径：未知硬件状态保持 Quarantined，DMA backing 独立保活，期限不代替安全证明；跨域准备不得各持资源无限等待。
- 覆盖固件保留、共享寄存器页、共享 IRQ、DMA 隔离粒度、共享复位影响、管理者失败和静态启动发现环。
- 未增加 ABI、代码或 crate；锁序、具体 syscall、PLIC/AIA/IOMMU 操作和平台故障证据属于后续实施门。

## 后续实施边界

设备实现开始前需为选定平台和设备形成唯一实施记录，固定寄存器/中断控制器/DMA 规范，闭合对象角色与撤销完成点、锁序、有界维护、保留内存和启动授权图。首个消费者可以是串口，但本设计不将串口选型视为已授权的实施任务。

必须先证明设备可独立分配、停止和恢复；无法证明 DMA 安全的平台只允许明确受信的驱动配置或受控代理访问。接口错误不能被转换成可重新出租的成功状态。

## 验证边界

本任务只执行文档与结构检查，不运行编译、host 测试或 QEMU，不以既有验收证明新设备机制。外部系统只作为事实参照，不能代替 Halcyon 的责任和硬件契约证明。

文档验证：`git diff --check` 通过；六个变更文档的 104 个本地链接均存在，代码围栏成对；两段 Mermaid 图源中，模块图的 20 个节点均有声明且依赖无环。未执行 Mermaid 渲染，图源检查不等同视觉验收。未运行编译或 QEMU，因为本次没有代码变化。
