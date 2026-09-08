# 系统审计批次 E-2：syscall/shared ABI、IPC/FAL/服务与工程化

## 2026-09-08 提交后复核（E-2，新增 P2，仍开放）

固定对象 `9ee2791d3e18fdb7857fe41c74bacc7bb0c7c774`。OliveWillow 独立只读复核；统筹者运行平台路线发现 E2-7-02，OliveWillow 再以目标代码交叉确认。

### E2-7-02 / P2：nofd 验收锚点与可选服务降级输出漂移

- 位置：`tools/qemu-acceptance.sh:63` 的 nofd profile 仍要求 `failed to start bin/test_fp: SpawnFailure { error: System(NotSupported), grants: Retained, cleanup_error: None }`；`user/services/srv_init/src/main.rs:339` 起实际输出为 `optional service bin/test_fp degraded: SpawnFailure...`。
- 复现：固定提交执行 `THROTTLE=100 just virt-nofd`，recipe 退出 1。guest 正常拒绝 D64、运行 Base64 core 并以 Requested reset 收束，wrapper 仅因缺旧锚点拒绝；完整日志 `artifacts/failed-acceptance-20260908-090925-47740.log`，第 125 行为实际降级输出；路线汇总见 `artifacts/review-9ee2791/virt-nofd.log`。
- 影响：无 F/D 的正式验收路线出现确定性假失败；不能以 guest reset 成功或其它路线通过宣告 nofd 通过。这是监督输出变更未同步消费者的工程回归。
- 修复与完成门：对齐稳定语义锚点，保留 Base64 domain、明确 D64/NotSupported 拒绝、grants/cleanup 与 reset 检查，不以删除锚点放松验收。新固定提交重跑 nofd，并验证未拒绝 D64 或未收束时仍不能通过。后续行动只在本报告，不新增重复 todo。

### 原 findings

| Finding | 结论与证据 |
|---|---|
| E2-5-01 / RPC reject | 闭合。`user/frameworks/librpc/src/caller.rs:80` reject_reply 逐项 close Handle 并 discard port；`:153` 起服务关闭、等待/接收错误、timeout 与 framing reject 隔离端口。`srv_init/src/main.rs:787` 双调用测试检查旧 Handle stale 和新端口成功。framing host 测试通过。 |
| E2-7-01 / lint 门 | 闭合。`Justfile:72` 七面 `-D warnings`，`:209` acceptance 先执行 Clippy；统筹者本轮实际七面全部通过，完整日志在 `artifacts/lint/`。 |

本轮运行证据：默认 `just acceptance` 在 50% stress 15/16 退出 124，单独 50% stress 同样失败；两次均为已登记 last-thread-exit-vs-kill 覆盖 flake且已走 failure shutdown，日志分别为 `artifacts/failed-acceptance-20260908-083623-32687.log`、`artifacts/failed-acceptance-20260908-083953-36586.log`。全速 stress 16/16、release、sifive_u、hetero 通过；nofd 按上述原因失败。不得把拆分成功路线写成单次 acceptance 聚合通过。

E2-7-02 尚开放，本报告保留根目录；首审两项关闭不代表全报告可归档。

---

以下为历史首审与此前 lint 实施记录，旧“已闭合”仅针对所标原 finding。


> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](todo-2026-09-review-program.md) 为准；RPC reject 归 capability/owner 计划，E2-7-01 lint 门由本报告保留复核证据，当前实现已闭合。

## 范围、基线与证据边界

审计分片 5（syscall/shared ABI）、分片 6（IPC/FAL/服务）和分片 7（工程化与全仓收口）。代码基线：`e5db4f32a507ca5bc26849b53e64c0a3b73fa82d`（`e5db4f3`）。工作树仅有预先存在的 plans 文档修改；本审计未修改文件、未提交代码。目标为系统审计首审，未经过独立 reviewer 核验；本报告 findings 状态为待核验/待修复。

审计阅读了 `plans/todo-2026-08-system-audit.md`、`plans/REVIEW.md`、`references/CONTRACTS.md`，以及相关 `notes/ideas/`、`notes/impls/`。A–D 已知 findings 只作交叉确认，不重复编号。

## 执行命令与结果

```text
cd shared && cargo test --target aarch64-apple-darwin
cd os && cargo test -p handle_table -p wait_context -p timer_queue -p remote_call -p memory_space --target aarch64-apple-darwin
cd user && cargo test -p librpc -p libfal -p libfs -p librunnel --target aarch64-apple-darwin
just check
just virt
just virt-release
just sifive_u
just virt-stress
THROTTLE=100 just virt-stress
```

