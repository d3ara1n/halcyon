# 构建与验证

构建、测试、QEMU 或调整验收工具前读取本篇。命令真值在 [Justfile](../Justfile)，验收目录在 [acceptance.py](../tools/acceptance.py)。任务完成条件由 [AGENTS](../AGENTS.md#任务适用范围与完成条件) 拥有。

## 验证边界

自动验收覆盖内核机制与公开系统调用契约。host 单测在纯机制边界验证状态转换、容量与回滚；QEMU 使用真实内核检查能力授权、映射、消息、进程、资源、时间和执行上下文。用户态夹具通过公开接口发起请求、检查结果和收束资源，不以 GDB 断点、源码行号、内部布局或人工安排内核执行窗口作为通过条件。

跨服务启动、RPC/文件/设备业务组合通过按需演示或人工验证确认，不进入自动内核验收门。普通内核路线通过只证明所列契约，不宣称服务业务、全部内部交错或故障注入已经覆盖。手动 GDB 故障调查与离线栈分析属于调查工具，不接入验收目录。

先确定受影响的 package、运行环境和契约，再选择检查。局部通过后停止；同一版本、配置和输入已经通过的检查不重复执行。普通验收、压力和额外平台是分别选择的活动，没有自动串联关系，不把历史专项或完整目录变成每次交付的固定门。

| 改动 | 起始验证 | 扩大范围的依据 |
|---|---|---|
| 文档、注释 | 差异与引用核对 | 实际改变构建或执行语义 |
| 纯逻辑 crate、局部依赖 | 对应 package 的 check/clippy、host 单测 | 实际消费者受 API、布局或行为变化影响 |
| 用户态库/服务 | 对应 target 检查、现有 host 测试，必要时人工运行 | 具体消费者或业务责任链发生变化 |
| 内核机制/共享 ABI | 内核和实际 ABI 消费者检查、相关 host 单测与内核领域验收 | 契约跨越内核领域，或局部失败指向相邻机制 |
| 编译器、底层运行时、目标配置 | 受影响 target 编译及所需执行平台 | 广泛影响代码生成或平台行为 |
| 竞态、资源退出 | 对应普通组；必要时显式选择既有压力 | 普通行为无法观察目标风险，且压力具有独立判定价值 |

失败先保留现场，区分构建、工具、guest、超时与清理问题，再定位当前路线。测试维护成本高于证据价值时移除，并在所属记录说明失去的覆盖；不为旧判据添加产品注入开关或另造测试框架。

QEMU 运行范围由用户决定；agent 不主动启动。用户要求逐条运行时，每条核对结果与清理后再启动下一条，不使用聚合或循环批量启动。当前分组与证据见[验收记录](archived/todo-2026-10-29-acceptance-groups.md)。

## 工具链与构建

开发机为 macOS，使用 Homebrew 的 just、dtc、qemu、riscv64-elf-binutils；手动调试可使用 riscv64-elf-gdb。系统级安装由用户执行。Rust 工具链由根目录 `rust-toolchain` 选择，当前为浮动 nightly；比较代码生成时记录实际版本。

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

`check`、`clippy` 无参数只显示选择；可传 package，或 `formation-host/shared-host/os-host/kernel/user-host/user/fp` 分面。`all` 显式选择全部静态分面，不运行 QEMU。user 分面覆盖库与程序，浮点程序由 fp 分面独立覆盖。`test-host` 只选一个库 package，可追加 testcase 名称；host target 从 rustc 获取并始终显式传入，不能继承 RISC-V 默认值。formation 的 host 检查/测试启用 host 编译器 feature。

`build_user` 用组态编译器的 `--list` 模式先合并和校验组态，列出引用映像；仅构建这些映像及 srv_init，按选择审计 ELF。`make_initfs` 用同一编译器核实际映像、编码与解码一致性，装入 `formation` 与 `image/<组件>`，strip 只操作归档副本。TOML 没有第二套解析/合并实现。

`ERHINO_FORMATION_SOURCES` 选择 `user/formations/` 下的源名列表，后层深合并覆盖前层；直接构建/演示默认 system。验收目录明确设置来源与判定字段。boot package、initfs 和导出 ELF 路径是串行共享产物，同一 checkout 不并行构建或运行 QEMU。

## 内核验收入口

```sh
just acceptance                          # 只列目录
just acceptance memory                   # 普通映射、Pool、付款与退款
just acceptance ipc
just acceptance process
just acceptance tunnel                   # 内核 Tunnel 能力、几何与生命周期
just acceptance time
just acceptance resources mmio
just acceptance resources irq
just acceptance resources dma
just acceptance platform fp
just acceptance resources irq-exit       # 公开 WaitOne/WaitSet 使用者退出与退款
just acceptance resources dma-shared
just acceptance resources dma-quota
just acceptance platform nofd
just acceptance process stress           # 显式压力，不隐含在普通组中
just acceptance all --dry-run             # 展示普通内核路线，不构建或运行
```

普通领域默认 release；`--mode debug` 可显式选择调试构建。无参数目录拥有全部可选 case。当前普通集合为 memory、ipc、process、tunnel、time、MMIO、IRQ、DMA、FP 九条；`all` 仅显式顺序运行这九条，首错停止，不含 host 测试、clippy、压力或额外平台。

| 分组 | 覆盖与边界 |
|---|---|
| memory | 映射、进程付款、双维 Pool；stress 为既有线程/映射压力，不宣称观察到内部 RetryFault |
| ipc | Sender、Mailbox、Notification、WaitSet、Once；stress 增加控制面重复操作 |
| process | 出生、Job/派生、Kill、已提交调用者退出；stress 增加非空 WaitSet 自动退休与16场景竞态矩阵 |
| tunnel | 内核 Invitation/Endpoint、映射冲突、独立 View、几何、关闭与退款；stress 重复生命周期；不装配 pm/RPC 业务消费者 |
| time | 公共 Deadline、超时与因果时间读取 |
| resources | MMIO/IRQ/DMA 的授权、撤销、退出与收束；irq-exit、dma-shared、dma-quota 分别检查公开退出、共享 backing 独立退休、额度恢复；不读取内部 Waiting/Publishing 等阶段 |
| platform | boot/nofd/sifive_u 使用最小出生/收集 task；nofd 核 Base64 域；fp/hetero 运行浮点任务，hetero 另核域划分；不携带业务服务 |

每个领域仅装配自己的 task 和必需辅助映像，资源组才取得平台根。suite/mode 由 `libprocess::birth` settings 选择，错误组合在运行前拒绝。init 只解释正式组态，不知道测试领域。普通组与压力的退款断言不完全相同，压力不代替普通组。

## 判定、日志与清理

[runner](../tools/qemu-acceptance.py) 要求所选 suite/mode 身份、领域完成、init task 完成、全部实例收集及 Requested Shutdown；FP 另核浮点结果。panic、系统失败终局、意外 reboot、非零退出、缺锚点或清理失败均失败。领域完成日志出现在 guest 检查和 Job 收束之后，不能以中途锚点冒充成功。

正常 virt 要求命令成功退出。sifive_u 没有 shutdown 后端：只有全部成功锚点已出现、明确 reset 失败才允许主动收割，超时仍失败。该例外不放宽其他平台判据。

自动检查只显示摘要；完整构建输出写 `artifacts/run/`，check/clippy/host 写 `artifacts/checks/`。每次验收在 `artifacts/acceptance/` 保存 guest.log 与 result.json，记录选择、命令、提交、ELF/boot package hash、退出与耗时。成功、失败日志均保留。

runner 用独立进程组启动命令，finally 收本轮组。[qemu_support.py](../tools/qemu_support.py) 先回收已退出的直接子进程，再 CONT/TERM，有限等待并核 PID/PGID，必要时 KILL 后复核组消散。leader 退出或信号发出不等于清理完成；清理错误不能覆盖为成功。手动调查工具也须遵守自身进程组清理。

默认 THROTTLE=50。超时用于防挂死，不是性能要求；默认值由 Justfile 唯一维护，环境变量可覆盖，不含编译。确认有界推进后撞限时按项目超时约定处理；panic、主动收割或工具错误不算撞限。耗时异常先检查测试范围、重复工作和判定价值，不能仅靠提高超时维持日常验收。

## 演示、清理与调查

`just virt [mode]` 默认 debug System；`just system [mode]` 明确选 System，默认 release；`just sifive_u [mode]` 默认 release System。它们用于手动运行，不判定自动验收，也不因空闲关机。各路线生成对应 DTB；`ERHINO_DTB` 可选直接演示的 DTB，验收使用明确的平台变体。

演示在 TTY 显示实时输出，非 TTY 写 `artifacts/run/`；`ERHINO_OUTPUT=live|summary` 可覆盖。临时演示由启动者负责退出与清理。手动调试步骤见 [README](../README.md#运行与验收)。

`just clean-preview` 显示 [clean-build.py](../tools/clean-build.py) 的产物清单，`just clean` 执行；日志与证据不删除。清理时不能有其他任务使用产物。tar 使用 ustar 并关闭 AppleDouble，避免 `._` 文件进入 initfs。

sifive_u 对应 HiFive Unleashed：hart0 无 MMU，可运行 hart1–4，DRAM128MiB、timebase1MHz，boot hart不固定。运行时探测现代 SBI，不恢复旧 ABI 或专用内核政策。DMA 测试不向硬件交付地址，不证明 DMA 静止；维护栈证书仅约束固定 ELF/源码。

挂起、损坏与非确定性故障按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 调查，工具问题按 [TOOLING-PITFALLS](TOOLING-PITFALLS.md) 处理。历史时钟等偶发异常只按 [KNOWN_ISSUES](../KNOWN_ISSUES.md) 的复发条件接续，不加入日常自动验收。
