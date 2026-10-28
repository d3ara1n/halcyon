"""原 core 消费者的 fs 双事实顺序取证；仅用 vCont 调度，不伪造报告或 DONE。"""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys

import qemu_gdb as debug


def verify_final_log(facts, transcript):
    """用同一串行 root 的后继实际时钟，为 primary 领取时刻提供总期限内的上界。"""
    lines = transcript.splitlines()
    committed = next((i, re.search(r"SYSTEM stop committed: services sealed, deadline=(\d+)", line))
                     for i, line in enumerate(lines) if "SYSTEM stop committed:" in line)
    primary = next(i for i, line in enumerate(lines)
                   if "FS stop report authenticated: role=Filesystem," in line)
    later = next((i, re.search(r"driver assembly stop started: now_ns=(\d+),", line))
                 for i, line in enumerate(lines) if "driver assembly stop started:" in line)
    reset = next(i for i, line in enumerate(lines)
                 if "system reset accepted: action Shutdown, reason Requested;" in line)
    if not committed[0] < primary < later[0] < reset:
        raise RuntimeError("primary consumption and later root clock are not from the same stop sequence")
    deadline = int(committed[1][1])
    upper = int(later[1][1])
    if upper >= deadline:
        raise RuntimeError("primary consumption lacks a measured upper bound before the aggregate deadline")
    if "stage_expired=true" not in facts["done_first"]["message"]:
        raise RuntimeError("primary DONE-first consumption did not use the original expiry recheck")
    delay = facts["root_delay"]
    if delay["ticks"] * 1_000_000_000 < delay["stage_timeout_ns"] * delay["timebase_hz"]:
        raise RuntimeError("actual guest tick delta is shorter than the original phase window")
    facts["aggregate_deadline"] = {
        "deadline_ns": deadline,
        "primary_consumption_upper_ns": upper,
        "upper_bound_source": "later serial root driver-stop snapshot after filesystem collection",
        "remaining_at_upper_ns": deadline - upper,
        "log_lines": {"committed": committed[0] + 1, "primary": primary + 1,
                      "later_clock": later[0] + 1, "requested_reset": reset + 1},
    }
    facts["requested_reset"] = True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("QEMU command missing")
    kernel = Path(os.environ["ERHINO_KERNEL_ELF"])
    logs = debug.fresh_logs(Path("artifacts/fs-stop-order"))
    evidence = logs / "evidence.json"
    symbols = debug.symbols(kernel)
    settle = next(name for name in symbols if "ProcessShell" in name and name.endswith("::step_settle"))
    hint = next(name for name in symbols if "wait_set::WaitSet" in name and "HintTarget" in name and name.endswith("::hint"))
    policy = Path("user/services/srv_fs/src/deployment.rs").read_text()
    timeout_ns = int(re.search(r"pub const STOP_TIMEOUT_NS: u64 = ([0-9_]+);", policy)[1].replace("_", ""))
    dts = Path("os/platforms/qemu/virt/device.dts").read_text()
    frequency = int(re.search(r"timebase-frequency\s*=\s*<(0x[0-9a-fA-F]+|[0-9]+)>", dts)[1], 0)
    facts = {"stage_timeout_ns": timeout_ns, "timebase_hz": frequency, "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
             "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
             "scope": "original core: secondary Stopped consumption before DONE; primary already-complete facts consumed DONE-first by original expiry recheck after root scheduling delay; no state/result mutation"}
    evidence.write_text(json.dumps(facts, indent=2) + "\n")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    substitutions = {"KERNEL": json.dumps(str(kernel.resolve())), "PORT": str(port),
                     "CONTEXT": debug.user_trap_context(kernel, symbols["handle_user_trap"]),
                     "USER_MEMORY": debug.USER_MEMORY, "TRAP": hex(symbols["handle_user_trap"]),
                     "SETTLE": hex(symbols[settle]), "HINT": hex(symbols[hint]), "PRINT": hex(symbols["kernel::syscall::debug_print"]), "EVIDENCE": json.dumps(str(evidence.resolve()))}
    script = SCRIPT
    for key, value in substitutions.items():
        script = script.replace("__" + key + "__", value)
    for body in re.findall(r"\npython\n(.*?)\nend\n", script, re.S):
        ast.parse(body)
    script_path = logs / "order.gdb"
    script_path.write_text(script)
    guest = debugger = None
    try:
        with (logs / "qemu.log").open("w") as output, (logs / "gdb.log").open("w") as transcript:
            guest = subprocess.Popen(command + ["-S", "-gdb", f"tcp:127.0.0.1:{port}"],
                                     stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            debugger = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)],
                                        stdout=transcript, stderr=subprocess.STDOUT, start_new_session=True)
            status = debugger.wait(timeout=args.timeout)
            if status or not json.loads(evidence.read_text()).get("verified"):
                raise RuntimeError(f"fs stop ordering evidence incomplete: {logs}")
            status = guest.wait(timeout=args.timeout)
            if status:
                raise RuntimeError(f"core failed after fs ordering observation: {logs}")
        # 外层原 core 判定器继续核全部业务锚点及 Requested reset。
        transcript = (logs / "qemu.log").read_text()
        facts = json.loads(evidence.read_text())
        verify_final_log(facts, transcript)
        evidence.write_text(json.dumps(facts, indent=2) + "\n")
        sys.stdout.write(transcript)
        print(f"FS stop order verified: secondary Stopped-first; primary DONE-first at original expiry recheck; evidence: {logs}")
    finally:
        if debugger is not None:
            debug.stop_group(debugger)
        if guest is not None:
            debug.stop_group(guest)


