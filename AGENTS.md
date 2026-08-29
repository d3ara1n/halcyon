# Halcyon / eRhino

个人兴趣项目，目标是成熟、可长期演进的 RV64 微内核系统。仓库叫 halcyon，操作系统叫 eRhino，内核二进制是 `erhino_kernel`，用户态标准库替代品是 `rinlib`——四个名字指同一个项目。`git tag pre-ai`（975c46f）之前为第一版手写实现；之后由 AI 协作重写，以成熟系统为标准（生产系统为主要参照，不做教学式简化）。

## 心智模型

以下事实决定每个改动的归属判断：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务（fs/pm 等系统服务），内核只做短路径转发；内核路径恒短，内核态不可打断（协作式）是推论不是选项。若某需求看起来必须内核抢占或内核线程，说明工作被放错了地方，修架构方向而不是中断模型。
- **shared/ 是内核与用户态的 ABI 边界**：改动它的数据结构/消息格式/调用号，内核与 rinlib 两侧同步改，不留单边。ABI 不冻结，随设计演进。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求从简；将来换复杂实现不动结构（调度域/类即范例：类可整体替换、域可横向扩展）。
- **notes/ 按视角分层，不是按阶段**：`ideas/` 与 `impls/` 是看待同一系统的两个视角，不是同一篇文档的前后状态。
    - **ideas/ 写「系统应该是什么」**：自顶向下的概念、边界与构想。动笔时机应领先于代码——天马行空是常态；为已有代码回补时也必须保持抽象视角，只讲概念与契约，不下沉到结构字段与代码引用。idea 的价值在于可脱离实现独立成立（未来的文档网站只收这一层），因此允许与当前代码不一致——那说明代码落后于设计，不是文档错了。
    - **impls/ 记「实际是怎么做的」**：自底向上的实现现状，应当引用具体模块、结构与路径，随代码演进同步修订，过时即改或删。
    - 同一主题允许两篇并存（如 `ideas/mm.md` 与 `impls/mm.md`）；判断方向意图读 ideas/，判断实现现状读 impls/ 与代码本身——不得拿 idea 篇当实现依据，也不得把 impl 篇当方向结论。
    - 根目录只放导读、索引与跨专题通用内容；方向性结论必须入档（聊天即焚，未入档的决策视为未发生）。

## 仓库结构

三个独立 cargo workspace，靠 path 依赖串联。拆成三个是因为 os（`riscv64gc-unknown-none-elf`）与 user（自定义 JSON target + build-std）运行时不同：cargo 不支持嵌套 workspace，一次构建也只有一个 target，无法合入单一 workspace。

```
os/        内核 workspace：
             kernel/          erhino_kernel（no_std）
             dtb/ frame_pool/ page_table/ tar/ elf/ handle_table/
             wait_context/ timer_queue/ stack_layout/ sched_domain/   纯逻辑 crate，host 可测
shared/    erhino_shared：内核与用户态共享的 ABI（syscall、消息格式、同步原语）；FAL 是纯用户态线协议，落 user/frameworks/libfal，不在此处
user/      用户态 workspace（rinlib、systems/、frameworks/、drivers/）
notes/     设计文档：
             根      导读、索引与跨专题通用内容
             ideas/  方向性设计——自顶向下的抽象视角
             impls/  实现记录——自底向上的细节视角
plans/     计划与档案，命名纪律见「约定」；入口 COMPASS.md（跨会话导航）
```

对照负载：`user/systems/` + `user/drivers/` 的五个服务（fs/init/pm/drv_spi_sifive/srv_fp，其中 srv_fp 是 D64 验证负载，经 gc target 单独构建）是集成验证负载，其中 fs 依赖 FAL——fs「干净被杀」（用户态 panic → 退出回收，内核不崩）是其依赖面就绪前的达标线。

## 构建与验证

