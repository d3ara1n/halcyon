# 内核调用

分为两种，Remote Call 和 System Cal。

## Remote Call

远程调用。
由 `ApplicationHart` 发送给另一个同类型 hart，没有返回值，用 IPI 实现。目前没实现也没应用。

## System Call

系统调用。
由进程调用内核，用 Ecall 实现，有返回值。有两种调用类型，异步和同步。同步调用是最常用的类型，也是大部分系统调用的实现，异步调用不会直接返回结果，而是先调度掉进程，等任务完成之后再返回结果。

异步调用期间内核不会等待，而是执行其他进程任务，为此创建了内核请求(Kernel Request)来表示排队中的请求。当进程调用一个异步系统调用，进程会被标记为 Pending 状态，不再加入调度，直到内核完成了请求并标记进程为 Fed，此时进程才会重新调度并以相同参数触发系统调用，完成后续任务。
