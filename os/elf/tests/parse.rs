//! ELF64 解析测试（host）：手工构造最小 ELF 镜像。

use elf::{parse, ElfError};

/// 构造含 `segments` 个 PT_LOAD 的 ELF64 镜像（段内容区紧随头表）。
/// 元组：`(offset, vaddr, filesz, memsz, flags)`。
fn build_elf(entry: u64, segments: &[(u64, u64, u64, u64, u32)]) -> Vec<u8> {
    let phoff = 64usize;
    let phnum = segments.len();
    let mut v = vec![0u8; phoff + phnum * 56];
    v[..4].copy_from_slice(b"\x7fELF");
    v[4] = 2; // ELF64
    v[5] = 1; // little-endian
    v[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    v[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    v[24..32].copy_from_slice(&entry.to_le_bytes());
    v[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
    v[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
    v[56..58].copy_from_slice(&(phnum as u16).to_le_bytes());
    for (i, &(offset, vaddr, filesz, memsz, flags)) in segments.iter().enumerate() {
        let p = &mut v[phoff + i * 56..][..56];
        p[0..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        p[4..8].copy_from_slice(&flags.to_le_bytes());
        let mut put = |off: usize, val: u64| p[off..off + 8].copy_from_slice(&val.to_le_bytes());
        put(8, offset);
        put(16, vaddr);
        put(32, filesz);
        put(40, memsz);
    }
    v
}

#[test]
fn parse_load_segments_sorted() {
    // rodata(只读) 在高地址、text(可执行) 在低地址——验证按 vaddr 排序输出。
    let buf = build_elf(
        0x1000,
        &[(0x1200, 0x2000, 16, 16, 0x4), (0, 0x1000, 4096, 8192, 0x5)],
    );
    let elf = parse(&buf).unwrap();
    assert_eq!(elf.entry, 0x1000);
    assert_eq!(elf.segments.len(), 2);
    assert_eq!(elf.segments[0].vaddr, 0x1000, "segments sorted by vaddr");
    assert_eq!(elf.segments[1].vaddr, 0x2000);
    assert_eq!(elf.segments[0].memsz, 8192, "BSS = memsz - filesz");
    assert_eq!(elf.segments[0].offset, 0);
    assert_eq!(elf.segments[1].offset, 0x1200);
    assert!(elf.segments[0].executable && !elf.segments[0].writable);
}

#[test]
fn non_load_headers_skipped() {
    let mut buf = build_elf(0, &[]);
    buf.resize(64 + 2 * 56, 0);
    buf[56..58].copy_from_slice(&2u16.to_le_bytes());
    // 第一个 header：PT_NULL（type=0），应被跳过。
    let p1 = &mut buf[64..][..56];
    p1[0..4].copy_from_slice(&0u32.to_le_bytes());
    // 第二个 header：PT_LOAD，vaddr=0x5000。
    let p2 = &mut buf[64 + 56..][..56];
    p2[0..4].copy_from_slice(&1u32.to_le_bytes());
    p2[4..8].copy_from_slice(&0x6u32.to_le_bytes()); // RW
    p2[16..24].copy_from_slice(&0x5000u64.to_le_bytes());
    p2[32..40].copy_from_slice(&256u64.to_le_bytes());
    p2[40..48].copy_from_slice(&256u64.to_le_bytes());

    let elf = parse(&buf).unwrap();
    assert_eq!(elf.segments.len(), 1);
    assert!(elf.segments[0].writable && !elf.segments[0].executable);
}

#[test]
fn rejects_bad_images() {
    assert_eq!(parse(&[0u8; 16]), Err(ElfError::TooShort));
    let mut buf = build_elf(0, &[]);
    buf[0] = 0x7e;
    assert_eq!(parse(&buf), Err(ElfError::BadMagic));
    let mut buf = build_elf(0, &[]);
    buf[4] = 1; // ELF32
    assert_eq!(parse(&buf), Err(ElfError::BadClass));
    let mut buf = build_elf(0, &[]);
    buf[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    assert_eq!(parse(&buf), Err(ElfError::BadType));
    let mut buf = build_elf(0, &[]);
    buf[18..20].copy_from_slice(&62u16.to_le_bytes()); // x86-64
    assert_eq!(parse(&buf), Err(ElfError::BadMachine));
    let mut buf = build_elf(0, &[]);
    buf[32..40].copy_from_slice(&0xdead0000u64.to_le_bytes()); // phoff 越界
    assert_eq!(parse(&buf), Err(ElfError::BadPhoff));
}
