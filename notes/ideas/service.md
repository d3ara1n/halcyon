# 服务进程

服务是普通进程通过发布协议 endpoint 承担的系统角色，不是内核特殊进程类型。调度、地址空间、HandleTable、Job 预算和退出语义与其他进程相同；差异只来自 launcher 交付的 capabilities 与服务主动发布的 endpoint。

## 启动与授权

launcher 在 Building 阶段以独立的 Map/Write、Grant 与 Attach 动作准备映像、用户态 StartupBlock、初始 capabilities 和线程现场，再由 ProcessStart 一次发布全部预育线程。常见组装方式是 launcher 创建 Mailbox、保留 sender，把唯一 owner 直接 GRANT 给 child；服务也可以启动后自行创建 endpoint。

普通服务的 payload 可采用用户态 LauncherParcel，按索引描述 args、配置、namespace routes 与 Handle 语义。内核不理解普通 StartupBlock outer、“服务邮箱”或任何业务 tag；只有 init bootstrap 保留内核构造的同形 outer。

成熟用户环境中 init 持 root Job 与平台根 capabilities，按最小权利启动资源管理、文件系统、驱动和其他服务。PID 创建关系不授予管理权；Process Controller capability 才能管理进程。

## 发现与调用

服务通过显式注册控制权向用户态目录发布 badged sender。一个原子 service record 同时包含 instance、protocol/version、endpoint 和记录代次，客户端一次读取就取得完整快照；不得逐字段拼接可能来自不同实例的值。

客户端取得 Handle 后直接调用。服务按内核提供的发送授权身份找到 grant/session，badge 是该授权的不可变标签，PID 只作 provenance。请求上下文拥有消息 Delivery 与 send-once 回复权，批量数据面交付 Tunnel Invitation；这些责任彼此独立。

注册上级 capability 限定可管理的名称或子树，单次注册控制权只管理对应注册实例；两者都与普通目录读写权分离。注册状态机是服务发布的唯一可写真值，FAL 只提供受控投影。普通创建、写入、删除、移动或 Take 都不能绕过注册规则；投影不能成为独立维护的第二份发布状态。

endpoint 必须具有其调用、观察和导出契约要求的权利，并已经符合发布者的出口政策；目录不能自行收窄未知业务协议的 badge 权限。共享调用入口与独立会话建立入口是不同的服务承诺：普通 Mailbox sender 的 Duplicate 保持发送授权身份；DirectoryGrant 发现通过目标 provider 的 Derive 产生独立发送授权。两者都不自动创建新付款账户、每客户端预算或独立业务 session；需要这些能力时由目标协议显式建立。

访问服务目录的权限与被发布服务的权限属于两个授权域。目录读取者必须获准读取记录及取得能力；目标 DirectoryGrant 的权限由记录的出口上限与目标 provider 的真实母授权校验决定，不能拿发现目录的只读权限机械裁剪目标文件服务的操作权限。

boot-critical 依赖在 Building 阶段经直接 grant 写入 StartupBlock；动态依赖可通过 FAL 服务目录发现。首个目录提供者由 init 的显式启动拓扑打破引导环，不需要 PID Send 后门。

## 生命周期

名称占用、发现可见性和单次注册控制权的寿命分别表示：

```text
注册实例：Starting → Ready → Draining → Terminal
提前终止：Starting → Draining → Terminal
名称绑定：Starting / Ready 独占；进入 Draining 即释放
发现投影：只有 Ready 存在目录成员；撤出后新注册使用新节点
```

注册实例采用本次启动中不复用的控制授权身份；它不是服务进程、endpoint 或名称的身份，数值本身不授予控制权。Starting 有有限建立期限，不向普通发现者显示诊断记录；管理者通过注册协议查询。发布后的记录不可原地改写，修改 endpoint 或协议须形成新实例。

同名 Register 在 Starting/Ready 占用期间明确失败。替换是条件撤出旧实例、再独占注册新实例，允许短暂不可发现，且撤出后第三方可能抢先注册；不承诺原子交换、无缝升级或保证重新取得名称。上级撤出必须核验预期实例及代次，旧控制权、旧回调和旧清理只能影响自身实例，不能删除同名替代者。

