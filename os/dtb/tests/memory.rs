//! 平台物理内存描述与区间规范化测试。

mod common;

use common::BlobBuilder;
use dtb::{
    Fdt,
    memory::{MemoryMapError, PhysicalRange, parse},
};

const PAGE_SIZE: u64 = 4096;

fn cells(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

fn tuple(address: u64, size: u64) -> Vec<u8> {
    cells(&[
        (address >> 32) as u32,
        address as u32,
        (size >> 32) as u32,
        size as u32,
    ])
}

fn base_tree() -> BlobBuilder {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);
    b
}

#[test]
fn normalizes_all_memory_and_reservation_ranges() {
    let mut b = base_tree();
    b.reservation(0x8000_1800, 0x400);

    b.begin("memory@80000001");
    b.prop_str("device_type", "memory");
    let mut reg = tuple(0x8000_0001, 0x5000);
    reg.extend_from_slice(&tuple(0x9000_0000, 0x4000));
    b.prop("reg", &reg);
    b.end();

    b.begin("memory@80005000");
    b.prop_str("device_type", "memory");
    b.prop("reg", &tuple(0x8000_5000, 0x3000));
    b.end();

    b.begin("reserved-memory");
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);
    b.prop("ranges", &[]);
    b.begin("firmware@80006001");
    b.prop("reg", &tuple(0x8000_6001, 0x1800));
    b.end();
    b.begin("secure@90001001");
    b.prop("reg", &tuple(0x9000_1001, 0x100));
    b.prop("no-map", &[]);
    b.end();
    b.end();

    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    let map = parse::<8, 8>(&fdt, PAGE_SIZE).unwrap();

    assert_eq!(
        map.memories(),
        [
            PhysicalRange {
                start: 0x8000_1000,
                end: 0x8000_8000,
            },
            PhysicalRange {
                start: 0x9000_0000,
                end: 0x9000_4000,
            },
        ]
    );
    assert_eq!(
        map.reservations(),
        [
            PhysicalRange {
                start: 0x8000_1000,
                end: 0x8000_2000,
            },
            PhysicalRange {
                start: 0x8000_6000,
                end: 0x8000_8000,
            },
            PhysicalRange {
                start: 0x9000_1000,
                end: 0x9000_2000,
            },
        ]
    );
    assert_eq!(
        map.no_map(),
        [PhysicalRange {
            start: 0x9000_1000,
            end: 0x9000_2000,
        }]
    );
}

#[test]
fn disabled_memory_and_reserved_children_are_ignored() {
    let mut b = base_tree();
    b.begin("memory@80000000");
    b.prop_str("device_type", "memory");
    b.prop("reg", &tuple(0x8000_0000, 0x4000));
    b.end();
    b.begin("memory@90000000");
    b.prop_str("device_type", "memory");
    b.prop_str("status", "disabled");
    b.prop("reg", &tuple(0x9000_0000, 0x4000));
    b.end();
    b.begin("reserved-memory");
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);
    b.prop("ranges", &[]);
    b.begin("unused@80001000");
    b.prop_str("status", "disabled");
    b.prop("reg", &tuple(0x8000_1000, 0x1000));
    b.end();
    b.end();
    b.end();

    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    let map = parse::<4, 4>(&fdt, PAGE_SIZE).unwrap();
    assert_eq!(
        map.memories(),
        [PhysicalRange {
            start: 0x8000_0000,
            end: 0x8000_4000,
        }]
    );
    assert!(map.reservations().is_empty());
}

fn reserved_error(property: &str, data: &[u8]) -> MemoryMapError {
    let mut b = base_tree();
    b.begin("memory@80000000");
    b.prop_str("device_type", "memory");
    b.prop("reg", &tuple(0x8000_0000, 0x8000));
    b.end();
    b.begin("reserved-memory");
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);
    b.prop("ranges", &[]);
    b.begin("candidate");
    b.prop(property, data);
    b.end();
    b.end();
    b.end();

    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    parse::<4, 4>(&fdt, PAGE_SIZE).err().unwrap()
}

#[test]
fn rejects_reserved_memory_requiring_unimplemented_mechanisms() {
    assert_eq!(
        reserved_error("size", &cells(&[0, 0x1000])),
        MemoryMapError::UnsupportedDynamicReservation
    );
    assert_eq!(
        reserved_error("reusable", &[]),
        MemoryMapError::UnsupportedReusable
    );
}

#[test]
fn rejects_overlapping_memory_and_capacity_exhaustion() {
    let mut b = base_tree();
    for (address, size) in [(0x8000_0000, 0x4000), (0x8000_2000, 0x4000)] {
        b.begin("memory");
        b.prop_str("device_type", "memory");
        b.prop("reg", &tuple(address, size));
        b.end();
    }
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(
        parse::<4, 4>(&fdt, PAGE_SIZE),
        Err(MemoryMapError::MemoryOverlap)
    ));

    let mut b = base_tree();
    b.begin("memory");
    b.prop_str("device_type", "memory");
    let mut reg = tuple(0x8000_0000, 0x1000);
    reg.extend_from_slice(&tuple(0x9000_0000, 0x1000));
    b.prop("reg", &reg);
    b.end();
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(
        parse::<1, 4>(&fdt, PAGE_SIZE),
        Err(MemoryMapError::CapacityExceeded)
    ));
}

#[test]
fn rejects_malformed_reserved_memory_parent() {
    let mut b = base_tree();
    b.begin("memory");
    b.prop_str("device_type", "memory");
    b.prop("reg", &tuple(0x8000_0000, 0x4000));
    b.end();
    b.begin("reserved-memory");
    b.prop_u32("#address-cells", 2);
    b.prop_u32("#size-cells", 2);
    b.begin("firmware");
    b.prop("reg", &tuple(0x8000_1000, 0x1000));
    b.end();
    b.end();
    b.end();
    let blob = b.finish();
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(
        parse::<4, 4>(&fdt, PAGE_SIZE),
        Err(MemoryMapError::MalformedReservedMemory)
    ));
}
