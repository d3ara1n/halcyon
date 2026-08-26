# 服务进程

服务是普通进程通过发布协议端点承担的一种系统角色，不是内核特殊进程类型。调度、地址空间、Handle 表和退出语义与其他进程相同；差异来自启动授权方交付的资源以及服务主动发布的邮箱 sender Handle。

## 启动与授权

进程启动 = 一个只读启动快照 + 一组已安装 Handle：快照携带身份（pid/parent）、args、配置等**信息**，Handle 安装交付对他人对象的**权利**；快照与安装在同一 launch 事务内原子完成，新进程 runnable 前定形。服务像读取参数一样按 tag 主动发现所需资源；Mailbox receiver 是可选资源，不占固定入口寄存器或固定 Handle 数值，需要而未被授予的进程自己创建。

成熟用户环境中 init 读取启动快照携带的 initfs 配置，创建服务并按最小权限原则交付设备、内存、服务依赖和管理 Handle。init 尚未接管时，内核启动装载者以同一 launch 事务暂代授权方（为集成负载组装邮箱对）；策略整体迁入 init 后内核只启动 init，不改变快照、Handle move 或对象契约，也不保留 `Send(pid)` 后门。

## 发现与调用

服务向目录服务登记可复制或可转移的邮箱 sender Handle 及协议标识。客户端查询名称后，目录在鉴权下返回裁剪 rights 的 Handle；客户端随后以该 Handle 发送请求。PID 只服务审计、父子关系和进程管理，不是服务地址。

请求需要回复时，客户端随消息转入回复邮箱 sender Handle；需要批量数据面时，任一方转入隧道 peer invitation。服务协议解释 kind、验证 sender 身份并决定是否接受附带 Handle；内核只保证 sender 不可伪造、rights 不放大和 move 原子。

## 生命周期

服务退出会关闭邮箱 receiver-owner、drain Handle 表，并使客户端现有 sender Handle 观察到 `CLOSED`。目录撤销名称只阻止新的发现，不改变既有 Handle；主动撤销需要服务对象或目录协议的间接层，而非 PID 重用或猜测对象存活。
