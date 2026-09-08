#!/usr/bin/env python3
"""以外部 GDB 注入验证 Ready 前失败广播，不在内核中加入测试开关。"""

import argparse
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys


def symbols(kernel):
    output = subprocess.check_output(
        ["riscv64-elf-nm", "-C", "--defined-only", str(kernel)], text=True
    )
    result = {}
    for line in output.splitlines():
        fields = line.split(maxsplit=2)
        if len(fields) == 3:
            result[fields[2]] = int(fields[0], 16)
    return result


def stop_group(process):
    # throttle wrapper 及其 QEMU 必须一起清理，即使调试器超时。
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def run_case(mode, kernel, addresses, command, logs, timeout):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    load = addresses["erhino_kernel::boot::load"]
    park = addresses["erhino_kernel::hart::park"]
    gate = addresses["erhino_kernel::registry::GATE"]
    allocation = addresses["erhino_kernel::rt::handle_alloc_error"]
    # 在 boot::load 函数入口（尚无 prologue）注入：坏 envelope 长度、Layout
    # 分配失败 handler、真实非法指令 S-mode trap。Layout 两个机器字均为 8，
    # 不依赖其内部 size/alignment 字段次序。ABI 由钉住的 debug binary 决定。
    injection = {
        "panic": "set $a1 = 0",
        "alloc": f"set $a0 = 8\nset $a1 = 8\nset $pc = {allocation:#x}",
        "fatal": "set {unsigned int}$pc = 0",
    }[mode]
    script = f"""set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel}"
target remote 127.0.0.1:{port}
hbreak *{load:#x}
continue
delete breakpoints
if *(unsigned char*){gate:#x} != 0
  echo Runtime gate was not Preparing at injection.\\n
  quit 1
end
{injection}
set $parked = 0
hbreak *{park:#x}
commands
  silent
  if *(unsigned char*){gate:#x} != 2
    echo Runtime gate was not Failed before parking.\\n
    quit 1
  end
  printf "PARKED thread=%d gate=2\\n", $_thread
  set $parked = $parked + 1
  if $parked == 4
    detach
    quit 0
  end
  continue
end
continue
"""
    gdb_file = logs / f"{mode}.gdb"
    gdb_file.write_text(script)
    qemu_log = logs / f"{mode}-qemu.log"
    gdb_log = logs / f"{mode}-gdb.log"
    with qemu_log.open("w") as qlog, gdb_log.open("w") as glog:
        process = subprocess.Popen(
            command + ["-S", "-gdb", f"tcp:127.0.0.1:{port}"],
            stdout=qlog, stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            completed = subprocess.run(
                ["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(gdb_file)],
                stdout=glog, stderr=subprocess.STDOUT, timeout=timeout,
            )
        finally:
            stop_group(process)
    output = gdb_log.read_text()
    parked = re.findall(r"PARKED thread=(\d+) gate=2", output)
    serial = qemu_log.read_text(errors="replace")
    expected = {
        "panic": "BootPackage envelope is invalid",
        "alloc": "heap allocation error",
        "fatal": "fatal trap",
    }[mode]
    if completed.returncode or len(parked) != 4 or len(set(parked)) != 4 or expected not in serial:
        raise RuntimeError(f"{mode}: incomplete failure/park evidence; logs: {gdb_log}, {qemu_log}")
    print(f"Boot failure passed: {mode}; all 4 harts parked with Gate Failed")


def main():
    parser = argparse.ArgumentParser(description="Inject pre-Ready failures and verify all harts park after Gate Failed.")
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--logs", type=Path, default=Path("artifacts/boot-failure"))
    parser.add_argument("--timeout", type=int, default=45)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("QEMU command is required")
    args.logs.mkdir(parents=True, exist_ok=True)
    addresses = symbols(args.kernel)
    for mode in ("panic", "alloc", "fatal"):
        run_case(mode, args.kernel, addresses, command, args.logs, args.timeout)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.SubprocessError, KeyError) as error:
        print(f"Boot failure validation failed: {error}", file=sys.stderr)
        sys.exit(1)
