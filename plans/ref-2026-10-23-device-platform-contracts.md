# 设备首条平台路线的固定外部证据

取证日期：2026-10-23。用于[设备设计](todo-2026-09-device-system-design.md)的硬件与协议约束，不表示驱动已经实现或 QEMU 路线已经运行。规范与模拟器实现分别取证；QEMU 的简化不能作为真实硬件的保证。源码链接固定为 QEMU v11.1.0；后续验收若使用其他版本，先核对差异并记录实际版本。

## QEMU virt、PCI 与中断

固定来源：[virt.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/riscv/virt.c)、[virt.h](https://github.com/qemu/qemu/blob/v11.1.0/include/hw/riscv/virt.h)、[gpex.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/pci-host/gpex.c)、[virt 说明](../references/implementations/qemu-v11.1.0/riscv/virt.rst)。

- PCI host 的 FDT compatible 为 `pci-host-ecam-generic`；ECAM 窗口为 `0x30000000 + 0x10000000`，32 位 MMIO 为 `0x40000000 + 0x40000000`，PIO 为 `0x03000000 + 0x10000`。这些是该机器的证据，运行实现仍消费经校验的 DT/平台授权，不把地址常量变成通用协议。
- PCI 节点声明 `dma-coherent`。根总线为 `pcie.0`。`aia=none` 使用 PLIC；PCI INTx 有四条源 32—35，interrupt-map 按 slot/pin swizzle，不能将某个 IRQ 常量硬编码给所有 PCI 设备。
- `msi-parent` 只在对应 IMSIC 配置生成。首条路线不使用 AIA/MSI，也不假设存在 IOMMU；可选 IOMMU 是另一能力配置。
- 当前仓库 QEMU 构建路线使用自备 DTS。接入时必须同步设备配置、DT interrupt-map/ranges 与实际机器；不能只增加命令行设备而继续使用不相符的平台描述。

## PLIC 1.0.0

固定来源：[PLIC 1.0.0](https://github.com/riscv/riscv-plic-spec/tree/1.0.0)，tag 对应提交 `f8ec1b7e9b1a92c34af8e0ab7eb33838813785a3`；实现交叉核对 [sifive_plic.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/intc/sifive_plic.c)。

- source 0 表示没有中断；有效源从 1 开始。claim 原子选择最高优先级 pending 源并清 pending，不受 threshold 过滤方式替代。
- priority 位于 `base + 4*source`；pending 从 `+0x1000` 起；enable 为 `+0x2000 + context*0x80`；threshold 与 claim/complete 为 `+0x200000/+0x200004 + context*0x1000`。
- gateway 在收到 complete 前不会再转交该源下一次请求。complete 不验证软件所谓“最后领取者”；软件必须维护对应使用代次和正在服务的源。
- **源未 enable 时 complete 可被忽略**。停止持有 in-service 的源时，先在 enable 仍有效时完成该 claim，再关闭 enable；不能先禁用再期待 complete 释放 gateway。
- pending 不是可任意清除的普通读写寄存器。新绑定可能观察到旧 pending 提示，必须重新检查设备状态；旧 token 不得确认新绑定的中断。
- QEMU virt 的内部 num-sources=96 含保留源 0，DT ndev=95、有效源 1—95，优先级 0—7；具体 context 从 DT hart/context 对应关系确定，不以 hart 编号直接猜测。

## xHCI 1.2b 与 QEMU 实现

规范：[Intel xHCI Revision 1.2b，文档 625472](https://cdrdv2-public.intel.com/625472/625472_xHCI_Rev1_2b.pdf)。实现：[hcd-xhci-pci.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/hcd-xhci-pci.c)、[hcd-xhci.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/hcd-xhci.c)。

| 事实 | 固定依据与设计影响 |
|---|---|
| 寄存器分区 | §5.3、§5.6；CAPLENGTH、DBOFF、RTSOFF 决定 Operational、Doorbell、Runtime 地址，不猜相邻布局 |
| 正常控制器停止 | §5.4.1、§5.4.1.1、§5.4.2：清 USBCMD.R/S，等待 USBSTS.HCH；规范给出 halt 时间要求。超出平台期限进入故障，不把等待结束当作已经 halt |
| 复位 | HCH 成立后才能按合同置 HCRST；等待其清除和 CNR=0，再写 Operational/Doorbell；§4.2、§5.4.1—2 |
| DMA 布局 | TRB 16 字节；ring segment 不跨 64 KiB。command/event ring、ERST、DCBAA 需 64 字节对齐；transfer ring 至少 16 字节对齐；context 大小按 CSZ；§4.9、§6.1、§6.5、表 6-91 |
| 传输分段 | 数据跨 64 KiB 边界时按 TRB 规则拆分，不能用虚拟连续代替物理连续；固定页内的小 ring 可满足首批要求 |
| 热插事件 | §4.19.3、§5.4.8：CCS 改变、CSC 与 Port Status Change Event；CSC 按 RW1CS 处理 |
| 真实设备停止 | process DONE、CPU fence、关闭 MMIO 映射均不替代 xHCI 的停止或总线隔离证据 |

QEMU 的 `qemu-xhci`：BAR0 为 64 位 MMIO，大小 `0x4000`；DMA 使用 PCI address space。MSI/MSI-X 关闭时 interrupter 0 走 INTx，故首条路线显式设 `msi=off,msix=off`。设备能力报告 64 slots、16 interrupters、32 字节 contexts、64 位 DMA、零 scratchpad；实现要求 event ring segment 数为 1。驱动应读取并校验能力，超出所支持形状明确拒绝，不把模拟器常量推广为 xHCI 通用限制。

QEMU `xhci_stop` 同步设置 HCH，CNR 未实现真实异步就绪过程。模拟器通过不能验证真实硬件的 halt 延迟、cache maintenance、非一致 DMA 或复位时序。

## USB 2.0 枚举与 HID 键盘

USB-IF 官方来源：[USB 2.0 specification 页面](https://www.usb.org/document-library/usb-20-specification)，[2025-06-03 发布包](https://www.usb.org/sites/default/files/usb_20_20250603.zip)内 `usb_20.pdf`，核心规范 Revision 2.0。HID：[HID 1.11](https://usb.org/sites/default/files/hid1_11.pdf)、[Usage Tables 1.4](https://usb.org/sites/default/files/hut1_4.pdf)。

- USB §9.3、表 9-2：Setup 固定 8 字节，`bmRequestType:u8, bRequest:u8, wValue:u16, wIndex:u16, wLength:u16`；多字节字段按 USB 小端。Recipient、方向、长度都属于权限与缓冲检查内容。
- §9.4.3/6/7：GET_DESCRIPTOR、SET_ADDRESS、SET_CONFIGURATION。地址和配置由总线 owner 管理，不能无条件交给接口驱动。
- §9.6.1/3/5/6、表 9-8/10/12/13：设备、配置、接口、端点描述符。配置 `wTotalLength` 覆盖所属描述符串；逐项按 `bLength` 遍历，并检查最短头、非零推进、剩余长度与类型要求。端点号为低四位，方向为 bit 7。
- HID §4.2、§7.2.5—6、Appendix B.1：Boot Keyboard 为 interface class 3/subclass 1/protocol 1；默认 Report Protocol，首批显式 SET_PROTOCOL(0)。Boot 输入报告 8 字节：modifier、保留字节、六个 usage。ErrorRollOver 和 modifier usage 按 Usage Tables，不把错误报告当正常按键释放。
- QEMU [dev-hid.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/dev-hid.c)、[hid.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/input/hid.c)实现 `usb-kbd` 与对应控制请求。首批只承诺 boot keyboard 输入；通用 HID report descriptor、其他设备类别和打印协议另行扩展。原 hid-core.c 路径在固定 tag 中不存在，本轮取证已更正。

## 热插控制与证据边界

固定来源：[USB 设备说明](https://github.com/qemu/qemu/blob/v11.1.0/docs/system/devices/usb.rst)、[QMP qdev schema](https://github.com/qemu/qemu/blob/v11.1.0/qapi/qdev.json)、[USB bus](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/bus.c)。固定控制器在启动时装入；运行期通过 QMP `device_add`/`device_del` 在 `xhci.0` 加减 `usb-kbd`。这验证 USB 设备热插，不声称 PCI host/function 热插已经实现。通用 `device_del` 的成功回复不自动等于删除完成，验收需观察 QMP 事件与客体登记/绑定终态，不用主机命令返回代替客体收束。

最终停止还依赖 [SBI 3.0 System Reset](../references/normative/riscv-sbi-v3.0/src/ext-sys-reset.adoc)：成功 reset 不返回；失败可返回错误。Cold reboot 表示完整复位，不能以 Warm reboot 替代所需硬件复位；提交 reset 本身不证明此前已安全退款。CPU/I/O 顺序与跨 hart 页表失效按[固定 ISA 索引](../references/CONTRACTS.md#risc-v-isa-与特权架构)，不将 FENCE 当成 cache flush 或设备完成。

## 施工准入补充取证

本轮研究任务重新读取 xHCI 1.2b、QEMU 固定源码和版本差异；主代理另外核对本机版本、设备属性与无 IOMMU 的 DMA address space 回落。以下为资料证据，不是实际 USB/IRQ/DMA 运行结果。

### 取消、停止及内存释放

同一 [xHCI 1.2b 规范](https://cdrdv2-public.intel.com/625472/625472_xHCI_Rev1_2b.pdf)的约束如下，页码为印刷页码：

| 动作 | 固定章节与原文要点 | 不能推导的保证 |
|---|---|---|
| Stop Endpoint | §4.6.9，p118–125：“If a USB IN or OUT transaction is in-flight, it shall be completed”；命令完成前写回最终 dequeue pointer/DCS，停止时生成 Stopped Transfer Event | 同节明确 “resources and bandwidth … are not released”；不能把 Stop 命令提交或单独的停止提示当成 endpoint/ring 已释放 |
| Set TR Dequeue Pointer | §4.6.10，p125–128：仅 Error/Stopped 状态；“Invalidate any xHC TDs that may be cached”；部分 TD 剩余 TRB 不再执行 | 不等完成就复用旧 TD/地址仍无保证；正常运行状态不能任意改 dequeue |
| Disable Slot | §4.6.4，p96：endpoint 为 Stopped 或 Running 中 Idle、相关 outstanding Transfer Events 已收到、定向命令完成；完成后 “software can free the Device Context, Transfer Ring, Stream Context Array, etc.”，随后清 DCBAA 槽 | 不满足前置是未定义行为；不能因 USB 物理拔出直接释放 |
| R/S 清零与 HCH | §5.4.1/§5.4.1.1/§5.4.2：HCH 表示 pending pipelined transactions 完成并停止；halt 过程有 16ms 规范界。未遵守先停止 endpoint/command、收完成事件的建议，queued 工作可能丢失/中止 | 不保证 Event Ring 已空；§4.19.2 和 halt 章节记录 HCH/ER full 下事件可能被门控或丢失 |
| BME 清零 | §4.21.2：先 halt 并确认 HCH=1，再清 Bus Master Enable | 单清 BME 不是已经结束 DMA 的证据 |

CPU 对 ring 的消费和软件请求完成仍由驱动负责；硬件完成证明与软件“所有请求均成功”是两个事实。规范中没有 “HCH=1 ⇒ Event Ring empty” 的保证。

### PCI DMA 与输入/删除控制

- [pci.c v11.1.0](https://github.com/qemu/qemu/blob/v11.1.0/hw/pci/pci.c)：写 PCI_COMMAND 通过 `pci_set_master` 启停 bus_master_enable_region；BAR 解码受 MEMORY/IO 位控制，INTx 受 INTX_DISABLE 控制。[hcd-xhci-pci.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/hcd-xhci-pci.c) 把 xhci.as 设为 pci_get_address_space，ring DMA 走该地址空间。故仅写 BAR 不会自动接通 PCI DMA。
- 主代理读取 [pci.c v11.1.2](https://github.com/qemu/qemu/blob/v11.1.2/hw/pci/pci.c) 的 `pci_device_iommu_address_space`：有 IOMMU 使用其 get_address_space，否则 `return &address_space_memory`。结合 virt 默认无 IOMMU 与 dma-coherent 声明，支持所选模拟平台的物理地址 DMA；不由 MMIO ranges 恒等单独推导 DMA 恒等。
- [virt.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/riscv/virt.c) 的 PCI memory ranges 在该机型 child/parent 相同，PIO 则 child=0、parent=0x03000000；运行实现仍须解析地址空间类型，不把所有 ranges 当恒等。
- [qapi/ui.json](https://github.com/qemu/qemu/blob/v11.1.0/qapi/ui.json)、[ui/input.c](https://github.com/qemu/qemu/blob/v11.1.0/ui/input.c) 提供 `input-send-event` 和 `send-key`；前者要求 VM running 且有对应输入 handler。[hid.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/input/hid.c) 注册 INPUT_EVENT_MASK_KEY handler。状态型键盘验收应分别发送按下/释放，在两次之间等待客体 Query 观察，不能依赖短暂边沿不被合并。
- USB [bus.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/bus.c) 只挂同步 unplug；[qdev-hotplug.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/core/qdev-hotplug.c) 执行 unrealize，[qdev.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/core/qdev.c) 在 finalize 发送 DEVICE_DELETED。事件与命令回复分别接收，不假设固定先后；主机 object 已删除也不证明客体 Binding 已安全 Released。

### 本机 11.1.2 与固定 11.1.0 的适用性

`qemu-system-riscv64 --version` 实际返回 **11.1.2**；`-device qemu-xhci,help` 确认 msi/msix、p2/p3、streams 等属性存在。两条只查询版本/属性的命令均正常退出，没有启动客体。

研究任务对固定 tag 的 virt/gpex/PLIC、PCI、xHCI、USB HID/bus、QMP 输入/删除相关文件逐字节比较：virt.c 的差异为 mtimer DT 节点地址修正；hcd-xhci.c 增加 timer 的 mem_reentrancy_guard，并整理 ISO 重试。其余所检查的相关头文件、PCI/PLIC/HID/USB/QMP 路径无差异。差异见[两 tag 比较](https://github.com/qemu/qemu/compare/v11.1.0...v11.1.2)，具体源码采用 [virt.c v11.1.2](https://github.com/qemu/qemu/blob/v11.1.2/hw/riscv/virt.c)、[hcd-xhci.c v11.1.2](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c)。

以上差异未改变本案使用的 BAR、DMA 地址空间、PLIC/INTx 和 boot keyboard 合同；首批实际验收选本机 11.1.2，保留 11.1.0 取证来源及这份差异说明。ISO 非首批承诺，不将其行为差异描述为全 xHCI 等价。仍未验证真实硬件 halt 延迟、非一致缓存、恶意 DMA 隔离或实际热插运行。

## 2026-10-24：PCI 平台准入取证

本节只记录规范与固定源码事实；本轮实际运行证据由[设备主计划](todo-2026-09-device-system-design.md)拥有。

- QEMU [virt.c v11.1.2](https://github.com/qemu/qemu/blob/v11.1.2/hw/riscv/virt.c) 的 RV64 high MMIO 长度固定为 16 GiB，基址为 `ROUND_UP(0x80000000 + machine->ram_size, 16 GiB)`，该公式不消费 maxram_size。仓库显式 `-m 1G` 得到 `0x400000000 + 0x400000000`，与现有 DTS 一致；RAM 超过 14 GiB 后基址可能上移，不能把本轮 profile 常量当成任意 RAM 配置。ECAM、mem32、PIO 分别为 `0x30000000/0x10000000`、`0x40000000/0x40000000`、`0x03000000/0x10000`。
- [pcie_host.h v11.1.2](https://github.com/qemu/qemu/blob/v11.1.2/include/hw/pci/pcie_host.h) 给出每 bus 1 MiB、每 function 4 KiB 的 ECAM 几何；[gpex.c](https://github.com/qemu/qemu/blob/v11.1.2/hw/pci-host/gpex.c) 与 virt 将 ECAM/memory alias 映射到 system memory。`create_fdt_pcie` 发布 bus-range 0..255、三条 ranges、dma-coherent，aia=none 时不发布 msi-parent。
- [PCI Bus Binding to Open Firmware Rev 2.1](https://www.devicetree.org/open-firmware/bindings/pci/pci2_1.pdf) §2.2.1、§3.1.1、§4.1.1、§12：PCI 地址三 cell、size 两 cell；phys.hi 的 ss 区分 configuration/IO/memory32/memory64，p 表示 prefetchable，n/t 另有语义。ranges 的 child bus 地址、parent CPU 地址和长度分别编码，不能混同。当前 parser 仅支持 host ranges 中无 BDF/register/n/t 的明确形状，其余返回 Unsupported。
- Linux 固定 [host-generic-pci.yaml v6.12](https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/Documentation/devicetree/bindings/pci/host-generic-pci.yaml?h=v6.12) 要求至少 non-prefetchable memory，IO 和 prefetchable memory 为可选；因此可只发行 memory 权限而不开放 PIO。`reg` 基址对应 bus-range 首 bus，映射长度须能覆盖所声明总线。QEMU [hcd-xhci-pci.c v11.1.2](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci-pci.c) 只注册一个 64 位 memory BAR，没有 IO BAR。
- DTSpec v0.4 §2.3.9 没有规定 dma-ranges absent 的通用缺省含义。当前平台恒等 DMA 的依据是前文固定 QEMU 无 IOMMU 的 `address_space_memory` 回落，不是由属性缺省或 dma-coherent 推导。dma-coherent 只表达一致性，IRQ/DMA authority 仍需各自实现和准入。

## 2026-10-25：PCI BAR 与 COMMAND 的补充固定源码

QEMU v11.1.2 [`pci_register_bar`](https://github.com/qemu/qemu/blob/v11.1.2/hw/pci/pci.c#L1504) 为 size 建立 `~(size-1)` 可写 mask，64 位 BAR 同时覆盖低高 DWORD；qemu-xhci BAR0 大小 0x4000、type=4，因此全 1 探测读取低值 0xffffc004、高值 0xffffffff。[`pci_default_write_config`](https://github.com/qemu/qemu/blob/v11.1.2/hw/pci/pci.c#L1807) 按 wmask 更新配置，COMMAND 或 BAR 写触发映射重算；[`pci_bar_address`](https://github.com/qemu/qemu/blob/v11.1.2/hw/pci/pci.c#L1667) 在 MEMORY=0 时不解码 BAR，但配置读写仍可访问。64 位 BAR 可将高 DWORD 置零放入受授权的 32 位 window，仍须 size 对齐、非零、范围无回绕并受平台实际 alias 覆盖。

位真值来自 [`standard-headers/linux/pci_regs.h`](https://github.com/qemu/qemu/blob/v11.1.2/include/standard-headers/linux/pci_regs.h#L40)：IO=1、MEMORY=2、MASTER=4、INTX_DISABLE=0x400。COMMAND 以 16 位访问，避免误写相邻 status 的 W1C 位。清 MEMORY 只影响 CPU BAR 解码，清 MASTER 关闭 bus_master_as 的 DMA alias，INTX_DISABLE 控制已有 INTx 电平的上传；三者不互相替代，任何一项都不能单独证明在途 DMA 已结束。此处仅为源码取证，不是硬件运行验证。

## 2026-10-24：PLIC 与固件交接取证

- [QEMU v11.1.2 virt.h](https://github.com/qemu/qemu/blob/v11.1.2/include/hw/riscv/virt.h) 的 `VIRT_IRQCHIP_NUM_SOURCES=96` 包含保留 source 0；[sifive_plic.c](https://github.com/qemu/qemu/blob/v11.1.2/hw/intc/sifive_plic.c) 属性注释明确含 0。[virt.c](https://github.com/qemu/qemu/blob/v11.1.2/hw/riscv/virt.c) 向 DT 写入减一后的 `riscv,ndev=95`，实际可用编号为 1–95。固定 reg 为 `0x0c000000 + 0x600000`。TCG 的每 hart context 顺序为 M、S；内核仍依据实际 DT 的位置和 CPU intc phandle，不硬编码 `hartid*2+1`。
- [OpenSBI v1.8.1 fdt_fixup.c](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/utils/fdt/fdt_fixup.c#L215-L240) 的 `fdt_plic_fixup` 将 interrupts-extended 每对 cell 中值为 IRQ_M_EXT 的第二项改成 `0xffffffff`，第一项 phandle 与项数保持。generic cold boot 的 final_init 经 fdt_fixups 在交接前调用它；核对 v1.9 同函数内容一致。Linux [v6.12 PLIC binding](https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/Documentation/devicetree/bindings/interrupt-controller/sifive,plic-1.0.0.yaml?h=v6.12) 定义 -1 表示该 context 不提供，跳过时仍保留 context index。实际客体 DT 的捕获证据归设备工作记录拥有。
- 固定 QEMU `sifive_plic_write` 的 enable 分支只存 `plic->enable[...]`，不调用 `sifive_plic_update`；priority/threshold/complete 写及 claim 读会重算输出。故只在 enable 前写 priority，无法保证为启用前已 pending 的源重新评估输出。该行为是模拟器实现事实；硬件操作顺序由设备主计划记录，不把它推广成 PLIC 规范要求。
- 同实现的 claim 使用 threshold 筛选，complete 不检查 enable/领取者；与 PLIC 1.0.0 的 claim 不受 threshold 影响、未 enable 的 complete 可忽略有所不同。当前 threshold=0，软件仍核 generation/sequence/in-service，并遵守先 complete 再 disable，不能利用模拟器宽松行为证明真实硬件正确。
- PLIC 1.0.0 只规定 LW/SW 原子寄存器和非幂等 claim/complete，没有承诺所有平台 MMIO 强序。固定 ISA v20250508 `machine.adoc` 的 Memory-Ordering PMAs 区分强/弱序 IO，`rv32.adoc` 的 FENCE 定义 I/O/R/W 排序。内核显式 `fence iorw,iorw` 排序控制器访问；QEMU 不验证真实 PMA 或设备完成。

## 2026-10-25：xHCI ERST 读取与停止事件取证

主代理亲读固定 QEMU v11.1.2 [`xhci_runtime_write`](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c#L3133)：ERSTBA 高 DWORD 写入直接调用 [`xhci_er_reset`](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c#L806)，它立即经 `xhci->as` 读取 ERST 第一个 segment（L824），校验 segment size 为 16–4096，失败置 HCE（L835–837）。[`xhci_write_event`](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c#L611) 直接写 event ring，`xhci_ring_fetch`（L695）直接从当前 dequeue DMA 读 TRB。结合上文 BME 对 bus master address space 的门控，不能在 BME 关闭时向这个实现发布 ERSTBA，再把 HCH 清除当作成功初始化。实际实现采用完整 pin/CPU 初始化→受限原轮 EnableBusMaster→发布 DMA 寄存器地址/ERSTBA→Run，并核 HCE/HSE。这是固定模拟器的读取时机证据，不表示所有硬件都在 ERSTBA 高 DWORD 写入时缓存表；真实硬件的地址发布亦须在稳定 DMA owner/授权已成立后。

[`xhci_stop_ep`](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c#L1339) 对已连接、NAK 重试中的 interrupt IN 调 `xhci_ep_nuke_xfers(...CC_STOPPED)`，`xhci_ep_nuke_one_xfer`（L1219）同步生成 completion 26 的 Transfer Event，指向 TD 首 TRB；同一命令返回后才写 Command Completion（L2603），故该实现先写停止传输事件再写停止命令完成。Stop Endpoint 发布内部 dequeue/DCS，但不改变其推进事实；[`xhci_set_ep_dequeue`](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c#L1437) 只接受 Stopped，重新建立 ring pointer/cycle。detach 则经 `xhci_detach_slot`（L2432）以 report=0 清重试队列和 timer，HID 同步路径没有迟到异步 callback；之后 Stop→SetDequeue→DisableSlot 仍成功。不能把 QEMU 已丢弃 TD、未产生停止传输事件的拔出差异推广到真实硬件；规范的命令/事件和缓冲寿命要求保持原合同。

## OpenSBI v1.8.1 的逐 hart PLIC 接管

真实客体固件横幅为 OpenSBI v1.8.1。本节依据该固定 tag，不把固件初始化策略当作 PLIC 规范本身。

- [`plic_context_init`](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/utils/irqchip/plic.c#L108-L125) 先写 context enable 数组，再写 threshold；[`plic_warm_irqchip_init`](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/utils/irqchip/plic.c#L192-L221) 用 current_hartindex 查 context_map，对本 hart 的 M/S context 分别调用该函数，普通 virt 的 enable 全零、threshold 为 `0x7`。Ariane workaround 的 enable 例外不适用于本配置。
- [`plic_cold_irqchip_init`](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/utils/irqchip/plic.c#L223-L286) 清 source priority 并注册 warm 回调；cold 函数本身不初始化 context。context 身份来自 [`fdt_irqchip_plic.c`](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/utils/irqchip/fdt_irqchip_plic.c#L20-L94) 的原 DT `interrupts-extended`。
- [`sbi_irqchip_init`](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/sbi/sbi_irqchip.c#L36-L60) 在 cold 时先初始化平台 irqchip，随后 cold/warm 都遍历 warm 回调。boot hart 自己亦执行一次上述 context 初始化；secondary 经 [`init_warm_startup`](https://github.com/riscv-software-src/opensbi/blob/v1.8.1/lib/sbi/sbi_init.c#L409-L449) 在返回 S 态内核之前执行 `sbi_irqchip_init(scratch,false)`。因此内核在 HSM start 前写其他 hart context 的值可能被其固件 warm 初始化覆盖。

本项目据此采用唯一顺序：固件完成本 hart cold/warm 初始化→共同 formal CSR 基线/stvec→本 hart 清自己的 S context enable、threshold=0 并核读回/置 SEIE→发布 Online→全员 Online 后发布 Ready→用户资源激活。全局 Controller 仅建立唯一寄存器 owner 和 slot→context 表；不跨 hart 提前设置 context，不接管 M context，也不在运行期反复重设。该顺序复用原 Online/Ready 屏障。

故障取证 `artifacts/failed-acceptance-20261025-064544-76062.log`：boot hart 1，实际 source33/target slot0；QMP 只读 priority=1、pending/enable 的 bit33=1，但 S context1 threshold=7。未读取有副作用的 claim。修复后的正式路线 `artifacts/qemu-acceptance-20261025-064822-78040.log` 在 boot hart3、同一 target slot0 实际取得 IRQ token，且 IRQ 来源在任务 timer 到期前唤醒；它证明实际中断消费，未宣称所有 hart 同时 idle。

## QEMU v11.1.2 的多键盘输入路由

首批无显示设备，不将 usb-kbd 的 qdev ID 塞入 `input-send-event.device`：该参数由 [ui/input.c](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/ui/input.c#L155) 交 [console 查找](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/ui/console.c#L1235)，表示拥有 QemuConsole 的显示设备；USB keyboard 没有 console。未指定 device 时，[find_handler](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/ui/input.c#L113) 选第一个未绑定 console 的适配 handler，事件只交该一个目标，不广播。

[hid_init](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/input/hid.c#L502) 在键盘 realize 时 register 并 activate；[activate](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/ui/input.c#L68) 把 handler 移到队头，unrealize 经 hid_free 注销。[SET_PROTOCOL](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/hw/usb/dev-hid.c#L631) 只改协议，interrupt poll 和键盘 reset 不重排 handler；SET_IDLE 中的 activate 属于指针设备。这里的“最新”是 QMP device_add 的 realize 顺序，不是客体枚举完成顺序。

实际 A/B/C 场景据此向后插 B 输入，删除 A 后继续向 B 输入，再插 C 后向 C 输入并 Query B 证实没有串改。该规则只用于固定模拟器测试操作者；客体驱动不依赖它，不能推广为 USB 协议或硬件输入路由合同。QAPI 字段说明见 [qapi/ui.json](https://gitlab.com/qemu-project/qemu/-/blob/v11.1.2/qapi/ui.json#L1246)。

## QEMU v11.1.2 的逐hart外部调试推进

固定实现：[gdb_handle_vcont](https://github.com/qemu/qemu/blob/v11.1.2/gdbstub/gdbstub.c#L739-L875)、[gdb_continue_partial](https://github.com/qemu/qemu/blob/v11.1.2/gdbstub/system.c#L553-L598)、[GDB说明](https://github.com/qemu/qemu/blob/v11.1.2/docs/system/gdb.rst)。带具体thread-id的vCont只恢复列出的CPU，未列出的保持暂停；同一cluster中thread-id为CPUindex+1，不能把GDB编号直接当客体PID。断点触发仍是all-stop；普通c或无thread-id的vCont;c会恢复全部CPU。使用scheduler-locking必须核本机GDB实际发出的thread-scoped命令，不能只凭命令名称推定其他hart暂停。

一个hart恢复后全局虚拟时钟仍可推进，其他暂停hart的定时中断可积累；不以这种调试安排证明公平性、响应时间或WCET。默认single-step标志含ENABLE/NOIRQ/NOTIMER，但NOTIMER的消费位于[RR执行器](https://github.com/qemu/qemu/blob/v11.1.2/accel/tcg/tcg-accel-ops-rr.c#L276-L277)，不能外推MTTCG全局时间冻结。调试链可用[gdbstub trace事件](https://github.com/qemu/qemu/blob/v11.1.2/gdbstub/trace-events)核resume/step目标；不改产品调度/中断合同。
