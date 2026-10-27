#!/usr/bin/env python3
"""在真实 DMA 保留之后拒绝最终 SBI reset，核 init DONE、保留责任和全 hart 停驻。"""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import signal
import qemu_gdb as support
import qemu_dma_gdb as dma


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, required=True)
    parser.add_argument('--boot-package', type=Path)
    parser.add_argument('--timeout', type=int, required=True)
    parser.add_argument('--ipi-failure', action='store_true')
    parser.add_argument('--logs', type=Path, default=Path('artifacts/dma-retention'))
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('QEMU command is required')
    logs = support.fresh_logs(args.logs)
    setup, finish = dma.program(args.kernel, support.symbols(args.kernel), ipi_failure=args.ipi_failure)
    ast.parse(setup)
    ast.parse(finish)
    fingerprints = {
        'kernel_sha256': hashlib.sha256(args.kernel.read_bytes()).hexdigest(),
        'boot_package_sha256': hashlib.sha256(args.boot_package.read_bytes()).hexdigest() if args.boot_package else None,
    }
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    script = f'''set pagination off
set confirm off
set language rust
set tcp auto-retry on
set tcp connect-timeout 10
file "{args.kernel.resolve()}"
target remote 127.0.0.1:{port}
python
{setup}
end
continue
python
{finish}
end
detach
quit 0
'''
    gdb_file = logs/'retention.gdb'
    gdb_file.write_text(script)
    # 执行前固定身份；失败/超时也保留当轮产物，不能等后续构建后再推断hash。
    evidence = {
        **fingerprints,
        'script_sha256': hashlib.sha256(gdb_file.read_bytes()).hexdigest(),
        'ipi_failure': args.ipi_failure,
        'status': 'not completed',
    }
    (logs/'evidence.json').write_text(json.dumps(evidence, indent=2)+'\n')
    def interrupted(signum, _frame):
        raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM, interrupted)
    with (logs/'qemu.log').open('w') as qlog, (logs/'gdb.log').open('w') as glog:
        process = subprocess.Popen(command+['-S', '-gdb', f'tcp:127.0.0.1:{port}'], stdout=qlog, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            result = subprocess.run(['riscv64-elf-gdb', '-q', '-nx', '--batch', '-x', str(gdb_file)], stdout=glog, stderr=subprocess.STDOUT, timeout=args.timeout)
        finally:
            support.stop_group(process)
    output = (logs/'gdb.log').read_text()
    guest = (logs/'qemu.log').read_text()
    if result.returncode or 'DMA_RETENTION_PASSED' not in output or 'DMA process retirement acceptance passed:' not in guest or 'initial process reached DONE' not in guest:
        raise RuntimeError(f'incomplete DMA terminal evidence: {logs}')
    if args.ipi_failure and 'DMA_IPI_RETENTION_PASSED' not in output:
        raise RuntimeError(f'incomplete DMA/IPI combination evidence: {logs}')
    (logs/'evidence.json').write_text(json.dumps({
        **evidence, 'gdb_exit': result.returncode, 'status': 'completed',
        'scope': 'virt core actual DMA pins; SBI reset rejection, boot DONE observer and four terminal parks',
        'limits': 'addresses are not programmed into hardware; no DMA quiescence, IOMMU, timing or exclusive wake-source claim',
    }, indent=2)+'\n')
    print(f'DMA retention acceptance passed: {logs}')


if __name__ == '__main__':
    main()