结果：shared 18/18；handle_table 17、memory_space 19、remote_call 5、timer_queue 6、wait_context 8；librpc 5、libfal 39、libfs 17、librunnel 8；`just check`、`just virt`、`just virt-release`、`just sifive_u`通过。默认 `virt-stress` 以已知 `last-thread-exit-vs-kill` 场景 15/16 后退出 124，全速 `THROTTLE=100` 同一 workload 16/16 通过；按 `KNOWN_ISSUES.md` 不新增内核 finding。sifive_u 的 `SystemReset: NotSupported` 是已知平台终态，随后进入 steady-state supervisor 并由 recipe 收割。

另运行 `python3 -m py_compile tools/make-boot-package.py tools/audit-user-elf.py` 及代表性 clippy。全域 `clippy --all-targets -- -D warnings` 未通过，见 E2-7-01。未运行 `just acceptance` 聚合、`virt-hetero`、`virt-nofd`、OOM/恶意输入/跨进程 FAL/MemoryObject/RNL2 专项注入。

## 分片 5：syscall 与 shared ABI

调用号、a7/a0–a5 约定、未知 syscall 错误返回、shared/kernel/rinlib 调用链、固定宽布局和 reserved 字段、Handle generation/rights/badge、TRANSIT/GRANT、uaccess 逐页 U/R/W、SUM guard、WaitMany 参数和 signal/timeout/closed wire 形状均得到代码与 host 测试支持。Mailbox/Notification/Tunnel 输出写回的 rollback 结构正常路径成立。

分片 5 没有发现新的 P0/P1。A–D 已知 findings 仍限制最终收口：WritePermit rollback、object owner retire、post-Commit retire capacity、EXECUTE rights、DT/FramePool/Drop、bootstrap post-commit、supervisor/IPI/RemoteCall/epoch/UserStack 等均只作交叉证据。Seal/RX/EXECUTE rights、跨进程 MemoryObject 和 OOM/fault 注入仍未验证。

**结论：正常 ABI 基座有条件通过首审，不能最终收口。**

## 分片 6：IPC、FAL 与服务

HandleTable 的 generation/rights/role/badge、Mailbox send/receive rollback、Notification OR/take、Invitation attach/close、Endpoint/Connection/lease retire、WaitMany level signal、Runnel/RPC/FAL framing 和 `srv_fs` mailbox pump 均有 host/QEMU 正常路径证据。当前外部 Tunnel/Runnel 仍为单页 RNL1/u32；RNL2、多页 Tunnel 和动态 ring 已登记在 `todo-2026-09-memory-object-data-plane.md` 切片 8/9，不把当前实现误判为最终数据面。

`srv_pm` 的 delegated JobControl 权限边界成立；`srv_init` 的 SystemReset capability 和 sifive_u steady-state 行为成立。但 C-1 的必选服务静默降级、监督失败丢 control 仍存在，本报告不重复编号。跨进程 FAL、跨进程 MemoryObject/Seal/RX、多页数据面和 malformed RPC response 均未专项验证。

### E2-5-01 / P1：librpc 拒绝响应时不关闭已接收 Handle，也不废弃 ReplyPort

位置：`user/frameworks/librpc/src/caller.rs:149-176`。

可达前提：服务已获得合法 reply send-once，并在 response 中携带额外可转移 Handle，或发送错误 version/kind/txid/framing；客户端收到后进入错误分支。ServiceClosed 与 reply/迟到响应竞态也可触发。

直接证据：ServiceClosed 直接返回 `CallError::ServiceClosed`，未调用 `discard_port()`；header kind、`RpcPrefix::decode`、response kind、txid mismatch 等错误分支直接返回；只有 framing 全部通过才把 handles 移入 Reply。`ReceivedMessage` 中的 Handle 数值 Drop 不会关闭已经安装在 caller HandleTable 中的 capability；`ensure_port` 后续会复用旧 port。

违反契约：`notes/ideas/rpc.md` 要求 timeout/迟到回复隔离 ReplyPort；`notes/ideas/object.md` 要求跨表 Handle 的生命周期由显式 close/discard 收束。

影响：被拒绝 response 携带的 Handle 可能长期泄漏，反复触发可耗尽 HandleTable；未废弃的 port 可让迟到 response 污染下一 call 的 FIFO，并继续触发 txid 错误和 Handle 泄漏。现有 srv_fs 回复不带 Handle，QEMU 正常路径未触发该分支。

建议：统一 `reject_reply(message, reason)`，先逐项 close received handles，再 discard 当前 ReplyPort；ServiceClosed、所有 framing/kind/txid 错误和 Receive 后异常都走该 helper。补坏 version/txid 携带 1–8 Handle、ServiceClosed 并发、timeout 后迟到 response 测试，断言 HandleTable 不增长且下一 call 使用新 port。

**分片 6 结论：正常 IPC/FAL/Runnel 闭包有较强证据，但新增 E2-5-01 为 P1，且跨进程/多页/MemoryObject 专项未收口。**

## 分片 7：工程化与全仓收口

