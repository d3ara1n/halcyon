# todo：BootPackage / 用户态 launcher 统一 review

状态：**机会型任务（有空就做），不阻塞后续计划**。2026-08-26 机制层审查已完成（[archived/review-2026-08-26-bootstrap-launcher-mechanism.md](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)）：机制方向成立，launcher 基座可承接 process-lifecycle 工作；本计划的十切片代码审查保留，作为后续接入时发现问题的路线图，但不再是进入 `archived/todo-2026-08-26-process-lifecycle.md` 的前置条件。本文件冻结提交边界、设计契约、审查切片与核证入口；切片清单在执行代码审查时直接充当检查表。

## 提交范围

| 性质 | 提交 |
|---|---|
| 审查基线（不含） | `29c6519fc6c12222c72d3daf469829e245fc3827` `feat(abi): 重构能力运输与启动基座` |
| BootPackage / launcher 实现 | `1bc83ac4596d548f47798e053a8104a14a429d97` `feat(boot): 建立用户态启动与进程构造链` |

统一 review 的代码范围固定为 `29c6519..1bc83ac`。审查必须以提交内容和当前实现为证据，不得把实施期讨论、QEMU 跑通或既有 review 当作本次已经通过的证明。

方向契约：

- `notes/ideas/bootstrap.md`
- `notes/ideas/{object,task,service}.md`

实现现状：

- `notes/impls/{startup,task,mm,ipc}.md`
- `plans/archived/todo-2026-08-26-bootstrap-launcher.md`
- `plans/ref-2026-08-bootstrap-package-userboot-loader.md`

## 原任务边界

本提交完成以下纵向切换：

- 固定 BootPackage v1 envelope 取代内核可见的 tar/initfs 协议；
- 内核只装载唯一 init，把 opaque payload 作为只读 borrowed backing 交付；
- root JobControl 作为 init 的显式根 authority；
- JobCreate、ProcessCreate、Building-only ProcessMap/ProcessWrite、事务化 ProcessStart；
- affine ProcessBuilder 成功 Start 后消费并返回 ProcessControl；
- shared/kernel/rinlib 同步 ABI；
- 公共用户态 `libprocess` 负责 ELF 页规划、回填、栈与启动描述；
- init 以临时测试政策启动 pm/fs/driver，内核删除服务名称与 mailbox 拓扑策略；
- StartupBlock 支持 Handle 后零 padding 与页对齐外部 payload；
- ProcessStart 提交前预构造 Thread、进程表 reservation 和 ready reservation；
- 页表批量 unmap 修正跨子表基址，失败路径按 backing 生命周期回滚；
- 新可执行页在每次新 dispatch 前执行本 hart `fence.i`。

## 明确不在本次 review 内冻结的方向

以下只能核对“当前边界是否诚实且不泄漏 authority”，不得在本 review 中顺手设计或实现：

- initfs manifest、archive、路径与服务拓扑协议；
- ProcessKill、JobKill、exit-status 查询和多线程终止屏障；
- ThreadSpawn、active-hart teardown 与远端 TLB shootdown；
- MemoryObject、共享代码页、COW 与动态链接器；
- D64 调度域 eligibility；当前公开 Start 必须明确拒绝；
- 内核整体 W^X 重构。`os/platforms/linker.ld` 在本提交前后相同，kernel ELF RWE LOAD 与 `KERNEL_DIRECT` RWX 是基线债务；不得误归因于本提交，也不得用隐藏 linker warning 代替后续机制修复。

这些方向分别由后续计划承接；若审查发现本提交已经提前冻结了错误接口，则仍属本次 finding。

## 外部契约取证

凡结论由规范决定，必须从 `references/INDEX.md` 定位固定文本并在 review 报告中引用具体小节：

- Devicetree `/chosen`、`reg`、address/size cells：`normative/devicetree-v0.4/source/`；
- ELF program header、RISC-V attributes 与 psABI 栈/入口：`normative/riscv-psabi-v1.0/`；
- `FENCE.I` 与跨 hart 指令流同步：`normative/riscv-isa-v20250508/src/zifencei.adoc`；
- RVWMO release/acquire 与发布顺序：同目录 `rvwmo.adoc`；
- `SFENCE.VMA`、satp 与 hart 间地址翻译同步：`supervisor.adoc`、`priv-insns.adoc`。

