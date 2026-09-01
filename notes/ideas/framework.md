# 用户态服务框架

rinlib 只提供 StartupBlock outer、对象、消息、等待和 Tunnel 的基础封装。用户态框架在其上实现通用协议流程，不能绕过 capability 或把 PID 当地址。装载链库（libelf、libprocess、ld-erhino）的定位由 [bootstrap](bootstrap.md) 拥有；本篇拥有运行期协议与服务框架库。

## RPC 框架（librpc）

librpc 实现 RpcPrefix、txid 分配、ReplyPort、send-once 回复授权、同步 Caller 与异步 dispatcher。它暴露 `sender_pid/sender_badge`，但不替业务协议决定身份或授权。

## 服务框架（libsrv）

libsrv 从 rinlib 取得 StartupBlock Handle 数组与 opaque payload，并按用户态 LauncherParcel 解释 args、服务 owner、namespace 和依赖。服务 owner 在 ProcessStart 前已经由 ProcessGrant 安装；libsrv 不靠尚不存在的邮箱接收“启动资源消息”。

若服务协议需要 runnable 后的 activate/drain/reload 控制，应定义独立版本化 control message，不与 launch 混称 STARTUP。

libsrv 在普通用户线程上以 WaitMany 组合请求 Mailbox、控制 endpoint、timer 和 Notification，并提供 session/badge 分发、关闭传播与每客户端准入积木。

## 文件系统协议与 provider 工具（libfal）

libfal 定义 FAL wire、固定宽编解码、版本/长度校验和 provider 侧分发积木。协议 codec、provider toolkit 与参考 memfs 应保持分层，使客户端或真实 provider 不被迫依赖参考存储模型。

## 文件系统客户端（libfs）

libfs 管理 `prefix → DirectoryGrant` 私有 namespace，负责路径规范化、最长前缀、走路、符号链接展开、Delegate 与从 LauncherParcel 组装初始 grants。它不承担所有系统服务的强制基础层；boot-critical endpoint 仍可直接 GRANT。

## 驱动框架（libdrv）

libdrv 在 MMIO/IRQ/DMA capabilities、badged service sender、消息和 Tunnel 上定义驱动协议与租借生命周期。设备枚举和匹配属于用户态设备管理服务，不下沉内核。
