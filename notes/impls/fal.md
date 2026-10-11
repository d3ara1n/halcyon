# 原 FAL 边界

原 libfal 包、公共出口和源码已删除。文件的协议、值、backing、授权与客户端由 [libfs](../../user/libraries/libfs/src/lib.rs) 拥有，当前面貌见[文件实现](fs.md)；服务描述、发布、目录和订阅由 [libservice](../../user/libraries/libservice/src/lib.rs) 拥有，当前面貌见[服务目录实现](service.md)。发现表示、Namespace 和客户端接续由 liboal 拥有，通用消息与调用生命周期由[RPC](rpc.md)拥有。

文件和服务没有 FAL facade、ServiceRecord 文件投影或 DirectoryGrant/Position 兼容入口。srv_fs 的文件与 registry mailbox/backing 共用 Runtime，不使用旧 ServiceBackend 分流。init 与 test_fs 直接组合各领域协议；文件/服务 Watch 使用独立控制入口和真实 NotificationObserver，文件流与 Copy 由各自当前 owner 推进。具体生命周期与限制分别由 fs/service/RPC 实现篇拥有，验证与整体验收由[唯一工作记录](../../plans/todo-2026-10-07-fal-service-redesign.md)承接。
