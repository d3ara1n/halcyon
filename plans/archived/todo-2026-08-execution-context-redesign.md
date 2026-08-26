# 执行环境整体重构提案

> 架构决策已确认，稳定契约见 `notes/impls/execution-context.md`。本篇保留推导与边界细节；整体切换已随 a9a65cb 落地并归档。

## 1. 推导边界

本设计只从 eRhino 自身约束推出：

1. 内核是协作式微内核，内核路径短且不可抢占；S 态 trap 是致命故障，不是第二条正常执行流。
2. 内核必须能运行在无 F/D/V 的 hart；用户进程可以使用其运行 hart 具备的扩展状态。
3. 用户上下文归属线程，迁移不能依赖目标 hart 的残留状态。
4. 硬件事实、内核身份、调度策略相互独立；raw hartid 不能兼任数组下标或策略类别。
5. bootstrap 可以混沌，但正式运行环境必须从单一、显式、不可逆的边界建立。
6. 结构正确性和机械可证性优先于当前性能；优化只能在同一模型内减少工作。

RISC-V、SBI、psABI 和 Devicetree 只定义外部契约。Linux 等实现仅用于检索现实中的指令/API用法和反例，不作为架构选择理由。

## 2. Bootstrap 与正式运行环境

### 2.1 独立 bootstrap

固件只把一个 cold boot hart 交给内核；其它 hart 由 HSM 保持 STOPPED，随后显式启动。这是 eRhino 的平台准入契约。

cold boot hart 首先运行于不属于任何正式 hart 的 `BootstrapContext`：

- 独立 bootstrap stack；
- 临时页表；
- 固件传入的 raw boot hartid 与 DTB PA；
- 早期 trap vector，任何异常只停机，不进入正式 trap。

它不借用 `HartSlot(0)`、`HartLocal[0]` 或正式内核栈。bootstrap 单核完成 SBI 探测、DT 解析、内存布局、正式页表、hart registry、调度域候选和所有 `HartBootRecord` 的构造；正式 active/domain 集合只在全体 admitted hart Online 后冻结。

### 2.2 不可逆切换

正式 registry 建立后，boot hart 以自己的 raw hartid 查询其正式 slot，经非返回 `enter_hart(record)`：

1. 切换正式 satp 并完成本地地址翻译同步；
2. 建立 kernel gp、HartLocal tp、sscratch、正式 stack；
3. 归一化全部项目拥有的 CSR；
4. 安装共同 trap vector；
5. 发布本 hart Online。

secondary 的 HSM opaque 是 `HartBootRecord` 的物理地址。HSM 从 PA、satp=Bare 开始，不能直接切换到只有高半区的正式页表：永久低地址前导 `secondary_bootstrap_pa` 先以 Acquire 消费 record，写入同时覆盖当前 PA 与高半区别名的永久过渡 satp，执行 `sfence.vma` 后跳到高半区；再写正式 satp并再次 `sfence.vma`。cold boot hart 已由自身临时页表到达高半区，不重复这段 Bare 前导。两条路径只在高半区 `enter_hart_high(record)` 汇合，此后没有两套正式执行环境。

过渡页表、PA 前导和其早期 fatal vector 属于永久 hart-entry 设施，不随 cold-bootstrap 回收；它们不使用 Rust stack，也不暴露为运行时执行路径。

### 2.3 Bootstrap 退役

cold-bootstrap 专用代码、数据、stack 和临时页表放入页对齐的 `.bootstrap` 区间；secondary/HSM 入口位于永久 `.text`。boot hart 已在正式 stack/satp 上且所有 secondary 不依赖该区间后，清除最后引用并把完整 bootstrap 页交给帧池。回收动作不可能从 bootstrap stack 自身返回。

### 2.4 全局启动闸门

