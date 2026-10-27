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
# 用户态验收负载为 core/stress/memory/device/device-reuse；profile 选择停留在 srv_init，不进入内核 ABI。
ACCEPTANCE_WORKLOAD := env_var_or_default("ERHINO_ACCEPTANCE_WORKLOAD", "core")
INIT_FEATURES := if ACCEPTANCE_WORKLOAD == "device-reuse" { "--features srv_init/acceptance-device-reuse" } else if ACCEPTANCE_WORKLOAD == "memory" { "--features srv_init/acceptance-memory" } else if ACCEPTANCE_WORKLOAD == "stress" { "--features srv_init/acceptance-stress" } else if ACCEPTANCE_WORKLOAD == "device" { "--features srv_init/acceptance-device" } else { "" }

TARGET_DIR := invocation_directory()/"artifacts"
KERNEL_TARGET_DIR := TARGET_DIR/"cargo"/PLATFORM/MODEL

# 平台产物按 MODEL 隔离：切平台不可能读到旧平台的 dtb/内核
MODEL_DIR := TARGET_DIR/PLATFORM/MODEL
KERNEL_ELF := MODEL_DIR/"kernel"
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
# QEMU RAM 必须与平台 DTS 的 /memory 一致；它也决定 firmware 放置运行时 DTB 的上界。
QEMU_MEMORY := if MODEL == "virt" { "1024M" } else { "128M" }
# 与 virt DTS 声明的 Zkr 能力一致；sifive_u 不声明该扩展。
QEMU_CPU := if MODEL == "virt" { "-cpu rv64,zkr=true" } else { "" }
QEMU_DEVICE := if MODEL == "virt" { "-nic none -device qemu-xhci,id=xhci,bus=pcie.0,addr=1,msi=off,msix=off" } else { "" }
QEMU_LAUNCH := "qemu-system-riscv64 -M "+MODEL+" -m "+QEMU_MEMORY+" -nographic -kernel '"+KERNEL_BIN+"' -dtb '"+DTB+"' -device loader,file="+BOOT_PACKAGE+",addr="+BOOT_PACKAGE_ADDR+" " + QEMU_CPU+" "+QEMU_DEVICE
# CPU 节流百分比（tools/qemu-throttle.sh）：保留为跑飞/panic 或异常忙循环时
# 的宿主资源保护；stress 也默认节流，不把它当作全速性能基准。
# 1-99 按比例节流；100 = 全速。默认 50。常规验证保持默认节流，只有需要固定
# 全速条件的专项诊断才显式使用 `THROTTLE=100 just virt-stress`。
# 环境变量会穿透嵌套 just 调用；recipe 参数与 --set 不穿透嵌套子进程。
THROTTLE := env_var_or_default("THROTTLE", "50")
# 各路线 QEMU 运行超时只在本处维护，可用同名环境变量覆盖；调整政策见 plans/BUILD-AND-TEST.md。
VIRT_TIMEOUT := env_var_or_default("VIRT_TIMEOUT", "360")
VIRT_RELEASE_TIMEOUT := env_var_or_default("VIRT_RELEASE_TIMEOUT", "70")
# 两轮热插每轮四个30秒等待边界，加原core收束；仅防挂死，不是性能要求。
VIRT_DEVICE_TIMEOUT := env_var_or_default("VIRT_DEVICE_TIMEOUT", "1120")
VIRT_STRESS_TIMEOUT := env_var_or_default("VIRT_STRESS_TIMEOUT", "420")
VIRT_HETERO_TIMEOUT := env_var_or_default("VIRT_HETERO_TIMEOUT", "90")
VIRT_NOFD_TIMEOUT := env_var_or_default("VIRT_NOFD_TIMEOUT", "180")
SIFIVE_U_TIMEOUT := env_var_or_default("SIFIVE_U_TIMEOUT", "240")
VIRT_BOOT_FAILURE_TIMEOUT := env_var_or_default("VIRT_BOOT_FAILURE_TIMEOUT", "45")
VIRT_IPI_FAILURE_TIMEOUT := env_var_or_default("VIRT_IPI_FAILURE_TIMEOUT", "60")
VIRT_VM_FAULT_TIMEOUT := env_var_or_default("VIRT_VM_FAULT_TIMEOUT", "420")
VIRT_DMA_RETENTION_TIMEOUT := env_var_or_default("VIRT_DMA_RETENTION_TIMEOUT", "720")

