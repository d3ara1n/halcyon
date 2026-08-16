# Halcyon / eRhino

Rust 编写的 RV64 教学操作系统。仓库叫 halcyon，操作系统叫 eRhino，内核二进制是 `erhino_kernel`，用户态标准库替代品是 `rinlib`——四个名字指同一个项目。

## 仓库结构

三个独立 cargo workspace + 一个硬依赖子模块，靠 path 依赖串联。拆成三个是因为 os（`riscv64gc-unknown-none-elf`）与 user（自定义 JSON target + build-std）运行时不同：cargo 不支持嵌套 workspace，一次构建也只有一个 target，无法合入单一 workspace。

```
os/        内核 workspace（erhino_kernel，no_std）
  └─ platforms/  链接脚本 linker.ld、各平台 dts（qemu/virt、qemu/sifive_u）
user/      用户态 workspace（rinlib、systems/、frameworks/、drivers/）
shared/    erhino_shared：内核与用户态共享的 ABI（syscall、消息格式、FAL 接口、同步原语）
submodules/dtb_parser  内核硬依赖，克隆后必须 git submodule update --init
notes/     设计文档，是架构意图的事实来源（改动涉及 IPC/FAL/任务模型时先读对应篇）
plans/     待办计划；2026-07-code-review.md 有未完成的 P0/P1/P2 事项
```

**shared 是内核与用户态的 ABI 边界**：改动它的数据结构/消息格式通常两侧都要同步改，不能只改一边。

## 构建与验证

- 构建系统是 [Just](https://just.systems)，**统一走 `just`，不要裸跑 cargo build**——内核的链接脚本和链接器（`riscv64-elf-ld`）靠 Justfile 注入 RUSTFLAGS；用户态靠自定义 target（`rinlib/riscv64-unknown-erhino-elf.json` + build-std）。
- 快速验证（几秒级）：
  ```sh
  cd os && cargo check      # 内核，无需 RUSTFLAGS
  cd shared && cargo check
  ```
- 完整验证：`just virt`（4 核）/ `just sifive_u`（5 核，#0 禁用）。QEMU 不会自行退出，在 agent 环境里必须带超时跑、看启动日志判断，禁止起了解耦不管。
- 开发机是 macOS：`just dtc qemu riscv64-elf-binutils riscv64-elf-gdb` 来自 Homebrew；打 tar 包时注意 bsdtar 的 `._` AppleDouble 文件会污染 initfs（历史上因此 panic 过）。
- Rust nightly（`rust-toolchain` 钉住），edition 2024。

## 已知问题

@KNOWN_ISSUES.md

## 约定

- 文档、注释、提交信息都用中文。
- `git tag pre-ai`（975c46f）之前的提交全部为人工编写，不含 AI 参与；之后的提交如由 AI 辅助，加 trailer 标记，例如：
  ```
  Co-Authored-By: Claude <noreply@anthropic.com>
  ```
- 设计取舍记录在 notes/，不要在代码里留「原来是 A 改成 B」式的历史注释，追溯看 git log。
