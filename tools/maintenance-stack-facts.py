#!/usr/bin/env python3
"""提取固定ELF的CFI/控制转移事实；不自行签发生命周期剪边或累计栈上界。"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import struct


def command(*args):
    return subprocess.check_output(args, text=True)


def extract(elf):
    image = elf.read_bytes()
    if image[:6] != b'\x7fELF\x02\x01':
        raise ValueError('expected ELF64 little-endian')
    section_offset = struct.unpack_from('<Q',image,40)[0]
    section_stride, section_count = struct.unpack_from('<HH',image,58)
    readonly=[]
    for index in range(section_count):
        start=section_offset+index*section_stride
        flags,address,offset,size=struct.unpack_from('<QQQQ',image,start+8)
        if flags & 2 and not flags & 5:
            readonly.append((address,image[offset:offset+size]))
    symbols = {}
    aliases = {};absolutes={}
    for line in command('riscv64-elf-nm', '-S', '-C', str(elf)).splitlines():
        absolute=re.match(r'([0-9a-f]+)\s+A\s+(\S+)$',line)
        if absolute:absolutes[absolute[2]]=int(absolute[1],16)
        match = re.match(r'([0-9a-f]+) ([0-9a-f]+) [tTwW] (.*)', line)
        if match:
            address, size, name = int(match[1], 16), int(match[2], 16), match[3]
            if address in symbols:
                aliases.setdefault(hex(address), []).append(name)
            else:
                symbols[address] = dict(name=name, size=size, calls=[], dynamic=[], local=[], returns=[], instructions=[])
    cfi = command('riscv64-elf-readelf', '--debug-dump=frames', str(elf))
    for block in re.split(r'\n(?=[0-9a-f]+ )', cfi):
        match = re.search(r'FDE .*pc=([0-9a-f]+)\.\.([0-9a-f]+)', block)
        if not match:
            continue
        start, end = int(match[1], 16), int(match[2], 16)
        owners = [a for a, row in symbols.items() if a <= start < a + row['size']]
        if len(owners) != 1:
            continue
        row = symbols[owners[0]]
        offsets = [int(x) for x in re.findall(r'DW_CFA_def_cfa_offset: (\d+)', block)]
        offsets += [int(x) for x in re.findall(r'DW_CFA_def_cfa: r2 \(sp\) ofs (\d+)', block)]
        row['frame'] = max(row.get('frame', 0), *([0] + offsets))
        row.setdefault('cfi_ranges', []).append([hex(start), hex(end)])
        unsupported = [line.strip() for line in block.splitlines() if re.search(r'DW_CFA_(?:def_cfa_expression|expression|val_expression|def_cfa_sf|def_cfa_offset_sf)', line)]
        registers = re.findall(r'DW_CFA_def_cfa_register: (.*)', block)
        unsupported += [register for register in registers if register not in ('r2 (sp)',)]
        definitions = re.findall(r'DW_CFA_def_cfa: (.*)', block)
        unsupported += [definition for definition in definitions if not re.fullmatch(r'r2 \(sp\) ofs \d+',definition)]
        row.setdefault('unsupported_cfi', []).extend(unsupported)
    current = None
    assembly = command('riscv64-elf-objdump', '-d', '-C', str(elf))
    unresolved = []
    for line in assembly.splitlines():
        label = re.match(r'([0-9a-f]+) <.*>:', line)
        if label:
            current = int(label[1], 16)
            continue
        if current not in symbols:
            continue
        instruction = re.match(r'\s*([0-9a-f]+):\s+([0-9a-f]+)\s+(\S+)(?:\s+(.*))?', line)
        if not instruction:
            continue
        site, encoding, opcode, operands = instruction[1], instruction[2], instruction[3], instruction[4] or ''
        row = symbols[current]
        row['instructions'].append(dict(site='0x'+site, encoding=encoding, opcode=opcode, operands=operands))
        if opcode == 'ret' or (opcode in ('jr', 'jalr') and operands in ('ra', 'zero,0(ra)')):
            row['returns'].append('0x'+site)
            continue
        branches = ('beq', 'bne', 'blt', 'bge', 'bltu', 'bgeu', 'beqz', 'bnez', 'bgez', 'bltz', 'blez', 'bgtz', 'bgt', 'ble', 'bgtu', 'bleu')
        if opcode not in ('jalr', 'jal', 'jr', 'j') + branches:
            continue
        record = dict(site='0x'+site, opcode=opcode, instruction=opcode+' '+operands)
        target = re.search(r'(?:#\s*|^|,)([0-9a-f]{8,})\s*<', operands)
        if target:
            address = int(target[1], 16)
            record['target'] = hex(address)
            if current <= address < current + row['size']:
                row['local'].append(record)
                if address == current and opcode in ('jal', 'jalr'):
                    row.setdefault('self_calls', []).append('0x'+site)
            elif address in symbols:
                row['calls'].append(record)
            else:
                unresolved.append(dict(caller=hex(current), name=row['name'], **record))
        elif opcode in ('jalr', 'jr'):
            record['preceding_instructions'] = row['instructions'][-25:-1]
            row['dynamic'].append(record)
        else:
            unresolved.append(dict(caller=hex(current), name=row['name'], **record))
    for row in symbols.values():
        if 'frame' not in row and len(row['instructions']) == 1 and row['instructions'][0]['opcode'] == 'ret':
            row['frame'] = 0
            row['cfi_leaf_lemma'] = 'fixed disassembly is exactly ret, no stack movement or call'
        functions=set();vtables={}
        for instruction in row['instructions']:
            if instruction['opcode'] not in ('addi','ld','lui'):
                continue
            reference=re.search(r'#\s*([0-9a-f]+)\s*<',instruction['operands'])
            if not reference:continue
            address=int(reference[1],16)
            if address in symbols:functions.add(hex(address))
            for base,data in readonly:
                offset=address-base
                if offset<0 or offset+32>len(data):continue
                drop,size,align,method=struct.unpack_from('<QQQQ',data,offset)
                if (drop==0 or drop in symbols) and align and not align & (align-1) and method in symbols:
                    targets=[]
                    cursor=offset+24
                    while cursor+8<=len(data):
                        target=struct.unpack_from('<Q',data,cursor)[0]
                        if target not in symbols:break
                        targets.append(hex(target));cursor+=8
                    vtables[hex(address)]=targets
        row['constant_functions']=sorted(functions)
        row['constant_vtables']=vtables
    return dict(
        elf=str(elf), sha256=hashlib.sha256(elf.read_bytes()).hexdigest(),
        toolchain={tool: command(tool, '--version').splitlines()[0] for tool in ('riscv64-elf-nm', 'riscv64-elf-objdump', 'riscv64-elf-readelf')},
        symbols={hex(a): row for a, row in symbols.items()}, aliases=aliases,
        absolute_symbols=absolutes,
        unresolved_direct=unresolved,
    ), cfi


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('elf', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    report, cfi = extract(args.elf)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False))
    args.output.with_suffix('.cfi.txt').write_text(cfi)
    rows = report['symbols'].values()
    print('CFI facts:', len(report['symbols']), 'symbols;', sum('frame' in row for row in rows), 'with CFI;', len(report['unresolved_direct']), 'unresolved direct transfers')
    print('ELF:', report['sha256'], 'output:', args.output)


if __name__ == '__main__':
    main()
