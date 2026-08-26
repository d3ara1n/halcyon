# FAL 实现现状

方向与契约见 [ideas/fal.md](../ideas/fal.md) 与 [ideas/rpc.md](../ideas/rpc.md)；本篇记录已落地的实现现状。内核对文件系统零感知，全部内容在用户态。

## 分层落点

- `user/frameworks/librpc`：通用 RPC framing。`RpcPrefix`（16B LE：rpc 版本、request/response/oneway、txid、保留区零校验）+ per-process 单调非零 txid。同步 `Caller`：懒创建 ReplyPort、slot 0 约定（期待回复的 request move 裁剪至 WRITE|TRANSIT 的 send-once）、单 outstanding、超时关 port 废弃重建、迟到回复随 owner 关闭消亡。异步 dispatcher（免 tombstone：无 pending 静默丢弃）尚未接入。
- `user/frameworks/libfal`：FAL 线协议与提供者积木。`PROTOCOL_ID = "FAL1"` 占消息 kind；payload 布局 `[RpcPrefix][FalHeader][body]`，FalHeader 16B（协议版本、kind、总长度、保留区 u64）；Handle 槽约定 slot 0 = 回复授权、slot 1 = 帧锚目录。kind 号冻结 12 个，动词自含对象（Lookup/Enumerate/Create/Link/Read/Write/Open/ReadAt/WriteAt/Move/Copy/Delete——Read/Write 指 Property 整值，At 后缀指 Stream 偏移，Link 创建符号链接）；Found 应答的 NodeInfo 带 kind 判别的自描述 value 尾（SymbolicLink = target，NoFollowFinal 终段即得）；Status 12 值含 `SymbolicLinkEncountered`（op 内部行走遇链接，客户端展开重试）。
  - `bytes.rs` LE 编解码游标；`node.rs` 四类节点 + RWX 标记 + `validate_path`（相对、无 `.`/`..`/空段/通配符、UTF-8、≤512）。
  - `lookup.rs` 三值应答与 `ResolvePolicy`；`op.rs`/`io.rs` 非 Lookup 操作的寻址前奏 `(policy, reserved, rel)` + 参数；`enumerate.rs` cursor 分页；`property.rs` 属性类型系统（Integer/Decimal/String/Blob/`Handle[T]`/`Array<T>`，watch 位 create/delete/modify/rename）。
  - `memfs.rs` 内存树参考积木：walk 核心统一 X 穿越/R 枚举/W 增删检查，链接只返边界不解释，枚举 cursor 编码 `(generation, index)`，代数不符即 CursorInvalid。
  - `provider.rs` 纯编解码分发：FalHeader 起的请求 → memfs 操作 → 应答 body。Move/Copy/Open（tunnel 交付）为 Unsupported 桩，kind 号已留。
- `user/frameworks/libfs`：客户端命名空间库。`prefix.rs` 前缀表（段边界匹配——`/a` 不匹配 `/ab`；重复挂载替换；卸载返还 Handle）。`resolve.rs` 走路引擎：单逻辑位置列表 + 帧栈（`frames[i].base` 分区 provider 区域）；`..` 词法回退跨帧、根处钳制；绝对 target 对前缀表重启整次解析，相对 target 原地替换链接分量；限额 SYMLINK=40 / 组件 4096 / 字节 65536 / Lookup 步 256。解析产出 `Position{anchor, rel, info}`，后续 op 以 slot 1 = anchor、body 内 rel 寻址。
- `systems/fs`：内存 FAL 提供者 + 同进程自客户端验收负载。每次调用经内核 mailbox 真路径（send → provider receive → serve → send-once 回复 → 客户端 receive）；「泵」在等回复期间服务提供者邮箱。演示覆盖目录/属性/流创建、枚举分页、属性 Array 类型往返、符号链接边界与客户端展开、偏移读写。

## 与旧实现的切换

旧内核直连 fs ABI（`shared/src/fal.rs`、`shared/src/path.rs`、`rinlib::fs`、call 号 0x70–0x79）已整体删除；内核从不 dispatch 这些调用号，无内核侧改动。跨进程客户端与服务发现（`/srv/...` 属性、Handle[T] 铸造、Delegate 真实跨界）随服务化批次接入。

## 已知简化

- 同进程泵不支持并发 in-flight；跨进程使用 librpc `Caller`（单 outstanding）。
- memfs 无 Handle 属性与委托边界；组合提供者（namespacefs）未实现。
- Move/Copy/Open 为 Unsupported；流数据面（tunnel+Runnel）后续接入。
- 属性值写入按原始字节存储；提供者侧类型校验随 Handle[T]/watch 接入。
