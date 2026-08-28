# 调度域 eligibility 接线与 D64 开放计划

> 生命周期主线 `todo-2026-08-26-process-lifecycle.md` 的 step 8。设计已拍板（2026-08-28），方向公理入档 `notes/ideas/execution-context.md`「调度域」；本篇记录决策推导、机制清单与实施验收。

## 目标

把 ELF 执行需求（`IsaRequirement`）与 hart 硬件能力（`HartCapabilities`）之间的准入判定接线为「兼容调度域」：boot 期按能力事实推导域划分，ProcessStart 按执行需求路由进兼容域并冻结绑定，随后开放 D64 profile。完成后 `ProcessStart` 不再对 D64 整体拒绝，无兼容 hart 的 profile 以明确错误拒绝。

## 当前基座（已就绪，无需重做）

- 事实侧：DT 现代属性逐 hart 解析 → `SLOT_CAPS` 快照（`registry::store_caps`，注释即预留本批启用）；
- 需求侧：`elf::IsaRequirement` + `compatible()` 谓词（Q 谓词缺陷见决策 7）；
- 执行面：FP 上下文完整落地（条件保存/恢复、FS 纪律、HartLocal `fp_enabled` 槽位）；
- 结构面：`SchedDomain` / `SchedClass` trait / HartLocal 三层就位，但单域单类硬编码静态（`static FAIR` / `static DOMAIN`）；
- 用户侧：双 target JSON（`riscv64imac-` / `riscv64gc-unknown-erhino-elf`）已备，`libprocess` 已映射 `ExecutionProfile`。

## 已拍板的结构决策（2026-08-28）

1. **域推导 = 需求满足签名等价类**：域 = 满足同一组执行需求的 hart 等价类；与需求无关的能力差异（如 V）不产生调度边界。以当前需求全集 {Base64, D64} 计，签名只有 `{B}` 与 `{B,D}` 两种——**最多两域**（D64 兼容域 = `d ∧ ¬q` 的 hart，其余为 Base64-only 域）。新需求（V64/Fp128）加入时按同一规则细化，细化只分裂既有域、不使既有绑定失效（细化后各成员仍满足原域全部需求）。否决：全 caps 等价类（无关扩展差异碎片化，D64 可跑容量被无谓分割）；格子/重叠模型（违反 hart 单域归属，HartLocal 单域指针、IPI scoping、quiescent 全部复杂化）。
2. **绑定冻结点三处**：hart→域 boot 冻结（域对象 `Box::leak` 成 `'static`，永不析构；axiom——caps 是 DT boot 事实，运行中不变，热插拔/域收缩是未来平台议题）；process→域在 ProcessStart 线性化点冻结（与 Building→Running 同一临界区）；线程经 process 间接持有。跨域迁移是显式政策操作（ideas 公理已有），本批不提供迁移路径——无调用者，绑定即终局。
3. **多域默认政策 = 最弱兼容域**：无状态结构规则，稀缺能力容量留给必须使用它的线程（big.LITTLE 惯例）；放置政策的显式覆盖（affinity ABI）推迟到 ThreadSpawn/迁移纪元——ABI 不冻结，现行两板（virt / sifive_u admitted 集）皆单域，默认政策无实操差异。否决：最大兼容域（吞吐优先但 D64 到达即争用）；现在加 ABI 字段（无消费者）。
4. **eligibility 判定接入**：`ProcessStart` 无兼容域 → `NotSupported`（语义自然从「内核不支持」转为「平台无兼容 hart」，两种情况调用者均无法补救，错误码不改）；init bootstrap 路径同判定，无兼容域 → boot fatal（RuntimeGate 整体失败不降级）；dispatch 处 debug 断言 `me.domain` 接受 `t.requirement`（结构上 hart 只从本域 pick，断言是纵深防御）。Base64 恒有兼容域是准入不变量（admission ⇒ RV64IMAC 基线 ⇒ Base64 兼容，board 基线断言保证）。
5. **F2 预留通道上收为 `SchedClass` trait 契约**（推导唯一解，非偏好）：marker 的意义是目标容器容量占位，容量必须预留在线程将要进入的那个类的队列里，域层独立通道无法保证类队列容量。reserve/commit/rollback 进 trait（协议四要素不变），域提供 `reserve(requirement)` 路由到政策选定的类；token 保持全局单调。关闭 carryover F2。
6. **CPU 预约与 MemoryObject 不入本批**：预约的驱动者是不可信域与资源政策，两者不存在，设计无独立理由锚点。接口与文档已留（用户拍板约束）：正交公理与 pick 边界接入契约入档 `notes/ideas/task.md`（配额过滤在 pick 边界、eligibility 判定在入队侧域路由、接入预约不触碰域路由结构），未来实施按既定设计走。
7. **D64 谓词修正（随批必修）**：有效 FLEN = `q?128 : d?64 : f?32 : 0`，D64 兼容要求 FLEN 恰 64（即 `d ∧ ¬q`）。当前 `flen()` 忽略 Q，Q hart 会被误判兼容——与 execution-context.md「Q-capable 不能跑 D64」矛盾。

