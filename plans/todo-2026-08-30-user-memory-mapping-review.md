# 用户内存映射与 ThreadSpawn 8B Review 计划

> 【未来审查计划】对象是用户内存切片 1–8B 的八笔提交；Review 纪律见 [`REVIEW.md`](REVIEW.md)。方向契约见 [`notes/ideas/mm.md`](../notes/ideas/mm.md) 与 [`notes/ideas/task.md`](../notes/ideas/task.md)，实现现状见 `notes/impls/{mm,call,internals,task,ipc}.md`，实施档案见 [`archived/todo-2026-09-user-memory-mapping.md`](archived/todo-2026-09-user-memory-mapping.md) 与 [`todo-2026-09-thread-model.md`](todo-2026-09-thread-model.md)。

## 提交对照

| 提交 | 内容 |
|---|---|
| `1cd6ab2` | 有界帧库存、order 树与 affine 帧所有权 |
| `6150d40` | 纯逻辑 `MemorySpace` planner、区域/事务/lease 状态机 |
| `6199985` | reservation-aware 页表发布、Remote Call 与 AddressSpace epoch |
| `c82d91a` | 统一 AddressSpace ledger/backing/PTE/MemoryChange 与终止接管 |
| `23ec19d` | Tunnel ObjectView、WritePermit、HandleClose 与 Drain lease 收束 |
| `6825e19` | 公开 MemoryMap/Unmap/Protect ABI、UserWriteLease 与 rinlib `MappedRegion` |
| `9358963` | 8A 单线程公开面、guard/termination 与完整平台矩阵 |
| `bdc83ef` | ThreadSpawn/Exit/Yield、ThreadControl DONE、guarded stack/join 与 8B 多线程矩阵 |

## Review 轴

### 单一地址空间与 affine 所有权

- AddressSpace 是否真正成为 Building、Running、bootstrap、Tunnel 与 Drain 的唯一 ledger/backing/PTE seam，旧字段、brk/Extend、按 VA 解除与旁路页表操作是否删除干净。
- frame inventory、OwnedExtents/ObjectView、WritePermit、UserWriteLease、MappedRegion、Handle pin 与 Remote slot 的每次进入和退出是否都有唯一结构所有者；失败、取消与进程消散时是否守恒。
- planner 与 kernel adapter 是否只在边界转换类型，不复制区间、owner、permit 或事务阶段真值。

### 事务、结果交付与失败原子

- `Validate → Reserve → Commit → Publish → Synchronize → Retire → Complete` 是否覆盖 Map/Unmap/Protect、Tunnel close、ProcessDrain 与 ThreadSpawn；Commit 前所有失败是否零业务副作用，Commit 后是否只剩有界必成工作。
- Map 固定物理结果投影、payload 写入与 cookie release 提交是否拒绝发起线程消散后的 uaccess；ObjectBusy 重试是否只发生在完整失败事务边界并保留 affine token。
- ThreadSpawn 固定宽输出失败是否正确回滚 Spawning/Handle/Ready，尤其复核 `ADDRESS_SPACE → LEAF` 反序修复及所有错误分支的锁释放顺序。

### 页表、Remote Call 与 RVWMO

- PTE Publish、epoch、Remote slot、SBI IPI、目标 `SFENCE.VMA`/`FENCE.I`、ack、Retire 与 backing 复用之间逐边重建 happens-before，不依赖未入档的锁实现偶然屏障。
- active snapshot/revalidate、enter/leave 与 Running→Terminating 是否闭合漏 hart、ABA、IPI 丢失和同 AddressSpace 多 hart 交错。
- mega leaf split、页表帧 reservation、部分 Unmap 与 Protect 重组是否保持 Publish 后不可失败及 Drop 子树守恒。

### 生命周期与双层义务

- 进程级 `mandatory_ops` 是否只保护 AddressSpace Drain，线程级 Map-result obligation 是否只保护成员摘除、ThreadControl DONE 与 JoinHandle 栈接管，两者是否存在错误替代或重复终局。
- ProcessKill 当前先由 mandatory completion 挡住 committed Map，因此线程级 obligation 不独立成为最终阻塞者；Review 应评估此防线在未来 ThreadKill/线程局部终止接入前是否有足够的模型或单元证据，不为制造日志添加测试专用 ABI。
- Memory completion、ThreadDeparture、Process REAPABLE 与 WaitContext completion 的发布顺序是否避免提前 DONE、提前栈解除、丢 wake 或反向 Lock Ladder。

### 用户态线程组合所有权

- `Builder → UserStack + Packet → ThreadSpawn → JoinHandle` 是否在成功、spawn 失败、显式 join、Drop 等待与进程终止路径保持唯一所有权；双 guard、sp 对齐、结果 release/acquire 与栈解除顺序是否成立。
- ThreadControl close 是否只消散观察权；首版无 detach/reaper/ThreadKill 是否保持接口极小且不锁死未来扩展。
- tid 单调、并发成员上界、Spawning→Ready 不可失败提交与 scheduler reservation 是否在 spawn/kill/exit 风暴下守恒。

## 验证复核

- 重跑全部 host 逻辑测试与 shared ABI；覆盖 planner、page_table、remote_call、wait_context、handle_table、frame_pool 与 sched_domain。
- common debug/release、virt-hetero、virt-nofd、sifive_u 均复核 13/13、同 AddressSpace stale translation、Tunnel lease 并发、join/stack 回收、末线程终局与 reset 锚点。
- 批三完成后把 spawn/kill、末线程 exit/kill、join 发布、线程风暴、壳 close 与 carryover IPC 压力结果纳入同一 Review，不以现有 8B 子集替代完整线程模型验收。
