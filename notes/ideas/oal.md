# 开放发现与命名空间

OAL（Object Abstraction Layer）提供跨领域的发现表示与客户端接续。文件、服务、设备和功能协议分别拥有自己的对象、授权与操作；共同的目录面貌不使它们成为文件节点。

## 概念与关系

- ObjectInfo 描述对象公开的 traits 与摘要，业务身份和可操作能力由所属领域解释。
- Discover 提供描述、枚举及边界；Boundary 表示一次路径接续所需的入口、位置关系和协议面貌。
- Namespace 表达逻辑前缀与真实入口的组合。系统服务的初始命名空间由上游 init 按组态装配交付，不以各客户端自行发现并拼出系统视图为前提；使用方的本地表示不等于装配政策的归属。路径文本不产生 authority。
- 客户端 walk 组合本地命名空间、符号链接与 provider 边界。最终业务操作仍由目标领域鉴权并执行。

发现所需的公共表示由 liboal 拥有。文件目录的内容来自 libfs，服务目录来自 libservice，设备和功能视图来自相应领域。各 provider 从自己的权威状态生成视图，不维护第二份通用对象数据库。

## 系统中的位置

应用通过 OAL 寻找入口，再使用目标协议调用。启动关键依赖可以直接经 capability 交付，避免依赖尚未建立的发现服务。默认路径布局属于装配政策，不成为协议或库的硬编码依赖。

可执行映像可以来自 OAL 文件或其他字节对象，发现与访问能力不等于进程创建权。装载创建入口见 [pm](process-management.md)，应用包的解释与启动关系见 [application](application.md)。

详细路径、分页、授权及接续合同见 [OAL/fs/服务目录计划](../../plans/todo-2026-10-07-fal-service-redesign.md)。文件概念见 [fs](fs.md)，服务目录见 [service](service.md)，设备关系见 [device](device.md)。
