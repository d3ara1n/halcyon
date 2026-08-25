/* 平台内存常量（qemu sifive_u）。以常量而非 MEMORY 命令提供：MEMORY 会诱发
 * ld 对未显式指定区域的段按属性自动选区，高半区链接必须杜绝该行为。 */

SBI_START = 0x80000000;        /* OpenSBI 段起点 */
KERNEL_PA_START = 0x80200000;  /* 内核镜像 PA 加载基址 */
/* 每 hart 栈物理量：formal(0x5000) + emergency(0x1000)；两个 guard 洞
 * 纯虚拟不占帧。formal 较旧布局的 0x3000 增加。 */
STACK_SIZE = 0x6000;
