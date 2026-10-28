"""正式 System 部署的有限观察：真实根来源停驻，不把外部收割记作正常停止。"""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import time

import qemu_gdb as debug


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("QEMU command missing")
    kernel = Path(os.environ["ERHINO_KERNEL_ELF"])
    package = Path(os.environ["ERHINO_BOOT_PACKAGE"])
    logs = debug.fresh_logs(Path("artifacts/system-observe"))
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    facts = {"kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
             "boot_package_sha256": hashlib.sha256(package.read_bytes()).hexdigest(),
             "scope": "finite System observation; actual init WaitSet wait and attached infinite source; external cleanup is not normal shutdown"}
    evidence = logs / "evidence.json"
    evidence.write_text(json.dumps(facts, indent=2) + "\n")
    symbols = debug.symbols(kernel)
    script = SCRIPT.replace("__KERNEL__", json.dumps(str(kernel.resolve()))).replace("__PORT__", str(port))
    script = script.replace("__CONTEXT__", debug.user_trap_context(kernel, symbols["handle_user_trap"]))
    script = script.replace("__USER_MEMORY__", debug.USER_MEMORY).replace("__TRAP__", hex(symbols["handle_user_trap"]))
    script = script.replace("__EVIDENCE__", json.dumps(str(evidence.resolve())))
    for body in re.findall(r"\npython\n(.*?)\nend\n", script, re.S):
        ast.parse(body)
    script_path = logs / "observe.gdb"
    script_path.write_text(script)
    guest = debugger = None
    try:
        with (logs / "qemu.log").open("w") as output, (logs / "gdb.log").open("w") as transcript:
            guest = subprocess.Popen(command + ["-S", "-gdb", f"tcp:127.0.0.1:{port}"],
                                     stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            debugger = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)],
                                        stdout=transcript, stderr=subprocess.STDOUT, start_new_session=True)
            status = debugger.wait(timeout=args.timeout)
            if status or not json.loads(evidence.read_text()).get("root_parked"):
                raise RuntimeError(f"System source observation incomplete: {logs}")
            # GDB 已 detach，让原根与服务自然运行；有限观察不宣称无限活性。
            end = time.monotonic() + 2
            while time.monotonic() < end:
                if guest.poll() is not None:
                    raise RuntimeError("System exited after reaching idle")
                time.sleep(.1)
        text = (logs / "qemu.log").read_text()
        for marker in ("SYSTEM foundation ready:", "profile=System", "role=Device", "role=Driver", "role=Filesystem"):
            if marker not in text:
                raise RuntimeError(f"System missing {marker}: {logs}")
        if any(marker in text for marker in ("acceptance workload:", "system reset accepted:", "system failure terminal", "Panicking")):
            raise RuntimeError(f"System ran acceptance or entered a terminal: {logs}")
        facts = json.loads(evidence.read_text())
        facts.update(verified=True, cleanup="external process group cleanup, not normal shutdown")
        evidence.write_text(json.dumps(facts, indent=2) + "\n")
        print(f"System finite observation passed: three required hosts, actual root infinite source wait; evidence: {logs}")
    finally:
        if debugger is not None:
            debug.stop_group(debugger)
        if guest is not None:
            debug.stop_group(guest)


SCRIPT = r'''set pagination off
set confirm off
set language rust
set tcp auto-retry on
set tcp connect-timeout 10
file __KERNEL__
target remote 127.0.0.1:__PORT__
python
import gdb,json,struct,threading,signal,os
from pathlib import Path
EVIDENCE=Path(__EVIDENCE__)
ready=False
last_set=None
root_thread=None
__CONTEXT__
__USER_MEMORY__
class RootWait(gdb.Breakpoint):
    def stop(self):
        global ready,last_set,root_thread
        if int(gdb.parse_and_eval('$a0'))!=8:return False
        anchor=int(gdb.parse_and_eval('$tp')) & ((1<<64)-1)
        pointer=word(anchor+hart_offsets['current_thread'])
        if not pointer:return False
        shell=word(pointer+process_offset+ptr_offset+pointer_offset)
        if word(shell+data_offset+pid_offset)!=1:return False
        frame=int(gdb.parse_and_eval('$a2')) & ((1<<64)-1)
        if frame!=pointer+thread_offsets['frame']:raise gdb.GdbError('root frame owner mismatch')
        registers=frame+context_offsets['x']
        call=word(registers+17*8)
        satp=word(pointer+thread_offsets['satp'])
        if call==0x01:
            length=word(registers+11*8)
            if length<4096:
                message=user_memory(satp,word(registers+10*8),length)
                ready |= b'SYSTEM foundation ready:' in message
        if call==0x3b:last_set=word(registers+10*8)
        if call!=0x32 or not ready:return False
        request=user_memory(satp,word(registers+10*8),32)
        handle,signals,kind,reserved,at=struct.unpack('<QQIIQ',request)
        if handle!=last_set or kind!=0 or reserved or at:return False
        root_thread=pointer
        facts=json.loads(EVIDENCE.read_text())
        facts.update(root_wait={'pid':1,'thread':hex(pointer),'set_handle':hex(handle),'interest':signals,'deadline':'Infinite'})
        EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
        self.enabled=False
        return True
RootWait('*__TRAP__',type=gdb.BP_HARDWARE_BREAKPOINT)
end
continue
python
if root_thread is None:raise gdb.GdbError('root never entered its infinite Assembly wait')
# 放行原 WaitOne，随后普通 all-stop 核实际来源 loan；不改客体 PC、CSR 或状态。
timer=threading.Timer(.5,lambda:os.kill(os.getpid(),signal.SIGINT))
with gdb.blocked_signals():timer.start()
try:
    try:gdb.execute('continue',to_string=True)
    except KeyboardInterrupt:pass
finally:
    timer.cancel()
    timer.join()
def optional(value):
    name=next((field.name for field in value.type.fields() if field.name),None)
    if name=='None':return None
    if name!='Some':raise gdb.GdbError('unexpected root wait Option layout')
    inner=value['Some']
    return inner[next(field.name for field in inner.type.fields() if field.name)]
thread=gdb.Value(root_thread).cast(thread_type.pointer()).dereference()
entry=optional(thread['wait_entry']['value'])
if entry is None or not bool(entry['source_attached']) or bool(entry['timer_prepared']) or optional(entry['expires_at']) is not None or optional(entry['source']) is None:
    raise gdb.GdbError('root did not retain an actual infinite source wait')
facts=json.loads(EVIDENCE.read_text())
facts.update(root_parked=True,wait_epoch=int(entry['epoch']),source_attached=True,timer_prepared=False)
EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
print('SYSTEM_ROOT_SOURCE_PARKED actual attached source, no deadline timer')
end
detach
quit 0
'''

if __name__ == "__main__":
    main()
