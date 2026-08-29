# 服务进程

服务是普通进程通过发布协议 endpoint 承担的系统角色，不是内核特殊进程类型。调度、地址空间、HandleTable、Job 预算和退出语义与其他进程相同；差异只来自 launcher 交付的 capabilities 与服务主动发布的 endpoint。

## 启动与授权

launcher 在 Building 阶段以独立的 Map/Write、Grant 与 Attach 动作准备映像、用户态 StartupBlock、初始 capabilities 和线程现场，再由 ProcessStart 一次发布全部预育线程。常见组装方式是 launcher 创建 Mailbox、保留 sender，把唯一 owner 直接 GRANT 给 child；服务也可以启动后自行创建 endpoint。

普通服务的 payload 可采用用户态 LauncherParcel，按索引描述 args、配置、namespace routes 与 Handle 语义。内核不理解普通 StartupBlock outer、“服务邮箱”或任何业务 tag；只有 init bootstrap 保留内核构造的同形 outer。

成熟用户环境中 init 持 root Job 与平台根 capabilities，按最小权利启动资源管理、文件系统、驱动和其他服务。PID 创建关系不授予管理权；Process Controller capability 才能管理进程。

## 发现与调用

服务可以向用户态目录发布 badged Mailbox sender 与原子 service record。record 至少把 instance、protocol/version 和 endpoint 作为一个一致快照。客户端取得 Handle 后直接 Send；badge 让同一服务队列区分 grant/session，PID 只用于审计。

请求回复时，客户端 TRANSIT send-once；批量数据面 TRANSIT Tunnel invitation。服务按 sender_badge 查 GrantState，并把 sender_pid 仅作为 provenance 或额外身份政策输入。内核只保证 capability 不可伪造、rights 不放大和事务原子。

boot-critical 依赖在 Building 阶段经直接 grant 写入 StartupBlock；动态依赖可通过 FAL 服务目录发现。首个目录提供者由 init 的显式启动拓扑打破引导环，不需要 PID Send 后门。

## 生命周期

服务目录状态应显式为：

```text
Absent -> Starting -> Ready(instance, protocol, endpoint) -> Draining -> Absent
```

只有 Ready record 可供新客户端发现。服务退出关闭 owner，现有 sender 观察 `CLOSED`；目录清理旧 instance 后才发布替代者。客户端可在 CLOSED 后重新发现，但是否重试取决于协议幂等语义。

撤销名称只阻止新发现，不追溯销毁已授 capability。需要主动撤销时，服务删除 badge 对应 GrantState、使用 lease/session 或代理层；不以 PID 重用模拟撤销。
