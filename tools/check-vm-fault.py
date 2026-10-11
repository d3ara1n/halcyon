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
        try:
            completed = subprocess.run(['riscv64-elf-gdb', '-q', '-nx', '--batch', '-x', str(gdb_file)],
                stdout=glog, stderr=subprocess.STDOUT, timeout=args.timeout)
            if completed.returncode:
                raise RuntimeError(f'GDB observer failed with {completed.returncode}: {gdb_log}')
            try:
                # 已取得唯一所需观察就detach，原线程/映射消费者在无断点下收束。
                status = process.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                raise RuntimeError(f'guest did not finish after observer detached: {qemu_log}') from error
            if status:
                raise RuntimeError(f'QEMU exited with {status}: {qemu_log}')
        finally:
            boot.stop_group(process)
            # 既有qemu-acceptance wrapper拥有全profile锚点判定；同时保存原始日志。
            sys.stdout.write(qemu_log.read_text(errors='replace'))
    output, guest = gdb_log.read_text(), qemu_log.read_text(errors='replace')
    if 'VM_FAULT_PASSED' not in output or 'committed mapping access passed: cookie before return=true' not in guest:
        raise RuntimeError(f'publishing window was not exercised: {gdb_log}, {qemu_log}')
    evidence = {
        'kernel_sha256': hashlib.sha256(args.kernel.read_bytes()).hexdigest(),
        'retry_fault': hex(retry), 'gdb_exit': completed.returncode,
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
