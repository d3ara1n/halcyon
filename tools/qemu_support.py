"""宿主验证公共设施：证据目录与调用者独占的进程组清理。"""
from datetime import datetime, timezone
import os
from pathlib import Path
import signal
import subprocess
import time


def _group_members(pgid):
    rows = subprocess.check_output(["ps", "-axo", "pid=,pgid="], text=True)
    return [int(pid) for pid, group in (line.split() for line in rows.splitlines())
            if int(group) == pgid]


def stop_group(process):
    """收 start_new_session 创建的专有组；leader 退出后仍核对后代。"""
    pgid = process.pid
    # 先收已退出的直接子进程，避免 macOS 对仅含 zombie 的组返回 EPERM。
    process.poll()

    def send(signum):
        try:
            os.killpg(pgid, signum)
        except ProcessLookupError:
            pass
        except PermissionError:
            if _group_members(pgid):
                raise

    def wait_empty():
        end = time.monotonic() + 5
        while True:
            process.poll()
            if not _group_members(pgid):
                process.wait(timeout=5)
                return True
            if time.monotonic() >= end:
                return False
            time.sleep(.05)

    send(signal.SIGCONT)
    send(signal.SIGTERM)
    if wait_empty():
        return
    send(signal.SIGKILL)
    if not wait_empty():
        raise RuntimeError(f"owned process group did not disappear: pgid={pgid}, pids={_group_members(pgid)}")


def fresh_logs(root: Path):
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
    path = root / (stamp + '-' + str(os.getpid()))
    path.mkdir(parents=True, exist_ok=False)
    return path
