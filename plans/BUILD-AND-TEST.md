# 构建与验证

构建、host 测试、QEMU 演示或调查前读取本篇。命令真值在 [Justfile](../Justfile)。任务完成条件由 [AGENTS](../AGENTS.md#任务适用范围与完成条件) 拥有。

## 验证边界

自动测试以 host 可执行的纯机制、协议和构建工具为边界，验证状态转换、容量、编码、授权规则与回滚。内核和用户态 target 继续做编译、clippy 与 ELF 审计；这些静态检查不证明真实系统调用、跨进程生命周期、硬件行为或全部并发交错。

运行验证围绕正式 System、用户态应用和实际设备开展，由使用场景与具体风险决定观察内容。保留代码检查与已有 host 回归；精确失败窗口随真实反例或相关责任链修改再取证，不为填满覆盖矩阵建设专门测试进程、测试驱动或客体 harness。已有能力的覆盖缺口与尚未实现的能力分别登记，长期无异常不自动记为已证明正确。

先确定受影响 package、运行环境和契约，再选择检查。同一版本、配置和输入已经通过的检查不重复执行，全仓检查不是每次交付的固定门。

| 改动 | 起始验证 | 扩大范围的依据 |
|---|---|---|
| 文档、注释 | 差异与引用核对 | 实际改变构建或执行语义 |
| 纯逻辑 crate、局部依赖 | 对应 package 的 check/clippy、host 单测 | 实际消费者受 API、布局或行为变化影响 |
| 用户态库/服务 | 对应 target 检查、现有 host 测试 | 正式消费者或业务责任链发生变化时，按需运行观察 |
| 内核机制/共享 ABI | 内核与实际 ABI 消费者检查、相关 host 单测和代码核查 | 具体运行问题或目标场景需在正式环境中验证 |
| 编译器、底层运行时、目标配置 | 受影响 target 编译及 ELF 审计 | 代码生成或平台行为需要实际环境取证 |

QEMU 运行范围由用户决定，agent 不主动启动。出现失败先保留版本、配置、首个错误、实例/原轮身份和收束状态，再区分构建、工具、应用与内核问题。具体修复后，在能表达该契约的最小 host 边界补回归，必要的运行复核沿原正式场景进行。

## 工具链与构建

开发机为 macOS，使用 Homebrew 的 just、dtc、qemu、riscv64-elf-binutils；手动调试可使用 riscv64-elf-gdb。系统级安装由用户执行。Rust 工具链由根目录 `rust-toolchain` 选择，当前为浮动 nightly；比较代码生成时记录实际版本。

三个 workspace 分别位于 os、shared、user。内核 target 为 `os/riscv64imac-unknown-erhino.json`，用户态默认 target 为 `user/platforms/riscv64imac-unknown-erhino-elf.json`，均使用 build-std。`user/platforms/riscv64gc-unknown-erhino-elf.json` 提供浮点 target 描述，当前 System 没有单独的浮点程序构建分支。内核 package、binary 和产物名均为 kernel。构建统一走 just。

```sh
just build_kernel
just build_user
just make_boot_package
just check kernel
just clippy user
just clippy formation-host
just test-host formation
just test-host libprocess -- --features management
python3 -m unittest discover -s tools/tests
python3 -m unittest discover -s os/tools -p 'test_*.py'
```

`check`、`clippy` 无参数只显示选择；可传 package，或 `formation-host/shared-host/os-host/kernel/user-host/user` 分面。`all` 显式选择全部静态分面，不运行 QEMU。`test-host` 只选一个库 package，可追加 testcase 名称；host target 从 rustc 获取并始终显式传入，不能继承 RISC-V 默认值。formation 的 host 检查/测试启用 host 编译器 feature。

`build_user` 用组态编译器的 `--list` 模式先合并和校验组态，列出引用映像；仅构建这些映像及 srv_init，按选择审计 ELF。`make_initfs` 用同一编译器核实际映像、编码与解码一致性，装入 `formation` 与 `image/<组件>`，strip 只操作归档副本。TOML 没有第二套解析/合并实现。

`ERHINO_FORMATION_SOURCES` 选择 `user/formations/` 下的源名列表，后层深合并覆盖前层；默认 system。boot package、initfs 和导出 ELF 路径是串行共享产物，同一 checkout 不并行构建或运行 QEMU。

## 正式运行与调查

```sh
just system                 # 正式 System，默认 release
just virt                   # 正式 System，默认 debug
just sifive_u               # sifive_u System，默认 release
just clean-qemu             # 清理遗留 QEMU
```

System 常驻，不因空闲自动关机。运行入口不判定测试成功；应用行为、设备状态和退出结果由当次使用场景解释。sifive_u 没有 shutdown 后端，观察到 reset 失败后仍需操作者结束 QEMU，不能把主动结束等同正常关机。

`ERHINO_DTB` 可选择演示使用的 DTB；`make_dtb` 仍写所选平台的默认产物。默认 THROTTLE=50，以宿主 STOP/CONT 限制 QEMU 占用；手动 GDB 调试按需使用 THROTTLE=100。节流不是性能指标，比较耗时必须记录配置和负载。

GDB 手动调试步骤见 [README](../README.md#运行与验证)。挂起、损坏与非确定性故障按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 调查，工具问题按 [TOOLING-PITFALLS](TOOLING-PITFALLS.md) 处理。离线栈分析工具继续以固定 ELF/源码为输入，其证书不外推到其它版本。历史偶发异常只按 [KNOWN_ISSUES](../KNOWN_ISSUES.md) 的复发条件接续。

## 判定、日志与清理

check/clippy/host 日志写入 `artifacts/checks/`，终端显示退出码与路径。演示在 TTY 显示实时输出，非 TTY 写 `artifacts/run/`；`ERHINO_OUTPUT=live|summary` 可覆盖。保存日志不等于取得通过证据，编译成功、有限观察、完整业务完成和未覆盖部分分别记录。

临时运行由启动者负责退出与清理；手动调查同时管理 QEMU/GDB 和自身创建的辅助进程。不得只因 leader 已退出就推断后代已经消失。时间限制用于防止调查或工具卡死，不是性能要求；确认有界推进后撞限时按项目超时约定处理。

`just clean-preview` 显示 [clean-build.py](../tools/clean-build.py) 的产物清单，`just clean` 执行；日志与证据不删除。清理时不能有其它任务使用产物。tar 使用 ustar 并关闭 AppleDouble，避免 `._` 文件进入 initfs。

sifive_u 对应 HiFive Unleashed：hart0 无 MMU，可运行 hart1–4，DRAM128MiB、timebase1MHz，boot hart 不固定。运行时探测现代 SBI，不恢复旧 ABI 或专用内核政策。QEMU 使用事实不证明真实硬件的 DMA 静止或隔离能力。
