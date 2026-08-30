# 操作系统内核设计参考索引

> 更新基线：2026-08-30。本文是长期维护的资料索引，不是 Halcyon 的设计结论；具体取舍仍须从项目需求独立推导并写入 `notes/ideas/`。
> 表中的实时仓库和网页只用于发现与初筛，不能替代 `normative/` 的固定版本证据。进入设计、编码或 review 前，必须固定上游版本，或另作带版本与取证边界的系统专题报告。

## 口径

- **“主流”有三种含义**：实际商业部署形成的工程主流、学术与高保证领域形成的参照主流、由大型厂商持续投入的产品级内核。三者不混为“市场份额”。
- **年代**优先给出首次公开论文、公开发行或公开仓库时间；无法把内部起源与首次公开可靠分离时，以 `≈` 标记项目起源。具体版本与家族起点不混用，必要时在单元格内并列说明。
- **活跃期**指持续设计与开发的主要时期，不等于产品服役期。`活跃至 2026` 表示 2025–2026 仍可见官方发布或仓库活动；`维护`表示仍有活动但演进较慢。
- **成熟度**区分量产、生产/认证、研究原型、爱好系统和教程；项目自称“production-ready”不自动视作量产事实。
- 清单以“具有独立设计参考价值的系统或框架”为单位，而不是罗列版本。严格微内核会在架构列明确写作“微内核”；框架、分区内核、hypervisor、unikernel、混合内核和单体系统均不计作严格微内核，相关章节也会声明分类边界。
- 许可证按内核或核心仓库概括；多仓库项目的用户态组件可能采用其他许可证。无法从官方许可证文件确认时明确写“未核实”，不以“源码公开”推断授权。专有系统的实现语言若无官方证据则不猜测。
- **深挖优先级**只表示对 Halcyon 当前方向的潜在参考价值：高＝值得形成专题报告；中＝遇到对应主题时调查；低＝了解谱系或反例即可。

本文收录 **107 个参考单元**。它追求广覆盖和可继续调查，而不是声称穷尽所有操作系统。

## 主流系统与年代速览

| 系统 | 主流性质 | 首次公开与主要活跃期 | 2026 状态 | 首要参考价值 |
|---|---|---|---|---|
| QNX / QNX Neutrino | 商业微内核 RTOS | 1982–至今；Neutrino 自 2001 | 活跃、量产 | 同步消息、资源管理器、实时调度与故障隔离 |
| Mach | 历史学术与产业主流 | 1985–1994；后裔延续至今 | 原版停止 | task/thread/port、外部 pager，以及“大微内核”的性能教训 |
| Chorus / ChorusOS | 历史商用分布式微内核 | 1979–2003 | 停止 | 分布式 IPC、动态重配置和电信 RTOS 工程 |
| L4 家族 | 高性能微内核主流谱系 | 1993–至今 | 多支延续 | 最小内核、IPC fast path、机制与策略分离 |
| MINIX 3 | 教学与可靠性研究主流 | 2005–2018 高活跃 | 开放版休眠 | 用户态驱动、服务重启与故障恢复 |
| seL4 | 高保证微内核的事实标准 | 2009–至今；2014 开源 | 活跃 | capability、MCS、形式化验证、静态系统构建 |
| Fiasco.OC + L4Re | L4 工程化与商用安全系统 | 2008–至今 | 活跃 | capability 内核对象、用户态服务框架、虚拟化 |
| NOVA | 微型 hypervisor 代表 | 2010–至今 | 活跃 | 极小 TCB、SMP、虚拟化与 Genode 组合 |
| Zircon / Fuchsia | 大型厂商现代微内核 | ≈2016–至今 | 活跃 | 内核对象、handle rights、异步 channel、VMO、Job |
| INTEGRITY | 商业分离内核/RTOS | 1997–至今 | 活跃、认证量产 | 高保证时空隔离与认证约束 |
| PikeOS | 商业分离内核/hypervisor | 2005–至今 | 活跃、认证量产 | 混合关键性、ARINC 653、RISC-V 虚拟化 |
| LynxSecure / LynxOS-178 | 商业分离内核与分区 RTOS | 2003/2005–至今 | 活跃、认证量产 | 分区调度、MOSA、POSIX 与高保证边界 |
| Genode | 多内核组件 OS 框架，非单一内核 | 2008–至今 | 活跃、产品化 | capability 传播、资源配额、用户态组件体系 |
| GNU Hurd + GNU Mach | 长寿的多服务器微内核系统 | 1990–至今 | 低速活跃 | translator、多服务器组合及资源归因反例 |
| Linux | 模块化单体内核的工程基线 | 1991–至今 | 活跃、广泛量产 | 特权层边界、调度、异步 I/O、驱动与 ABI 的反向对照 |
| Windows NT | 商业混合内核基线 | 1993–至今 | 活跃、广泛量产 | executive、对象/句柄、Job、驱动与 IRQL |
| BSD 家族 | 生产级 Unix 单体内核谱系 | 1993–至今 | 多支活跃 | jail、kqueue、pledge/unveil、rump kernel 与 per-CPU 设计 |
| illumos | Solaris 遗产的生产级单体内核 | 2010–至今；上承 OpenSolaris | 活跃 | DTrace、zones、doors、contracts 与 ZFS |

**历史脉络可粗分为五段**：1969–1984 的 nucleus/capability/分布式奠基；1985–1994 的 Mach/Chorus/Amoeba 与第一轮产业化；1993–2008 的 L4 性能重构和商用 RTOS/分区内核；2009–2018 的验证、capability 与组件化；2015–至今的 Rust、语言安全、异构、多内核及静态系统构建。

## 仓库已有专题覆盖

以下标记用于各表的“已有覆盖”列；它只表示仓库已有专题中出现过该系统，不表示已经完成内核总览分析。

- `IPC`：`ref-2026-08-ipc-contract-research.md`
- `启动`：`ref-2026-08-startup-research.md`
- `终止`：`ref-2026-08-task-termination-research.md`
- `Job`：`ref-2026-08-27-job-enumerate-derive-research.md`
- `引导`：`ref-2026-08-bootstrap-package-userboot-loader.md`
- `NT`：`ref-2026-08-windows-nt-semantics.md`

## A. 微内核主谱系、商业系统与历史基础