# gdb
GDB_BINARY := "riscv64-elf-gdb"
GDB_TARGET := KERNEL_ELF

alias b := build_kernel
alias c := clean

# 内核 target 的 build-std 许可（host 测试走 --target host，不受影响）
ZFLAGS := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"

# 秒级检查：与 build_kernel 同参
check:
    @cd os && cargo check --quiet {{ZFLAGS}}

# 全仓静态 lint：host 纯逻辑 targets 与两套 RISC-V runtime targets 分开，
# 避免把只在目标架构开放的服务接口误编进 host。完整输出保留在 artifacts/lint。
clippy:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p artifacts/lint
    run_lint() {
        local label="$1"
        local log="$2"
        shift 2
        if "$@" >"$log" 2>&1; then
            echo "Clippy passed: $label"
        else
            status=$?
            echo "Clippy failed: $label (full log: $log)" >&2
            # 失败细节由对应日志拥有，终端只报告结果。
            return "$status"
        fi
    }
    run_lint user-device artifacts/lint/user-device.log \
        bash -c 'cd user && cargo clippy -p srv_init --bin srv_init --features acceptance-device-reuse {{ZFLAGS_USER}} -- -D warnings'
    run_lint user-resources artifacts/lint/user-resources.log \
        bash -c 'cd user && cargo clippy -p srv_init -p test_target --bins --features srv_init/acceptance-irq-retirement,srv_init/acceptance-irq-exit,srv_init/acceptance-mmio-publication,srv_init/acceptance-mmio-forced,srv_init/acceptance-dma-races,srv_init/acceptance-dma-shared,srv_init/acceptance-dma-quota,srv_init/acceptance-dma-install-failure {{ZFLAGS_USER}} -- -D warnings'
    run_lint shared-host artifacts/lint/shared.log \
        bash -c 'cd shared && cargo clippy --workspace --all-targets --target aarch64-apple-darwin -- -D warnings'
    run_lint os-host artifacts/lint/os-host.log \
        bash -c 'cd os && cargo clippy --workspace --exclude kernel --all-targets --target aarch64-apple-darwin -- -D warnings'
    run_lint kernel-target artifacts/lint/kernel.log \
        bash -c 'cd os && cargo clippy -p kernel --bin kernel {{ZFLAGS}} -- -D warnings'
    run_lint user-host artifacts/lint/user-host.log \
        bash -c 'cd user && cargo clippy -p libkernel -p libthread -p libruntime -p libbudget -p libexecution -p librpc -p librunnel -p liboal -p libservice -p libdevice -p libpci -p libdriver -p libusb -p libhid -p libprocess -p libfs --all-targets --target aarch64-apple-darwin -- -D warnings'
    run_lint user-target artifacts/lint/user-target.log \
        bash -c 'cd user && cargo clippy --workspace --lib --bins --exclude test_fp {{ZFLAGS_USER}} -- -D warnings'
    run_lint user-stress artifacts/lint/user-stress.log \
        bash -c 'cd user && cargo clippy -p srv_init --bin srv_init --features acceptance-stress {{ZFLAGS_USER}} -- -D warnings'
    run_lint user-memory artifacts/lint/user-memory.log \
        bash -c 'cd user && cargo clippy -p srv_init --bin srv_init --features acceptance-memory {{ZFLAGS_USER}} -- -D warnings'
    run_lint user-fp artifacts/lint/user-fp.log \
        bash -c 'cd user && cargo clippy -p test_fp --bin test_fp --target platforms/riscv64gc-unknown-erhino-elf.json {{ZFLAGS_USER}} -- -D warnings'

