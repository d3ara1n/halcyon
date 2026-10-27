#!/usr/bin/env python3
"""正式设备路线的QMP操作者；逐步等客体状态确认，使用原QEMU验收/节流/退出判定。"""
import argparse
from contextlib import ExitStack
import json
import hashlib
import os
import re
from pathlib import Path
import selectors
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time
import traceback

import qemu_gdb as debug
import qemu_dma_gdb as dma
import qemu_rpc_gdb as rpc_probe
import qemu_function_gdb as function_probe
import qemu_mmio_gdb as mmio_probe
import qemu_mmio_forced_gdb as mmio_forced_probe
import qemu_dma_race_gdb as dma_probe
import qemu_dma_install_gdb as dma_install_probe
import qemu_irq_exit_gdb as irq_exit_probe
import qemu_driver_retry_gdb as retry_probe


class Qmp:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(3)
        self.socket.connect(str(path))
        self.file = self.socket.makefile("rwb", buffering=0)
        greeting = json.loads(self.file.readline())
        if "QMP" not in greeting:
            raise RuntimeError("QMP greeting missing")
        self.sequence = 0
        self.deleted = set()
        self.events = []
        self.call("qmp_capabilities")

    def call(self, command, arguments=None):
        self.sequence += 1
        message = {"execute": command, "id": self.sequence}
        if arguments is not None:
            message["arguments"] = arguments
        self.file.write(json.dumps(message).encode() + b"\n")
        while True:
            line = self.file.readline()
            if not line:
                raise RuntimeError("QMP closed before command receipt")
            response = json.loads(line)
            self.record_event(response)
            if response.get("id") != self.sequence:
                continue
            if "error" in response:
                raise RuntimeError(f"QMP {command} rejected: {response['error']}")
            print(f"DEVICE QMP confirmed: {command} {arguments or {}}", flush=True)
            return response.get("return")

    def record_event(self, response):
        if "event" not in response:
            return
        self.events.append(response)
        if response["event"] == "DEVICE_DELETED":
            self.deleted.add(response.get("data", {}).get("device"))

    def receive_events(self):
        while select.select([self.socket], [], [], 0)[0]:
            line = self.file.readline()
            if not line:
                raise RuntimeError("QMP closed before reset evidence")
            self.record_event(json.loads(line))

    def wait_deleted(self, device):
        # DEVICE_DELETED可先于device_del回复；call与这里消费同一条原QMP流。
        while device not in self.deleted:
            line = self.file.readline()
            if not line:
                raise RuntimeError("QMP closed before DEVICE_DELETED")
            event = json.loads(line)
            self.record_event(event)
        self.deleted.remove(device)
        print(f"DEVICE QMP deletion completed: {device}", flush=True)

    def key(self, down, key="a"):
        self.call("input-send-event", {"events": [
            {"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": key}}}
        ]})

    def close(self):
        self.file.close()
        self.socket.close()


def inject_process_fault(kernel, port, pid, role="HID", start_target=None, reject_reset=False, arm_path=None):
    # 复用正式trap入口和调试类型，只模拟目标PID的一次非法指令异常；不改设备/供应事实。
    logs=debug.fresh_logs(Path("artifacts/device-fault"))
    symbols=debug.symbols(kernel)
    entry=symbols["handle_user_trap"]
    start_gate=(next(name for name in symbols if name.endswith("::start_commit_gate"))
                if start_target else "")
    script=f'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb
import json
import time
from pathlib import Path
TARGET={pid}
matched=False
armed={start_target is None}
START_TARGET={json.dumps(str(start_target)) if start_target else "None"}
ARM_PATH={json.dumps(str(arm_path)) if arm_path else "None"}
{debug.user_trap_context(kernel,entry)}
class TargetTrap(gdb.Breakpoint):
    def stop(self):
        global matched
        if not armed:return False
        if int(gdb.parse_and_eval('$a0'))!=8:return False
        anchor=int(gdb.parse_and_eval('$tp')) & ((1<<64)-1)
        pointer=word(anchor+hart_offsets['current_thread'])
        if not pointer:return False
        shell=word(pointer+process_offset+ptr_offset+pointer_offset)
        if word(shell+data_offset+pid_offset)!=TARGET:return False
        frame=int(gdb.parse_and_eval('$a2')) & ((1<<64)-1)
        if frame!=pointer+thread_offsets['frame']:raise gdb.GdbError('trap frame is not target execution owner')
        print('DEVICE_PROCESS_FAULT_ORIGINAL pid=%d cause=%d stval=%#x sepc=%#x' % (TARGET,int(gdb.parse_and_eval('$a0')),int(gdb.parse_and_eval('$a1')),word(frame+context_offsets['sepc'])))
        gdb.execute('set $a0 = 2')
        gdb.execute('set $a1 = 0')
        matched=True
        self.enabled=False
        print('DEVICE_PROCESS_FAULT_INJECTED pid=%d frame=%#x hart=%d' % (TARGET,frame,gdb.selected_thread().num))
        return True
