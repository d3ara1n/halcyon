//! 固定 DT 关系的反例，不模拟 PLIC 硬件。
use dtb::{Fdt, plic::Plic};
fn fixture() -> Vec<u8> {
    include_bytes!("fixtures/virt.dtb").to_vec()
}
fn replace_property(bytes: &mut [u8], node: &str, property: &str, replacement: &[u8]) {
    let tree = Fdt::new(bytes).unwrap();
    let node = tree.root().child("soc").unwrap().child(node).unwrap();
    let value = node.prop(property).unwrap();
    assert_eq!(value.len(), replacement.len());
    let offset = value.as_ptr() as usize - bytes.as_ptr() as usize;
    bytes[offset..offset + replacement.len()].copy_from_slice(replacement);
}
#[test]
fn actual_virt_contexts_and_pci_routing() {
    let bytes = fixture();
    let tree = Fdt::new(&bytes).unwrap();
    let soc = tree.root().child("soc").unwrap();
    let node = soc.child("plic@c000000").unwrap();
    let plic = Plic::parse(&tree, &node).unwrap();
    assert_eq!(plic.sources, 95);
    let contexts = plic.contexts().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(contexts.len(), 8);
    for (hart, pair) in contexts.as_chunks::<2>().0.iter().enumerate() {
        assert_eq!((pair[0].hartid, pair[0].interrupt), (hart as u64, 11));
        assert_eq!((pair[1].hartid, pair[1].interrupt), (hart as u64, 9));
        assert_eq!(pair[1].index, (hart * 2 + 1) as u32);
    }
    assert_eq!(
        plic.virt_pci_sources(&soc.child("pci@30000000").unwrap()),
        Ok((32, 4))
    );
}
#[test]
fn context_uses_phandle_instead_of_assuming_hart_order() {
    let mut bytes = fixture();
    let mut extended = {
        let tree = Fdt::new(&bytes).unwrap();
        tree.root()
            .child("soc")
            .unwrap()
            .child("plic@c000000")
            .unwrap()
            .prop("interrupts-extended")
            .unwrap()
            .to_vec()
    };
    let first = extended[..16].to_vec();
    let second = extended[16..32].to_vec();
    extended[..16].copy_from_slice(&second);
    extended[16..32].copy_from_slice(&first);
    replace_property(&mut bytes, "plic@c000000", "interrupts-extended", &extended);
    let tree = Fdt::new(&bytes).unwrap();
    let node = tree
        .root()
        .child("soc")
        .unwrap()
        .child("plic@c000000")
        .unwrap();
    let plic = Plic::parse(&tree, &node).unwrap();
    let contexts = plic.contexts().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!((contexts[1].index, contexts[1].hartid), (1, 1));
    assert_eq!((contexts[3].index, contexts[3].hartid), (3, 0));
}
#[test]
fn wrong_irq_parent_and_duplicate_route_are_rejected() {
    for duplicate in [false, true] {
        let mut bytes = fixture();
        let mut map = {
            let tree = Fdt::new(&bytes).unwrap();
            tree.root()
                .child("soc")
                .unwrap()
                .child("pci@30000000")
                .unwrap()
                .prop("interrupt-map")
                .unwrap()
                .to_vec()
        };
        if duplicate {
            let first = map[..24].to_vec();
            map[24..48].copy_from_slice(&first);
        } else {
            map[16..20].copy_from_slice(&1234u32.to_be_bytes());
        }
        replace_property(&mut bytes, "pci@30000000", "interrupt-map", &map);
        let tree = Fdt::new(&bytes).unwrap();
        let soc = tree.root().child("soc").unwrap();
        let node = soc.child("plic@c000000").unwrap();
        let plic = Plic::parse(&tree, &node).unwrap();
        assert!(
            plic.virt_pci_sources(&soc.child("pci@30000000").unwrap())
                .is_err()
        );
    }
}
#[test]
fn unknown_context_phandle_is_rejected() {
    let mut bytes = fixture();
    let mut extended = {
        let tree = Fdt::new(&bytes).unwrap();
        tree.root()
            .child("soc")
            .unwrap()
            .child("plic@c000000")
            .unwrap()
            .prop("interrupts-extended")
            .unwrap()
            .to_vec()
    };
    extended[..4].copy_from_slice(&1234u32.to_be_bytes());
    replace_property(&mut bytes, "plic@c000000", "interrupts-extended", &extended);
    let tree = Fdt::new(&bytes).unwrap();
    let node = tree
        .root()
        .child("soc")
        .unwrap()
        .child("plic@c000000")
        .unwrap();
    assert!(Plic::parse(&tree, &node).is_err());
}

#[test]
fn firmware_disabled_contexts_preserve_hardware_indices() {
    let mut bytes = fixture();
    let mut extended = {
        let tree = Fdt::new(&bytes).unwrap();
        tree.root()
            .child("soc")
            .unwrap()
            .child("plic@c000000")
            .unwrap()
            .prop("interrupts-extended")
            .unwrap()
            .to_vec()
    };
    for pair in extended.as_chunks_mut::<16>().0 {
        pair[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
    }
    replace_property(&mut bytes, "plic@c000000", "interrupts-extended", &extended);
    let tree = Fdt::new(&bytes).unwrap();
    let node = tree
        .root()
        .child("soc")
        .unwrap()
        .child("plic@c000000")
        .unwrap();
    let plic = Plic::parse(&tree, &node).unwrap();
    let supervisor: Vec<_> = plic
        .contexts()
        .map(Result::unwrap)
        .filter(|item| item.interrupt == 9)
        .collect();
    assert_eq!(
        supervisor.iter().map(|item| item.index).collect::<Vec<_>>(),
        [1, 3, 5, 7]
    );
}
