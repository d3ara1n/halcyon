//! 静态 ELF admission 测试：program-header 分类、装载几何、页权限与入口。

use elf::{ElfError, IsaRequirement, LoadLimits, validate};

const PAGE_SIZE: u64 = 4096;
const IMAGE_LIMIT: u64 = 1 << 30;
const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_RISCV_ATTRIBUTES: u32 = 0x7000_0003;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

#[derive(Clone, Copy)]
struct Header {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

impl Header {
    const fn load(offset: u64, vaddr: u64, filesz: u64, memsz: u64, flags: u32) -> Self {
        Self {
            kind: PT_LOAD,
            flags,
            offset,
            vaddr,
            filesz,
            memsz,
            align: PAGE_SIZE,
        }
    }
}

fn attributes() -> Vec<u8> {
    let arch = b"rv64i2p1_m2p0_a2p1_c2p0_zicsr2p0_zifencei2p0_zicntr2p0\0";
    let body_len = 1 + arch.len();
    let subsection_size = 1 + 4 + body_len;
    let mut data = vec![b'A', 0, 0, 0, 0];
    data.extend_from_slice(b"riscv\0");
    data.push(1); // Tag_File
    data.extend_from_slice(&(subsection_size as u32).to_le_bytes());
    data.push(5); // Tag_RISCV_arch
    data.extend_from_slice(arch);
    let len = data.len() as u32;
    data[1..5].copy_from_slice(&len.to_le_bytes());
    data
}

fn build_elf(entry: u64, headers: &[Header]) -> Vec<u8> {
    let phoff = 64usize;
    let phend = phoff + headers.len() * 56;
    let file_end = headers
        .iter()
        .filter_map(|header| usize::try_from(header.offset.checked_add(header.filesz)?).ok())
        .max()
        .unwrap_or(phend)
        .max(phend);
    let attributes = attributes();
    let attr_offset = file_end;
    let shoff = attr_offset + attributes.len();
    let mut image = vec![0u8; shoff + 64];
    image[..4].copy_from_slice(b"\x7fELF");
    image[4] = 2; // ELF64
    image[5] = 1; // little-endian
    image[6] = 1; // EV_CURRENT
    image[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    image[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    image[20..24].copy_from_slice(&1u32.to_le_bytes()); // EV_CURRENT
    image[24..32].copy_from_slice(&entry.to_le_bytes());
    image[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
    image[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
    image[48..52].copy_from_slice(&1u32.to_le_bytes()); // EF_RISCV_RVC
    image[52..54].copy_from_slice(&64u16.to_le_bytes());
    image[54..56].copy_from_slice(&56u16.to_le_bytes());
    image[56..58].copy_from_slice(&(headers.len() as u16).to_le_bytes());
    image[58..60].copy_from_slice(&64u16.to_le_bytes());
    image[60..62].copy_from_slice(&1u16.to_le_bytes());

    for (index, header) in headers.iter().enumerate() {
        let program = &mut image[phoff + index * 56..][..56];
        program[0..4].copy_from_slice(&header.kind.to_le_bytes());
        program[4..8].copy_from_slice(&header.flags.to_le_bytes());
        program[8..16].copy_from_slice(&header.offset.to_le_bytes());
        program[16..24].copy_from_slice(&header.vaddr.to_le_bytes());
        program[32..40].copy_from_slice(&header.filesz.to_le_bytes());
        program[40..48].copy_from_slice(&header.memsz.to_le_bytes());
        program[48..56].copy_from_slice(&header.align.to_le_bytes());
    }
    image[attr_offset..shoff].copy_from_slice(&attributes);
    let section = &mut image[shoff..shoff + 64];
    section[4..8].copy_from_slice(&0x7000_0003u32.to_le_bytes());
    section[24..32].copy_from_slice(&(attr_offset as u64).to_le_bytes());
    section[32..40].copy_from_slice(&(attributes.len() as u64).to_le_bytes());
    image
}

fn validate_test(image: &[u8]) -> Result<elf::Elf, ElfError> {
    validate(
        image,
        LoadLimits {
            page_size: PAGE_SIZE,
            image_limit: IMAGE_LIMIT,
        },
    )
}

#[test]
fn validates_segments_and_builds_page_runs() {
    let image = build_elf(
        0x1000,
        &[
            Header::load(0, 0x1000, 0x1000, 0x1800, PF_R | PF_X),
            Header::load(0x2000, 0x4000, 0x800, 0x1000, PF_R | PF_W),
        ],
    );
    let elf = validate_test(&image).unwrap();
    assert_eq!(elf.entry(), 0x1000);
    assert_eq!(elf.requirement(), IsaRequirement::Base64);
    assert_eq!(elf.segments().len(), 2);
    assert_eq!(elf.runs().len(), 2);
    assert_eq!((elf.runs()[0].vaddr, elf.runs()[0].memsz), (0x1000, 0x2000));
    assert!(elf.runs()[0].readable && elf.runs()[0].executable && !elf.runs()[0].writable);
    assert_eq!((elf.runs()[1].vaddr, elf.runs()[1].memsz), (0x4000, 0x1000));
    assert!(elf.runs()[1].readable && elf.runs()[1].writable && !elf.runs()[1].executable);
    assert_eq!(elf.image_end(), 0x5000);
}

#[test]
fn accepts_only_supported_auxiliary_headers() {
    let load = Header::load(0, 0x1000, 0x1000, 0x1000, PF_R | PF_X);
    let supported = [
        Header {
            kind: PT_PHDR,
            flags: PF_R,
            offset: 64,
            vaddr: 0x1040,
            filesz: 6 * 56,
            memsz: 6 * 56,
            align: 8,
        },
        load,
        Header {
            kind: PT_NULL,
            flags: u32::MAX,
            offset: u64::MAX,
            vaddr: 3,
            filesz: u64::MAX,
            memsz: 7,
            align: 3,
        },
        Header {
            kind: PT_NOTE,
            flags: PF_R,
            offset: 0,
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            align: 1,
        },
        Header {
            kind: PT_GNU_STACK,
            flags: PF_R | PF_W,
            offset: 0,
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            align: 0,
        },
        Header {
            kind: PT_RISCV_ATTRIBUTES,
            flags: PF_R,
            offset: 0,
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            align: 1,
        },
    ];
    assert!(validate_test(&build_elf(0x1000, &supported)).is_ok());

    for kind in [PT_DYNAMIC, PT_INTERP, PT_TLS, 0x6000_1234] {
        let image = build_elf(
            0x1000,
            &[
                load,
                Header {
                    kind,
                    flags: PF_R,
                    offset: 0,
                    vaddr: 0,
                    filesz: 0,
                    memsz: 0,
                    align: 1,
                },
            ],
        );
        assert_eq!(
            validate_test(&image),
            Err(ElfError::UnsupportedProgramHeader)
        );
    }
}

#[test]
fn rejects_bad_load_geometry_alignment_and_permissions() {
    let cases = [
        (
            Header::load(0, 0x1000, 0x1001, 0x1000, PF_R),
            ElfError::BadSegmentGeometry,
        ),
        (
            Header::load(1, 0x1000, 0x1000, 0x1000, PF_R),
            ElfError::BadSegmentAlignment,
        ),
        (
            Header::load(0, 0x1000, 0x1000, 0x1000, 0),
            ElfError::UnsupportedProtection,
        ),
        (
            Header::load(0, 0x1000, 0x1000, 0x1000, PF_W),
            ElfError::UnsupportedProtection,
        ),
        (
            Header::load(0, 0x1000, 0x1000, 0x1000, PF_R | 0x8),
            ElfError::BadProgramHeaderFlags,
        ),
    ];
    for (header, error) in cases {
        assert_eq!(validate_test(&build_elf(0x1000, &[header])), Err(error));
    }
    let execute_only = validate_test(&build_elf(
        0x1000,
        &[Header::load(0, 0x1000, 0x1000, 0x1000, PF_X)],
    ))
    .unwrap();
    assert!(execute_only.runs()[0].readable && execute_only.runs()[0].executable);
}

#[test]
fn entry_must_be_in_executable_file_bytes_not_bss() {
    let load = Header::load(0, 0x1000, 0x800, 0x1000, PF_R | PF_X);
    assert!(validate_test(&build_elf(0x17ff, &[load])).is_ok());
    assert_eq!(
        validate_test(&build_elf(0x1800, &[load])),
        Err(ElfError::BadEntry)
    );
}

#[test]
fn rejects_overlapping_or_out_of_order_load_bytes() {
    let first = Header::load(0, 0x1000, 0x1000, 0x1800, PF_R | PF_X);
    let overlapping = Header::load(0x1000, 0x2000, 0x1000, 0x1000, PF_R);
    assert_eq!(
        validate_test(&build_elf(0x1000, &[first, overlapping])),
        Err(ElfError::OverlappingSegments)
    );
    let lower = Header::load(0x3000, 0, 0x1000, 0x1000, PF_R);
    assert_eq!(
        validate_test(&build_elf(0x1000, &[first, lower])),
        Err(ElfError::OverlappingSegments)
    );
}

#[test]
fn rejects_page_level_write_execute_union() {
    let image = build_elf(
        0x1000,
        &[
            Header::load(0, 0x1000, 0x800, 0x800, PF_R | PF_X),
            Header::load(0x800, 0x1800, 0x800, 0x800, PF_R | PF_W),
        ],
    );
    assert_eq!(validate_test(&image), Err(ElfError::WriteExecutePage));
}

#[test]
fn rejects_program_and_load_header_capacity_overflow() {
    let loads: Vec<_> = (0..65u64)
        .map(|index| {
            Header::load(
                index * PAGE_SIZE,
                0x1000 + index * PAGE_SIZE,
                PAGE_SIZE,
                PAGE_SIZE,
                if index == 0 { PF_R | PF_X } else { PF_R },
            )
        })
        .collect();
    assert_eq!(
        validate_test(&build_elf(0x1000, &loads)),
        Err(ElfError::TooManyLoadSegments)
    );

    let headers = vec![
        Header {
            kind: PT_NULL,
            flags: 0,
            offset: 0,
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            align: 0,
        };
        129
    ];
    assert_eq!(
        validate_test(&build_elf(0, &headers)),
        Err(ElfError::TooManyProgramHeaders)
    );
}

#[test]
fn rejects_executable_stack_and_bad_images() {
    let load = Header::load(0, 0x1000, 0x1000, 0x1000, PF_R | PF_X);
    let stack = Header {
        kind: PT_GNU_STACK,
        flags: PF_R | PF_W | PF_X,
        offset: 0,
        vaddr: 0,
        filesz: 0,
        memsz: 0,
        align: 0,
    };
    assert_eq!(
        validate_test(&build_elf(0x1000, &[load, stack])),
        Err(ElfError::UnsupportedProtection)
    );

    assert_eq!(validate_test(&[0u8; 16]), Err(ElfError::TooShort));
    let mut image = build_elf(0x1000, &[load]);
    image[0] = 0x7e;
    assert_eq!(validate_test(&image), Err(ElfError::BadMagic));
    let mut image = build_elf(0x1000, &[load]);
    image[4] = 1;
    assert_eq!(validate_test(&image), Err(ElfError::BadClass));
    let mut image = build_elf(0x1000, &[load]);
    image[16..18].copy_from_slice(&3u16.to_le_bytes());
    assert_eq!(validate_test(&image), Err(ElfError::BadType));
    let mut image = build_elf(0x1000, &[load]);
    image[18..20].copy_from_slice(&62u16.to_le_bytes());
    assert_eq!(validate_test(&image), Err(ElfError::BadMachine));
    let mut image = build_elf(0x1000, &[load]);
    image[32..40].copy_from_slice(&0xdead0000u64.to_le_bytes());
    assert_eq!(validate_test(&image), Err(ElfError::BadProgramHeaders));
}
