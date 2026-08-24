# rust
MODE := "debug"
RELEASE := if MODE == "release" { "--release" } else { "" }

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
DTB := MODEL_DIR/"device.dtb"
INITFS := TARGET_DIR/"initfs.tar"   # 用户态产物，平台无关

# QEMU
# initfs 装载地址：virt DRAM 1GiB 取高址；sifive_u 实际 DRAM 仅 128MiB，
# 取 dtb 声明的 0x86000000（与 dts 的 initfs reg 保持一致）。
INITFS_ADDR := if MODEL == "virt" { "0xB0000000" } else { "0x86000000" }
# virt 开启 Zkr（seed CSR 硬件熵源，rand 模块主路）；sifive_u 平台无此扩展。
QEMU_CPU := if MODEL == "virt" { "-cpu rv64,zkr=true" } else { "" }
QEMU_LAUNCH := "qemu-system-riscv64 -M "+MODEL+" -m 1024M -nographic -kernel '"+KERNEL_BIN+"' -dtb '"+DTB+"' -device loader,file="+INITFS+",addr="+INITFS_ADDR+" " + QEMU_CPU
# CPU 节流百分比（tools/qemu-throttle.sh）：跑飞/panic 时 QEMU 满核空转的兜底。
# 1-99 按比例节流；100 = 全速。默认 50，全速需显式 THROTTLE=100。
THROTTLE := "50"

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
    @dtc -O dtb -o "{{DTB}}" "{{DTS}}"

build_user: artifact_dir
    @cd user && RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build --bins {{RELEASE}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build"
    @echo -e "\033[0;32mUser space programs build successfully!\033[0m"

make_initfs: build_user
    @mkdir -p "{{TARGET_DIR}}/initfs/bin"
    @cp {{TARGET_DIR}}/build/srv_* "{{TARGET_DIR}}/initfs/bin"
    @cp {{TARGET_DIR}}/build/drv_* "{{TARGET_DIR}}/initfs/bin"
    @cd "{{TARGET_DIR}}/initfs" && find . -type f | sed 's|^\./||' | sort | tar --format=ustar -cvf "{{INITFS}}" -T -

build_kernel: artifact_dir
    @echo -e "\033[0;36mBuild kernel: {{PLATFORM}}/{{MODEL}}\033[0m"
    @cd os && CARGO_TARGET_DIR="{{KERNEL_TARGET_DIR}}" ERHINO_MEMORY_SCRIPT="{{MEMORY_SCRIPT}}" RUSTFLAGS="{{RUSTFLAGS_OS}}" cargo build --bin erhino_kernel {{RELEASE}} {{ZFLAGS}} -Z json-target-spec -Z unstable-options --artifact-dir "{{MODEL_DIR}}"
    @riscv64-elf-objcopy {{KERNEL_ELF}} -O binary {{KERNEL_BIN}}
    @python3 os/tools/audit_elf.py {{KERNEL_ELF}}
    @echo -e "\033[0;32mKernel build successfully!\033[0m"

run_qemu +OPTIONS: make_dtb make_initfs build_kernel
    @echo -e "\033[0;36mQEMU: Simulating (CPU throttled to {{THROTTLE}}%)\033[0m"
    @tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}

run_qemu_dump_dtb:
    @{{QEMU_LAUNCH}} -machine dumpdtb="{{TARGET_DIR}}/dump.dtb"
    @dtc -O dts -o "{{TARGET_DIR}}/dump.dts" -I dtb "{{TARGET_DIR}}/dump.dtb"

virt:
    @just PLATFORM=qemu MODEL=virt MODE=debug run_qemu -smp cores=4

# sifive_u 无 shutdown 设备：负载完成后 QEMU 不自退出，运行阶段以 timeout
# 收束（AGENTS.md：运行阶段硬上限 10s）；通过与否以日志关键行人工判定
# （全员回收 / [Sched] system quiescent）。
sifive_u:
    @just PLATFORM=qemu MODEL=sifive_u MODE=debug run_qemu_timed -smp cores=5

# 清理泄漏的孤儿 qemu（PPID=1 残留；agent 裸跑 run_qemu 后易遗漏）。
# 默认一键清理孤儿；传参透传脚本：-l 仅列出，-y 跳过确认，-f 连有父进程的一并杀。
clean-qemu *args:
    @./tools/clean-qemu.sh {{if args == "" { "-y" } else { args }}}

[private]
run_qemu_timed +OPTIONS: make_dtb build_kernel
    #!/usr/bin/env bash
    set +e
    timeout --foreground 4 {{QEMU_LAUNCH}} {{OPTIONS}}
    code=$?
    if [ "$code" -eq 124 ]; then
        echo -e "\033[0;33msifive_u: run phase timed out (platform has no shutdown device); verify key log lines above\033[0m"
        exit 0
    fi
    exit "$code"
