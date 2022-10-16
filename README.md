# ~~kzios~~ eRhino!

操作系统学习：RV64 嵌入式系统

## 进度

- [x] 进入 Rust 环境
- [ ] 陷入处理
  - [x] 捕获异常并输出陷入帧概览
  - [ ] 外部/软/时钟中断统筹与转发
- [ ] 系统调用
  - [ ] 调用框架
  - [ ] 具体调用实现
- [x] ~~线程(内核不支持线程)~~
- [ ] 进程
  - [ ] 进程管理
  - [ ] 调度和多核
- [ ] 内存分页
  - [x] map, 支持大中小页
  - [ ] write
  - [ ] unmap, 可能会有大页中取消次级页的复杂情况
- [ ] 信号
  - [ ] 进程的信号处理函数设置
  - [ ] 内核调用处理函数
  - [ ] 返回进程空间的跳板系统调用
- [ ] 进程级别系统服务设计
  - [ ] 终端输入输出服务
  - [ ] 文件系统服务
    - [ ] 虚拟文件系统
    - [ ] IPC 接口
- [ ] 内核 IPC
- [ ] 外围设备管理
- [ ] 用户可执行程序
- [ ] ...

## (将)受支持的平台

- qemu-system-riscv64: 1 core 8MB ram with MMU
- k210: 2 cores (suspend #1) 8MB ram with MMU

## 标准库

~~Porting std is a huge thing, I wont do it at the current stage.~~

仅提供 ~~`kinlib`~~`rinlib`