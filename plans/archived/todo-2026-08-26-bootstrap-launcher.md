# BootPackage 与用户态 launcher 实施计划

## 目标

把启动责任收敛为：内核只启动 init，并把 opaque initfs 作为 StartupBlock payload 只读映射；init 解析配置与其他 ELF，通过 Job/Process capability 构造并启动全部服务。完成后内核不依赖 tar，不识别任何服务二进制名称。

方向契约见 `notes/ideas/bootstrap.md`；外部证据见 `plans/ref-2026-08-bootstrap-package-userboot-loader.md`。

> 状态：已完成。收口边界经确认限定为 BootPackage、唯一 init bootstrap、Building process 与用户态 launcher。ProcessKill/JobKill、exit-status 查询及多线程终止屏障转入 `plans/todo-2026-08-26-process-lifecycle.md`；initfs manifest/archive 另案设计。

## 已确认

- BootPackage 使用独立固定 envelope，不以“tar 中某个特殊路径”作为内核 ABI。
- BootPackage 包含唯一 initial ELF 与 page-aligned opaque payload。
- payload 只给 init，直接只读映射，不引入 Archive/MemoryObject Handle。
- initfs 内部格式只属于 init；初版可以继续使用 ustar。
- init 进入用户态后没有 PID/名称触发的 ambient 权限；完整 root authority 来自显式 capabilities。
- init 是临时授权根：可以 spawn/kill/派生，但按配置把长期管理权交给服务后退出；退出不级联终止、不重启、不重铸未交付 authority。
- 内核只为 init 解析 ELF，后续 ELF 由公共用户态 loader 解析。
- 不保留“测试模式下内核遍历 bin/”的兼容路径；集成负载迁到 init 启动。
- initfs 内部归档/配置协议延期；本阶段只要求 opaque payload 可访问，并允许集成 init 采用最小测试政策启动现有负载。

## 已采用的实施选择

### A. Building 地址空间的首版数据来源

**A1（已采用）：匿名页 + Building-only bounded write**

- `ProcessMap` 为目标 Building process 分配零页并以最终权限映射；
- `ProcessWrite` 从 launcher 用户缓冲向已映目标页复制，单次严格限长；
- init 解析 ELF 并循环完成 program segments；
- 后续增加 `ProcessMapMemory` 时不替换生命周期或 Start 事务。

优点：不把 boot payload 强行对象化，机制小且足以完成用户态 ELF loader。缺点：可执行页初次装载存在一次 copy，尚无共享代码页。

**A2（未采用）：本阶段同时引入通用 immutable MemoryObject**

优点：后续 ELF/文件缓存可共享 backing。缺点：必须同时设计 map lease、跨进程映射、COW/写权限与外部物理帧回收，显著扩大核心面。

### B. Process 构造权生命周期

**B1（已采用）：affine ProcessBuilder，Start 成功后消费并返回 ProcessControl**

Building-only 操作在 role 上不可带入 Running；关闭 builder 自动回收未发布进程。ProcessControl 以 rights 区分管理和观察。

**B2（未采用）：同一个 ProcessController 跨越 Building/Running**

对象数较少，但每个操作都依赖 state 拒绝，rights 无法表达“构造权已经永久消失”，更易误用。

## BootPackage v1

### Envelope

固定 64 字节、little-endian：

```text
magic          u64   "ERHBOOT\0"
version        u16   1
header_len     u16   64
flags          u32   v1 必须为 0
total_len      u64   含尾部零填充，页对齐
init_off       u64
init_len       u64
payload_off    u64   页对齐
payload_len    u64
reserved       u64   0
```

不变量：

- Devicetree `/chosen/boot-package/reg` 给最大物理装载窗口；base 必须页对齐；
- `header_len <= init_off`，initial ELF 非空；payload 可为空且与 ELF 不重叠；
- `payload_off % PAGE_SIZE == 0`；
- `payload_off + payload_len <= total_len <= DTB capacity`，全程 checked arithmetic；
- `[payload_off + payload_len, total_len)` 必须由 packer 写零；
- kernel 在 frame pool 注册前用 header 将最大窗口收窄为实际 `total_len`。

DTS `compatible` 改为 `"erhino,boot-package-v1"`。内核模块和日志统一使用 BootPackage；initfs 一词只指 opaque payload。

### 构建

新增项目内非交互 packer：输入 init ELF 与 payload archive，写固定 envelope、对齐 padding 和内容。`Justfile`：

1. 构建用户 binaries；
2. 把除 initial ELF 外的测试服务文件打成确定序 opaque payload（首版沿用 ustar）；
3. pack 为 `artifacts/boot-package.bin`；
4. QEMU loader 放到平台声明地址。

packer 错误输出使用正式英文。所有长度来自实际文件，禁止把 DTS capacity 当数据长度。

## StartupBlock 扩展

