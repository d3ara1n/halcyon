# 启动资源与用户态 launcher 实现

方向见 [`../ideas/bootstrap.md`](../ideas/bootstrap.md)、[`../ideas/object.md`](../ideas/object.md) 与 [`../ideas/task.md`](../ideas/task.md)。当前启动链是 BootPackage → 唯一 init → 用户态 launcher；内核不遍历归档、不识别服务名或服务拓扑。

## BootPackage v1

`shared/src/boot.rs` 定义 64 字节 little-endian envelope：magic、version、header_len、flags、total_len、initial ELF offset/length、payload offset/length和 reserved。validator 使用 checked arithmetic，要求 canonical offset、页对齐 payload、零 padding和窗口内完整几何；payload 可为空。

`tools/make-boot-package.py` 原子生成 `artifacts/boot-package.bin`。Just 构建把 `srv_init` 作为唯一 initial ELF，其余验证程序暂以确定序 ustar 组成 opaque payload。DTS `/chosen/boot-package` 只声明物理装载窗口；`board.rs` 验证窗口完整落在 DT memory 内。

`boot.rs` 在帧池注册前验证 envelope，并以实际 `total_len` 收窄保留区。内核只解析 initial ELF，不解释 payload。

## StartupBlock outer ABI

`shared/src/startup.rs` 定义：

```text
[StartupBlockHeader (48 B)]
[Handle × handle_count]
[zero padding]
[opaque payload]
```

Header 保存 magic、version、块长、pid、parent_pid、Handle 数、payload offset/length 与 reserved。几何要求 `handles_end <= payload_off`，间隙全零。普通 ProcessStart 构造紧凑块，bootstrap 允许把 prefix 补齐到页边界。

Handle 数组保存 child HandleTable reservation 生成的真实值，不从 index 推导 slot/generation。outer validator 不解释 payload。

## 唯一 init bootstrap

`os/kernel/src/boot.rs` 的流程是：

1. 解析 initial ELF，创建 pid 1 的 AddressSpace 与 root Job 成员 core；
2. 创建 root JobControl，并为 init 创建自身 ProcessControl；
3. 以真实 child Handles 构造页对齐 StartupBlock prefix；
4. prefix 使用 owned 只读页，payload 以 BootPackage 保留帧映射为 `U|R|A`，映入即收编为 init AddressSpace 的 owned backing；
5. 构造主线程、绑定兼容调度域并 enqueue。

init 的 StartupBlock Handle 顺序固定为：Handle[0] root JobControl，Handle[1] init ProcessControl。普通进程 Handle 数组完全来自 launcher 提供的 grants，slot 含义由具体启动协议定义，不属于通用 outer ABI。

initial ELF 与 prefix 完成后，package 前缀页回投帧池；payload backing 随 init AddressSpace 回收。内核没有 pid 特判的保留洞。

## Job/Process 构造 ABI

`shared/src/proc.rs` 与 `shared/src/call.rs` 定义 fixed-width ABI，rinlib 封装位于 `user/rinlib/src/process.rs`：

- JobControl `CREATE`：JobCreate、ProcessCreate；
- JobControl `MANAGE`：JobSeal、JobDerive；`READ`：JobQuery、JobEnumerate；
- ProcessCreate：一次事务交付 affine ProcessBuilder 与稳定 ProcessControl；
- ProcessMap/Write：只操作 Building process，建立匿名零页并回填 backing；
- ProcessStart：验证入口、栈、profile、payload 和 grants，成功消费 builder并首次发布进程；返回 `()`，ProcessControl 已由 ProcessCreate 交付。

ProcessBuilder 不可 duplicate，最后一个 builder 关闭触发 Building abandonment。ProcessMap 最终权限拒绝 W+X；ProcessWrite 不要求最终 PTE 可写。

### ProcessStart 事务

提交前：

1. 拷入并验证 descriptor、payload 与 grants；
2. 为按请求顺序承载 grant entries 的 Vec 预留容量；
3. reserve child Handle slots，以真实 Handles 构造并映射 StartupBlock；
4. 构造主线程并在目标调度类 reserve Ready 容量；
5. 在调用者 HandleTable 同一临界区验证 builder MANAGE、grants GRANT/rights 子集/去重，并 pin 全部 entries。

Job 祖先链锁内的 seal 检查与 `Building → Running` 是提交线性化点。提交后按 grant 请求顺序定点取 pinned entries、提交 child slots、消费 builder、绑定调度域并发布 Ready；该区不扫描聚合、不分配、不可失败。

提交前任一失败都撤销 caller pins、Ready reservation、StartupBlock 映射和 child reservation；builder 与 grants 保持原值，Start 可重试。调用者 Handle 槽位顺序不影响 child Handle 数组顺序。

## 用户态公共 loader

`os/elf` 是 bootstrap 与用户态共用的纯逻辑 parser。`user/frameworks/libprocess` 验证 entry、segment overlap、文件边界和页级 W^X，合并连续同权限页，分块 ProcessMap/ProcessWrite，映射固定主栈并组装 ProcessStart descriptor。它不产生 authority，调用者必须显式持 JobControl。

## init/pm 当前政策

`user/systems/init` 把 opaque payload 当作私有 ustar，建立：

```text
root
├─ init
└─ services
   ├─ pm_domain
   └─ acceptance
```

所有常规服务是 services 的直接成员。init 保留每个 ProcessControl，按 REAPABLE|CLOSED → ProcessDrain → Query 收束。pm 通过 StartupBlock grants 获得 Handle[0] mailbox owner 和 Handle[1] pm_domain JobControl；后者 rights 为 `MANAGE | READ | WAIT`，不含 CREATE。init 保留 pm_domain control 作为兜底。pm 对委托域执行枚举→派生→kill→drain→seal。

acceptance 收容一次性 IPC、FAL、Job 与竞态验证负载，结束后整域 job_kill。init 在全部服务完成后常驻管理端点，不自终止；无 runnable、无 timeout owner 时系统进入 quiescent shutdown。

## 验证

- shared host：BootPackage/StartupBlock canonical geometry、零 padding 与空 payload；
- handle_table host：Start pin 顺序、rights 回滚、reservation 与 TRANSIT/GRANT；
- libprocess host：entry、segment overlap 与页级 W^X；
- QEMU acceptance：`virt`、`virt-release`、hetero、nofd 与 `sifive_u` 均要求最小预算 Drain、竞态矩阵 10/10、服务监督、委托域终态和 quiescent 锚点；失败锚点或缺失 profile 锚点使 recipe 失败。
