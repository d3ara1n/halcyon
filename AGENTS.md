# Halcyon / eRhino

个人兴趣项目，目标是成熟、可长期演进的 RV64 微内核系统。仓库叫 halcyon，操作系统叫 eRhino，内核二进制是 `erhino_kernel`，用户态标准库替代品是 `rinlib`——四个名字指同一个项目。`git tag pre-ai`（975c46f）之前为第一版手写实现；之后由 AI 协作重写，以成熟系统为标准（生产系统为主要参照，不做教学式简化）。

## 心智模型

以下事实决定每个改动的归属判断：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务（fs/pm 等系统服务），内核只做短路径转发；内核路径恒短，内核态不可打断（协作式）是推论不是选项。若某需求看起来必须内核抢占或内核线程，说明工作被放错了地方，修架构方向而不是中断模型。
- **shared/ 是内核与用户态的 ABI 边界**：改动它的数据结构/消息格式/调用号，内核与 rinlib 两侧同步改，不留单边。ABI 不冻结，随设计演进。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求从简；将来换复杂实现不动结构（调度域/类即范例：类可整体替换、域可横向扩展）。
- **notes/ 是设计事实来源**：架构意图、机制契约、取舍记录都在 notes/；改动涉及 IPC/FAL/任务模型时先读对应篇。方向性结论必须入档（聊天即焚，未入档的决策视为未发生）。

## 仓库结构

三个独立 cargo workspace，靠 path 依赖串联。拆成三个是因为 os（`riscv64gc-unknown-none-elf`）与 user（自定义 JSON target + build-std）运行时不同：cargo 不支持嵌套 workspace，一次构建也只有一个 target，无法合入单一 workspace。

```
os/        内核 workspace：
             kernel/          erhino_kernel（no_std）
             dtb/ frame_pool/ page_table/ tar/ elf/   纯逻辑 crate，host 可测
shared/    erhino_shared：内核与用户态共享的 ABI（syscall、消息格式、FAL 接口、同步原语）
user/      用户态 workspace（rinlib、systems/、frameworks/、drivers/）
notes/     设计文档（架构事实来源）
plans/     compass.md（跨会话导航：方向/位置/戒律）+ 考古与教训档案（按需参考，非任务）
```

对照负载：`user/systems/` + `user/drivers/` 的四个服务（fs/init/pm/drv_spi_sifive）是集成验证负载，其中 fs 依赖 FAL——fs「干净被杀」（用户态 panic → 退出回收，内核不崩）是其依赖面就绪前的达标线。

## 构建与验证

- 构建系统是 [Just](https://just.systems)，**统一走 `just`，不裸跑 `cargo build`**——内核的链接脚本和链接器（`riscv64-elf-ld`）靠 Justfile 注入 RUSTFLAGS；用户态靠自定义 target（`rinlib/riscv64-unknown-erhino-elf.json` + build-std）。
- 秒级检查：`cd os && cargo check`；`cd shared && cargo check`。
- host 单测（纯逻辑 crate，毫秒级）：**必须显式指 host target**——os workspace 默认 target 是 riscv，`cargo test` 直接跑会拿 no_std 环境去链 std：
  ```sh
  cd os && cargo test -p tar -p elf -p page_table -p frame_pool -p dtb --target aarch64-apple-darwin
  ```
- 集成验证：`just virt`（4 核）/ `just sifive_u`（5 核，#0 禁用）。对照负载跑完后系统静默自停机（SBI SRST），QEMU 退出即通过；仍需防挂起与异常 panic——agent 环境带超时跑，判定看启动日志关键行而非仅退出码。
- 开发机是 macOS：`just dtc qemu riscv64-elf-binutils riscv64-elf-gdb` 来自 Homebrew。打 tar 包时注意 bsdtar 的 `._` AppleDouble 文件会污染 initfs（历史上因此 panic 过）。
- Rust nightly（`rust-toolchain` 钉住），edition 2024。

## 约定

- 文档、注释、提交信息都用中文。格式上使用 Conventional Commits，以前的提交未使用标准格式，不当作参考。
- `git tag pre-ai` 之前的提交全部为人工编写，不含 AI 参与；之后的提交如由 AI 辅助，加 trailer 标记，例如：
  ```
  Co-Authored-By: Claude <noreply@anthropic.com>
  ```
- 设计取舍记录在 notes/，不要在代码里留「原来是 A 改成 B」式的历史注释，追溯看 git log。

## 设计原则

全周期适用的章法：

- **决策即讨论**：架构级决策点必须带选项与理由交用户拍板，不经确认不自行实施；语义模糊或 notes/plans 与目标冲突时，先问后动。
- **决策即文档**：讨论的结果和决策可以得出的宽泛的设计需要进入到 notes 文档以便固化成长久的设计和方向，而非实际的代码与实施细节。
- **独立理由**：方案可以与旧实现或任何参照系殊途同归，但理由必须从需求独立推导成立；「沿用」「以前就这样」不是理由。
- **高起点**：设计以「成熟系统会怎么做」为目标（生产系统为主要参照，引用前先查证官方文档/源码，不凭印象），不以兼容、延续、仿照现有代码为目标。
- **优雅鲁棒优先**：结构正确性高于性能，可为优雅与鲁棒损失必要性能。
- **范式纪律**：性能问题的答案是在现有范式内逐点优化，不是更换范式（例：微内核 IPC 慢的答案是优化各环节，而非改成宏内核）；范式级推翻只在范式被证明错误时发生。
- **先设计后编码**：核心模块（mm/task/ipc/fs）的实现必须先有成文设计并经确认。

## 已知问题

@KNOWN_ISSUES.md