所有 admitted hart 的 record 与完整集合先构造；boot 以 `Starting.store(Release)` 发布每条 record 后才发出对应 `hart_start`。secondary 的 PA 前导首先 `load(Acquire)` 看见 Starting，再读取 record 和页表字段；formal entry 完成后以 `Online.store(Release)` 发布，boot 以 Acquire 等待。该协议要求 admitted harts 对普通主存保持硬件 cache coherence，这是 SMP 平台准入条件，不能由 SBI 调用代替。

每条 record 只有 `Prepared → Starting → Online`。任何同步 HSM 错误（包括 ALREADY_AVAILABLE）、成功请求后的上线超时或状态矛盾都使全局启动失败；成功的 `hart_start` 不能被软件超时取消，因此失败后也不回收 record、entry page table、HartLocal 或 stack，晚到 hart 只能看到 Failed gate 后停驻，等待系统重置。这里不做部分降级，避免不确定 active 集合渗入正式域。

Online secondary 以 Acquire 在全局 `RuntimeGate` 等待。所有 admitted hart Online 后，boot 冻结 active slots、完成调度域和初始任务装载，再以 Release 把 gate 从 Preparing 置 Ready；失败则置 Failed。只有观察到 Ready 的 hart 可以进入调度器，避免 secondary 抢先触发静默判定。

## 3. Hart 身份、能力与拓扑

### 3.1 三种正交事实

- `HartId`：DT `reg`/SBI 使用的 raw hartid；可以稀疏，只在外部边界使用。
- `HartSlot`：内核为 admitted hart 分配的稠密下标；HartLocal、stack、bitmap 均按 slot 索引。
- `HartTopology`：可选 `/cpus/cpu-map` 描述的 socket/cluster/core/thread 层级。

slot 按 admitted raw hartid 升序分配，稳定且不受拓扑描述变化影响。`HartRegistry` 提供双向 `HartId ↔ HartSlot` 映射。SBI IPI 边界把 slot 集合转换成 raw hartid；初版可逐 hart 发送，绝不把内部 slot bitmap 直接解释为 SBI hart mask。

### 3.2 只接受现代 DT

CPU 节点必须提供现代 ISA/MMU 信息；通用 `status` 缺省为 okay，显式 disabled 不准入：

- `reg`；
- `riscv,isa-base`；
- `riscv,isa-extensions`；
- `mmu-type`。

不解析已弃用的 `riscv,isa`。仓库内两个平台 DTS 同步改为现代属性；只提供旧属性的平台由 firmware/bootloader 先升级 DT。

`cpu-map` 可选；不存在时拓扑为平坦集合。拓扑只作为将来 cache affinity、SMT sibling、cluster power policy 的输入，不用于推断 capability 或分配 slot。

### 3.3 能力模型

`HartCapabilities` 是 DT 与项目平台契约共同确认的硬件事实，至少覆盖：

- 内核基线：RV64 I/M/A/C、Zicsr、Zifencei、Zicntr、Sv39；
- 用户扩展：F、D，后续可加入其它独立状态；
- MMU 上限及与系统选定模式的兼容性。

boot hart 在执行 Rust 内核前已经使用编译基线和临时 Sv39，因此这些条件对 boot hart 是固件准入前提，DT 只能做一致性核验；secondary 可在启动前依据 DT 排除。`time` CSR 必须在 S 态可读并与 DT `timebase-frequency`/SBI TIME 使用同一时间基准。

系统按标准扩展关系得出有效 FLEN：无 F 为 0、F 为 32、D 为 64、Q 为 128；它是 eligibility 事实而非“至少这么宽”的排序捷径。每种用户可见持久状态在 hart 进入用户调度域前必须满足三者之一：内核完整保存/恢复、已核验的硬件 gate 可保持 Off、或该 hart 被排除。存在 Ssstateen 时还要核验 S 态实际拥有对应控制权；仅在 DT 出现扩展名不等于内核已经能隔离它。

系统不因 cluster 相同推断能力相同，也不把能力位改名为 `HartKind`。调度域是能力与策略的派生对象。

## 4. 内核 ISA、ABI 与可选状态代码

