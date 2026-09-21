# 服务进程

服务是普通进程通过发布协议 endpoint 承担的系统角色，不是内核特殊进程类型。调度、地址空间、HandleTable、Job 预算和退出语义与其他进程相同；差异只来自 launcher 交付的 capabilities 与服务主动发布的 endpoint。

## 启动与授权

launcher 在 Building 阶段以独立的 Map/Write、Grant 与 Attach 动作准备映像、用户态 StartupBlock、初始 capabilities 和线程现场，再由 ProcessStart 一次发布全部预育线程。常见组装方式是 launcher 创建 Mailbox、保留 sender，把唯一 owner 直接 GRANT 给 child；服务也可以启动后自行创建 endpoint。

普通服务的 payload 可采用用户态 LauncherParcel，按索引描述 args、配置、namespace routes 与 Handle 语义。内核不理解普通 StartupBlock outer、“服务邮箱”或任何业务 tag；只有 init bootstrap 保留内核构造的同形 outer。

成熟用户环境中 init 持 root Job 与平台根 capabilities，按最小权利启动资源管理、文件系统、驱动和其他服务。PID 创建关系不授予管理权；Process Controller capability 才能管理进程。

## 发现与调用

服务通过显式注册控制权向用户态目录发布 badged sender。一个原子 service record 同时包含 instance、protocol/version、endpoint 和记录代次，客户端一次读取就取得完整快照；不得逐字段拼接可能来自不同实例的值。

客户端取得 Handle 后直接调用。服务按内核提供的发送授权身份找到 grant/session，badge 是该授权的不可变标签，PID 只作 provenance。请求上下文拥有消息 Delivery 与 send-once 回复权，批量数据面交付 Tunnel Invitation；这些责任彼此独立。

注册控制权限定可管理的名称或子树，与普通目录读写权分离。服务目录后端只通过注册状态机发布 Ready record，不能被普通 PropertyWrite 绕过。endpoint 必须有足够的查询、等待、复制和运输权，且已经符合发布者的出口政策；目录不能自行收窄一个未知业务协议的 badge 权限。

boot-critical 依赖在 Building 阶段经直接 grant 写入 StartupBlock；动态依赖可通过 FAL 服务目录发现。首个目录提供者由 init 的显式启动拓扑打破引导环，不需要 PID Send 后门。

## 生命周期

服务目录状态应显式为：

```text
Absent -> Starting -> Ready(instance, protocol, endpoint) -> Draining -> Absent
```

只有在 Ready 状态取得的记录快照可供发现。Starting 有建立期限；Draining 不再交付新的 endpoint。服务退出关闭 owner，客户端观察 CLOSED；endpoint 关闭或注册控制权的 Lifetime 终止都使目录撤销对应记录。

撤销和延迟清理携带 instance/记录代次条件，旧实例的完成不能删除替代者。一次已经取得的 Ready 快照可能随后失效，发现不是可用性保证。客户端可重新发现，但是否重试由业务幂等语义决定。

撤销名称只阻止新发现，不追溯销毁已授 capability。政策撤销阻止特定授权的新操作准入；已准入请求和独立建立的连接按自身契约收束，不隐含跨服务递归撤销。capability 转交后的寿命由真实引用及交付责任决定，不依赖原进程保活或周期续租。

服务记录 schema 与注册控制属于 libservice，通用 Record、能力值和目录投影属于 FAL。首个承载者由启动拓扑指定，不因此成为所有进程必须经过的全局注册权威。

## 监督与接管

系统配置必须把启动项声明为必选或可选。必选服务的映像缺失、构造失败或授权失败使整组启动失败，不能进入看似正常的服务拓扑；可选项失败则形成显式 Degraded 记录。launcher 只有在必选集合完整发布后才能宣布 stage Ready。

监督 authority 在目标完成 Drain 且终态快照核验成功前不得关闭。每次等待、Drain、Query 与枚举停滞都受 policy 的 deadline、工作预算和重试次数约束；预算耗尽返回当前阶段、已完成工作与仍持 control 的 owner。局部监督者可以重试或把该 owner 连同进度交给上级；若目标仍在 Job 成员表内，上级也可用保留的 JobControl 重新派生 control 并执行整域收束。

委托管理不消除根接管权。子域管理者持一份域内 JobControl，root supervisor 保留另一份独立 control；前者失败时停止关闭残留 authority并发布 handoff/unmanaged 诊断，后者按自己的 policy 接管。内核只发布 REAPABLE/CLOSED 与有界 Drain，不实现重启、deadline 或递归 JobKill。
