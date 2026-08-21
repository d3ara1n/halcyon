//! 现代 ISA 属性（string-list）与 cpu-map 拓扑解析的契约测试。

mod common;

use common::BlobBuilder;
use dtb::{topology, Fdt};

/// 构造一个含 2 个 cpu 节点 + socket/cluster/core 三层 cpu-map 的 blob。
fn topo_blob() -> Vec<u8> {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.begin("cpus");
    b.prop_u32("#address-cells", 1);
    b.prop_u32("#size-cells", 0);

    b.begin("cpu@0");
    b.prop_str_list("compatible", &["riscv"]);
    b.prop_u32("reg", 0);
    b.prop_u32("phandle", 0x10);
    b.prop_str("device_type", "cpu");
    b.prop_str("riscv,isa-base", "rv64i");
    b.prop_str_list("riscv,isa-extensions", &["i", "m", "a", "c", "zicsr"]);
    b.end();

    b.begin("cpu@1");
    b.prop_str_list("compatible", &["riscv"]);
    b.prop_u32("reg", 1);
    b.prop_u32("phandle", 0x11);
    b.prop_str("device_type", "cpu");
    b.prop_str("riscv,isa-base", "rv64i");
    b.prop_str_list("riscv,isa-extensions", &["i", "m", "a", "f", "d", "c", "zicsr"]);
    b.end();

    b.begin("cpu-map");
    b.begin("socket0");
    b.begin("cluster0");
    b.begin("core0");
    b.prop_u32("cpu", 0x10);
    b.end();
    b.begin("core1");
    b.begin("thread0");
    b.prop_u32("cpu", 0x11);
    b.end();
    b.end();
    b.end();
    b.end();
    b.end(); // cpu-map
    b.end(); // cpus
    b.end(); // root
    b.finish()
}

#[test]
fn string_list_parses() {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.prop_str_list("exts", &["i", "m", "a"]);
    b.prop_str("single", "abc");
    b.prop("empty", &[0]);
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    let root = fdt.root();
    assert_eq!(
        root.prop_str_list("exts").unwrap().collect::<Vec<_>>(),
        ["i", "m", "a"]
    );
    // 单字符串属性同样可按列表读
    assert_eq!(
        root.prop_str_list("single").unwrap().collect::<Vec<_>>(),
        ["abc"]
    );
    assert!(root.prop_str_list("absent").is_none());
}

#[test]
fn string_list_rejects_bad_utf8_and_missing_nul() {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.prop("bad", &[b'a', 0xFF, 0]);
    b.prop("no_term", &[b'x', b'y']); // 末段缺 NUL：非法
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    let root = fdt.root();
    assert!(root.prop_str_list("bad").is_none());
    assert!(root.prop_str_list("no_term").is_none());
}

#[test]
fn cpu_map_three_levels() {
    let blob = topo_blob();
    let fdt = Fdt::new(&blob).unwrap();
    let cpus = fdt.root().child("cpus").unwrap();
    let map = cpus.child("cpu-map").unwrap();

    let leaves = topology::parse(&map).unwrap();
    assert_eq!(leaves.len(), 2);
    let by_cpu = |ph: u32| leaves.iter().find(|l| l.cpu == ph).unwrap();
    assert_eq!(
        by_cpu(0x10).path,
        vec![
            topology::TopoLevel::Socket(0),
            topology::TopoLevel::Cluster(0),
            topology::TopoLevel::Core(0),
        ]
    );
    assert_eq!(
        by_cpu(0x11).path,
        vec![
            topology::TopoLevel::Socket(0),
            topology::TopoLevel::Cluster(0),
            topology::TopoLevel::Core(1),
            topology::TopoLevel::Thread(0),
        ]
    );

    let hartids = topology::cpu_phandle_hartids(&cpus, 1);
    assert_eq!(hartids, vec![(0x10, 0), (0x11, 1)]);
}

#[test]
fn cpu_map_rejects_duplicate_leaf() {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.begin("cpus");
    b.begin("cpu-map");
    b.begin("socket0");
    b.begin("core0");
    b.prop_u32("cpu", 0x10);
    b.end();
    b.begin("core1");
    b.prop_u32("cpu", 0x10); // 同一 phandle 两个叶子：非法
    b.end();
    b.end();
    b.end();
    b.end();
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    let map = fdt.root().child("cpus").unwrap().child("cpu-map").unwrap();
    assert_eq!(topology::parse(&map), Err(topology::TopoError::DuplicateCpu));
}

#[test]
fn level_names_parse() {
    use topology::TopoLevel;
    assert_eq!(TopoLevel::from_name("socket12"), Some(TopoLevel::Socket(12)));
    assert_eq!(TopoLevel::from_name("thread0"), Some(TopoLevel::Thread(0)));
    assert_eq!(TopoLevel::from_name("cores0"), None);
    assert_eq!(TopoLevel::from_name("socket"), None);
    assert_eq!(TopoLevel::from_name("socketx"), None);
}

/// 真实 virt 平台 blob：cluster 直挂 core（无 socket 层），phandle 可解析。
#[test]
fn virt_topology() {
    const VIRT: &[u8] = include_bytes!("fixtures/virt.dtb");
    let fdt = Fdt::new(VIRT).unwrap();
    let cpus = fdt.root().child("cpus").unwrap();
    let map = cpus.child("cpu-map").unwrap();
    let leaves = topology::parse(&map).unwrap();
    assert_eq!(leaves.len(), 4);
    let hartids = topology::cpu_phandle_hartids(&cpus, 1);
    let resolved: Vec<u64> = leaves
        .iter()
        .map(|l| hartids.iter().find(|(ph, _)| *ph == l.cpu).unwrap().1)
        .collect();
    assert_eq!(resolved, [0, 1, 2, 3]);
}
