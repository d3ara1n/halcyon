# 设备授权

设备管理分为内核短路径机制与用户态策略。内核不维护供任意进程按路径领取的 raw-device 列表，也不把中断注入用户 handler。

## 平台根

内核依据可信 Devicetree 与平台事实铸造 MMIO region、IRQ source、DMA window 等 primordial capabilities，并在初始 launch 中交给 init 或资源管理服务。内核负责对象边界、映射、等待、确认与回收；由哪个驱动取得哪些资源是用户态策略。

## 驱动租借

资源管理服务按设备节点与驱动需求派生最小 capability 集，通过 ProcessStart 的直接 GRANT 启动驱动。驱动崩溃或被终止时，Handle drain 解除映射、屏蔽或回收中断/DMA 资源，使设备可由管理服务重新初始化和租借。

中断以可等待对象表达，并定义 mask、ack、关闭与失主后的安全状态。高层设备请求经驱动发布的 badged service sender、消息和 Tunnel 传输，不让客户端直接持 raw MMIO。

## 发现与投影

`/sys/dev` 可以是设备管理服务向 FAL 提供的用户态投影，用于枚举、属性和服务发现；它不是设备 authority 的来源。客户端真正获得的是协议 endpoint 或设备 lease capability，路径字符串本身不授权访问。
