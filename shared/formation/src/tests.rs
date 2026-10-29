use super::*;
use alloc::{collections::BTreeMap, string::ToString, vec};

const SENDER: Rights = Rights::from_raw(
    Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

fn empty_settings() -> Vec<u8> {
    settings::encode(&BTreeMap::new()).unwrap()
}

fn export(name: &str) -> Export {
    Export {
        name: name.into(),
        kind: ObjectKind::MailboxSender as u32,
        rights: SENDER,
    }
}

fn resident(name: &str, image: &str) -> Instance {
    Instance {
        name: name.into(),
        image: image.into(),
        kind: Kind::Resident,
        job: None,
        pool: Budget {
            work_pages: 16,
            metadata_bytes: 4096,
        },
        inputs: vec![Input {
            name: "memory.pool".into(),
            source: Source::SelfPool,
            rights: None,
            revoke: None,
        }],
        exports: Vec::new(),
        settings: empty_settings(),
        start_timeout_ms: 5000,
        stop_timeout_ms: 40000,
        after: Vec::new(),
    }
}

/// System 的形状：device 签发预限定来源，driver 经可撤销路由消费，worker Job 由 driver 付款。
fn system() -> Formation {
    let mut device = resident("device", "srv_device");
    device.inputs.push(Input {
        name: "platform.root".into(),
        source: Source::PlatformRoot,
        rights: None,
        revoke: None,
    });
    device.exports = vec![
        export("device.root"),
        export("device.root.admin"),
        export("device.source.driver"),
        export("device.source.driver.admin"),
    ];
    let mut driver = resident("driver", "srv_driver");
    driver.inputs.push(Input {
        name: "device.source".into(),
        source: Source::parse("device/device.source.driver").unwrap(),
        rights: None,
        revoke: Source::parse("device/device.source.driver.admin"),
    });
    driver.inputs.push(Input {
        name: "driver.workers".into(),
        source: Source::Job("drivers".into()),
        rights: None,
        revoke: None,
    });
    driver.exports = vec![export("driver.control")];
    let fs = resident("fs", "srv_fs");
    Formation {
        finish: Finish::Stay,
        pools: Vec::new(),
        jobs: vec![Job {
            name: "drivers".into(),
            parent: SERVICES_JOB.into(),
            payer: "driver".into(),
        }],
        // driver 先于 device 声明，顺序仍须由依赖决定。
        instances: vec![driver, fs, device],
    }
}

fn images(name: &str) -> bool {
    matches!(name, "srv_device" | "srv_driver" | "srv_fs" | "test_kernel")
}

fn rejects(formation: &Formation, reason: &str) {
    match formation.validate(images) {
        Err(Error::Invalid { reason: got, .. }) => assert_eq!(got, reason),
        other => panic!("expected {reason}, got {other:?}"),
    }
}

#[test]
fn names_follow_the_segment_grammar() {
    for good in [
        "device",
        "device.root.admin",
        "platform.root.0",
        "file.root.12",
    ] {
        assert!(valid_name(good), "{good}");
    }
    for bad in [
        "",
        "Device",
        "0device",
        "device..root",
        "device.",
        "root.01",
        "a.0.b",
        "device-root",
        "abcdefghijklmnopqrstuvwxyzabcdefg",
    ] {
        assert!(!valid_name(bad), "{bad}");
    }
}

#[test]
fn sources_round_trip_through_their_text_form() {
    for text in [
        "init/platform.description",
        "init/platform.root",
        "init/image.drv_usb_hid",
        "init/job.drivers",
        "init/pool.retention",
        "self/memory.pool",
        "device/device.source.driver",
    ] {
        assert_eq!(Source::parse(text).unwrap().to_string(), text);
    }
    for bad in [
        "init/clock",
        "self/job",
        "device",
        "Init/x.y",
        "init/image.Bad",
    ] {
        assert!(Source::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn encoding_round_trips_and_rejects_corruption() {
    let formation = system();
    let bytes = formation.encode().unwrap();
    assert_eq!(Formation::decode(&bytes).unwrap(), formation);
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        Formation::decode(&trailing),
        Err(Error::Format("trailing bytes"))
    );
    for cut in 0..bytes.len() {
        assert!(Formation::decode(&bytes[..cut]).is_err(), "cut {cut}");
    }
    let mut magic = bytes;
    magic[0] ^= 1;
    assert_eq!(Formation::decode(&magic), Err(Error::Format("bad magic")));
}

#[test]
fn order_follows_dependencies_then_declaration() {
    let formation = system();
    formation.validate(images).unwrap();
    let names: Vec<&str> = formation
        .order()
        .unwrap()
        .into_iter()
        .map(|i| formation.instances[i].name.as_str())
        .collect();
    assert_eq!(names, ["fs", "device", "driver"]);
}

#[test]
fn static_validation_rejects_bad_compositions() {
    let mut f = system();
    f.instances[0].image = "srv_missing".into();
    rejects(&f, "missing image");

    let mut f = system();
    f.instances[1].name = "driver".into();
    rejects(&f, "duplicate instance");

    let mut f = system();
    f.instances[1].name = "init".into();
    rejects(&f, "invalid instance name");

    let mut f = system();
    f.instances[0].inputs[1].source = Source::parse("device/device.source.other").unwrap();
    rejects(&f, "undeclared export");

    let mut f = system();
    f.instances[0].inputs[1].rights = Some(SENDER | Rights::MANAGE);
    rejects(&f, "route rights exceed the export");

    let mut f = system();
    f.instances[0].inputs[1].revoke = Source::parse("device/device.root.admin");
    rejects(&f, "revoke must name <export>.admin of the provider");

    let mut f = system();
    f.instances[0].inputs.push(Input {
        name: "instance.stop".into(),
        source: Source::SelfPool,
        rights: None,
        revoke: None,
    });
    rejects(&f, "invalid input name");

    let mut f = system();
    f.instances[2].after = vec!["driver".into()];
    rejects(&f, "instance dependency cycle");

    let mut f = system();
    f.jobs[0].parent = "drivers".into();
    rejects(&f, "job parent cycle");

    let mut f = system();
    f.instances[1].stop_timeout_ms = 0;
    rejects(&f, "resident instance requires both timeouts");

    let mut f = system();
    f.instances[1].pool.work_pages = u64::MAX;
    rejects(&f, "budget sum overflows");

    let mut f = system();
    f.finish = Finish::Shutdown;
    rejects(&f, "finish action requires at least one task");

    let mut f = system();
    f.instances[1].settings = vec![0];
    rejects(&f, "invalid settings encoding");
}

#[test]
fn kind_and_right_names_resolve() {
    assert_eq!(
        kind_by_name("mailbox_sender"),
        Some(ObjectKind::MailboxSender as u32)
    );
    assert_eq!(
        kind_name(ObjectKind::DmaMapping as u32),
        Some("dma_mapping")
    );
    assert_eq!(kind_by_name("mailbox"), None);
    assert_eq!(right_by_name("grant"), Some(Rights::GRANT));
    assert_eq!(right_by_name("own"), None);
}
