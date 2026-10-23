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
- QEMU virt 采用 96 个源、优先级 0—7；具体 context 从 DT hart/context 对应关系确定，不以 hart 编号直接猜测。

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
- QEMU [dev-hid.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/dev-hid.c)、[hid-core.c](https://github.com/qemu/qemu/blob/v11.1.0/hw/input/hid-core.c)实现 `usb-kbd` 与对应控制请求。首批只承诺 boot keyboard 输入；通用 HID report descriptor、其他设备类别和打印协议另行扩展。

## 热插控制与证据边界

固定来源：[USB 设备说明](https://github.com/qemu/qemu/blob/v11.1.0/docs/system/devices/usb.rst)、[QMP qdev schema](https://github.com/qemu/qemu/blob/v11.1.0/qapi/qdev.json)、[USB bus](https://github.com/qemu/qemu/blob/v11.1.0/hw/usb/bus.c)。固定控制器在启动时装入；运行期通过 QMP `device_add`/`device_del` 在 `xhci.0` 加减 `usb-kbd`。这验证 USB 设备热插，不声称 PCI host/function 热插已经实现。通用 `device_del` 的成功回复不自动等于删除完成，验收需观察 QMP 事件与客体登记/绑定终态，不用主机命令返回代替客体收束。

最终停止还依赖 [SBI 3.0 System Reset](../references/normative/riscv-sbi-v3.0/src/ext-sys-reset.adoc)：成功 reset 不返回；失败可返回错误。Cold reboot 表示完整复位，不能以 Warm reboot 替代所需硬件复位；提交 reset 本身不证明此前已安全退款。CPU/I/O 顺序与跨 hart 页表失效按[固定 ISA 索引](../references/CONTRACTS.md#risc-v-isa-与特权架构)，不将 FENCE 当成 cache flush 或设备完成。