外部实现只用于比较机制，不替代本项目不变量；`pre-ai` 只可作为缺陷样本。

## Review 切片

### 1. BootPackage envelope 与构建链

核查 `shared/src/boot.rs`、`tools/make-boot-package.py`、`Justfile` 与两平台 DTS：

- header 定宽、little-endian、magic/version/header_len/flags/reserved；
- 所有 offset/length/round-up 均 checked，canonical offset 唯一；
- initial ELF 非空，payload 可为空，区间不重叠；
- payload offset、total_len 与物理 base 页对齐；
- header、段间与尾部 padding 必须为零；
- validator 只在 DT capacity 内取 slice，实际 `total_len` 不得越界；
- packer 临时文件、flush/replace 与失败清理是否保持原子产出；
- ustar 制作不混入 AppleDouble，initial ELF 不重复进入 payload；
- virt DTS memory、QEMU `-m`、BootPackage 地址/capacity 三者一致；
- sifive_u 128MiB 边界与 0x86000000 窗口完整落在 memory node 内。

重点反例：整数溢出、截断 header、非零 padding、payload_off 回退、total_len 小于实际使用、capacity 跨 memory 末端、BootPackage 与内核/DTB/栈物理区间重叠。

### 2. 启动物理 reservation 与 borrowed backing

核查 `os/kernel/src/{board,boot,frame,rt}.rs`、`task/proc.rs::map_bootstrap_block`：

- frame pool 注册前先 inspect envelope，再把 capacity 收窄为实际 total_len；
- reservation 剔除与 bootstrap `free_range` 不重叠、不重复回投；
- initial ELF 完整复制、StartupBlock prefix 完整构造后才回投 `[package base, payload_pa)`；
- payload_pa 页对齐，prefix reclaim 绝不覆盖首个 borrowed payload 页；
- payload 最后一页的可见尾部只能是 packer 零 padding；
- payload 为空时是否可安全回投整个 package；
- init AddressSpace teardown 只清 borrowed PTE，不把 payload backing 当 owned frame 释放；
- init 退出后保留 payload reservation 是否与后续 frame count、重复分配和系统关机一致；
- boot failure 的 panic 边界只接受平台/镜像错误，任何普通用户进程输入不得进入该边界。

必须画出 package 物理页所有权从 QEMU loader → boot reservation → prefix reclaim / payload borrow 的时间线，并逐页核对边界。

### 3. StartupBlock outer 与真实 child Handles

核查 `shared/src/startup.rs`、rinlib env parser 与两类 builder：

- `handles_end <= payload_off`、block_len 等式、reserved 和零间隙；
- 非对齐读不得形成未对齐引用或 UB；
-普通 builder 紧凑布局与 bootstrap padded prefix 共用同一 validator；
- child Handle 值来自 reservation，不依赖 slot/generation 推导；
- payload 为空、Handle 为空、最大长度与 checked arithmetic；
- `try_reserve` 覆盖 prefix 和 payload 两次增长，构造失败不得 panic；
- outer 不解释 JobControl、路径、参数或服务角色。

### 4. Job、ProcessBuilder 与 capability 边界

核查 `task/{job,process,object,handle}.rs` 与 `shared/src/{object,proc}.rs`：

- PID、名称、parent_pid、创建关系均不授权；
- ProcessCreate 只接受持 CREATE 的 JobControl；
- Job parent 引用方向不形成 Job/Handle/Process 永久环；
- ProcessBuilder affine：无 DUPLICATE，close_handle/close_transit 都回收 Building process；
- TRANSIT/GRANT Builder 后仍只有单一构造 authority；
- ProcessControl rights 不能放大，close 只消散 authority、不终止目标；
- Process 退出后 Control 只保留轻量 CLOSED/exit-code 壳，不保留 AddressSpace、HandleTable 或 Thread；
- D64 profile 在 eligibility 未接线时稳定返回 NotSupported；
- 当前单线程前提必须显式，不能把未来同进程并发 close/start 的竞态伪装成已解决。

### 5. ProcessMap / ProcessWrite 地址空间构造

核查 `task/proc.rs` 与 `task/process.rs`：

