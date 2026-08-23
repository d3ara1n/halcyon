# ECS for Robot

eRhino 本身就是单纯的通用微内核系统，ECS 由服务实现。

## Entity 含义

Entity 是相关 Components 的集合，可以代表真实存在的硬件，也可以单纯用于归纳同族数据。

## Component 表示离散的数据

比起数据，更像是具有某种特质，例如轮子 `WheelComponent` 更多表现出来的是可以驱动的特性，携带的数据为驱动功能服务。

## System 是进程

System 实质就是进程，负责更新 Components 数据，将数据输出到真实世界。例如 `WheelDriverSystem` 遍历 `WheelComponent`，根据保存的轮子信息，调用对应的驱动程序去驱动实际的轮子。

## 示例

```sh
/bin/world_server world.xml
```

世界文件 `world.xml`

```xml
<World>
    <Entity x:name="Car">
        <Component x:reference="Position" x="0" y="0" z="0">
        <Entity x:name="Chassis">
            <Component x:reference="Position" x="1.5" y="-0.5" z="0">
            <Entity x:name="LeftWheel">
                <Component x:reference="Wheel" position="LEFT" speed="0"/>
            </Entity>
            <Entity x:name="RightWheel">
                <Component x:reference="Position" x="-1.5" y="-0.5" z="0">
                <Entity x:name="LeftWheel">
                    <Component x:reference="Wheel" position="RIGHT" speed="0"/>
                </Entity>
            </Entity>
        </Entity>
    </Entity>
    <System x:name="WheelDriver" executable="/bin/car/wheel_driver"/>
</World>
```

组件如何定义？由文件定义还是单纯匿名传输，双方约定（共享库）编码解码。后者意味着世界服务器中的组件都只是一串字节，需要 System Process 去解码，更新，发送回去，需要做锁处理，性能就不怎么样。
