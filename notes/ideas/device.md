# 设备资源

设备接入分为平台资源机制、用户态资源管理和驱动协议。内核不维护按路径领取 raw device 的策略列表，也不把中断注入用户 handler。

## 平台资源根

内核依据可信 Devicetree 与平台事实铸造 MMIO region、IRQ source、DMA window 等 primordial resource capabilities，并在初始启动中交给 init 或资源管理服务。资源 capability 只表达对特定硬件范围或来源的 authority，不隐含驱动匹配、设备类别或服务发现政策。

MMIO 映射、IRQ 控制与 DMA 授权必须各自有明确对象边界和撤权路径。资源编号、物理地址和路径字符串都不是 authority；用户态只有取得相应 capability 才能操作资源。

## 驱动授权与恢复

资源管理服务根据设备节点与驱动需求派生最小资源集合，并通过 ProcessStart 的直接 GRANT 启动驱动。高层客户端只取得驱动发布的协议 endpoint，不直接持 raw MMIO、IRQ 或 DMA authority。

驱动失效后的安全状态分层处理：内核资源对象撤销映射和后续访问；驱动或资源管理服务负责停止 DMA、屏蔽设备侧事件、复位硬件并判断能否重新授权。设备级 session 或 lease 是用户态资源服务协议，不预设为一个通用内核对象。

## IRQ 设计边界

本篇不提前选择 IRQ source 是直接可等待对象、绑定 Notification，还是通过其他事件端口投递。该选择必须在设备/中断接入设计中同时闭合：

- interrupt controller 的 mask/ack 与重入顺序；
- 共享 IRQ 与 MSI/MSI-X；
- 事件合并、溢出和背压；
- capability 撤权、驱动退出与重新租借；
- 设备 reset、DMA 停止与中断安全状态的责任边界。

无论采用哪种传输，IRQ 到达只产生普通内核短路径事件，不执行用户 handler；用户态驱动在自己的线程和调用栈上处理。

## 发现投影

设备管理服务可以经 FAL 发布设备记录，用于枚举属性和发现驱动服务。目录布局由文件系统 namespace 政策拥有，FAL 只运输记录与 endpoint capability；路径名称不授予设备访问权。