| # | 系统 / 家族 | 架构 | 年代与 2026 状态 | 用途 / 成熟度 | 技术 / 许可 | 官方入口与设计资料 | Halcyon 参考主题 / 深挖 | 已有覆盖 |
|---:|---|---|---|---|---|---|---|---|
| 1 | RC 4000 Multiprogramming System | nucleus；微内核思想先驱 | 1969；1970s 后停止 | 工业计算机系统；历史成熟 | 汇编；历史专有 | [系统论文](https://brinch-hansen.net/papers/1970a.pdf) | 进程层级与机制/策略分离的源头 / 中 | 无 |
| 2 | Hydra | capability-based kernel | 1971–1981；停止 | CMU 研究系统 | ALGOL/汇编；许可未核实 | [Hydra: The Kernel](https://dl.acm.org/doi/10.1145/355616.364017) | capability 权限与对象类型、保护域 / 中 | 无 |
| 3 | Thoth | 消息传递实时内核 | 1976–1982；停止 | Waterloo 研究系统；影响 QNX | C/汇编；许可未核实 | [Thoth 论文](https://cs.uwaterloo.ca/research/tr/1979/CS-79-02.pdf) | 同步消息、进程团队、实时内核最小面 / 中 | 无 |
| 4 | Accent | capability + message 微内核；Mach 前身 | 1981–1985；停止 | CMU 研究系统 | C；许可未核实 | [Accent 论文](https://dl.acm.org/doi/10.1145/800216.806593) | 端口、消息和网络透明性；到 Mach 的演化 / 低 | 无 |
| 5 | CMU Mach 3.0 | 二代微内核 | Mach 1985 起源/1986 公开；3.0 于 1990；CMU 主活跃至 1994 | 研究与工业衍生；历史成熟 | C；CMU 宽松许可 | [源码归档](https://www.cs.cmu.edu/afs/cs/project/mach/public/www/sources/sources_top.html) · [Kernel Principles](http://www.shakthimaan.com/downloads/hurd/kernel_principles.pdf) | task/thread/port、用户 pager；IPC 与内核对象膨胀反例 / **高** | IPC、启动 |
| 6 | GNU Mach + GNU Hurd | Mach 上多服务器 OS | 1990–至今；低速活跃，2025 仍有 Debian/Hurd 工作 | 完整但非生产级自由系统 | C，部分新组件 Rust；GPL-2.0+ | [Hurd](https://git.savannah.gnu.org/cgit/hurd/hurd.git/) · [GNU Mach](https://git.savannah.gnu.org/cgit/hurd/gnumach.git/) · [文档](https://www.gnu.org/software/hurd/documentation.html) | translator、用户态服务组合；资源归因和全局策略缺失反例 / **高** | IPC |
| 7 | XNU | Mach + BSD + IOKit 混合内核，**非微内核** | 1996–至今；活跃 | macOS/iOS 量产 | C/C++；APSL-2.0 | [源码](https://github.com/apple-oss-distributions/xnu) · [Mach 概览](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html) | 微内核边界回流单体的工程原因、端口与 VM / 中 | 无 |
| 8 | Chorus / ChorusOS | 分布式微内核、实时 OS | 1979–2003；停止 | 曾用于电信与嵌入式产品 | C/C++；许可未核实 | [Oracle ChorusOS 文档](https://docs.oracle.com/cd/E19048-01/chorus5/) · [综述](https://www.cs.unibo.it/~renzo/so/old/articoli2/RAA+92.pdf) | actor/port、位置透明 IPC、热重启和动态配置 / 中 | 无 |
| 9 | Amoeba | capability + RPC 分布式微内核 | 1981–2000；停止 | 分布式 OS 研究里程碑 | C；许可未核实（源码公开） | [源码与手册](https://www.cs.vu.nl/pub/amoeba/) · [保存镜像](https://github.com/OSPreservProject/amoeba) | capability 对象、IDL/RPC、服务定位与复制 / 中 | 无 |
| 10 | V kernel | 分布式微内核 | 1983–1988；停止 | Stanford 研究系统 | C；许可未核实 | [V Kernel 论文](https://dl.acm.org/doi/10.1145/800217.806609) | 本地/远程 IPC 统一、用户态协议服务 / 低 | 无 |
| 11 | Spring | 面向对象分布式微内核 | ≈1987–1998；停止、未公开源码 | Sun Labs 研究原型 | C++/Cedar/C；未发布 | [Spring Nucleus](https://www.usenix.org/conference/usenix-summer-1993-technical-conference/spring-nucleus-microkernel-objects) | 强类型接口、contracts/subcontracts；OO 复杂度反例 / 低 | 无 |
| 12 | VSTa | 小型类 Unix 微内核 | ≈1991–2004；停止 | 个人/实验系统 | C；GPL（早期许可记录不一） | [项目归档](http://www.vsta.org/) · [源码镜像](https://github.com/JamesLinus/vsta) | POSIX 放在 libc/服务器层的早期实践 / 低 | 无 |
| 13 | KeyKOS | 纯 capability OS、持久化对象系统 | GNOSIS 1975；KeyKOS 1983–1990s；停止 | 商业安全系统 | 汇编/PL/I；源码未公开 | [文档档案](http://www.cap-lore.com/CapTheory/KK/Arch/) · [Nanokernel](https://pdos.csail.mit.edu/6.828/2008/readings/keykos.pdf) | capability、域、持久化、检查点和极小 TCB / 中 | 无 |
| 14 | EROS | KeyKOS 后继 capability 微内核 | 1991–2005；停止 | 研究系统 | C；GPL | [项目存档](https://web.archive.org/web/20031029002231/http://www.eros-os.org/) · [SOSP'99](https://doi.org/10.1145/319151.319163) | 快速 capability invocation、撤销与正交持久化 / 中 | 无 |
| 15 | CapROS | EROS 后继 capability OS | ≈2002–2013；停止 | 研究系统 | C；GPL-2.0 | [源码](https://github.com/capros-os/capros) · [文档](https://www.capros.org/) | capability 分层撤销、可靠系统构造 / 中 | 无 |
| 16 | Coyotos | 面向验证的 capability 微内核 | 2004–2009；未完成、停止 | 研究原型 | BitC/C；GPL-2.0 | [源码镜像](https://github.com/vsrinivas/coyotos) · [项目存档](https://web.archive.org/web/20060203013048/http://www.coyotos.org/) | 验证目标与语言/工具链共同设计的收益和风险 / 低 | 无 |
| 17 | QNX / QNX Neutrino | 同步消息微内核 RTOS | 1982–至今；Neutrino 自 2001，QNX 8 活跃 | 汽车、医疗、工业量产 | 专有 | [系统架构](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/intro_MICROKERNELARCH.html) | Send/Receive/Reply、resource manager、pulse、APS、故障隔离 / **高** | IPC |
| 18 | MINIX 3 | 可靠性导向 multiserver 微内核 | 2005–2018 高活跃；开放版休眠 | 教学与研究；衍生版本曾量产 | C；BSD-3-Clause | [源码](https://github.com/Stichting-MINIX-Research-Institute/minix) · [官网](https://www.minix3.org/) | 用户态驱动、reincarnation server、故障注入和自恢复 / **高** | 无 |
| 19 | L3 | L4 前身、地址空间与 IPC 微内核 | 1988–1993；停止 | GMD 研究系统 | 汇编/C；许可未核实 | [L4 家族历史](https://os.inf.tu-dresden.de/L4/) | 从二代微内核到最小高性能内核的转折 / 中 | 无 |
| 20 | Original L4 | 高性能同步 IPC 微内核 | 1993 原型/1995 公开论文；原版主活跃至 1998，家族延续 | 研究里程碑 | x86 汇编；原版未开源 | [SOSP'95](https://www.sigops.org/Conferences/SOSP/95/p112-liedtke.pdf) · [L4 家族](https://os.inf.tu-dresden.de/L4/) | 最小性原则、rendezvous IPC、fast path 与调度耦合 / **高** | IPC |
| 21 | L4Ka::Pistachio | 可移植 L4 v4 微内核 | 2001–2005；停止 | 研究系统、OKL4 前身 | C++；BSD-2-Clause | [项目与源码](https://www.l4ka.org/projects/pistachio/) | 跨架构组织、寄存器消息和 ABI 演进 / 中 | 无 |
| 22 | Fiasco（经典） | 实时、可抢占 L4 微内核 | 1998–2010；由 Fiasco.OC 继承 | 研究到工程化 | C++；GPL-2.0 | [TUD L4](https://os.inf.tu-dresden.de/L4/) | 实时 L4、内核抢占与 debug 机制，适合作为 Halcyon 反向比较 / 中 | 无 |
| 23 | Fiasco.OC | capability L4 微内核 | 2008–至今；活跃至 2026 | 商业维护的安全/虚拟化基座 | C++；GPL-2.0 | [源码](https://github.com/kernkonzept/fiasco) · [文档](https://l4re.org/fiasco/) | kernel object、capability、IPC、SMP、RISC-V 与虚拟化 / **高** | 无 |
| 24 | L4Re | Fiasco.OC 用户态运行环境，**非内核** | 2008–至今；活跃 | 商业维护的组件框架 | C++；MIT 主许可，组件另有 GPL/Apache 等 | [源码](https://github.com/kernkonzept/l4re-core) · [文档](https://l4re.org/) | 用户态服务、factory、dataspace、名称空间和资源管理 / **高** | IPC |
| 25 | OKL4 | 嵌入式 L4 / microvisor | 2006–2012；开源线停止 | 曾大规模部署于移动设备 | C/C++；开源版与商业许可 | [项目存档](https://web.archive.org/web/20080820043831/http://okl4.org/) · [Microvisor 论文](https://conferences.sigcomm.org/sigcomm/2010/papers/apsys/p19.pdf) | 微内核商业化、Linux guest、移动设备资源约束 / 中 | 无 |
| 26 | Codezero | ARM 嵌入式 L4 微内核 | 2009–2011；停止 | 小型商业/开源尝试 | C；GPL-3.0 | [源码镜像](https://github.com/jserv/codezero) · [Genode 说明](https://genode.org/documentation/platforms/codezero) | 小型 ARM capability kernel 的组织与失败样本 / 低 | 无 |
| 27 | NICTA/UNSW L4 | Pistachio 衍生的嵌入式 L4 支系 | 2005–2010；并入 OKL4/seL4 谱系 | 研究与产业过渡 | C；许可未核实 | [L4.verified 档案](https://trustworthy.systems/projects/OLD/l4.verified/) · [seL4 历史](https://sel4.systems/About/history.html) | 从传统 L4 到可验证 capability kernel 的取舍链 / 中 | 无 |
| 28 | seL4 | capability L4 微内核、形式化验证 | 2009–至今；2014 开源，活跃至 2026 | 高保证生产与研究 | C + Isabelle/HOL；GPL-2.0，生态多为 BSD | [内核](https://github.com/seL4/seL4) · [证明](https://github.com/seL4/l4v) · [文档](https://docs.sel4.systems/) | capability、IPC、MCS、显式资源、证明边界、RISC-V / **高** | IPC、启动、终止、Job、引导 |
| 29 | NOVA | capability microhypervisor，**非 OS 微内核** | 2010–至今；活跃 | 研究内核；有商业维护生态 | C++；GPL-2.0 | [源码](https://github.com/udosteinberg/NOVA) · [EuroSys'10](https://hypervisor.org/eurosys2010.pdf) | 极小 TCB、SMP、虚拟化对象与用户态 VMM / **高** | 无 |
| 30 | L4Linux | L4 上的 Linux personality，**非独立内核谱系** | 1995–至今；维护 | 研究/教学兼容层 | C；GPL-2.0 | [项目](https://l4linux.org/) · [TUD 概览](https://os.inf.tu-dresden.de/L4/LinuxOnL4/overview.shtml) | 微内核上承载完整兼容 personality 的边界和成本 / 中 | 无 |
| 31 | Symbian EKA2 | nanokernel + user servers；微内核式 RTOS | ≈1998–2012；停止 | 手机量产、历史成熟 | C++；历史专有，Symbian 部分曾 EPL | [Symbian Foundation 镜像](https://github.com/SymbianSource/oss.FCL.sf.os.kernelhwsrv) · [体系资料](https://www.symbianos.org/) | nanokernel/OS server 分层、active object、软实时手机工程 / 中 | 无 |
| 32 | INTEGRITY / INTEGRITY-178B | 分离内核/RTOS，**不属传统微内核** | 1997–至今；活跃 | 航空、国防认证量产 | 专有 | [产品](https://www.ghs.com/products/rtos/integrity.html) · [认证](https://www.ghs.com/products/safety_critical/integrity_178_certifications.html) | 时空隔离、MILS、安全认证下的最小 TCB / 中 | 无 |
| 33 | PikeOS | 分离内核 + type-1 hypervisor，**非微内核** | 2005–至今；活跃 | 航空、汽车、铁路认证量产 | 专有 | [产品](https://www.sysgo.com/pikeos) · [RISC-V](https://www.sysgo.com/risc-v) | ARINC 653、混合关键性、原生 RT 与 guest 共存、RISC-V H / **高** | 无 |
| 34 | LynxOS-178 + LynxSecure | 分区 RTOS + 分离内核 hypervisor，**非微内核** | 2003/2005–至今；活跃 | 航空与国防认证量产 | 专有 | [LynxOS-178](https://www.lynx.com/products/lynxos-178-do-178c-certified-posix-rtos) · [LynxSecure](https://www.lynx.com/products/lynxsecure-separation-kernel-hypervisor) | ARINC 653、MOSA、POSIX 分区与管理面边界 / 中 | 无 |
| 35 | VxWorks 653 | ARINC 653 分区 RTOS，**非传统微内核** | ≈2002–至今；活跃 | 航空 IMA 认证量产 | 专有 | [产品概览](https://www.windriver.com/resource/vxworks-653-product-overview) | module/partition OS 分层、健康监控、层级调度 / 中 | 无 |
| 36 | Zircon / Fuchsia | 对象与 handle-rights 微内核 | ≈2016–至今；活跃 | 大型厂商产品级系统 | C++、Rust 用户态；BSD 类混合许可 | [源码](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/) · [内核概念](https://fuchsia.dev/fuchsia-src/concepts/kernel) | object/handle、channel、VMO、port、Job、userboot、生命周期 / **高** | IPC、启动、终止、Job、引导 |
| 37 | HelenOS | 可移植 multiserver 微内核 | 2005–至今；低强度维护，2024 有发行 | 教学/研究系统 | C；BSD-3-Clause | [源码](https://github.com/HelenOS/helenos) · [文档](https://www.helenos.org/wiki/UsersGuide) | 异步 IPC、多平台、用户态驱动和服务拆分 / 中 | 无 |
| 38 | Genode base-hw | Genode 自有小内核；配套组件 OS | ≈2011–至今；活跃 | 商业维护框架中的自有内核 | C++；AGPL-3.0/商业 | [Genode](https://github.com/genodelabs/genode) · [Foundations](https://genode.org/documentation/genode-foundations/index) | capability 委派、资源配额、parent-child 组合、用户态驱动 / **高** | 无 |
| 39 | Genode OS Framework | 多内核组件框架，**非内核** | 2008–至今；季度发布活跃 | 产品与研究 | C++；AGPL-3.0/商业 | [源码](https://github.com/genodelabs/genode) · [文档](https://genode.org/documentation/) | 在 seL4/NOVA/Fiasco.OC/base-hw 间比较同一用户态模型 / **高** | 无 |

## B. 现代研究、新兴与爱好系统

这些项目的目标差异很大。将它们放在一起是为了发现新结构和可读源码，不代表成熟度或安全性相当。

| # | 系统 | 架构 | 年代与 2026 状态 | 用途 / 成熟度 | 技术 / 许可 | 官方入口与设计资料 | Halcyon 参考主题 / 深挖 | 已有覆盖 |
|---:|---|---|---|---|---|---|---|---|
| 40 | Redox | Rust 微内核 + 用户态 scheme 服务 | 2015–至今；活跃、alpha | 通用 OS 与爱好/研究 | Rust；MIT | [源码](https://gitlab.redox-os.org/redox-os/redox) · [Book](https://doc.redox-os.org/book/) | Rust no_std、用户态驱动、URL/scheme 服务命名、完整 OS 集成 / **高** | 无 |
| 41 | managarm | 异步 multiserver 微内核 | ≈2014–至今；活跃至 2026 | 通用桌面/服务器爱好系统 | C++20；MIT | [源码](https://github.com/managarm/managarm) · [设计说明](https://managarm.org/) | 全异步 I/O、POSIX/Linux ABI、用户态驱动与协议生成 / **高** | 无 |
| 42 | Theseus | 单地址空间、语言内隔离，**非微内核** | ≈2015–至今；研究活跃 | 研究 OS，明确未成熟 | Rust；MIT | [源码](https://github.com/theseus-os/Theseus) · [设计文档](https://www.theseus-os.com/Theseus/book/design/design.html) | cell、运行时替换、编译期不变量、无特权边界组件化 / **高** | 无 |
| 43 | Twizzler | object-capability、持久对象 OS | ≈2019–至今；研究活跃 | 研究 OS | Rust/C；BSD-3-Clause | [源码](https://github.com/twizzler-operating-system/twizzler) · [官网](https://twizzler.io/) | 全局对象、持久化、跨进程共享与 object capability / **高** | 无 |
| 44 | RedLeaf | 语言安全隔离域 OS | 2019–2021 主研究期；仓库低活跃 | 研究原型 | Rust；未提供顶层许可证 | [源码](https://github.com/mars-research/redleaf) · [OSDI'20](https://www.usenix.org/conference/osdi20/presentation/narayanan-vikram) | typed interface、零拷贝、语言隔离及 unsafe TCB / 中 | 无 |
| 45 | Asterinas | framekernel：小 unsafe framework + safe Rust 服务，**非微内核** | ≈2022–至今；活跃至 2026 | Linux ABI 通用 OS，研究/工程快速发展 | Rust；MPL-2.0 | [源码](https://github.com/asterinas/asterinas) · [文档](https://asterinas.github.io/) | framekernel 边界、safe Rust 内核服务、Linux ABI 与 RISC-V / **高** | 无 |
| 46 | Hubris | 内存保护、消息传递嵌入式 OS | 2020–至今；活跃 | Oxide 设备固件生产使用 | Rust；MPL-2.0 | [源码](https://github.com/oxidecomputer/hubris) · [设计演讲/文档](https://hubris.oxide.computer/) | 静态任务图、同步 IPC、MPU/PMP、故障即重启、构建期资源分配 / **高** | 无 |
| 47 | Tock | capability-oriented 嵌入式内核，非严格微内核 | 2015–至今；活跃 | 嵌入式研究与部署 | Rust；Apache-2.0 OR MIT | [源码](https://github.com/tock/tock) · [Book](https://book.tockos.org/) | capsule、grant、异步 syscall、MPU/PMP、Rust 内核边界 / **高** | 无 |
| 48 | RIOT | 小型模块化 IoT RTOS；常称 microkernel-like | 2013–至今；活跃 | 物联网生产/研究 | C/C++；LGPL-2.1 | [源码](https://github.com/RIOT-OS/RIOT) · [文档](https://doc.riot-os.org/) | 极小设备上的线程、IPC、网络栈与模块边界 / 中 | 无 |
| 49 | Zephyr | 模块化单体 RTOS，**非微内核** | 2015–至今；活跃 | 大规模嵌入式生态 | C/Rust 组件；Apache-2.0 | [源码](https://github.com/zephyrproject-rtos/zephyr) · [架构文档](https://docs.zephyrproject.org/latest/kernel/services/index.html) | 配置生成、驱动模型、对象权限、SMP/异构工程；作为规模反例 / 中 | 无 |
| 50 | Motor OS | Rust cloud/VM OS | ≈2022–至今；活跃 | 新兴爱好/工程项目 | Rust；Apache-2.0 | [源码](https://github.com/moturus/motor-os) · [官网](https://moturus.com/) | VM-first、异步运行时、Rust 用户/内核边界 / 中 | 无 |
| 51 | Maestro | Rust Unix-like 内核，**单体** | ≈2023–至今；活跃 | 新兴爱好系统 | Rust；GPL-3.0 | [源码](https://github.com/maestro-os/maestro) | 可读的 Rust Unix 内核、Linux ABI；用于比较而非照搬 / 低 | 无 |
| 52 | Kerla | Rust Linux ABI 内核，**单体** | 2021–2022 主活跃期；休眠 | 爱好/实验 | Rust；Apache-2.0/MIT | [源码](https://github.com/nuta/kerla) | 小型 Rust 内核如何承载 Linux 用户程序 / 低 | 无 |
| 53 | Aero | Rust 类 Unix 内核，**单体** | 2021–2024 主活跃期；低活跃 | 爱好系统 | Rust；GPL-3.0 | [源码](https://github.com/Andy-Python-Programmer/aero) | x86_64 驱动、ELF、SMP 的可读实现 / 低 | 无 |
| 54 | axle | 小型图形化 OS，**单体/爱好** | ≈2016–2023 主活跃期 | 爱好系统 | C/汇编；BSD-2-Clause | [源码](https://github.com/codyd51/axle) · [博客](https://axleos.com/) | 从零构建 GUI、驱动和用户态边界的可读案例 / 低 | 无 |
| 55 | VeridianOS | capability-oriented Rust 微内核 | ≈2020–至今；早期开发 | 新兴研究/爱好系统 | Rust；MIT OR Apache-2.0 | [源码](https://github.com/doublegate/VeridianOS) | x86_64/AArch64/RISC-V、多架构 capability 设计草案 / 中 | 无 |
| 56 | Hadron | capability-based Rust 微内核 | ≈2021–至今；早期开发 | 新兴爱好/研究 | Rust；GPL-3.0 | [源码](https://github.com/asterism-labs/hadron) | 小型 capability kernel 的现代 Rust 表达；成熟度需持续复核 / 中 | 无 |
| 57 | SerenityOS | 完整桌面 OS，**单体** | 2018–至今；活跃 | 高质量爱好系统 | C++；BSD-2-Clause | [源码](https://github.com/SerenityOS/serenity) · [开发文档](https://github.com/SerenityOS/serenity/tree/master/Documentation) | 完整用户态、浏览器、驱动和 ABI 工程；非微内核对照 / 中 | 无 |
| 58 | Haiku | BeOS 风格混合/模块化单体内核 | 2001–至今；活跃 | 可日用开源桌面 OS | C/C++；MIT | [源码](https://github.com/haiku/haiku) · [开发文档](https://www.haiku-os.org/development/) | server 化 GUI、端口 IPC、驱动与桌面系统整合 / 中 | 无 |
| 59 | 9front | Plan 9 活跃分支；**单体内核** | 2010–至今；活跃 | 小众但完整可用 | C；MIT | [源码](https://git.9front.org/plan9front/plan9front/HEAD/info.html) · [FQA](https://fqa.9front.org/) | per-process namespace、9P、用户态文件服务与系统一致性 / **高** | 无 |
| 60 | ToaruOS | Unix-like 图形 OS，**单体** | 2010–至今；维护/活跃 | 爱好系统 | C；NCSA | [源码](https://github.com/klange/toaruos) · [文档](https://toaruos.org/) | 小而完整的用户态、合成器、网络与工具链 / 低 | 无 |
| 61 | Sortix | 独立 Unix-like OS，**单体** | ≈2011–至今；维护 | 爱好/实验系统 | C/C++；ISC | [源码](https://gitlab.com/sortix/sortix) · [官网](https://sortix.org/) | 自有 libc、POSIX 契约和完整自举 / 低 | 无 |
| 62 | Mezzano | Common Lisp OS，**单地址空间/非微内核** | ≈2014–至今；低活跃 | 语言系统实验 | Common Lisp；MIT | [源码](https://github.com/froggey/Mezzano) | 动态语言运行时与 OS 合一、交互调试 / 中 | 无 |
| 63 | MikanOS | UEFI/x86-64 教学爱好 OS，**单体** | ≈2019–至今；教材生态活跃 | 教程/爱好 | C++；Apache-2.0 | [源码](https://github.com/uchan-nos/mikanos) | 现代固件、图形和设备初始化的清晰实现 / 低 | 无 |
| 64 | rCore-Tutorial-v3 | Rust 教学内核，**非产品** | ≈2020–至今；教材维护 | 教程 | Rust；GPL-3.0 | [源码](https://github.com/rcore-os/rCore-Tutorial-v3) · [教程](https://rcore-os.cn/rCore-Tutorial-Book-v3/) | RISC-V/Rust 教学表达；只作机制入门，不作成熟设计证据 / 低 | 无 |
| 65 | ArceOS | 组件化 unikernel，**非微内核** | ≈2023–至今；活跃 | 研究/工程框架 | Rust；Apache-2.0/MulanPSL-2.0 | [源码](https://github.com/rcore-os/arceos) · [文档](https://arceos.org/) | Rust 组件化、trait 驱动的平台/设备抽象、unikernel 组合 / 中 | 无 |
| 66 | Blog OS | Rust 教学内核 | 2015–2022 主教程期；维护 | 教程 | Rust；MIT/Apache-2.0 | [源码](https://github.com/phil-opp/blog_os) · [教程](https://os.phil-opp.com/) | x86_64/Rust 入门和测试框架；不作为系统架构参照 / 低 | 无 |
| 67 | intermezzOS | Rust 教学/实验内核 | ≈2015–2018；停止 | 教程原型 | Rust；MIT | [源码](https://github.com/intermezzOS/kernel) | 早期 Rust OS 工具链历史和失败样本 / 低 | 无 |

## C. 邻近范式与验证系统

这部分不是“微内核候选清单”，而是对 Halcyon 的机制、边界和反例有价值的邻近系统。除 M3 等明确采用微内核基座的系统外，本节条目均不视为严格微内核；架构列给出其实际类别。

| # | 系统 | 架构 | 年代与 2026 状态 | 用途 / 成熟度 | 技术 / 许可 | 官方入口与设计资料 | Halcyon 参考主题 / 深挖 | 已有覆盖 |
|---:|---|---|---|---|---|---|---|---|
| 68 | MIT Exokernel（Aegis/Xok/ExOS） | exokernel + library OS | 1994–2000；停止 | 研究里程碑 | C；许可未核实 | [项目归档](https://pdos.csail.mit.edu/archive/exo/) · [SOSP'95](https://doi.org/10.1145/224057.224076) | 安全复用裸资源、visible revocation、用户态策略 / **高** | 无 |
| 69 | Nemesis | vertically structured library OS | 1993–1998；停止 | Cambridge 研究原型 | C；许可未核实 | [项目归档](https://www.cl.cam.ac.uk/research/srg/netos/projects/archive/nemesis/) · [论文](https://www.cl.cam.ac.uk/research/srg/netos/papers/1997-jsac.pdf) | QoS 资源记账、domain、调度器激活和干扰控制 / **高** | 无 |
| 70 | SPIN | 类型安全可扩展单体内核 | 1993–1999；停止 | 研究原型 | Modula-3；许可未核实 | [项目](https://www-spin.cs.washington.edu/) · [SOSP'95](https://doi.org/10.1145/224057.224077) | 语言安全内核扩展和动态专用化 / 中 | 无 |
| 71 | Synthesis | 运行时代码合成单体内核 | 1986–1994；停止 | 研究性能标杆 | C/汇编；许可未核实 | [论文](https://dl.acm.org/doi/10.5555/143219) | factoring invariants、层消除和运行时特化 / 低 | 无 |
| 72 | Scout | path-oriented communication OS | 1994–2000；停止 | 研究原型 | C；许可未核实 | [项目](https://www.cs.arizona.edu/projects/scout/) · [OSDI'96](https://www.usenix.org/legacy/publications/library/proceedings/osdi96/full_papers/mosberger/index.html) | 端到端 path、按路径资源归属和调度 / 中 | 无 |
| 73 | Barrelfish | multikernel | 2009–2020；项目停止 | 多核研究里程碑 | C；MIT | [源码](https://github.com/BarrelfishOS/barrelfish) · [SOSP'09](https://www.barrelfish.org/publications/barrelfish_sosp09.pdf) | shared-nothing、多核消息、状态复制和异构 / **高** | 无 |
| 74 | Helios | satellite-kernel 异构 OS | 2009；研究期短、停止 | 研究原型 | Sing#/C#；许可未核实 | [SOSP'09](https://www.microsoft.com/en-us/research/publication/helios-heterogeneous-multiprocessing-with-satellite-kernels/) | 异构设备 placement、远程消息和 satellite kernel / 中 | 无 |
| 75 | M3 | microkernel-based heterogeneous manycore OS | 2016–至今；研究维护 | 研究原型 | Rust/C++；GPL-2.0 | [源码](https://github.com/Barkhausen-Institut/M3) · [ASPLOS'16](https://dl.acm.org/doi/10.1145/2872362.2872371) | capability、TCU/DTU、专用 kernel core、异构资源 / **高** | 无 |
| 76 | Hive | multicellular kernel | 1994–1997；停止 | 研究原型 | C；许可未核实 | [SOSP'95](https://doi.org/10.1145/224056.224059) | fault containment cell、共享内存上的分布式内核 / 中 | 无 |
| 77 | Corey | exokernel-style multicore OS | 2008–2010；停止 | 研究原型 | C；许可未核实 | [项目](https://pdos.csail.mit.edu/archive/corey/) · [OSDI'08](https://www.usenix.org/legacy/events/osdi08/tech/full_papers/boyd-wickizer/boyd_wickizer.pdf) | 应用控制共享、kernel core 与多核扩展 / 中 | 无 |
| 78 | Composite | component-based OS | 2007–至今；研究低活跃 | 研究原型 | C；GPL-2.0/混合 | [源码](https://github.com/gparmer/Composite) · [项目](https://composite.seas.gwu.edu/) | 调度器/内存管理用户化、组件图、fault recovery、无锁路径 / **高** | 无 |
| 79 | K42 | object-oriented server kernel | 1996–2009；停止 | IBM 研究系统 | C++；LGPL | [源码镜像](https://github.com/jimix/k42) · [综述](https://www.usenix.org/legacy/events/osdi06/work_in_progress/krieger.pdf) | per-object concurrency、热替换、避免全局锁 / 中 | 无 |
| 80 | Singularity | 语言安全组件 OS | 2003–2008；停止 | Microsoft Research | Sing#；许可未核实 | [项目](https://www.microsoft.com/en-us/research/project/singularity/) · [SOSP'05](https://www.microsoft.com/en-us/research/publication/singularity-rethinking-the-software-stack/) | SIP、channel contract、静态所有权和软件隔离 / **高** | 无 |
| 81 | Midori | 语言安全微内核式系统 | ≈2008 外部披露；2008–2015 内部开发；停止、未开源 | Microsoft 内部原型/部署 | M#/C#；专有 | [设计回顾](https://joeduffyblog.com/2015/11/03/blogging-about-midori/) | capability、全异步、语言/运行时/OS 协同；证据受限 / 中 | 无 |
| 82 | Plan 9 | namespace-oriented 分布式 OS，**单体内核** | 1987 起源/1992 首次外部发布；官方主活跃至 2015，9front 延续 | 历史成熟、小众使用 | C；MIT | [源码与论文](https://9p.io/plan9/) · [系统论文](https://9p.io/sys/doc/9.pdf) | per-process namespace、9P、文件服务和统一命名 / **高** | 启动 |
| 83 | Inferno | Dis VM + Limbo + namespace OS | 1995–2010s；低活跃 | 曾商用、现研究/爱好 | Limbo/C；GPL/LGPL/商业历史许可 | [源码](https://github.com/inferno-os/inferno-os) · [官网](https://www.vitanuova.com/inferno/) | 类型安全并发、可移植 VM、9P namespace / 中 | 无 |
| 84 | Arrakis | control/data-plane split OS | 2014–2017；停止 | 研究原型 | C；MIT | [源码](https://github.com/UWNetworksLab/arrakis) · [OSDI'14](https://www.usenix.org/conference/osdi14/technical-sessions/presentation/peter) | 用户态直接 I/O、控制面短路径、设备虚拟化 / **高** | 无 |
| 85 | IX | protected dataplane OS | 2014–2016；停止 | 研究原型 | C；MIT 类许可 | [源码](https://github.com/ix-project/ix) · [OSDI'14](https://www.usenix.org/conference/osdi14/technical-sessions/presentation/belay) | 批处理、run-to-completion、VMX 隔离与零拷贝 / 中 | 无 |
| 86 | Demikernel | kernel-bypass library OS | 2019–至今；低速维护 | 研究原型 | Rust/C；MIT | [源码](https://github.com/microsoft/demikernel) · [SOSP'21](https://www.microsoft.com/en-us/research/publication/demikernel-an-operating-system-architecture-for-kernel-bypass/) | 统一异步 I/O、队列 token、Rust 内存安全 / **高** | 无 |
| 87 | Unikraft | modular library OS / unikernel | 2017–至今；活跃 | 活跃开源项目与商业生态 | C；BSD-3-Clause | [源码](https://github.com/unikraft/unikraft) · [EuroSys'20](https://dl.acm.org/doi/10.1145/3342195.3387529) | 组件裁剪、构建配置、最小镜像和快速启动 / 中 | 无 |
| 88 | MirageOS | typed library OS / unikernel | 2013–至今；活跃 | 研究与工程使用 | OCaml；ISC | [源码](https://github.com/mirage/mirage) · [EuroSys'13](https://dl.acm.org/doi/10.1145/2465351.246538) | 类型化协议栈、模块组合、无通用内核的边界 / 中 | 无 |
| 89 | IncludeOS | C++ library OS / unikernel | 2014–至今；低活跃 | 研究/工程原型 | C++；Apache-2.0 | [源码](https://github.com/includeos/IncludeOS) · [文档](https://includeos.github.io/) | 链接期裁剪、live update、C++ 系统组件 / 低 | 无 |
| 90 | OSv | 单应用 Linux ABI unikernel | 2013–至今；社区低活跃 | 曾生产试用 | C++；BSD-3-Clause/混合 | [源码](https://github.com/cloudius-systems/osv) · [Wiki](https://github.com/cloudius-systems/osv/wiki) | Linux ABI、单地址空间和管理面 / 低 | 无 |
| 91 | Rumprun | NetBSD rump-kernel unikernel | 2014–2020；停止 | 研究/演示 | C；BSD | [源码](https://github.com/rumpkernel/rumprun) · [rump kernel](https://rumpkernel.org/) | 复用成熟驱动/文件系统组件的边界 / 低 | 无 |
| 92 | Solo5 | unikernel execution environment，**非内核** | 2015–至今；活跃 | MirageOS 等生产基座 | C；ISC | [源码](https://github.com/Solo5/solo5) · [架构](https://github.com/Solo5/solo5/blob/main/docs/architecture.md) | tender、极小 host ABI、沙箱边界 / 中 | 无 |
| 93 | Nanos | 单应用 unikernel | 2016–至今；商业维护 | 厂商提供云端部署工具链 | C；Apache-2.0 | [源码](https://github.com/nanovms/nanos) · [文档](https://docs.ops.city/) | 极小攻击面、云镜像和单进程语义 / 低 | 无 |
| 94 | Quest-V | virtualized multikernel / separation kernel | ≈2010–至今；学术维护 | 混合关键性研究 | C；GPL-3.0 | [源码](https://github.com/QuestOS/quest) · [论文](https://arxiv.org/abs/1310.6349) | sandbox-per-core、VCPU、故障隔离和无中央 hypervisor / 中 | 无 |
| 95 | Bao | 静态分区 hypervisor | 2019–至今；活跃 | 研究系统；项目称有工业采用 | C；Apache-2.0 | [源码](https://github.com/bao-project/bao-hypervisor) · [官网](https://bao-project.org/) | RISC-V 静态分区、无调度器、直通 I/O、缓存着色 / **高** | 无 |
| 96 | Jailhouse | Linux-assisted partitioning hypervisor | 2013–至今；活跃 | 工业与实时系统 | C；GPL-2.0 | [源码](https://github.com/siemens/jailhouse) · [README/文档](https://github.com/siemens/jailhouse/tree/master/Documentation) | root cell 引导、静态 cell、Linux 与实时域共存 / 中 | 无 |
| 97 | XtratuM | ARINC 653 para-virtual hypervisor | ≈2010–至今；开源低活跃、商业延续 | 航天/研究 | C；GPL-2.0 | [源码镜像](https://github.com/lfd/XtratuM) · [产品](https://www.fentiss.com/xtratum/) | 固定循环分区调度、health monitor 和静态配置 / 中 | 无 |
| 98 | Tessellation | cell-based partitioned OS | 2009–2013；停止 | Berkeley 研究原型 | C；许可未核实 | [项目](https://tessellation.cs.berkeley.edu/) · [HotPar'09](https://www.usenix.org/legacy/event/hotpar09/tech/full_papers/liu/liu.pdf) | 两级调度、cell 的时空资源隔离和 QoS / **高** | 无 |
| 99 | Muen | SPARK separation kernel | 2014–至今；活跃/维护 | 高保证研究与工程 | Ada/SPARK；GPL-3.0 | [源码](https://github.com/codelabs-ch/muen) · [规范](https://muen.sk/muen-kernel-spec.pdf) | 策略生成、静态系统、SPARK 证明与分区隔离 / **高** | 无 |
| 100 | CertiKOS / mCertiKOS | 分层可组合验证内核 | 2010–2021 主研究期；低活跃 | 学术验证原型 | C/汇编 + Coq；许可未核实 | [项目](https://flint.cs.yale.edu/certikos/) · [源码组织](https://github.com/CertiKOS) | 深规范、层化精化、并发内核证明 / **高** | 无 |

## D. 验证与静态系统构建工具链补充

这些项目不计入编号的 107 个“系统参考单元”，但调查 seL4、静态系统和低成本验证时应成组阅读。

| 项目 | 性质 | 活跃期 / 状态 | 入口 | 参考价值 |
|---|---|---|---|---|
| CAmkES | seL4 组件系统框架，非内核 | ≈2010–至今；活跃 | [源码](https://github.com/seL4/camkes-tool) · [文档](https://docs.sel4.systems/projects/camkes/) | ADL、connector、生成式 glue code、静态组件图 |
| seL4 Microkit | seL4 静态系统 SDK，非内核 | 2019–至今；活跃 | [源码](https://github.com/seL4/microkit) · [文档](https://docs.sel4.systems/projects/microkit/) | 声明式系统描述、静态资源初始化、极小运行时 |
| Hyperkernel | SMT 驱动内核验证研究 | 2017–2019；停止 | [源码](https://github.com/uw-unsat/hyperkernel) · [SOSP'17](https://syslab.cs.washington.edu/papers/nelson-hyperkernel.pdf) | push-button syscall 验证及其规格限制 |
| Serval | Rosette 符号执行验证框架 | 2019–2021；低活跃 | [源码](https://github.com/uw-unsat/serval) · [SOSP'19](https://jamesbornholt.com/papers/serval-sosp19.pdf) | 为已有系统构建验证器、组合符号执行 |
| Verve | 自动验证类型安全 OS | 2010–2012；停止 | [PLDI'10](https://www.microsoft.com/en-us/research/publication/safe-to-the-last-instruction-automated-verification-of-a-type-safe-operating-system/) | 从汇编到类型安全运行时的验证链 |
| Ironclad Apps | 全栈自动验证方法，非内核 | 2014–2016；停止 | [源码](https://github.com/microsoft/ironclad) · [项目](https://www.microsoft.com/en-us/research/project/ironclad/) | 应用—OS—硬件端到端规格 |
| Komodo | TrustZone 验证 reference monitor，非内核 | 2017–2020；归档 | [源码](https://github.com/microsoft/Komodo) · [项目](https://www.microsoft.com/en-us/research/project/komodo/) | 小 TCB enclave 语义和机器检查 |
| Keystone | RISC-V TEE 框架，非微内核 | 2019–至今；研究维护 | [源码](https://github.com/keystone-enclave/keystone) · [文档](https://docs.keystone-enclave.org/) | M/S/U/PMP 边界、attestation 与 security monitor |

## E. 主流非微内核基线

这些系统构成口径中“工程主流”的实际部署主体，全部不是严格微内核：Linux 与 BSD 家族是（模块化）单体，Windows NT 是混合内核，illumos 是 Solaris 遗产的单体。它们对 Halcyon 的价值在反例与基线对照——单体/混合内核把哪些机制放进特权层、代价与演化路径是什么；架构列给出实际类别，不因“常用”而含糊。Windows NT 无公开内核源码，明确标为专有。

| # | 系统 | 架构 | 年代与 2026 状态 | 用途 / 成熟度 | 技术 / 许可 | 官方入口与设计资料 | Halcyon 参考主题 / 深挖 | 已有覆盖 |
|---:|---|---|---|---|---|---|---|---|
| 101 | Linux | 模块化单体内核（单体内核 + 可加载模块） | 1991 起源/公开；活跃至 2026 | 工程主流；服务器、嵌入式与桌面广泛量产 | C，Rust 基础设施已入主线；GPL-2.0 | [git.kernel.org](https://git.kernel.org/)（torvalds/linux.git）· [官方内核文档](https://docs.kernel.org/) | syscall/ABI 面、模块机制 vs 用户态服务、Rust 组件边界、EEVDF 调度、io_uring 异步 I/O / **高** | 启动、终止、Job |
| 102 | Windows NT | 混合内核（executive 驻核心态） | 1988–1989 起源；NT 3.1 于 1993 首发；活跃至 2026 | 工程主流；桌面与服务器广泛量产 | C/C++；专有，无公开内核源码 | [WDK 内核文档](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/) · [Windows Internals](https://learn.microsoft.com/en-us/sysinternals/resources/windows-internals) · [架构概述](https://learn.microsoft.com/en-us/previous-versions/cc750820(v=technet.10)) | 混合架构取舍、对象/句柄与 Job 会计、WDK 驱动框架、IRQL 同步模型 / **高** | NT、Job |
| 103 | FreeBSD | 模块化单体内核（内核核心 + KLD 可加载模块） | 起源 1993（386BSD patchkit 团队）；1.0 首发 1993-11；2.0 转 4.4BSD-Lite 基线（1994）；活跃至 2026 | 工程主流；服务器/网络设备/存储量产 | C；BSD-2-Clause | [cgit.freebsd.org/src](https://cgit.freebsd.org/src/) · GitLab [freebsd-src](https://gitlab.com/FreeBSD/freebsd-src) · [Architecture Handbook](https://docs.freebsd.org/en/books/arch-handbook/) | 模块边界与 KLD、kqueue 事件机制、jail 分区、SMPng 同步、new-bus 驱动框架 / 中 | 无 |
| 104 | OpenBSD | 单体内核（安全加固、模块化从简） | 1995 自 NetBSD 分叉；2.0 于 1996 首发；活跃至 2026 | 服务器、防火墙与安全设备的生产使用 | C；ISC/BSD 等 | [cvsweb.openbsd.org](https://cvsweb.openbsd.org/) · GitHub [openbsd/src](https://github.com/openbsd/src)（只读镜像）· [官网](https://www.openbsd.org/) · [官方论文](https://www.openbsd.org/papers/) | pledge/unveil 权限收缩 vs capability、特权分离、最小攻击面与审计文化 / **高** | 无 |
| 105 | NetBSD | 单体内核（可加载模块；可移植性优先） | 1993 起源；0.8 于同年首发；活跃至 2026 | 嵌入式、旧硬件与网络设备的生产使用 | C；BSD-2-Clause | [netbsd.org](https://www.netbsd.org/) · GitHub [NetBSD/src](https://github.com/NetBSD/src)（只读镜像）· [Guide](https://www.netbsd.org/docs/guide/) · [man 9](https://man.netbsd.org/intro.9) | 可移植性分层、rump kernel、ABI 稳定性与调度器 / 中 | 无 |
| 106 | DragonFly BSD | 单体内核（消息传递式内核线程 + per-CPU 重构） | 2003 自 FreeBSD 分叉；1.0 于 2004 首发；活跃至 2026 | 小众工作站、服务器与存储系统 | C；BSD-3-Clause | [gitweb.dragonflybsd.org](https://gitweb.dragonflybsd.org/) · GitHub [DragonFlyBSD](https://github.com/DragonFlyBSD/DragonFlyBSD)（只读镜像）· [Handbook](https://www.dragonflybsd.org/docs/handbook/) · [HAMMER/HAMMER2](https://wiki.dragonflybsd.org/hammer/) | 内核内消息传递 vs 微内核 IPC、per-CPU 调度、HAMMER2 与 token 锁 / 中 | 无 |
| 107 | illumos | 单体内核（unix/genunix/可加载模块三层；Solaris 遗产） | 起源 OpenSolaris 开源 2005-06；illumos 公告 2010-08-03；滚动发布；活跃至 2026 | 生产（云/存储：SmartOS、OmniOS、Nexenta、Delphix）；量产 | C；CDDL-1.0 | [code.illumos.org](https://code.illumos.org/plugins/gitiles/illumos-gate)（官方）· GitHub [illumos-gate](https://github.com/illumos/illumos-gate)（只读镜像）· [illumos 文档](https://illumos.org/docs/) · [Writing Device Drivers](https://www.illumos.org/books/wdd/) | DTrace 可观测性、zones 分区 vs capability 边界、door IPC（内核内同步 RPC）、contracts 监督、ZFS 设计 / **高** | 无 |

> 证据边界：Windows NT 无公开的现代内核源码，体系信息以微软 WDK 与 Windows Internals 为主；illumos 采用滚动开发；BSD 家族各文件可能混用多种宽松许可证，表中只概括核心仓库的主要许可。

## 建议的首批深度报告候选

清单阶段不替 Halcyon 作方向选择。若后续按“能提供新设计信息，而非只确认已有直觉”排序，首批候选可分为：

1. **直接相关系统与运行时**：QNX Neutrino、seL4、Fiasco.OC/L4Re、Zircon、MINIX 3、NOVA。
2. **能力与系统构造**：KeyKOS→EROS/CapROS 与 L4→seL4 两支 capability 对照、Genode、Twizzler、CAmkES/Microkit。
3. **协作式、资源与调度**：Nemesis、Composite、Tessellation、Hubris。
4. **多核与异构**：Barrelfish、M3、Bao。
5. **Rust 与语言安全**：Theseus、Asterinas、Tock、RedLeaf、Redox。
6. **反例和历史教训**：Mach、GNU Hurd、XNU、Singularity、OKL4。

每份深度报告至少应回答：内核最小承诺、对象与授权模型、IPC/通知/共享内存、调度与资源记账、地址空间与回收、用户态服务拓扑、启动与故障恢复、SMP/异构、验证边界、已知失败和对 Halcyon 的可迁移/不可迁移部分。
