# 流运输与 Runnel 闭包未来代码 Review

> 【未来审查计划】固定对象为 `a2aabed`（`feat(transport): 流运输与 Runnel 闭包——typed 角色收口与观察草稿面删除`），父提交 `a6ae767`。提交后生成，只安排已完成代码的独立只读复核，不重开二轮拆分裁决（观察/登记/取消并入通用执行闭包），不阻塞通用执行闭包设计。

## 提交范围

- librunnel 删除无消费者观察草稿面：`Producer/Consumer::{register, peer_attached, prepare_wait, all_consumed}`（随基线 `d22b9d7` 入库）。register/peer_attached 在运行期 fail 关闭 Endpoint 后绕过 `Channel::check` 直接经 `Guest::endpoint()` 访问映射、panic 于 `closed channel accessed its mapping` 的缺陷随之消失。
- librunnel 删除原始 ABI 工厂 `create_producer/create_consumer/attach_producer/attach_consumer`（unsafe raw Handle 形态）及其 `rinlib::ipc::object::close` 清理分支；typed `Producer/Consumer::create/attach` 成为唯一构造入口，`CreateFailure::Protocol` 双 owner（Endpoint + Invitation）与 `AttachFailure::{Unconsumed, Consumed}` 不变。
- srv_init 数据面创建侧迁移至 `blocking::Consumer::create`：typed `Invitation` 直接 `into_capability()` 进 `Packet::push`，消除 `Capability::from_raw(invitation)` 接缝及其 SAFETY 约定；`CreateFailure::Protocol` 失败分支显式关闭本地映射与未发布邀请双 owner 并记录，Drop 兜底不变。
- 计划与文档：执行前置计划补二轮拆分裁决与流闭包实施记录；`notes/impls/runnel.md` 的 owner/当前施工节改为最终形态；COMPASS 状态同步。
- 内核与 shared 零改动；pm 接收侧已 typed 不变；srv_init 自检/race 与 test_hammer 的 rinlib 原始 tunnel 调用属刻意内核契约验证，保留为 raw。

## 复核清单

1. 观察面删除完整性：全仓（librunnel 外）无 `register/peer_attached/prepare_wait/all_consumed` 残留调用；删除后 librunnel 无任何绕过 `Channel::check` 的终态后 endpoint 访问路径（`wait_peer_closed` 有 `check()` 前置；`Transport` 方法仅经先 check 的 Channel 路径触达）。
2. raw 工厂删除完整性：无 `create_producer/create_consumer/attach_producer/attach_consumer` 残留；librunnel 对 rinlib 的依赖只剩 typed `tunnel::Endpoint` 与 `invitation::Invitation`，无 `unsafe` 直调残留。
3. srv_init 创建侧：成功路径 typed Invitation 单次移交（`into_capability` 后无再使用权）；`CreateFailure::Protocol` 分支 endpoint 与 invitation 均显式 close，close 失败仅记录不吞 owner（`(Self, error)` 返回形态下 Drop 兜底恰一次）；push/try_send 失败路径的 owner 返还沿用消息闭包形态，无双重关闭。
4. 阻塞路径行为不变：`write_all/read_exact_or_eof` 的 acknowledge→重查→wait 私有协议与 host 断言未改动；删除仅触达公开 API 面，`ProducerCore/ConsumerCore` 与 host 测试零改动。
5. 无计划外 API 形态变化：本提交不新增任何公开方法（纯删除 + 消费者迁移），typed 构造签名与消息闭包定稿的 owner 风格一致。
6. 文档一致性：`notes/impls/runnel.md` 与计划实施记录描述的是最终形态而非变更过程；观察面归属通用执行闭包的表述在 COMPASS、计划、impls 三处一致。

## 已有验证证据

- 用户态框架 host 95 项（librunnel 15、rinlib 7、librpc 6、libfal 42、libfs 17、libprocess 3、libsrv 5）全部通过。
- 七面 `just clippy` 全部通过。
- `just virt` core 与 `just virt-release` 全绿：RNL2 数据面锚（`bytes=65536, capacity=12160`）、typed Invitation 转移、peer closed 观察与显式 reset 锚点完整。
- sifive_u/virt-nofd/stress 属四闭包组合完成门，未在本提交运行。

## 边界与完成门

本计划仅拥有 `a2aabed` 的固定提交复核；通用执行与 RPC 闭包（含 Runnel 登记接入面与三条件观察结果的形态定稿）由[执行前置计划](todo-2026-09-13-service-runtime-prerequisites.md)继续拥有，不在本 Review 内预审。未来 reviewer 使用新上下文只读审查目标提交，核对清单各点，记录有证据的 finding 或确认无 finding；无 finding 后归档本计划，有 finding 由一份 review 报告承载修复与复核。
