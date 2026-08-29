# rust
MODE := "debug"
RELEASE := if MODE == "release" { "--release" } else { "" }

# 用户态 target 同样需要 build-std 许可（config 不能全局开启，同内核）
ZFLAGS_USER := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"

# platform
PLATFORM := "qemu"
MODEL := "virt"
DTS := invocation_directory()/"os/platforms"/PLATFORM/MODEL/"device.dts"
MEMORY_SCRIPT := invocation_directory()/"os/platforms"/PLATFORM/MODEL/"memory.x"

# compile
RUSTFLAGS_OS := "-Clinker=riscv64-elf-ld"
RUSTFLAGS_USER := ""

TARGET_DIR := invocation_directory()/"artifacts"
KERNEL_TARGET_DIR := TARGET_DIR/"cargo"/PLATFORM/MODEL

# 平台产物按 MODEL 隔离：切平台不可能读到旧平台的 dtb/内核
MODEL_DIR := TARGET_DIR/PLATFORM/MODEL
KERNEL_ELF := MODEL_DIR/"erhino_kernel"
KERNEL_BIN := KERNEL_ELF+".bin"
# 官方 DTB 固定生成路径；ERHINO_DTB 只覆盖 QEMU 载入的 -dtb（异构
# 域变体用，见 virt-hetero/virt-nofd），不重定向 make_dtb 的产出。
DTB_DEFAULT := MODEL_DIR/"device.dtb"
DTB := env_var_or_default("ERHINO_DTB", DTB_DEFAULT)
INIT_PAYLOAD := TARGET_DIR/"initfs.tar" # init 私有 payload，内核不解释
BOOT_PACKAGE := TARGET_DIR/"boot-package.bin"

# QEMU
# BootPackage 装载地址：virt DRAM 1GiB 取高址；sifive_u 实际 DRAM 仅 128MiB，
# 取 dtb 声明的 0x84000000（与 dts 的 boot-package reg 保持一致；尾部 64MB 作装载区，
# 帧池只按实际包长排除，扩窗零内存代价）。
BOOT_PACKAGE_ADDR := if MODEL == "virt" { "0xB0000000" } else { "0x84000000" }
# 与 virt DTS 声明的 Zkr 能力一致；sifive_u 不声明该扩展。
QEMU_CPU := if MODEL == "virt" { "-cpu rv64,zkr=true" } else { "" }
QEMU_LAUNCH := "qemu-system-riscv64 -M "+MODEL+" -m 1024M -nographic -kernel '"+KERNEL_BIN+"' -dtb '"+DTB+"' -device loader,file="+BOOT_PACKAGE+",addr="+BOOT_PACKAGE_ADDR+" " + QEMU_CPU
# CPU 节流百分比（tools/qemu-throttle.sh）：跑飞/panic 时 QEMU 满核空转的兜底。
# 1-99 按比例节流；100 = 全速。默认 50；自定义经环境变量：
# `THROTTLE=100 just virt`（env 穿透嵌套 just 调用；recipe 参数与
# --set 均不穿透嵌套子进程，故不用它们传油门）。
THROTTLE := env_var_or_default("THROTTLE", "50")
# 无 shutdown 设备平台（sifive_u）的运行阶段硬上限。它只是挂死兜底，
# 不是期望耗时：完整验收实测约 15s（全速）/ 21s（节流 50%；矩阵多为
# timer 驱动，节流不线性放大），随后显式 reset 返回失败并由 wrapper 收割。
# 60s 约 3x 余量——不得调小到完整验收面以内。
ACCEPTANCE_TIMEOUT := env_var_or_default("ACCEPTANCE_TIMEOUT", "60")

# gdb
GDB_BINARY := "riscv64-elf-gdb"
GDB_TARGET := KERNEL_ELF

alias b := build_kernel
alias c := clean

# 内核 target 的 build-std 许可（host 测试走 --target host，不受影响）
ZFLAGS := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"

# 秒级检查：与 build_kernel 同参
check:
    @cd os && cargo check {{ZFLAGS}}

