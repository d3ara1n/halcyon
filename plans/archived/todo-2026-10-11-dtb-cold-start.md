# DTB 冷启动目录修复

状态：已实施并完成针对性验证，随本次构建入口整理提交。基线为 `8a2a178`，分支 `refactor/system-redesign`；接手时工作树干净。

## 原因与范围

清理产物后，`just make_dtb` 可复现 `Couldn't open output file .../artifacts/qemu/virt/device.dtb: No such file or directory`，退出码 1。`just virt` 的启动依赖依次为 `make_dtb`、`make_boot_package`、`build_kernel`；原 `artifact_dir` 只创建根目录、build 和 initfs，平台目录要等后面的 Cargo 产物导出才创建。因此已有平台目录掩盖了遗漏；DTB 本身每次都会重新生成，并非持续读取旧 DTB。

`Justfile` 的 `artifact_dir` 使用统一的 `TARGET_DIR`、`MODEL_DIR` 和 `mkdir -p` 准备目录；`virt-hetero`、`virt-nofd` 在生成变体 DTS 前显式依赖目录准备。构建手册说明自动生成行为。范围限于产物目录前置，不涉及内核、ABI、所有权、权限或运行时生命周期；无未闭合责任与延期项。

## 验证与交付检查

- 在项目内临时目录下分别执行 virt、sifive_u 的真实 `just make_dtb`：此前不存在的输出目录自动创建，产物具有 FDT magic；用无效旧内容覆盖 DTB 后再执行，确认重新生成。临时目录已清理。日志：`artifacts/dtb-fix/generation.log`。
- `just --dry-run virt-hetero`、`just --dry-run virt-nofd` 确认目录准备先于变体生成；没有运行这两条完整 QEMU 路线。
- 在默认平台目录缺失、DTB 缺失的状态执行 `just virt`：完成用户态与内核构建、ELF 审计、core 全部验收锚点与显式 shutdown；验收脚本完成成功分支，未输出失败。构建及验收摘要日志：`artifacts/dtb-fix/virt.log`。启动进程及 QEMU/节流/验收进程均已退出。
- `git diff --check` 通过。结构检查核对所有已有 `make_dtb` 消费者共享目录前置、生成与加载路径一致，保留 `ERHINO_DTB` 的加载覆盖语义；删除重复目录判断，无新增过渡机制。
- 仅修改 Justfile 目录准备和文档，不改变 Rust 行为，因此没有另跑 `just check`、clippy 或全量平台验收。既有 DTS/linker 警告不属于本次目录修复。

当前修复无剩余实施步骤。用户已授权提交，并明确本次属于构建入口与输出整理，不涉及系统代码和机制修改，不登记独立 Review。
