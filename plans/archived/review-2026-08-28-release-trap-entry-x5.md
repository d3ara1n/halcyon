# release 构建不收束调查：trap 入口破坏用户 x5

> 调查归档（2026-08-28，只读）。症状首次入档于 KNOWN_ISSUES「release
> 构建不收束」，当日定位并修复；本文件留调查路径与证据链。

## 症状

`MODE=release`（virt 4 核，节流 50%）全负载走到 `init: steady-state
supervision` 后无 `[Sched] system quiescent` 停机、QEMU 不退出；debug
同负载稳定收束。伴随分叉：pm 委托域走 `not collected by pm; init
collecting` 兜底、`job kill composition FAILED: InternalError`、最终拓扑
root 0/0、pid 9（srv_target）泄漏存活。干净 HEAD（b5c2bfe）同样复现，
与 step 7 改造无关。

## 排除过程

- **速度/时序排除**：debug `THROTTLE=100`（全速）收束正常；release
  `THROTTLE=25`（1/4 速）仍挂起且仍走同一兜底路径——非竞态时序。
- **二进制定位**：全部失败点（pm 枚举、derive-kill、job_kill 组合、
  acceptance 收集、拓扑快照）共用 `enumerate_job` 原语；插桩显示 rinlib
  契约校验拒绝的异常值是 `cap=0`（128 元素数组的 `buf.len()` 在 syscall
  返回后读作 0），其余字段（next_cursor/actual/more）均为合法内核结果。
- **调用方/被调方反汇编**：调用方正确传 `a5=128`；被调方把 `buf.len`
  存于 **t0(x5) 跨 ecall**，返回后读到 0。
- **内核侧入口观测**：ecall 到达时（asm 已保存 UserContext）帧内
  `x5=0` 而 `a4=0x80` 正确——t0 在 trap 边界即被破坏，非内核执行期。
- **决定性对照**：用户侧 `raw_call` 显式声明 `out("x5") _` 等后 release
  全绿——确认内核往返破坏 x5。

## 根因

`_trap_entry` 在保存 UserContext **之前**用 t0 做 SPP 来源检查
（`csrr t0, sstatus; srli; andi; bnez`），随后 `.irp` 保存序列把已被覆写
的 x5 存入帧——**每次用户 trap 都把用户 x5 覆写为 SPP 位（U 态来源恒
0）**。debug 用户代码不在 ecall 周边把活值留在 t 系寄存器，破坏不可
观测；release 代码生成会（slice 长度跨 syscall 存 t0），触发 rinlib
枚举契约连锁违约。

停机失败的传导链：x5 破坏 → 枚举 InternalError → `job_kill` 组合失败
→ pid 9（srv_target `sys_sleep(1000)` 死循环）泄漏 → 期限表永驻 1 秒
重登记的 deadline 条目 → `is_quiescent`「期限表空」条件永假。停机谓词
本身行为正确（活进程应阻止停机）。

## 修复

SPP 检查改经**已保存**的 t5 中转（其原值已存入 scratch 槽 1）；
进入序列在 UserContext 保存完成前不再触碰任何未保存的用户寄存器。
纪律入档 `notes/impls/execution-context.md`「Trap 与 CSR」。

## 验证

debug virt、release virt ×5（InternalError 清零、composition 通过、
pm 委托域 confirmed Dead、拓扑与 debug 一致、quiescent 停机）、
sifive_u debug 全绿。

## 教训

- trap 进入序列的寄存器纪律是 ABI 契约的一部分，须成文并在新增进入
  序列时对照（本次入档 execution-context.md）。
- debug 构建不可作为用户侧寄存器保持语义的验证：两侧代码生成策略不
  同，debug 只测出内核侧逻辑。release 验证线应尽早接入常规验证（本
  次为首次 release 运行即暴露）。
- 诊断时「加打印即消失」的 Heisenbug 指向代码生成/寄存器分配差异，
  应第一时间反汇编对照而不是继续调时序。
