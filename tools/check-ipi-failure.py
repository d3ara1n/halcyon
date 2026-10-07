#!/usr/bin/env python3
"""在真实已发布责任的IPI ecall跳过一次发送，核无期限WFI目标醒来先停驻。"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys

import qemu_gdb as boot


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--logs", type=Path, default=Path("artifacts/ipi-failure"))
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("QEMU command is required")
    args.logs = boot.fresh_logs(args.logs)
    addresses = boot.symbols(args.kernel)
    start = addresses["kernel::sbi::sbi_call"]
    end = min(address for address in addresses.values() if address > start)
    asm = subprocess.check_output([
        "riscv64-elf-objdump", "-d", f"--start-address={start}",
        f"--stop-address={end}", str(args.kernel),
    ], text=True)
    ecalls = re.findall(r"^\s*([0-9a-f]+):\s+00000073\s+ecall", asm, re.M)
    if len(ecalls) != 1:
        raise RuntimeError("fixed SBI wrapper must expose one ecall for injection")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    script = r'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "__KERNEL__"
target remote 127.0.0.1:__PORT__
python
import gdb
injected = False
parked = set()
failed = None
target_number = None
REQ = __REQ__
STOP = __STOP__
DEADLINES = __DEADLINES__
__PHYSICAL_READ__

def word(address, size=8):
    return int.from_bytes(gdb.selected_inferior().read_memory(address, size).tobytes(), "little")

class Injection(gdb.Breakpoint):
    def stop(self):
        global injected, target_number
        if injected or int(gdb.parse_and_eval("$a7")) != 0x735049 or physical_word(REQ, 1):
            return False
        frame = gdb.newest_frame()
        committed = False
        while frame:
            if (frame.name() or "").startswith("kernel::runtime_stop::notify_slots"):
                committed = True
                break
            frame = frame.older()
        if not committed or int(gdb.parse_and_eval("$a0")) != 1:
            return False
        raw = int(gdb.parse_and_eval("$a1"))
        # 正式virt fixture为raw==dense 0..3；QEMU bare-target线程ID为raw+1。
        if raw not in range(4):
            return False
        sender = gdb.selected_thread()
        target = next((t for t in gdb.selected_inferior().threads() if t.ptid[1] == raw + 1), None)
        if target is None or target == sender:
            return False
        target.switch()
        pc = int(gdb.parse_and_eval("$pc"))
        waiting = word(pc, 4) == 0x10500073 or word(pc - 4, 4) == 0x10500073
        deadline = physical_word(DEADLINES + raw * 8)
        sender.switch()
        if not waiting or deadline != 0:
            return False
        injected = True
        target_number = target.num
        print("IPI_INJECT sender=%d target=%d raw=%d pc=%#x deadline=0" % (sender.num, target.num, raw, pc))
        # SBI返回允许的FAILED，不发这个IPI；后续停驻广播继续使用真实SBI。
        gdb.execute("set $pc = $pc + 4")
        gdb.execute("set $a0 = -1")
        gdb.execute("set $a1 = 0")
        return False

class Parking(gdb.Breakpoint):
    def stop(self):
        global failed
        if not injected:
            failed = "parking before an injected committed IPI failure"
            return True
        if physical_word(REQ, 1) != 1 or physical_word(STOP, 1) != 1:
            failed = "park preceded the stable runtime stop transition"
            return True
        number = gdb.selected_thread().num
        parked.add(number)
        print("IPI_PARK thread=%d target=%s requested=1 stopped=1" % (number, number == target_number))
        return len(parked) == 4

Injection("*__ECALL__", type=gdb.BP_HARDWARE_BREAKPOINT)
Parking("*__PARK__", type=gdb.BP_HARDWARE_BREAKPOINT)
end
continue
python
if failed or not injected or len(parked) != 4 or target_number not in parked:
    raise gdb.GdbError(failed or "incomplete IPI failure/park evidence")
print("IPI_FAILURE_PASSED: no-timer WFI target and all 4 harts reached terminal park")
end
detach
quit 0
'''
    replacements = {
        "KERNEL": str(args.kernel.resolve()), "PORT": str(port),
        "ECALL": "0x" + ecalls[0], "PARK": hex(addresses["kernel::hart::park"]),
        "REQ": hex(boot.physical_address(args.kernel, addresses["kernel::runtime_stop::REQUESTED"])),
        "STOP": hex(boot.physical_address(args.kernel, addresses["kernel::runtime_stop::STOPPED"])),
        "DEADLINES": hex(boot.physical_address(args.kernel, addresses["kernel::sched::HART_QUANTUM_DEADLINE"])),
        "PHYSICAL_READ": boot.PHYSICAL_READ,
    }
    for name, value in replacements.items():
        script = script.replace("__" + name + "__", value)
    gdb_file = args.logs / "ipi.gdb"
    gdb_file.write_text(script)
    qemu_log = args.logs / "ipi-qemu.log"
    gdb_log = args.logs / "ipi-gdb.log"
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    with qemu_log.open("w") as qlog, gdb_log.open("w") as glog:
        process = subprocess.Popen(command + ["-S", "-gdb", f"tcp:127.0.0.1:{port}"],
            stdout=qlog, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            completed = subprocess.run(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(gdb_file)],
                stdout=glog, stderr=subprocess.STDOUT, timeout=args.timeout)
        finally:
            boot.stop_group(process)
    output = gdb_log.read_text()
    if completed.returncode or "IPI_FAILURE_PASSED" not in output:
        raise RuntimeError(f"incomplete IPI failure evidence: {gdb_log}, {qemu_log}")
    evidence = {
        "kernel_sha256": hashlib.sha256(args.kernel.read_bytes()).hexdigest(),
        "ecall": replacements["ECALL"], "gdb_exit": completed.returncode,
        "injection": "skip one real SBI IPI ecall, return SBI_ERR_FAILED",
        "scope": "virt raw/dense 0..3, no-timer WFI target; subsequent real wake reaches park before scheduling",
        "limits": "debugger pause is not timing/performance evidence; device/DMA quiescence is not claimed",
        "gdb_log": str(gdb_log), "qemu_log": str(qemu_log),
    }
    (args.logs / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(f"IPI failure passed: stable stop and all 4 harts parked; logs: {args.logs}")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.SubprocessError, KeyError) as error:
        print(f"IPI failure validation failed: {error}", file=sys.stderr)
        sys.exit(1)
