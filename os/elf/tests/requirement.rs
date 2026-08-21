//! ISA 需求判定测试（host）：e_flags 契约面与 .riscv.attributes 解析。

use elf::{isa_requirement, IsaReqError, IsaRequirement};

/// 构造最小 ELF64 镜像：可选携带一个 SHT_RISCV_ATTRIBUTES 节。
fn build_elf(e_flags: u32, attributes: Option<&[u8]>) -> Vec<u8> {
    let shoff;
    let mut v = vec![0u8; 64];
    v[..4].copy_from_slice(b"\x7fELF");
    v[4] = 2; // ELF64
    v[5] = 1; // little-endian
    v[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    v[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    v[48..52].copy_from_slice(&e_flags.to_le_bytes());

    if let Some(attr) = attributes {
        let content_len = attr.len();
        // section header 表紧随 64 字节头 + 节数据
        shoff = 64 + content_len;
        v.resize(shoff + 64, 0);
        v[64..shoff].copy_from_slice(attr);
        let sh = &mut v[shoff..shoff + 64];
        sh[4..8].copy_from_slice(&0x7000_0003u32.to_le_bytes()); // SHT_RISCV_ATTRIBUTES
        sh[24..32].copy_from_slice(&64u64.to_le_bytes()); // sh_offset
        sh[32..40].copy_from_slice(&(content_len as u64).to_le_bytes());
        v[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
        v[58..60].copy_from_slice(&64u16.to_le_bytes()); // shentsize
        v[60..62].copy_from_slice(&1u16.to_le_bytes()); // shnum
    } else {
        shoff = 0;
        v[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
        v[58..60].copy_from_slice(&64u16.to_le_bytes()); // shentsize
        v[60..62].copy_from_slice(&0u16.to_le_bytes()); // 无节 → MissingArch
    }
    v
}

/// uleb128 编码。
fn uleb(v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut x = v;
    loop {
        let b = (x & 0x7F) as u8;
        x >>= 7;
        if x == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
    out
}

/// 完整 attributes 节数据：'A' + u32 总长 + 子节（vendor NTBS、
/// Tag_File uleb、u32 LE 内容长、属性序列）——与 rust-lld 实际输出同构。
fn attributes(arch: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&uleb(5)); // Tag_RISCV_arch
    body.extend_from_slice(arch.as_bytes());
    body.push(0);

    let mut sub = Vec::new();
    sub.extend_from_slice(b"riscv\0");
    // 子节 size 自 tag 字段起计：uleb(tag) + u32(size 字段) + 内容。
    let content_size = 1 + 4 + body.len();
    sub.extend_from_slice(&uleb(1)); // Tag_File
    sub.extend_from_slice(&(content_size as u32).to_le_bytes());
    sub.extend_from_slice(&body);

    let mut data = vec![b'A'];
    data.extend_from_slice(&(sub.len() as u32).to_le_bytes());
    data.extend_from_slice(&sub);
    data
}

const RV64I: &str = "rv64i2p1_m2p0_a2p1_c2p0_zicsr2p0_zifencei2p0_zicntr2p0";
const RV64IMAFDC: &str = "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0_zicsr2p0_zifencei2p0_zicntr2p0";
const LP64_SOFT: u32 = 0x0000 | 0x0001; // RVC
const LP64D: u32 = 0x0004 | 0x0001;

#[test]
fn base64_profile() {
    let elf = build_elf(LP64_SOFT, Some(&attributes(RV64I)));
    assert_eq!(isa_requirement(&elf), Ok(IsaRequirement::Base64));
}

#[test]
fn d64_profile() {
    let elf = build_elf(LP64D, Some(&attributes(RV64IMAFDC)));
    assert_eq!(isa_requirement(&elf), Ok(IsaRequirement::D64));
}

#[test]
fn packed_base_letters_accepted() {
    // 打包形式 rv64imac + 版本挂在 base token 尾部
    let elf = build_elf(
        LP64_SOFT,
        Some(&attributes("rv64imac2p1_zicsr2p0_zifencei2p0")),
    );
    assert_eq!(isa_requirement(&elf), Ok(IsaRequirement::Base64));
}

#[test]
fn missing_attributes_rejected() {
    let elf = build_elf(LP64_SOFT, None);
    assert_eq!(isa_requirement(&elf), Err(IsaReqError::MissingArch));
}

#[test]
fn tso_rve_reserved_rejected() {
    let arch = attributes(RV64I);
    for (flags, err) in [
        (LP64_SOFT | 0x0010, IsaReqError::Tso),
        (LP64_SOFT | 0x0008, IsaReqError::Rve),
        (LP64_SOFT | 0x0020, IsaReqError::BadFlags),
    ] {
        let elf = build_elf(flags, Some(&arch));
        assert_eq!(isa_requirement(&elf), Err(err));
    }
}

#[test]
fn float_abi_gates() {
    let arch_soft = attributes(RV64I);
    let arch_fd = attributes(RV64IMAFDC);
    // Quad ABI 拒绝
    let quad = build_elf(0x0006, Some(&attributes("rv64i2p1_q2p0_zicsr2p0")));
    assert_eq!(isa_requirement(&quad), Err(IsaReqError::QuadAbi));
    // Single ABI（F-only）拒绝
    let single = build_elf(0x0002, Some(&attributes("rv64i2p1_f2p2_c2p0_zicsr2p0")));
    assert_eq!(isa_requirement(&single), Err(IsaReqError::FOnly));
    // 双精度 ABI 却缺 f/d 声明：ABI 与 arch 不一致
    let mismatch = build_elf(LP64D & !0x0001, Some(&arch_soft));
    assert_eq!(isa_requirement(&mismatch), Err(IsaReqError::AbiArchMismatch));
    // soft ABI 却声明 f/d：拒绝而非降级
    let leaky = build_elf(LP64_SOFT, Some(&arch_fd));
    assert_eq!(isa_requirement(&leaky), Err(IsaReqError::AbiArchMismatch));
}

#[test]
fn unmodeled_extensions_rejected() {
    for arch in [
        "rv64i2p1_m2p0_a2p1_c2p0_v1p0_zicsr2p0",                 // V
        "rv64i2p1_m2p0_a2p1_c2p0_b1p0_zicsr2p0",                 // B
        "rv64i2p1_m2p0_a2p1_c2p0_zfh1p0_zicsr2p0",               // Zfh
        "rv64i2p1_m2p0_a2p1_c2p0_xandespmu5p0_zicsr2p0",         // vendor
        "rv64i2p1_m2p0_a2p1_c2p0_zba5p0_zbb5p0_zbs5p0_zicsr2p0", // 位操作未建模
    ] {
        let elf = build_elf(LP64_SOFT, Some(&attributes(arch)));
        assert_eq!(
            isa_requirement(&elf),
            Err(IsaReqError::UnsupportedExtension),
            "{arch}"
        );
    }
}

#[test]
fn non_rv64_base_rejected() {
    let elf = build_elf(LP64_SOFT, Some(&attributes("rv32i2p1_m2p0_a2p1_c2p0_zicsr2p0")));
    assert_eq!(isa_requirement(&elf), Err(IsaReqError::BadBase));
}

#[test]
fn bad_base_and_shorthand_rejected() {
    // g 缩写未展开：非规范形式
    let g = build_elf(LP64_SOFT, Some(&attributes("rv64i2p1_g2p0")));
    assert_eq!(
        isa_requirement(&g),
        Err(IsaReqError::UnsupportedExtension)
    );
}

#[test]
fn compatibility_gate() {
    use IsaRequirement::*;
    assert!(Base64.compatible(0));
    assert!(Base64.compatible(64));
    assert!(!D64.compatible(0));
    assert!(!D64.compatible(32));
    // FLEN=128（Q-capable）不进入 D64：有效 FLEN 必须恰为 64
    assert!(!D64.compatible(128));
    assert!(D64.compatible(64));
}