SCRIPT = r'''set pagination off
set confirm off
set breakpoint always-inserted on
set language rust
set tcp auto-retry on
set tcp connect-timeout 10
file __KERNEL__
target remote 127.0.0.1:__PORT__
python
import gdb,json,re,threading,signal,os
from pathlib import Path
EVIDENCE=Path(__EVIDENCE__)
facts=json.loads(EVIDENCE.read_text())
phase='await-secondary-report'
pids={}
tokens={}
stop_role=None
held=None
shell=None
__CONTEXT__
__USER_MEMORY__
def save():EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
def partial(excluded):
    actions=['c:p%x.%x'%t.ptid[:2] for t in gdb.selected_inferior().threads() if t.ptid!=excluded]
    packet='vCont;'+';'.join(actions)
    response=gdb.execute('maintenance packet '+packet,to_string=True)
    facts.setdefault('vCont',[]).append({'packet':packet,'response':response});save()
    match=re.search(r'thread:p([0-9a-f]+)\.([0-9a-f]+);',response)
    if match is None:raise gdb.GdbError('partial resume lacks an actual stopped CPU')
    stopped=tuple(int(part,16) for part in match.groups())
    gdb.execute('maintenance flush register-cache')
    next(t for t in gdb.selected_inferior().threads() if t.ptid[:2]==stopped).switch()
    gdb.execute('maintenance flush register-cache')
    gdb.parse_and_eval('$pc')
class Trap(gdb.Breakpoint):
    def stop(self):
        global phase,stop_role,held
        if int(gdb.parse_and_eval('$a0'))!=8:return False
        anchor=int(gdb.parse_and_eval('$tp')) & ((1<<64)-1)
        pointer=word(anchor+hart_offsets['current_thread'])
        if not pointer:return False
        process=word(pointer+process_offset+ptr_offset+pointer_offset)
        pid=word(process+data_offset+pid_offset)
        frame=int(gdb.parse_and_eval('$a2')) & ((1<<64)-1)
        if frame!=pointer+thread_offsets['frame']:raise gdb.GdbError('trap frame owner mismatch')
        registers=frame+context_offsets['x']
        call=word(registers+17*8)
        if phase=='await-primary-receive' and pid==1 and call==0x3b and stop_role=='Filesystem':
            held=gdb.selected_thread().ptid
            facts['primary_receive_held']={'pid':pids['Filesystem'],'hart':list(held)};save()
            return True
        return False
class Console(gdb.Breakpoint):
    def stop(self):
        global stop_role
        frame=int(gdb.parse_and_eval('$a0')) & ((1<<64)-1)
        pointer=int(gdb.parse_and_eval('$a1')) & ((1<<64)-1)
        process=word(pointer+process_offset+ptr_offset+pointer_offset)
        if word(process+data_offset+pid_offset)!=1:return False
        if frame!=pointer+thread_offsets['frame']:raise gdb.GdbError('console frame owner mismatch')
        registers=frame+context_offsets['x']
        length=word(registers+11*8)
        if length>4096:raise gdb.GdbError('unexpected debug message size')
        message=user_memory(word(pointer+thread_offsets['satp']),word(registers+10*8),length).decode(errors='replace')
        match=re.search(r'instance observer admitted before Start: role=(Filesystem|Secondary), pid=(\d+), token=(\d+)',message)
        if match:
            pids[match[1]]=int(match[2])
            tokens[match[1]]=int(match[3])
        match=re.search(r'instance observer normal stop intent: role=(Filesystem|Secondary),',message)
        if match:
            stop_role=match[1]
            if stop_role=='Filesystem' and phase=='await-primary-receive':trap.enabled=True
        if phase=='await-secondary-report' and 'FS stop report authenticated: role=Secondary,' in message:
            if 'DONE_seen=false' not in message:raise gdb.GdbError('secondary DONE had already been consumed before the observed Stopped report')
            facts['stopped_first']={'pid':pids['Secondary'],'message':message};save()
            return True
        if phase=='verify-primary' and 'FS stop report authenticated: role=Filesystem,' in message:
            if 'DONE_seen=true' not in message:raise gdb.GdbError('primary Stopped was consumed before the actual DONE snapshot')
            facts['done_first']={'pid':pids['Filesystem'],'message':message};save()
            return True
        return False
trap=Trap('*__TRAP__',type=gdb.BP_HARDWARE_BREAKPOINT)
trap.enabled=False
console=Console('*__PRINT__',type=gdb.BP_HARDWARE_BREAKPOINT)
settle=gdb.Breakpoint('*__SETTLE__',type=gdb.BP_HARDWARE_BREAKPOINT)
settle.enabled=False
hint=gdb.Breakpoint('*__HINT__',type=gdb.BP_HARDWARE_BREAKPOINT)
hint.enabled=False
end
continue
python
if 'stopped_first' not in facts:raise gdb.GdbError('secondary Stopped-before-DONE consumption window absent')
phase='await-primary-receive'
trap.enabled=False
end
continue
python
if phase!='await-primary-receive' or stop_role!='Filesystem':raise gdb.GdbError('primary pre-receive window absent')
trap.enabled=False
console.enabled=False
settle.enabled=True
partial(held)
if int(gdb.parse_and_eval('$pc')) & ((1<<64)-1) != __SETTLE__:raise gdb.GdbError('partial resume missed actual settle function')
shell=int(gdb.parse_and_eval('$a0')) & ((1<<64)-1)
if word(shell+pid_offset)!=pids['Filesystem']:raise gdb.GdbError('actual settle belongs to another process')
# 继续原退休 Work，直到同一 DONE token 的真实 Hint 已到集合；仅 Dead 不保证提示已传播。
settle.enabled=False
hint.enabled=True
while True:
    partial(held)
    if int(gdb.parse_and_eval('$pc')) & ((1<<64)-1) != __HINT__:raise gdb.GdbError('partial resume missed actual WaitSet hint')
    target_set=int(gdb.parse_and_eval('$a0')) & ((1<<64)-1)
    token=int(gdb.parse_and_eval('$a1')) & ((1<<64)-1)
    hint.enabled=False
    gdb.execute('set scheduler-locking on')
    gdb.execute('finish',to_string=True)
    gdb.execute('set scheduler-locking off')
    if token==tokens['Filesystem']:
        facts['done_hint']={'set':hex(target_set),'token':token,'enqueued_by_native_hint':True};save()
        break
    hint.enabled=True
state=gdb.Value(shell).cast(gdb.lookup_type('kernel::task::process::shell::ProcessShell').pointer()).dereference()['state']['data']['value']['state']
if int(state)!=3:raise gdb.GdbError('primary has not reached actual Dead')
facts['primary_settled']={'pid':pids['Filesystem'],'shell':hex(shell),'state':int(state)};save()
# Runtime 每轮输入预算可先领取旧报告提示。真实完成事实已齐，再延迟 root 超过原文件阶段窗口，
# 以原到期复查验证 DONE-first 消费及完成事实优先；不修改 Deadline、消息、状态或任务预算。
if gdb.selected_thread().ptid==held:raise gdb.GdbError('delay CPU is the held root CPU')
gdb.execute('set scheduler-locking on')
start_tick=int(gdb.parse_and_eval('$time'))
delay=facts['stage_timeout_ns']/1_000_000_000+.5
timer=threading.Timer(delay,lambda:os.kill(os.getpid(),signal.SIGINT))
with gdb.blocked_signals():timer.start()
try:
    try:gdb.execute('continue',to_string=True)
    except KeyboardInterrupt:pass
finally:
    timer.cancel();timer.join()
    gdb.execute('set scheduler-locking off')
elapsed=int(gdb.parse_and_eval('$time'))-start_tick
if elapsed*1_000_000_000 < facts['stage_timeout_ns']*facts['timebase_hz']:
    raise gdb.GdbError('guest clock did not reach the original phase window')
facts['root_delay']={'ticks':elapsed,'timebase_hz':facts['timebase_hz'],'stage_timeout_ns':facts['stage_timeout_ns']};save()
phase='verify-primary'
console.enabled=True
end
continue
python
if 'stopped_first' not in facts or 'done_first' not in facts:raise gdb.GdbError('both report orders were not observed')
facts['verified']=True;save()
print('FS_STOP_BOTH_ORDERS_VERIFIED real Stopped and actual DONE')
end
detach
quit 0
'''

if __name__ == "__main__":
    main()
