//! RISC-V CPU canonical admission 测试。

mod common;

use common::BlobBuilder;
use dtb::{
    Fdt,
    cpu::{CpuError, MmuType, parse},
};

fn begin_tree(timebase: u32) -> BlobBuilder {
    let mut b = BlobBuilder::new();
    b.begin("");
    b.begin("cpus");
    b.prop_u32("#address-cells", 1);
    b.prop_u32("#size-cells", 0);
    b.prop_u32("timebase-frequency", timebase);
    b
}

fn cpu(b: &mut BlobBuilder, hartid: u32, extensions: &[&str], status: Option<&str>) {
    b.begin(&format!("cpu@{hartid}"));
    b.prop_str("device_type", "cpu");
    b.prop_u32("reg", hartid);
    b.prop_str("riscv,isa-base", "rv64i");
    b.prop_str_list("riscv,isa-extensions", extensions);
    b.prop_str("mmu-type", "riscv,sv39");
    if let Some(status) = status {
        b.prop_str("status", status);
    }
    b.end();
}

fn finish(mut b: BlobBuilder) -> Vec<u8> {
    b.end();
    b.end();
    b.finish()
}

const BASE: &[&str] = &["i", "m", "a", "c", "zicsr", "zifencei", "zicntr"];

#[test]
fn sorts_hartids_and_freezes_capabilities() {
    let mut b = begin_tree(10_000_000);
    let mut fp = BASE.to_vec();
    fp.extend_from_slice(&["f", "d", "q", "v"]);
    cpu(&mut b, 7, &fp, None);
    cpu(&mut b, 2, BASE, Some("okay"));
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    let admission = parse::<4>(&fdt).unwrap();

    assert_eq!(admission.timebase_frequency(), 10_000_000);
    assert_eq!(admission.cpus()[0].hartid, 2);
    assert_eq!(admission.cpus()[1].hartid, 7);
    assert_eq!(admission.cpus()[0].mmu, MmuType::Sv39);
    assert!(admission.cpus()[1].capabilities.q);
    assert!(admission.cpus()[1].capabilities.d);
    assert!(admission.cpus()[1].capabilities.f);
    assert!(admission.cpus()[1].capabilities.v);
}

#[test]
fn skips_every_specified_unavailable_status() {
    let mut b = begin_tree(1_000_000);
    cpu(&mut b, 0, BASE, None);
    for (hartid, status) in [
        (1, "disabled"),
        (2, "reserved"),
        (3, "fail"),
        (4, "fail-selftest"),
    ] {
        cpu(&mut b, hartid, BASE, Some(status));
    }
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    let admission = parse::<8>(&fdt).unwrap();
    assert_eq!(admission.cpus().len(), 1);
    assert_eq!(admission.cpus()[0].hartid, 0);
}

#[test]
fn rejects_unknown_or_malformed_status() {
    let mut b = begin_tree(1_000_000);
    cpu(&mut b, 0, BASE, Some("ok"));
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(parse::<2>(&fdt), Err(CpuError::UnknownStatus)));

    let mut b = begin_tree(1_000_000);
    b.begin("cpu@0");
    b.prop_str("device_type", "cpu");
    b.prop_u32("reg", 0);
    b.prop("status", b"okay");
    b.end();
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(parse::<2>(&fdt), Err(CpuError::MalformedStatus)));
}

#[test]
fn rejects_duplicate_hartids_after_sorting() {
    let mut b = begin_tree(1_000_000);
    cpu(&mut b, 4, BASE, None);
    cpu(&mut b, 1, BASE, None);
    cpu(&mut b, 4, BASE, None);
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(parse::<4>(&fdt), Err(CpuError::DuplicateHartId)));
}

#[test]
fn rejects_invalid_floating_point_dependency() {
    for extras in [&["d"][..], &["f", "q"][..]] {
        let mut extensions = BASE.to_vec();
        extensions.extend_from_slice(extras);
        let mut b = begin_tree(1_000_000);
        cpu(&mut b, 0, &extensions, None);
        let blob = finish(b);
        let fdt = Fdt::new(&blob).unwrap();
        assert!(matches!(
            parse::<2>(&fdt),
            Err(CpuError::InvalidCapabilityDependency)
        ));
    }
}

#[test]
fn rejects_duplicate_or_missing_baseline_extensions() {
    let mut duplicate = BASE.to_vec();
    duplicate.push("m");
    let mut b = begin_tree(1_000_000);
    cpu(&mut b, 0, &duplicate, None);
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(
        parse::<2>(&fdt),
        Err(CpuError::MalformedIsaExtensions)
    ));

    let mut b = begin_tree(1_000_000);
    cpu(&mut b, 0, &["i", "m", "a", "c", "zicsr", "zifencei"], None);
    let blob = finish(b);
    let fdt = Fdt::new(&blob).unwrap();
    assert!(matches!(
        parse::<2>(&fdt),
        Err(CpuError::MissingBaselineExtension)
    ));
}
