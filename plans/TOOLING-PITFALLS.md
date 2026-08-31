# 调试工具坑与手法（QEMU RISC-V virt/sifive_u）

> 持续累积的踩坑记录：每次调试新踩的坑合并进来，避免重复踩。
> 案例级的前因后果见各 dated postmortem（如
> `archived/review-2026-08-execution-context-stall.md`），本文件只收可复用的经验。

## 排查手法优先级

1. **`-d int -D file` 异常日志是第一利器**：完整记录每次 trap 的
   hart/cause/epc/tval。静默悬挂先看它——exec_page_fault 无限循环 =
   trap 进坏向量的死循环；最后一次正常事件到第一个异常之间就是案发窗口。
2. **legacy putchar 直写标记括夹**：绕开 console 锁与 DBCN，逐段二分定位，
   比反复猜快得多。单字符标记（'R'/'S'/'W'…）适合裸机早期无栈环境。
3. **抓悬挂现场**：循环脚本里 QEMU 带 `-s -d int` 后台起，检测到输出缺失
   即保持进程存活、gdb attach 读全部 hart 的 pc/CSR。判定以物理证据为准。

## gdb / QEMU gdbstub

- **软件断点在 Bare/过渡阶段插不上**：gdb 经当前 CPU 的地址翻译写内存，
  satp 关闭时高半区 VA 不可写，断点静默失效不报错。用硬件断点
  （`hbreak *地址` 或 `hbreak *符号`）。
- **断点要在 `-S` 冷启动时就位**：attach 到已悬挂的系统再设断点，代码早已
  跑过，永远命中不了。
- **boot hart 不保证是 CPU#0**：实测同一命令两次运行分别是 CPU#3 和
  CPU#0，按 mhartid 认领而非线程号。
- **sifive_u 上非 harts[0] 的 hart 断点打不上**（详见 KNOWN_ISSUES）；
  virt 上全 hart 可观测，CSR 读数可信。
- batch 模式下 continue 后紧跟的命令偶发 "Cannot execute this command
  while the target is running"：把序列拆短、输出重定向到文件、外层用
  timeout 包住 gdb 自身。

## CPU 节流（tools/qemu-throttle.sh）

- QEMU recipes 默认节流到 50% CPU（Justfile `THROTTLE`）：guest 跑飞/panic 时 QEMU 满核空转的兜底——QEMU 无内置 CPU 限制参数（`-icount` 对死循环无效），靠 OS 层 SIGSTOP/SIGCONT 冻结进程实现，对 guest 透明（感知为时间暂停）。
- 验收已经分档：`just virt`/`virt-release`/hetero/nofd/`sifive_u` 跑确定性 core，`just virt-stress` 才运行重复压力、最小预算 Drain 与完整竞态矩阵；阶段收尾用 `just acceptance` 聚合。2026-09 缓存构建实测 50% 档 core virt 约 10s、stress 约 57s，二者不可用同一超时。
- **调试（gdb -s）务必 `THROTTLE=100` 全速**：节流会周期性冻结整个 QEMU，使 gdb 交互卡顿并改变 `-icount` 复现时序。
- 节流档下 QEMU 被后台化，终端 Ctrl-C 不再直达 guest；脚本捕获后会先 SIGCONT，再终止并清理 QEMU。仅 SIGKILL 可能遗留 STOP 态进程，此时先 `kill -CONT <pid>`。
- `THROTTLE=100` 不等于绕过验收管道：100 档只是不执行 STOP/CONT，日志收割、锚点判定与路线硬超时仍然生效。计时对比必须使用同一 workload 与 throttle。

## 输出与日志

- **stdout 经管道/文件是缓冲的**：SIGTERM 杀 QEMU 丢失未刷出的尾部输出。
  「日志停在 X」可能只是缓冲假象——结论要靠 gdb/-d int 物理证据交叉验证。
  `tools/qemu-acceptance.sh` 挂起模式在终态锚点出现后也会主动杀 QEMU，
  收割前留 0.2s 等尾部刷出，判定仍以锚点集为准。
  「日志停在 X」可能只是缓冲假象——结论要靠 gdb/-d int 物理证据交叉验证。
- `-d exec` 日志在部分场景看不到内核地址的 TB（与实际执行矛盾，原因未知），
  勿作为证据。

## 汇编 / 构建链

- **集成汇编器会压缩指令**：C 扩展 target 下内联汇编/global_asm 里的
  `ld/sd/addi/ret` 可能被压缩成 C 编码。stvec 目标标签必须显式 `.align 2`
  ——低位非零会改变 stvec mode 字段（direct 变 vectored），这是潜伏雷。
- **改汇编后必须反汇编核对寄存器流**：删代码容易连带删掉寄存器真值赋值
  （曾把 PTE 写到 `_pa_fatal+slot*8`，故障率不降反升）；压测轮次要够多
  （30+）才能对比故障率。
- **内联汇编跨函数调用按 psABI 设 ra**；块外还有编译器生成的帧时任何路径
  都不得提前 ret，trap 与成功路径汇聚同一出口对称退栈。
- objdump 显示 `.insn 4, 0x…` 是 CSR 名不认识：手动解码 bits[31:20]
  即 CSR 地址（如 senvcfg=0x10A）。
- **编辑器 LSP 检查内核需自行注入 build-std**：os/ 的 config 不能全局
  开 build-std（host 测试与 sysroot 的 core/alloc 重复，E0152 实测），
  CLI 走 Justfile 注入；编辑器侧各自配——Zed 用 `.zed/settings.json`
  的 check.extraEnv 设 `CARGO_UNSTABLE_BUILD_STD`（只作用于 RA 自己的
  cargo check），否则内核 crate 全线 E0463「can't find crate for core」。
  换编辑器时按同思路配 LSP。

## 平台差异实证

- **QEMU sifive_u 的 U54 模型 senvcfg 可读不可写**：csrr 成功、csrw 触发
  illegal instruction。「实现了读」不等于「可写」，WARL 核验序列每一步都要守卫。
- sifive_u 无 shutdown device：显式 reset 返回失败后 QEMU 不自退出，`just sifive_u` 以 reset 后端失败或 panic 终态锚点主动收割，同时检查完整 core 锚点。该路线使用独立 `SIFIVE_U_TIMEOUT`（默认 45s）兜底真挂死；超时不得低于当前 core 验收面实测并应保留宿主抖动余量。
- **裸跑 QEMU 后必须 `just clean-qemu`**：验收脚本自己会清（挂起模式 trap 同时
  收 runner 与 tailer），但 agent 手动拼命令行跑完容易漏下孤儿进程占满核。
  已内置运行阶段超时收束，通过与否看完整业务锚点与显式 reset 结果。
