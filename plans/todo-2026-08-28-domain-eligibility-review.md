# 调度域 eligibility 与 D64 开放 Review 计划

> 【未来审查计划】对象是生命周期 step 8 的提交 `1d7dc92`；Review 纪律见 [`REVIEW.md`](REVIEW.md)。设计决策（批次范围、签名等价类域推导、最弱兼容域默认、F2 trait 上收、Q 谓词修正）与完整推导见 [archived/todo-2026-08-28-domain-eligibility.md](archived/todo-2026-08-28-domain-eligibility.md)，方向公理见 `notes/ideas/execution-context.md`「调度域」，实现现状见 `notes/impls/execution-context.md`「身份、能力与域」。

## 提交对照

| 提交 | 内容 |
|---|---|
| `1d7dc92` | feat(kernel)：step 8 主体——新纯逻辑 crate `os/sched_domain`（HartCapabilities 迁入 + flen 含 Q→128、签名等价类 plan、最弱兼容域 resolve、7 项 host 单测）；sched.rs 重写调度段（SchedDomain 运行期构造 + 域内 idle 位图、AtomicPtr\<DomainTable\> 发布、current_domain/resolve_domain、SchedClass trait 上收 reserve/commit/rollback、enqueue/requeue/wake/SSIP/quiescent 域化、dispatch 域 debug 断言）；Process 域绑定（AtomicPtr swap 单次断言）+ ProcessStart eligibility 接线（无兼容域 NotSupported）+ launch_bootstrap 同判定；srv_fp D64 验证负载（gc target）；tools/make-hetero-dts.py + virt-hetero/virt-nofd 验收线；notes 三篇与 plans 导航同步 |

## Review 轴（代码为主）

### 域推导与谓词

- `sched_domain::plan` 的等价类合并正确性：签名相同即同域（位置查找 + 追加），域按 slot 升序首现编号——审查 `DomainPlan::resolve` 的 min_by_key（popcount, index）tie-break 确定性与「最弱兼容域」语义的吻合（不可比签名集合出现时仍确定性）。
- flen 语义修正的完备性：`q→128` 对「Q 蕴含 D 但 FLEN 128」的编码前提（DT 契约校验 d∧¬f 拒绝，但 q∧¬d 未显式拒绝——病态输入下 flen=128 仍保守排除 D64，是否需要 board 解析层补 q⇒d 校验）。
- `REQUIREMENTS` 位序与 mask 消费点（kernel 拓扑打印、resolve）的单一真值性：新需求档位加入时只改 crate 内一处，无散落 mask 字面量。

### 域表发布与绑定冻结

- `AtomicPtr<DomainTable>` 的 Release/Acquire 发布：secondary hart 在 RuntimeGate Ready（Acquire）后进入调度循环，域名表 store（Release）先于 publish_ready（Release）——两段 Release/Acquire 链的可见性论证（publish_ready 是否提供足够 happens-before）。
- `Process::bind_domain` 的 swap 单次断言：Start 提交区与 bootstrap launch 各一次，无并发双写路径；未绑定即 domain() 的 expect 在多核下不可能被跨 hart 触达（enqueue 只发生在 commit_ready 之后，commit 与 bind 的顺序保证）。
- bind 与 commit_ready 的顺序（bind 先于 commit）：commit 后线程立即对其他 hart 可见（wake_one），并发 enqueue 读 domain() 必见已绑定值——Release swap 与 wake IPI 链的内存序。

### trait 上收与域路由

- `SchedClass::reserve` 的 token 全局单调（NEXT_READY_RESERVATION 全局）：跨域跨类 token 惟一，commit/rollback 在指定域的类内按 token find——无跨类错认窗口（Reservation 句柄携带 domain+token 的不可伪造性）。
- FairClass::pick 的 Reserved 轮转（pop→push_back）在有界轮次内终止：marker 不被 pick 消费、不参与 has_ready，协议四要素逐点复核。
- 域路由的唯一性：reserve_ready/commit_ready/rollback_ready 的 domain 参数全部来自同一 resolve 结果（start_staged 局部变量），无中途重解析导致域漂移。

### scoping 与 idle 路由

- enqueue 的域路由（t.process.domain）与 pick 的域来源（me_domain）一致性：debug 断言只覆盖 dispatch 点，wake 路径（wait::wake → sched::enqueue）与 Requeue 路径的域来源审查——是否所有容器转换都结构性地落在绑定域。
- per-domain idle 位图的双重检查闭合：登记 idle 后查本域 has_ready 的窗口（他域 enqueue 打本域门铃、本域 enqueue 在登记前）——IPI 丢失不可能（门铃只发给已登记 idle 位，登记后必有重查）。
- `is_quiescent` 已由显式系统复位删除；确认 per-domain idle 位图只用于 IPI 目标选择，不再聚合成整机生命周期谓词。
- IPI 目标展开（ipi_slots）消费域内位图：slot→raw hartid 转换边界未变。

### D64 负载与验证线

- srv_fp 的 FPR probe（f30/f31 硬名）对编译器 FP 使用的假设：rinlib/debug!/sys_sleep 路径当前无 FP 指令生成——依赖 gcc target 全量重编（build-std）下 rinlib 也无 FP；若未来 rinlib 引入 FP（如格式化浮点），probe 可能被调用点 clobber，验收线将以 FAILED 暴露——假设是否值得在服务内注释或 probe 加固（读写间无任何 Rust 调用）。
- fcsr RTZ 测试的数学前提（5/7 尾数余数 ≈ 6/7 ulp > 1/2）：RNE 进位、RTZ 截断必不同——前提的正确性复核。
- make-hetero-dts.py 的变换稳健性：初版块退出用 `}` 精确比较（DTS 块结束实为 `};`，死分支），靠后续 `cpu@N` 刷新碰巧正确——已在 `4280d3b` 改为花括号深度跟踪（嵌套子节点如 interrupt-controller 正确闭合）；审查修正版对单行多括号、cpu 块外 isa-extensions 等病态输入的行为与 hetero/all 两种变换的边界。
- ERHINO_DTB 覆盖与 make_dtb 固定路径：变体 DTB 不再被官方构建覆写；DTB_DEFAULT/DTB 双变量的语义分界清晰。

### 既有回归面

- 单域拓扑（virt/sifive_u）下域内 idle/pick/IPI 路由与多域使用同一机制；系统终局独立走 capability 授权的显式 reset。
- init 的 ustar 字母序启动新增 srv_fp（pid 序移位）：init/pm 剧本对 pid 的硬编码引用（若有）不受影响——验收线通过已旁证，审查确认无 pid 字面量假设。
- user workspace 裸 check（imac 默认 target）经 srv_fp 的位型接口保持绿：freg 操作数类已消除，确认无其他 target-feature 依赖面。