StartupBlock v2 header 布局不变，但几何从“payload 紧跟 Handles”推广为：

```text
handles_end <= payload_off
block_len = payload_off + payload_len
```

间隙必须全零。普通 `build_startup_block` 继续紧凑构造；bootstrap builder 将 prefix 放入自有只读页，并把 BootPackage payload 原页从下一页边界映射成同一连续用户 VA 区间。

新增 AddressSpace 映射计划，明确区分：

- owned pages：地址空间销毁时归还帧池；
- bootstrap borrowed pages：只清 PTE，payload 物理页由 BootPackage reservation 持有到系统结束；已复制的 header/initial ELF prefix 在发布 init 前回投帧池；
- object-leased pages：Tunnel 等对象关闭时解除，现状不与 bootstrap 混用。

bootstrap payload PTE 固定 `U|R|A`，无 W/X/D。最后一页可见 padding 必须由 packer 置零。

## Job 与 Process 对象

### Job

本阶段建立最小 Job 层级与 root Job：

- root Job 由内核创建，以完整 JobControl 作为 init StartupBlock `Handles[0]`；
- `JobCreate` 从已有 JobControl 派生子 Job；
- `ProcessCreate` 必须持 JobControl 的创建权；
- init 可行使完整能力，但按启动政策把长期 Job/Process controls 转交给 pm 等服务后退出；
- Job 先记录 parent、成员与计数接口，首版预算为 unlimited；预算算法另案，但 API 不绕过 Job。

JobControl 可按 rights duplicate、TRANSIT、GRANT。关闭管理 Handle 不隐式杀进程；终止域使用显式 JobKill，避免 capability 丢失等同政策动作。init 退出只让尚未交付的 controls 消散，已交付能力和进程不受影响。

### Process roles

- `ProcessBuilder`：affine，允许 MAP/WRITE/MANAGE、TRANSIT/GRANT，不允许 DUPLICATE；只接受 Building 操作；
- `ProcessControl`：Running/Dead 的管理与观察引用，MANAGE 可 kill，WAIT/READ 可观察终态，可按授权 duplicate/TRANSIT/GRANT。

Process 对象嵌入可等待终态。Running 资源与 Dead 观察壳分离：退出时立即 drain Handles、销毁 AddressSpace，随后发布 exit status/CLOSED；ProcessControl Arc 只保留轻量终态。

Building process 不进入调度队列。全局进程表可只在 Start 提交时插入；builder 是其强生命周期所有者，Job 成员关系使用不会形成 `Process → HandleTable → Process` 永久环的引用方式。

## 构造 ABI

所有请求使用 fixed-width descriptor，reserved 置零并校验；调用号只提供机制，不携带路径。

### `JobCreate`

输入 parent JobControl 与首版配置（reserved/unlimited），输出 child JobControl。

### `ProcessCreate`

输入 JobControl，分配不复用 PID，创建空 AddressSpace/HandleTable 与 Building state，输出 ProcessBuilder。`parent_pid` 为调用进程 PID，仅供诊断。

### `ProcessMap`

输入 builder、target VA、页对齐长度、R/W/X flags。首版只创建 anonymous zero pages：

- 仅 Building；
- 每次最多固定页数，launcher 循环；
- 禁止 W+X、用户半区越界、重叠、StartupBlock/栈窗口冲突；
- PTE 直接使用最终权限。

### `ProcessWrite`

输入 builder、target VA、caller source VA、长度：

- 仅 Building；
- 单次不超过 `MAX_USER_ACCESS`；
- target 必须已映射，写入通过物理直映射完成，不要求目标最终 PTE 有 W；
- source 先完成调用者地址空间读校验；
- 失败不写入未校验页，允许 launcher 按块重试。

### `ProcessStart`

descriptor 至少包含：entry、stack pointer、execution profile、opaque payload ptr/len、grant array ptr/count、ProcessControl 输出地址。普通 payload 与 grants 都有固定上限。

提交前验证：

- builder 仍为 Building；
- entry 位于 X 映射；
- stack pointer 满足 psABI 16-byte 对齐且下方位于可写映射；
- execution profile 是已知集合且至少存在一个兼容调度域；
- payload、grants、输出缓冲在调用者空间完整可读写；
- builder target 不得同时出现在 grant array。

事务顺序：

1. 拷入 descriptor、payload 与 grants；
2. 为 child HandleTable reserve，取得实际 child Handles；
3. 构造并映射 StartupBlock；
4. 为调用者 ProcessControl 输出 reserve；
5. 重新验证并原子 `extract_grants`；
6. commit child Handles，状态 Building→Running，创建主线程并插入进程表；
7. 消费 ProcessBuilder，安装/写出 ProcessControl；
8. 首次 enqueue runnable。

任何 commit 前失败都撤销 child reservation/StartupBlock/output reservation，调用者 grant Handles 与 builder 保持原值。成功后无可恢复失败步骤。