内核 Rust target 改为 RV64IMAC/LP64；普通 Rust 和汇编不允许生成 F/D/V 指令。kernel gp 通过带 `norelax` 的规范序列建立，之后允许编译器正常使用 gp-relative addressing。

用户 FP 状态仍嵌在持久 `UserContext`；可选 ISA 代码与数据位置正交：

```rust
#[repr(C)]
struct UserContext {
    x: [u64; 32],
    sepc: u64,
    fp: FpState,
}

#[repr(C)]
struct FpState {
    f: [u64; 32],
    fcsr: u64,
}
```

`save_fp(&mut context.fp)` / `restore_fp(&context.fp)` 是独立汇编 helper，放入 `.text.ctx_fp`，局部 `.option arch,+f,+d`；只有确认当前 hart 与任务均属于 D64 profile 后才能调用。独立输出 section 只是链接后机械审计边界，不承载状态，也不改变 UserContext 所有权。链接后检查：

- kernel ELF 为 LP64 base ABI；
- `.text.ctx_fp` 之外无 FP/fcsr 指令；
- helper 外无局部 F/D enable；
- 所有 Rust 可调用边界满足 16-byte stack alignment。

## 5. 用户执行需求与调度域

### 5.1 ELF 是需求真值

loader 读取 ELF `e_flags` 和 `.riscv.attributes` 的 `Tag_RISCV_arch`，规范化为 `IsaRequirement`。`e_flags` 负责调用 ABI、RVE、TSO 等约束，不能被当成“是否含 FP 指令”的替代品。

当前明确支持两个用户 profile：

- Base64：项目定义的 RV64 base 用户环境，LP64，FS=Off；
- D64：Base64 + F/D，LP64D，FS 可用，且 eligibility 要求 hart 的有效 FLEN **恰为 64**。

F-only、Q、V、未知或本内核未建模的状态扩展、畸形/缺失的必要 attributes 均在 load 时明确拒绝，不降级为 Base；`EF_RISCV_TSO` 当前同样拒绝，直到 Ztso 进入 capability/domain 模型。实现 Q/FLEN=128 的 hart 仍可进入 Base domain，但不能运行 D64：FS 没有独立 Q gate，不能依靠 ELF 声明约束恶意指令。未来支持 Q 时新增 Fp128 状态模型，而不是让 64-bit FpState 假装完整。

当前服务默认改为 RV64IMAC/LP64 target；真正需要 FP 的程序显式用 RV64GC/LP64D target。syscall 与 shared wire ABI 只使用整数寄存器和显式线格式，不随进程浮点 ABI 分裂。

### 5.2 能力决定 eligibility，class 决定策略

ISA requirement 不是调度 class。调度域包含能力等价、策略相同的一组 hart，并拥有自己的调度类实例；线程在任意时刻只归属一个 compatible domain 的一个 class：

兼容性由显式 `compatible(requirement, capabilities)` 判定：普通 ISA 位要求集合包含，D64 还要求有效 FLEN 恰为 64，用户可见扩展状态满足前述隔离不变量。

spawn 选择一个满足需求的 domain（初版以最小充分能力、可用负载为序）；之后的迁移是显式的 dequeue→换归属→enqueue 事务。Base 任务可被放入 base 或 D-capable domain，但不会同时存在于多个队列；D64 任务绝不会进入无 D domain。IPI 只通知目标 domain 的 idle slot。

这保持调度类的单一归属和公平语义，也避免“pick 后扫描并跳过不兼容线程”。

## 6. 用户 FP 状态机

不采用首次 FP fault、per-hart owner 或延迟归还模型。其复杂度对协作式短内核没有结构收益。

- 所有线程的 `FpState` 创建时全零；不存在 `fp_valid`。
- Base64 用户出口 FS=Off，从不执行 FP helper；若二进制违反声明执行 FP，按非法指令终止进程。
- D64 切入前暂开 FS，完整恢复 FPR/fcsr，随后请求 FS=Clean。
- trap 时先读取硬件 FS；仅 Dirty 时保存 FPR/fcsr，随后内核恢复 FS=Off。
- 硬件若把 Initial/Clean 保守报告成 Dirty，只增加保存次数，不改变正确性。

