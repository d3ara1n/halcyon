# FAL 实现现状

方向见 [`../ideas/fal.md`](../ideas/fal.md) 与 [`../ideas/fs.md`](../ideas/fs.md)。通用 RpcPrefix/Caller 的实现由 [`rpc.md`](rpc.md) 唯一拥有；本篇只记录 FAL wire、provider、libfs 与当前验收边界。

## libfal

`user/frameworks/libfal` 定义 FAL 线协议与 provider 积木：

- Mailbox message kind 使用 `PROTOCOL_ID = "FAL1"`；payload 为 `[RpcPrefix][FalHeader][body]`；
- FalHeader 16 字节：协议版本、kind、总长度和零 reserved；
- Handle slot 0 是 send-once 回复授权，slot 1 当前作为 directory anchor handle；
- kind 覆盖 Lookup、Enumerate、Create、Link、Property Read/Write、Open、ReadAt/WriteAt、Move、Copy、Delete；
- 12 个普通协议状态码，另有 `Internal = 0xFFFF` 保留错误码。

`bytes.rs` 提供 LE 游标；`node.rs` 定义 Directory/Property/Stream/SymbolicLink 与相对路径校验；`lookup.rs` 定义 Found/Delegate/SymbolicLinkBoundary 和 ResolvePolicy；`enumerate.rs` 使用 provider cursor；`property.rs` 定义属性类型与 watch 位。

`memfs.rs` 是内存树参考 provider：walk 统一检查 X 穿越、R 枚举、W 增删，链接只返回边界；枚举 cursor 编码 generation/index，代次失配返回 CursorInvalid。`provider.rs` 解码请求并分发到 memfs。

## libfs

`user/frameworks/libfs` 保存进程私有前缀表并执行客户端走路：

- 前缀按段边界最长匹配，重复挂载替换，卸载返还 Handle；
- `..` 经逻辑帧栈回退且不越 grant 根；
- 绝对符号链接从调用进程前缀表重启，相对链接在当前位置展开；
- 限制符号链接 40、组件 4096、总字节 65536、Lookup 步 256；
- 产出 `Position { anchor, rel, info }`，后续操作用 anchor Handle 与相对路径寻址。

## 当前 fs 验收边界

`user/systems/fs` 在同一进程内同时运行 memfs provider 与客户端泵。每次调用真实经过内核 Mailbox：send → provider receive/serve → send-once reply → client receive。它验证了 FAL 编解码、Mailbox、Handle move、send-once 与 WaitMany；覆盖创建、枚举、属性 Array、符号链接展开和 Stream 节点的 ReadAt/WriteAt。

该路径仍有明确边界：

- slot 1 是临时 directory anchor Handle 副本，provider 只关闭它；不是 DirectoryGrant badge，也没有 FAL rights ceiling；
- provider 与 client 同进程，不是跨进程服务；
- Lookup Delegate 只在类型与 libfs mock 中存在，provider 不产生真实 Delegate，Caller 当前拒绝 FAL reply 携带额外 Handle；
- Open 返回 Unsupported；Stream 只是 memfs 字节数组的偏移读写，尚未连接 Tunnel/Runnel；
- Move/Copy 返回 Unsupported；Handle[T]、DirectoryGrant 鉴权、每订阅者 Watch 与服务发现尚未落地。

独立的 init/pm Tunnel/Runnel 验收只证明数据面机制可用，不等同于 FAL Open 已接线。

## 验证

libfal/libfs host 测试覆盖 framing、路径、cursor、属性、符号链接与前缀走路；QEMU `srv_fs` 输出 `fs acceptance passed`，并由总体 acceptance 脚本与服务监督、显式 reset 终态共同判定。