clean:
    #!/usr/bin/env bash
    if [ -d "artifacts" ]; then
    	rm -r artifacts
    fi
    cargo clean --manifest-path os/Cargo.toml

artifact_dir:
    #!/usr/bin/env bash
    if [ ! -d "artifacts" ]; then
    	mkdir artifacts
    fi
    if [ ! -d "artifacts/build" ]; then
    	mkdir artifacts/build
    fi
    if [ ! -d "artifacts/initfs" ]; then
    	mkdir artifacts/initfs
    fi

make_dtb: artifact_dir
    @echo Selected DTS {{PLATFORM}}/{{MODEL}}.dts
    @dtc -O dtb -o "{{DTB_DEFAULT}}" "{{DTS}}"

build_user: artifact_dir
    @cd user && RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build --workspace --exclude test_fp --bins {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build"
    @cd user && RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build -p test_fp --target rinlib/riscv64gc-unknown-erhino-elf.json {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build"
    @python3 tools/audit-user-elf.py {{TARGET_DIR}}/build/srv_* {{TARGET_DIR}}/build/drv_* {{TARGET_DIR}}/build/test_*
    @echo -e "\033[0;32mUser space programs build successfully!\033[0m"

make_initfs: build_user
    @rm -rf "{{TARGET_DIR}}/initfs"
    @mkdir -p "{{TARGET_DIR}}/initfs/bin"
    @ditto "{{TARGET_DIR}}/build/srv_pm" "{{TARGET_DIR}}/initfs/bin/srv_pm"
    @ditto "{{TARGET_DIR}}/build/srv_fs" "{{TARGET_DIR}}/initfs/bin/srv_fs"
    @ditto "{{TARGET_DIR}}/build/test_target" "{{TARGET_DIR}}/initfs/bin/test_target"
    @ditto "{{TARGET_DIR}}/build/test_fp" "{{TARGET_DIR}}/initfs/bin/test_fp"
    @ditto "{{TARGET_DIR}}/build/test_hammer" "{{TARGET_DIR}}/initfs/bin/test_hammer"
    @for file in {{TARGET_DIR}}/build/drv_*; do ditto "$file" "{{TARGET_DIR}}/initfs/bin/${file##*/}"; done
    @cd "{{TARGET_DIR}}/initfs" && find . -type f | sed 's|^\./||' | sort | COPYFILE_DISABLE=1 tar --format=ustar -cvf "{{INIT_PAYLOAD}}" -T -

make_boot_package: make_initfs
    @python3 tools/make-boot-package.py --init "{{TARGET_DIR}}/build/srv_init" --payload "{{INIT_PAYLOAD}}" --output "{{BOOT_PACKAGE}}"

build_kernel: artifact_dir
    @echo -e "\033[0;36mBuild kernel: {{PLATFORM}}/{{MODEL}}\033[0m"
    @cd os && CARGO_TARGET_DIR="{{KERNEL_TARGET_DIR}}" ERHINO_MEMORY_SCRIPT="{{MEMORY_SCRIPT}}" RUSTFLAGS="{{RUSTFLAGS_OS}}" cargo build --bin erhino_kernel {{RELEASE}} {{ZFLAGS}} -Z json-target-spec -Z unstable-options --artifact-dir "{{MODEL_DIR}}"
    @riscv64-elf-objcopy {{KERNEL_ELF}} -O binary {{KERNEL_BIN}}
    @python3 os/tools/audit_elf.py {{KERNEL_ELF}}
    @echo -e "\033[0;32mKernel build successfully!\033[0m"

run_qemu +OPTIONS: make_dtb make_boot_package build_kernel
    @echo -e "\033[0;36mQEMU: Simulating (CPU throttled to {{THROTTLE}}%)\033[0m"
    @tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}

run_qemu_dump_dtb:
    @{{QEMU_LAUNCH}} -machine dumpdtb="{{TARGET_DIR}}/dump.dtb"
    @dtc -O dts -o "{{TARGET_DIR}}/dump.dts" -I dtb "{{TARGET_DIR}}/dump.dtb"

virt:
    @QEMU_ACCEPTANCE_PROFILE=common just PLATFORM=qemu MODEL=virt MODE=debug run_qemu_acceptance -smp cores=4

# 多域 eligibility 集成验证：cpu@0 声明无 F/D（内核信 DT，保守正确）→
# Base64-only 域 {0} + D64 域 {1,2,3}。验收：域拓扑快照两行、D64 验收
# 进程只在 FD 域运行（fp verification passed）、全负载收束后显式停机。
virt-hetero:
    @python3 tools/make-hetero-dts.py os/platforms/qemu/virt/device.dts artifacts/virt-hetero.dts 0
    @dtc -O dtb -o artifacts/virt-hetero.dtb artifacts/virt-hetero.dts
    @ERHINO_DTB=artifacts/virt-hetero.dtb QEMU_ACCEPTANCE_PROFILE=hetero just PLATFORM=qemu MODEL=virt MODE=debug run_qemu_acceptance -smp cores=4

# 无兼容域验证：全部 cpu 去掉 F/D，D64 profile 的 test_fp 启动即拒绝
# （NotSupported → init spawn 失败路径），Base64 负载照常收束。
virt-nofd:
    @python3 tools/make-hetero-dts.py os/platforms/qemu/virt/device.dts artifacts/virt-nofd.dts all
    @dtc -O dtb -o artifacts/virt-nofd.dtb artifacts/virt-nofd.dts
    @ERHINO_DTB=artifacts/virt-nofd.dtb QEMU_ACCEPTANCE_PROFILE=nofd just PLATFORM=qemu MODEL=virt MODE=debug run_qemu_acceptance -smp cores=4

# release 验证线（阶段收尾必跑）：debug 代码生成不在 ecall 周边把活值留在
# t 系寄存器，用户侧寄存器保持语义只有 release 能测出（2026-08-28 trap
# 入口 x5 破坏事故，调查档案见 plans/archived/）。通过标准与 virt 同：
# 全负载验收线 + 显式 reset 后 QEMU 退出。
virt-release:
    @QEMU_ACCEPTANCE_PROFILE=common just PLATFORM=qemu MODEL=virt MODE=release run_qemu_acceptance -smp cores=4

# sifive_u 无 shutdown 设备：显式 reset 返回失败后 QEMU 不自退出，wrapper
# 在失败终态锚点出现时主动收割；ACCEPTANCE_TIMEOUT 只兜底真挂死。只有
# 全部验收锚点齐全且无其他失败锚点时，该平台结果才转换为成功。
sifive_u:
    @QEMU_ACCEPTANCE_PROFILE=common just PLATFORM=qemu MODEL=sifive_u MODE=debug run_qemu_acceptance_timed -smp cores=5

# 清理泄漏的孤儿 qemu（PPID=1 残留；agent 裸跑 run_qemu 后易遗漏）。
# 默认一键清理孤儿；传参透传脚本：-l 仅列出，-y 跳过确认，-f 连有父进程的一并杀。
clean-qemu *args:
    @./tools/clean-qemu.sh {{if args == "" { "-y" } else { args }}}

[private]
run_qemu_acceptance_timed +OPTIONS: make_dtb make_boot_package build_kernel
    @echo -e "\033[0;36mQEMU: Simulating acceptance (CPU throttled to {{THROTTLE}}%, hard timeout {{ACCEPTANCE_TIMEOUT}}s)\033[0m"
    @tools/qemu-acceptance.sh --allow-timeout -- timeout --foreground {{ACCEPTANCE_TIMEOUT}} tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}

[private]
run_qemu_acceptance +OPTIONS: make_dtb make_boot_package build_kernel
    @echo -e "\033[0;36mQEMU: Simulating acceptance (CPU throttled to {{THROTTLE}}%)\033[0m"
    @tools/qemu-acceptance.sh -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}