第一次 D64 运行也从全零 FpState 完整恢复，因此用户绝不会观察到前一线程或 firmware 的 FPR/fcsr。D64 只在 FLEN=64 hart 上运行，FSD/FLD 因而保存完整物理 FP register state；Q-capable hart 不进入该 eligibility 集合。

## 7. 共同 trap 入口

正式环境的 stvec 恒指共同 direct-mode 入口；不在用户/内核之间切换 vector。硬件 `sstatus.SPP` 是来源的唯一真值。

正式环境中 sscratch 恒指本 hart 的 HartLocal，无论当前处于 U 或 S。入口使用 t5/t6 与 HartLocal 两个 scratch 槽建立锚：

1. `csrrw` 取得 HartLocal 并临时保存被交换的 t6；
2. 把原 t5/t6 写入 HartLocal scratch；
3. 立即恢复 sscratch=HartLocal；
4. 读取 SPP 分流。

SPP=0 才允许定位当前 UserContext、保存用户寄存器并切入内核 gp/tp/stack；SPP=1 保存 FatalFrame 后切 per-hart emergency stack。返回用户尾部即使同步异常，SPP 仍为 1，因此只会进入 fatal，绝不会把内核现场解释成 UserContext。

fatal 路径有 per-hart 递归 guard：首个 fatal 保存完整证据并进入最小诊断；再次 fatal 不覆盖首帧，直接进入无栈停驻/重置路径。

`scause` 始终按 `(is_interrupt, code)` 分发。U ecall 是 exception 8；其它用户同步异常终止当前进程。内核路径不以清掉 Interrupt bit 后的裸编号匹配。

## 8. CSR 所有权表

CSR 由边界集中设置，不保留 HSM/firmware 未定义残值。表中“pre-sret”是执行 SRET 前的编程值；SRET 之后硬件必把 SPIE 置 1，不能把两者混称用户稳态：

| 字段 | formal hart entry | 内核稳态 | pre-sret |
|---|---|---|---|
| stvec | 共同入口 Direct | 不变 | 不变 |
| sscratch | HartLocal | HartLocal | HartLocal |
| sie | 精确 SSIE\|STIE | 不变 | 不变 |
| sip.SSIP | 清零 | trap/IPI 路径清 | 不变 |
| timer/STIP | SBI TIME 先卸载 | 调度器拥有 | 按量子/期限编程 |
| sstatus.SIE | 0 | 0 | 0（U 态忽略） |
| sstatus.SPIE | 0 | 0 | 0；SRET 后硬件置 1 |
| sstatus.SPP | 无语义依赖 | trap 来源只读 | 明确清零 |
| UXL | 写 64 并 readback，否则拒绝 hart | 64 | 64 |
| UBE | 写 0 并 readback，否则拒绝 hart | 0 | 0 |
| FS | Off | Off | Base Off；D64 Clean |
| VS | Off | Off | Off |
| SUM/MXR | 0 | 0，user-copy 临时 guard | 0 |
| scounteren | 0 | 0 | 0 |
| senvcfg | 仅清项目拥有字段并核验，保留 WPRI | owned fields=0 | owned fields=0 |
| sstateen* | S 态实际可访问时仅清拥有字段并核验 | owned fields=0 | owned fields=0 |
| SDT | 锚与早期 vector 建立后清零 | 正常路径 0；trap 硬件窗口可置 1 | 清零 |

未 advertised 或上层未授权的可选 CSR 不读取；实际可访问后其所有权和值必须进入本表。若实现 Ssdbltrp，最小入口先在 SDT 保持置位时建立 HartLocal 锚并保存首要 CSR。正常用户 trap 保存完成后清 SDT再进入 Rust；fatal 先 test-and-set 递归 guard并保存完整首帧，随后清 SDT进入诊断。诊断再次 trap 时共同入口看到 guard 后不覆盖首帧，直接停驻；若首帧尚未保存就再次异常，则由硬件升级到 M 态，这是不可伪造的软件证据边界。

