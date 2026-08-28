# 机器人应用层 ECS 构想

> 本篇是机器人应用框架构想，不属于 eRhino 内核、系统 ABI 或基础服务契约。

ECS 由普通用户态服务实现。Entity 表示相关 Component 的集合；Component 是由应用协议定义的离散数据；System 是读取和更新这些数据的普通进程。内核只提供进程、capability、消息与共享内存，不识别 Entity、Component 或 System。

世界描述、Component schema、并发更新、持久化和跨进程编码均由未来应用框架单独设计。应用层可以选择共享 schema、消息协议或共享内存，但不得把 ECS 身份提升为内核 authority：访问仍由交付给各 System 的 capability 决定。
