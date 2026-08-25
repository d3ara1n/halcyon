# 服务进程

服务是普通进程通过发布协议端点承担的一种系统角色，不是内核特殊进程类型。调度、地址空间、Handle 表和退出语义与其他进程相同；差异来自启动授权方交付的资源以及服务主动发布的邮箱 sender Handle。

## 启动与授权

`ProcessCreate`/`ProcessStart` 是建立进程与其初始资源的统一事务。新进程在 runnable 前获得可枚举的 typed/tagged startup resources，服务像读取参数一样主动发现所需 grants；Mailbox receiver 是可选资源，不占固定入口寄存器或固定 Handle 数值。获得启动邮箱的服务再从版本化 `STARTUP` 消息或后续授权消息取得动态资源，不凭 PID 或全局对象名取得权限。

成熟用户环境中 init 读取 initfs 配置，创建服务并按最小权限原则交付设备、内存、服务依赖和管理 Handle。init 尚未接管时，内核启动装载者只通过同一内部 launch primitive 暂代根授权方：sender 为零，按集成配置给 `srv_init` grants。未来把策略整体移入 init 后，删除内核策略，不改变 startup-resource、消息或 Handle move 契约。

## 发现与调用

服务向目录服务登记可复制或可转移的邮箱 sender Handle 及协议标识。客户端查询名称后，目录在鉴权下返回裁剪 rights 的 Handle；客户端随后以该 Handle 发送请求。PID 只服务审计、父子关系和进程管理，不是服务地址。

请求需要回复时，客户端随消息转入回复邮箱 sender Handle；需要批量数据面时，任一方转入隧道 peer invitation。服务协议解释 kind、验证 sender 身份并决定是否接受附带 Handle；内核只保证 sender 不可伪造、rights 不放大和 move 原子。

## 生命周期

服务退出会关闭邮箱 receiver-owner、drain Handle 表，并使客户端现有 sender Handle 观察到 `CLOSED`。目录撤销名称只阻止新的发现，不改变既有 Handle；主动撤销需要服务对象或目录协议的间接层，而非 PID 重用或猜测对象存活。
