#!/usr/bin/env python3
"""旁观正式线程/映射消费者的发布窗口fault，验证RetryFault与memory负载完整收束。"""

import argparse
import hashlib
import json
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

import qemu_gdb as boot


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, required=True)
    parser.add_argument('--logs', type=Path, default=Path('artifacts/vm-fault'))
    parser.add_argument('--timeout', type=int, default=240)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('QEMU command is required')
    args.logs = boot.fresh_logs(args.logs)
    addresses = boot.symbols(args.kernel)
    retry = addresses['<kernel::task::wait::WaitPlan>::retry_fault']
    reset = addresses['kernel::sbi::system_reset']
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    script = f'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "{args.kernel.resolve()}"
target remote 127.0.0.1:{port}
set $fault_waits = 0
hbreak *{retry:#x}
commands
silent
if $a3 != 12 && $a3 != 13 && $a3 != 15
    echo Invalid RetryFault cause.\\n
    quit 1
end
set $fault_waits = $fault_waits + 1
printf "FAULT_WAIT cause=%lu address=%#lx thread=%u\\n", $a3, $a4, $_thread
end
hbreak *{reset:#x}
continue
if $fault_waits < 1
    echo No real publishing fault wait was observed.\\n
    quit 1
end
printf "VM_FAULT_PASSED waits=%u\\n", $fault_waits
detach
quit 0
'''
    gdb_file = args.logs / 'fault.gdb'
    gdb_file.write_text(script)
    qemu_log, gdb_log = args.logs / 'fault-qemu.log', args.logs / 'fault-gdb.log'
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    with qemu_log.open('w') as qlog, gdb_log.open('w') as glog:
        deadline = time.monotonic() + args.timeout
        process = subprocess.Popen(command + ['-S', '-gdb', f'tcp:127.0.0.1:{port}'],
            stdout=qlog, stderr=subprocess.STDOUT, start_new_session=True)
        debugger = None
        with qemu_log.open('rb') as guest_stream:
            pending = b''
            def check_guest_failure():
                nonlocal pending
                pending += guest_stream.read()
                lines = pending.split(b'\n')
                pending = lines.pop()
                for raw in lines:
                    line = raw.decode('utf-8', errors='replace')
                    if 'panicking' in line.lower() or 'system reset accepted: action Reboot,' in line:
                        raise RuntimeError(f'Guest reported failure; stopping this boot: {line}')

            def wait_for(child, stage):
                while child.poll() is None:
                    check_guest_failure()
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f'{stage} exceeded the route deadline: {qemu_log}')
                    time.sleep(.05)
                check_guest_failure()
                return child.returncode

            try:
                debugger = subprocess.Popen(['riscv64-elf-gdb', '-q', '-nx', '--batch', '-x', str(gdb_file)],
                    stdout=glog, stderr=subprocess.STDOUT, start_new_session=True)
                gdb_status = wait_for(debugger, 'GDB observer')
                if gdb_status:
                    raise RuntimeError(f'GDB observer failed with {gdb_status}: {gdb_log}')
                # 已取得唯一所需观察就detach，原消费者继续；明确失败时不等待下一轮boot。
                status = wait_for(process, 'Guest after observer detached')
                if status:
                    raise RuntimeError(f'QEMU exited with {status}: {qemu_log}')
            finally:
                try:
                    if debugger is not None:
                        boot.stop_group(debugger)
                finally:
                    try:
                        boot.stop_group(process)
                    finally:
                        # 原wrapper继续拥有完整profile判据，失败也保存第一轮原始输出。
                        sys.stdout.write(qemu_log.read_text(errors='replace'))
    output, guest = gdb_log.read_text(), qemu_log.read_text(errors='replace')
    if 'VM_FAULT_PASSED' not in output or 'committed mapping access passed: cookie before return=true' not in guest:
        raise RuntimeError(f'publishing window was not exercised: {gdb_log}, {qemu_log}')
    evidence = {
        'kernel_sha256': hashlib.sha256(args.kernel.read_bytes()).hexdigest(),
        'retry_fault': hex(retry), 'gdb_exit': gdb_status,
        'observations': [line for line in output.splitlines() if 'FAULT_WAIT ' in line or 'VM_FAULT_PASSED' in line],
        'scope': 'formal test_hammer/thread suite, release cookie then tail-page read/write before Map returns; no injected exception or kernel test switch',
        'observer': 'detach after first RetryFault; acceptance wrapper checks thread completion, job collection, auxiliary capability cleanup and reset',
        'limits': 'GDB pause is not timing/WCET evidence; stale permission repair and instruction-fence windows need their own evidence',
        'gdb_log': str(gdb_log), 'qemu_log': str(qemu_log),
    }
    (args.logs / 'evidence.json').write_text(json.dumps(evidence, indent=2) + '\n')
    print(f'VM fault observer passed; logs: {args.logs}')


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.SubprocessError, KeyError) as error:
        print(f'VM fault observation failed: {error}', file=sys.stderr)
        sys.exit(1)