- VA/length 页对齐、用户半区、固定栈窗口、StartupBlock/heap 边界；
- anonymous page 初始清零，权限只来自 known READ/WRITE/EXECUTE 位；
- W-only 与 W+X 拒绝，PTE 使用最终权限；
- ProcessWrite 只能操作 Building process，target 全区间先验证后写；
- source 用户区间有长度上限、完整读校验与 checked arithmetic；
- 最终不可写/可执行页只能经 Building backing 回填，Running 后无同类入口；
- `AddressSpace.frames` 在安装任何 PTE 前预留元数据容量；
- 每个批量映射失败时先逆序 unmap，再 Drop backing；
- 页表表帧耗尽映射为 OutOfMemory，不伪装成 IllegalArgument；
- external/owned/borrowed 三类 backing 不混淆。

重点做 allocator/page-table failure injection，证明不存在“PTE 仍指向已归还帧”的 UAF。

### 6. ProcessStart 原子事务

逐行核查 `task/process.rs::start` 与 HandleTable reservation/extract：

- descriptor、payload、grants、output 的 fixed-width、reserved、上限与对齐；
- builder target 不得同时作为 grant；重复 source Handle、未知 rights、rights 放大必须原子拒绝；
- child reservation 先产生实际 Handles，StartupBlock 使用同一组值；
- Thread Arc、ProcessControl、进程表 marker、ready marker、caller output slot 全在 GRANT commit 前准备；
- 枚举 commit 之后的每一条语句，证明无堆/帧/表分配和可恢复错误；
- 任一 commit 前失败都撤销 StartupBlock、child/output/table/ready reservations；
- `extract_grants` 失败时 caller HandleTable 完全不变；
- 输出 Handle 只在 child grants、builder 消费和 caller control slot 均不可失败后写回；
- builder close callback 在 consume 后不会误回收 Running process；
- 进程表可见、ProcessControl 安装、ready 发布的顺序不暴露半发布对象；
- 当前 check_range→write 的单线程前提与 `KNOWN_ISSUES.md` 的多线程写回窗口一致。

Review 报告应附一张事务阶段表：资源、owner、reservation token、失败动作和提交线性化点。

### 7. 进程表、ready marker 与 SMP

核查 `task/table.rs` 与 `sched.rs`：

- reservation token/PID 回绕与唯一性；
- marker 对普通 lookup/remove 不可见；
- rollback/commit 恰好一次，marker 不泄漏；
- ready `pick` 跳过 marker时保持普通线程之间的 FIFO；
- `has_ready` 忽略 marker 只影响调度可见性，不把 reservation 当作可运行线程；
- commit_ready 替换 marker 后才发 IPI，idle 双重检查仍闭合；
- 多 hart 同时 pick/commit/rollback 的锁序无环；
- enqueue/requeue 的既有分配行为与 ProcessStart 的“提交区零分配”边界不要混为一谈。

至少构造 marker 位于队首/队中/多个 marker、并行 pick 与 idle 唤醒的模型测试或等价证明。

### 8. ELF loader、入口和页级 W^X

核查 `os/elf`、`user/frameworks/libprocess`、rinlib linker 与 `tools/audit-user-elf.py`：

- 仅接受预期 ELF class/endian/type/machine 与静态 ET_EXEC；
- PT_LOAD 按 VA 排序，filesz <= memsz，文件区间完整；
- vaddr/offset 页内同余、segment byte range 不重叠；
- entry 必须落在实际 executable segment 字节区间，而非仅落在 X 页；
- 相邻 segment 的页级权限并集不得产生 W+X；
- BSS 依赖 anonymous zero page，回填不能越过 filesz；
- image 不侵入固定主栈/StartupBlock 区；sp 满足 psABI 16-byte 对齐且下方可写；
- audit 脚本与 libprocess 采用同一页级规则，链接脚本实际产物四个用户 ELF 全过；
- PT_INTERP、ET_DYN、relocation 未实现时必须明确拒绝或不产出，不能半支持。

### 9. 指令流与地址翻译同步

核查 ProcessWrite → ProcessStart → enqueue → 首次/迁移 dispatch 的 happens-before：

