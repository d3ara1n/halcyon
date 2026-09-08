# 批次 D-2：BootPackage / 用户态 launcher 代码 Review

## 2026-09-08 提交后复核（D-2，通过）

对象 `9ee2791d3e18fdb7857fe41c74bacc7bb0c7c774`；OliveWillow 独立只读复核。

| Finding | 结论与证据（行号对应该提交） |
|---|---|
| F-01 / payload 无 owner | 闭合。`os/kernel/src/boot.rs:27` 拆分 BootHeldExtent；`task/proc.rs:4941` 起先 fund 再借用映射、发布前安装 BootFundedExtent，失败由完整 owner 承接。 |
| F-02 / holes 与 package 重叠 | 闭合。`os/kernel/src/frame.rs:125`、`:323` page-cover、排序归一化并拒绝 package 与 permanent/kernel/SBI、DTB/bootstrap 重叠；memory_supply 统一 clip/normalize/subtract。 |
| F-03 / sifive_u 内存参数 | 闭合。`Justfile:39` 按模型固定 virt=1024M、sifive_u=128M，与 DT 一致。 |
| F-04 / Control 与 Job 记账 | 闭合。`task/process.rs:398` 起创建 core/Builder/Control 并以同一 Handle reservation 输出，Job member 提交与生命周期接通。 |
| F-05 / entry 与 BSS | 闭合。`os/elf/src/lib.rs:349` 要求 entry 落 executable file bytes；Bootstrap/launcher 消费同一 validated image；`task/proc.rs:4286` Attach 再校验 entry U/X 和 stack U/W。 |
| F-06 / 未实现 headers | 闭合。`os/elf/src/lib.rs:250`、`:262` 分类 program headers、拒绝未知 flags/类型和 PT_INTERP 等未支持语义，合法可忽略 headers 保持明确区分；audit 调用同 Rust validator。 |
| F-07 / token 回绕 | 闭合。`os/monotonic_id/src/lib.rs:15` 最大值发行后永久耗尽，`task/handle.rs:105` 映射 ReachLimit。 |

elf/dtb/frame_pool/handle_table/monotonic_id/shared/libprocess host tests 通过。统筹补跑全速 stress、release、sifive_u、hetero 通过；nofd 的预期 D64 拒绝已发生，但脚本旧锚点失败归 E2-7-02。未逐项执行 BootPackage/ELF/资源失败 guest 注入。七项 finding 均关闭，本报告归档；不将测试限制写成未来能力不存在。

---

以下保留历史目标的首审内容及不通过判定。


> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](todo-2026-09-review-program.md) 为准；正文建议保留首审语境，不作为现行实施顺序。

## 范围与基线

目标范围固定为 `29c6519..1bc83ac`，目标实现提交 `1bc83ac4596d548f47798e053a8104a14a429d97`。当前工作树 HEAD 为 `61490ae`，代码证据全部来自 `git archive 1bc83ac` 隔离快照及目标提交内容，不以当前树后续实现替代历史事实。全程只读，未修改或提交代码。

审查遵循 `plans/REVIEW.md`，参考归档 bootstrap-launcher 计划及机制审查、`notes/ideas/{bootstrap,object,task,service}.md`、`notes/impls/{startup,task,mm,ipc}.md` 与 `references/CONTRACTS.md`。机制层既有 bootstrap 报告不重复方向结论，只审代码十切片。

## 执行命令与结果

```text
git log --oneline 29c6519..1bc83ac
git diff --stat/name-status 29c6519 1bc83ac
git diff --check
git archive 1bc83ac | tar -x -C /tmp/halcyon-d2-review
cd /tmp/halcyon-d2-review/shared && cargo test --target aarch64-apple-darwin
cd /tmp/halcyon-d2-review/os && cargo test -p elf -p page_table -p frame_pool -p dtb -p handle_table --target aarch64-apple-darwin
python3 -m py_compile tools/make-boot-package.py tools/audit-user-elf.py
```

shared 7/7、elf 13、page_table 18、frame_pool 11、dtb 11、handle_table 12 项 host 测试通过；Python 脚本编译通过。目标快照的 dtb/page_table 测试有 dead_code warning。

