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
# 组态源由统一验收目录选择；直接演示默认正式 System。
FORMATION_SOURCES := env_var_or_default("ERHINO_FORMATION_SOURCES", "system")
HOST_TARGET := `rustc -vV | awk '/^host:/ {print $2}'`

TARGET_DIR := invocation_directory()/"artifacts"
KERNEL_TARGET_DIR := TARGET_DIR/"cargo"/PLATFORM/MODEL

# 平台产物按 MODEL 隔离：切平台不可能读到旧平台的 dtb/内核
MODEL_DIR := TARGET_DIR/PLATFORM/MODEL
KERNEL_ELF := MODEL_DIR/"kernel"
KERNEL_BIN := KERNEL_ELF+".bin"
# 官方 DTB 固定生成路径；ERHINO_DTB 只覆盖 QEMU 载入的 -dtb（异构
# 域变体用，见 virt-hetero/virt-nofd），不重定向 make_dtb 的产出。
DTB_DEFAULT := MODEL_DIR/"device.dtb"
DTB := if env_var_or_default("ERHINO_DTB", "") == "" { DTB_DEFAULT } else { env_var("ERHINO_DTB") }
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
# 资源观察沿用原专项预算；只防挂死，不是性能要求。
VIRT_DEVICE_TIMEOUT := env_var_or_default("VIRT_DEVICE_TIMEOUT", "1120")
VIRT_STRESS_TIMEOUT := env_var_or_default("VIRT_STRESS_TIMEOUT", "420")
VIRT_HETERO_TIMEOUT := env_var_or_default("VIRT_HETERO_TIMEOUT", "90")
VIRT_NOFD_TIMEOUT := env_var_or_default("VIRT_NOFD_TIMEOUT", "180")
SIFIVE_U_TIMEOUT := env_var_or_default("SIFIVE_U_TIMEOUT", "240")
VIRT_BOOT_FAILURE_TIMEOUT := env_var_or_default("VIRT_BOOT_FAILURE_TIMEOUT", "45")
VIRT_IPI_FAILURE_TIMEOUT := env_var_or_default("VIRT_IPI_FAILURE_TIMEOUT", "60")
VIRT_VM_FAULT_TIMEOUT := env_var_or_default("VIRT_VM_FAULT_TIMEOUT", "420")
VIRT_DMA_RETENTION_TIMEOUT := env_var_or_default("VIRT_DMA_RETENTION_TIMEOUT", "720")

alias b := build_kernel
alias c := clean

# 内核 target 的 build-std 许可（host 测试走 --target host，不受影响）
ZFLAGS := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"

# 选择 package 或分面；无参数只列出选择，all 为显式全仓静态检查。
check scope="":
    @python3 tools/checks.py check {{scope}}

clippy scope="":
    @python3 tools/checks.py clippy {{scope}}

test-host package *filters:
    @python3 tools/checks.py test {{package}} {{filters}}

clean:
    @python3 tools/clean-build.py

clean-preview:
    @python3 tools/clean-build.py --dry-run

artifact_dir:
    @mkdir -p "{{TARGET_DIR}}/build" "{{TARGET_DIR}}/initfs" "{{MODEL_DIR}}"

make_dtb: artifact_dir
    @echo Selected DTS {{PLATFORM}}/{{MODEL}}.dts
    @dtc -O dtb -o "{{DTB_DEFAULT}}" "{{DTS}}"

