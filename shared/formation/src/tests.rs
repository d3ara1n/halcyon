use super::*;
use alloc::{collections::BTreeMap, vec};
fn call() -> Rights {
    Rights::WRITE | Rights::WAIT | Rights::TRANSIT
}
fn object(name: &str) -> Source {
    Source::Resource {
        name: name.into(),
        output: ResourceOutput::Main,
    }
}
fn lifetime(name: &str) -> Source {
    Source::Resource {
        name: name.into(),
        output: ResourceOutput::Lifetime,
    }
}
fn input(name: &str, source: Source) -> Input {
    Input {
        name: name.into(),
        source,
        rights: None,
    }
}
fn instance(name: &str) -> Instance {
    Instance {
        name: name.into(),
        image: "srv_host".into(),
        job: None,
        pool: Budget {
            work_pages: 2,
            metadata_bytes: 4096,
        },
        inputs: Vec::new(),
        namespace: Vec::new(),
        settings: settings::encode(&BTreeMap::new()).unwrap(),
    }
}
fn formation() -> Formation {
    let mut i = instance("host");
    i.inputs = vec![
        input("receiver", object("mailbox")),
        input("proof", object("entry")),
        input("life", lifetime("entry")),
    ];
    Formation {
        pools: Vec::new(),
        jobs: Vec::new(),
        resources: vec![
            Resource {
                name: "mailbox".into(),
                payer: "init".into(),
                kind: ResourceKind::Mailbox {
                    rights: Rights::READ | Rights::WAIT,
                },
                terminal_revoke: false,
            },
            Resource {
                name: "entry".into(),
                payer: "host".into(),
                kind: ResourceKind::Sender {
                    mailbox: "mailbox".into(),
                    badge: 91,
                    rights: call(),
                },
                terminal_revoke: false,
            },
        ],
        instances: vec![i],
    }
}
#[test]
fn v2_resources_namespace_roundtrip_and_reject_legacy_truncated_trailing() {
    let mut f = formation();
    f.instances[0].namespace.push(NamespaceBinding {
        prefix: "/雪".into(),
        source: object("entry"),
        rights: call(),
    });
    f.resources[1].terminal_revoke = true;
    if let ResourceKind::Sender { rights, .. } = &mut f.resources[1].kind {
        *rights |= Rights::DUPLICATE;
    }
    f.validate(|_| true).unwrap();
    let bytes = f.encode().unwrap();
    assert_eq!(Formation::decode(&bytes).unwrap(), f);
    for n in 0..bytes.len() {
        assert!(Formation::decode(&bytes[..n]).is_err());
    }
    let mut old = bytes.clone();
    old[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert!(Formation::decode(&old).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(Formation::decode(&trailing).is_err());
}
#[test]
fn static_sender_routes_do_not_create_instance_topology() {
    let mut f = formation();
    let mut peer = instance("peer");
    peer.inputs.push(input("receiver", object("peer.mailbox")));
    peer.inputs.push(input("host", object("entry")));
    f.instances[0]
        .inputs
        .push(input("peer", object("peer.entry")));
    f.instances.push(peer);
    f.resources.push(Resource {
        name: "peer.mailbox".into(),
        payer: "host".into(),
        kind: ResourceKind::Mailbox {
            rights: Rights::READ | Rights::WAIT,
        },
        terminal_revoke: false,
    });
    f.resources.push(Resource {
        name: "peer.entry".into(),
        payer: "peer".into(),
        kind: ResourceKind::Sender {
            mailbox: "peer.mailbox".into(),
            badge: 92,
            rights: call(),
        },
        terminal_revoke: false,
    });
    // 双方都持对方真实入口，并分别用对方或自身 Pool 付款；没有启动先后依赖。
    f.resources[1].payer = "peer".into();
    f.validate(|_| true).unwrap();
    f.instances.reverse();
    f.validate(|_| true).unwrap();
}

#[test]
fn receiver_exactly_one_and_lifetime_at_most_one_move_owner() {
    let f = formation();
    let mut missing = f.clone();
    missing.instances[0].inputs.remove(0);
    assert!(missing.validate(|_| true).is_err());
    let mut duplicate = f.clone();
    duplicate.instances[0]
        .inputs
        .push(input("other", object("mailbox")));
    assert!(duplicate.validate(|_| true).is_err());
    let mut duplicate = f.clone();
    duplicate.instances[0]
        .inputs
        .push(input("other", lifetime("entry")));
    assert!(duplicate.validate(|_| true).is_err());
    let mut unused = f;
    unused.instances[0].inputs.pop();
    unused.validate(|_| true).unwrap();
}
#[test]
fn references_payers_kinds_and_images_checked() {
    let mut f = formation();
    f.resources[1].payer = "missing".into();
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    if let ResourceKind::Sender { mailbox, .. } = &mut f.resources[1].kind {
        *mailbox = "entry".into();
    }
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0].inputs[2].source = lifetime("mailbox");
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0].inputs[1].source = object("unknown");
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input("image", Source::Image("drv_worker".into())));
    assert!(f.validate(|name| name != "drv_worker").is_err());
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input("pool", Source::Pool("host".into())));
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.pools.push(Pool {
        name: "host".into(),
        budget: Budget::default(),
    });
    assert!(f.validate(|_| true).is_err());
}
#[test]
fn rights_at_every_edge_and_object_factory() {
    let mut f = formation();
    f.instances[0].inputs[1].rights = Some(Rights::MANAGE);
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0].inputs[2].rights = Some(Rights::DUPLICATE);
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input("description", Source::PlatformDescription));
    f.instances[0].inputs.last_mut().unwrap().rights = Some(Rights::WRITE);
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.resources[0].kind = ResourceKind::Mailbox {
        rights: Rights::DUPLICATE,
    };
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    if let ResourceKind::Sender { rights, .. } = &mut f.resources[1].kind {
        *rights = Rights::READ;
    }
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0].inputs[1].rights = Some(Rights::WRITE);
    f.validate(|_| true).unwrap();
    assert_eq!(f.source_rights(&lifetime("entry")), Ok(Rights::WAIT));
}
#[test]
fn namespaces_are_explicit_sender_views_and_cannot_shadow_hidden_inputs() {
    let bind = |source, rights| NamespaceBinding {
        prefix: "/".into(),
        source,
        rights,
    };
    for b in [
        bind(object("mailbox"), Rights::READ),
        bind(lifetime("entry"), Rights::WAIT),
        bind(Source::PlatformRoot, call()),
        bind(object("entry"), Rights::WRITE),
    ] {
        let mut f = formation();
        f.instances[0].namespace.push(b);
        assert!(f.validate(|_| true).is_err());
    }
    let mut f = formation();
    f.instances[0].namespace = vec![bind(object("entry"), call()), bind(object("entry"), call())];
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0].namespace.push(NamespaceBinding {
        prefix: "/a/..".into(),
        source: object("entry"),
        rights: call(),
    });
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input("oal.entry.0", object("entry")));
    assert!(f.validate(|_| true).is_err());
}
#[test]
fn actual_grants_include_platform_expansion_and_namespace() {
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input("platform", Source::PlatformRoot));
    f.instances[0].namespace.push(NamespaceBinding {
        prefix: "/".into(),
        source: object("entry"),
        rights: call(),
    });
    f.validate(|_| true).unwrap();
    f.validate_grants(MAX_INPUTS - 4).unwrap();
    assert!(f.validate_grants(MAX_INPUTS - 3).is_err());
    assert!(f.validate_grants(usize::MAX).is_err());
    // host 无法知道平台展开数，但可拒绝普通输入和 Namespace 已经超限的部署。
    for n in 0..MAX_INPUTS {
        f.instances[0]
            .inputs
            .push(input(&alloc::format!("extra.{n}"), object("entry")));
    }
    assert!(f.validate(|_| true).is_err());
}
#[test]
fn terminal_revoke_is_explicit_not_a_suffix_policy() {
    let mut f = formation();
    f.resources[1].name = "entry.admin".into();
    f.instances[0].inputs[1].source = object("entry.admin");
    f.instances[0].inputs[2].source = lifetime("entry.admin");
    f.validate(|_| true).unwrap();
    assert!(!f.resources[1].terminal_revoke);
    f.resources[1].terminal_revoke = true;
    assert!(f.validate(|_| true).is_err());
    if let ResourceKind::Sender { rights, .. } = &mut f.resources[1].kind {
        *rights |= Rights::DUPLICATE;
    }
    f.validate(|_| true).unwrap();
    f.resources[0].terminal_revoke = true;
    assert!(f.validate(|_| true).is_err());
    for missing in [
        Rights::WRITE,
        Rights::WAIT,
        Rights::TRANSIT,
        Rights::DUPLICATE,
    ] {
        let mut f = formation();
        f.resources[1].terminal_revoke = true;
        if let ResourceKind::Sender { rights, .. } = &mut f.resources[1].kind {
            *rights = Rights::from_raw((call() | Rights::DUPLICATE).raw() & !missing.raw());
        }
        assert!(f.validate(|_| true).is_err());
    }
}
#[test]
fn job_parent_cycles_budget_overflow_and_duplicate_names_rejected() {
    let mut f = formation();
    f.jobs = vec![
        Job {
            name: "a".into(),
            parent: "b".into(),
            payer: "init".into(),
        },
        Job {
            name: "b".into(),
            parent: "a".into(),
            payer: "host".into(),
        },
    ];
    assert!(f.validate(|_| true).is_err());
    f.jobs[1].parent = SERVICES_JOB.into();
    f.validate(|_| true).unwrap();
    let mut f = formation();
    f.resources.push(f.resources[1].clone());
    assert!(f.validate(|_| true).is_err());
    let mut f = formation();
    f.pools.push(Pool {
        name: "overflow".into(),
        budget: Budget {
            work_pages: u64::MAX,
            metadata_bytes: 0,
        },
    });
    assert!(f.validate(|_| true).is_err());
}
#[test]
fn static_source_syntax_names_and_reserved_old_routes() {
    for s in [
        "init/platform.description",
        "init/platform.root",
        "init/image.srv_fs",
        "init/job.services",
        "init/pool.extra",
        "self/memory.pool",
        "init/object.file.root.0",
        "init/lifetime.file.root.admin",
    ] {
        assert_eq!(Source::parse(s).unwrap().to_string(), s);
    }
    for s in [
        "fs/file.root.0",
        "init/object.",
        "init/object.a..b",
        "init/lifetime.bad/route",
    ] {
        assert!(Source::parse(s).is_none());
    }
    for s in ["a", "file.root.0", "file.root.admin"] {
        assert!(valid_name(s));
    }
    for s in ["", "A", "a..b", "a.01", "a.1.b"] {
        assert!(!valid_name(s));
    }
}

#[test]
fn expanded_platform_names_cannot_alias_ordinary_inputs() {
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input("platform", Source::PlatformRoot));
    f.instances[0]
        .inputs
        .push(input("platform.0", object("entry")));
    f.validate(|_| true).unwrap();
    assert!(f.validate_grants(1).is_err());
    let mut f = formation();
    f.instances[0]
        .inputs
        .push(input(&"a".repeat(MAX_NAME), Source::PlatformRoot));
    assert!(f.validate_grants(1).is_err());
}
