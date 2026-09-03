/* 平台内存常量（qemu sifive_u）。以常量而非 MEMORY 命令提供：MEMORY 会诱发
 * ld 对未显式指定区域的段按属性自动选区，高半区链接必须杜绝该行为。 */

SBI_START = 0x80000000;        /* OpenSBI 段起点 */
KERNEL_PA_START = 0x80200000;  /* 内核镜像 PA 加载基址 */
/* 每 hart 栈物理量：formal(0xF000) + emergency(0x1000)；两个 guard 洞
 * 纯虚拟不占帧。约束的是**调用链总和**而非单帧（单帧另由 audit_elf.py 卡
 * guard 洞跨度）：debug 构建下 compiler-builtins 与深层内存事务链的组合峰值
 * 已超过 0x9000，实测在 Tunnel 建立路径触发 guard page hit。8 hart × 64KiB
 * = 512KiB，占该平台 128MiB DRAM 的 0.39%。 */
STACK_SIZE = 0x10000;
