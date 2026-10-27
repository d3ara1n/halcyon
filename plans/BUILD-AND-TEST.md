# 构建与验证

运行构建、测试、QEMU 或调整验收基础设施前读取本篇。任务是否完成由 [AGENTS「任务适用范围与完成条件」](../AGENTS.md#任务适用范围与完成条件)判断；同一路线在施工期间可作开发检查，路线通过本身不宣告机制完成。具体命令和默认参数以 [Justfile](../Justfile) 为执行真值。

## 工具链与构建入口

- 开发机为 macOS，系统工具为 Homebrew 的 `just`、`dtc`、`qemu`、`riscv64-elf-binutils`、`riscv64-elf-gdb`；系统级安装由用户执行。
- Rust edition 2024，工具链由根目录 `rust-toolchain` 选择；当前内容是浮动 `nightly`，并未固定日期。取证或比较代码生成时记录实际 rustc 版本。
- 内核、共享库、用户态分别属于 `os/`、`shared/`、`user/` 三个 workspace，以 path 依赖连接。内核 target 为 `os/riscv64imac-unknown-erhino.json`，用户态默认 target 为 `user/platforms/riscv64imac-unknown-erhino-elf.json`，均使用 build-std；浮点验收 `test_fp` 使用独立 gc target。
- **统一走 `just`，不裸跑 `cargo build`**。内核链接器 `riscv64-elf-ld` 和链接脚本由构建入口配置，用户态由自定义 target 与 build-std 提供运行环境。
- 内核 package、默认 binary target 和产物名统一为 `kernel`；本文命令与构建入口以此为准。
- initfs 使用 tar 时避免 bsdtar 的 `._` AppleDouble 文件混入归档。

```sh
just build_kernel
just check
(cd shared && cargo check)
just clippy
```

`just check` 对 os workspace 执行带 build-std 的 cargo check，包含它的实际依赖，不等于检查所有 workspace。`just clippy` 覆盖 shared/os/user host、kernel/user RISC-V、stress/memory/device/resource feature 与独立 gc target 十个分面，全部使用 `-D warnings`；resource同时编译真实function/IRQ、MMIO/DMA竞争消费者，user RISC-V 同时选择库与程序，覆盖尚未被可执行程序链接的领域库。完整日志写入 `artifacts/lint/`。

所有 QEMU 启动路线自动准备产物目录，并通过 `make_dtb` 从对应平台的 `device.dts` 重新生成 `artifacts/<PLATFORM>/<MODEL>/device.dtb`，无需手动预生成；清理产物后可直接运行 `just virt`。`virt-hetero`、`virt-nofd` 在生成变体 DTS 前同样准备目录；`ERHINO_DTB` 只选择 QEMU 加载的 DTB，不改变标准 DTB 的生成位置。

## Host 测试

纯逻辑 crate 的 host 测试必须显式指定 host target，不能继承 RISC-V 默认 target 去链接 std。当前开发机命令：

```sh
(cd os && cargo test --workspace --exclude kernel --target aarch64-apple-darwin)
(cd shared && cargo test --workspace --target aarch64-apple-darwin)
(cd user && cargo test -p libkernel -p libthread -p libruntime -p libbudget -p libexecution -p librpc -p librunnel -p liboal -p libservice -p libdevice -p libpci -p libdriver -p libusb -p libhid -p libprocess -p libfs --target aarch64-apple-darwin)
```

user 的 host 命令只选择公共库，不能把 RV64 服务二进制纳入 host `--workspace` 测试。根据改动先选择受影响的 package 或现有 testcase。新增测试说明其独立契约和目标错误；无关测试或新增 harness 不作为默认交付物。clippy 编译测试目标不等于执行 host 测试，QEMU 聚合同样不替代它们。

## QEMU 路线与选择

workload（core/stress/memory/device/device-reuse）由用户态 `srv_init` 编译期控制，内核不感知测试政策。服务、驱动、验收进程分别位于 `user/services/`、`user/drivers/`、`user/tests/`；验收配置中的进程也可承载正式机制，成熟度由对应 impls 与任务记录分别说明。

| 命令 | 覆盖与用途 |
|---|---|
| `just virt [mode]` | 直接运行 virt 演示；默认 debug，终端实时显示编译和系统输出 |
| `just virt-stress` | core 加 control/Tunnel 重复压力、非空 WaitSet 进程自动退休与完整 16/16 竞态矩阵 |
| `just virt-release` | `just virt release` 的演示快捷入口 |
| `just acceptance-virt [mode]` | virt core 自动验收；默认 release，可指定 debug |
| `just sifive_u [mode]` | 直接运行 sifive_u 演示；默认 release，终态由用户退出 |
| `just acceptance-device [mode]` | 正式 Devices/FunctionAccess/OAL 键盘客户端：无键盘启动、两轮同端口 QMP 热插、客体确认按下/释放、主机删除与客体 Seat 释放分别确认、IRQ Take/Ack 和实际 worker Pool 退款；随后保留 core 全部业务 |
| `just acceptance-device-reuse` | 正式 debug ELF/GDB 外部用户 fault：A/B 同 HID PID、停 A 保旧 function/Watch、B 继续输入、C 复用、CPU 故障后 B/C 原 Seat Released 与单次进程收集；以本轮 controller Ready 为插入门，不声称 idle |
| `just acceptance-device-candidates` | 同一debug复用/QMP路线的显式负装配：真实PCI映像在原固定identity/角色门明确Permission且无物理/子登记授权，等原轮Closed/Released/Detach，在同DeviceId新epoch接真实HID及输入；正常声明和原工作集不变 |
| `just acceptance-device-manager-fault Device/Driver [wait/start/stopping/loading]` | 同一debug装配和外部GDB：root业务等待、Start后首ecall、正常停止中的指定管理者fault。loading仅Driver：核实际test_target首段/首页指纹、剩余复制及未完成子进程未Start即进入终局。均核唯一封口与QMP guest RESET，不以reset证明硬件停止或退款 |
| `just acceptance-device-reset-rejection` | 真实 xHCI 已交付环地址后使 srv_device fault，拒绝真实 ColdReboot/SystemFailure；复用原 DMA 观察体核 init DONE/四 hart 停驻后同一组 pin/backing/write 许可仍保留，不能据此断言设备 DMA 静止 |
| `just acceptance-device-controller-fault` | 同一正式debug装配中使实际xHCI PID在真实trap入口fault，核原ProcessTask DONE与Managed受信主控故障、外层唯一终局；拒绝ColdReboot后核实际非空exposed DMA集合跨init DONE/四hart停驻保留，不以CPU完成补静止或Retire |
| `just acceptance-device-unknown Claim/Prepare/ProvisionGet/GetResource/Activate/QueryUse/Master` | 同一debug core装配，GDB仅令一份真实成功回复的运输Send期限At0，核真实Once/context、DeadlineExpired及原请求Sent错误。Claim走唯一故障终局；Prepare核首次Preparing/Prepared接纳回复；GetResource要求暴露位保持true、PCI未开始ECAM访问、原Use报告后安全释放；QueryUse核激活接纳后的首次权限核验失败，须无资源暴露/硬件访问并退原轮。其余核原轮停止或Master原context/绝对期限恢复及完整core收尾。不声称回复入队后丢失 |
| `just acceptance-device-function-session` | 显式仅自动PCI的debug core装配；init缩权Claim真实xHCI两轮并复用libusb完成Run/停止/DMA DONE，旧function跨新Prepared核Permission、实际Stop传播后核Closed；只读GDB观察真实认证context与ECAM BME，不新增产品操作 |
| `just acceptance-irq-retirement` | 同一真实Controller两轮QMP插入/Take，保留旧token跨原provider Revoke；只读核complete先于disable、旧use Closed、新代已taken时旧token拒绝且原新token恰一次complete，随后完整core收束 |
| `just acceptance-irq-idle` | 上述真实消费者增量：插入前必须取得同一四hart idle/队列/maintenance/remote/期限快照，暂停时QMP真实插入，恢复后核同源claim/Take；记录timer共现，不把全停或Waiting日志当idle证据 |
| `just acceptance-irq-exit` | 原init/test_target取得实际IrqUse：直接变体核实际Waiting/Line来源，集合变体持续持有Set并用Receive复检候选，核真实Set/Line成员后Kill（不要求集合宿主同时Waiting）；保留母本核摘链/loan归还、原Pool零/Revoke/DONE及完整core |
| `just investigate-clock` | 原IRQ退出消费者/4hart/无icount的一次时钟首因调查；THROTTLE100、冷启动-S，只有正常时钟read的两错误路径只读点，核实际STATE/self与写前failed、局部样本/几何/当前高水位和原栈后让原终局执行。正常结束无命中仅记未复现，不验证IRQ来源/公平门，不自动重复 |
| `just acceptance-mmio-publication` | 同一core的原test_target/Map/Revoke消费者，外部vCont分别卡最终许可前/成功后且VM锁前；父Revoke真实返回后放行，核ObjectBusy/后续新Map Closed或原Map成功、活VM全部原范围PTE失效、scope DONE及独立Pool最终退款 |
| `just acceptance-mmio-forced` | 同一test_target两次32页Map，核非空FIFO/已提交Publishing的原Work在父Revoke成功及Kill/Frozen后继续或交整VM退休；栈上Execution与实际VM/原mapping关联，最后scope DONE、Killed/Control关闭及独立Pool零 |
| `just acceptance-device-retry-race` | 原device-reuse的C已安全Released/Detach后，两独立Caller用同Control授权发显式Retry；GDB只读核实际两begin同settled记录、一次重置及另一次迟到commit。仅旧记录已移除的观察不代表新Rc/原Rc新epoch保护；无窗口即记未覆盖 |
| `just acceptance-dma-races` | 同一core的原test_target/Pin/Read/Retire消费者，外部vCont核Pin写许可前后Seal、非空准备退出、reader持有时Retire等待；分别核许可/域DONE、父对象关闭及独立子Pool最终退款 |
| `just acceptance-dma-shared` | 同一MemoryObject的两份实际Pin共享完整Backing/SG，逐份DONE/关闭、写许可2→1→0与子metadata归还；父root仍在不退物理页，最后父对象/CPU/Control关闭后核独立Pool零和原父RAM退款 |
| `just acceptance-dma-quota` | 原test_target以真实Notification owner耗尽独立Pool metadata，核原Pin明确Quota、释放压力后同Use恢复及最终原账零；不代表精确失败前缀或allocator OOM |
| `just acceptance-dma-install-failure` | 原test_target的Pin完整取得use/write并ReadyUnexposed后，外部GDB在最终句柄安装的releases Buffer真实allocator入口注入一次null；核实际OutOfMemory、无handle/地址暴露、原Work收束及独立Pool最终零。指定分配失败不代表真实堆耗尽 |
| `just virt-platform-funding` | 复用原boot失败设施，分别在首MMIO根Node构造、DMA未发布空根retained条目和全批Building cap表预约的真实未执行分配入口指定失败；DMA模式核出生前后原账相同且不排正常Root Work，其余模式按实际owner区别局部退款与稳定清理接管；均核无能力逃逸、真实boot错误及四hart Gate Failed停驻 |
| `just acceptance-sifive_u [mode]` | 128MiB、非零 boot hart 等板级差异的 core 自动验收；默认 release |
| `just virt-nofd [mode]` | 无 F/D、无兼容浮点域的完整 core；默认 release，可指定 debug |
| `just virt-hetero [mode]` | 异构调度域及完整 core；默认 release，可指定 debug，涉及调度域时额外运行 |
| `just virt-boot-failure` | 外部 GDB 对正式 debug binary 注入 Ready 前 panic/alloc/fatal，确认所有 hart 在 Failed 后停驻 |
| `just virt-ipi-failure` | 外部GDB跳过真实已发布责任的一次SBI IPI并返回FAILED；核无timer WFI目标在后续真实广播唤醒后先停驻，确认稳定停止事实 |
| `just virt-vm-fault` | debug memory：复用 stress 中的线程/映射消费者，旁观真实 Map 发布窗口 fault→RetryFault；detach 后核目标 Dead/Completed、Job 收束、辅助能力关闭与显式 reset |
| `just virt-vm-repair` | 外部GDB保留真实PTE与用户现场，分别注入R/W/X入口故障及重复故障；核实际SFENCE/FENCE.I、原寄存器与pc重试、无进展终止界 |
| `just virt-dma-retention` | 完整 debug core 的真实 DMA pin；GDB 拒绝最终 SBI reset，核 init DONE 前后及四 hart 停驻后仍持原页/写许可，mapping Work 为 Idle |
| `just virt-dma-ipi-retention` | 同一严格core保留观察，额外让真实InitialWatch DONE后第一次request广播中的一个非发送者WFI目标IPI返回FAILED；核实际返回/后续原广播和同组mapping跨四hart停驻保留，不排除timer/其他唤醒 |
| `just acceptance-core` | 常规跨机制回归：clippy、无调试器 debug stress、release core |
| `just acceptance-platforms` | 平台组合：release sifive_u core、release nofd core |
| `just acceptance-faults` | 特权失败与发布窗口：boot-failure、IPI failure、VM fault、VM repair、DMA retention |
| `just acceptance` | 完整收尾：依次执行上述三组，各路线仅一次 |

演示与自动验收是独立入口：`virt`、`virt-release`、`sifive_u` 直接运行 QEMU，不执行验收锚点判定，也不自动收割终态。日常自动验证优先受影响 host 测试及 `acceptance-virt`；需要 debug 验收用 `acceptance-virt debug`，涉及竞争与退休用 `virt-stress`。常规跨机制回归用 `acceptance-core`；平台、DT、hart 启动/能力、资源布局变化补 `acceptance-platforms`，调度域变化再加 hetero；启动失败、IPI/停驻、VM 发布/修复、uaccess 变化补相关故障路线。完整收尾保留 `acceptance`，聚合只调用测试入口，不调用演示入口。

core/stress 及携带 core 的平台路线同时要求实际 srv_device 就绪、init 私有装配认证、来源封口与正常退出，以及设备枚举/OAL/Watch 初次观察和撤销终态的日志锚点；memory 聚焦路线不启动该服务。这些锚点只覆盖管理宿主装配，不证明平台 Provision、真实驱动或硬件停止已接通。

施工中路线结果只作为开发证据。机制闭包收尾按实际影响执行 `just check`、`just clippy` 与必要路线；已由聚合覆盖的检查不另行重复。平台完整业务保留，不用“板级差异”理由删去可能暴露资源/调度交错的真实消费者。文档等不改变执行行为的工作按影响核验，不运行无关的代码验收。

## 节流、超时与判定

演示经 `tools/qemu-throttle.sh` 直接运行；自动验收再由 `tools/qemu-acceptance.sh` 或专项诊断器判定。默认 `THROTTLE=50`，stress 和自动 GDB 路线同样节流，不作为全速性能基准；需要全速的专项诊断显式设 `THROTTLE=100`。节流限制宿主执行占空比，不能由 50% 推导整条路线耗时恰好翻倍。演示没有验收超时，运行到系统退出或用户中断；非交互环境需要防挂死时由调用者显式使用 `timeout`。

各路线有独立超时，可用 `VIRT_TIMEOUT`、`VIRT_STRESS_TIMEOUT`、`VIRT_RELEASE_TIMEOUT`、`VIRT_DEVICE_TIMEOUT`、`VIRT_HETERO_TIMEOUT`、`VIRT_NOFD_TIMEOUT`、`SIFIVE_U_TIMEOUT`、`VIRT_BOOT_FAILURE_TIMEOUT`、`VIRT_IPI_FAILURE_TIMEOUT`、`VIRT_VM_FAULT_TIMEOUT` 覆盖。默认值只在 Justfile 维护；virt stress 为竞态矩阵和文件资源/退出组合预留较长预算。VM fault 的 memory 负载有独立预算，旁观与 detach 后收束共用总期限；聚焦负载后仍保留原预算，不把提速当作收紧防挂死期限的依据。平台路线预算同时允许显式 debug，异构域的 Base64 系统负载固定在单 hart。超时从 QEMU 运行阶段计，不含冷编译；聚合命令不另设跨路线总时限。

路线超时是可扩大的运行预算，采用**撞限翻倍**政策：实际触及路线期限时，将该路线在 Justfile 中的默认值翻倍，并重跑核对完整业务收束与 reset；不为保留旧期限删减正式场景。默认数值只由 Justfile 拥有，手册和验收脚本不另设副本。超时从 QEMU 运行阶段计，不是性能目标；panic或明确错误触发的主动收割不算撞限，不能仅凭退出码误调期限。挂起或失败的具体原因仍按调查手册定位，翻倍不替代业务锚点与正确性验证。

判定依据 workload 身份、该 workload 的完整收束和显式 reset 锚点，不能用矩阵中途超时宣称内核失败。`virt-boot-failure` 同样节流，由 `tools/check-boot-failure.py` 独立判定预期故障、限制 GDB 时间并回收进程组，完整日志在 `artifacts/boot-failure/`。

GDB专项路线在各自artifacts目录下使用时间戳与进程号创建独立日志，保留ELF hash、注入位置和原始观察。共同工具[qemu_gdb.py](../tools/qemu_gdb.py)按ELF PT_LOAD换算物理地址，并用QEMU的物理内存远程包读取跨hart全局事实，读取后恢复原模式；不依赖所选hart当前satp。VM repair中的异常原因/地址由诊断器注入，证明修复分支及现场合同，不宣称复现真实硬件TLB缓存陈旧；VM fault则要求正式消费者产生真实发布窗口故障。GDB停顿不作为计时、WCET或调度公平性证据。

DMA retention 使用原 init/test_target 的实际角色和 pin，不向设备提交地址。脚本在最终真实 SBI reset ecall 返回 NotSupported；通过正式运行期各页表共享的内核高半区读取稳定 owner，并在 init DONE/全 hart 停驻处复核同一组 backing 区间和写许可、无 runnable mapping 退休债务。`VIRT_DMA_RETENTION_TIMEOUT` 由 Justfile 拥有，路线固定 THROTTLE=100 以避免调试器被周期冻结；它不作为性能证据，也不证明硬件 DMA 已静止。每次运行的 QEMU/GDB 由脚本 finally 收束。

DMA＋IPI 组合在原 InitialWatch DONE 的 request 入口暂用 GDB `scheduler-locking on` 只推进原请求者，使其他 hart 不会在选择失败目标前先提交 STOPPED；一次注入失败被原调用方消费后立即恢复 `off`，后续原停止广播和四 hart 停驻均在恢复调度后观察。若 request 结束仍没有合格目标，恢复调度并明确失败，不写 guest 停止标志或目标 PC 来制造资格。WFI/无 quantum、同 call 的原始返回与 Result 消费、同目标后续广播和同组 DMA 保留判据保持独立；不排除 timer/其他唤醒，不证明全部 IPI 永久失败下的停机活性。返回结果的栈地址按当前 ELF 的布局常量与 `StackWindowLayout` 的正式栈映射换算，仅接受原 sender 的正式栈；栈窗口 NOLOAD 的 ELF p_paddr 和内核直映射偏移都不能代替真实栈物理布局。物理模式内完成原 Result 的 DWARF 解码并保存字节，退出后只保留标量。

M累计栈工具从固定ELF重建CFI与动态目标，源码语义前提须人工核实后由[固定证书](../tools/maintenance-stack-certificate.json)绑定；没有证书、未知site/CFI/业务环或源码变化均不签发上界。本轮推导、命令和拒签负向证据由[对象链修复档案](archived/review-2026-10-07-object-chain.md#新elf累计栈与拒签证据)拥有；证书不是对未来构建的自动许可。

验收脚本只要求当前实际消费者与仍在调用的自检锚点；已删除机制的自检、日志与容量路线同步删除，不保留兼容锚点或为使旧门通过重建机制。路线通过证明该 workload 的当前可观察行为；内部交错、失败退款和付款保证的验证缺口由所属工作记录承载，不以旧锚点缺失代替风险判断。

所有路线通过、失败、中断或超时后，都要确认本次启动的进程已清理。手动启动 QEMU/GDB 也由启动者负责清理，不干扰其他任务的进程。

共享`qemu_gdb.stop_group`只收启动者以`start_new_session`创建的专有组：先CONT/TERM，有限核实际PID/PGID集合，必要KILL后再次核空；leader退出或发出信号均不替代组消散。设备操作者的短清理段避免重复终止打断，独立VM/调试器/QMP/临时文件均须收尾；清理异常单独留存，保留原验收失败事实。

## sifive_u 平台边界

该模型对应 HiFive Unleashed：hart 0 无 MMU，可运行 hart 为 1–4，DRAM 128MiB，timebase 1MHz，boot hart 不固定。内核继续运行时探测现代 SBI，不因板型历史调用 v0.1 核心 ABI，也不引入专用内核政策。

该模型没有可用 shutdown 后端。`just sifive_u` 是演示，系统报告 reset 失败后仍保留现场，由用户 Ctrl-C 退出。`just acceptance-sifive_u` 遇到明确 reset 失败或 panic 终态时主动收割，再检查完整 core 锚点；收割本身不代表验收通过。

## 日志与调查

演示入口 `virt`、`virt-release`、`sifive_u` 在 Justfile 中按 stdout 是否 TTY 选择输出：终端直接显示编译、DTS 诊断和 QEMU 内核/用户态输出；非 TTY 将整次输出保存到 `artifacts/run/`，只显示开始、完成或失败及日志路径。演示完成仅表示运行结束，不表示自动验收通过。实时演示不额外捕获日志，需要留存时由调用者重定向或使用 `tee`。

`ERHINO_OUTPUT=live just virt` 强制实时演示，`ERHINO_OUTPUT=summary just virt` 强制概要；通过管道或 `tee` 查看实时输出时需显式选择 live。该开关仅控制演示，不影响验收与测试。

自动验收和测试始终隐藏内部输出，包括构建警告和 QEMU 输出；终端只报告结果、退出码和日志位置，失败详情查日志，不自动回放日志末尾。Justfile 将构建及诊断器输出保存在 `artifacts/run/`；`qemu-acceptance.sh` 独立保留失败 QEMU 日志及元数据，`QEMU_ACCEPTANCE_KEEP_LOG=1` 可额外保留成功现场。clippy 同样只报告各分面结果和日志位置。底层 `build_kernel`、`build_user`、`make_dtb`、`run_qemu` 命令保留原始输出，便于直接诊断。

通过规定检查后停止；只有新改动、失败或具体未解决风险触发补测。挂起与损坏按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 调查，工具、GDB 和汇编细节见 [TOOLING-PITFALLS](TOOLING-PITFALLS.md)。
