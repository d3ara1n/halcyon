# 进程启动资源交付外部调研（Fuchsia / Linux / seL4 / Mach / Plan 9）

> 用途：为 eRhino 的进程启动资源交付 A/B/C/D 决策提供外部事实。本文只记录成熟系统的实际契约、内核感知边界与扩展方式，不替 eRhino 选择方案。
>
> 取证原则：优先官方 ABI、官方手册与一方源码；实现细节以所列版本为准。Fuchsia 的 `processargs.h` 是当前源码快照，Linux 使用 man-pages，seL4 使用官方教程/接口资料，Apple 使用 Apple man page 与 XNU 一方源码，Plan 9 使用原生手册。

## 1. Fuchsia Zircon：processargs 是启动清单 + Handle 表 + 用户态命名空间

### 官方出处

- `zx_process_start` / 进程装载概念：
  [Fuchsia program loading](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/docs/concepts/process/program_loading.md)
- 协议常量与消息结构：
  [`zircon/processargs.h`](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/public/zircon/processargs.h)
- 构造、校验与解析实现：
  [`process_args.rs`](https://fuchsia.googlesource.com/fuchsia/+/389072e9e82220acd584ac21149fdadb4972c24f/src/lib/process_builder/src/process_args.rs)、
  [`fuchsia-runtime`](https://fuchsia.googlesource.com/fuchsia/+/31644b052a2fa994a75df0b33eb2e1176d0b2f83/src/lib/fuchsia-runtime/src/lib.rs)

### 交付形状

`zx_process_start` 的启动参数包含入口地址、初始栈指针、一个启动 Handle（bootstrap channel）以及入口参数；新进程从该 channel 读取 `processargs` 协议消息。启动 Handle 是普通 Zircon channel 的 Handle，但其消息被用户态 runtime 解释为启动协议，而不是内核为每类启动资源提供专用查询 syscall。

协议头 `zx_proc_args_t` 包含：

- `protocol` 与 `version`，用于区分协议及版本；
- `handle_info_off`：Handle 元数据数组相对消息起始位置的偏移；
- `args_off` / `args_num`：以 NUL 结尾的 UTF-8 参数字符串数组；
- `environ_off` / `environ_num`：环境字符串数组；
- `names_off` / `names_num`：命名空间路径字符串表。

每个 Handle 有一个 `uint32` 元数据项。`PA_HND(type, arg)` 将低 8 位用于类型、较高部分用于类型参数；`PA_HND_TYPE` 与 `PA_HND_ARG` 可解码。类型参数对 `PA_NS_DIR` 是 `names` 表索引，因此 namespace 目录 Handle 与路径名通过同一消息中的两张表配对。`PA_FD` 的参数是目标文件描述符编号。

标准类型包括：

- 进程/地址空间：`PA_PROC_SELF`、`PA_THREAD_SELF`、`PA_JOB_DEFAULT`、`PA_VMAR_ROOT`、`PA_VMAR_LOADED`；
- 装载器与映像：`PA_LDSVC_LOADER`、`PA_VMO_VDSO`、`PA_VMO_EXECUTABLE`、`PA_VMO_BOOTDATA`；
- 启动 I/O 与资源：`PA_FD`、`PA_NS_DIR`、`PA_CLOCK_UTC`、`PA_RESOURCE`；
- 生命周期与服务：`PA_LIFECYCLE`、`PA_DIRECTORY_REQUEST`；
- 用户扩展：`PA_USER0`、`PA_USER1`、`PA_USER2`。

因此 argv、environment、namespace 路径、Handle 元数据和实际 Handle 数组是同一启动消息的不同区域；不是每种资源一个 ABI。动态链接器需要的 loader service、executable VMO、VDSO VMO 等也通过同一 Handle 表交付。`PA_NS_DIR` 只描述“某 Handle 对应某路径”，路径含义由用户态 namespace/runtime 解释；内核只传递 channel 消息和 Handle。

### 内核感知度与扩展

Zircon 内核理解的是 `zx_process_start` 的入口/栈/Handle 参数和 channel/Handle 生命周期，不理解 `PA_NS_DIR` 的路径含义、argv 的语义或用户自定义 `PA_USER*` 的协议。标准 `PA_*` 类型由 libc、runtime、loader 等用户态组件解释；新增用户资源可以采用用户态协议或保留类型，而无需新增内核对象查询接口。与此同时，内核仍可能对某些启动 Handle 的来源和权限有策略约束——“内核不解释 payload”不等于“Handle 无权利检查”。

该形状最接近 **D + A 的组合**：数据在一条 bootstrap channel 消息中，Handle 在 channel 的 Handle 携带机制中原子交付；消息内容是可变长度的版本化清单，而非固定 bootstrap mailbox。

## 2. Linux ELF：初始栈上的 argv/envp/auxv，标准键由内核与动态链接器共同演进

### 官方出处

- [`getauxval(3)`](https://man7.org/linux/man-pages/man3/getauxval.3.html)
- [`ld.so(8)`](https://man7.org/linux/man-pages/man8/ld.so.8.html)
- [`vdso(7)`](https://www.man7.org/linux/man-pages/man7/vdso.7.html)
- [Linux x86 ELF auxiliary vector](https://docs.kernel.org/arch/x86/elf_auxvec.html)

### 交付形状

Linux `execve` 建立新映像时，在用户地址空间的初始栈附近布置参数字符串、`argv[]` 指针数组、环境字符串、`envp[]` 指针数组以及以 `AT_NULL` 结束的 auxiliary vector。auxv 每项是 `(type, value)`；`getauxval()` 是 libc 对该区域的查询封装。初始栈是连续快照，读取发生在用户入口/运行时初始化阶段，不需要为每个键发 syscall。

典型键包括：

- ELF 装载：`AT_PHDR`、`AT_PHENT`、`AT_PHNUM`、`AT_ENTRY`、`AT_BASE`；
- 执行与安全：`AT_EXECFN`、`AT_RANDOM`、`AT_SECURE`；
- 机器能力：`AT_HWCAP`、`AT_HWCAP2`、`AT_PLATFORM`、`AT_PAGESZ`；
- vDSO：`AT_SYSINFO_EHDR` 指向内核映射的 vDSO ELF 映像；某些架构另有 `AT_SYSINFO`。

动态链接器是 auxv 的主要消费者。`AT_BASE` 可指示程序解释器（通常为动态链接器），`AT_SECURE` 会改变动态链接器对环境变量的处理；`AT_SYSINFO_EHDR` 让 libc/运行时通过 ELF 符号查找使用 vDSO。这个机制说明：初始清单既可承载内核提供的事实，也可承载仅供特定用户态运行时使用的启动元数据，但 Linux 的 auxv 键集合仍属于平台 ABI，不是任意类型的通用 Handle 表。

### 内核感知度与扩展

内核 ELF loader 负责生成一组标准 auxv 项、映射 vDSO 并布置初始栈；动态链接器/libc 解释其余语义。新 `AT_*` 键需要平台 ABI、内核和用户态消费者协同演进；未知键通常可被忽略，因条目由类型和值组成且以 `AT_NULL` 终止。扩展一个新类型的成本低于新增 syscall，但资源句柄本身通常仍走 fd 继承/传递，而不是 auxv：auxv 更像“指针/数值事实表”，不是能力安装表。

该形状明显属于 **A**：用户从初始栈主动解析；`getauxval` 的 API 让其表现得像查询，但数据本体不是每次查询由内核重新生成。与 eRhino 的差异是 Linux 初始栈主要交付数值、指针和字符串，fd/namespace 的复杂策略由 exec、继承的 fd 表和用户态完成。

## 3. seL4：BootInfo 只向 root task 交付初始能力与物理资源清单

### 官方出处

- [Capabilities tutorial](https://docs.sel4.systems/Tutorials/capabilities.html)
- [Untyped tutorial](https://docs.sel4.systems/Tutorials/untyped.html)
- [seL4 API reference](https://docs.sel4.systems/projects/sel4/api-doc.html)
- [Rust root-task tutorial](https://docs.sel4.systems/projects/rust/tutorial/root-task/)

### 交付形状

seL4 启动时向 root task 提供 `seL4_BootInfo`。它描述初始 CSpace/CNode 的布局、空闲 capability slots、初始 capability 范围、untyped memory 列表（物理地址、大小、是否 device memory）以及初始线程的 IPC buffer 等启动信息。BootInfo 是用户可读的启动结构；root task 用其中的 untyped capabilities 通过 `seL4_Untyped_Retype` 创建 TCB、CNode、endpoint、frame 等对象，再自行构造其他进程。

初始 IPC buffer 是线程运行所需的用户映射缓冲区；为新线程建立 IPC buffer 时，用户态分配 frame 并用 TCB API 设置。能力的交付不是 `(tag, integer)` 描述，而是 CSpace slot 中不可伪造的 capability：slot 位置、capability 权利和对象类型共同构成资源授权。

### 内核感知度与扩展

内核理解 BootInfo 的布局、初始 capability 和 untyped 的物理资源属性，因为这些直接影响内核对象创建与权限；但 root task 如何把 capability 分发给子系统、如何定义服务名称/namespace、如何编排组件，属于用户态。BootInfo 不是一个面向每个新进程的通用启动资源枚举 syscall。

扩展通常通过用户态 root task/启动框架构造新的 CSpace、IPC buffer、endpoint 和消息协议完成。若需要内核对象或新权利，必须有内核对象/调用语义；若只是服务配置，则可放在用户态协议中。该模型对 eRhino 的影响是：**Handle 安装与资源语义可以分层**，但 seL4 的强 capability 权限不能简单等同于可读清单；清单必须引用已经安装的本地 capability/Handle。

## 4. Mach / Apple：bootstrap port 是特殊能力入口，服务命名在用户态 bootstrap 服务

### 官方出处

- [Apple Mach Overview](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html)
- [`posix_spawnattr_setspecialport_np(3)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/posix_spawnattr_setspecialport_np.3.html)
- XNU [`task_special_ports.h`](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task_special_ports.h)
- Apple launchd [`libbootstrap.c`](https://github.com/Apple-FOSS-Mirror/launchd/blob/master/liblaunch/libbootstrap.c)

### 交付形状

Mach task 有一组 special ports；`TASK_BOOTSTRAP_PORT` 是其中之一。进程启动时通常继承 parent/launchd 提供的 bootstrap send right，进程通过 `task_get_special_port` 或用户态 bootstrap API 使用它。`bootstrap_look_up()` 按服务名向 bootstrap server 查询服务；`bootstrap_check_in()` 用于服务登记/接管。因此“命名空间”不是内核把路径清单解释成目录，而是一个可调用的 bootstrap 服务协议。

Apple 的非标准 `posix_spawnattr_setspecialport_np()` 可以把指定 Mach special port 设置到新进程，语义等同于新进程调用 `task_set_special_port()`；失败包括属性无效和资源不足。POSIX spawn 的 file actions/attributes 还可配置 fd 关闭、复制、打开、重定向以及调度/信号等属性，但这是一组过程创建操作，不是一个开放的 typed startup descriptor 表。

### 内核感知度与扩展

Mach 内核理解 task、port right、special-port selector 和消息传递；不理解 bootstrap 服务名称对应的服务语义。新增服务通常是 bootstrap server 的用户态命名协议，而非新增内核 special-port 类型。special port 本身是例外：selector 集合属于内核/ABI，扩展 selector 会增加内核 ABI 成本。

该模型接近 **C + D**：进程有可查询的 task/self 对象和固定 special-port 属性；具体服务资源通过一个 bootstrap port 的消息协议取得。它警示：把“Process-self”设计成固定隐式 Handle 可能只是把特殊入口改名，只有当 Process 对象和属性查询本身已经成立时才真正解决边界问题。

## 5. Plan 9：rfork/fd 继承与 namespace 复制，没有通用启动清单

### 官方出处

- [Plan 9 `fork`/`rfork` manual](https://9p.io/sys/man/2/fork)
- [Plan 9 namespace manual](https://9p.io/magic/man2html/6/namespace)
- [Plan 9 `srv` manual](https://9p.io/sources/plan9/sys/man/3/srv)
- [Plan 9port `rfork` reference](https://9fans.github.io/plan9port/man/man3/rfork.html)

### 交付形状

`rfork` 通过标志决定子进程是否共享或复制资源：`RFNAMEG` 建立独立 namespace，`RFCNAMEG` 清空 namespace；`RFFDG` 复制 file-descriptor group，否则 fd 表可与父进程共享。以 `RFPROC` 配合 `RFFDG` 可得到类似 fork 的进程创建形状。打开文件通过 fd 表继承，namespace 通过 mount/bind 等操作继承或复制；没有一个内核生成的“所有启动资源 descriptor 数组”。

Plan 9 的 `/fd`、`/proc/<pid>/fd` 和 `/proc/<pid>/ns` 提供对现有 fd/namespace 状态的用户态可见性；`/srv` 可发布一个已打开的文件/服务引用，其他进程通过服务文件取得它。因而服务发现、路径语义和共享资源名称都主要是用户态文件服务协议。

### 内核感知度与扩展

内核理解 fd、进程资源组、namespace 的复制/共享标志，以及文件系统操作所需的基本对象；不理解某个服务路径或 fd 所代表的业务资源。扩展靠新的文件服务、namespace 配置和约定，不靠扩展启动 ABI。该模型是“继承现有资源状态”的极简对照，不能直接满足 eRhino 需要的可枚举 typed/tagged grants，但支持“授权方只安装资源，接收方/库解释名称”的分层原则。

## 6. 交叉比较：内核理解什么，用户态协议理解什么

| 系统 | 低层交付机制 | 内核直接理解 | 用户态解释 | 新资源类型的常见扩展路径 |
|---|---|---|---|---|
| Fuchsia | `zx_process_start` + bootstrap channel + Handle 表 | 进程/线程/VMO/channel/Handle 权利与生命周期 | `PA_*` 元数据、argv/env、namespace 路径、loader/service 语义 | 新 `PA_*`、`PA_USER*` 或 bootstrap/服务协议；多数不需新 syscall |
| Linux | 初始栈 `argv/envp/auxv`，另有 fd/exec 继承 | ELF 映像、栈、vDSO、标准 auxv 生成 | 动态链接、`getauxval`、环境和 fd/namespace 约定 | 新 `AT_*` 需 ABI 协同；业务资源通常走 fd/用户态协议 |
| seL4 | BootInfo + 初始 CSpace capability + untyped | capability、对象、物理资源、IPC buffer | root task 的对象创建、服务命名、子进程启动协议 | 用户态构造 CSpace/协议；新内核权利需内核对象/调用 |
| Mach/Apple | task special ports + Mach messages + spawn attrs | task/port rights/special-port selector | bootstrap service 名称与查找、launchd 协议 | 用户态 bootstrap 服务；新增 special port 才需内核 ABI |
| Plan 9 | `rfork` 的 fd/namespace 继承 | 进程资源组、fd、namespace 操作 | 文件服务、mount/bind、`/srv` 服务发布 | 新文件服务/约定，不扩展启动清单 |

共同事实不是“所有系统都用一种形状”，而是：**内核交付不可伪造的低层对象/引用；资源的业务语义、名称和路由尽量留在用户态协议**。Fuchsia 最接近“通用启动清单 + Handle 表”，Linux 最接近“只读初始内存块”，seL4 最接近“内核 capability 集 + 用户态继续编排”，Mach 体现“固定 bootstrap 入口承载用户态命名服务”，Plan 9 则体现“继承状态而非描述状态”。

## 7. A/B/C/D 对照矩阵

| 方案 | 清单存放 | 读取时机 | 内核感知 | 扩展成本 | 与消息面的关系 | 失败/回收语义 |
|---|---|---|---|---|---|---|
| **A 用户映射只读启动块** | 用户只读映射的初栈、独立页或启动区域；入口传指针/地址 | `_start`/runtime 早期主动解析；可缓存快照 | 仅需映射、保护、边界和入口参数；不解释 tag/payload | 增加版本化 header/TLV/descriptor 通常为用户态 ABI 成本；改变映射布局或入口参数成本较高 | 可完全独立于 IPC；也可把清单仅用于索引已安装 Handle。Fuchsia 初始消息和 Linux auxv 是相邻实例 | 创建者需保证映射在 runnable 前完整、只读且生命周期明确；解析失败由 runtime 拒绝启动；未使用 Handle 由创建/退出清理路径回收 |
| **B StartupResource 查询 ABI** | 内核或父进程保存启动快照，用户不直接获得存储 | runtime 通过版本化 syscall 枚举/读取；可能多次查询 | 内核需理解快照边界、枚举和写回 ABI，但可不理解资源 tag 语义 | 增加 descriptor 类型可保持用户态协议；新增查询字段需 ABI 版本/快照规则 | 与 STARTUP 消息可完全分离，或把查询返回值视为消息面的等价清单；需要明确是否仍投递消息 | 需要定义查询快照何时冻结、读取失败、重复读取和未消费 Handle；内核保留清单直至进程终止/显式释放，资源回收路径更集中 |
| **C Process-self + 属性/资源查询** | Process 对象的属性或其关联内核状态；可能另有隐式/显式 self Handle | runtime 通过 Process API 查询身份与资源 | 内核理解 Process 对象、属性和查询权限；若资源仍是 tag/payload，语义仍可用户态化 | Process ABI 初始设计成本最高；之后属性扩展清晰，但固定 self Handle/selector 会形成特殊入口 | 可以用 Process API 取身份、用消息/Handle 表取资源；也可以把 startup 作为 Process 属性，但要避免复制两套模型 | 对象生命周期和进程终止天然关联；查询失败是对象/API 错误；若 Handle 已预安装仍需定义进程失败前的撤销/关闭 |
| **D STARTUP 消息 payload** | 现有 IPC mailbox/channel 中的一条 STARTUP 消息；payload 内 `StartupHeader` + grants | rinlib 在 `main` 前接收/解析一次 | 内核只识别投递消息与 Handle 安装，不解释 tag、前缀或 payload | 增加 tag/TLV/版本字段主要是消息协议成本；依赖固定消息入口、固定 mailbox 的过渡结构 | 与 IPC 直接耦合；可表达内容但受消息大小、投递时机和入口 mailbox 约束 | 需保证消息投递与 Handle 安装的事务性；接收前进程失败时消息/Handle 由 IPC/进程退出回收；消费语义和重复读取由 mailbox 定义 |

矩阵中的“内核感知”只描述机制，不代表授权方可以绕过权利检查。四案都可以满足“内核不感知 fs 路由表”等抽象资源，前提是资源本身作为已安装 Handle/共享区域/字节 payload 交付，路由表的格式和语义由用户态库与授权方协议定义。

## 8. 对三案的客观影响（不下结论）

### A：用户映射只读启动块

- 有成熟先例：Linux 初始栈把 argv/envp/auxv 作为一次性启动快照；Fuchsia 的 processargs 也把可变长度表放在一条启动消息中。
- 对“编译器 wrapper 只保留两个参数”的影响最大：需要受控 `_start`、固定映射地址或由入口传递 block 指针；否则只能把 block 地址放进现有可达参数。
- 能自然表达任意字节 payload、TLV、descriptor 和成对 namespace 数据，且读取不依赖 mailbox/查询 syscall。
- 需要明确映射布局、最大长度、只读保护、用户栈关系、跨 ABI 对齐和终止/回收时机。

### B：通用 StartupResource 查询 ABI

- 有“逻辑查询但物理快照”的先例：Linux `getauxval` 查询的是已在初始地址空间的 auxv；seL4 通过 BootInfo 读取一次内核提供的启动状态。但两者并非证明 B 必须使用 syscall。
- 可避开 ELF/Rust wrapper 参数传递限制，入口只需最小身份信息或不传清单指针。
- 对重复读取、快照一致性、内核保存清单、写回 buffer 验证、Handle 数值稳定性和启动失败回收提出明确 ABI 要求。
- 若枚举接口返回的 descriptor 只描述本地已安装 Handle，并允许用户态 payload/标签，仍可通过 fs 路由表验收；内核只负责复制/枚举字节，不需解释资源。

### C：Process-self + 属性/资源查询

- Mach 的 task special ports 展示了“对象属性/固定入口 + 用户态 bootstrap 服务”的可行性；seL4 的 TCB/CNode 则展示了对象能力与属性操作的完整模型。
- 若 eRhino 尚未完成 Process 对象、权限、生命周期和属性 ABI，C 的前置设计面最大；其价值取决于 Process-self 是否是一个真正的可授权对象，而非另一个固定隐式 Handle。
- 身份（PID、parent、生命周期）与附属资源可在对象模型中分层；但把资源清单也塞入 Process 属性会有与 B 重复的风险。
- 对资源回收有清晰的对象生命周期优势，但需要定义进程终止、对象关闭、未读取资源和权限裁剪之间的关系。

### D：STARTUP 消息 payload（当前过渡）

- 已证明可以承载 `StartupHeader`/`StartupGrant`，也能传递内核不理解的 tag 和 payload；Fuchsia 证明“启动消息 + Handle 表 + 用户态解释”是成熟形状。
- 主要结构性负担不是 payload，而是固定 bootstrap mailbox、固定 syscall、强制接收，以及把启动资源与普通 IPC 入口绑定。
- 若保留通用 channel/消息作为底座，D 可以演化为“版本化启动消息 + 普通 Handle descriptor”，但需要删除固定 Mailbox 特例并定义失败回收、消息大小、重复读取和启动事务。
- 因此 D 的现状并不能单独回答 A/B/C 的清单存放问题；它更像是“清单通过消息传递”的具体实现点，可与 A 的映射 payload 或 B 的查询快照在未来并存/替换。

## 参考来源清单

- Fuchsia process loading：<https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/docs/concepts/process/program_loading.md>
- Fuchsia `processargs.h`：<https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/public/zircon/processargs.h>
- Fuchsia process builder：<https://fuchsia.googlesource.com/fuchsia/+/389072e9e82220acd584ac21149fdadb4972c24f/src/lib/process_builder/src/process_args.rs>
- Fuchsia runtime：<https://fuchsia.googlesource.com/fuchsia/+/31644b052a2fa994a75df0b33eb2e1176d0b2f83/src/lib/fuchsia-runtime/src/lib.rs>
- Linux `getauxval(3)`：<https://man7.org/linux/man-pages/man3/getauxval.3.html>
- Linux `ld.so(8)`：<https://man7.org/linux/man-pages/man8/ld.so.8.html>
- Linux `vdso(7)`：<https://www.man7.org/linux/man-pages/man7/vdso.7.html>
- Linux ELF auxvec：<https://docs.kernel.org/arch/x86/elf_auxvec.html>
- seL4 capabilities：<https://docs.sel4.systems/Tutorials/capabilities.html>
- seL4 untyped：<https://docs.sel4.systems/Tutorials/untyped.html>
- Apple Mach Overview：<https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html>
- Apple `posix_spawnattr_setspecialport_np(3)`：<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/posix_spawnattr_setspecialport_np.3.html>
- XNU special ports：<https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task_special_ports.h>
- Plan 9 `fork/rfork`：<https://9p.io/sys/man/2/fork>
- Plan 9 namespace：<https://9p.io/magic/man2html/6/namespace>
- Plan 9 `srv`：<https://9p.io/sources/plan9/sys/man/3/srv>