clean:
    #!/usr/bin/env bash
    if [ -d "artifacts" ]; then
    	rm -r artifacts
    fi
    cargo clean --manifest-path os/Cargo.toml

artifact_dir:
    @mkdir -p "{{TARGET_DIR}}/build" "{{TARGET_DIR}}/initfs" "{{MODEL_DIR}}"

make_dtb: artifact_dir
    @echo Selected DTS {{PLATFORM}}/{{MODEL}}.dts
    @dtc -O dtb -o "{{DTB_DEFAULT}}" "{{DTS}}"

build_user: artifact_dir
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ACCEPTANCE_WORKLOAD}}" in
        core|stress|memory|device|device-reuse) ;;
        *) echo "Unknown acceptance workload: {{ACCEPTANCE_WORKLOAD}}" >&2; exit 2 ;;
    esac
    cd user
    RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build --workspace --exclude test_fp --exclude test_hammer --bins {{INIT_FEATURES}} {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build"
    if [ "{{ACCEPTANCE_WORKLOAD}}" != core ]; then
        RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build -p test_hammer {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build"
    fi
    RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build -p test_fp --target platforms/riscv64gc-unknown-erhino-elf.json {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build"
    python3 ../tools/audit-user-elf.py "{{TARGET_DIR}}"/build/srv_* "{{TARGET_DIR}}"/build/drv_* "{{TARGET_DIR}}"/build/test_*
    echo -e "\033[0;32mUser space programs build successfully!\033[0m"

make_initfs: build_user
    @rm -rf "{{TARGET_DIR}}/initfs"
    @mkdir -p "{{TARGET_DIR}}/initfs/bin"
    @ditto "{{TARGET_DIR}}/build/srv_pm" "{{TARGET_DIR}}/initfs/bin/srv_pm"
    @ditto "{{TARGET_DIR}}/build/srv_fs" "{{TARGET_DIR}}/initfs/bin/srv_fs"
    @ditto "{{TARGET_DIR}}/build/srv_device" "{{TARGET_DIR}}/initfs/bin/srv_device"
    @ditto "{{TARGET_DIR}}/build/srv_driver" "{{TARGET_DIR}}/initfs/bin/srv_driver"
    @ditto "{{TARGET_DIR}}/build/test_target" "{{TARGET_DIR}}/initfs/bin/test_target"
    @ditto "{{TARGET_DIR}}/build/test_fs" "{{TARGET_DIR}}/initfs/bin/test_fs"
    @ditto "{{TARGET_DIR}}/build/test_device" "{{TARGET_DIR}}/initfs/bin/test_device"
    @ditto "{{TARGET_DIR}}/build/test_fp" "{{TARGET_DIR}}/initfs/bin/test_fp"
    @if [ "{{ACCEPTANCE_WORKLOAD}}" != core ]; then ditto "{{TARGET_DIR}}/build/test_hammer" "{{TARGET_DIR}}/initfs/bin/test_hammer"; fi
    @ditto "{{TARGET_DIR}}/build/drv_pci_generic" "{{TARGET_DIR}}/initfs/bin/drv_pci_generic"
    @ditto "{{TARGET_DIR}}/build/drv_usb_xhci" "{{TARGET_DIR}}/initfs/bin/drv_usb_xhci"
    @ditto "{{TARGET_DIR}}/build/drv_usb_hid" "{{TARGET_DIR}}/initfs/bin/drv_usb_hid"
    # 保留 build 目录的完整 ELF 调试信息，initfs 只承载装载所需部分。
    @for file in "{{TARGET_DIR}}/initfs"/bin/*; do riscv64-elf-strip --strip-debug "$file"; done
    @cd "{{TARGET_DIR}}/initfs" && find . -type f | sed 's|^\./||' | sort | COPYFILE_DISABLE=1 tar --format=ustar -cvf "{{INIT_PAYLOAD}}" -T -

make_boot_package: make_initfs
    @python3 tools/make-boot-package.py --init "{{TARGET_DIR}}/build/srv_init" --payload "{{INIT_PAYLOAD}}" --output "{{BOOT_PACKAGE}}"

build_kernel: artifact_dir
    @echo -e "\033[0;36mBuild kernel: {{PLATFORM}}/{{MODEL}}\033[0m"
    @cd os && CARGO_TARGET_DIR="{{KERNEL_TARGET_DIR}}" ERHINO_MEMORY_SCRIPT="{{MEMORY_SCRIPT}}" RUSTFLAGS="{{RUSTFLAGS_OS}}" cargo build --bin kernel {{RELEASE}} {{ZFLAGS}} -Z json-target-spec -Z unstable-options --artifact-dir "{{MODEL_DIR}}"
    @riscv64-elf-objcopy {{KERNEL_ELF}} -O binary {{KERNEL_BIN}}
    @python3 os/tools/audit_elf.py {{KERNEL_ELF}}
    @echo -e "\033[0;32mKernel build successfully!\033[0m"

run_qemu +OPTIONS: make_dtb make_boot_package build_kernel
    @echo -e "\033[0;36mQEMU: Simulating (CPU throttled to {{THROTTLE}}%)\033[0m"
    @tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}

run_qemu_dump_dtb:
    @{{QEMU_LAUNCH}} -machine dumpdtb="{{TARGET_DIR}}/dump.dtb"
    @dtc -O dts -o "{{TARGET_DIR}}/dump.dts" -I dtb "{{TARGET_DIR}}/dump.dtb"

# 演示在终端直接显示完整输出；非终端和所有测试只报告结果与日志位置。
[private]
output label purpose +COMMAND:
    #!/usr/bin/env bash
    set -uo pipefail
    mode="${ERHINO_OUTPUT:-auto}"
    if [[ "{{purpose}}" == test ]]; then mode=summary; fi
    case "$mode" in
        auto) if [[ -t 1 ]]; then mode=live; else mode=summary; fi ;;
        live|summary) ;;
        *) echo "ERHINO_OUTPUT must be auto, live, or summary" >&2; exit 2 ;;
    esac
    if [[ "$mode" == live ]]; then exec {{COMMAND}}; fi
    mkdir -p artifacts/run
    log="artifacts/run/$(date +%Y%m%d-%H%M%S)-$$-{{label}}.log"
    echo "Starting {{label}}; log: $log"
    if {{COMMAND}} >"$log" 2>&1; then
        if [[ "{{purpose}}" == test ]]; then result=Passed; else result=Completed; fi
        echo "$result {{label}}; log: $log"
    else
        status=$?
        echo "Failed {{label}}: exit $status; log: $log" >&2
        exit "$status"
    fi

# 演示入口直接运行系统，不做验收锚点判定或自动收割。
virt mode="debug":
    @just output virt demo env ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=virt MODE={{mode}} run_qemu -smp cores=4

virt-release:
    @just virt release

# sifive_u 没有可用 shutdown 后端；演示终态由终端用户主动退出。
sifive_u mode="release":
    @just output sifive_u demo env ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=sifive_u MODE={{mode}} run_qemu -smp cores=5

# 自动验收独立于演示入口，始终隐藏内部输出。
acceptance-virt mode="release":
    @just output acceptance-virt-{{mode}} test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE={{mode}} run_qemu_acceptance_bounded {{if mode == "release" { VIRT_RELEASE_TIMEOUT } else { VIRT_TIMEOUT }}} -smp cores=4

acceptance-device mode="release":
    @just output acceptance-device-{{mode}} test env ERHINO_ACCEPTANCE_WORKLOAD=device QEMU_ACCEPTANCE_PROFILE=device just PLATFORM=qemu MODEL=virt MODE={{mode}} run_device_acceptance

# 复用/CPU故障需要正式debug ELF类型信息，仍用同一设备客户端/QMP设施。
acceptance-device-reuse:
    @just output acceptance-device-reuse test env ERHINO_ACCEPTANCE_WORKLOAD=device-reuse QEMU_ACCEPTANCE_PROFILE=device-reuse just PLATFORM=qemu MODEL=virt MODE=debug run_device_acceptance_reuse

acceptance-device-retry-race:
    @just output acceptance-device-retry-race test env ERHINO_ACCEPTANCE_WORKLOAD=device-reuse QEMU_ACCEPTANCE_PROFILE=device-reuse just PLATFORM=qemu MODEL=virt MODE=debug run_device_retry_race

[private]
run_device_retry_race:
    @just --set INIT_FEATURES " --features srv_init/acceptance-device-retry-race" PLATFORM=qemu MODEL=virt MODE=debug run_device_acceptance_reuse false true

# 显式负装配使用同一复用路线与真实PCI/HID映像，不改变正常支持声明。
acceptance-device-candidates:
    @just output acceptance-device-candidates test env ERHINO_ACCEPTANCE_WORKLOAD=device-reuse QEMU_ACCEPTANCE_PROFILE=device-reuse just PLATFORM=qemu MODEL=virt MODE=debug run_device_candidates

[private]
run_device_candidates:
    @just --set INIT_FEATURES " --features srv_init/acceptance-device-candidates" PLATFORM=qemu MODEL=virt MODE=debug run_device_acceptance_reuse true

# 主线程仍在原设备客户端同步wait时，外部fault实际管理者，核独立观察和真实ColdReboot。
acceptance-device-manager-fault manager phase="wait":
    @just output acceptance-device-manager-fault-{{manager}}-{{phase}} test env ERHINO_ACCEPTANCE_WORKLOAD=device just PLATFORM=qemu MODEL=virt MODE=debug run_device_manager_fault {{manager}} {{phase}}

[private]
run_device_manager_fault manager phase: make_dtb make_boot_package build_kernel
    @ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" python3 tools/check-device.py --manager-fault {{manager}} --fault-phase {{phase}} --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 真实受信xHCI执行者fault；原DMA观察体拒绝ColdReboot并核责任保留。
acceptance-device-controller-fault:
    @just output acceptance-device-controller-fault test env ERHINO_ACCEPTANCE_WORKLOAD=device just PLATFORM=qemu MODEL=virt MODE=debug run_device_controller_fault

[private]
run_device_controller_fault: make_dtb make_boot_package build_kernel
    @ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" python3 tools/check-device.py --worker-fault Xhci --reject-reset --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 分操作核已提交业务的回复缺失，真实core装配不增加测试worker/provider。
acceptance-device-unknown operation:
    @just output acceptance-device-unknown-{{operation}} test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_device_unknown {{operation}}

[private]
run_device_unknown operation: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core {{if operation == "Claim" { "" } else if operation == "GetResource" { "" } else { "tools/qemu-acceptance.sh --" }}} python3 tools/check-device.py --unknown {{operation}} --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 同一实际PCI function两轮授权；init复用libusb控制器承担真实硬件停止责任。
acceptance-device-function-session:
    @just output acceptance-device-function-session test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_device_function_config

[private]
run_device_function_config:
    @just --set INIT_FEATURES " --features srv_init/acceptance-device-function-session" PLATFORM=qemu MODEL=virt MODE=debug run_device_function_session

[private]
run_device_function_session: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core tools/qemu-acceptance.sh -- python3 tools/check-device.py --function-session --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

acceptance-irq-retirement:
    @just output acceptance-irq-retirement test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_irq_retirement_config

acceptance-irq-idle:
    @just output acceptance-irq-idle test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_irq_retirement_config true

[private]
run_irq_retirement_config idle="false":
    @just --set INIT_FEATURES " --features srv_init/acceptance-irq-retirement" PLATFORM=qemu MODEL=virt MODE=debug run_irq_retirement {{idle}}

[private]
run_irq_retirement idle="false": make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core tools/qemu-acceptance.sh -- python3 tools/check-device.py --function-session --irq-retirement {{if idle == "true" { "--irq-idle" } else { "" }}} --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 单次平台时钟首因调查；原IRQ消费者，100%调试，两错误路径点，不判IRQ门完成。
investigate-clock:
    @just output investigate-clock test env THROTTLE=100 ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=virt MODE=debug run_clock_failure_config

[private]
run_clock_failure_config:
    @just --set INIT_FEATURES " --features srv_init/acceptance-irq-exit" PLATFORM=qemu MODEL=virt MODE=debug run_clock_failure

[private]
run_clock_failure: make_dtb make_boot_package build_kernel
    @python3 tools/check-clock-failure.py --kernel "{{KERNEL_ELF}}" --init "{{TARGET_DIR}}/build/srv_init" --boot-package "{{BOOT_PACKAGE}}" --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh 100 {{QEMU_LAUNCH}} -smp cores=4

acceptance-irq-exit:
    @just output acceptance-irq-exit test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_irq_exit_config

[private]
run_irq_exit_config:
    @just --set INIT_FEATURES " --features srv_init/acceptance-irq-exit" PLATFORM=qemu MODEL=virt MODE=debug run_irq_exit

[private]
run_irq_exit: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core tools/qemu-acceptance.sh -- python3 tools/check-device.py --irq-exit --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 原32页MMIO消费者的许可前/后Revoke交错及独立Pool退款。
acceptance-mmio-publication:
    @just output acceptance-mmio-publication test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_mmio_publication_config

[private]
run_mmio_publication_config:
    @just --set INIT_FEATURES " --features srv_init/acceptance-mmio-publication" PLATFORM=qemu MODEL=virt MODE=debug run_mmio_publication

[private]
run_mmio_publication: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core tools/qemu-acceptance.sh -- python3 tools/check-device.py --mmio-publication --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

acceptance-dma-races:
    @just output acceptance-dma-races test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_dma_races_config

acceptance-mmio-forced:
    @just output acceptance-mmio-forced test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_mmio_forced_config

[private]
run_mmio_forced_config:
    @just --set INIT_FEATURES " --features srv_init/acceptance-mmio-forced" PLATFORM=qemu MODEL=virt MODE=debug run_mmio_forced

[private]
run_mmio_forced: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core tools/qemu-acceptance.sh -- python3 tools/check-device.py --mmio-forced --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

acceptance-dma-shared:
    @just output acceptance-dma-shared test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_dma_races_config shared

acceptance-dma-quota:
    @just output acceptance-dma-quota test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_dma_races_config quota

acceptance-dma-install-failure:
    @just output acceptance-dma-install-failure test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=virt MODE=debug run_dma_races_config install-failure

[private]
run_dma_races_config scenario="races":
    @just --set INIT_FEATURES " --features srv_init/acceptance-dma-{{scenario}}" PLATFORM=qemu MODEL=virt MODE=debug run_dma_races {{scenario}}

[private]
run_dma_races scenario="races": make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=core tools/qemu-acceptance.sh -- python3 tools/check-device.py --dma-{{scenario}} --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

acceptance-device-reset-rejection:
    @just output acceptance-device-reset-rejection test env ERHINO_ACCEPTANCE_WORKLOAD=device just PLATFORM=qemu MODEL=virt MODE=debug run_device_reset_rejection

[private]
run_device_reset_rejection: make_dtb make_boot_package build_kernel
    @ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" python3 tools/check-device.py --manager-fault Device --reject-reset --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

[private]
run_device_acceptance_reuse candidates="false" retry_race="false": make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=device-reuse tools/qemu-acceptance.sh -- python3 tools/check-device.py --reuse {{if candidates == "true" { "--candidates" } else { "" }}} {{if retry_race == "true" { "--retry-race" } else { "" }}} --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

[private]
run_device_acceptance: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=device tools/qemu-acceptance.sh -- python3 tools/check-device.py --timeout {{VIRT_DEVICE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

acceptance-sifive_u mode="release":
    @just output acceptance-sifive_u test env ERHINO_ACCEPTANCE_WORKLOAD=core QEMU_ACCEPTANCE_PROFILE=core just PLATFORM=qemu MODEL=sifive_u MODE={{mode}} run_qemu_acceptance_platform {{SIFIVE_U_TIMEOUT}} -smp cores=5

# 完整 debug 压力验收：core 的全部语义、自动退休与 16/16 竞态矩阵。
virt-stress:
    @just output virt-stress test env ERHINO_ACCEPTANCE_WORKLOAD=stress QEMU_ACCEPTANCE_PROFILE=stress just PLATFORM=qemu MODEL=virt MODE=debug run_qemu_acceptance_bounded {{VIRT_STRESS_TIMEOUT}} -smp cores=4

# 多域验收：Base64-only 域 {0} 与 D64 域 {1,2,3}。
virt-hetero mode="release":
    @just output virt-hetero test just run_virt_hetero {{mode}}

[private]
run_virt_hetero mode: artifact_dir
    @python3 tools/make-hetero-dts.py os/platforms/qemu/virt/device.dts artifacts/virt-hetero.dts 0
    @dtc -O dtb -o artifacts/virt-hetero.dtb artifacts/virt-hetero.dts
    @ERHINO_ACCEPTANCE_WORKLOAD=core ERHINO_DTB=artifacts/virt-hetero.dtb QEMU_ACCEPTANCE_PROFILE=hetero just PLATFORM=qemu MODEL=virt MODE={{mode}} run_qemu_acceptance_bounded {{VIRT_HETERO_TIMEOUT}} -smp cores=4

# 无兼容域验收：D64 test_fp 拒绝启动，Base64 负载照常收束。
virt-nofd mode="release":
    @just output virt-nofd test just run_virt_nofd {{mode}}

[private]
run_virt_nofd mode: artifact_dir
    @python3 tools/make-hetero-dts.py os/platforms/qemu/virt/device.dts artifacts/virt-nofd.dts all
    @dtc -O dtb -o artifacts/virt-nofd.dtb artifacts/virt-nofd.dts
    @ERHINO_ACCEPTANCE_WORKLOAD=core ERHINO_DTB=artifacts/virt-nofd.dtb QEMU_ACCEPTANCE_PROFILE=nofd just PLATFORM=qemu MODEL=virt MODE={{mode}} run_qemu_acceptance_bounded {{VIRT_NOFD_TIMEOUT}} -smp cores=4

# 外部调试器在全员 Online / Ready 前注入失败，核验 Gate 与每个 hart 停驻。
virt-boot-failure:
    @just output virt-boot-failure test env ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=virt MODE=debug run_boot_failure

[private]
run_boot_failure: make_dtb make_boot_package build_kernel
    @python3 tools/check-boot-failure.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_BOOT_FAILURE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

virt-platform-funding scenario="all":
    @just output virt-platform-funding test env ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=virt MODE=debug run_platform_funding {{scenario}}

[private]
run_platform_funding scenario="all": make_dtb make_boot_package build_kernel
    @python3 tools/check-boot-failure.py --platform-funding --funding-case {{scenario}} --kernel "{{KERNEL_ELF}}" --boot-package "{{BOOT_PACKAGE}}" --timeout {{VIRT_BOOT_FAILURE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 真实已发布责任的门铃失败验收。
virt-ipi-failure:
    @just output virt-ipi-failure test env ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=virt MODE=debug run_ipi_failure

[private]
run_ipi_failure: make_dtb make_boot_package build_kernel
    @python3 tools/check-ipi-failure.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_IPI_FAILURE_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 只运行既有线程/映射消费者及完整收束，必须命中真实发布窗口 fault 等待。
virt-vm-fault:
    @just output virt-vm-fault test env ERHINO_ACCEPTANCE_WORKLOAD=memory just PLATFORM=qemu MODEL=virt MODE=debug run_vm_fault

[private]
run_vm_fault: make_dtb make_boot_package build_kernel
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" QEMU_ACCEPTANCE_PROFILE=memory tools/qemu-acceptance.sh -- python3 tools/check-vm-fault.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_VM_FAULT_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 模拟入口故障、保留真实 PTE 与用户现场的修复验收。
virt-vm-repair:
    @just output virt-vm-repair test env ERHINO_ACCEPTANCE_WORKLOAD=core just PLATFORM=qemu MODEL=virt MODE=debug run_vm_repair

[private]
run_vm_repair: make_dtb make_boot_package build_kernel
    @python3 tools/check-vm-repair.py --kernel "{{KERNEL_ELF}}" -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 真实 DMA 静态保留、reset 失败与 init DONE；调试器全速运行，不作为性能证据。
virt-dma-retention:
    @just output virt-dma-retention test env ERHINO_ACCEPTANCE_WORKLOAD=core THROTTLE=100 just PLATFORM=qemu MODEL=virt MODE=debug run_dma_retention

[private]
run_dma_retention: make_dtb make_boot_package build_kernel
    @python3 tools/check-dma-retention.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_DMA_RETENTION_TIMEOUT}} -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 同一实际pin/停止观察，另核一次request广播IPI失败时原DMA责任不消散。
virt-dma-ipi-retention:
    @just output virt-dma-ipi-retention test env ERHINO_ACCEPTANCE_WORKLOAD=core THROTTLE=100 just PLATFORM=qemu MODEL=virt MODE=debug run_dma_ipi_retention

[private]
run_dma_ipi_retention: make_dtb make_boot_package build_kernel
    @python3 tools/check-dma-retention.py --kernel "{{KERNEL_ELF}}" --boot-package "{{BOOT_PACKAGE}}" --timeout {{VIRT_DMA_RETENTION_TIMEOUT}} --ipi-failure --logs artifacts/dma-ipi-retention -- tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} -smp cores=4

# 常规跨机制回归：静态门、无调试器的完整 debug 压力和 release core。
acceptance-core:
    @just clippy
    @just virt-stress
    @just acceptance-virt

# 平台差异仍携带完整 core；各条验收可显式选择 debug。
acceptance-platforms:
    @just acceptance-sifive_u
    @just virt-nofd

acceptance-faults:
    @just virt-boot-failure
    @just virt-ipi-failure
    @just virt-vm-fault
    @just virt-vm-repair
    @just virt-dma-retention

# 完整验收只组合测试入口，不调用演示入口。
acceptance:
    @just acceptance-core
    @just acceptance-platforms
    @just acceptance-faults

# 清理泄漏的孤儿 qemu（PPID=1 残留；agent 裸跑 run_qemu 后易遗漏）。
# 默认一键清理孤儿；传参透传脚本：-l 仅列出，-y 跳过确认，-f 连有父进程的一并杀。
clean-qemu *args:
    @./tools/clean-qemu.sh {{if args == "" { "-y" } else { args }}}

[private]
run_qemu_acceptance_platform timeout +OPTIONS: make_dtb make_boot_package build_kernel
    @echo -e "\033[0;36mQEMU: Simulating acceptance ({{ACCEPTANCE_WORKLOAD}}, CPU throttled to {{THROTTLE}}%, hard timeout {{timeout}}s)\033[0m"
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" tools/qemu-acceptance.sh --allow-timeout -- timeout --foreground {{timeout}} tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}

[private]
run_qemu_acceptance_bounded timeout +OPTIONS: make_dtb make_boot_package build_kernel
    @echo -e "\033[0;36mQEMU: Simulating acceptance ({{ACCEPTANCE_WORKLOAD}}, CPU throttled to {{THROTTLE}}%, hard timeout {{timeout}}s)\033[0m"
    @PLATFORM="{{PLATFORM}}" MODEL="{{MODEL}}" MODE="{{MODE}}" ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}" tools/qemu-acceptance.sh -- timeout --foreground {{timeout}} tools/qemu-throttle.sh {{THROTTLE}} {{QEMU_LAUNCH}} {{OPTIONS}}