- caller 对 backing 的数据写入经何种锁/原子发布到目标 hart；
- ready queue Release/Acquire 是否先于目标 hart `fence.i`；
- 每个可能首次执行或迁移到的新 hart 都执行本地 `fence.i`；
- Running process 不再存在 Building write，因此 Resume 热路径省略 fence 的前提成立；
- PTE 发布与 `SFENCE.VMA` 覆盖首次 satp 使用、当前地址空间新增映射和外部 lease；
- 不得把本地 `fence.i` 错写成跨 hart remote fence 的替代品；结论必须引用 Zifencei 规范。

### 10. ABI 纵向一致性与错误边界

对照 shared/kernel/rinlib 三侧逐调用核查：

- SystemCall 编号、a0–a5 参数位置、返回值和错误码；
- `repr(C, align(8))`、定宽字段、size/align const assert；
- Handle/Rights 未知位、reserved 与地址宽度；
- host 测试 cfg 不改变 erhino target 的 panic/allocator/ecall runtime；
- 用户可触发的非法地址、OOM、冲突、坏 ELF、坏 grant 只返回错误或使用户态 launcher 失败，不 panic 内核；
- boot-only malformed package/initial ELF 允许确定性 boot failure，但诊断必须正式英文且可定位。

## 故障注入与负向验证

现有成功路径回归不替代下列验证；review 可补测试，也可把缺口列为独立 fix todo：

- BootPackage 每个 header 字段的截断、溢出、非 canonical offset 与非零 padding；
- payload 为空、恰好一页、跨页且尾页有 padding；
- package capacity 与 DT memory/kernel reservation 冲突；
- page-table 表帧在 map、mega split、跨子表 unmap 各阶段耗尽；
- `AddressSpace.frames`、Thread Arc、ProcessControl、进程表 marker、ready marker、Handle reservation 各阶段 OOM；
- ProcessStart grant 中重复 Handle、builder alias、rights 放大、失效 generation、输出不可写；
- entry 落 X 页 padding、segment byte overlap、同页 W/X 并集、filesz 越界；
- ProcessBuilder close、Start 失败与 target process frame/Handle 守恒；
- ready queue 含 reservation marker 时多 hart pick、idle 与 IPI 唤醒闭包；
- init fault/退出后 payload backing 不被误回收，普通 child 退出后 owned frames 全部回收。

## 已有回归证据

实现提交形成时已通过：

- shared 7 项 host 测试；
- page_table 17 项 host 测试，含跨子表 unmap 与 split OOM；
- elf 13 项、HandleTable 12 项、tar 4 项 host 测试；
- libprocess 4 项入口/overlap/页级 W^X host 测试；
- `just check`、`just build_user`、用户 ELF audit 与 kernel ELF audit；
- virt 四核：init 启动 driver/fs/pm，IPC/FAL/Runnel 验收完成，全员回收；当前回归线在全部锚点后显式提交 reset；
- sifive_u 四个 Application hart：同一负载完成，显式 reset 返回 `NotSupported` 后由 wrapper 收割；
- BootPackage prefix 回投与终态帧数仍用于证明 DTS capacity 未整段浪费，不作为停机意图判据。

这些只定义回归基线，不证明锁序、失败原子性、规范符合性或不可达分支已经完成 review。

## Review 产出格式

完成时新建只读档案 `plans/review-<日期>-bootstrap-launcher.md`，至少包含：

1. 审查提交范围与实际复现环境；
2. 上述十个切片逐项结论；
3. 每个 finding 的严重度、可达前提、直接证据、受影响不变量和建议修复方向；
4. 已运行命令及关键输出，不以“构建通过”代替语义证明；
5. 哪些结论进入 `notes/impls/`，哪些债务进入 `KNOWN_ISSUES.md` 或新 todo；
6. 对 `plans/archived/todo-2026-08-26-process-lifecycle.md` 的前置约束。

发现缺陷后不得在 review 档案内混写实施过程：修复另立 todo/提交；方向性结论进入 notes，暂时可达债务进入 KNOWN_ISSUES。

## 完成条件

- 十个切片均有基于代码、测试或固定规范的明确结论；
- 所有阻塞/高风险 finding 已有 owner 和修复计划；
- launcher 基座能否作为进程生命周期工作的前提已明确判定；
- 结论已由只读 `review-*` 档案承接；
- 本 todo 随后移入 `plans/archived/`，不再充当未完成任务入口。
