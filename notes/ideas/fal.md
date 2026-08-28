# 文件系统抽象层

FAL（Filesystem Abstract Layer）是用户态客户端库与目录提供者之间的固定宽协议。内核对路径、节点、挂载、属性和文件权限零感知，只提供对象、Mailbox、Tunnel 与等待。

## DirectoryGrant

**DirectoryGrant** 是对 provider 内某个目录根及操作上限的用户态 capability，不是内核 Directory 对象。推荐由 badged Mailbox sender 承载：provider 以 sender_badge 查得根节点、FAL rights ceiling 与生命周期状态。

请求路径只在 grant 根内命名对象，不携带 authority。进程如何用前缀表选择初始 grant、挂载和卸载由 [`fs.md`](fs.md) 唯一拥有。FAL 只规定 provider 可以在子树边界返回更具体的 DirectoryGrant，供客户端继续组合走路。

## 权限

系统不要求 uid/gid 或 Unix 用户权限。授权分两层：

- 内核 Handle rights 控制 Send、Wait、Duplicate、TRANSIT、GRANT；
- FAL grant rights 控制 Traverse、Enumerate、Create、Remove、Read、Write 等目录操作。

provider 把 FAL rights ceiling 绑定在 GrantState 中。持有者不能自行放大；进入子树或缩小 rights 时调用 DeriveGrant，由 provider 铸造新 badge。若未来加入多用户，ACL 只参与 DirectoryGrant 铸造和收窄，不改变内核。

节点返回的可用操作是“当前 grant 下允许什么”，不是全系统通用的 inode mode。provider 可以内部实现 ACL、只读介质或更细政策，但 FAL 不强制身份模型。

## 走路

客户端负责路径规范化、前缀选择、符号链接展开与跨 provider 组合。请求相对某个 DirectoryGrant 携带剩余路径；provider 在 grant 根和 rights ceiling 内尽量行进，返回：

- **Found**：抵达终点节点，携带类型与元数据；
- **Delegate**：抵达下级 provider 边界，返回新的 DirectoryGrant、已消费前缀与剩余后缀；
- **SymbolicLinkBoundary**：途中遇到符号链接，返回父位置、target 文本与剩余后缀，客户端继续解释。

逻辑位置由 DirectoryGrant、相对路径与权限上界组成。`..` 由客户端逻辑栈解释，不能越过 grant 根；绝对符号链接从调用进程自己的前缀表重启。整次解析限制展开次数、总字节、组件数和 provider 跳数。

客户端库不是信任边界。provider 必须重新校验路径编码、长度、偏移、cursor、badge 对应 GrantState 与每次操作 rights。

## 节点与属性

节点分目录、属性、流与符号链接。挂载点不是节点类型。符号链接是 provider 持久化的路径文本，不携带 Handle、不铸造 authority，允许悬空，并在调用者 namespace 中解释。

硬链接不进入通用协议：节点身份、unlink、watch、配额与链接计数无法在 provider 间透明统一。存储去重可由 provider 内部 reflink/COW 完成。

属性是节点上的具名类型值，覆盖固定宽整数、浮点、字符串、字节集、`Array<T>` 和 `Handle[T]`。T 是协议类型提示，客户端仍以实际对象操作验证 role。

重复读取的 `Handle[T]` 属性要求 provider 母本同时具备 DUPLICATE 与 TRANSIT；每次读取派生、按调用 grant 收窄，并在应答中 TRANSIT。affine 值应定义为一次性属性，成功读取后变空，不能伪装成可重复读取。写入 Handle 属性是原子替换：成功接收新 entry 后关闭旧值。

## 服务发现

动态服务可以作为目录中的原子 service record 发布，record 同时包含 instance、protocol/version 与 endpoint capability。客户端读取 endpoint 属性即取得经 provider 鉴权和收窄的 badged sender。

boot-critical 依赖仍由 StartupBlock 直接 GRANT；首个目录 provider 由 init 的显式启动拓扑建立。服务死亡由 endpoint `CLOSED` 表达，目录随后撤销旧 record；客户端是否重新发现和重试由业务协议决定。

## Watch

Watch 使用显式 Subscribe RPC。客户端为每个订阅创建独立 Notification，并把 signaler TRANSIT 给 provider；provider 只向该订阅提交 create/delete/modify/rename 位。客户端关闭 owner 即结束订阅。Notification 的等待、Take 与竞争消费语义由 [`signal.md`](signal.md) 拥有；FAL 只规定各位的业务含义。需要可重放、高频或带负载事件时使用消息或共享内存日志。

## 流

Open 由 provider 建立 Tunnel，并在应答中 TRANSIT peer invitation；客户端 attach 后取得本地 Endpoint。流的顺序、EOF、backpressure 与页内错误由 Runnel 等协议表达，控制面只负责建立和最终状态。

随机小块访问可用 ReadAt/WriteAt，受消息 payload 上限约束；大数据不在控制面分片。Endpoint 或服务关闭转换为客户端错误，不能把 PEER_CLOSED 当作正常 EOF。

## 固定宽 RPC

FAL payload 在通用 [`RpcPrefix`](rpc.md) 后追加 FAL 版本、kind、总长度与 reserved；slot 0 的 send-once 约定由通用 RPC 拥有。FAL 字段固定宽、little-endian、版本化，写者置零 reserved，读者验证长度和业务不变量。

kind 覆盖 Lookup、Enumerate、属性 Read/Write、Create、Delete、Move、Copy、Link、Open、ReadAt、WriteAt。目录枚举使用不透明 cursor；并发修改可令 cursor 失效。

v1 的 Move 只承诺同一 provider/capability domain 内原子完成；跨 provider 返回 CrossDevice。跨域 Copy 由客户端经流编排并允许部分目标，Move 不隐式退化为 copy+delete。

## 边界

FAL 不规定 provider 内部存储、缓存、一致性、ACL 或配额算法。内核不解析 badge、路径与节点；provider 负责 badge→GrantState、请求鉴权、准入和资源治理。
