# Halcyon / eRhino

个人兴趣项目，目标是成熟、可长期演进的 RV64 微内核系统。完整项目与仓库叫 Halcyon，包含 eRhino 微内核、用户态系统服务及跨组件契约；内核二进制是 `erhino_kernel`，用户态标准库替代品是 `rinlib`。`git tag pre-ai`（975c46f）之前为第一版手写实现；之后由 AI 协作重写，以成熟系统为标准（生产系统为主要参照，不做教学式简化）。

## 心智模型

以下事实决定每个改动的归属判断：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务（fs/pm 等系统服务），内核只做短路径转发；内核路径恒短，内核态不可打断（协作式）是推论不是选项。若某需求看起来必须内核抢占或内核线程，说明工作被放错了地方，修架构方向而不是中断模型。
- **shared/ 是内核与用户态的 ABI 边界**：改动它的数据结构/消息格式/调用号，内核与 rinlib 两侧同步改，不留单边。ABI 不冻结，随设计演进。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求从简；将来换复杂实现不动结构（调度域/类即范例：类可整体替换、域可横向扩展）。
- **notes/ 按视角分层，不是按阶段**：`ideas/` 与 `impls/` 是看待同一系统的两个视角，不是同一篇文档的前后状态。
    - **ideas/ 写「系统应该是什么」**：自顶向下的概念、边界与构想。动笔时机应领先于代码——天马行空是常态；为已有代码回补时也必须保持抽象视角，只讲概念与契约，不下沉到结构字段与代码引用。idea 的价值在于可脱离实现独立成立（未来的文档网站只收这一层），因此允许与当前代码不一致；不一致既可能是尚未实施，也可能是构想需要修正，必须按目标、外部证据与实际机制重新判断，不能据此预判文档或代码正确。
    - **impls/ 记「实际是怎么做的」**：自底向上的实现现状，应当引用具体模块、结构与路径，随代码演进同步修订，过时即改或删。
    - 同一主题允许两篇并存（如 `ideas/mm.md` 与 `impls/mm.md`）；判断方向意图读 ideas/，判断实现现状读 impls/ 与代码本身——不得拿 idea 篇当实现依据，也不得把 impl 篇当方向结论。
    - 根目录只放导读、索引与跨专题通用内容；方向性结论必须入档（聊天即焚，未入档的决策视为未发生）。

## 仓库结构

三个独立 cargo workspace，靠 path 依赖串联。拆成三个是因为 os（`riscv64gc-unknown-none-elf`）与 user（自定义 JSON target + build-std）运行时不同：cargo 不支持嵌套 workspace，一次构建也只有一个 target，无法合入单一 workspace。

```
os/        内核 workspace：
             kernel/          erhino_kernel（no_std）
             dtb/ frame_pool/ page_table/ handle_table/ wait_context/
             stack_layout/ sched_domain/ ready_queue/ memory_space/ remote_call/
             runtime_gate/ memory_supply/ memory_pool/ funded_frame/ work_debt
shared/    跨层 workspace：
             erhino_shared/  内核与用户态共享的 ABI（syscall、消息格式、同步原语）
             elf/ tar/ monotonic_id/ ordered_table/ timer_queue/ metadata_admission/
             内核与用户态共用的可移植纯逻辑库
user/      用户态 workspace：
             rinlib/
             services/    系统服务（srv_*）
             drivers/     用户态驱动（drv_*）
             tests/       验收进程（test_*）
             frameworks/  用户态公共框架与正式库
notes/     设计文档：
             根      导读、索引与跨专题通用内容
             ideas/  方向性设计——自顶向下的抽象视角
             impls/  实现记录——自底向上的细节视角
plans/     计划与档案，命名纪律见「约定」；入口 COMPASS.md（跨会话导航）
```

对照负载由 `user/services/`、`user/drivers/` 与 `user/tests/` 共同组成：当前服务为 `srv_init`、`srv_fs`、`srv_pm`，驱动为 `drv_spi_sifive`，验收进程为 `test_fp`、`test_hammer`、`test_target`。它们服务于集成验证，不代表未来正式运行配置；其中 `test_fp` 使用 gc target 单独构建。

## 构建与验证