# 构建前通过同一组态编译器列出引用映像，只编译当前组和必需 init。
build_user: artifact_dir
    #!/usr/bin/env bash
    set -euo pipefail
    sources=()
    for name in {{FORMATION_SOURCES}}; do sources+=("../user/formations/$name.toml"); done
    (cd shared && cargo run -q -p formation --features host --target {{HOST_TARGET}} -- --list "${sources[@]}") > "{{TARGET_DIR}}/build/selected.images"
    packages=(-p srv_init)
    images=("{{TARGET_DIR}}/build/srv_init")
    fp=false
    while read -r component; do
        images+=("{{TARGET_DIR}}/build/$component")
        if [[ "$component" == test_fp ]]; then fp=true; else packages+=(-p "$component"); fi
    done < "{{TARGET_DIR}}/build/selected.images"
    (cd user && RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build "${packages[@]}" --bins {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build")
    if $fp; then
        (cd user && RUSTFLAGS="{{RUSTFLAGS_USER}}" cargo build -p test_fp --target platforms/riscv64gc-unknown-erhino-elf.json {{RELEASE}} {{ZFLAGS_USER}} -Z unstable-options --artifact-dir "{{TARGET_DIR}}/build")
    fi
    python3 tools/audit-user-elf.py "${images[@]}"

# 编译所选组态并按引用清单组装 initfs；strip 只作用于归档副本。
make_initfs: build_user
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf "{{TARGET_DIR}}/initfs"
    mkdir -p "{{TARGET_DIR}}/initfs/image"
    sources=()
    for name in {{FORMATION_SOURCES}}; do sources+=("../user/formations/$name.toml"); done
    (cd shared && cargo run -q -p formation --features host --target {{HOST_TARGET}} -- "{{TARGET_DIR}}/build" "{{TARGET_DIR}}/initfs/formation" "${sources[@]}")
    while read -r component; do
        ditto "{{TARGET_DIR}}/build/$component" "{{TARGET_DIR}}/initfs/image/$component"
        riscv64-elf-strip --strip-all "{{TARGET_DIR}}/initfs/image/$component"
    done < "{{TARGET_DIR}}/initfs/formation.images"
    rm "{{TARGET_DIR}}/initfs/formation.images"
    cd "{{TARGET_DIR}}/initfs" && find . -type f | sed 's|^\./||' | sort | COPYFILE_DISABLE=1 tar --format=ustar -cf "{{INIT_PAYLOAD}}" -T -

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
[positional-arguments]
output label purpose +COMMAND:
    #!/usr/bin/env bash
    set -uo pipefail
    shift 2
    mode="${ERHINO_OUTPUT:-auto}"
    if [[ "{{purpose}}" == test ]]; then mode=summary; fi
    case "$mode" in
        auto) if [[ -t 1 ]]; then mode=live; else mode=summary; fi ;;
        live|summary) ;;
        *) echo "ERHINO_OUTPUT must be auto, live, or summary" >&2; exit 2 ;;
    esac
    if [[ "$mode" == live ]]; then exec "$@"; fi
    mkdir -p artifacts/run
    log="artifacts/run/$(date +%Y%m%d-%H%M%S)-$$-{{label}}.log"
    echo "Starting {{label}}; log: $log"
    if "$@" >"$log" 2>&1; then
        if [[ "{{purpose}}" == test ]]; then result=Passed; else result=Completed; fi
        echo "$result {{label}}; log: $log"
    else
        status=$?
        echo "Failed {{label}}: exit $status; log: $log" >&2
        exit "$status"
    fi

# 演示不进行自动判定，默认 System 常驻至操作者退出。
virt mode="debug":
    @just output virt demo just PLATFORM=qemu MODEL=virt MODE={{mode}} run_qemu -smp cores=4

system mode="release":
    @env ERHINO_FORMATION_SOURCES=system just virt {{mode}}

sifive_u mode="release":
    @just output sifive_u demo just PLATFORM=qemu MODEL=sifive_u MODE={{mode}} run_qemu -smp cores=5

# 无参数只列分组；all 也不包含压力与故障注入。
acceptance *selection:
    @python3 tools/acceptance.py {{selection}}

