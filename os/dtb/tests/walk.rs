//! DTB 就地游标测试（host）。fixture 为仓库两平台 dts 经 dtc 编译的
//! 真实 blob；另手工构造 blob 覆盖 NOP、奇数长度属性填充与错误路径。

use dtb::{cells_u64, Fdt, FdtError};

const FDT_MAGIC: u32 = 0xD00D_FEED;
const HEADER: usize = 40;

const VIRT: &[u8] = include_bytes!("fixtures/virt.dtb");
const SIFIVE_U: &[u8] = include_bytes!("fixtures/sifive_u.dtb");

/// 真实 virt blob：结构、cells 继承（chosen 自带 2/1）、多 tuple reg。
#[test]
fn virt_layout() {
    let fdt = Fdt::new(VIRT).unwrap();
    let root = fdt.root();
    assert_eq!(root.name().unwrap(), "");

    let names: Vec<_> = root.children().filter_map(|n| n.name().ok()).collect();
    for expect in ["cpus", "chosen", "memory@80000000"] {
        assert!(names.iter().any(|n| n.starts_with(expect)), "缺 {expect}: {names:?}");
    }

    // /cpus：timebase、#address-cells=1、4 个 cpu 子节点
    let cpus = root.child("cpus").unwrap();
    assert_eq!(cpus.prop_u32("timebase-frequency"), Some(10_000_000));
    let hartids: Vec<_> = cpus
        .children()
        .filter(|n| n.name().is_ok_and(|name| name.split('@').next() == Some("cpu")))
        .map(|n| {
            let reg = n.prop("reg").unwrap();
            cells_u64(reg, 1).unwrap() as usize
        })
        .collect();
    assert_eq!(hartids, [0, 1, 2, 3]);

    // memory@80000000：root cells 2/2
    let mem = root.child("memory@80000000").unwrap();
    assert_eq!(mem.prop_str("device_type"), Some("memory"));
    let reg = mem.prop("reg").unwrap();
    assert_eq!(cells_u64(reg, 2).unwrap(), 0x8000_0000);
    assert_eq!(cells_u64(&reg[8..], 2).unwrap(), 0x800_0000);

    // /chosen/initfs：chosen 覆盖 cells 为 2/1
    let chosen = root.child("chosen").unwrap();
    assert_eq!(chosen.prop_u32("#address-cells"), Some(2));
    assert_eq!(chosen.prop_u32("#size-cells"), Some(1));
    let initfs = chosen.child("initfs").unwrap();
    let reg = initfs.prop("reg").unwrap();
    assert_eq!(cells_u64(reg, 2).unwrap(), 0xB000_0000);
    assert_eq!(cells_u64(&reg[8..], 1).unwrap(), 0x1_0000_000);

    // flash：4 tuple reg（2/2），验证多段与奇偶长度无关的推进
    let flash = root.child("flash@20000000").unwrap();
    assert_eq!(flash.prop("reg").unwrap().len(), 32);
}

/// sifive_u：5 cpu、126MiB 内存。
#[test]
fn sifive_layout() {
    let fdt = Fdt::new(SIFIVE_U).unwrap();
    let root = fdt.root();
    let cpus = root.child("cpus").unwrap();
    let count = cpus
        .children()
        .filter(|n| n.name().is_ok_and(|name| name.split('@').next() == Some("cpu")))
        .count();
    assert_eq!(count, 5);

    let mem = root.child("memory@80000000").unwrap();
    let reg = mem.prop("reg").unwrap();
    assert_eq!(cells_u64(reg, 2).unwrap(), 0x8000_0000);
    assert_eq!(cells_u64(&reg[8..], 2).unwrap(), 0x800_0000); // 128MiB 整段（SBI 占用由帧池剔除）
}

// ---------------------------------------------------------------------------
// 手工构造 blob：token 文法边缘
// ---------------------------------------------------------------------------

struct BlobBuilder {
    struct_block: Vec<u8>,
    strings: Vec<u8>,
}

impl BlobBuilder {
    fn new() -> Self {
        Self {
            struct_block: Vec::new(),
            strings: Vec::new(),
        }
    }

    /// 登记字符串，返回 strings 块内偏移。
    fn string(&mut self, s: &str) -> u32 {
        let off = self.strings.len();
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        off as u32
    }

    fn push_u32(&mut self, v: u32) {
        self.struct_block.extend_from_slice(&v.to_be_bytes());
    }

    fn begin(&mut self, name: &str) {
        self.push_u32(0x1);
        let mut b = name.as_bytes().to_vec();
        b.push(0);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        self.struct_block.extend_from_slice(&b);
    }

