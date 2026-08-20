# rust
MODE := "debug"
RELEASE := if MODE == "release" { "--release" } else { "" }

# platform
PLATFORM := "qemu"
MODEL := "sifive_u"
DTS := invocation_directory()/"os/platforms"/PLATFORM/MODEL/"device.dts"
MEMORY_SCRIPT := invocation_directory()/"os/platforms"/PLATFORM/MODEL/"memory.x"

# compile
RUSTFLAGS_OS := "-Clink-arg=-Tplatforms/linker.ld -Clinker=riscv64-elf-ld"
RUSTFLAGS_USER := ""

TARGET_DIR := invocation_directory()/"artifacts"

KERNEL_ELF := TARGET_DIR/"erhino_kernel"
KERNEL_BIN := KERNEL_ELF+".bin"

DTB := TARGET_DIR/"device.dtb"

# QEMU
QEMU_LAUNCH := "qemu-system-riscv64 -M "+MODEL+" -m 1024M -nographic -kernel '"+KERNEL_BIN+"' -dtb '"+DTB+"' -device loader,file=artifacts/initfs.tar,addr=0xB0000000"

# gdb
GDB_BINARY := "riscv64-elf-gdb"
GDB_TARGET := KERNEL_ELF

alias b := build_kernel
alias c := clean

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
    @cd "{{TARGET_DIR}}/initfs" && find . -type f | sed 's|^\./||' | tar --format=ustar -cvf ../initfs.tar -T -

build_kernel: 
    @echo -e "\033[0;36mBuild kernel: {{PLATFORM}}\033[0m"
    @cp "{{MEMORY_SCRIPT}}" "{{TARGET_DIR}}"
    @cd os && RUSTFLAGS="{{RUSTFLAGS_OS}}" cargo build --bin erhino_kernel {{RELEASE}} -Z unstable-options --artifact-dir {{TARGET_DIR}}
    @riscv64-elf-objcopy {{KERNEL_ELF}} -O binary {{KERNEL_BIN}}
    @echo -e "\033[0;32mKernel build successfully!\033[0m"

run_qemu +OPTIONS: make_dtb make_initfs build_kernel
    @echo -e "\033[0;36mQEMU: Simulating\033[0m"
    @{{QEMU_LAUNCH}} {{OPTIONS}}

run_qemu_dump_dtb:
    @{{QEMU_LAUNCH}} -machine dumpdtb="{{TARGET_DIR}}/dump.dtb"
    @dtc -O dts -o "{{TARGET_DIR}}/dump.dts" -I dtb "{{TARGET_DIR}}/dump.dtb"

virt:
    @just PLATFORM=qemu MODEL=virt MODE=debug run_qemu -smp cores=4

sifive_u:
    @just PLATFORM=qemu MODEL=sifive_u MODE=debug run_qemu -smp cores=5