PublishReady 一次发布完整记录及目录可见性。BeginDrain 与完整快照取得由同一状态拥有者排序：已固定节点身份但尚未取得完整内容，不算取得发现快照；进入 Draining 后拒绝新的取得，此前取得的完整快照可以继续导出、交付。摘名不等于停止业务准入或完成旧请求；三者由各自 authority 推进。

重复 Ready 请求只在仍为 Ready 时幂等；Draining/Terminal 不可重新发布。重复 Drain 不倒退状态。显式放弃进入不可逆终态，使迟到发布不能复活实例；Query 只报告查询时状态，不能证明较早的请求不会稍后提交。注册回复未能交付时收回未交付控制权及不可见注册；回复入箱但未接收仍受建立期限约束。Ready/Drain 已提交后回复丢失不回滚可见性，调用者查询或显式终止，不能盲目重注册。

目录仅观察注册控制授权的 Lifetime，不保存控制 sender 母本。endpoint sender 的 CLOSED 表示目标 Mailbox owner 关闭；其 Lifetime 则可能被目录保存的母本保活，两者不能混用。endpoint 关闭、控制权最终消散和建立期限到期都撤出对应实例。Terminal 可保留有界诊断壳直到控制权消散，但不得因此保留名称、endpoint 或节点引用形成退休环。

撤销名称只阻止新发现，不追溯销毁已授 capability。政策撤销阻止特定授权的新操作准入；已准入请求和独立建立的连接按自身契约收束，不隐含跨服务递归撤销。capability 转交后的寿命由真实引用及交付责任决定，不依赖原进程保活或周期续租。

服务记录 schema 与注册控制属于 libservice，通用 Record、能力值和目录投影接口属于 FAL。首个承载者由启动拓扑指定，不因此成为所有进程必须经过的全局注册权威；每个服务目录仍有自身明确的受委托注册权威。

## 发现视图与失效

发现使用同一代次的完整快照，Watch 只提示该视图可能失效，不承诺事件次数、顺序或历史。Ready 发布、Draining 撤出、实例替换和撤销必须使相应发现视图的观察者能够察觉并重读；不能假定普通属性内部修改会通知父目录观察者。记录身份、目录成员关系与各自代次须与投影语义一致。

服务域的基本投影为平面目录。Ready 增加成员并推进目录代次；Drain/撤销删除成员、推进目录代次并终止旧节点观察；新实例永不复用旧节点。记录发布后不可变，因此无需把属性修改递归冒泡到父目录。名称占用但尚未 Ready 不产生虚假的可发现成员。

客户端先订阅再读取，处理枚举冲突及记录删除重建，不通过无保护的退订再订阅制造静默窗口。目录关闭时停止依赖旧观察；重新获得目录 authority 或报告不可用由显式启动/监督政策承担。endpoint 可调用性、注册可见性与业务健康是不同事实，发现不隐含健康检查或透明重试。

## 监督与接管

系统配置必须把启动项声明为必选或可选。必选服务的映像缺失、构造失败或授权失败使整组启动失败，不能进入看似正常的服务拓扑；可选项失败则形成显式 Degraded 记录。launcher 只有在必选集合完整发布后才能宣布 stage Ready。

监督 authority 在目标完成 Drain 且终态快照核验成功前不得关闭。每次等待、Drain、Query 与枚举停滞都受 policy 的 deadline、工作预算和重试次数约束；预算耗尽返回当前阶段、已完成工作与仍持 control 的 owner。局部监督者可以重试或把该 owner 连同进度交给上级；若目标仍在 Job 成员表内，上级也可用保留的 JobControl 重新派生 control 并执行整域收束。

委托管理不消除根接管权。子域管理者持一份域内 JobControl，root supervisor 保留另一份独立 control；前者失败时停止关闭残留 authority并发布 handoff/unmanaged 诊断，后者按自己的 policy 接管。内核只发布 REAPABLE/CLOSED 与有界 Drain，不实现重启、deadline 或递归 JobKill。
