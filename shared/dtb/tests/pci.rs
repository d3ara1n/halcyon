//! PCI host 几何的规范反例；沿用 DT blob 构造器，不模拟设备或 authority。
mod common;
use common::BlobBuilder;
use dtb::{
    Fdt,
    pci::{Error, Host, Space},
};
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|word| word.to_be_bytes()).collect()
}
fn blob(ranges: &[u32], buses: [u32; 2], ecam_bytes: u32, child_cells: u32) -> Vec<u8> {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.begin("pci");
    b.prop_str("device_type", "pci");
    b.prop_u32("#address-cells", child_cells);
    b.prop_u32("#size-cells", 2);
    b.prop("reg", &words(&[0, 0x3000_0000, 0, ecam_bytes]));
    b.prop("bus-range", &words(&buses));
    b.prop("dma-coherent", &[]);
    b.prop("ranges", &words(ranges));
    b.end();
    b.end();
    b.finish()
}
const MEMORY: [u32; 7] = [0x0200_0000, 0, 0x2000_0000, 0, 0x4000_0000, 0, 0x1000_0000];
fn parse_error(bytes: &[u8]) -> Option<Error> {
    let fdt = Fdt::new(bytes).unwrap();
    Host::parse(&fdt.root().child("pci").unwrap(), 2, 2).err()
}
#[test]
fn bus_and_cpu_addresses_remain_distinct() {
    let bytes = blob(&MEMORY, [0, 255], 0x1000_0000, 3);
    let fdt = Fdt::new(&bytes).unwrap();
    let host = Host::parse(&fdt.root().child("pci").unwrap(), 2, 2).unwrap();
    let window = host.windows().next().unwrap().unwrap();
    assert_eq!(window.bus.start, 0x2000_0000);
    assert_eq!(window.cpu.start, 0x4000_0000);
    assert_eq!(window.space, Space::Memory32);
    assert!(host.coherent);
}
#[test]
fn pio_is_not_memory_and_prefetchable_memory64_preserves_high_cells() {
    let ranges = [
        0x0100_0000,
        0,
        0,
        0,
        0x0300_0000,
        0,
        0x10000,
        0x4300_0000,
        4,
        0,
        8,
        0,
        4,
        0,
    ];
    let bytes = blob(&ranges, [0, 255], 0x1000_0000, 3);
    let fdt = Fdt::new(&bytes).unwrap();
    let host = Host::parse(&fdt.root().child("pci").unwrap(), 2, 2).unwrap();
    let mut windows = host.windows();
    assert_eq!(windows.next().unwrap().unwrap().space, Space::Io);
    let high = windows.next().unwrap().unwrap();
    assert_eq!(high.bus.start, 0x4_0000_0000);
    assert_eq!(high.cpu.start, 0x8_0000_0000);
    assert_eq!(high.cpu.end, 0xc_0000_0000);
    assert!(high.prefetchable);
}
#[test]
fn malformed_cells_truncation_and_bus_coverage_are_rejected() {
    assert_eq!(
        parse_error(&blob(&MEMORY[..6], [0, 255], 0x1000_0000, 3)),
        Some(Error::Malformed)
    );
    assert_eq!(
        parse_error(&blob(&MEMORY, [0, 255], 0x0ff0_0000, 3)),
        Some(Error::Malformed)
    );
    assert_eq!(
        parse_error(&blob(&MEMORY, [0, 256], 0x1000_0000, 3)),
        Some(Error::Malformed)
    );
    assert_eq!(
        parse_error(&blob(&MEMORY, [0, 255], 0x1000_0000, 2)),
        Some(Error::Malformed)
    );
}
#[test]
fn address_overflow_and_unknown_address_spaces_are_rejected() {
    let mut range = MEMORY;
    range[0] = 0x0300_0000;
    range[3] = u32::MAX;
    range[4] = 0xffff_f000;
    assert_eq!(
        parse_error(&blob(&range, [0, 255], 0x1000_0000, 3)),
        Some(Error::Overflow)
    );
    range = MEMORY;
    range[2] = 0xffff_f000;
    assert_eq!(
        parse_error(&blob(&range, [0, 255], 0x1000_0000, 3)),
        Some(Error::Overflow)
    );
    range = MEMORY;
    range[0] = 0;
    assert_eq!(
        parse_error(&blob(&range, [0, 255], 0x1000_0000, 3)),
        Some(Error::Unsupported)
    );
}
#[test]
fn actual_virt_fixture_has_ecam_and_two_cpu_memory_windows() {
    let fdt = Fdt::new(include_bytes!("fixtures/virt.dtb")).unwrap();
    let node = fdt
        .root()
        .child("soc")
        .unwrap()
        .child("pci@30000000")
        .unwrap();
    let host = Host::parse(&node, 2, 2).unwrap();
    assert_eq!(host.ecam.start, 0x3000_0000);
    let windows = host.windows().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(windows.len(), 3);
    assert_eq!(windows[0].space, Space::Io);
    assert_eq!(windows[1].cpu.start, 0x4000_0000);
    assert_eq!(windows[2].cpu.start, 0x4_0000_0000);
}
