# 启动资源与用户态 launcher 实现

方向见 [`../ideas/bootstrap.md`](../ideas/bootstrap.md)、[`../ideas/object.md`](../ideas/object.md) 与 [`../ideas/task.md`](../ideas/task.md)。当前启动链已经收敛为 BootPackage → 唯一 init → 用户态 launcher；内核不再遍历归档或识别服务名称。

## BootPackage v1

`shared/src/boot.rs` 定义 64 字节 little-endian envelope：magic、version、header_len、flags、total_len、initial ELF offset/length、payload offset/length 与 reserved。validator 使用 checked arithmetic，要求 canonical offset、页对齐 payload、零 padding 和窗口内完整几何；payload 可以为空。

`tools/make-boot-package.py` 原子生成 `artifacts/boot-package.bin`。Just 构建链把 `srv_init` 作为唯一 initial ELF，其余测试程序暂以确定序 ustar 组成 opaque payload。DTS `/chosen/boot-package` 只声明物理装载窗口；`os/kernel/src/board.rs` 同时验证窗口完整落在 DT memory 内。

`os/kernel/src/boot.rs` 在帧池注册前读取并验证 envelope，以实际 `total_len` 收窄保留区；DTS capacity 不会整段退出帧池。内核只把 initial ELF 交给 bootstrap loader，不解释 payload 字节。

## StartupBlock outer ABI

`shared/src/startup.rs` 定义：

```text
[StartupBlockHeader (40 B)]
[Handle × handle_count]
[zero padding]
[opaque payload]
```

Header 包含 magic、版本、块长、pid、parent_pid、Handle 数、payload 偏移/长度与 reserved。几何不变量是 `handles_end <= payload_off`；间隙必须为零。普通 builder 仍生成紧凑块，bootstrap builder 允许把 prefix 补齐到页边界。

Handle 区直接保存 child HandleTable reservation 产生的真实值，不允许由 index 推导 slot/generation。validator 只校验 outer、实际 Handle 与 padding，不解释 payload。

## 唯一 init bootstrap

`os/kernel/src/boot.rs` 是 BootPackage initial-process loader；它不依赖 `tar` crate、不查找 `bin/`、不含 pm/fs/driver 策略。流程是：

1. 解析 initial ELF，创建 pid 1 的 AddressSpace；
2. 创建 root Job，并为 init 生成完整 JobControl；
3. 以实际 child Handle 构造页对齐 StartupBlock prefix；
4. prefix 使用 owned 只读页，payload 以 BootPackage 保留区帧直接映射 `U|R|A` PTE（映入即收编为该地址空间的 owned backing），二者组成连续用户 VA 块；
5. 预构造主线程、插入进程表并 enqueue。

initial ELF 复制完成且 StartupBlock prefix 构造后，`[package base, payload_pa)` 页对齐前缀立即回投帧池；payload 页在映入 init 时即收编为该地址空间的 owned backing，随地址空间销毁自然归还帧池——无 pid 特判、无启动保留洞滞留。最后一页可见尾部来自 packer 的零 padding，不可写、不可执行。

## 公开 Job/Process 构造 ABI

`shared/src/proc.rs` 与 `shared/src/call.rs` 定义 JobCreate、ProcessCreate、ProcessMap、ProcessWrite、ProcessStart 的 fixed-width ABI；rinlib 封装位于 `user/rinlib/src/process.rs`。

- `JobControl`：持 CREATE 的 capability 才能派生 Job 或创建进程；root Job 只交给 init。
- `ProcessBuilder`：affine Building authority，不可 duplicate；关闭即回收未发布 Process。
- `ProcessMap`：为 Building process 建 anonymous zero pages，最终 PTE 禁止 W+X。
- `ProcessWrite`：从调用进程已验证的用户缓冲向 Building backing 回填，不要求目标最终 PTE 可写。
- `ProcessStart`：验证入口 X 映射、栈 W 映射、profile、payload、grants 与输出，成功消费 builder 并返回 ProcessControl。

### ProcessStart 事务

