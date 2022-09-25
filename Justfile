MODE := "debug"
RELEASE := if MODE == "release" { "--release" } else { "" }
BOARD := "qemu"
TARGET := "riscv64gc-unknown-none-elf"
KERNEL_ELF := "boards/target"/TARGET/MODE/"board_"+BOARD
KERNEL_BIN := "boards/target"/TARGET/MODE/"board_"+BOARD+".bin"
INIT_ELF := "user/target"/TARGET/MODE/"kzios_init0"
RUSTFLAGS_KERNEL := "'-Clink-arg=-T"+invocation_directory()+"/boards"/BOARD/"linker.ld -Cforce-frame-pointers=yes'"
RUSTFLAGS_INIT := "-Clink-arg=-T"+invocation_directory()+"/user/kinlib/linker.ld"

# aliases
alias b := build
alias c := clean
alias d := debug_qemu
alias r := run

# tools
OBJCOPY := "rust-objcopy"
K210_FLASH := "kflash"

# k210

# qemu
QEMU_MEMORY := "6m"
QEMU_CORES := "1"

QEMU_LAUNCH := "qemu-system-riscv64 -smp cores="+QEMU_CORES+" -M "+QEMU_MEMORY+" -machine virt -nographic -bios none -kernel "+KERNEL_ELF


default:
    @just --list
    

artifact_dir:
    #!/usr/bin/env sh
    if [ !-d "artfacts" ]; then
        mkdir artifacts
    fi

build_init: artifact_dir
    @cd user && RUSTFLAGS={{RUSTFLAGS_INIT}} cargo build {{RELEASE}}
    @cp {{INIT_ELF}} artifacts/

build_os: artifact_dir build_init # 暂时需要, 未来就不要求了
    @cd boards && RUSTFLAGS={{RUSTFLAGS_KERNEL}} cargo build {{RELEASE}}
    @{{OBJCOPY}} --strip-all {{KERNEL_ELF}} -O binary {{KERNEL_BIN}}
    @cp {{KERNEL_ELF}} artifacts/
    @cp {{KERNEL_BIN}} artifacts/

build: build_init build_os

debug_qemu EXPOSE="-s -S": build
    @{{QEMU_LAUNCH}} {{EXPOSE}}

debug_local: build
    @tmux new-session -d "{{QEMU_LAUNCH}} -s -S" && tmux split-window -h "riscv64-elf-gdb -ex 'file {{KERNEL_ELF}}' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'" && tmux -2 attach-session -d

run: (debug_qemu "")

clean:
    #!/usr/bin/env sh
    echo Cleaning boards workspace...
    cd user && cargo clean
    echo Cleaning user workspace...
    cd ../boards && cargo clean
    cd ..
    echo Removing artifacts...
    if [ -d "artfacts" ]; then
        rm -r artifacts
    fi
    echo Done!