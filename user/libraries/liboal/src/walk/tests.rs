use super::*;
use crate::{LinkBoundary, ProviderBoundary};
use erhino_shared::time::Deadline;

fn state(path: &str, root: &str) -> State<u32> {
    // 入口只是状态算法中的值，不伪造 kernel sender 或 RPC 运行体。
    State::new(
        root,
        1,
        String::from(path),
        FollowBudget::new(10, 10000, Deadline::at(100)).unwrap(),
    )
    .unwrap()
}

#[test]
fn alternating_boundaries_preserve_parent_frames_and_exact_unconsumed_path() {
    // 换入口后相对链接可留在新入口内，也可越根回已保留的上层入口。
    let mut walk = state("/mount/link/file", "/");
    assert!(matches!(
        walk.follow(
            Boundary::Provider(ProviderBoundary {
                consumed_bytes: 5,
                remaining: "link/file"
            }),
            Some(2)
        )
        .unwrap(),
        Advance::Local
    ));
    assert_eq!(walk.target().unwrap(), (&2, "link/file"));
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "album",
                remaining: "file"
            }),
            None
        )
        .unwrap(),
        Advance::Local
    ));
    assert_eq!(walk.target().unwrap(), (&2, "album/file"));
    assert!(matches!(
        walk.follow(
            Boundary::Provider(ProviderBoundary {
                consumed_bytes: 5,
                remaining: "file"
            }),
            Some(3)
        )
        .unwrap(),
        Advance::Local
    ));
    assert_eq!(walk.target().unwrap(), (&3, "file"));
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "../../other",
                remaining: ""
            }),
            None
        )
        .unwrap(),
        Advance::Local
    ));
    assert_eq!(walk.target().unwrap(), (&1, "other"));
    assert_eq!(walk.frames.len(), 1);
}

#[test]
fn empty_provider_suffix_queries_entry_root_without_leaf_session() {
    // 完整消耗前缀后应向新入口发空路径，而不是把边界当作对象。
    let mut walk = state("/mount", "/");
    walk.follow(
        Boundary::Provider(ProviderBoundary {
            consumed_bytes: 5,
            remaining: "",
        }),
        Some(2),
    )
    .unwrap();
    assert_eq!(walk.target().unwrap(), (&2, ""));
    assert!(
        walk.follow(
            Boundary::Provider(ProviderBoundary {
                consumed_bytes: 1,
                remaining: ""
            }),
            Some(3)
        )
        .is_err()
    );
}

#[test]
fn view_absolute_links_reroute_and_relative_escape_falls_back_to_namespace() {
    // 绝对 target 总是从 Namespace 重选；没有上层帧时相对 .. 也重选。
    let mut walk = state("/a/link/x", "/a");
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "/b",
                remaining: "x"
            }),
            None
        )
        .unwrap(),
        Advance::Reroute
    ));
    assert_eq!(walk.path, "/b/x");
    walk.reroute("/b", 2).unwrap();
    assert_eq!(walk.target().unwrap(), (&2, "x"));
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "../c",
                remaining: ""
            }),
            None
        )
        .unwrap(),
        Advance::Reroute
    ));
    assert_eq!(walk.path, "/c");
}

#[test]
fn escape_then_return_does_not_reuse_crossed_entry_frame() {
    // ../mount/file 越过 /mount 后又回其下，必须从父入口重新解析 mount。
    let mut walk = state("/mount/link", "/");
    walk.follow(
        Boundary::Provider(ProviderBoundary {
            consumed_bytes: 5,
            remaining: "link",
        }),
        Some(2),
    )
    .unwrap();
    walk.follow(
        Boundary::Link(LinkBoundary {
            parent_bytes: 0,
            target: "../mount/file",
            remaining: "",
        }),
        None,
    )
    .unwrap();
    assert_eq!(walk.target().unwrap(), (&1, "mount/file"));
}

#[test]
fn cycles_exhaust_original_call_or_expansion_budget_and_bad_boundaries_fail() {
    // 不设魔数链接上限；相同链接无限返回也必耗尽调用方的原始预算。
    let mut walk = state("/loop/x", "/");
    for _ in 0..10 {
        walk.budget.before_call(1).unwrap();
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "loop",
                remaining: "x",
            }),
            None,
        )
        .unwrap();
    }
    assert_eq!(walk.budget.before_call(1), Err(LocalError::BudgetExceeded));
    let mut walk = state("/loop/x", "/");
    walk.budget
        .consume_bytes(walk.budget.remaining_bytes() - 1)
        .unwrap();
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "loop",
                remaining: "x"
            }),
            None
        ),
        Err(LocalError::BudgetExceeded)
    ));
    let mut walk = state("/a/x", "/");
    assert!(matches!(
        walk.follow(
            Boundary::Provider(ProviderBoundary {
                consumed_bytes: 1,
                remaining: "wrong"
            }),
            Some(2)
        ),
        Err(LocalError::Protocol)
    ));
    assert!(matches!(
        walk.follow(
            Boundary::Provider(ProviderBoundary {
                consumed_bytes: 1,
                remaining: "x"
            }),
            None
        ),
        Err(LocalError::Protocol)
    ));
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "a",
                remaining: "x"
            }),
            Some(2)
        ),
        Err(LocalError::Protocol)
    ));
    assert!(matches!(
        walk.follow(
            Boundary::Link(LinkBoundary {
                parent_bytes: 0,
                target: "../a",
                remaining: "x"
            }),
            None
        ),
        Err(LocalError::InvalidPath)
    ));
}
