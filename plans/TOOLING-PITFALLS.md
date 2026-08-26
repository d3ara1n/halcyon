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

- `just virt` 默认节流到 50% CPU（Justfile `THROTTLE`）：guest 跑飞/panic 时 QEMU
  满核空转的兜底——QEMU 无内置 CPU 限制参数（`-icount` 对死循环无效），靠 OS 层
  SIGSTOP/SIGCONT 冻结进程实现，对 guest 透明（感知为时间暂停）。
- **调试（gdb -s）务必 `THROTTLE=100` 全速**：节流 = 周期性冻结整个 QEMU，
  gdb 交互一卡一卡、`-icount` 复现时序也被拖慢。
- 节流档下 QEMU 被后台化，终端 Ctrl-C 不再直达 guest（脚本捕获后清理退出）。
- 脚本退出/被杀必先 SIGCONT 解冻再清理，仅 SIGKILL 才可能残留 STOP 态
  （进程活着但完全不动），`kill -CONT <pid>` 解冻。

## 输出与日志

- **stdout 经管道/文件是缓冲的**：SIGTERM 杀 QEMU 丢失未刷出的尾部输出。
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
- sifive_u 无 shutdown device：负载完成后 QEMU 不自退出，`just sifive_u`
  已内置运行阶段超时收束，通过与否看日志关键行（全员回收 / `[Sched] 系统静默`）。