[private]
run_acceptance observer: make_dtb make_boot_package build_kernel
    #!/usr/bin/env bash
    set -euo pipefail
    export ERHINO_KERNEL_ELF="{{KERNEL_ELF}}" ERHINO_BOOT_PACKAGE="{{BOOT_PACKAGE}}"
    dtb="{{DTB}}"
    variant="${ERHINO_ACCEPTANCE_PLATFORM:-}"
    if [[ "$variant" == hetero || "$variant" == nofd ]]; then
        disabled=0
        if [[ "$variant" == nofd ]]; then disabled=all; fi
        python3 tools/make-hetero-dts.py "{{DTS}}" "{{MODEL_DIR}}/$variant.dts" "$disabled"
        dtc -O dtb -o "{{MODEL_DIR}}/$variant.dtb" "{{MODEL_DIR}}/$variant.dts"
        dtb="{{MODEL_DIR}}/$variant.dtb"
    fi
    cores=4
    if [[ "{{MODEL}}" == sifive_u ]]; then cores=5; fi
    command=(tools/qemu-throttle.sh {{THROTTLE}} qemu-system-riscv64 -M {{MODEL}} -m {{QEMU_MEMORY}} -nographic -kernel "{{KERNEL_BIN}}" -dtb "$dtb" -device loader,file={{BOOT_PACKAGE}},addr={{BOOT_PACKAGE_ADDR}} {{QEMU_CPU}} {{QEMU_DEVICE}} -smp cores=$cores)
    budget={{if MODE == "release" { VIRT_RELEASE_TIMEOUT } else { VIRT_TIMEOUT }}}
    if [[ "${ERHINO_ACCEPTANCE_MODE:-0}" != 0 ]]; then budget={{VIRT_STRESS_TIMEOUT}}; fi
    case "{{observer}}" in
        normal) ;;
        vm-fault)
            budget={{VIRT_VM_FAULT_TIMEOUT}}
            command=(python3 tools/check-vm-fault.py --kernel "{{KERNEL_ELF}}" --timeout "$budget" -- "${command[@]}") ;;
        irq-exit|mmio-publication|mmio-forced|dma-races|dma-shared|dma-quota|dma-install-failure)
            budget={{VIRT_DEVICE_TIMEOUT}}
            command=(python3 tools/check-resources.py --{{observer}} --timeout "$budget" -- "${command[@]}") ;;
        boot-failure)
            exec python3 tools/check-boot-failure.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_BOOT_FAILURE_TIMEOUT}} -- "${command[@]}" ;;
        platform-funding)
            exec python3 tools/check-boot-failure.py --platform-funding --funding-case all --kernel "{{KERNEL_ELF}}" --boot-package "{{BOOT_PACKAGE}}" --timeout {{VIRT_BOOT_FAILURE_TIMEOUT}} -- "${command[@]}" ;;
        ipi-failure)
            exec python3 tools/check-ipi-failure.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_IPI_FAILURE_TIMEOUT}} -- "${command[@]}" ;;
        clock)
            exec python3 tools/check-clock-failure.py --kernel "{{KERNEL_ELF}}" --init "{{TARGET_DIR}}/build/srv_init" --boot-package "{{BOOT_PACKAGE}}" --timeout {{VIRT_DEVICE_TIMEOUT}} -- "${command[@]}" ;;
        vm-repair)
            exec python3 tools/check-vm-repair.py --kernel "{{KERNEL_ELF}}" --timeout {{VIRT_VM_FAULT_TIMEOUT}} -- "${command[@]}" ;;
        dma-retention|dma-ipi-retention)
            options=()
            if [[ "{{observer}}" == dma-ipi-retention ]]; then options+=(--ipi-failure); fi
            exec python3 tools/check-dma-retention.py --kernel "{{KERNEL_ELF}}" --boot-package "{{BOOT_PACKAGE}}" --timeout {{VIRT_DMA_RETENTION_TIMEOUT}} "${options[@]}" -- "${command[@]}" ;;
        *) echo "Unknown acceptance observer: {{observer}}" >&2; exit 2 ;;
    esac
    options=()
    if [[ "{{MODEL}}" == sifive_u ]]; then budget={{SIFIVE_U_TIMEOUT}}; options+=(--allow-reset-failure); fi
    if [[ "$variant" == hetero ]]; then budget={{VIRT_HETERO_TIMEOUT}}; fi
    if [[ "$variant" == nofd ]]; then budget={{VIRT_NOFD_TIMEOUT}}; fi
    exec python3 tools/qemu-acceptance.py --timeout "$budget" "${options[@]}" -- "${command[@]}"

clean-qemu *args:
    @./tools/clean-qemu.sh {{if args == "" { "-y" } else { args }}}
