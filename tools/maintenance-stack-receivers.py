#!/usr/bin/env python3
"""提取Arc Work receiver最后drop的地址别名证据；不泛化到同类型其他对象。"""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess

spec=importlib.util.spec_from_file_location('sites',Path(__file__).with_name('maintenance-stack-sites.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)


def stack_argument(instructions, site):
    index=next(i for i,row in enumerate(instructions) if row['site']==site)
    register='a0'
    for row in reversed(instructions[max(0,index-12):index]):
        operands=row['operands'].split('#')[0].strip()
        match=re.fullmatch(r'(\w+),sp,(-?\d+)',operands)
        if row['opcode']=='addi' and match and match[1]==register:return int(match[2])
        match=re.fullmatch(r'(\w+),sp',operands)
        if row['opcode']=='mv' and match and match[1]==register:return 0
        match=re.fullmatch(r'(\w+),(\w+)',operands)
        if row['opcode']=='mv' and match and match[1]==register:
            register=match[2];continue
        if row['opcode'] in ('jal','jalr') or (operands.split(',')[0]==register and row['opcode'] not in ('sd','sw','sb')):
            return None
    return None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('facts',type=Path);parser.add_argument('output',type=Path)
    args=parser.parse_args();facts=json.loads(args.facts.read_text());symbols=facts['symbols']
    command=['riscv64-elf-gdb','-q','-nx','--batch',facts['elf']]
    steps=[]
    for a,row in symbols.items():
        match=re.fullmatch(r'<(.+) as kernel::maintenance::Work>::step',row['name'])
        if not match:continue
        steps.append((a,row,match[1]));command += ['-ex',f'echo RECEIVER {a}\\n','-ex',f'info scope *{int(a,16)+4:#x}']
    completed=subprocess.run(command,capture_output=True,text=True)
    if completed.returncode:raise RuntimeError(completed.stderr)
    blocks=re.split(r'RECEIVER (0x[0-9a-f]+)\n',completed.stdout)
    locations={}
    for i in range(1,len(blocks),2):
        match=re.search(r'Symbol self is a variable at frame base reg \$sp offset 0\+(-?\d+), length 8\.',blocks[i+1])
        if match:locations[blocks[i]]=int(match[1])
    witnesses=[]
    for a,row,kind in steps:
        slot=locations.get(a)
        for call in row['calls']:
            target=module.normalize(symbols[call['target']]['name'])
            if target!='core::ptr::drop_glue<alloc::sync::Arc<'+kind+'>>':continue
            offset=stack_argument(row['instructions'],call['site'])
            witnesses.append(dict(caller=a,name=row['name'],site=call['site'],target=call['target'],type=kind,
                dwarf_self_slot=slot,a0_stack_offset=offset,receiver=slot is not None and offset==slot,
                premise='此site的&mut Arc准确别名DWARF receiver槽；其他同类型Arc仍保留完整最后析构'))
    args.output.write_text(json.dumps(dict(sha256=facts['sha256'],witnesses=witnesses,scope_raw=completed.stdout),indent=2,ensure_ascii=False)+'\n')
    print('receiver drop witnesses:',sum(w['receiver'] for w in witnesses),'of',len(witnesses))
    for w in witnesses:print(w['receiver'],w['type'],w['site'],w['dwarf_self_slot'],w['a0_stack_offset'])


if __name__=='__main__':main()