每次用户出口在最后一个可能产生 LR/SC reservation 的内核操作之后，对 per-hart 专用清除槽执行 dummy SC，明确取消内核或前一用户遗留 reservation；reservation 不属于可迁移 UserContext。

## 9. 用户内存与指令发布

内核稳态 SUM=0。用户指针只能在地址空间验证且持有所需对象锁后，由 RAII user-copy guard 临时开启 SUM；guard 退出恢复原状态。共同 trap 入口无条件回到 SUM=0。

可执行映射发布时满足 W^X：loader 写入尚不可执行、尚不可运行的地址空间，完成后执行 writer data fence，再以 Release 发布代码代次和线程。调度器 Acquire 取得线程；每 hart 记录自己已观察的地址空间代次，在新代次首次 sret 前执行本地 `fence.i`，完成后才记录已观察。线程迁移不会假设其它 hart 的 I-cache 已同步。

已发布的 executable page 不允许原地写。未来动态装载必须先让该地址空间所有线程 quiescent、撤销旧执行许可，写入并递增代次后再重新发布；不能只在仍运行时递增计数。由此 writer fence、queue Release/Acquire、目标 `fence.i` 组成完整跨 hart 指令发布链。

## 10. 布局的单一真值

持久用户状态命名为 `UserContext`；fatal 证据为 `FatalFrame`；调度调用现场为 `SchedulerFrame`，三者不复用。

当前 SchedulerFrame 保存 ra+s0..s11，大小为 112 bytes，保持 psABI 16-byte 对齐。Rust `offset_of!` 是 UserContext、FpState、HartLocal、FatalFrame、SchedulerFrame 的唯一布局来源，经 `global_asm!` const operands 注入汇编；汇编不维护第二套数字。

kernel gp 在 bootstrap/formal entry/用户 trap 后均由同一规范序列建立；用户 gp 只存在 UserContext，并在 sret 尾部恢复。

## 11. 落地顺序

稳定结论已写入 `notes/impls/execution-context.md`。实现先准备不会被执行的契约数据面，最后以一个原子里程碑激活正式环境：

1. 固定现代 DT schemas；补 DT string-list/cpu-map 与 ELF attributes 的 host parser/tests；更新平台 DTS。
2. 建立 IMAC/LP64 构建、独立 FP helper section、链接后 ISA/ABI 审计和 Rust const offsets。
3. 准备但不激活 BootstrapContext、HartRegistry/HartSlot、HartBootRecord、RuntimeGate、per-domain scheduler、UserContext/FpState 与共同 trap/CSR helper。
4. 原子激活完整执行边界：PA 过渡入口、高半区 formal entry、共同 trap、CSR 表、112-byte frame、scause、SUM guard、reservation clear、I-cache 发布和 capability-aware domain 同时替换旧路径；不存在“新 formal entry 配旧 vector/CSR”的中间运行模型。
5. 切换默认用户 LP64 target，保留显式 D64 构建入口；用合成 DT/ELF/topology 做 host contract tests。
6. 对最终 diff 做独立规范 review，逐项关闭 `plans/review-2026-08-audit-01-sbi.md`、`plans/review-2026-08-audit-02-trap-context.md` findings，并按实现反馈同步 `notes/`。

实施中可以按依赖保持工作区暂时不可运行，但不提交或保留过渡兼容模型。

## 12. 外部契约

- RISC-V Privileged/ISA：`references/normative/riscv-isa-v20250508/`
- RISC-V psABI/ELF：`references/normative/riscv-psabi-v1.0/`
- SBI：`references/normative/riscv-sbi-v3.0/`
- Devicetree：`references/normative/devicetree-v0.4/`
- CPU topology schema：`references/normative/devicetree-schema-v2026.06/`
- 现代 RISC-V CPU/ISA bindings：`references/normative/riscv-dt-bindings-linux-818bebeb/`