未运行 `just check`、`build_user`、virt/virt-stress/virt-release/sifive_u/acceptance`，未做故障注入、QEMU 或 litmus 验证；不能用历史计划中的成功回归替代本批语义证明。

## 十切片结论

1. **BootPackage envelope/packer/Justfile/DTS：不通过。** Rust validator 的定宽、LE、checked arithmetic、canonical offset、零 padding 和基本反例覆盖成立；但启动 reservation 区间处理和 sifive_u 内存参数存在 F-02/F-03。
2. **启动物理 reservation/borrowed backing：不通过。** payload PTE 建立后没有对应 AddressSpace owner/teardown 记录，见 F-01；另有未 checked 的地址加法。
3. **StartupBlock outer/child Handles：部分通过。** unaligned 读取、handles_end/payload_off、zero gap、真实 Handle reservation 和 padded prefix validator 基本成立；目标批 PID 仍为 u32，不倒灌后续拓宽。
4. **Job/ProcessBuilder/capability：不通过。** rights、GRANT/TRANSIT 子集校验形状存在，但 ProcessCreate 不产生稳定 ProcessControl，Job 没有成员/lifecycle 记账，见 F-04。
5. **ProcessMap/ProcessWrite：部分通过。** Building-only、用户半区、W-only/W+X、uaccess 完整校验存在；缺少统一 preflight/安装 PTE rollback 的充分证明，初始 ELF 入口问题见 F-05。
6. **ProcessStart：部分通过。** 基本 reservation 和 extract-grants 顺序存在；post-commit 可失败窗口属于已知 B-2，不重复编号；entry/Control 仍有 F-04/F-05/F-06 问题。
7. **process table/ready/SMP：部分通过。** marker 对普通 lookup/pick 隐藏，FIFO 轮转方向成立；reservation token 长期回绕见 F-07，缺并行 marker 模型测试。
8. **ELF loader/入口/W^X：不通过。** 用户 planner/audit 与内核均按 `vaddr..vaddr+memsz` 判定 entry executable，允许 entry 落 BSS；bootstrap 未调用入口验证；PT_INTERP/未知 flags 被静默忽略，见 F-05/F-06。
9. **FENCE.I/SFENCE.VMA：部分通过。** dispatch 本地 `fence.i`、satp 后 `sfence.vma` 存在；缺多 hart litmus 和所有首次 satp/新增 PTE 路径的动态证据，不能把本地 fence 当跨 hart fence。
10. **ABI/error boundary：不通过。** syscall/布局基础对齐；F-04 的 Control/Job 缺口与 F-05 的坏 ELF 入口会把应拒绝输入推迟为运行期 fault；boot-only malformed ELF 的 fatal expect 与普通 ProcessStart 错误边界需区分。

## Findings

### F-01 / P1（历史 finding，已由后续主线修复）：bootstrap payload 映射没有 AddressSpace owner，退出/失败不回收

位置：`os/kernel/src/task/proc.rs:437-513`，尤其 `489-512`；`os/kernel/src/boot.rs:57-67`；`os/kernel/src/frame.rs:61-66`。

可达前提：bootstrap 成功后 init 退出/fault 或进入 teardown，且 `payload_len > 0`。

直接证据（目标提交）：`map_bootstrap_block` 只为 prefix 分配并将 tracker 放入 `frames`；payload PTE 直接指向 `payload_pa/page`，没有保存 payload tracker/extent。AddressSpace Drop 只释放 `frames`；payload 物理页已被 frame init 的 boot-package hole 永久剔除，既不归还也无 owner。当前 HEAD 已由 `BootFundedExtent`/`BootBorrowed`/`install_bootstrap_funding` 接入 payload owner；本条保留目标提交缺口和后续修复证据，不作为当前债务。

违反契约：bootstrap payload backing 应与 init 地址空间生命周期闭合；实现文档声明 payload 在地址空间收束时物理 extent/charge 一并归还；MemoryPool/FramePool 守恒。

建议：引入不可复制的 bootstrap borrowed/owned backing 记录，明确每页/尾页 owner；teardown 先撤 PTE，再锁外一次性归还物理 reservation/charge；失败路径同样由 owner RAII 覆盖，避免先回库存再取出的窗口。

### F-02 / P1：启动 holes 未排序且不检查 package 与 kernel/SBI 重叠

位置：目标 `os/kernel/src/frame.rs:61-76`、`os/kernel/src/board.rs:273-282`。

可达前提：DTS/loader 给出位于 kernel/SBI 之前的 package，或 package 与 kernel/SBI 区间重叠；validator 不改变这组 capacity reservation。

直接证据：`subtract` 假定 holes 已按地址有序，只按传入顺序推进 cursor；没有 sort/merge/overlap reject。frame 初始化的 `addr + len` 也未 checked。board 只验证 package 落在 memory node 内，不验证 reserved interval 两两不重叠。

违反契约：启动 reservation 不重叠、不可重复回投；Devicetree memory/reg 区间边界必须在 admission 阶段闭合。

建议：所有 permanent/boot-held interval 先 checked normalize、排序、合并并显式拒绝或裁剪重叠；FramePool 注册前只消费该 reservation plan 的 token。

### F-03 / P1（历史 finding，已由后续主线修复）：sifive_u QEMU 内存参数与 DTS 不一致

位置：目标 `Justfile:35` 的 QEMU launch 参数；`os/platforms/qemu/sifive_u/device.dts:29-32`；BootPackage window `0x86000000..0x88000000`。

可达前提：执行目标快照的 sifive_u recipe，QEMU/firmware 按 `-m` 生成或重定位 DTB。

直接证据（目标提交）：目标 recipe 硬编码 `-m 1024M`，而 sifive_u DTS memory 为 128MiB。当前 HEAD 的 Justfile 已按 MODEL 选择 virt=1024M、sifive_u=128M；本条保留历史目标缺口，不作为当前债务。

违反契约：项目平台启动边界和 bootstrap reservation 假设。

建议：按模型选择 QEMU memory（virt 1024M、sifive_u 128M），recipe 打印并校验三者一致；启动时拒绝运行时 memory 与静态平台契约不一致。

### F-04 / P1（历史 finding，已由后续主线修复/重构）：ProcessCreate 不产生稳定 ProcessControl，Job 无成员/lifecycle 记账

位置：目标 `os/kernel/src/task/process.rs:209-248`；`os/kernel/src/task/proc.rs:659-683`；`os/kernel/src/task/job.rs:18-23,100-120`；`shared/src/proc.rs:61-68`。

可达前提：用户执行 JobCreate → ProcessCreate 后，在 Start 前 builder 失败/丢失，或管理者需要观察、封口、收束 Building 目标。

直接证据（目标提交）：ProcessCreate 只创建 Process（`control=None`）、ProcessBuilder，输出仅 builder/pid/reservation；ProcessControl 在 ProcessStart 才创建并 attach。当前 HEAD 的 `process::create` 已预构造 ProcessControl 并写入 `ProcessCreateResult`；本条保留历史目标缺口，不作为当前债务。

违反契约：ProcessCreate 应产生稳定 ProcessControl；Job 是创建/收束域，成员关系强持至 Dead；Building shell/Control 生命周期应闭合。

影响：Start 前没有稳定管理 capability，builder 失败只能依赖 Drop；JobControl 无法枚举/封口/观察成员，后续 ProcessControl/Drain 契约无法成立。

建议：ProcessCreate 事务预构造并输出 stable control，Job member marker 与 shell 同步提交；接入 Job 生命周期真值。若本批只承诺最小能力，则必须同步收窄 ABI/ideas/impls，不得继续宣称完整 control/Job 闭包。

### F-05 / P1：bootstrap 初始 ELF 未复用入口/页级执行校验，entry 可落 BSS 或非 X

位置：目标 `os/kernel/src/task/proc.rs:758-778,807-893`；`AddressSpace::load_elf:269-360`；`validate_initial_context:247-264`；`user/frameworks/libprocess/src/lib.rs:113-145`。

可达前提：BootPackage initial ELF 的 `e_entry` 不在实际文件字节区，或 ELF 段几何恶意但仍通过 parser。

直接证据：`spawn_from_elf` 仅执行 ISA requirement、load_elf、map_stack；`launch_bootstrap` 直接 prepare_main_thread，没有调用 `validate_initial_context`。用户 planner/audit 也使用 `start <= entry < start + memsz`，把 PT_LOAD BSS/零页当作 executable entry。

违反契约：entry 必须落在 executable segment 的实际文件字节区；未实现的输入应在 boot boundary 确定性拒绝，不能延迟成用户 fault。

建议：libelf 统一返回 executable file-byte interval；bootstrap 与 libprocess 共用 validator；Start/boot 检查 entry 页 PTE U|X，拒绝 BSS/非-X/越界 entry。

### F-06 / P1：ELF PT_INTERP/未知 program-header flags 被静默忽略，形成半支持

位置：目标 `os/elf/src/lib.rs:100-155`；`tools/audit-user-elf.py:41-55`。

可达前提：ET_EXEC ELF 含 PT_INTERP 或 program-header flags 含未实现位。

直接证据：parser 对非-PT_LOAD header 直接 continue，没有拒绝 PT_INTERP；PT_LOAD 只抽取 R/W/X 三个位，未知位被丢弃；静态 loader 仍可继续处理该 ELF。

违反契约：未实现动态链接必须明确拒绝；ELF program-header flags 不能静默删减；zero-permission PT_LOAD 也不应被半支持。

建议：libelf 遍历时遇 PT_INTERP/不支持 header 直接返回 Unsupported；拒绝 `p_flags & !PF_RWX` 和 zero-permission PT_LOAD；audit 与运行时 parser 共用同一规则。

### F-07 / P2：reservation token 回绕可与存活 marker 冲突

位置：目标 `os/kernel/src/sched.rs:70-82`；`os/kernel/src/task/table.rs:17-55`；`os/kernel/src/task/handle.rs:20-25`。

可达前提：运行足够久使 token 回绕，且旧 marker/transaction 仍存活。

直接证据：只拒绝 token==0；回绕后从 1 重用，未扫描/永久退休/实例 identity；旧 token 可能仍在途。

违反契约：reservation token 应作为单调、不会错误重用的凭据。

建议：不可回绕并耗尽 fail-closed，或引入 generation+实例域/永久退休策略；不要只把零值当耗尽。

## 已证实不变量

- BootPackage validator 的 64B/LE/checked/canonical/zero padding/边界检查和 shared 测试成立。
- StartupBlock 的 unaligned 读取、真实 Handle、handles_end/payload_off、zero gap 和 padded prefix 校验成立。
- HandleTable 的 GRANT/TRANSIT 分离、source 去重、rights 子集和 extract 失败源表不变成立。
- ready marker 不参与普通 lookup/has_ready/pick，FIFO 轮转方向成立；并行 marker 模型测试缺失。
- uaccess 先按 AddressSpace 锁逐页验证 U+R/W，普通非法地址返回 `MemoryNotAccessible`。
- 本地 `fence.i`、satp 后 `sfence.vma` 存在，但跨 hart/所有首次 satp 路径缺动态证据。

## 与 A-C 去重

- 不重复 A 的 WritePermit rollback、retiring owner、post-Commit Vec、EXECUTE capability。
- 不重复 B-1 DT status、FramePool arithmetic、rinlib MemoryPool Drop、SystemSupply ticket query；F-02 是 package reservation interval 的独立边界。
- 不重复 B-2 bootstrap `table.commit` 后失败窗口；该问题作为已知 finding 引用。F-04/F-05 是独立的 Control/Job 与 ELF 校验缺口。
- 不重复 C-1 supervisor、服务静默降级、raw hartid、q-only DT、ThreadControl CLOSED、无限监督等待。
- 不重复 C-2 RemoteCalls identity、epoch overflow、UserStack cleanup。

## 验证缺口

未运行完整 build/check/QEMU/acceptance；未做 BootPackage 截断、overflow、padding、capacity overlap、AppleDouble、frame/page-table OOM、ProcessStart alias/rights/output fault、marker 并行模型、ELF entry-in-BSS/PT_INTERP/unknown flags、RVWMO/satp/SFENCE 多 hart 测试。

## 后续行动与复核条件

以下为首审建议与复核条件；当前有效输入/ELF 问题归 admission、token 归 identity，已修 payload/平台参数/Control 条目不重复实施：

1. 修复 payload owner/teardown 和 reservation interval admission；
2. 修正 sifive_u recipe memory 闭包；
3. 闭合 ProcessCreate stable Control 与 Job membership，或同步收窄承诺的 ABI/设计；
4. 统一 bootstrap/libprocess/audit 的 ELF entry、PT_INTERP、flags 与 zero-permission 拒绝；
5. 处理 reservation token 回绕并补故障注入、host/QEMU 负向验证；
6. 同步更新 `notes/ideas/{bootstrap,task}.md` 与 `notes/impls/{startup,mm,task}.md`；
7. 全部 findings 修复并复核后移入 `plans/archived/`。

## 最终判定

**不通过。** 本批至少有六项 P1（F-01～F-06）和一项 P2（F-07）；其中 F-03 为平台 recipe 硬阻塞，F-01/F-04/F-05/F-06 为运行时或契约硬阻塞。B-2 的 post-commit failure finding 已去重，不重新编号。
