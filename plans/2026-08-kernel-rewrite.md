# 内核重写计划（os/ 推倒重实现）

> 决策：2026-08 与用户确认。**架构与设计全部保留，os/ 内核实现推倒重写**，在新分支进行。
> 本文件是重写会话的完整目标交代——新会话读此文件 + notes/ 即可开工，无需额外背景口述。

## 保留与替换

**保留（重写时不得改变，除非明确论证不适用）：**

- `notes/` 全部设计文档是架构事实来源：task（进程/线程=资源容器/执行容器概念）、ipc、message、tunnel、signal、fal、fs、device、ecs、call、service、framework。
- **shared/ ABI 不冻结**：最终 ABI 相似即可，允许随新设计演进（演进时内核与用户态两侧同步改）。重写期间现有 user/ 二进制持续作为集成对照负载。
- **user/ 与 rinlib 不动**。重写期间用户态二进制持续作为集成测试负载。
- **hart 分离设计**（每核独立调度上下文、`hart::` 模块划分）保留。
- 构建体系：Just + 自定义 target + linker 脚本的结构沿用（条目可精简，机制不变）。

**替换（实现质量问题的重灾区，重写重点）：**

- `mm/`：手写多级递归映射算法整体换设计（区域切段），规避清单见 `plans/2026-08-mm-map-bug.md`「重写注意事项」6 条——**必读**。
- `task/sched/` + 进程管理：初级实现全部重写；SMP soundness（`Arc<UpSafeCell>` 别名 UB）在新设计里消灭，方案备选见 `plans/2026-07-code-review.md` P0 #4。
- 全部 `static mut` 全局（PROC_TABLE / FRAME_ALLOCATOR / ROOT / MOUNTPOINTS / HARTS 等）与 `#![allow(static_mut_refs)]` 豁免。
- unsafe 边界整体收窄：只在 CSR/页表激活/trampoline 等真硬件交界处保留，且逐处 SAFETY 注释。

**顺带消灭**（来自 2026-07 review 清单，重写时按新设计自然解决，不单独打补丁）：P0 #4/#6、P1 的 `PageTableIter` 自引用、`extend` 2 的幂限制、`is_address_in` 下溢、`Send` syscall 语义、进程回收缺失、用户 fault 走 `todo!()` panic 等。

## 分支与基线

- 分支名建议 `rewrite/kernel`，从补丁后的 main 分出。
- 基线：2026-08 已打 mm 最小补丁（map/free 未对齐分支 `table`→`container`，两处），`just virt` 恢复可运行——重写期间旧内核是行为对照系统。
- 已知遗留（重写目标的一部分，不在旧树上修）：单核调度挂起（见 2026-08-mm-map-bug.md「附带发现」）。

## 里程碑（每个都设验收门，过门才进下一个）

### M0 · 地基起楼

linker script、rt（boot/panic/alloc——talc 堆，claim DTB 多段内存）、console、sbi 封装、board/dtb 解析。
**验收**：`just virt` 出 banner 与 `[Hart #N]` 行；`cargo check` 零 `static_mut_refs` 豁免。

### M1 · 同步原语与全局结构

自研关中断 Spinlock（LR/SC + sstatus.SIE，包装为 `lock_api::RawMutex`，注入 talc 与全局容器）；全局状态按 `notes/internals.md` 分层改造——hart 私有走 tp（HartLocal），init-once 用 `OnceLock`，标量用 Atomic，复合容器 `OnceLock<Spinlock<T>>`，禁 `static mut`。
**验收**：M0 验收保持 + 全仓 grep 无 `static mut`（外部汇编符号除外）。

### M2 · 内存管理（重写核心）

设计先行，见 `notes/mm.md`（帧池/sv39 纯逻辑 crate/高半区启动协议）。

frame 分配器 + 页表 + MemoryUnit。**必须先设计后编码**：区域切段算法（按当前级别对齐边界切 `[vpn, vpn+count)`，每段单一路径），LeafTable/MidTable 类型分离，映射冲突显式报错。
**纯逻辑 crate 双目标**：页表编码/索引/分配逻辑放独立 crate，`cargo test` 在 macOS host 上跑（不依赖 QEMU），内核 target 复用同一份代码。测试用例含 2026-08-mm-map-bug.md 的数值：`vpn=65, count=8192`（32MB 未对齐扩展）。
**验收**：host 单测绿（含未对齐/跨表/大页分裂/重复映射冲突/free 后重映射）+ `just virt` 用户程序加载运行。

### M3 · 任务模型

调度器、进程/线程（保留资源容器/执行容器概念，见 notes/task.md）。设计要求：SMP sound（无别名 UB）、单核不挂起（对照旧系统单核挂起问题）、抢占公平性可观测。用户态页故障 → 杀进程而非内核 panic。
**验收**：4 服务（fs/init/pm/drv_sifive_spi）在 `just virt` 4 核与 `-smp cores=1` 下都稳定完成用户态 main，无 panic 无挂起。

### M4 · IPC / FS / syscall 面

按 notes/ipc、message、tunnel、signal、fal、fs 实现，ABI 严格按 shared/，不新增不改号。内核侧 syscall 处理无 `.expect()` 杀内核路径。
**验收**：fs 的 main 全流程通过（建目录、遍历打印、属性读写、读 srv_init 前 8 字节）——这是旧系统的完整用户场景，作为对照基线。

### M5 · 收尾

删旧代码与过渡兼容层、清理 KNOWN_ISSUES 已消灭条目、`just virt` + `just sifive_u` 双平台过、更新 notes/ 中因实现变化需修订的段落。

## 验证体系（贯穿所有里程碑）

1. 秒级：`cd os && cargo check`；`cd shared && cargo check`
2. 毫秒级：host `cargo test`（M2 起的纯逻辑 crate）
3. 集成：`just virt`（4 核）/ `just sifive_u`——QEMU 不自退，agent 环境必须「启动→观察→kill」一条命令自包含；判定看启动日志关键行而非退出码

## 工作约定提醒

- 统一走 `just`，不裸 `cargo build`（linker/RUSTFLAGS 靠 Justfile 注入）
- 文档/注释/提交信息中文；AI 辅助的提交加 `Co-Authored-By` trailer
- 设计取舍记录进 notes/，代码不留历史痕迹注释
- 每个里程碑完成停下向用户确认再推进