TargetTrap('*{entry:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
class StartGate(gdb.Breakpoint):
    def stop(self):
        global TARGET,armed
        # 宿主只写真实Start前准入记录；短调试交接不构成客体计时/公平性证据。
        target=Path(START_TARGET)
        end=time.monotonic()+0.5
        while not target.exists() and time.monotonic()<end:time.sleep(0.01)
        if not target.exists():return False
        TARGET=int(json.loads(target.read_text())['pid'])
        # 函数名断点让GDB按DWARF跳过prologue；不假设Rust返回值的隐藏sret寄存器。
        pointer=int(gdb.newest_frame().read_var('process')) & ((1<<64)-1)
        # &Arc本身可在当前内核栈窗口，不属于ELF直映区；在命中hart的有效栈映射读取。
        shell=int.from_bytes(gdb.selected_inferior().read_memory(pointer+ptr_offset+pointer_offset,8).tobytes(),'little')
        if word(shell+data_offset+pid_offset)!=TARGET:return False
        armed=True
        self.enabled=False
        print('DEVICE_PROCESS_PRE_START pid=%d shell=%#x; members not published' % (TARGET,shell))
        return False
if START_TARGET is not None:
    StartGate({json.dumps("'" + start_gate + "'")},type=gdb.BP_HARDWARE_BREAKPOINT)
if ARM_PATH is not None:
    Path(ARM_PATH).write_text("armed\\n")
    print('DEVICE_PROCESS_BREAKPOINT_ARMED pid=%d' % TARGET)
end
continue
python
if not matched:raise gdb.GdbError('target process ecall was not observed')
end
__AFTER_FAULT__
detach
quit 0
'''
    after_fault=""
    if reject_reset:
        setup,finish=dma.program(kernel,symbols,True)
        after_fault=f"python\n{setup}\nend\ncontinue\npython\n{finish}\nend\n"
    script=script.replace("__AFTER_FAULT__",after_fault)
    boot_package=Path(os.environ["ERHINO_BOOT_PACKAGE"])
    (logs/"evidence.json").write_text(json.dumps({"boot_package":str(boot_package),"boot_package_sha256":hashlib.sha256(boot_package.read_bytes()).hexdigest(),"kernel":str(kernel),"kernel_sha256":hashlib.sha256(kernel.read_bytes()).hexdigest(),"role":role,"pid":pid,"trap_entry":hex(entry),"cause":2,"stval":0,"reset_rejected":reject_reset,"scope":"one actual target user ecall, simulated illegal instruction; no timing claim"},indent=2)+"\n")
    script_path=logs/"process-fault.gdb"
    script_path.write_text(script.replace("__PHYSICAL_READ__",debug.PHYSICAL_READ))
    if start_target is not None or arm_path is not None:
        output=(logs/"process-fault.log").open("w")
        child=subprocess.Popen(["riscv64-elf-gdb","-q","-nx","--batch","-x",str(script_path)],
                               stdout=output,stderr=subprocess.STDOUT)
        return child,output,logs
    with (logs/"process-fault.log").open("w") as output:
        result=subprocess.run(["riscv64-elf-gdb","-q","-nx","--batch","-x",str(script_path)],stdout=output,stderr=subprocess.STDOUT,timeout=30)
    verify_process_fault(logs,result.returncode,pid,role)
    if reject_reset and "DMA_RETENTION_PASSED" not in (logs/"process-fault.log").read_text():
        raise RuntimeError(f"rejected reset lacks stable DMA/initial DONE/four parked harts: {logs}")
    return logs


def verify_process_fault(logs, status, pid, role, before_start=False):
    text=(logs/"process-fault.log").read_text()
    if status or f"DEVICE_PROCESS_FAULT_INJECTED pid={pid} " not in text:
        raise RuntimeError(f"{role} fault injection failed: {logs}")
    if before_start and f"DEVICE_PROCESS_PRE_START pid={pid} " not in text:
        raise RuntimeError(f"first-ecall fault lacks pre-Start gate evidence: {logs}")
    print(f"DEVICE {role} process fault injected: pid={pid}; evidence={logs}",flush=True)

def stop_debugger(debugger):
    child, output, _ = debugger
    try:
        if child.poll() is None:
            child.terminate()
            try: child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
    finally:
        output.close()


def stop_qemu(process):
    try:
        debug.stop_group(process)
    finally:
        process.stdout.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reuse",action="store_true")
    parser.add_argument("--candidates", action="store_true")
    parser.add_argument("--retry-race", action="store_true")
    parser.add_argument("--manager-fault", choices=("Device", "Driver"))
    parser.add_argument("--worker-fault", choices=("Xhci",))
    parser.add_argument("--unknown", choices=rpc_probe.OPERATIONS)
    parser.add_argument("--function-session", action="store_true")
    parser.add_argument("--irq-retirement", action="store_true")
    parser.add_argument("--irq-idle", action="store_true")
    parser.add_argument("--irq-exit", action="store_true")
    parser.add_argument("--mmio-publication", action="store_true")
    parser.add_argument("--mmio-forced", action="store_true")
    parser.add_argument("--dma-races", action="store_true")
    parser.add_argument("--dma-shared", action="store_true")
    parser.add_argument("--dma-quota", action="store_true")
    parser.add_argument("--dma-install-failure", action="store_true")
    parser.add_argument("--fault-phase", choices=("wait", "stopping", "start"), default="wait")
    parser.add_argument("--reject-reset", action="store_true")
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.candidates and not args.reuse:
        parser.error("candidate assembly uses the same reuse scenario")
    if args.retry_race and (not args.reuse or args.candidates):
        parser.error("Retry race uses the ordinary real reuse assembly")
    if args.reuse and args.manager_fault:
        parser.error("reuse and manager fault are separate scenarios")
    if args.worker_fault and (args.manager_fault or args.reuse or args.fault_phase != "wait"):
        parser.error("physical worker fault is a separate wait scenario")
    if args.unknown and (args.worker_fault or args.manager_fault or args.reuse or args.reject_reset):
        parser.error("reply-loss is a separate core assembly scenario")
    if args.function_session and (args.unknown or args.worker_fault or args.manager_fault or args.reuse or args.reject_reset):
        parser.error("function session is a separate core assembly scenario")
    resource_modes = (args.mmio_publication, args.mmio_forced, args.dma_races, args.dma_shared, args.dma_quota,
                      args.dma_install_failure, args.irq_exit)
    if sum(resource_modes) > 1 or (any(resource_modes) and (
            args.function_session or args.unknown or args.worker_fault or args.manager_fault or args.reuse or args.reject_reset)):
        parser.error("resource consumer variants are separate core scenarios")
    if args.irq_retirement and not args.function_session:
        parser.error("IRQ retirement uses the actual function session consumer")
    if args.irq_idle and not args.irq_retirement:
        parser.error("all-idle insertion uses the real IRQ retirement consumer")
    if args.reject_reset and not (args.worker_fault or (args.manager_fault == "Device" and args.fault_phase == "wait")):
        parser.error("reset rejection needs a real Device manager or physical worker failure with active DMA")
    fault_scenario = bool(args.manager_fault or args.worker_fault or args.unknown in ("Claim", "GetResource"))
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("QEMU command missing")
    directory = Path("artifacts/device")
    directory.mkdir(parents=True, exist_ok=True)
    private = Path(tempfile.mkdtemp(prefix="run-", dir=directory)).resolve()
    qmp_path = private / "qmp"
    process = None
    qmp = None
    selector = selectors.DefaultSelector()
    stage = 0
    completed = False
    reuse_waiting=False
    client_waiting = False
    host_waiting = False
    irq_seen = False
    bindings = {}
    reuse = {}
    collected = []
    # 本次主机取证，不是客体的候选/占用表；记录取消连接及实际三条键盘链。
    candidates = {}
    prepare_cancel = None
    retry_verified = False
    retry_debugger = None
    retry_client_verified = False
    retry_facts = None
    connections = {}
    incompatible_pids = set()
    managers = {}
    main_waiting = False
    failure_terminal = 0
    fault_done = False
    reboot_accepted = False
    reboot_rejected = False
    initial_done = False
    job_sealed = False
    platform_sealed = False
    rings_configured = None
    physical_pid = None
    physical_faulted = False
    physical_owner_done = False
    physical_manager_failed = False
    physical_verified = False
    physical_debugger = None
    physical_woken = False
    physical_armed = private / "physical-armed"
    unknown_probe = None
    function_debugger = None
    function_verified = False
    function_address = private / "function-address.json"
    function_rounds = {}
    function_passed = False
    function_old_rejected = False
    irq_target = private / "irq-target.json"
    irq_idle_ready = private / "irq-idle-ready.json"
    irq_idle_inserted = private / "irq-idle-inserted"
    irq_rounds = {}
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
    unknown_facts = None
    unknown_errors = {}
    unknown_exposed = False
    unknown_released = set()
    workers = {}
    master_inputs = []
    master_commits = []
    dma_baseline_retired = False
    kernel = Path(os.environ["ERHINO_KERNEL_ELF"])
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        gdb_port = listener.getsockname()[1]
    early_fault = None
    early_verified = args.fault_phase != "start"
    early_target = private / "target.json"
    try:
        process = subprocess.Popen(command + ["-qmp", f"unix:{qmp_path},server=on,wait=off", "-gdb", f"tcp:127.0.0.1:{gdb_port}"] +
                                   (["-S"] if args.manager_fault and args.fault_phase == "start" else []),
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
        if args.manager_fault and args.fault_phase == "start":
            early_fault = inject_process_fault(kernel,gdb_port,0,args.manager_fault,early_target)
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + args.timeout
        pending = b""
        while selector.get_map():
            if time.monotonic() >= deadline:
                raise TimeoutError("device QMP route deadline exceeded")
            if early_fault and early_fault[0].poll() is not None:
                child,output,logs=early_fault
                output.close()
                verify_process_fault(logs,child.returncode,managers.get(args.manager_fault),args.manager_fault,True)
                early_fault=None
                early_verified=True
            if physical_debugger:
                child, output, logs = physical_debugger
                if physical_armed.exists() and not physical_woken:
                    # 断点先装好，再用真实设备事件唤醒；已在推进的目标也可先命中。
                    qmp.call("device_add", {"driver": "usb-kbd", "bus": "xhci.0", "id": "fault-wake", "port": "1"})
                    physical_woken = True
                    print("DEVICE physical target wake requested by real keyboard insertion", flush=True)
                if child.poll() is not None:
                    output.close()
                    verify_process_fault(logs, child.returncode, physical_pid, args.worker_fault)
                    if args.reject_reset and "DMA_RETENTION_PASSED" not in (logs/"process-fault.log").read_text():
                        raise RuntimeError(f"physical fault lacks stable DMA/initial DONE/four parked harts: {logs}")
                    physical_verified = True
                    physical_debugger = None
            if unknown_probe and unknown_probe[0].poll() is not None:
                child, output, logs = unknown_probe
                output.close()
                unknown_facts = rpc_probe.verify(logs, child.returncode, args.unknown)
                unknown_probe = None
                expected = workers.get("pci-generic") if args.unknown in ("Activate", "Master") else managers.get("Device")
                if unknown_facts["pid"] != expected:
                    raise RuntimeError("reply-loss did not target the actual service/worker instance")
                print(f"DEVICE {args.unknown} successful response omitted before enqueue; evidence={logs}", flush=True)
            if function_debugger and function_debugger[0].poll() is not None:
                child, output, logs = function_debugger
                output.close()
                function_probe.verify(logs, child.returncode, workers.get("pci-generic"))
                function_verified = True
                function_debugger = None
                print(f"DEVICE function contexts/BME read-only evidence verified: {logs}", flush=True)
            if irq_exit_debugger and irq_exit_debugger[0].poll() is not None:
                child, output, logs = irq_exit_debugger
                output.close()
                irq_exit_probe.verify(logs, child.returncode)
                irq_exit_verified = True
                irq_exit_debugger = None
                print(f"IRQ original Waiting and Line loans verified: {logs}", flush=True)
            if retry_debugger and retry_debugger[0].poll() is not None:
                child, output, logs = retry_debugger
                output.close()
                retry_facts = retry_probe.verify(logs, child.returncode)
                if retry_facts["pid"] != managers.get("Driver") or retry_facts["device"] != reuse.get("c"):
                    raise RuntimeError("Retry probe changed the actual manager or terminal device")
                retry_debugger = None
                print(f"DEVICE Retry late-query coverage={retry_facts['coverage']}; evidence={logs}", flush=True)
            if args.irq_idle and irq_idle_ready.exists() and not irq_idle_inserted.exists():
                snapshot = json.loads(irq_idle_ready.read_text())
                target = irq_rounds.get(1, {})
                if (snapshot["source"],snapshot["target_slot"],snapshot["generation"]) != (target.get("source"),target.get("target"),target.get("generation")):
                    raise RuntimeError("all-idle snapshot changed the actual IRQ authority")
                qmp.call("device_add", {"driver":"usb-kbd","bus":"xhci.0","id":"irq-1","port":"1"})
                irq_idle_inserted.write_text("QMP device_add confirmed\n")
                print("DEVICE actual insertion completed while four-hart idle snapshot remained paused", flush=True)
            if mmio_debugger and mmio_debugger[0].poll() is not None:
                child, output, logs = mmio_debugger
                output.close()
                facts = (mmio_forced_probe if args.mmio_forced else mmio_probe).verify(logs, child.returncode)
                for record in facts["windows"]:
                    target = mmio_records.get(record["mode"], {})
                    if target.get("pid") != record["pid"] or target.get("scope") != record["scope"]:
                        raise RuntimeError("MMIO window changed the actual target instance")
                mmio_verified = True
                mmio_debugger = None
                print(f"DEVICE MMIO publication windows verified: {logs}", flush=True)
            if dma_debugger and dma_debugger[0].poll() is not None:
                child, output, logs = dma_debugger
                output.close()
                facts = (dma_install_probe if args.dma_install_failure else dma_probe).verify(logs, child.returncode)
                for record in ([facts["window"]] if args.dma_install_failure else facts["windows"]):
                    if dma_records.get(record["mode"], {}).get("pid") != record["pid"]:
                        raise RuntimeError("DMA window changed the actual target instance")
                dma_verified = True
                dma_debugger = None
                print(f"DEVICE DMA exact windows verified: {logs}", flush=True)
            unknown_observed = bool(unknown_facts and unknown_facts["txid"] in unknown_errors)
            if fault_scenario and stage == 30 and qmp:
                qmp.receive_events()
                reset = any(event.get("event") == "RESET" and
                            event.get("data", {}).get("guest") is True for event in qmp.events)
                if args.reject_reset and reset:
                    raise RuntimeError("reset rejection unexpectedly rebooted the guest")
                boundary=(reboot_rejected and initial_done) if args.reject_reset else reset
                # reset拒绝由原DMA观察体核init DONE/四hart停驻；主线也可合法先取得唯一终局。
                fault_proved = (physical_faulted and physical_manager_failed and fault_done and physical_verified) if args.worker_fault else (
                    (unknown_observed and fault_done and (args.unknown != "GetResource" or unknown_exposed)) if args.unknown else
                    (fault_done or args.reject_reset or args.fault_phase == "stopping"))
                if boundary and reboot_accepted and fault_proved and early_verified:
                    if failure_terminal != 1 or not job_sealed or not platform_sealed:
                        raise RuntimeError("manager failure lacks one terminal owner or committed seals")
                    if args.worker_fault:
                        print(f"DEVICE physical worker fault confirmed: role={args.worker_fault}, pid={physical_pid}; authenticated Faulted/DONE, trusted owner failure and one terminal", flush=True)
                    if args.unknown:
                        print(f"DEVICE {args.unknown} committed request outcome Unknown confirmed: Sent error, original authority retained and one fault terminal", flush=True)
                    if args.reject_reset:
                        print("DEVICE rejected cold reboot retention confirmed: init DONE, four parked harts, same exposed DMA pins/writers; no QMP RESET", flush=True)
                    elif args.unknown:
                        print(f"DEVICE RPC {args.unknown} outcome Unknown fault terminal confirmed by QMP guest RESET", flush=True)
                    else:
                        print(f"DEVICE manager failure cold reboot confirmed: role={args.manager_fault}; phase={args.fault_phase}; QMP guest RESET", flush=True)
                    qmp.call("quit")
                    completed = True
                    stage = 31
            for key, _ in selector.select(timeout=0.2):
                data = os.read(key.fd, 65536)
                if not data:
                    selector.unregister(key.fileobj)
                    break
                pending += data
                while b"\n" in pending:
                    raw, pending = pending.split(b"\n", 1)
                    line = raw.decode("utf-8", errors="replace")
                    print(line, flush=True)
                    if args.irq_exit:
                        if "device observer admitted before Start: role=Device," in line:
                            irq_exit_debugger = irq_exit_probe.start(kernel, gdb_port, irq_exit_target)
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
                        if "device failure terminal entered:" in line:
                            raise RuntimeError("IRQ exit entered failure terminal")
                        if "system reset accepted: action Shutdown, reason Requested" in line:
                            if not irq_exit_verified or set(irq_exit_records) != irq_exit_probe.MODES or not all(r.get("closed") for r in irq_exit_records.values()):
                                raise RuntimeError("IRQ exit shutdown lacks actual Waiting and original cleanup")
                            completed = True
                            stage = 41
                        continue
                    if args.dma_races or args.dma_shared or args.dma_quota or args.dma_install_failure:
                        if (args.dma_races or args.dma_install_failure) and "device observer admitted before Start: role=Device," in line:
                            dma_debugger = (dma_install_probe if args.dma_install_failure else dma_probe).start(kernel, gdb_port, dma_target)
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
                        if "device failure terminal entered:" in line:
                            raise RuntimeError("DMA races entered device failure terminal")
                        if "system reset accepted: action Shutdown, reason Requested" in line:
                            verified = dma_quota_verified if args.dma_quota else (dma_shared_verified if args.dma_shared else dma_verified)
                            if args.dma_install_failure:
                                verified = dma_verified and dma_install_returned
                            if not verified or set(dma_records) != dma_modes or not all(r.get("refunded") for r in dma_records.values()):
                                raise RuntimeError("DMA shutdown lacks exact windows and original Pool refunds")
                            completed = True
                            stage = 41
                        continue
                    if args.mmio_publication or args.mmio_forced:
                        if "device observer admitted before Start: role=Device," in line:
                            mmio_debugger = (mmio_forced_probe if args.mmio_forced else mmio_probe).start(kernel, gdb_port, mmio_target)
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
                        if "device failure terminal entered:" in line:
                            raise RuntimeError("MMIO publication entered device failure terminal")
                        if "system reset accepted: action Shutdown, reason Requested" in line:
                            expected = mmio_forced_probe.MODES if args.mmio_forced else {"mmio-before","mmio-after"}
                            if not mmio_verified or set(mmio_records) != expected or not all(r.get("refunded") for r in mmio_records.values()):
                                raise RuntimeError("MMIO publication shutdown lacks both exact windows/refunds")
                            completed = True
                            stage = 41
                        continue
                    if args.function_session:
                        admitted = re.search(r"device observer admitted before Start: role=Device, pid=(\d+),", line)
                        if admitted:
                            function_debugger = function_probe.start(kernel, gdb_port, function_address, irq_target if args.irq_retirement else None, args.irq_idle)
                            if args.irq_retirement: qmp = Qmp(qmp_path)
                        spawned = re.search(r"driver process spawned: key=(\S+), process=\d+, pid=(\d+),", line)
                        if spawned:
                            if spawned[1] != "pci-generic":
                                raise RuntimeError("function session assembly automatically launched another worker")
                            workers[spawned[1]] = int(spawned[2])
                        prepared = re.search(r"function session round prepared: round=(\d+), device=(\d+), epoch=(\d+), command=(0x[0-9a-f]+)", line)
                        if prepared:
                            round_, serial, epoch = map(int, prepared.groups()[:3])
                            record = function_rounds.setdefault(round_, {})
                            if record or (round_ == 2 and not function_rounds.get(1, {}).get("released")):
                                raise RuntimeError("new function Prepare preceded original Released")
                            record.update(device=serial, epoch=epoch)
                            actual = {"device": serial, "epoch": epoch, "command": int(prepared[4],16)}
                            if round_ == 1:
                                function_address.write_text(json.dumps(actual))
                            elif (serial != function_rounds[1]["device"] or epoch <= function_rounds[1]["epoch"]):
                                raise RuntimeError("function rounds changed DeviceId or reused epoch")
                        event = re.search(r"function session (controller running|provider Stop observed|hardware stopped|Binding released): round=(\d+), device=(\d+), epoch=(\d+)", line)
                        if event:
                            action, round_, serial, epoch = event.groups()
                            record = function_rounds.get(int(round_), {})
                            if (record.get("device"),record.get("epoch")) != (int(serial),int(epoch)):
                                raise RuntimeError("function event changed original BindingRef")
                            key, predecessor = {
                                "controller running": ("running", "epoch"),
                                "provider Stop observed": ("stopped", "running"),
                                "hardware stopped": ("hardware", "stopped"),
                                "Binding released": ("released", "hardware"),
                            }[action]
                            if not record.get(predecessor) or record.get(key):
                                raise RuntimeError("function hardware/report/release order mismatch")
                            record[key] = True
                        function_old_rejected |= "function session old alias rejected:" in line and "error=Permission" in line
                        function_passed |= "function session checks passed:" in line
                        if args.irq_retirement:
                            waiting = re.search(r"IRQ retirement awaiting real insertion: round=(\d+), device=(\d+), epoch=(\d+), source=(\d+), target=(\d+), generation=(\d+)", line)
                            if waiting:
                                round_, serial, epoch, source, target, generation = map(int,waiting.groups())
                                if round_ in irq_rounds: raise RuntimeError("IRQ insertion gate repeated")
                                record = {"round": round_, "device": serial, "epoch": epoch, "source": source, "target": target, "generation": generation}
                                irq_rounds[round_] = record
                                irq_target.write_text(json.dumps(record))
                                if not args.irq_idle or round_ != 1:
                                    qmp.call("device_add", {"driver":"usb-kbd","bus":"xhci.0","id":f"irq-{round_}","port":"1"})
                            progress = re.search(r"IRQ persistent READABLE allowed Control progress: round=(\d+), device=(\d+), epoch=(\d+), source=(\d+), generation=(\d+)", line)
                            if progress:
                                round_, serial, epoch, source, generation = map(int, progress.groups())
                                record = irq_rounds.get(round_, {})
                                if any(record.get(key) != value for key, value in (("device", serial), ("epoch", epoch), ("source", source), ("generation", generation))) or record.get("readable_progress"):
                                    raise RuntimeError("persistent IRQ progress changed the original binding or use")
                                record["readable_progress"] = True
                            token = re.search(r"IRQ retirement real token held: round=(\d+), generation=(\d+), sequence=(\d+)",line)
                            if token:
                                record=irq_rounds.get(int(token[1]),{})
                                if record.get("generation")!=int(token[2]):raise RuntimeError("IRQ Take changed actual use generation")
                                if not record.get("readable_progress"):raise RuntimeError("IRQ Take preceded persistent-level Control progress")
                                record["sequence"]=int(token[3])
                            if event and event[1] == "Binding released":
                                round_=int(event[2])
                                if not irq_rounds.get(round_,{}).get("sequence"):raise RuntimeError("IRQ binding Released without actual taken token")
                                qmp.call("device_del", {"id":f"irq-{round_}"})
                                qmp.wait_deleted(f"irq-{round_}")
                        if "device failure terminal entered:" in line:
                            raise RuntimeError("function session entered failure terminal")
                        if "system reset accepted: action Shutdown, reason Requested" in line:
                            if not function_verified or not function_passed or not function_old_rejected or set(function_rounds) != {1,2} or not all(r.get("released") for r in function_rounds.values()):
                                raise RuntimeError("function session shutdown preceded complete real evidence")
                            completed = True
                            stage = 41
                        continue
                    if args.unknown:
                        admitted = re.search(r"device observer admitted before Start: role=(Device|Driver), pid=(\d+),", line)
                        if admitted:
                            managers[admitted[1]] = int(admitted[2])
                            if admitted[1] == "Device":
                                qmp = Qmp(qmp_path)
                                unknown_probe = rpc_probe.start(kernel, gdb_port, args.unknown)
                                stage = 30 if fault_scenario else 40
                        spawned = re.search(r"driver process spawned: key=(\S+), process=\d+, pid=(\d+),", line)
                        if spawned:
                            if spawned[1] in workers:
                                raise RuntimeError("reply-loss replaced the original worker process")
                            workers[spawned[1]] = int(spawned[2])
                        prefixes = {
                            "Claim": "driver RPC failed: operation=Claim,",
                            "Prepare": "device Provision RPC failed:",
                            "ProvisionGet": "device Provision RPC failed:",
                            "GetResource": "PCI RPC failed:",
                            "Activate": "driver RPC failed: operation=Activate,",
                            "Master": "xHCI EnableMaster reply failed:",
                        }
                        if prefixes[args.unknown] in line and "phase=Sent," in line:
                            matched = re.search(r"txid=(?:Some\()?(\d+)", line)
                            if matched:
                                if args.unknown == "Prepare" and "command=Prepare(1)" not in line:
                                    raise RuntimeError("wrong Provision operation failed")
                                if args.unknown == "ProvisionGet" and "command=Get { epoch: 1, index: 0 }" not in line:
                                    raise RuntimeError("wrong Provision Get failed")
                                unknown_errors[int(matched[1])] = line
                        unknown_exposed |= ("device Binding stopping:" in line and "serial: 1 }" in line
                                            and "epoch: 1 }" in line and "resources_exposed=true, reported=false" in line)
                        matched = re.search(r"device binding owner retired: Work \{ reference: BindingRef \{ device: DeviceId \{ incarnation: \d+, serial: (\d+) \}, epoch: 1 \}, .*next: Released,", line)
                        if matched:
                            unknown_released.add(int(matched[1]))
                        if args.unknown == "GetResource" and "device binding owner retired:" in line and "serial: 1 }" in line:
                            raise RuntimeError("exposed resource reply loss was converted into safe release")
                        matched = re.search(r"xHCI EnableMaster submitted: device=(\d+), epoch=(\d+), context=(\d+), deadline=(\d+)", line)
                        if matched:
                            master_inputs.append(tuple(map(int, matched.groups())))
                        matched = re.search(r"PCI EnableMaster committed: epoch=(\d+), context=(\d+)", line)
                        if matched:
                            master_commits.append(tuple(map(int, matched.groups())))
                        fault_done |= f"device observer manager DONE: pid={managers.get('Driver')}," in line
                        if not fault_scenario:
                            if "device failure terminal entered:" in line:
                                raise RuntimeError("reply-loss recovery entered a fault terminal")
                            if "system reset accepted: action Shutdown, reason Requested" in line:
                                if not unknown_observed:
                                    raise RuntimeError("normal shutdown preceded the actual Sent/Unknown observation")
                                if 1 not in unknown_released:
                                    raise RuntimeError("normal shutdown lacks the original Binding's Released retirement")
                                if args.unknown == "Master":
                                    if len(master_inputs) != 2 or master_inputs[0] != master_inputs[1]:
                                        raise RuntimeError("Master retry changed original ref/context/deadline or did not retry once")
                                    if len(master_commits) != 2 or master_commits[0] != master_commits[1]:
                                        raise RuntimeError("Master retry did not reuse the original Prepared function")
                                    if master_inputs[0][0] not in unknown_released:
                                        raise RuntimeError("Master recovery did not safely retire its original Binding")
                                completed = True
                                stage = 41
                            continue
                    if args.candidates:
                        matched = re.search(r"USB boot keyboard registered: connection=(\d+), serial=(\d+)", line)
                        if matched:
                            connections[int(matched[1])] = int(matched[2])
                        matched = re.search(r"driver process spawned: key=usb-incompatible-pci, process=\d+, pid=(\d+),", line)
                        if matched:
                            incompatible_pids.add(int(matched[1]))
                        pid = re.search(r"pid (\d+):", line)
                        if pid and int(pid[1]) in incompatible_pids:
                            if any(text in line for text in ("PCI resource Describe/Get matched", "ECAM mapped", "PCI function:")):
                                raise RuntimeError("incompatible PCI candidate accessed hardware")
                        matched = re.search(r"driver binding input prepared: operation=(Claim|Activate), device=(\d+), epoch=(\d+), key=(\S+), registrar=(true|false), trusted_physical=(true|false)", line)
                        if matched:
                            operation, serial, epoch, key_name, registrar, physical = matched.groups()
                            serial, epoch = int(serial), int(epoch)
                            if key_name == "usb-incompatible-pci":
                                if registrar != "false" or physical != "false":
                                    raise RuntimeError("incompatible candidate acquired physical or child authority")
                                if operation == "Claim":
                                    if serial in candidates:
                                        raise RuntimeError("candidate retried in the same selection")
                                    candidates[serial] = {"epoch": epoch}
                                else:
                                    if candidates.get(serial, {}).get("epoch") != epoch:
                                        raise RuntimeError("incompatible activation changed binding")
                                    candidates[serial]["activation"] = True
                            elif key_name == "usb-hid" and operation == "Claim":
                                record = candidates.get(serial, {})
                                if not record.get("advanced") or record.get("next_epoch") != epoch:
                                    raise RuntimeError("HID Claim preceded safe candidate release/detach")
                                record["claimed"] = True
                        matched = re.search(r"driver activation input rejected before resource acquisition: device=(\d+), epoch=(\d+), identity_match=false, registrar_match=false", line)
                        if matched:
                            serial, epoch = int(matched[1]), int(matched[2])
                            record = candidates.get(serial, {})
                            if not pid or int(pid[1]) not in incompatible_pids or not record.get("activation") or record.get("epoch") != epoch:
                                raise RuntimeError("candidate rejection did not match its real identity/role mismatch")
                            record.update(rejected=True, pid=int(pid[1]))
                        matched = re.search(r"driver operation rejected: device=(\d+), operation=Activate, error=(\w+)", line)
                        if matched and int(matched[1]) in candidates:
                            record = candidates[int(matched[1])]
                            if not record.get("rejected") or matched[2] != "Permission":
                                raise RuntimeError("candidate failure was not its explicit identity/role rejection")
                            record["result"] = True
                        matched = re.search(r"USB interface supply closed: connection=(\d+), epoch=(\d+)", line)
                        if matched:
                            serial = connections.get(int(matched[1]))
                            record = candidates.get(serial, {})
                            if record.get("epoch") == int(matched[2]):
                                if not record.get("result"):
                                    raise RuntimeError("candidate supply closed before explicit rejection")
                                record["closed"] = True
                        matched = re.search(r"driver binding Released: device=(\d+)", line)
                        if matched:
                            record = candidates.get(int(matched[1]), {})
                            if record and not record.get("advanced"):
                                if not record.get("closed"):
                                    raise RuntimeError("candidate Binding released before Provision Closed")
                                record["released"] = True
                            elif record and record.get("cancelled"):
                                if not record.get("cancel_closed"):
                                    raise RuntimeError("cancelled reprepare Binding released before hardware Closed")
                                record["cancel_released"] = True
                        matched = re.search(r"driver attempt retired: device=(\d+), epoch=(\d+), key=usb-incompatible-pci, released=true, failure=Some\(Permission\)", line)
                        if matched:
                            record = candidates.get(int(matched[1]), {})
                            if not record.get("released") or record.get("epoch") != int(matched[2]):
                                raise RuntimeError("candidate retired without its original Released proof")
                            record["retired"] = True
                        matched = re.search(r"driver candidate advanced: device=(\d+), from_epoch=(\d+), to_epoch=(\d+), from=usb-incompatible-pci, to=usb-hid", line)
                        if matched:
                            record = candidates.get(int(matched[1]), {})
                            if not record.get("retired") or record.get("epoch") != int(matched[2]) or int(matched[3]) <= int(matched[2]):
                                raise RuntimeError("candidate advancement lost original identity or release ordering")
                            record.update(advanced=True, next_epoch=int(matched[3]))
                        matched = re.search(r"USB hardware transition: connection=(\d+), port=\d+, Reset->WaitReset,", line)
                        if matched and prepare_cancel is None and stage == 10:
                            serial = connections.get(int(matched[1]))
                            record = candidates.get(serial, {})
                            if record.get("advanced") and record.get("claimed"):
                                # 只在原连接的新Prepare确已开始后拔出；首轮枚举不触发此分支。
                                prepare_cancel = serial
                                record["cancelled"] = True
                                qmp.call("device_del", {"id": "reuseA"})
                                qmp.wait_deleted("reuseA")
                                print(f"DEVICE reprepare cancellation requested: serial={serial}, connection={matched[1]}", flush=True)
                        matched = re.search(r"USB interface stopping: connection=(\d+), epoch=(\d+), descriptor_ready=(true|false)", line)
                        if matched and prepare_cancel is not None and connections.get(int(matched[1])) == prepare_cancel:
                            record = candidates[prepare_cancel]
                            if int(matched[2]) == record["next_epoch"]:
                                if matched[3] != "false":
                                    raise RuntimeError("reprepare cancellation missed the descriptor-absent window")
                                record["cancel_stop"] = True
                        matched = re.search(r"USB interface supply closed: connection=(\d+), epoch=(\d+)", line)
                        if matched and prepare_cancel is not None and connections.get(int(matched[1])) == prepare_cancel:
                            record = candidates[prepare_cancel]
                            if int(matched[2]) == record["next_epoch"]:
                                if not record.get("cancel_stop"):
                                    raise RuntimeError("reprepare Closed lacks descriptor-absent Stop evidence")
                                record["cancel_closed"] = True
                        matched = re.search(r"driver attempt retired: device=(\d+), epoch=(\d+), key=usb-hid, released=true,", line)
                        if matched and int(matched[1]) == prepare_cancel:
                            record = candidates[prepare_cancel]
                            if not record.get("cancel_released") or int(matched[2]) != record["next_epoch"]:
                                raise RuntimeError("cancelled reprepare did not detach its original Released binding")
                            record["cancel_retired"] = True
                            qmp.call("device_add", {"driver": "usb-kbd", "bus": "xhci.0", "id": "reuseA", "port": "1"})
                            print(f"DEVICE descriptor-absent reprepare safely released: serial={prepare_cancel}; same-port reinsertion requested", flush=True)
                    if args.retry_race:
                        admitted = re.search(r"device observer admitted before Start: role=Driver, pid=(\d+),", line)
                        if admitted:
                            managers["Driver"] = int(admitted[1])
                        observed = re.search(r"DEVICE Drivers concurrent Retry replies: serial=(\d+), .*thread_joined=true", line)
                        if observed:
                            if int(observed[1]) != reuse.get("c"):
                                raise RuntimeError("Retry clients changed the original terminal C")
                            retry_client_verified = True
                    if fault_scenario:
                        admitted = re.search(r"device observer admitted before Start: role=(Device|Driver), pid=(\d+),", line)
                        if admitted:
                            managers[admitted[1]] = int(admitted[2])
                            if args.fault_phase == "start" and admitted[1] == args.manager_fault:
                                pid=managers[args.manager_fault]
                                early_target.write_text(json.dumps({"pid":pid})+"\n")
                                evidence=early_fault[2]/"evidence.json"
                                facts=json.loads(evidence.read_text())
                                facts.update(pid=pid,scope="pre-Start gate then first actual user ecall; no timing claim")
                                evidence.write_text(json.dumps(facts,indent=2)+"\n")
                                qmp=Qmp(qmp_path)
                                stage=30
                        main_waiting |= "device client synchronous wait entered:" in line
                        host_waiting |= "xHCI activation ready:" in line
                        dma_baseline_retired |= "DMA platform acceptance passed:" in line
                        if "xHCI rings configured:" in line:
                            rings_configured=line.split("xHCI rings configured:",1)[1].strip()
                        if args.worker_fault and "xHCI activation ready:" in line:
                            match = re.search(r"pid (\d+):", line)
                            if not match:
                                raise RuntimeError("xHCI Ready lacks an actual process identity")
                            physical_pid = int(match[1])
                        if args.worker_fault:
                            physical_faulted |= f"pid {physical_pid} aborted: scause=0x2 " in line
                            if f"trusted physical worker DONE without Binding release: pid={physical_pid}," in line:
                                if "reported=false" not in line:
                                    raise RuntimeError("physical fault scenario already had a stop report")
                                physical_owner_done = True
                            if "driver manager failed:" in line:
                                if not physical_owner_done or "Unavailable" not in line:
                                    raise RuntimeError("Driver failure did not originate from this physical worker's unconfirmed DONE")
                                physical_manager_failed = True
                            fault_done |= f"device observer manager DONE: pid={managers.get('Driver')}," in line
                        if "device failure terminal entered:" in line:
                            if stage != 30:
                                raise RuntimeError("assembly failed before requested manager fault")
                            failure_terminal += 1
                        if args.manager_fault:
                            fault_done |= f"device observer manager DONE: pid={managers.get(args.manager_fault)}," in line
                        reboot_accepted |= "system reset accepted: action Reboot, reason SystemFailure" in line
                        reboot_rejected |= "device failure cold reboot rejected:" in line
                        initial_done |= "initial process reached DONE" in line
                        job_sealed |= "device failure JobSeal committed" in line
                        platform_sealed |= "device failure platform root Seal committed" in line
                        trigger = (args.fault_phase == "wait" and stage == 0 and main_waiting and host_waiting) or (
                            args.fault_phase == "stopping" and
                            f"device observer normal stop intent: role={args.manager_fault}," in line)
                        if trigger and args.worker_fault and physical_pid is not None:
                            if not dma_baseline_retired or rings_configured is None:
                                raise RuntimeError("physical fault is not linked to the actual live xHCI rings")
                            qmp = Qmp(qmp_path)
                            physical_debugger = inject_process_fault(kernel, gdb_port, physical_pid, args.worker_fault,
                                                                     reject_reset=args.reject_reset, arm_path=physical_armed)
                            evidence = physical_debugger[2]/"evidence.json"
                            facts = json.loads(evidence.read_text())
                            facts.update(xhci_rings=rings_configured, scope="live xHCI worker fault after ring address delivery; actual exposed DMA set, ColdReboot rejection/initial DONE/four parks when requested; no DMA-stop or timing claim")
                            evidence.write_text(json.dumps(facts, indent=2)+"\n")
                            stage = 30
                        if trigger and args.manager_fault in managers:
                            if qmp is None:
                                qmp = Qmp(qmp_path)
                            fault_logs=inject_process_fault(kernel, gdb_port, managers[args.manager_fault], args.manager_fault,
                                                           reject_reset=args.reject_reset)
                            if args.reject_reset:
                                if not dma_baseline_retired or rings_configured is None:
                                    raise RuntimeError("retention is not linked to this live xHCI controller")
                                evidence=fault_logs/"evidence.json"
                                facts=json.loads(evidence.read_text())
                                facts.update(xhci_rings=rings_configured,
                                             scope="live xHCI four ring pages before FS/core retirement consumers; Device fault, rejected ColdReboot, init DONE and four parked harts")
                                evidence.write_text(json.dumps(facts,indent=2)+"\n")
                            stage = 30
                        if args.fault_phase in ("wait", "start") or stage >= 30:
                            continue
                    if stage == 0 and "driver process spawned: key=usb-hid," in line:
                        raise RuntimeError("HID process ran before keyboard insertion")
                    matched=re.search(r"driver activation ready: device=(\d+), worker=(\d+)",line)
                    if matched:bindings[int(matched[1])]=int(matched[2])
                    matched=re.search(r"driver worker collected: pid=(\d+)",line)
                    if matched:collected.append(int(matched[1]))
                    irq_seen |= "xHCI INTx acknowledged:" in line
                    client_waiting |= "DEVICE waiting first insertion:" in line
                    reuse_waiting |= "DEVICE reuse waiting A" in line
                    host_waiting |= "xHCI waiting for USB connection:" in line or (args.reuse and "xHCI activation ready:" in line)
                    if stage==0 and args.reuse and reuse_waiting and host_waiting:
                        qmp=Qmp(qmp_path)
                        print("DEVICE no HID worker before insertion confirmed",flush=True)
                        qmp.call("device_add", {"driver":"usb-kbd","bus":"xhci.0","id":"reuseA","port":"1"})
                        stage=10
                    elif stage == 0 and not args.reuse and client_waiting and host_waiting:
                        qmp = Qmp(qmp_path)
                        print("DEVICE no HID worker before insertion confirmed", flush=True)
                        qmp.call("device_add", {"driver": "usb-kbd", "bus": "xhci.0", "id": "keyboard1", "port": "1"})
                        stage = 1
                    elif stage in (1, 5) and f"DEVICE keyboard ready: round={(stage + 3) // 4}," in line:
                        qmp.key(True)
                        stage += 1
                    elif stage in (2, 6) and f"DEVICE press observed: round={(stage + 2) // 4}," in line:
                        if stage == 2 and not irq_seen:
                            # 固定virt/PLIC首次真实输入无token时保留只读诊断；不读有副作用的claim。
                            for monitor in ("info registers", "xp /1wx 0xc000084", "xp /1wx 0xc001004", "xp /1wx 0xc002084", "xp /1wx 0xc201000"):
                                observed=qmp.call("human-monitor-command", {"command-line":monitor})
                                print(f"DEVICE IRQ diagnostic {monitor}: {observed}",flush=True)
                        qmp.key(False)
                        stage += 1
                    elif stage in (3, 7) and f"DEVICE release observed: round={(stage + 1) // 4}," in line:
                        device = f"keyboard{(stage + 1) // 4}"
                        qmp.call("device_del", {"id": device})
                        qmp.wait_deleted(device)
                        stage += 1
                    elif stage == 4 and "DEVICE disconnected and binding released: round=1," in line:
                        qmp.call("device_add", {"driver": "usb-kbd", "bus": "xhci.0", "id": "keyboard2", "port": "1"})
                        stage = 5
                    elif stage == 8 and "DEVICE disconnected and binding released: round=2," in line:
                        stage = 9
                    elif "DEVICE hotplug keyboard function acceptance passed" in line:
                        if stage != 9:
                            raise RuntimeError("guest completion preceded confirmed QMP sequence")
                        completed = not args.reuse
                    elif stage == 10 and "DEVICE reuse A ready: serial=" in line:
                        reuse["a"]=int(line.split("serial=",1)[1])
                        qmp.call("device_add", {"driver":"usb-kbd","bus":"xhci.0","id":"reuseB","port":"2"})
                        stage=11
                    elif stage == 11 and "DEVICE reuse B ready: serial=" in line:
                        reuse["b"]=int(line.split("serial=",1)[1])
                    elif stage == 12 and "DEVICE reuse B press observed:" in line:
                        qmp.key(False)
                        stage=13
                    elif stage == 13 and "DEVICE reuse B release observed:" in line:
                        qmp.call("device_del", {"id":"reuseA"})
                        qmp.wait_deleted("reuseA")
                        stage=14
                    elif stage == 14 and "DEVICE reuse A Released with old aliases retained" in line:
                        qmp.key(True)
                        stage=15
                    elif stage == 15 and "DEVICE reuse B after A press observed:" in line:
                        qmp.key(False)
                        stage=16
                    elif stage == 16 and "DEVICE reuse B after A release observed:" in line:
                        qmp.call("device_add", {"driver":"usb-kbd","bus":"xhci.0","id":"reuseC","port":"1"})
                        stage=17
                    elif stage == 17 and "DEVICE reuse C ready; A terminal and B live: serial=" in line:
                        reuse["c"]=int(line.split("serial=",1)[1])
                    elif stage == 18 and "DEVICE reuse C press observed:" in line:
                        qmp.key(False,"b")
                        stage=19
                    elif stage == 19 and "DEVICE reuse C release observed; waiting HID process fault:" in line:
                        inject_process_fault(kernel,gdb_port,reuse["pid"])
                        if args.retry_race:
                            retry_debugger = retry_probe.start(kernel, gdb_port, managers["Driver"], reuse["c"])
                        stage=20
                    elif stage == 20 and "DEVICE reuse B/C Released after process fault" in line:
                        if args.candidates:
                            for device in ("reuseB","reuseC"):
                                qmp.call("device_del", {"id":device})
                                qmp.wait_deleted(device)
                        stage=21
                    elif stage == 21 and not args.candidates and "DEVICE Drivers Control Retry reset terminal selection:" in line:
                        serial = int(line.split("serial=", 1)[1])
                        if serial != reuse.get("c"):
                            raise RuntimeError("Control Retry did not reset the original terminal C identity")
                        retry_verified = True
                        for device in ("reuseB", "reuseC"):
                            qmp.call("device_del", {"id": device})
                            qmp.wait_deleted(device)
                        stage = 22
                    elif "DEVICE multi-binding function acceptance passed" in line:
                        if stage!=21:raise RuntimeError("reuse completion preceded process fault and safe releases")
                        completed=True
                    # InstallFunction可先于manager消费其回复；等待同一日志流补齐真实PID证据。
                    if stage==11 and "b" in reuse and all(reuse[name] in bindings for name in ("a","b")):
                        reuse["pid"]=bindings[reuse["a"]]
                        if bindings[reuse["b"]]!=reuse["pid"]:raise RuntimeError("A/B did not reuse one HID process")
                        print(f"DEVICE shared HID process A/B confirmed: pid={reuse['pid']}",flush=True)
                        qmp.key(True)
                        stage=12
                    if stage==17 and "c" in reuse and reuse["c"] in bindings:
                        if bindings[reuse["c"]]!=reuse["pid"]:raise RuntimeError("C did not reuse original HID process")
                        print(f"DEVICE shared HID process A/B/C confirmed: pid={reuse['pid']}",flush=True)
                        qmp.key(True,"b")
                        stage=18
        if pending:
            print(pending.decode("utf-8", errors="replace"), end="", flush=True)
        status = process.wait(timeout=3)
        if fault_scenario and stage != 31:
            raise RuntimeError("device fault did not reach its authenticated reset or reset-rejection boundary")
        if args.unknown and not fault_scenario and stage != 41:
            raise RuntimeError("reply-loss did not reach normal shutdown")
        if status != 0 or not completed:
            raise RuntimeError(f"device route incomplete: QEMU exit={status}, stage={stage}")
        if args.reuse and collected.count(reuse.get("pid"))!=1:
            raise RuntimeError("shared HID process was not collected exactly once")
        if args.reuse and not args.candidates and not retry_verified:
            raise RuntimeError("released/detached terminal selection Retry was not acknowledged")
        if args.retry_race and (retry_facts is None or not retry_client_verified):
            raise RuntimeError("Retry race lacks both original clients, thread retirement and late-query evidence")
        if args.candidates:
            serials = {reuse.get(name) for name in ("a", "b", "c")}
            if prepare_cancel is None or not candidates.get(prepare_cancel, {}).get("cancel_retired"):
                raise RuntimeError("descriptor-absent reprepare cancellation was not observed")
            serials.add(prepare_cancel)
            if set(candidates) != serials or len(incompatible_pids) != len(serials):
                raise RuntimeError("candidate evidence did not cover cancelled reprepare and three real input connections")
            for record in candidates.values():
                if not record.get("claimed") or collected.count(record.get("pid")) != 1:
                    raise RuntimeError("candidate did not hand off to HID or collect exactly once")
            print("DEVICE incompatible real PCI candidates safely released; same-device newer-epoch HID input and original Pool refund confirmed", flush=True)
        if args.reuse:print("DEVICE shared HID process collected exactly once",flush=True)
        return 0
    finally:
        # 外层失败锚点可能打断正在运行的GDB。短清理段不再接受重复终止，且任一
        # callback失败也必须执行其他owner的清理；先收本次专有VM组，再关调试/运输。
        primary = sys.exc_info()[1]
        try:
            with ExitStack() as cleanup:
                for signum in (signal.SIGTERM, signal.SIGINT):
                    previous = signal.signal(signum, signal.SIG_IGN)
                    cleanup.callback(signal.signal, signum, previous)
                cleanup.callback(private.rmdir)
                for path in (qmp_path, early_target, physical_armed, function_address, irq_target,
                             irq_idle_ready, irq_idle_inserted, mmio_target, mmio_target.with_suffix(".next"),
                             dma_target, dma_target.with_suffix(".next"),
                             irq_exit_target, irq_exit_target.with_suffix(".next")):
                    cleanup.callback(path.unlink, missing_ok=True)
                for debugger in (early_fault, physical_debugger, unknown_probe, function_debugger,
                                 mmio_debugger, dma_debugger, irq_exit_debugger, retry_debugger):
                    if debugger:
                        cleanup.callback(stop_debugger, debugger)
                if qmp:
                    cleanup.callback(qmp.close)
                if process:
                    cleanup.callback(stop_qemu, process)
                selector.close()
        except BaseException as error:
            details = "".join(traceback.format_exception(type(error), error, error.__traceback__))
            cleanup_log = private.with_name(private.name + "-cleanup.log")
            cleanup_log.write_text(f"Original failure: {primary!r}\nCleanup failure:\n{details}")
            print(f"DEVICE cleanup failed; original={primary!r}; details={cleanup_log}", file=sys.stderr, flush=True)
            if primary is None: raise


def interrupted(_signal, _frame):
    raise SystemExit(128 + _signal)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        sys.exit(main())
    except Exception as error:
        print(f"DEVICE acceptance failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
