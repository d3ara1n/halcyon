# 针对特定服务的库

rinlib 提供对象、消息、等待和隧道的基础封装，保持纯运行时；服务框架在其上封装固定协议流程，避免每个服务重复实现启动、回复 Handle、版本校验和资源关闭。

## RPC 框架（librpc）

librpc 实现通用 [RPC](rpc.md) 信封：前缀编解码、请求/应答关联与 ReplyPort 复用。libfal、libfs 与 libsrv 依赖它。

## 服务框架（libsrv）

libsrv 接收版本化 `STARTUP` 消息，建立服务邮箱与请求循环，并在普通用户线程上通过 WaitMany 分发事件。它不绕过 Handle 授权或把 PID 当作服务地址。

## 文件系统框架（libfal）

libfal 实现 FAL 的固定宽 RPC：请求与应答以消息传递，流以隧道建立；它负责版本、长度和不变量校验，并提供提供者侧积木（节点模型、内存文件系统、分发循环），不把文件系统语义下沉到内核。

## 文件系统客户端库（libfs）

libfs 承担客户端职责：前缀路由表、路径规范化与走路、符号链接展开、从 startup grants 组装 namespace，以及服务发现客户端（见 [FAL](fal.md)）。

## 驱动框架（libdrv）

libdrv 在授权的设备对象、消息和隧道之上封装驱动服务协议。
