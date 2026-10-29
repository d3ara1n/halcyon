# 构建与验证

构建、测试、QEMU 或调整验收工具前读取本篇。命令真值在 [Justfile](../Justfile)，验收分组在 [acceptance.py](../tools/acceptance.py)。任务完成条件由 [AGENTS](../AGENTS.md#任务适用范围与完成条件) 拥有。

## 按改动选择验证

先确定受影响的 package、运行环境和跨组件契约，再选择对应检查。局部通过后停止；同一版本、配置和输入已经通过的检查不重复执行。全仓检查、所有普通领域回归、压力和故障注入是不同选择，没有自动串联关系。

| 改动 | 起始验证 | 扩大范围的条件 |
|---|---|---|
| 文档、注释 | 差异与引用核对 | 实际改变构建或执行语义 |
| 纯逻辑 crate、局部依赖升级 | 对应 package 的 check/clippy、host 测试 | 实际消费者受 API、布局或行为变化影响 |
| 用户态库/服务 | 对应 target 检查及可用 host 测试；需要运行时行为时选择所属领域 | 修改跨领域契约，或局部失败指向相邻依赖 |
| 内核机制/共享 ABI | 内核与实际 ABI 消费者检查，相关纯逻辑测试及领域验收 | 责任链跨领域、全局调度/内存/代码生成变化 |
| 编译器、底层运行时、目标配置 | 受影响 target 编译、所需平台能力和领域组合 | 变化广泛影响代码生成或多平台行为，选择更广普通回归 |
| 竞态、故障恢复、资源退出 | 对应普通组与相关 stress/故障场景 | 普通行为无法观察目标交错，或现场明确要求专项 |

失败后先保留第一轮日志，区分构建、工具、guest、超时和清理失败，再选择相邻依赖或组合测试。扩大范围服务于定位，不自动从头全跑。跨组件变更可以直接验证实际责任链，不必先等局部失败。历史专项不因年代删除，也不因过去通过而常驻所有改动的验收门。

QEMU 的运行范围由用户决定；agent 不主动启动。分组重建后的运行证据见[本轮记录](todo-2026-10-29-acceptance-groups.md)，未实跑的路线不能称为验收通过。

## 工具链与构建

开发机为 macOS，使用 Homebrew 的 just、dtc、qemu、riscv64-elf-binutils、riscv64-elf-gdb。系统级安装由用户执行。Rust 工具链由根目录 `rust-toolchain` 选择，当前为浮动 nightly；比较代码生成时记录实际版本。

三个 workspace 分别位于 os、shared、user。内核 target 为 `os/riscv64imac-unknown-erhino.json`，用户态为 `user/platforms/riscv64imac-unknown-erhino-elf.json`，均使用 build-std；test_fp 单独使用 gc target。内核 package、binary 和产物名均为 kernel。构建统一走 just。

```sh
just build_kernel
just build_user
just check kernel
just check test_kernel
just clippy formation-host
just clippy libprocess
just test-host formation
just test-host libprocess
```

`check`、`clippy` 无参数只显示选择；可传 package，或 `formation-host/shared-host/os-host/kernel/user-host/user/fp` 分面。`all` 显式选择全部静态分面，不运行 QEMU。user 分面覆盖库与程序，浮点程序由 fp 分面独立覆盖。`test-host` 只选一个库 package，可追加 testcase 名称；host target 从 rustc 获取并始终显式传入，不能继承 RISC-V 默认值。formation package 的 host 检查/测试启用 host 编译器 feature。

`build_user` 用组态编译器的 `--list` 模式先合并和校验组态，列出引用映像；仅构建这些映像及 srv_init，按选择审计 ELF。`make_initfs` 用同一编译器核实际映像、编码与解码一致性，装入 `formation` 与 `image/<组件>`，strip 只操作归档副本。TOML 没有第二套解析/合并实现。

`ERHINO_FORMATION_SOURCES` 选择 `user/formations/` 下的源名列表，后层深合并覆盖前层；直接构建/演示默认 system。验收目录明确设置完整来源与判定字段，不继承旧的负载选择。共享 boot package、initfs 和导出 ELF 路径仍是串行构建产物，同一 checkout 不并行执行构建或 QEMU 路线。

## 验收入口

```sh
just acceptance                              # 只列目录
just acceptance memory                      # 普通映射、Pool、付款/退款
just acceptance ipc
just acceptance process
just acceptance transport
just acceptance resources dma               # 只跑 DMA
just acceptance resources                   # 顺序跑 MMIO、IRQ、DMA，各一次
just acceptance services                    # 正式组态就绪、停止与撤销链
just acceptance platform nofd
just acceptance ipc stress
just acceptance resources dma-quota
just acceptance memory fault
just acceptance all --dry-run               # 展示具体命令，不构建或运行
just acceptance all                         # 显式所有普通领域，不含压力/故障/额外平台
```

普通领域默认 release，`--mode debug` 选择调试构建；外部观察器强制 debug。完整可选 case 由无参数目录拥有，以下解释覆盖边界。

| 分组 | 覆盖与边界 |
|---|---|
| memory | 普通映射、进程付款、双维 Pool；stress 只运行既有线程/映射 storm，fault 另观察真实 RetryFault，repair 注入受限 VM 修复故障 |
| ipc | Sender、Mailbox、Notification、WaitSet、Once；stress 加控制面重复压力 |
| process | 出生、Job/派生、Kill、已提交调用者退出；stress 加非空 WaitSet 自动退休和 16/16 竞态矩阵 |
| transport | RPC 拒绝/取消、Tunnel 几何/失败 owner、真实 pm 流与背压；stress 加 Tunnel 生命周期重复组合 |
| time | 公共 Deadline 与时间行为 |
| resources | MMIO、IRQ、DMA 各自授权/退出/收束；缺所选平台根明确失败。publication、forced、races、install-failure、irq-exit 是外部观察专项；shared/quota 只观察既有客体自检；retention 拒绝 reset 后核保留责任，IPI 组合另核失败唤醒 |
| services | System 三宿主 Ready 后运行 startup task，其完成触发反序停止、来源撤销与正常收集；没有 FS/设备业务验收 task，不宣称键盘热插或 Copy/Watch 客户端覆盖 |
| platform | boot 只运行出生/收集 task，fp 只运行浮点 task；nofd 运行 Base64 出生/收集 task 并核域声明，不再声称测试了 FP 启动拒绝；hetero 核域声明和浮点消费者；sifive_u 运行出生/收集 task 观察启动与终局，不携带全套业务。boot/IPI/funding 是显式故障诊断；clock 仅在历史异常复发或用户要求时运行 |

`all` 顺序执行 memory/ipc/process/transport/time、三项 resources、services、platform fp。任一失败即停止。它不包含 host 单测或 clippy，不重复已经显式执行的静态检查，也不隐含压力、GDB 或平台矩阵。普通与 stress 的退款断言并不完全相同，stress 不代替普通组。

有限服务部署要求存在 task；services 使用既有 test_kernel 的 startup 分支，在三宿主 Ready 后仅采用出生输入、收集自身 Job 并正常退出，不运行其他领域检查。每个 guest 领域只装配自己的 task 和必需辅助映像；仅资源组收平台根。资源退出不经过通用 Job 业务。RPC/Tunnel 使用原 pm 组合消费者，因此 transport 组仍覆盖其必需 IPC/Job 接缝。suite 与 mode 来自 `libprocess::birth` settings，错误组合在运行前拒绝。init 只解释正式组态，不知道测试领域。

## 判定、日志与清理

[runner](../tools/qemu-acceptance.py) 要求所选 suite/mode 身份、该组完成、init task 完成、全部实例收集及 Requested Shutdown；services 还要求三宿主分别 Ready/Stopped，FP 要求浮点结果。panic、系统失败终局、意外 reboot、非零退出、缺锚点或清理失败均失败。分组完成日志出现在 guest 检查与 Job 收束之后，不能以子集的中途锚点冒充成功。

正常 virt 要求命令成功退出。sifive_u 没有 shutdown 后端：只有全部成功锚点已出现、明确 reset 失败才允许主动收割，超时仍失败。诊断型故障工具拥有各自预期故障判据，不能把故意故障视作普通组通过。

自动检查始终只显示摘要；完整构建/工具输出写 `artifacts/run/`，check/clippy/host 写 `artifacts/checks/`。每次验收在 `artifacts/acceptance/` 保存 guest 日志与 result.json，包括选择、命令、提交、ELF/boot package hash、退出与耗时；成功日志也保留，便于判断重复工作和比较运行耗时。专项观察证据保存在各自目录。

runner 用独立进程组启动命令，finally 收本轮组；外部观察器同样清理自己独立创建的 QEMU/GDB 组。共享 `qemu_gdb.stop_group` 先 CONT/TERM，有限等待并核 PID/PGID，必要时 KILL 后复核组消散。leader 退出或信号发出不等于清理完成；清理错误保留，不能覆盖为成功。

默认 THROTTLE=50；精确资源观察和 DMA 保留/时钟调查显式 100，GDB 停顿与节流结果均不作性能/WCET 证据。超时只防挂死，默认值集中在 Justfile，可用同名环境变量覆盖；不含编译，聚合没有额外总期限。确认仍在有界推进而撞限时翻倍对应预算并重跑；panic、主动收割和工具失败不算撞限，也不通过删场景掩盖。

## 演示、清理与调查

`just virt [mode]` 直接运行选定组态，默认 debug System；`just system [mode]` 明确选 System，默认 release；`just sifive_u [mode]` 默认 release System。它们不判定验收，也不因空闲关机。所有路线生成对应 DTB；`ERHINO_DTB` 可选直接演示的 DTB，验收入口使用明确的平台变体。

演示在 TTY 显示实时输出，非 TTY 写 `artifacts/run/`；`ERHINO_OUTPUT=live|summary` 可覆盖，自动验收不受其影响。临时演示由启动者负责退出与清理。

`just clean-preview` 显示 [clean-build.py](../tools/clean-build.py) 的构建产物清单，`just clean` 执行；运行日志与证据不删除。清理时不能有其他任务使用产物。tar 使用 ustar 并关闭 AppleDouble，避免 macOS 的 `._` 文件进入 initfs。

sifive_u 对应 HiFive Unleashed：hart 0 无 MMU，可运行 hart 1–4，DRAM 128MiB、timebase 1MHz，boot hart 不固定。继续运行时探测现代 SBI，不恢复旧 ABI 或专用内核政策。

GDB 物理地址转换与共享内存读取见 [qemu_gdb.py](../tools/qemu_gdb.py)，不依赖当前 hart 的 satp。DMA retention 不向硬件提交地址，不证明 DMA 静止。维护栈证书仍只约束固定 ELF/源码，不自动许可未来构建。

挂起、损坏或非确定性故障按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 调查，工具问题按 [TOOLING-PITFALLS](TOOLING-PITFALLS.md)。历史偶发项只按 [KNOWN_ISSUES](../KNOWN_ISSUES.md) 的复发条件接续。