### ProcessControl 当前边界

本阶段发布 ProcessControl 的 CLOSED 等待终态；关闭 control 不终止进程。MANAGE kill、固定宽状态查询、JobKill 与 Ready/Running/Waiting 的统一终止收束不在 launcher 事务内实现，转入 `plans/todo-2026-08-26-process-lifecycle.md`。

## 用户态 launcher

新增用户态层次：

- initfs source：首版校验 ustar 并提供按路径读取；正式配置/归档协议延期；
- `libelf`：kernel bootstrap 与用户态 launcher 共用的纯逻辑 ELF parser；
- `libprocess`：在用户态生成页粒度权限并集、驱动 ProcessBuilder，并组装参数、namespace、grants 与 ProcessStart；
- `ld-erhino`：为未来 `PT_INTERP`、重定位和 loader service 保留独立层，init/pm 不亲自实现动态链接；本阶段静态服务无需落地；
- init integration policy：以最小测试配置启动现有服务并组装 grants。

init、pm 及未来获 Job capability 的 launcher 共用 libelf/libprocess；普通服务不持 spawn authority 时通过 pm 协议请求创建，不复制 loader 实现。

当前集成负载迁移：

- init 创建 pm Mailbox，保留 sender，把 owner 作为 pm grant index 0；
- init 启动 pm 后继续现有 IPC/Tunnel/Runnel 验证；
- fs 与 driver 由 manifest 启动；
- 内核日志只出现 initial process 的装载，不出现任何服务路径判断；
- initfs 内容语义延期；现有 fs 自测继续验证 FAL 流，不把 archive 路由纳入本阶段。

## 实施分段

### 1. 固化共享格式与纯逻辑测试

- [x] `shared` 增加 BootPackage header/validator；
- [x] StartupBlock validator 接受零填充 offset 并验证 padding；
- [x] validator 覆盖 corruption/overflow/alignment/zero padding，packer 原子产出 canonical package；
- [x] DTS/Justfile 产出 BootPackage。

### 2. 建立 Job/Process 构造对象

- [x] ObjectKind/HandleRole/rights/终态；
- [x] root Job 与 ProcessBuilder/Control；
- [x] AddressSpace map/write/startup rollback primitives；
- [x] shared syscall descriptors、kernel handlers、rinlib wrappers；
- [x] Building close、Start failure与 GRANT 原子事务。

### 3. 实现用户态 loader/initfs

- [x] kernel 依赖面不含 ustar；init 仅以最小测试 walker 消费 opaque payload；
- [x] libelf/libprocess 与 process builder wrapper；
- [x] init 解析 ELF、映射/回填、直接 grant、启动 pm/fs/driver；
- [x] 保持现有集成验证语义。

initfs checksum、duplicate、manifest 与正式 archive 协议不属于本阶段。

### 4. 切换唯一 init bootstrap

- [x] kernel 只读取 BootPackage initial ELF；
- [x] root Job 安装到 init `Handles[0]`；
- [x] payload 零拷贝只读组成 StartupBlock；
- [x] 删除内核 tar/service policy 和 kernel tar dependency；
- [x] 删除旧 `compatible = "tar"`、`initfs.tar` 内核命名与所有服务名特判。

不保留内核“测试加载全部 bin”路径。切换与用户态 launcher 在同一工作树完成，任何中间不可运行状态不作为完成点。

### 5. 验证与文档

- [x] shared/elf/tar/handle_table/page_table/libprocess host tests；
- [x] `just check`、`just build_user`、BootPackage 与用户 ELF 格式审计；
- [x] virt：init 启动其余三类负载、旧验收线全过、全员回收；
- [x] sifive_u：规定运行超时内关键日志完整、到达静默判定；
- [x] malformed BootPackage/StartupBlock 由纯逻辑 validator 确定性拒绝；invalid child ELF 在用户态 loader 返回错误；
- [x] frame count 验证 DTS capacity 未被整段浪费；
- [x] 审查并修复锁序、rollback、物理 backing 生命周期、W^X、跨 hart `fence.i`、提交后 OOM；
- [x] 更新 `notes/impls/{startup,task,mm,ipc}.md`、`notes/README.md` 与 `plans/COMPASS.md`。

## 完成标准

- 内核二进制依赖图不含 tar；
- 内核没有 `bin/`、`srv_*`、`drv_*` 或服务间 mailbox 策略；
- 仅 initial ELF 由内核解析；其他 ELF 的 program-header 处理可在用户态代码定位；
- initfs 通过 StartupBlock payload 只读访问，无 Archive/MemoryObject Handle；
- 后续进程只能经 Job/Process capabilities 构造并直接 GRANT 启动资源；
- 任何用户可触发的构造失败都回滚或保持 Building，不 panic kernel；
- virt/sifive_u 对照负载达到现有验收线。
