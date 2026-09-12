# FAL 服务能力与正式流接入

> 待设计/实施计划。方向由 `notes/ideas/{fal,fs,service}.md` 拥有；当前实现见 `notes/impls/fal.md`。本文件是 DirectoryGrant、跨 provider 路由、服务发现、每订阅者 Watch 与正式 Open 剩余能力的唯一实施入口，不把它们隐含在 RNL2 施工中。

## 当前事实与触发

`libfal` 已有协议编解码、memfs 与基础 provider 分发，`libfs` 已有前缀表/路径组合；当前 `srv_fs` 以临时 directory anchor 运行，尚无 DirectoryGrant badge 与 FAL rights ceiling。Open 返回 Unsupported，Stream 仅支持 memfs 偏移读写，没有 Tunnel/Runnel Open 调用链。

多页 Tunnel/RNL2 完成后进入本专题，先闭合服务 authority 与生命周期，再交付正式流。单调时间/全调用 RPC deadline 是正交前置：若本专题开放有限超时，先满足 [`deadline 计划`](todo-2026-09-monotonic-time-rpc-deadline.md)，不能把当前相对回复等待称为完整调用期限。

## 目标与设计义务

1. DirectoryGrant 以正式 badged sender 绑定根、rights ceiling、派生与生命周期；客户端路径不携带 authority，provider 重验全部请求。
2. 真实跨进程 provider 与 Delegate 路径；显式 capability 交付和前缀表组装，不建立中央 VFS 或隐藏全局注册表。
3. 原子 service record 与 endpoint capability 的发布/发现；取消、失效和服务退出行为闭合。
4. Open 的 wire 请求/响应、方向、错误、最终状态与资源归属完整冻结。provider 创建 Tunnel，reply 搬运 Invitation，客户端 Attach；响应投递失败、客户端不 Attach、半开、双方退出必须都有同一套 owner 清理。
5. Provider 长流任务留在用户态；流暂停/背压时不能无意堵住提供者的全部控制请求。并发执行结构、连接上界、每连接内存来源与收束政策在实施前决定。
6. 每订阅者 Watch 使用独立 Notification，订阅/取消、provider 退出和信号聚合按既定方向闭合。

本计划不预先决定尚无取证的服务拓扑、worker 形态或 Open 线格式；实施前按当前 codebase 完成全套方案，再采用可论证的推荐设计。Move/Copy、Handle[T] 等同属 FAL 后续面的能力在本专题细化时明确能力范围与唯一承接，不默认因 Open 完成而完成。

## 自然顺序

DirectoryGrant/授权与生命周期 → 跨 provider 路由/显式服务 capability → 服务发现与 Open 完整协议及执行结构 → RNL2 接线与真实服务消费者 → Watch 及本专题其余已冻结能力。

这一顺序允许内部依赖施工，不能留下临时 anchor 与正式 grant 的长期双轨。实际设计若证明某项独立，修订本计划的依赖后再施工。

## 完成与删除门

- shared 保持内核/用户 ABI 边界，FAL wire 留在 `user/frameworks/libfal`。
- 删除临时 directory anchor、无鉴权 provider 路径与被替代的 Unsupported 分支；范围之外的能力仍如实声明。
- `srv_fs` 与真实跨进程客户端通过 Open 取得多页 RNL2 流，至少一条流传输超过单页并实际跨环，验证完整字节、EOF、背压、最终状态与关闭。
- Open 的发送失败、Attach 失败/未发生、客户端退出和 provider 退出均有资源守恒与最终收束证据；不能拿 init↔pm 的机制验收替代。
- DirectoryGrant 越权/根逃逸、Delegate 跳转、服务记录一致性与订阅终态有端到端证据。
- 全部承诺能力/真实消费者/失败路径/旧机制删除完成后做组合验证，更新 notes/impls 与 COMPASS；提交后登记未来 Review。