- 构建系统是 [Just](https://just.systems)，**统一走 `just`，不裸跑 `cargo build`**——内核的链接脚本和链接器（`riscv64-elf-ld`）靠 Justfile 注入 RUSTFLAGS；用户态靠自定义 target（`rinlib/riscv64-unknown-erhino-elf.json` + build-std）。
- 秒级检查：`just check`（内核 target 需要 build-std，等价于 `cd os && cargo check -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）；`cd shared && cargo check`。
- host 单测（纯逻辑 crate，毫秒级）：**必须显式指 host target**——os workspace 默认 target 是 riscv，`cargo test` 直接跑会拿 no_std 环境去链 std：
  ```sh
  cd os && cargo test -p tar -p elf -p page_table -p frame_pool -p dtb -p handle_table -p wait_context -p timer_queue -p stack_layout -p sched_domain --target aarch64-apple-darwin
  cd shared && cargo test --target aarch64-apple-darwin   # shared 也需显式 host target
  ```
- 集成验证：`just virt`（4 核）——对照负载与资源收束完成后，init 以 `SystemReset` capability 显式提交 shutdown；内核接受请求且 QEMU 退出才通过。agent 环境仍需带超时防挂起，判定看启动日志关键行。`just virt` 默认经 `tools/qemu-throttle.sh` 节流到 50% CPU（guest 跑飞/panic 时 QEMU 满核空转的兜底，见 Justfile `THROTTLE`），墙钟耗时约翻倍、验证语义不变，全速调试用 `THROTTLE=100 just virt`（油门经环境变量穿透嵌套 recipe，recipe 参数写法无效）；**阶段收尾前必跑 `just virt-release`**——debug 代码生成不在 ecall 周边把活值留在 t 系寄存器，用户侧寄存器保持语义只有 release 能测出（trap 入口 x5 破坏事故，见 plans/archived 调查档案）；`sifive_u` 是老的 HiFive Unleashed 硬件模型：hart 0 无 MMU、可运行 hart 为 1–4、DRAM 仅 128MiB、timebase 为 1MHz，boot hart 不固定。当前 QEMU 使用 OpenSBI 现代 SBI（以启动日志和 BASE 探测为准），不得因机器模型历史包袱使用 SBI v0.1 核心 ABI。该模型没有平台 shutdown device，SRST 扩展可能存在但 shutdown 不保证让 QEMU 退出；运行阶段统一走 **`just sifive_u`**：经 throttle 包装，`tools/qemu-acceptance.sh` 在日志出现明确 reset 后端失败或 panic 终态时主动收割，正常轮次实测约 20s；`ACCEPTANCE_TIMEOUT`（默认 60s）只是真挂死的兜底，不是期望耗时，不得为“看着快”把它调小到验收面完成时长以下——砍在矩阵中段的形态与真挂死无法区分。判定只看锚点与最后若干行输出，不得把冷编译耗时计作内核运行卡死。`sifive_u` 只覆盖上述板级差异，不为其引入专用内核机制；出现挂起先检查内核并发、IPI、timer、SMP 启动同步，再判断为平台限制。
- 开发机是 macOS：`just dtc qemu riscv64-elf-binutils riscv64-elf-gdb` 来自 Homebrew。打 tar 包时注意 bsdtar 的 `._` AppleDouble 文件会污染 initfs（历史上因此 panic 过）。
- Rust nightly（`rust-toolchain` 钉住），edition 2024。
- 在调查内核异常比如卡死的情况时，需要先编译后运行，编译会在十秒内完成，所以超时时间为 10 秒，运行会在 2 秒内完成，所以超时时间只能是 2 秒（`just virt` 默认节流 50% 时墙钟约翻倍，按 4 秒计）。两个同时进行那么超时时间就是 12 秒（节流档 14 秒）。任何高于这个时间的超时设定都没有任何能产生实质改变的意义，只会增加 qemu 进程的空跑，增加电脑发烫时间。

## 约定

- 文档、注释、提交信息都用中文。格式上使用 Conventional Commits，以前的提交未使用标准格式，不当作参考。
- 库与服务的命名跟随领域术语的正式名（FAL → `libfal`，Runnel → `librunnel`）；不为省字发明新缩写。
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

## 设计原则

全周期适用的章法：

- **决策即讨论**：架构级决策点必须带选项与理由交用户拍板，不经确认不自行实施；语义模糊或 notes/plans 与目标冲突时，先问后动。
- **决策即文档**：讨论的结果和决策可以得出的宽泛的设计需要进入到 notes 文档以便固化成长久的设计和方向，而非实际的代码与实施细节。
- **独立理由**：方案可以与旧实现或任何参照系殊途同归，但理由必须从需求独立推导成立；「沿用」「以前就这样」不是理由。
- **高起点**：设计以「成熟系统会怎么做」为目标（生产系统为主要参照，引用前先查证官方文档/源码，不凭印象），不以兼容、延续、仿照现有代码为目标。
- **外部契约先取证**：凡语义由硬件、ABI 或协议决定，编码和 review 前先从 `references/INDEX.md` 定位固定规范并引用具体章节；外部实现只作参照，`pre-ai` 只作缺陷样本，记忆不作证据。
- **优雅鲁棒优先**：结构正确性高于性能，可为优雅与鲁棒损失必要性能。
- **语义闭包先于实施**：设计从目标契约自顶向下追踪前置，直到当前承诺可由现有机制完整成立。发现缺失前置即暂停当前实现，先立案并同步修订 notes/plans/COMPASS、重排自然序，待前置收口后恢复；阶段性从简只缩小明确声明的能力范围，不削弱正确性——该范围内必须独立完整，未来扩展应是能力增量或实现替换，而不是补偿当前已知欠账。
- **范式纪律**：性能问题的答案是在现有范式内逐点优化，不是更换范式（例：微内核 IPC 慢的答案是优化各环节，而非改成宏内核）；范式级推翻只在范式被证明错误时发生。
- **机制重构优先于逐点修补**：解决问题先自顶向下审视全局，寻找机制层面的合并、转化或重构，让一次修复消灭一类问题并为后续演进铺路；在调用点逐个打补丁只会把系统缝成补丁衣。该视角属于计划阶段——设计时就带着它，审查只是回望时更容易看到改进点。
- **重构即收益**：没有存量用户，迁移与 ABI 破坏不是砝码——统一性收益是唯一衡量，破坏点如实汇报即可。halcyon 的意义正是借一次次重构脱开主流经验系统的惯性，长出自己的面貌。
- **先设计后编码**：核心模块（mm/task/ipc/fs）的实现必须先有成文设计并经确认。

## 已知问题

@KNOWN_ISSUES.md