- 构建系统是 [Just](https://just.systems)，**统一走 `just`，不裸跑 `cargo build`**——内核的链接脚本和链接器（`riscv64-elf-ld`）靠 Justfile 注入 RUSTFLAGS；用户态靠自定义 target（`rinlib/riscv64-unknown-erhino-elf.json` + build-std）。
- 秒级检查：`just check`（内核 target 需要 build-std，等价于 `cd os && cargo check -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）；`cd shared && cargo check`。全仓 lint 统一走 `just clippy`，按 shared/os/user host、kernel/user RISC-V、stress feature 与独立 gc target 分面执行 `-D warnings`，完整日志写入 `artifacts/lint/`；`just acceptance` 会先通过该门再运行 QEMU 路线。
- host 单测（纯逻辑 crate，毫秒级）：**必须显式指 host target**——os workspace 默认 target 是 riscv，`cargo test` 直接跑会拿 no_std 环境去链 std：
  ```sh
  cd os && cargo test --workspace --exclude erhino_kernel --target aarch64-apple-darwin
  cd shared && cargo test --workspace --target aarch64-apple-darwin   # shared 也需显式 host target
  ```
- 集成验证分档由用户态 `srv_init` 编译期 workload 控制，内核不感知测试政策：`just virt` 是日常 core 快线（确定性内存/IPC/Tunnel/Job/监督/reset）；`just virt-stress` 追加 control/Tunnel 重复压力、`max_work=1` Drain 与完整 16/16 竞态矩阵；`just virt-release` 以 core 覆盖优化代码生成和 trap 寄存器保持；`just acceptance` 是阶段收尾聚合，静态门后执行 debug stress、release core、`sifive_u` core、`virt-nofd` 和 `virt-boot-failure`。后者用外部 GDB 对正式 debug binary 注入 Ready 前 panic/alloc/fatal，并确认所有 hart 在 Failed 后停驻；完整日志见 `artifacts/boot-failure/`。涉及调度域契约时另跑 `virt-hetero`。
- 常规 QEMU recipe 经 `tools/qemu-throttle.sh`（默认 50% 节流运行；stress 同样保持节流，不作为全速性能基准）和 `tools/qemu-acceptance.sh`，并按路线使用独立、可由同名 `*_TIMEOUT` 环境变量覆盖的运行超时。`virt-boot-failure` 同样节流，但由 `tools/check-boot-failure.py` 独立判定预期故障、限制调试器运行时间并回收进程组，超时可用 `VIRT_BOOT_FAILURE_TIMEOUT` 覆盖。超时从 QEMU 运行阶段计，不含冷编译。默认值依据对应 workload 的近期实测耗时留出宽裕余量，验收面或运行成本变化时应直接重校，不把旧数值当架构约束。判定以 workload 身份、业务收束与 reset 锚点为准，不能把矩阵中途超时当成内核挂死。全速调试用 `THROTTLE=100`。
- `sifive_u` 是老的 HiFive Unleashed 模型：hart 0 无 MMU、可运行 hart 为 1–4、DRAM 128MiB、timebase 1MHz、boot hart 不固定。内核仍使用运行时探测到的现代 SBI，不因平台历史包袱调用 v0.1 核心 ABI。该模型无可用 shutdown 后端；`just sifive_u` 在日志出现明确 reset 失败或 panic 终态时主动收割，仍检查完整 core 锚点。它只覆盖板级差异，不为平台引入专用内核机制。
- 开发机是 macOS：`just dtc qemu riscv64-elf-binutils riscv64-elf-gdb` 来自 Homebrew。打 tar 包时注意 bsdtar 的 `._` AppleDouble 文件会污染 initfs（历史上因此 panic 过）。
- Rust nightly（`rust-toolchain` 钉住），edition 2024。
- QEMU 超时只负责收割异常停滞的 guest，不是性能目标或固定上限；CPU 节流负责限制跑飞时的宿主资源占用。各路线默认值通常取近期正常耗时的宽裕倍数，若正常验收经常逼近或超过现值，应同步调大 recipe 并更新相关文档，而不是把基础设施截断误判为内核故障。调查已定位的早期卡死时可临时按最后锚点收紧；任何退出或超时后确认无残留进程。
- 长时间构建、测试和验收命令默认压缩终端输出：脚本应将完整输出写入日志，只展示末尾摘要；失败必须保留完整日志并打印路径。摘要行数可通过对应脚本的环境变量调整。不得用 `tail` 截断后丢失退出码或覆盖错误上下文；需要诊断时再按日志定位读取完整错误。

## 约定

- 文档、注释、提交信息都用中文。格式上使用 Conventional Commits，以前的提交未使用标准格式，不当作参考。
- **Rust 组件身份唯一**：目录叶名、Cargo package 名、默认 binary/library target 名与 crate identifier 必须一致，不设置只为补偿命名差异的别名。系统服务使用 `srv_<domain>`，用户态驱动使用 `drv_<domain>`，验收进程使用 `test_<domain>`；正式库跟随领域正式名（FAL → `libfal`，Runnel → `librunnel`）。禁止 `_src`、`_impl` 等实现痕迹后缀，不为省字发明新缩写。这里约束的是源码与 Rust 构建身份，不预设未来的 Display Name、服务发现名或运行时实例名。
- **运行时输出统一正式英文**：内核与用户态日志、panic/assert/expect 消息、构建工具输出（如 Justfile 的 echo）一律用正式英文措辞，保证可 grep、可跨终端阅读；中文仅出现在文档、注释与提交信息中。
- `git tag pre-ai` 之前的提交全部为人工编写，不含 AI 参与；之后的提交如由 AI 辅助，提交前按当前会话的实际模型与 provider 生成 `Co-Authored-By` trailer。格式为：
  ```
  Co-Authored-By: <实际模型显示名> <对应 provider 的 noreply 邮箱>
  ```
  不确定自己是什么模型时直接问用户，不猜测；模型没有对应邮箱时缺省用 `noreply@pi.dev`，不得伪造真实 provider 域名。
- 设计取舍记录在 notes/，不要在代码里留「原来是 A 改成 B」式的历史注释，追溯看 git log。

### plans/ 命名纪律

文件名即性质，从名字直接读出生命周期，不靠打开内容判断：

- **全大写**（`COMPASS.md`、`DEBUG-PLAYBOOK.md`、`TOOLING-PITFALLS.md`）：常驻手册，长期有效、经常阅读；
- **`todo-<日期>-<主题>.md`**：待实施计划。完成后归档，有留存价值的结论转 notes/ 或 KNOWN_ISSUES；子类【未来审查计划】（`todo-<日期>-<主题>-review.md`）在**提交之后**生成：记录任务对应的提交哈希与改动概要，供日后 Review 对照（Review 的对象是提交）；
- **`<类型>-<日期>-<主题>.md`**：调查复盘与参考资料（现有类型 `review-` 调查归档、`ref-` 对照资料），只读；
- **`archived/`**：已结束且无留存价值的计划尸体，只进不出；
- 新类型前缀按需增设，但必须能一句话说清其生命周期；不引入需要枚举场景才能维持的分类。

## 标准施工流程

每次开始或恢复专题，统一按以下顺序推进。专题计划只补充本专题的目标、依赖、设计选择和完成门，不重复定义流程。任一步骤发现前提不成立，都回到相应步骤修订唯一计划、依赖图和文档，再继续施工。

1. **接手与基线**：核对分支、提交、工作树、`plans/COMPASS.md`、唯一 todo、目标/非目标、已完成前置及可追溯验证证据。完成标准是范围、基线和当前真值点明确。
2. **任务规模审计**：从目标契约盘点真实消费者、ABI 两侧、owner/authority、资源来源、锁序、跨 hart、正常/失败/取消/退出/超时/退款路径、旧机制和外部契约。规模按语义责任链衡量，不按文件数或代码行数衡量。完成标准是全部责任链有代码/文档落点，未知项和缺失前置已列明。
3. **拆分/合并与依赖图**：按可独立证明的语义闭包划分任务；强耦合的 ABI、所有权、通知、退休和真实消费者共同迁移，缺失前置独立立案。完成标准是唯一计划记录每项的前置、真实调用者、失败边界、删除条件、验证门和自然顺序。
4. **设计闭包**：确定目标类型图、所有权/授权图、状态机、线性化点、锁阶、停驻/唤醒、失败/取消/退出/退款及公开语义；涉及硬件、ABI 或协议时先取证。完成标准是当前范围语义闭合，未决选择均已裁决或成为阻塞项，方向结论进入 `notes/ideas/`，实现基线进入 `notes/impls/` 或计划盘点。
5. **按依赖实施**：自底向上修改公共契约与核心机制，同时迁移全部真实消费者、失败/退役路径并删除旧路径。完成标准是当前闭包的代码、调用者和清理责任全部接通，不存在未登记的兼容层、双轨或测试专用运行体。
6. **分层验证**：先做受影响的 host/target 检查和单测，再做 `just check`、`just clippy` 及必要 QEMU/平台/退出组合；失败保留完整日志和责任定位。局部通过不等于专题完成，已知延期项不得冒充通过。
7. **结构收口 Review**：对照最终设计检查 bug、性能、锁序、owner、重复真值、旧路径、文档一致性和残留清单；已有 findings 必须复核。完成标准是开放问题均有关闭证据或唯一延期条目。
8. **组合收口与归档**：完成跨机制组合门，更新 `notes/impls/`、`plans/COMPASS.md` 和专题状态；完成的计划归档，提交前展示摘要并取得授权，提交后登记固定提交 Review。实现证据推翻设计时，停止当前步骤并回到规模审计/设计，不以编译通过掩盖边界错误。

## 设计与施工准则

以下是各阶段的判断准则，不另定义施工顺序：

- **决策边界**：默认直接采用经过论证的推荐方案。只有改变已确认外部语义、无法形成语义闭包，或 notes/plans 与目标冲突的架构点才暂停交用户拍板；普通内部结构、容量布局和实现取舍由实施者决定。
- **决策即文档**：可长期成立的方向与契约进入 `notes/ideas/`，当前实现事实进入 `notes/impls/`，任务边界、依赖、完成证据和延期进入唯一计划；聊天中的决定不构成项目真值。
- **独立理由与高起点**：方案从需求独立推导，以成熟生产系统为主要参照，不以兼容、延续、仿照现有代码或 `pre-ai` 为理由。外部实现引用前查证官方资料；需要系统对照时从 `references/systems/INDEX.md` 广泛取样。
- **外部契约先取证**：凡语义由硬件、ABI 或协议决定，编码和 Review 前从 `references/CONTRACTS.md` 定位固定规范并引用具体章节；记忆不作证据。
- **长期合理性与最终形态优先**：从整体所有权、生命周期、失败边界和未来扩展选择机制，优先消灭一类问题的机制重构。阶段实现可以从简，但目标结构必须允许后续以能力增量或实现替换演进。
- **语义闭包**：当前承诺范围内的前置、真实调用者、失败/退役和退款必须完整。发现缺失前置即回到流程第 2–4 步并重排；缩小范围只能减少能力，不能留下已知正确性欠账。
- **任务边界**：任务粒度以可独立论证和验证的机制闭包为准；ABI 两侧、所有权转移、来源通知、退休及其真实消费者强耦合时共同设计和迁移。纯文件分割、syscall 分割、happy path 或临时 adapter 不构成闭包。同一机制保持单一设计所有权；接力闭包默认在既有铺路上续建，不另起平行方案。
- **铺路面与消费者**：无即时消费者不是删除或禁建的理由——系统完整性与为后续闭包铺路都是正当交付。已铺路的实现默认续用，替换须有严格更好的替代（正确性、解耦或语义消重）并记录依据；删除判据是「有更好替代」或「确证错误」，不是「暂无消费者」。闭包声称完成的能力仍必须接通真实调用者与失败/清理路径；形状依赖首个真实消费者使用模式的机制先定接缝、不提前建实现。
- **工程限额必须有依据**：栈容量、guard 布局、审计阈值等自设容量须来自硬件、ABI、正确性或资源成本。没有独立依据的旧限额应放宽并同步重校布局、审计和文档；可推导阈值只保留一个真值来源。
- **范式纪律**：性能问题先在既定范式内逐点优化；只有范式本身被证明错误时才推翻。没有存量用户，迁移与 ABI 破坏不是保留旧结构的理由，统一性与长期正确性优先。
- **延期与临时机制**：只有缩小后的范围仍语义闭合且补齐收益较低时才延期；同一缺口只进入一个 `todo-*.md`，写明触发条件、完成标准和自然顺序。任何过渡类型、字段、adapter、兼容分支或重复验证路径必须同时登记替代物、保留期限、删除条件和清理验证。
- **残留即见即清**：当前闭包内可消灭的过渡类型、重复真值和错位命名立即清理；超出闭包的残留立即进入唯一专题清单。清单记录现状、目标、位置、删除条件、验证标准和自然顺序，完成后删除条目或转入长期 notes。
- **Review 面向最终结构**：Review 不只检查 bug、性能和测试，还要识别迁移后遗留的旧路径、重复 owner、条件分支和文档错位；确认目标结构后直接迁移调用点并删除旧机制。

## 已知问题

@KNOWN_ISSUES.md
