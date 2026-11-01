# 移除客体测试设施，保留 host 验证

状态：已完成设施移除、静态/host 验证和交付前结构检查，用户已授权本轮整理提交。基线 `964b77a`，分支 `refactor/system-redesign`，接手工作树干净。本篇是本轮唯一工作记录。

## 已确认范围与取舍

用户决定移除专门的测试进程、测试驱动和客体测试 harness，保留可在 host 验证的测试，后续围绕正式用户态环境开发与使用取证。已实现机制的精确交错覆盖缺口随真实反例或责任链变化处理，不提前补建测试应用或故障框架。尚未实现的硬隔离、持久化与恢复能力仍须在承诺相应使用范围前完成。

本轮删除四个 `user/tests/test_*` 进程、旧 IPC 验证负载 `srv_pm`、`libprocess::race`、专用组态、QEMU 自动验收目录/runner 及其专属测试和脚本；移除内核 boot 调用的 ClockState、MemoryPool 与 frame 自检。正式运行中的输入校验、安全断言和错误处理保留。PCI/xHCI/HID 是正式驱动，继续保留。

正式 System 组态、构建与 ELF 审计、QEMU 演示、GDB 手动调试和离线栈分析继续使用。仅服务于旧 IRQ 消费者的时钟调查脚本随依赖删除，历史异常与原始证据不核销。纯机制 Rust host 测试和构建工具 host 测试保留；删除设施自身的测试随被测对象删除，公共输出参数边界用例保留。

## 依赖盘点与实施顺序

- `srv_init` 已不依赖测试 crate；删除测试 workspace 成员及独占依赖后，System 装配协议不变。
- `srv_pm` 仍是旧运输/流控测试负载，正式 System 未装配它；`race` 的消费者仅为测试进程。
- `Justfile` 的验收 recipe、超时和 `test_fp` 构建分支，以及 `checks.py` 的 fp 特例一并删除。组态编译和按引用打包机制保留。
- `qemu_support` 的日志目录功能仍被静态检查使用；将其就地归入检查入口后删除无消费者的客体 runner 支撑模块。
- 删除内核自检入口及仅由其使用的辅助接口，保留正常启动初始化与资金机制。
- 更新 README、构建手册、AGENTS、受影响 impls 和当前任务导航。既有 archived 档案保持历史事实；活跃计划的历史运行结果不改写成当前覆盖。

本轮为测试设施移除，不设计新的 ABI、所有权、并发或服务能力；无新增锁、unsafe 或故障注入。自检删除只取消启动测试工作，不更改正式 ClockState/Pool/frame 算法。

## 验证门

静态检查覆盖修改的内核接线、用户态 workspace 与组态编译；构建正式 System 的 initfs/BootPackage 和内核以核实真实链接与打包。运行受影响 formation/libprocess 现有 host 测试、保留的输出包装 host 用例及脚本语法检查；核 workspace/lockfile、删除路径引用、文档与 diff。全程不启动 QEMU，不把编译或 host 成功表述为系统运行验证。

## 覆盖变化与后续

移除内核 syscall/跨进程生命周期、MMIO/IRQ/DMA、浮点/异构、压力与竞态矩阵的自动运行覆盖，以及启动时真实 Pool/frame 付款退款和 ClockState 样本自检。既有纯机制 host 测试不替代这些集成证据。历史通过结果仍只对应原提交与配置。

后续进入首个可交互用户态应用的现状盘点与设计，优先明确输入输出、应用装配和生命周期；尚未授权本轮实现控制台或服务管理器。真实使用发现问题时保留版本、实例/原轮身份、首个错误与收束状态，再针对具体缺陷修复并在可行的 host 边界补回归。

## 实施与验证记录

- 已删除 53 个专用文件：四个测试进程、旧 `srv_pm` 负载、竞态协议、19 份专用组态、自动目录/runner、DTS 变体脚本及依赖旧 IRQ 消费者的时钟调查脚本与其支撑模块。正式 System 仍只装配三宿主及 PCI/xHCI/HID 驱动。
- 内核只删除 boot 自检调用、对应测试体及两个无正式消费者的 FundedFrames 查询方法。第一次 `just clippy kernel` 因这两个方法变为 dead code 退出 101，原日志 `artifacts/checks/20261101T094947.834633Z-4240/kernel.log` 保留；删除后通过。
- `just clippy kernel` 通过：`artifacts/checks/20261101T095144.427086Z-4532/kernel.log`。
- `just clippy user` 通过：`artifacts/checks/20261101T095148.771914Z-4583/user.log`。
- `just test-host libprocess -- --features management` 通过，16 项单测及两组各 4 项集成 host 用例，共 24 项：`artifacts/checks/20261101T095344.332358Z-4896/libprocess.log`。
- `just test-host formation` 通过，6 项：`artifacts/checks/20261101T095350.910383Z-4977/formation.log`。
- `just output host-only-system-build just make_boot_package build_kernel` 通过：`artifacts/run/20261101-175358-5018-host-only-system-build.log`。七个正式用户态 ELF 与内核审计通过；链接器报告一个 LOAD segment 的 RWX 警告，未在本轮修改链接布局或消除此警告。
- 迁移保留的 `tools/tests/test_output.py` 参数边界用例通过；脚本语法检查通过。未给删除设施另造替代测试。Rust 最后仅整理删除留下的空行，语义未变，不重复已通过构建。
- 构建与源码扫描确认当前运行代码没有测试进程/自检/验收入口引用。固定 `maintenance-stack-certificate.json` 仍保留当时源码 hash，这是历史证书，不改写或宣称绑定本轮 ELF。Cargo.lock 由构建按实际 workspace 更新，仓库不跟踪 lockfile。
- 回填了 startup/runtime、设备、IPC、文件/服务、时间、内存和执行上下文说明，去除已删测试消费者及旧 init 编排的现状叙述。AGENTS、构建手册、README、COMPASS 与各主计划接力已同步 host 自动验证和正式使用取证的边界。

## 交付前结构检查

主代理按 REVIEW 核对最终 diff：删除对象均为测试设施或其独占依赖；三个正式驱动、System 组态、统一出生/停止契约与公开内核 ABI 保留。没有新 owner、锁、unsafe、旁路权限或产品故障注入。普通运行的安全断言、ClockState 故障锁存、Pool 付款退款和 frame 清零仍由正式路径承担。纯逻辑 host 测试保留，输出包装用例随公共入口迁移；被测对象已删除的 runner 测试退出。

本轮取消了动态测试能力，不把它们记为 host 覆盖或已修复。未启动 QEMU、未取得本轮运行证据、未验证真实硬件或性能。没有专用测试替代设施待接线。此次交付为测试设施维护，不新增机制或设计 Review 登记；用户于 2026-11-01 授权整理为一次本地提交，未授权 push。

最终文档引用、组态打包清单、workspace、入口与差异检查见 `artifacts/host-only-validation/verification.json`。后续按上节进入正式用户态环境设计，不恢复旧测试清单。