## 机制清单（改动面）

| 现状（单域硬连） | 改为 |
|---|---|
| `static FAIR` / `static DOMAIN` | boot 期按签名推导构造域（各含自己的 FairClass 实例），`Box::leak`；域构造位于 registry caps 冻结后、初始任务装载前（RuntimeGate「调度域与初始任务就绪后才发布 Ready」既有序） |
| HartLocal 无域指针 | `SLOT_DOMAINS: [&'static SchedDomain; N]`（与 `SLOT_CAPS` 同型，boot 写后只读；HartLocal 128B/16 槽已满，不扩） |
| `reserve_ready` 自由函数 | trait 方法 + 域路由（决策 5） |
| `enqueue`/Requeue → `FAIR` | `t.process.domain` 的公平类 |
| `IDLE_MASK` 全局 | 每域一个 `AtomicU64`，wake 门铃只打本域 idle hart；idle 双重检查同样限本域 |
| `domain_has_ready()`（SSIP 分支） | `me.domain.has_ready()` |
| quiescent 单域谓词 | 全 idle ∧ 所有域 `!has_ready` ∧ 期限表全空 |
| `ProcessStart` D64 → `NotSupported` | requirement → 兼容域集合 → 空则 `NotSupported`、非空按最弱兼容域选定 → 目标域公平类 reserve |

期限表 per-hart 不受影响：到期 hart 只做 enqueue + 打线程所属域的门铃，自身不必属于该域。

## 实施顺序（全部完成，2026-08-28 收口）

1. ~~谓词修正 + 域推导纯逻辑（host 可测：签名划分、Q 反例、Base64 恒兼容不变量、最弱兼容域选择）；~~
2. ~~域构造与 `SLOT_DOMAINS`、scoping 改造（enqueue/requeue/wake/IDLE_MASK/SSIP/quiescent）；~~
3. ~~F2 trait 上收与域路由 reserve；~~
4. ~~`ProcessStart` eligibility 接线、D64 拒绝移除、init bootstrap 判定；~~
5. ~~D64 验证服务（gc target，FP 计算结果校验）接入对照负载；~~
6. ~~多域集成测试：定制 DTB（virt 部分 hart 声明缺 f/d——内核信 DT，保守正确）验证两域拓扑、D64 落 FD 域、Base64 落最弱域、全无 FD 时 D64 → `NotSupported`；~~
7. ~~收尾：`just virt-release`、sifive_u、文档同步（impls/task.md 调度节与演进点、execution-context.md、startup.md D64 拒绝行、COMPASS）。~~

## 实施收口注记（2026-08-28）

- 域推导/谓词落在 `os/sched_domain` 纯逻辑 crate（`HartCapabilities` 自 board.rs 迁入，`flen()` 含 Q→128）；`SLOT_CAPS` 配套 `load_caps` 读回。域表落地为 `sched::DOMAINS`（`AtomicPtr<DomainTable>` Release/Acquire 发布，含 plan/by_slot/domains 三字段）而非机制清单原设想的独立 `SLOT_DOMAINS` 静态数组——访问模式等价，命名以代码为准。
- 域表发布用 `AtomicPtr`（boot 单核 Release 写、运行期 Acquire 读），Process 域绑定同型（swap 断言单次）。
- D64 验证负载 `srv_fp`（gc target 独立构建，Justfile `--exclude` 排出 imac 批量）：fsqrt/fmadd 位型、f30/f31 与 fcsr(RTZ) 跨 trap 往返、sleep 轮转复检（4 轮）；FP 计算以位型接口 + 硬寄存器名内联（freg 操作数类在 imac 默认 target 下非法，会拖垮 user workspace 裸 check）。
- 多域变体经 `tools/make-hetero-dts.py` + `ERHINO_DTB` 环境变量（官方 DTB 固定生成路径，不随 env 重定向——make_dtb 曾因此覆写变体）。验收矩阵：virt/virt-release/virt-hetero(±release)/virt-nofd/sifive_u/host 全绿。

## 完成标准

- D64 profile 在有兼容 hart 的平台可正常启动运行，FP 状态经真实用户态负载验证；
- 无兼容 hart 的 profile 得到明确错误，内核不 panic、不降级；
- 单归属不变量在多域下保持：线程只在所属域的类队列出现，IPI 门铃只达本域 idle hart；
- quiescent 静默停机在多域拓扑下不误判；
- reserve/commit/rollback 协议成为类契约，域路由无自由函数旁路；
- host 单测与 virt / sifive_u / release 集成线全绿。