    fn end(&mut self) {
        self.push_u32(0x2);
    }

    fn nop(&mut self) {
        self.push_u32(0x4);
    }

    fn prop(&mut self, name: &str, data: &[u8]) {
        let nameoff = self.string(name);
        self.push_u32(0x3);
        self.push_u32(data.len() as u32);
        self.push_u32(nameoff);
        self.struct_block.extend_from_slice(data);
        while self.struct_block.len() % 4 != 0 {
            self.struct_block.push(0xAA); // 填充字节任意，读取方必须跳过
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.push_u32(0x9); // FDT_END

        let strings_off = HEADER + self.struct_block.len().max(8).next_power_of_two();
        // 对齐无关紧要，取整只为可读
        let totalsize = strings_off + self.strings.len();
        let mut out = Vec::with_capacity(totalsize);
        out.extend_from_slice(&FDT_MAGIC.to_be_bytes());
        out.extend_from_slice(&(totalsize as u32).to_be_bytes());
        out.extend_from_slice(&(HEADER as u32).to_be_bytes()); // off_struct
        out.extend_from_slice(&(strings_off as u32).to_be_bytes());
        out.extend_from_slice(&((HEADER + 8) as u32).to_be_bytes()); // off_mem_rsvmap（未用）
        out.extend_from_slice(&17u32.to_be_bytes()); // version
        out.extend_from_slice(&16u32.to_be_bytes()); // last_comp
        out.extend_from_slice(&0u32.to_be_bytes()); // boot_cpuid
        out.extend_from_slice(&(self.strings.len() as u32).to_be_bytes());
        out.extend_from_slice(&(self.struct_block.len() as u32).to_be_bytes());
        debug_assert_eq!(out.len(), HEADER);
        out.resize(HEADER, 0);
        out.extend_from_slice(&self.struct_block);
        out.resize(strings_off, 0);
        out.extend_from_slice(&self.strings);
        out
    }
}



/// NOP 混入、奇数长度属性、深层嵌套、名字查找。
#[test]
fn crafted_tokens() {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.nop();
    b.prop("ok-str", b"ab"); // 奇数长度 → 1 字节填充
    b.prop("u32", &0x1234_5678u32.to_be_bytes());
    b.begin("outer");
    b.prop("outer-prop", b"x");
    b.begin("mid");
    b.begin("inner");
    b.prop("deep", &[9u8; 3]);
    b.end();
    b.end();
    b.end();
    b.nop();
    b.begin("sibling");
    b.end();
    b.end();
    let blob = b.finish();

    let fdt = Fdt::new(&blob).unwrap();
    let root = fdt.root();
    assert_eq!(root.prop("ok-str"), Some(b"ab".as_slice()));
    assert_eq!(root.prop_u32("u32"), Some(0x1234_5678));
    assert_eq!(root.prop("absent"), None);

    let inner = root
        .child("outer")
        .and_then(|o| o.child("mid"))
        .and_then(|m| m.child("inner"))
        .unwrap();
    assert_eq!(inner.prop("deep"), Some([9u8; 3].as_slice()));

    // NOP 不影响子节点迭代
    let names: Vec<_> = root.children().filter_map(|n| n.name().ok()).collect();
    assert_eq!(names, ["outer", "sibling"]);
}

/// 同名属性取首个。
#[test]
fn duplicate_prop_takes_first() {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.prop("dup", b"first");
    b.prop("dup", b"second");
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    assert_eq!(fdt.root().prop("dup"), Some(b"first".as_slice()));
}

/// 错误路径：坏魔数、截断、根非 BEGIN、子树未闭合。
#[test]
fn malformed_blobs() {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.end();
    let good = b.finish();

    let mut bad_magic = good.clone();
    bad_magic[0] ^= 0xFF;
    assert_eq!(Fdt::new(&bad_magic).unwrap_err(), FdtError::BadMagic);

    assert_eq!(Fdt::new(&good[..good.len() - 1]).unwrap_err(), FdtError::Truncated);

    // 根之前插入 PROP → UnexpectedToken
    let mut b2 = BlobBuilder::new();
    b2.prop("x", b"y");
    b2.begin("");
    b2.end();
    let bad_root = b2.finish();
    assert_eq!(Fdt::new(&bad_root).unwrap_err(), FdtError::UnexpectedToken);

    // 未闭合子树（缺最后 END_NODE）
    let mut b3 = BlobBuilder::new();
    b3.begin("");
    b3.begin("child");
    let unclosed = b3.finish();
    assert_eq!(Fdt::new(&unclosed).unwrap_err(), FdtError::UnexpectedToken);
}
