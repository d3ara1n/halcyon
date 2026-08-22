# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## sifive_u 非确定性内核野跳转

四服务负载在 sifive_u 5 核下能全部装载、运行并回收，静默判定与 SRST
停机在多数轮次收敛（2026-09 地基工程后；此前的两大根因——IDLE_MASK
slot/raw 混用、Extend 单位错配引发的 OOM 自锁——已修复，见
`plans/2026-09-pre-ipc-groundwork.md`）。

剩余问题：约 1/3 轮次出现 S 态致命 trap——**cause=0xc（取指页故障）、
stval=0x0、sepc=0x0** 的内核野跳转，所在 hart 停驻进而阻塞静默收敛。
历史启动期成因（过渡页表并发清零竞态）已于 2026-08 修复，见
`plans/2026-08-execution-context-stall.md`。

### 2026-08 压测取证进展（诊断插桩未入库）

无缓冲串口（`-serial file:`）+ `-d int` 压测取证，已确证：

- virt 15/15 通过，sifive_u 高频失败——强时序敏感，非纯平台指令差异；
- 崩溃窗口恒定在 fs 用户 panic/回收前后，多 hart 连环崩，签名多样：
  pc=0/ra=0 野跳转、write_meta 写 PA0 别名（帧号 0）、memcpy 前置条件
  panic、TIMERS Vec 头损坏（sched.rs:192 空指针解引用）——指向
  **静态数据/堆被野写**，非单一指令缺陷；
- rdtime/senvcfg 非法为平台行为差异（OpenSBI M 态模拟/redirect），
  已排除为根因，详见平台 README（另：固件实为 v1.8.1）；
- `_fatal_entry` 曾复用 HL_SCRATCH 致转储 x30 实为 a0（已修）。

### 2026-08 深挖二：回归定位与嫌疑面收窄（诊断插桩未入库）

worktree A/B + 探针二分取得的硬结论：

1. **回归提交为 `623cba7`（IPC 前地基工程）**：该提交前的内核
   （9ddadcc，自带 initfs）跑同一负载 12/12 零崩溃；之后 ~100% 崩。
   该提交首次让进程回收真正把帧归还帧池——归还路径上线即带病。
2. **泄漏实验**：reap 时故意泄漏地址空间（不还帧）→ 零崩溃；只还
   表帧、泄漏数据帧 → 零崩溃。⇒ 元凶在**数据帧归还链**内。
3. **时序探针**：在 FrameTracker::drop 内加几条指令的真实延迟即
   100% 抑制崩溃（空操作探针不抑制）；延迟 ALLOC 侧无效。
   ⇒ 极窄的竞态窗口，且与归还时机直接相关。
4. 帧池 LR/SC→CAS 锁实现回退无效（锁无罪）；宿主端重放实测序列
   因串口日志丢失事件无法忠实重建（已建 `frame_pool/tests/replay_f3.rs`
   重放基建）；归还前填毒 0xA5 未在转储中现形。
5. 待验主假说：野写者把垃圾写进包括 **OpenSBI scratch（PA 0x80044xxx
   起，直映射可达）在内的任意内核可达内存**——ecall 返回时寄存器从
   被污染的 scratch 恢复，呈现「OpenSBI 脏值 + 客户机 sp/tp」混合现场，
   随后 pc=0 野跳。真正的第一写者在数据帧归还时机相关的路径上。

观察者效应已量化：FREE 路径加真实指令延迟即 100% 抑制、空操作探针
崩溃回归、全量内存记录亦完全抑制——窗口为纳秒级。取证基建已就位：
QEMU monitor `pmemsave` 提取 DIAG_BUF（崩溃后全 hart 停放、内存稳定，
物理地址直读，绕开 gdb 批量模式与串口乱流两坑）。

下一步候选：① `-icount` 固定客户机指令时序使竞态确定性复现，再叠加
全量日志（日志改变指令数但竞态仍以某速率显形）；② 精读 Drop 链与
等待模型发布时序的内存序论证；③ 抓第一笔野写的物理地址反查归属。

补充排除：帧池空闲链与 TableTree Drop 链的**顺序**逻辑均经宿主测试
验证无罪（`frame_pool/tests/replay_f3.rs`、`page_table/tests/drop_chain.rs`），
独立 review 亦结构性排除等待完成方与 reap 的 UserContext 交错
（Arc 强持有阻断）。⇒ 缺陷为纯多 hart 时序类，静态瞪眼与顺序重放
均不可达，须确定性复现或动态抓捕。

## page_table unmap_range 跨表批量解除算错子表基址

`os/page_table/src/lib.rs` `unmap_range` 对每个递归子表都用初始
`vpn_start` 推导 `table_base`，未传入该表实际覆盖的 VA 基址——跨
512 页边界的批量解除会解除错误的 PTE 区间并遗留残留映射，随后归还
数据帧即形成 UAF。当前唯一调用方 `extend_heap` 回滚逐页调用单页
unmap，不触发该路径；属潜伏缺陷，接入批量 unmap 前必须修复并补
跨表测试。

trap/上下文、hart 身份、能力调度与启动发布的已知契约缺口记录在
`plans/reviews/system-audit/01-sbi.md`、`02-trap-context.md`，统一设计见
`notes/execution-context.md`。在取得直接证据前不得添加平台专用补丁。
`just sifive_u` 已内置运行阶段超时收束，通过与否看日志关键行。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
