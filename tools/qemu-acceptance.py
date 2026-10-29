#!/usr/bin/env python3
"""运行一个已选定的验收负载；成功、失败和中断均保留日志并清理本轮进程组。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from qemu_gdb import fresh_logs, stop_group

SHUTDOWN = "system reset accepted: action Shutdown, reason Requested"
FAILED = ("panicking", "system failure terminal entered:", "init formation rejected:",
          "system reset accepted: action reboot,", "kernel acceptance failed:")
SUITES = ("memory", "ipc", "process", "transport", "mmio", "irq", "dma", "time", "startup")


def requirements(suite, mode, services, fp):
    anchors = ["formation tasks complete", "formation stopped: all instances collected",
               "init: formation complete, submitting Shutdown", SHUTDOWN]
    if suite:
        anchors += [f"acceptance suite: {suite}, mode={mode}",
                    f"kernel acceptance completed: {suite}"]
    if suite == "transport":
        anchors.append("pm: active mailbox stop and refund passed")
    if services:
        for service in ("device", "driver", "fs"):
            anchors += [f"formation instance ready: {service}", f"formation instance stopped: {service}"]
    if fp:
        anchors.append("fp verification passed")
    return anchors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--allow-reset-failure", action="store_true")
    parser.add_argument("--logs", type=Path, default=Path("artifacts/acceptance"))
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    suite = os.environ.get("ERHINO_ACCEPTANCE_SUITE", "")
    mode = int(os.environ.get("ERHINO_ACCEPTANCE_MODE", "0"))
    services = os.environ.get("ERHINO_ACCEPTANCE_SERVICES") == "1"
    fp = os.environ.get("ERHINO_ACCEPTANCE_FP") == "1"
    if not command or args.timeout <= 0 or suite not in ("", *SUITES) or not (suite or services or fp):
        parser.error("a valid workload, positive timeout and command are required")
    required = requirements(suite, mode, services, fp)
    platform = os.environ.get("ERHINO_ACCEPTANCE_PLATFORM", "")
    if platform == "nofd":
        required += ["domain 0 [Base64] -> harts [0, 1, 2, 3]"]
    elif platform == "hetero":
        required += ["domain 0 [Base64] -> harts [0]", "domain 1 [Base64+D64] -> harts [1, 2, 3]"]
    logs = fresh_logs(args.logs)
    log = logs / "guest.log"
    evidence = {"command": command, "suite": suite, "mode": mode, "services": services,
                "fp": fp, "formation": os.environ.get("ERHINO_FORMATION_SOURCES"),
                "status": "running", "timeout": args.timeout}
    for key in ("ERHINO_KERNEL_ELF", "ERHINO_BOOT_PACKAGE"):
        path = os.environ.get(key)
        if path:
            evidence[key] = {"path": path, "sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest()}
    evidence["commit"] = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    (logs / "result.json").write_text(json.dumps(evidence, indent=2) + "\n")
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    process = None
    started = time.monotonic()
    status = 1
    seen = set()
    harvested = False
    try:
        with log.open("wb") as output, log.open("rb") as reader:
            process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            pending = b""
            while True:
                exited = process.poll()
                pending += reader.read()
                lines = pending.split(b"\n")
                pending = lines.pop()
                if exited is not None and pending:
                    lines.append(pending)
                    pending = b""
                for raw in lines:
                    line = raw.decode("utf-8", errors="replace")
                    if " FAILED" in line or any(anchor in line.lower() for anchor in FAILED):
                        raise RuntimeError("guest failure: " + line)
                    seen.update(anchor for anchor in required if anchor in line)
                    if "system reset failed:" in line:
                        if not args.allow_reset_failure or set(required) - seen:
                            raise RuntimeError("reset failed before successful workload completion")
                        harvested = True
                if harvested or exited is not None:
                    break
                if time.monotonic() - started >= args.timeout:
                    raise TimeoutError("QEMU acceptance deadline exceeded")
                time.sleep(0.05)
        if process.returncode not in (None, 0) or (not harvested and process.returncode is None):
            raise RuntimeError(f"runner exited with status {process.returncode}")
        missing = set(required) - seen
        if missing:
            raise RuntimeError("missing anchors: " + "; ".join(sorted(missing)))
        status = 0
        evidence["status"] = "passed"
        evidence["harvested_after_reset_failure"] = harvested
    except (Exception, KeyboardInterrupt, SystemExit) as error:
        status = error.code if isinstance(error, SystemExit) else (130 if isinstance(error, KeyboardInterrupt) else 1)
        evidence["status"] = "failed"
        evidence["error"] = str(error)
    finally:
        # 已进入有界收尾，重复终止不能跳过本轮进程组回收。
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if process:
            evidence["runner_exit"] = process.poll()
            try:
                stop_group(process)
            except Exception as error:
                status = 1
                evidence["status"] = "failed"
                evidence["cleanup_error"] = str(error)
        evidence["elapsed_seconds"] = round(time.monotonic() - started, 3)
        evidence["exit_code"] = status
        (logs / "result.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(f"Acceptance {evidence['status']}: {logs}; exit {status}")
    return status


if __name__ == "__main__":
    sys.exit(main())
