# 演示与验收的输出边界

状态：已实施，针对性验证与交付前检查完成，随本次构建入口整理提交。基线为 `8a2a178`；本次同时交付 [DTB 冷启动目录修复](todo-2026-10-11-dtb-cold-start.md)。

## 已确认边界与最终实现

用户明确：`virt`、`sifive_u` 等是演示运行，终端用户需要完整的编译与系统输出；验收/测试始终隐藏内部输出，只报告通过或失败、退出码及日志位置，失败详情通过日志查询。按用途先区分入口，TTY 只决定演示的显示方式。

- `Justfile` 的 `virt [mode]`、`virt-release`、`sifive_u [mode]` 直接调用 `run_qemu`，不再进入验收脚本。默认档位保持 debug/release/release；当前运行的用户态负载仍为 core。演示不做验收锚点判定、验收超时或终态自动收割；sifive_u reset 失败后的现场由用户 Ctrl-C 退出。
- 独立 `acceptance-virt [mode]`、`acceptance-sifive_u [mode]` 承接原 core 自动验收，默认 release。stress、hetero、nofd 及故障注入入口继续是测试。聚合改为调用这些验收入口，保留原 workload、档位、超时及判定覆盖。
- Justfile 内一个输出 recipe 处理展示：演示默认按 stdout TTY 决定直接 exec 或重定向整次输出；`ERHINO_OUTPUT=live|summary` 只影响演示。非交互演示和全部测试在 `artifacts/run/` 留日志，只显示开始与结果。测试忽略 live 覆盖，失败不回放内部输出；退出码仍向上传递。Cargo build 不用 `--quiet`，终端演示能看到编译进度。
- `qemu-acceptance.sh` 始终写独立日志，不判断 TTY、不转发实时输出，也不回放末尾日志。成功报告通过，失败报告原因和日志位置，并在 metadata 记录原因。required/rejected/terminal 集合与运行监督路径保持基线行为。
- 移除本会话早期方案新增的 `tools/run-output.py`、FIFO/tee、进程组包装及其展示变量；输出职责收回 Justfile 与既有验收脚本。clippy 失败同样只报告结果与日志位置。

长期用法由 `plans/BUILD-AND-TEST.md` 拥有，README 提供入口。本次不变更 Rust、ABI、内核/用户态机制或正式验收消费者。

## 验证与检查

最终结构证据位于 `artifacts/demo-output/`：

- `results.json`、`demo-summary.log`：真实非交互 `just virt-release` 正常 shutdown，退出 0；终端只有两行概要，日志包含编译与系统输出，没有验收成功标签。
- `demo-live.log`：真实 PTY 运行 `just virt-release`，退出 0；终端直接收到编译进度、DTS 诊断和系统输出，QEMU 退出前已经观察到 core 日志，结束没有验收通过标签。
- `acceptance-tty.log`：真实 PTY 中设置 `ERHINO_OUTPUT=live` 执行 `just acceptance-virt`，仍只显示概要；core 验收和 shutdown 通过，退出 0。显示开关不能把测试变成演示。
- `failure-summary.log`、`acceptance-failure-summary.log`：输出 recipe 与直接验收脚本都保留命令失败码 17，内部错误文本仅在日志，即使显式 live 也不回放。
- 比较基线与最终脚本的 required/rejected/terminal 和 workload 分支逐字一致。Justfile 聚合展开确认仍为 clippy + debug stress + release core、release sifive_u + release nofd 及四条故障测试，不调用演示。
- Bash 语法、Justfile 解析、`git diff --check` 通过；运行后无本次 QEMU/节流/验收进程残留。此前 FIFO/进程组包装测试不再作为最终实现证据。

验证过程曾因 Cargo 在真实 TTY 输出 ANSI 样式而误判未出现 `Finished`，修正临时检查脚本的文本匹配后重测通过，没有修改产品实现来迎合检查。

未重跑 Rust check/clippy、全量压力/平台及 GDB 故障组合：本次改动只改变命令入口和输出，原验收判定及平台参数不变，真实系统验证覆盖演示和 core 自动验收。sifive_u 新演示入口与独立验收入口已核对命令展开，没有新增板级运行证据。

交付前检查确认演示与验收入口职责分离、旧通用输出包装完全删除、文档与源码一致，没有未闭合责任与延期项。用户已授权提交，并明确本次属于构建入口与输出整理，不涉及系统代码和机制修改，不登记独立 Review。
