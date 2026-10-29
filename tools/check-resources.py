#!/usr/bin/env python3
"""独立 MMIO/IRQ/DMA 资源观察；复用现存 guest 与 GDB 取证，不启动系统服务。"""
import argparse
import json
import os
from pathlib import Path
import re
import selectors
import signal
import socket
import subprocess
import time

import qemu_gdb as debug
import qemu_mmio_gdb as mmio_probe
import qemu_mmio_forced_gdb as mmio_forced_probe
import qemu_dma_race_gdb as dma_probe
import qemu_dma_install_gdb as dma_install_probe
import qemu_irq_exit_gdb as irq_exit_probe


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    for name in ("irq-exit", "mmio-publication", "mmio-forced", "dma-races", "dma-shared", "dma-quota", "dma-install-failure"):
        selection.add_argument("--" + name, action="store_true")
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("QEMU command missing")
    private = debug.fresh_logs(Path("artifacts/resources")).resolve()
    kernel = Path(os.environ["ERHINO_KERNEL_ELF"])
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        gdb_port = listener.getsockname()[1]
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    irq_exit_debugger = None
    irq_exit_verified = False
    irq_exit_target = private / "irq-exit.json"
    irq_exit_records = {}
    mmio_debugger = None
    mmio_verified = False
    mmio_target = private / "mmio-target.json"
    mmio_records = {}
    dma_debugger = None
    dma_verified = False
    dma_target = private / "dma-target.json"
    dma_records = {}
    dma_modes = ({"dma-pin-install-failure"} if args.dma_install_failure else
                 ({"dma-pin-quota"} if args.dma_quota else ({"dma-shared"} if args.dma_shared else dma_probe.MODES)))
    dma_shared_verified = False
    dma_quota_verified = False
    dma_install_returned = False
    process = None
    selector = selectors.DefaultSelector()
    try:
        process = subprocess.Popen(command + ["-gdb", f"tcp:127.0.0.1:{gdb_port}"],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + args.timeout
        pending = b""
        while selector.get_map() or any(probe for probe in (irq_exit_debugger, mmio_debugger, dma_debugger)):
            if time.monotonic() >= deadline:
                raise TimeoutError("resource observation deadline exceeded")
            if irq_exit_debugger and irq_exit_debugger[0].poll() is not None:
                child, output, logs = irq_exit_debugger
                output.close()
                debug.stop_group(child)
                irq_exit_probe.verify(logs, child.returncode)
                irq_exit_verified = True
                irq_exit_debugger = None
                print(f"IRQ original Waiting and Line loans verified: {logs}", flush=True)
            if mmio_debugger and mmio_debugger[0].poll() is not None:
                child, output, logs = mmio_debugger
                output.close()
                debug.stop_group(child)
                facts = (mmio_forced_probe if args.mmio_forced else mmio_probe).verify(logs, child.returncode)
                for record in facts["windows"]:
                    target = mmio_records.get(record["mode"], {})
                    if target.get("pid") != record["pid"] or target.get("scope") != record["scope"]:
                        raise RuntimeError("MMIO window changed the actual target instance")
                mmio_verified = True
                mmio_debugger = None
                print(f"MMIO publication windows verified: {logs}", flush=True)
            if dma_debugger and dma_debugger[0].poll() is not None:
                child, output, logs = dma_debugger
                output.close()
                debug.stop_group(child)
                facts = (dma_install_probe if args.dma_install_failure else dma_probe).verify(logs, child.returncode)
                for record in ([facts["window"]] if args.dma_install_failure else facts["windows"]):
                    if dma_records.get(record["mode"], {}).get("pid") != record["pid"]:
                        raise RuntimeError("DMA window changed the actual target instance")
                dma_verified = True
                dma_debugger = None
                print(f"DMA exact windows verified: {logs}", flush=True)
            for key, _ in selector.select(timeout=0.05):
                data = os.read(key.fd, 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    continue
                pending += data
                while b"\n" in pending:
                    raw, pending = pending.split(b"\n", 1)
                    line = raw.decode("utf-8", errors="replace")
                    print(line, flush=True)
                    if "panicking" in line.lower() or "system failure terminal entered:" in line:
                        raise RuntimeError("guest failed during resource observation")
                    prepared_kernel = re.search(r"formation instance started: kernel, pid=(\d+)", line)
                    if args.irq_exit:
                        if prepared_kernel:
                            irq_exit_debugger = irq_exit_probe.start(kernel, gdb_port, irq_exit_target, int(prepared_kernel[1]))
                        admitted = re.search(r"IRQ exit target admitted: mode=(\S+), pid=(\d+), source=(\d+), generation=(\d+)", line)
                        if admitted:
                            mode, pid, source, generation = admitted.groups()
                            if mode not in irq_exit_probe.MODES or mode in irq_exit_records:
                                raise RuntimeError("IRQ exit target is unknown or restarted")
                            irq_exit_records[mode] = {"mode": mode, "pid": int(pid), "source": int(source), "generation": int(generation)}
                            temporary = irq_exit_target.with_suffix(".next")
                            temporary.write_text(json.dumps({str(r["pid"]): r for r in irq_exit_records.values()}))
                            temporary.replace(irq_exit_target)
                        closed = re.search(r"IRQ exit target closed: mode=(\S+), pid=(\d+), source=(\d+), generation=(\d+), pool=\d+, pages=0/0/0, metadata=0/0/0, scope=DONE", line)
                        if closed:
                            mode, pid, source, generation = closed.groups()
                            record = irq_exit_records.get(mode, {})
                            if (record.get("pid"), record.get("source"), record.get("generation")) != (int(pid), int(source), int(generation)):
                                raise RuntimeError("IRQ exit cleanup changed the original target/use")
                            record["closed"] = True
                        continue
                    if args.dma_races or args.dma_shared or args.dma_quota or args.dma_install_failure:
                        if (args.dma_races or args.dma_install_failure) and prepared_kernel:
                            dma_debugger = (dma_install_probe if args.dma_install_failure else dma_probe).start(kernel, gdb_port, dma_target, int(prepared_kernel[1]))
                        admitted = re.search(r"DMA race target admitted: mode=(\S+), pid=(\d+), scope=(\d+), memory=(\d+), identity=(\d+)", line)
                        if admitted:
                            mode, pid, scope, memory, identity = admitted.groups()
                            if mode not in dma_modes or mode in dma_records:
                                raise RuntimeError("DMA race case is unknown or restarted")
                            record = {"mode": mode, "pid": int(pid), "scope": int(scope),
                                      "memory": int(memory), "identity": int(identity)}
                            dma_records[mode] = record
                            if args.dma_races:
                                dma_target.write_text(json.dumps(record))
                        if args.dma_install_failure:
                            pool = re.search(r"DMA install target Pool: pid=(\d+), pool=(\d+)", line)
                            if pool:
                                record = dma_records.get("dma-pin-install-failure")
                                if not record or record["pid"] != int(pool[1]):
                                    raise RuntimeError("DMA installation Pool lacks its original admission")
                                record["pool"] = int(pool[2])
                                temporary = dma_target.with_suffix(".next")
                                temporary.write_text(json.dumps(record))
                                temporary.replace(dma_target)
                            observed = re.search(r"pid (\d+): DMA installation failure collected: error=OutOfMemory, handle=false, exposed=false, members=0, writer=0", line)
                            if observed:
                                if dma_records.get("dma-pin-install-failure", {}).get("pid") != int(observed[1]):
                                    raise RuntimeError("DMA installation failure changed the actual target")
                                dma_install_returned = True
                        done = re.search(r"DMA race scope DONE: mode=(\S+), members=0, writer=0", line)
                        if done:
                            if done[1] not in dma_records:
                                raise RuntimeError("DMA DONE lacks its original admission")
                            dma_records[done[1]]["done"] = True
                        refunded = re.search(r"DMA race Pool: mode=(\S+), phase=closed, id=\d+, pages=0/0/0, metadata=0/0/0", line)
                        if refunded:
                            if not dma_records.get(refunded[1], {}).get("done"):
                                raise RuntimeError("DMA refund lacks original scope completion")
                            dma_records[refunded[1]]["refunded"] = True
                        dma_shared_verified |= "DMA shared Backing retirement passed:" in line
                        if args.dma_quota:
                            observed = re.search(r"pid (\d+): DMA quota source recovered: quota=true, members=0, writer=0, fresh=ReadyUnexposed", line)
                            if observed:
                                if dma_records.get("dma-pin-quota", {}).get("pid") != int(observed[1]):
                                    raise RuntimeError("DMA quota recovery changed the actual target instance")
                                dma_quota_verified = True
                        continue
                    if args.mmio_publication or args.mmio_forced:
                        if prepared_kernel:
                            mmio_debugger = (mmio_forced_probe if args.mmio_forced else mmio_probe).start(kernel, gdb_port, mmio_target, int(prepared_kernel[1]))
                        admitted = re.search(r"MMIO publication target admitted: mode=(mmio-before|mmio-after|mmio-forced-queued|mmio-forced-publishing), pid=(\d+), scope=(\d+)", line)
                        if admitted:
                            mode, pid, scope = admitted.groups()
                            if mode in mmio_records:
                                raise RuntimeError("MMIO publication case restarted")
                            record = {"mode": mode, "pid": int(pid), "scope": int(scope)}
                            mmio_records[mode] = record
                            temporary = mmio_target.with_suffix(".next")
                            temporary.write_text(json.dumps(record))
                            temporary.replace(mmio_target)
                        if args.mmio_forced:
                            returned = re.search(r"MMIO forced Revoke returned before Kill: mode=(\S+), pid=(\d+)", line)
                            if returned:
                                record = mmio_records.get(returned[1], {})
                                if record.get("pid") != int(returned[2]):
                                    raise RuntimeError("MMIO forced Revoke changed its original target")
                                record["revoke_returned"] = True
                            retired = re.search(r"MMIO forced Frozen retirement passed: mode=(\S+), pid=(\d+), members=0, cpu=Killed", line)
                            if retired:
                                record = mmio_records.get(retired[1], {})
                                if record.get("pid") != int(retired[2]) or not record.get("revoke_returned"):
                                    raise RuntimeError("MMIO forced completion lacks its original Revoke")
                                record["done"] = True
                        result = re.search(r"pid (\d+): MMIO publication result: mode=(before|after), error=(\w+)", line)
                        if result:
                            mode = "mmio-" + result[2]
                            record = mmio_records.get(mode, {})
                            if record.get("pid") != int(result[1]) or result[3] != ("ObjectBusy" if mode == "mmio-before" else "None"):
                                raise RuntimeError("MMIO real Map result did not match its exact permit window")
                            if mode == "mmio-before" and "fresh_after_done=ObjectClosed" not in line:
                                raise RuntimeError("MMIO before-permission cancellation lacks sealed-source rejection")
                            record["map_result"] = True
                        done = re.search(r"MMIO publication scope DONE while target Running: mode=(mmio-before|mmio-after), pid=(\d+), members=0", line)
                        if done:
                            record = mmio_records.get(done[1], {})
                            if record.get("pid") != int(done[2]) or not record.get("map_result"):
                                raise RuntimeError("MMIO DONE preceded actual Map result or target liveness")
                            record["done"] = True
                        refunded = re.search(r"MMIO publication Pool: mode=(mmio-before|mmio-after|mmio-forced-queued|mmio-forced-publishing), phase=closed, id=\d+, pages_allocated=0, pages_delegated=0, metadata_allocated=0, metadata_delegated=0", line)
                        if refunded:
                            record = mmio_records.get(refunded[1], {})
                            if not record.get("done"):
                                raise RuntimeError("MMIO Pool closure lacks its live-VM revoke proof")
                            record["refunded"] = True
                        continue

        if process.wait(timeout=max(0.1, deadline-time.monotonic())) != 0:
            raise RuntimeError("QEMU exited unsuccessfully")
        if args.irq_exit:
            if not irq_exit_verified or set(irq_exit_records) != irq_exit_probe.MODES or not all(r.get("closed") for r in irq_exit_records.values()):
                raise RuntimeError("IRQ exit lacks original Waiting and cleanup")
        elif args.mmio_publication or args.mmio_forced:
            expected = mmio_forced_probe.MODES if args.mmio_forced else {"mmio-before", "mmio-after"}
            if not mmio_verified or set(mmio_records) != expected or not all(r.get("refunded") for r in mmio_records.values()):
                raise RuntimeError("MMIO observation lacks exact windows and refunds")
        else:
            verified = dma_quota_verified if args.dma_quota else (dma_shared_verified if args.dma_shared else dma_verified)
            if args.dma_install_failure:
                verified = dma_verified and dma_install_returned
            if not verified or set(dma_records) != dma_modes or not all(r.get("refunded") for r in dma_records.values()):
                raise RuntimeError("DMA observation lacks exact windows and refunds")
        print("Resource observation passed", flush=True)
    finally:
        # 外层 runner 可能同时发现 guest 失败；重复终止不能打断 QEMU/GDB 回收。
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        selector.close()
        # 每个调试器与 QEMU 都由本轮独立进程组拥有；清理错误不得阻止其他组清理。
        failures = []
        for probe in (irq_exit_debugger, mmio_debugger, dma_debugger):
            if probe:
                try:
                    debug.stop_group(probe[0])
                except Exception as error:
                    failures.append(str(error))
                finally:
                    probe[1].close()
        if process:
            try:
                debug.stop_group(process)
            except Exception as error:
                failures.append(str(error))
            if process.stdout:
                process.stdout.close()
        if failures:
            raise RuntimeError("resource cleanup failed: " + "; ".join(failures))


if __name__ == "__main__":
    main()
