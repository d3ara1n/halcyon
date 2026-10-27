"""有限旁观原PCI Query完成和同advance的下一次提交，不在首次submit前暂停。"""
import hashlib
import json
from pathlib import Path
import re
import subprocess


def program():
    elf = Path("artifacts/build/drv_pci_generic").resolve()
    source = "drivers/drv_pci_generic/src/server/engine.rs"
    result = subprocess.check_output([
        "riscv64-elf-gdb", "-q", "-nx", "--batch", str(elf),
        "-ex", f"info line {source}:235", "-ex", f"info line {source}:485",
    ], text=True, stderr=subprocess.STDOUT)
    points = {name: int(re.search(rf"Line {line} .*starts at address (0x[0-9a-f]+)", result)[1], 16)
              for name, line in (("completion", 235), ("submit", 485))}
    facts = {"elf": str(elf), "elf_sha256": hashlib.sha256(elf.read_bytes()).hexdigest(),
             "points": points, "dwarf": result,
             "scope": "original Input.now_ns and next_poll; first submit not paused; no elapsed-time claim"}
    return CODE.replace("__ELF__", json.dumps(str(elf))).replace("__POINTS__", repr(points)), facts


CODE = r'''
gdb.execute('add-symbol-file '+json.dumps(__ELF__),to_string=True)
irq_poll_points=__POINTS__
irq_poll_armed=False
def irq_poll_owner():
    # 当前是用户PC，不把用户tp当HartLocal；只用首个kernel帧固定的物理布局。
    address,size,stride,_=irq_idle_layout
    raw=physical_word(address,size).to_bytes(size,'little')
    hartid=int(gdb.parse_and_eval('$mhartid'))
    for offset in range(0,size,stride):
        if not struct.unpack_from('<Q',raw,offset+hart_offsets['emergency_sp'])[0]:continue
        if struct.unpack_from('<Q',raw,offset+hart_offsets['hartid'])[0]!=hartid:continue
        thread=struct.unpack_from('<Q',raw,offset+hart_offsets['current_thread'])[0]
        if not thread:return None,None
        shell=word(thread+process_offset+ptr_offset+pointer_offset)
        return thread,word(shell+data_offset+pid_offset)
    raise gdb.GdbError('PCI diagnostic CPU has no admitted HartLocal')
def irq_poll_arm():
    global irq_poll_armed
    if not irq_poll_armed and facts.get('irq_idle_snapshots',0)>=2 and 'irq_idle' not in facts:
        irq_poll_armed=True
        irq_poll_completion.enabled=True
class PciQuery(gdb.Breakpoint):
    def __init__(self,completion):
        self.completion=completion
        super().__init__('*%#x'%irq_poll_points['completion' if completion else 'submit'],type=gdb.BP_HARDWARE_BREAKPOINT)
        self.enabled=False
        self.silent=True
    def stop(self):
        global failure
        try:
            thread,pid=irq_poll_owner()
            if pid!=provider:return False
            frame=gdb.newest_frame()
            engine=frame.read_var('self').dereference()
            input_=frame.read_var('input').dereference()
            event={'pid':pid,'thread':thread,'engine':int(engine.address),'task':int(frame.read_var('id')),
                   'advance_sp':int(gdb.parse_and_eval('$sp'))&((1<<64)-1),'input_now_ns':int(input_['now_ns']),
                   'next_poll_ns':int(engine['next_poll']['at_ns']),'pc':hex(int(gdb.parse_and_eval('$pc'))&((1<<64)-1))}
            observed=facts.setdefault('pci_query_poll',{})
            if self.completion:
                event['txid']=int(frame.read_var('txid'))
                event['expired_in_original_input']=event['input_now_ns']>=event['next_poll_ns']
                observed['completion']=event
                self.enabled=False
                irq_poll_submit.enabled=True
            else:
                original=observed['completion']
                event['same_advance']=all(event[key]==original[key] for key in ('thread','engine','task','advance_sp','input_now_ns'))
                observed['next_submit']=event
                self.enabled=False
            EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
            print('PCI_QUERY_OWNER_PROGRESS '+json.dumps(observed,sort_keys=True))
            return False
        except Exception as error:
            failure=str(error)
            return True
irq_poll_completion=PciQuery(True)
irq_poll_submit=PciQuery(False)
'''
