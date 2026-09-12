# 设计审查后续内存机制 Review

> 【未来审查计划】固定对象为提交 `4b27ce6`（可执行映射同步与表页初始化）与 `8aa7bc2`（对象来源保活与封印期收缩）。本计划只安排这两笔提交的事后 Review，不重开已归档的 A–E Review program，也不承接 RNL2 或 RPC deadline 的未来实现。

## 提交范围

### `4b27ce6`

- Running MemoryChange 从真实 Install/Protect intent 推导 instruction epoch，RX object Map 不再遗漏远端 `FENCE.I`。
- 删除 scheduler 重复 `fence.i`，保留 AddressSpace epoch 同步与 `_ret_to_user` 地址空间切换边界。
- 删除 funded table frame 交付后的第二次清零，初始化责任归 funding seam。

### `8aa7bc2`

- Unmap/Protect 在 Validate 的 AddressSpace 临界区冻结 `ObjectId + Arc<MemoryObjectCore>`，来源经 plan、prepared 与 rollback owner 贯穿 Commit 前窗口，失败不回查 live view 表。
- planner 标记 writable replacement 是否继承自旧 writable 区域；Sealing 允许原写范围内的切分或收缩，拒绝只读范围升权及新 writable view。
- `PermitRequirement` 由 planner 构造并整体交给对象状态机，对象锁内复核 ObjectId；调用点不自行声明 successor 属性。
- 冻结来源采用 Commit 前可失败分配的盒化 owner；表页 prepare 独立成帧，debug ELF 最大帧保持 `0x2720 < 0x2800`。
- host 与 `srv_init` 增加 Sealing 中段降权、重新加写拒绝、最后 permit 退役进入 Executable 的验证。

## Review 清单

1. RX Install 与进入或离开 RX 的 Protect 都推进 instruction epoch；Remote Resume、地址空间切换和空 active 集合均有且仅有足够的 `FENCE.I`。
2. Validate 后对象 core 由 frozen source、live view owner 或 retiring view 保活；prepare、shootdown、stale rollback 不依赖 live table。
3. `write_successor` 只能来自原区域 permit，且 replacement 范围是其子集；混合 RW/RO Protect 不得借退役 RW 为原 RO 升权。
4. successor 预留、Reserve rollback、Commit、Synchronize 和 Retire 全程不提前把 permit 计数降为零；容量耗尽返回全部 affine owner。
5. `MEMORY_OBJECT < ADDRESS_SPACE < LIFECYCLE` 保持单向；诊断断言不回取对象锁；表页拆帧不改变失败 owner 交还。
6. funded clear 覆盖 root 和中间表全部生产路径，删除调用点清零后不存在未清零表页。
7. 扫描旧残留：手工 instruction bool、permit rollback 回查 `view_core`、Sealing 一律拒绝 successor、重复清零或重复调度 fence。

## 已有验证证据

- `just check` 与七面 `just clippy` 通过。
- `memory_space` host debug/release 各 20 项通过。
- `THROTTLE=100 just virt`、`virt-release`、`virt-stress` 通过；stress 竞态矩阵 16/16。
- debug kernel ELF 最大帧 `0x2720`，未放宽 `0x2800` 门。
- 中间失败日志 `artifacts/failed-acceptance-20260908-145615-89136.log` 记录诊断断言曾逆锁阶取对象锁；最终实现改为冻结无锁 ObjectId 后通过同一路线。

## 边界与完成门

RNL1 回绕、Endpoint 安全 owner、共享字节访问与门铃握手的交付证据见 `archived/todo-2026-09-memory-object-data-plane.md`，固定提交范围由统一架构 Review 入口登记；单调时间与 RPC 全调用期限由 `todo-2026-09-monotonic-time-rpc-deadline.md` 承接，不在本 Review 扩大范围。

未来 reviewer 对两笔固定提交及其组合状态完成只读复核，记录新 finding 或确认无 finding，并核对对应验证后，本计划移入 `plans/archived/`。Review 失败只在本文件登记修复与复核，不创建重复 todo。