根 toolchain、独立 workspace、target/build-std、Justfile、user/kernel ELF audit、virt debug/release、sifive_u wrapper、节流/日志/超时收束均按已运行路线工作。默认 stress 的 15/16 是已知有限轮次 flake，全速 16/16 通过；不能把节流失败当内核回归。

已登记的 RNL1→RNL2、多页 Tunnel、raw `alloc_user_order` selftest adapter 属计划中的未来能力/过渡项，不是未登记残留。当前代码不再出现隐式系统 shutdown 主路径。

### E2-7-01 / P2：全域 clippy -D warnings 未闭合，Justfile 无静态 lint 门（已闭合）

首审证据：`cargo clippy --all-targets --target aarch64-apple-darwin -- -D warnings` 在 shared、dtb/memory_space、librunnel/libfal 等 crate 失败，涉及 `too_many_arguments`、`new_without_default`、`manual_is_multiple_of`、`needless_lifetimes`、`dead_code` 等；`just check` 仅编译，不运行 clippy，不能推出 lint-clean。

影响：不是已证实运行时内核缺陷，但工程门无法持续发现 lint debt、真实 dead_code 和接口复杂度问题；首审时不能写“clippy clean”。

复核证据：`just clippy` 现以 `-D warnings` 顺序覆盖 shared host、排除内核的 os host、RISC-V kernel、仅库的 user host、Base64 user bins、`srv_init` stress feature，以及独立 `riscv64gc` `test_fp` 七个编译面，完整输出分别保存在 `artifacts/lint/`。接口、迭代器、算术与测试 lint 已逐项修复；必须保持 affine owner 完整错误返回或一次提交全部发布维度的接口，仅使用带结构理由的局部 `#[expect]`，未增加 crate 级 blanket allow。`just acceptance` 在三条 QEMU 路线前先运行该静态门，后续改动不能绕开。

**分片 7 复核：E2-7-01 已闭合；构建、静态 lint 与 QEMU 聚合现由同一阶段收口入口持续执行。**

## 已证实闭包

1. syscall 调用号、错误码投影、shared 固定宽结构和 reserved/size/alignment 断言在 shared/kernel/rinlib 纵向一致。
2. uaccess 逐页 U/R/W、SUM guard、同锁 check+copy 和 WaitMany 写回错误边界有实现证据。
3. Handle generation 退休、rights 子集、TRANSIT/GRANT、badge、Mailbox rollback、Notification OR/take 和 TimeoutRegistration 有代码与 host 测试支持。
4. Tunnel/Endpoint/Invitation/Connection 正常 authority/lease/close/peer signal 路径通过 virt core/release 和全速 stress；sifive_u 明确 reset NotSupported 后保持 supervisor。
5. FAL/RPC/FS framing 和 path/symlink host tests 通过；srv_fs mailbox pump 与 srv_pm/init 负载已运行。
6. Justfile target/build-std、ELF audit 和已运行 QEMU wrapper 路线按设计工作。

## 与 A–D findings 去重

本报告不重新编号 A 的 MemoryObject/WritePermit/retire、B-1 的 DT/FramePool/Drop/SystemSupply、B-2 的 bootstrap post-commit、C-1 的 supervisor/IPI/DT/ThreadControl/timeout、C-2 的 RemoteCalls/epoch/UserStack、D-1 的 WaitContext/deadline/MappingLease、D-2 的 launcher/payload/ELF/token findings。E-2 只记录 E2-5-01 和 E2-7-01 两项新增 findings，并说明已有问题对 ABI、服务和工程化总闭包的影响。

## 验证缺口与后续归属

- 未运行 `virt-hetero`/`virt-nofd`；D64/无 F/D 路线未现场证明。
- 未做恶意 RPC response 携带 Handle、Caller timeout/ServiceClosed/txid mismatch 后 port 重建、跨进程 FAL、跨进程 MemoryObject Seal/EXECUTE/RX、多页 Tunnel/RNL2、OOM/地址 fault 注入。
- 当前 RNL1/单页/u32、raw selftest adapter 继续由 `todo-2026-09-memory-object-data-plane.md` 承接。
- E2-5-01 已由 capability/owner 计划闭合，E2-7-01 lint 门也已按本报告条件复核闭合；本报告保留目标提交的首审事实，等待固定修复提交后的整批 Review。实现事实同步 `notes/impls/rpc.md`、`ipc.md`、`internals.md`，方向契约同步 `notes/ideas/rpc.md`。
- 所有 findings 修复并复核后，本报告才移入 `plans/archived/`。

## 最终判定

**E-2 分片 5–7 首审不通过最终收口。** 首审新增 1 项 P1（librpc capability/ReplyPort 隔离）和 1 项 P2（clippy/工程 lint 门）；二者当前实现与验证均已闭合，但该历史判定只在形成固定修复提交并完成整批 Review 后更新为最终复核结论。