提交前依次完成：

1. 拷入 descriptor、payload 与 grants；
2. reserve child Handle slots，以真实 Handle 构造并映射 StartupBlock；
3. 预构造 ProcessControl、主线程 Arc；
4. 在公平类就绪队列放入不可见 reservation marker；
5. reserve 调用者输出 slot，并在同一 HandleTable 锁下复检、原子 extract GRANT entries。

提交区只做已预留结构的替换、builder 消费、Handle commit、输出写回、ProcessControl 绑定与 ready marker 发布，不再分配。此前任一步失败都会回滚 StartupBlock、两类 marker、Handle reservations；调用者 grants 与 builder 保持原值。

未 Dead 进程的生命周期根是 Job 直接成员表（ProcessCreate 的 marker 即落在该表）；PID 由全局单调分配器分配、不复用。公平类队列同样用 marker 预留容量，`pick` 跳过 marker，`has_ready` 不把 marker 视为 runnable，因此不改变 FIFO 或静默判定。

ProcessControl 已发布 CLOSED 可等待终态，关闭 control 不杀进程；异步幂等 ProcessKill、固定宽 ProcessQuery、REAPABLE 电平与有界 ProcessDrain 已接入；递归 JobKill 由用户态管理者经 JobSeal、分页枚举、JobDerive 与 ProcessKill 组合（公共实现在 `libprocess::job_kill`）。D64 profile 在调度域 eligibility 接线前明确返回 NotSupported。

## 用户态公共 loader

`os/elf` 是内核 bootstrap 与用户态共同依赖的纯逻辑 ELF parser。`user/frameworks/libprocess` 负责：

- 要求 entry 落在实际 executable segment 字节区间；
- 拒绝 segment byte overlap、文件越界、W-only 与页级 W+X 权限并集；
- 合并连续同权限页并分块 ProcessMap；
- 分块 ProcessWrite program segment，依赖匿名页初始清零形成 BSS；
- 映射固定主栈并组装 ProcessStart descriptor。

这只是实现集中化，不产生 authority；调用者仍须显式持有 JobControl。动态链接器、共享代码页与 MemoryObject 不在当前实现面。

## 当前 init 集成政策

`user/systems/init` 从 `startup_payload()` 读取 opaque 字节：最小 ustar walker、按归档条目序启动，仍是私有政策。init 是持久 root supervisor：

- 拓扑：root Job 只含 init 与 services Job；服务全部入 services 域；
  `pm_domain`（委托域，预置 Running 靶，JobControl 经 StartupBlock grants 交 pm，init 保留复制件作直接收束权）与 acceptance（一次性验收自测收容所，用完 job_kill 收净）是 services 的子 Job；
- 监督：对每个服务保留 ProcessControl，等 REAPABLE|CLOSED → Drain 至 Complete → 固定宽快照 → close；不重启（重启政策维度存在，当前配置为无）；
- 委托：pm 只持 pm_domain 的 MANAGE|READ|WAIT（无 CREATE），对域内成员走 枚举 → 派生（铸造路径）→ kill → drain → 封口；
- 终态：全部收束后 init 常驻等待管理端点，不自我终止；系统经 quiescent 判定静默停机（IPC 等待者不阻止静默，见 `impls/internals.md`）。

manifest、initfs 正式格式与服务编排协议仍属未来设计，内核 ABI 不含这些概念。

## 验证

- shared host 测试覆盖 BootPackage canonical geometry、零 padding、空 payload 与 StartupBlock padded prefix；
- page_table host 测试覆盖跨子表 unmap、mega split OOM 保持原映射；
- libprocess host 测试覆盖 entry、segment overlap 与页级 W^X 拒绝；
- HandleTable host 测试覆盖 reservation、TRANSIT/GRANT 与 badge；
- virt/sifive_u 均由 init 启动其余三类负载，完成现有 IPC/FAL/Runnel/Job 管理面验收、pm 委托域收束、全员回收与静默判定；virt 经 SRST 自退，sifive_u 无 shutdown 设备以日志关键行判定。
