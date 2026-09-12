/* 平台内存常量（qemu sifive_u）。以常量而非 MEMORY 命令提供：MEMORY 会诱发
 * ld 对未显式指定区域的段按属性自动选区，高半区链接必须杜绝该行为。 */

SBI_START = 0x80000000;        /* OpenSBI 段起点 */
KERNEL_PA_START = 0x80200000;  /* 内核镜像 PA 加载基址 */
/* 每 hart 与 virt 统一预留 256KiB，含 4KiB emergency；guard 纯虚拟。
 * 8 槽物理占用 2MiB，占 128MiB DRAM 的 1.56%，不以平台专属小栈约束通用代码。 */
STACK_SIZE = 0x40000;
