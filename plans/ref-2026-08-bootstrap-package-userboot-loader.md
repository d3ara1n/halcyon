# Bootstrap package、首个用户进程与用户态 ELF loader 外部取证

> 本文只记录官方文档与一方源码可验证的事实，不替 eRhino 做设计决定。链接指向 2026-08 调研时的官方页面或源码路径；源码主干可能继续演进。

## 1. Fuchsia/Zircon：ZBI → `userboot` → BOOTFS → component manager

### 官方出处

- [Zircon kernel to userspace bootstrapping (`userboot`)](https://fuchsia.dev/fuchsia-src/concepts/process/userboot)
- [Zircon program loading and dynamic linking](https://fuchsia.dev/fuchsia-src/concepts/process/program_loading)
- [Everything between power on and your component](https://fuchsia.dev/fuchsia-src/concepts/process/everything_between_power_on_and_your_component)
- [Zircon kernel command line options](https://fuchsia.dev/fuchsia-src/reference/kernel/kernel_cmdline)
- [ZBI header/format](https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/zbi-format/include/lib/zbi-format/zbi.h)
- [processargs ABI](https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/public/zircon/processargs.h)

### 内核理解的启动容器边界

1. 引导器把 Zircon kernel 和一个 ZBI data blob 放入内存。ZBI 是简单容器，包含硬件信息、kernel command line 和 RAM-disk image；RAM-disk 通常压缩。Zircon kernel 在早期启动只提取自身需要的部分。[userboot § Boot loader and kernel startup](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#boot_loader_and_kernel_startup)
2. ZBI 中的 BOOTFS item 是只读文件系统镜像：文件名以及在 BOOTFS 镜像内的 offset/size；文档明确指出字段要求 page-aligned 且受 32-bit 限制。BOOTFS 内容包括可执行文件、共享库和数据文件，启动后通常作为 `/boot` 的只读树提供。[userboot § BOOTFS](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#bootfs)
3. kernel **不包含** zstd 解压和 BOOTFS 格式解析代码；这些由第一个用户进程 `userboot` 完成。[userboot § BOOTFS / Kernel loads userboot](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#bootfs)

因此，证据支持的边界是：kernel 认识 ZBI 中启动所需的 item/VMO 交付和启动参数，但不把 BOOTFS 当作内核文件系统，也不由 kernel 解析其中的 ELF 文件名或目录。

### 首个用户进程及其初始能力

1. `userboot` 是普通用户进程，只能像其他进程一样通过 vDSO 使用标准 syscall，并受完整 vDSO enforcement 约束。特殊之处仅在于 kernel 如何装载它。[userboot § Kernel loads userboot](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#kernel_loads_userboot)
2. `userboot` 是编译期嵌入 kernel 的 ELF dynamic shared object；其简单 RODSO 布局使 kernel 不需要启动时解析 ELF header。构建时提取只读段大小、可执行段大小和入口地址作为 kernel 常量；kernel 将 userboot 与 vDSO 映射到首个用户进程并启动。[同上]
3. kernel 用正常的 `processargs` bootstrap channel 协议启动 `userboot`。kernel command line 被拆成 environment strings；userboot 及系统其余启动阶段需要的 handles 放入消息，并用 handle-info 标记用途。消息中包括 `PA_VMO_VDSO`，以及 `PA_VMO_BOOTDATA`：后者是包含引导器 ZBI 的 VMO。[userboot § Kernel sends `processargs` message / userboot decompresses BOOTFS](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#kernel_sends_processargs_message)
4. `userboot` 从 `PA_VMO_BOOTDATA` VMO 扫描 `ZBI_TYPE_STORAGE_BOOTFS`。它映射该 item；若压缩，则用自带 zstd 支持解压到 fresh VMO。[userboot § userboot decompresses BOOTFS](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#userboot_decompresses_bootfs)

文档没有把 userboot 描述为拥有一个绕过 handle 权限模型的隐式“root” syscall 权限；相反，它明确称其为普通用户进程。它当然拿到 kernel 特意交付的启动 handles（包括 bootdata、vDSO 及系统启动所需资源），这些是显式能力/handles，而非“所有权”语义。

### 谁解析 ELF、谁构造后续进程地址空间

1. userboot 根据环境中的 `userboot.next=<file>+<arg...>` 选择下一个程序；没有该选项时默认 `bin/component_manager+--boot`。[userboot](https://fuchsia.dev/fuchsia-src/concepts/process/userboot)；参数语义也见[kernel command line](https://fuchsia.dev/fuchsia-src/reference/kernel/kernel_cmdline#userboot-next-path)。
2. userboot 实现完整 ELF program loader：从 BOOTFS 找到程序；若有 `PT_INTERP`，再找到并装载解释器；随机地址装载 vDSO；创建新 process/thread/channel；构造标准 `processargs` 消息；最后通过 `zx_process_start()` 设置入口、栈、bootstrap channel 和 vDSO base。[userboot § userboot loads the first real user process](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#userboot_loads_the_first_real_user_process)；[program loading § ET_DYN](https://fuchsia.dev/fuchsia-src/concepts/process/program_loading#an-elf-et_dyn-file-with-no-pt_interp)
3. 有 `PT_INTERP` 时，userboot 先给动态链接器一条 loader bootstrap message，其中含主 ELF executable VMO 和 loader-service channel；动态链接器读取主 ELF 并通过 loader service 取得共享库 VMO。loader service 是 channel RPC，`LOADER_SVC_OP_LOAD_OBJECT` 返回包含对象内容的 VMO。[program loading § PT_INTERP / loader service](https://fuchsia.dev/fuchsia-src/concepts/process/program_loading#an-elf-et_dyn-file-with-a-pt_interp)
4. Zircon 的低层 program loading API 以 VMO、VMAR、process、thread 和 channel 为基础，不要求 kernel 访问 filesystem。一个加载请求至少包含 executable VMO（需 `ZX_RIGHT_READ` 与 `ZX_RIGHT_EXECUTE`）、argv/env 和初始 handles；执行映像的创建者负责把这些资源交给新进程。[program loading](https://fuchsia.dev/fuchsia-src/concepts/process/program_loading)
5. 之后的加载由用户态 program loader / component manager 等创建者执行：kernel 提供 VMO、VMAR、process、thread 等 building blocks，而不是传统 `execve` 式“按文件名由 kernel 读取并解析 ELF”。[program loading § Zircon program loading](https://fuchsia.dev/fuchsia-src/concepts/process/program_loading#zircon_program_loading)

### 只读/零拷贝交付的证据边界

- BOOTFS 本身是只读格式；压缩 BOOTFS 必须先解压到 fresh VMO，所以从压缩 ZBI item 到可访问 BOOTFS 存在一次解压/生成新 VMO 的过程。[userboot](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#userboot_decompresses_bootfs)
- userboot 从 BOOTFS 找到 executable 和 libraries；文档明确说动态 linker、executable、shared libraries 都从同一 BOOTFS pages 加载，而这些 pages 后来成为 `/boot` 文件。[userboot § loader service](https://fuchsia.dev/fuchsia-src/concepts/process/userboot#userboot_loader_service)
- 对后续普通 ELF，标准模型是“VMO 作为文件内容 + 映射 PT_LOAD”，而非把文件复制进 kernel 私有 ELF 缓冲区；但上述文档没有对每个平台/压缩状态作“物理页永不复制”的强保证。因此可验证结论是只读 VMO/映射模型和避免逐个用户态文件拷贝，不应将其扩大解释成所有启动路径都严格 zero-copy。

## 2. seL4：boot image → root task/initial thread → 用户态创建对象与加载 ELF

### 官方出处

- [seL4 Libraries: processes & ELF loading](https://docs.sel4.systems/Tutorials/libraries-3.html)
- [seL4 elfloader](https://docs.sel4.systems/projects/elfloader/)
- [seL4 kernel `src/kernel/boot.c`](https://github.com/seL4/seL4/blob/master/src/kernel/boot.c)
- [CapDL loader `src/main.c`](https://github.com/seL4/capdl-loader-app/blob/master/src/main.c)
- [seL4 capabilities tutorial](https://docs.sel4.systems/Tutorials/capabilities.html)
- [seL4 untyped tutorial](https://docs.sel4.systems/Tutorials/untyped.html)

### 启动镜像与 root task

1. elfloader 从嵌入的 CPIO archive 加载 kernel 和 user image，并初始化 kernel 初始页表；启动 kernel 时传递 user image（以及可选 DTB）信息。[elfloader](https://docs.sel4.systems/projects/elfloader/)
2. kernel 建立并启动 root server/root task（其初始线程是用户态的 initial thread），为它创建 root CNode、TCB、IPC buffer、BootInfo frame、VSpace 等根对象；`boot.c` 中的 `calculate_rootserver_size()` 明列这些对象，`create_bi_frame_cap()` 把 BootInfo frame cap 放入 root CNode。[kernel `boot.c`](https://github.com/seL4/seL4/blob/master/src/kernel/boot.c)
3. `populate_bi_frame()` 写入 `seL4_BootInfo`：node 信息、IPC buffer 地址、initial thread CNode size/domain、extra boot-info 长度等；`create_untypeds()` 为可用物理内存建立 untyped capabilities，并在 BootInfo 的 untyped slot region 中报告它们。[同一 `boot.c`](https://github.com/seL4/seL4/blob/master/src/kernel/boot.c)
4. 官方 capabilities/untyped 教程把 untyped 描述为 root task 可用于 retype 创建新 kernel objects 的初始能力；因此 root task 不是普通、资源空白的第一个进程，而是被赋予系统初始化所需的初始 capability authority。该 authority 是显式 capability 集合，不是“首进程名字触发的隐式特权”。[capabilities](https://docs.sel4.systems/Tutorials/capabilities.html)；[untyped](https://docs.sel4.systems/Tutorials/untyped.html)

### 后续进程、地址空间与 ELF

1. seL4 的抽象是 TCB、CNode 和 VSpace；官方教程中 `sel4utils_configure_process_custom()` 组合出一个新 process 的资源，`sel4utils_spawn_process_v()` 启动它，并明确演示新线程使用独立 CSpace。[libraries-3](https://docs.sel4.systems/Tutorials/libraries-3.html)
2. 该教程的 ELF loader 由用户态库完成：从 CPIO archive 取得独立 ELF 文件，展开到 VSpace，随后创建/配置线程并启动；教程也明确将“separate ELF file loaded and expanded into a VSpace”作为学习目标。[libraries-3](https://docs.sel4.systems/Tutorials/libraries-3.html)
3. CapDL loader 是更完整的一方实现：`main()` 的 `init_system()` 依次读取 `seL4_GetBootInfo()`、解析 BootInfo、按 CapDL specification 创建对象、初始化 ELF、VSpace、TCB/CSpace，然后 resume threads。[CapDL `src/main.c`](https://github.com/seL4/capdl-loader-app/blob/master/src/main.c)
4. 同一源码的 `elf_load_frames()` 从嵌入 CPIO archive 取 ELF，检查 ELF，遍历 `PT_LOAD`，把目标 frame 映射进 loader 的地址空间并写入 segment；`init_vspace()` 映射页目录/页表/页，`init_tcb()` 配置 TCB 的 CSpace、VSpace、IPC buffer，最后 `start_threads()` resume。[CapDL `src/main.c`](https://github.com/seL4/capdl-loader-app/blob/master/src/main.c)

所以在 seL4 的常见模型中，kernel 负责提供最初 root task、BootInfo 和 capability primitives；root task 或其用户态 bootstrap/CapDL loader 负责解析 ELF、分配/重类型化对象、构造 CSpace/VSpace/TCB 并启动后续线程。

### 只读/零拷贝交付的证据边界

- elfloader 的事实是“从嵌入 CPIO archive 加载 user image”；这证明 boot image 由早期加载器交付给 seL4，不证明 kernel 理解 CPIO 目录或 ELF。[elfloader](https://docs.sel4.systems/projects/elfloader/)
- CapDL loader 的 `elf_load_frames()` 明确从 CPIO 中读取每个 ELF 的 `PT_LOAD`，将目标 frame 映射到 loader 地址空间并写入；这是“用户态解析 + 写入目标 frame”的模型，不是严格 zero-copy ELF segment 映射。[CapDL `src/main.c`](https://github.com/seL4/capdl-loader-app/blob/master/src/main.c)
- seL4 的 BootInfo 交付重点是 capability/object/resource 描述；BootInfo 本身不是一个由 kernel 解析 ELF/CPIO 的接口。用户态 loader 是否保留、复制或映射原始 archive，取决于具体 bootstrap implementation；上述 CapDL 实现至少对最终目标 frames 执行了写入。

## 3. 对照表（仅归纳已取证事实）

| 维度 | Fuchsia/Zircon | seL4 常见 root-task/CapDL 模型 |
|---|---|---|
| kernel 对启动容器的理解 | 读取启动所需 ZBI item/交付 bootdata VMO；不解析 BOOTFS | kernel 启动 root server 并生成 BootInfo；elfloader 负责从 CPIO 取 user image，CapDL loader 在用户态读 CPIO/ELF |
| 首个用户进程 | `userboot`；普通用户进程，显式获得 bootdata/vDSO 等 handles | root task/initial thread；显式获得 root CNode、BootInfo、untyped 和根对象能力 |
| 后续 ELF loader | userboot 首次加载；之后由用户态 program loader/component manager 等创建者 | root task 或用户态库/CapDL loader |
| 地址空间构造者 | 用户态 loader 创建 process/VMAR/thread 并映射 VMO | 用户态通过 capability syscall/library 创建 VSpace/page tables/TCBs/CSpaces |
| boot image 只读/零拷贝 | BOOTFS 只读；压缩时先解压到 fresh VMO；同一 pages 可作为 `/boot` backing；文档不作绝对物理 zero-copy 保证 | CPIO archive 嵌入 user image；CapDL `PT_LOAD` 明确写入目标 frames，非严格 zero-copy |
| 首进程隐式特权 | 文档明确说 userboot 是普通用户进程；特殊能力来自 kernel 显式 handles | root task 具有广泛初始 capability authority；来源是 BootInfo/CNode/untyped 显式能力，而非进程名称 |

## 4. 未由这些来源证明的事项

- Fuchsia 当前主干所有构建配置下是否都采用同一压缩/页回收策略；官方启动概念页本身提示部分流程文档可能过时。
- Fuchsia BOOTFS 文件 VMO 在每一种 storage/compression 路径下的物理页共享和 COW 细节；概念文档只足以证明 VMO、只读 BOOTFS 与映射式加载模型。
- seL4 某个具体平台/版本的 user image 交付是否把 archive 原页直接授予 root task；不同 elfloader、root-task framework 和 CapDL 配置可能不同。
- “生产系统”不是单一 seL4/Fuchsia 契约：上述两者的启动镜像、root task/userboot 和 ELF loader 都是可替换的用户态/早期启动组件组合，不能从一份实现推导所有部署的统一规则。
