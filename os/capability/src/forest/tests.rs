//! 独立父数组模型只用于判环/连通，不重用 ETT 序列逻辑。
//! 捕获 split/join 平衡及 parent 修复、tour 切片方向、批量旧边误判与回滚丢边。

use super::*;
use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    vec,
    vec::Vec,
};

struct Fixture {
    ids: Vec<DomainId>,
    storage: Box<[Domain]>,
}

impl Fixture {
    fn new(n: usize) -> Self {
        let storage: Box<[Domain]> = (0..n).map(|_| Domain::new(true)).collect();
        // SAFETY: Box 内地址不变；fixture 单线程独占全部域，Drop 前切除所有边。
        let ids = storage
            .iter()
            .map(|d| unsafe { DomainId::new(d) })
            .collect();
        Self { ids, storage }
    }
    fn cut(&self, child: usize) -> Option<usize> {
        // SAFETY: fixture 私有、单线程，所有相关域由 Box 保活且地址稳定。
        unsafe { cut(self.ids[child]) }.map(|id| self.index(id))
    }
    fn link(&self, child: usize, p: usize) -> Result<(), LinkError> {
        // SAFETY: 同 cut；无外部参与者，无需实际锁。
        unsafe { link(self.ids[child], self.ids[p]) }
    }
    fn index(&self, id: DomainId) -> usize {
        self.ids.iter().position(|&d| d == id).unwrap()
    }
    fn install(&self, model: &[Option<usize>]) {
        for (child, p) in model.iter().enumerate() {
            if let Some(p) = p {
                self.link(child, *p).unwrap();
            }
        }
    }
    fn check(&self, model: &[Option<usize>]) {
        // SAFETY: 与 cut 相同；此检查不改变域状态。
        unsafe {
            for (i, &id) in self.ids.iter().enumerate() {
                assert_eq!(parent(id).map(|id| self.index(id)), model[i]);
                for (j, &other) in self.ids.iter().enumerate() {
                    assert_eq!(
                        connected(id, other),
                        model_root(model, i) == model_root(model, j)
                    );
                }
            }
        }
        self.check_structure(model);
    }
    fn check_structure(&self, model: &[Option<usize>]) {
        let mut roots = BTreeMap::new();
        let mut events = BTreeMap::new();
        for (i, &id) in self.ids.iter().enumerate() {
            roots.insert(root(id.marker()).0.as_ptr() as usize, root(id.marker()));
            events.insert(id.marker().0.as_ptr() as usize, (i, i));
            if let Some(p) = model[i] {
                events.insert(id.forward().0.as_ptr() as usize, (p, i));
                events.insert(id.backward().0.as_ptr() as usize, (i, p));
            } else {
                for t in [id.forward(), id.backward()] {
                    let n = t.read();
                    assert!(n.left.is_none() && n.right.is_none() && n.parent.is_none());
                    assert_eq!((n.height, n.size), (1, 1));
                }
            }
        }
        let mut visited = BTreeSet::new();
        for &r in roots.values() {
            let mut sequence = Vec::new();
            let (h, _) = validate(Some(r), None, &mut visited, &mut sequence);
            assert!(h <= MAX_HEIGHT);
            let start = events[&(sequence[0].0.as_ptr() as usize)].0;
            let mut current = start;
            // 独立验证循环 tour 的每条有向边前后端连续，顶点标记位于正确顶点。
            for token in sequence {
                let &(from, to) = events
                    .get(&(token.0.as_ptr() as usize))
                    .expect("Unexpected tour token");
                assert_eq!(current, from, "Euler tour endpoints must agree");
                current = to;
            }
            assert_eq!(current, start);
        }
        assert_eq!(
            visited.len(),
            events.len(),
            "Every active token must occur exactly once"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for &id in &self.ids {
            // SAFETY: Box 仍保活所有域；无外部参与者，切掉每个父边后必全为单点。
            unsafe {
                cut(id);
            }
        }
        for &id in &self.ids {
            // SAFETY: 同上，仍未销毁 storage。
            assert!(unsafe { is_isolated(id) });
        }
        assert_eq!(self.storage.len(), self.ids.len());
    }
}

fn validate(
    tree: Link,
    parent: Link,
    visited: &mut BTreeSet<usize>,
    sequence: &mut Vec<Token>,
) -> (usize, usize) {
    let Some(t) = tree else { return (0, 0) };
    assert!(
        visited.insert(t.0.as_ptr() as usize),
        "Duplicate or cyclic AVL token"
    );
    let n = t.read();
    assert_eq!(n.parent, parent);
    let (lh, ls) = validate(n.left, Some(t), visited, sequence);
    sequence.push(t);
    let (rh, rs) = validate(n.right, Some(t), visited, sequence);
    assert!(
        lh.abs_diff(rh) <= 1,
        "AVL balance violated: left={lh}, right={rh}"
    );
    assert_eq!(n.height, lh.max(rh) + 1);
    assert_eq!(n.size, ls + rs + 1);
    (n.height, n.size)
}

fn model_root(model: &[Option<usize>], mut i: usize) -> usize {
    while let Some(p) = model[i] {
        i = p;
    }
    i
}

fn acyclic(model: &[Option<usize>]) -> bool {
    for start in 0..model.len() {
        let mut seen = vec![false; model.len()];
        let mut at = Some(start);
        while let Some(i) = at {
            if seen[i] {
                return false;
            }
            seen[i] = true;
            at = model[i];
        }
    }
    true
}

fn forests(n: usize, mut visit: impl FnMut(Vec<Option<usize>>)) {
    for mut code in 0..(n + 1).pow(n as u32) {
        let mut model = vec![None; n];
        for p in &mut model {
            let digit = code % (n + 1);
            code /= n + 1;
            *p = (digit < n).then_some(digit);
        }
        if acyclic(&model) {
            visit(model);
        }
    }
}

#[test]
fn all_small_forests_cut_link_match_independent_model() {
    let mut count = 0;
    for n in 1..=5 {
        forests(n, |initial| {
            count += 1;
            let f = Fixture::new(n);
            f.install(&initial);
            f.check(&initial);
            for child in 0..n {
                let mut model = initial.clone();
                assert_eq!(f.cut(child), model[child].take());
                f.check(&model);
                assert_eq!(f.cut(child), None);
                f.check(&model);
                for p in 0..n {
                    let mut candidate = model.clone();
                    candidate[child] = Some(p);
                    if acyclic(&candidate) {
                        assert_eq!(f.link(child, p), Ok(()));
                        f.check(&candidate);
                        assert_eq!(f.link(child, p), Err(LinkError::NotRoot));
                        f.check(&candidate);
                        assert_eq!(f.cut(child), Some(p));
                    } else {
                        assert_eq!(f.link(child, p), Err(LinkError::Cycle));
                    }
                    f.check(&model);
                }
                if let Some(p) = initial[child] {
                    f.link(child, p).unwrap();
                }
                f.check(&initial);
            }
        });
    }
    assert_eq!(count, 1441);
}

/// 直接调用产品事务，不在测试侧复刻 cut/link/恢复算法。
fn batch<const K: usize>(
    f: &Fixture,
    children: &[Option<DomainId>],
    target: Option<DomainId>,
) -> bool {
    assert!(children.iter().flatten().all(|id| f.ids.contains(id)));
    assert!(target.is_none_or(|id| f.ids.contains(&id)));
    // SAFETY: fixture 单线程且全部 Box 存活，调用点输入不重复且长度不超过 K。
    match unsafe { transfer::<K>(children, target) } {
        Ok(()) => true,
        Err(LinkError::Cycle) => false,
        error => panic!("Unexpected transfer result: {error:?}"),
    }
}

#[test]
fn exhaustive_batch_success_and_rollback() {
    let (mut attempts, mut rejected) = (0usize, 0usize);
    for n in 1..=4 {
        forests(n, |initial| {
            for mask in 1..(1 << n) {
                for target in (0..n).map(Some).chain(core::iter::once(None)) {
                    let f = Fixture::new(n);
                    f.install(&initial);
                    let mut candidate = initial.clone();
                    for (i, p) in candidate.iter_mut().enumerate() {
                        if mask & (1 << i) != 0 {
                            *p = target;
                        }
                    }
                    let expected = acyclic(&candidate);
                    let children: Vec<_> = (0..n)
                        .filter(|i| mask & (1 << i) != 0)
                        .map(|i| Some(f.ids[i]))
                        .collect();
                    let result = batch::<4>(&f, &children, target.map(|i| f.ids[i]));
                    attempts += 1;
                    rejected += usize::from(!result);
                    assert_eq!(result, expected);
                    f.check(if result { &candidate } else { &initial });
                }
            }
        });
    }
    assert!(attempts > 9000 && rejected > 5000);
}

#[test]
fn fixed_64_record_batch_restores_a_late_cycle() {
    let n = 66;
    let f = Fixture::new(n);
    let mut initial = vec![None; n];
    // 0->1->...->63，另留两个根。上层记录确实用满64项，最后一项形成自环。
    for (i, p) in initial.iter_mut().enumerate().take(63) {
        *p = Some(i + 1);
    }
    f.install(&initial);
    let children: Vec<_> = f.ids[..64].iter().copied().map(Some).collect();
    assert!(!batch::<64>(&f, &children, Some(f.ids[63])));
    f.check(&initial);
}

#[test]
fn long_logical_chain_has_only_avl_height_bound() {
    let n = 16_384;
    let f = Fixture::new(n);
    let mut model = vec![None; n];
    for child in 1..n {
        f.link(child, child - 1).unwrap();
        model[child] = Some(child - 1);
        // 几何里程碑核全部父指针、size、height 和有向 tour，避免 host 二次方审计成本。
        if child.is_power_of_two() {
            f.check_structure(&model);
        }
    }
    f.check_structure(&model);
    // AVL 索引链不是业务链。两端连通和成环检测均不遍历这条16K业务链。
    // SAFETY: fixture 单线程且 Box 保活全部稳定域。
    unsafe {
        assert!(connected(f.ids[0], f.ids[n - 1]));
    }
    assert_eq!(f.link(0, n - 1), Err(LinkError::Cycle));
    assert_eq!(f.cut(n / 2), Some(n / 2 - 1));
    model[n / 2] = None;
    f.check_structure(&model);
    // SAFETY: 同上。
    unsafe {
        assert!(!connected(f.ids[0], f.ids[n - 1]));
    }
    f.link(n / 2, n / 2 - 1).unwrap();
    model[n / 2] = Some(n / 2 - 1);
    for child in (1..n).rev() {
        assert_eq!(f.cut(child), model[child].take());
        if child.is_power_of_two() {
            f.check_structure(&model);
        }
    }
    f.check_structure(&model);
}

#[test]
fn permanent_root_keeps_children_but_never_relinks() {
    let f = Fixture::new(5);
    let mut model = vec![None, Some(0), Some(1), Some(2), None];
    f.install(&model);
    // SAFETY: fixture 单线程且存储由 Box 保活。
    unsafe {
        assert_eq!(make_root(f.ids[2]), Some(f.ids[1]));
        model[2] = None;
        assert!(!is_controlled(f.ids[2]));
        assert_eq!(make_root(f.ids[2]), None);
        assert!(!is_isolated(f.ids[2]));
    }
    f.check(&model);
    assert_eq!(f.link(2, 4), Err(LinkError::Inactive));
    assert_eq!(f.link(4, 2), Err(LinkError::Inactive));
    f.check(&model);
    assert_eq!(f.cut(3), Some(2));
    model[3] = None;
    f.check(&model);
    // SAFETY: fixture 单线程，父/子边均已切除。
    unsafe {
        assert!(is_isolated(f.ids[2]));
    }
}

#[test]
fn failed_transfer_restores_old_edges_into_permanent_root() {
    let f = Fixture::new(5);
    let mut model = vec![None, Some(0), Some(1), Some(2), None];
    f.install(&model);
    // 2 永久转根，但3->2合法保留；事务随后必须精确恢复这条旧边。
    // SAFETY: fixture 单线程且全部稳定存储仍由 Box 保活。
    unsafe {
        make_root(f.ids[2]);
    }
    model[2] = None;
    f.check(&model);
    let children = [Some(f.ids[3]), None, Some(f.ids[2]), Some(f.ids[4])];
    assert!(!batch::<4>(&f, &children, Some(f.ids[4])));
    // 前项3->4已成功，末项4->4失败；永久根child=2被跳过。
    f.check(&model);
    // 向永久根移动只cut；永久根child不被重新激活，None为普通叶能力。
    assert!(batch::<4>(&f, &children, Some(f.ids[2])));
    model[3] = None;
    f.check(&model);
    // SAFETY: 同上；control状态由forest单一真值拥有。
    unsafe {
        assert!(!is_controlled(f.ids[2]));
    }
    assert!(batch::<0>(&f, &[], None));
}

#[test]
fn standalone_domains_need_no_registration_or_allocation_by_forest() {
    let domain = Box::new(Domain::new(false));
    // SAFETY: Box保活稳定单点，且该测试没有其他参与者。
    unsafe {
        let id = DomainId::new(&domain);
        assert_eq!(parent(id), None);
        assert!(!is_controlled(id));
        assert!(is_isolated(id));
        assert!(connected(id, id));
        assert_eq!(cut(id), None);
        assert_eq!(make_root(id), None);
    }
}

#[test]
fn derived_height_and_checked_size_boundaries() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Domain>();
    send_sync::<DomainId>();
    let mut minimum = vec![0usize, 1];
    while let Some(next) = minimum[minimum.len() - 1]
        .checked_add(minimum[minimum.len() - 2])
        .and_then(|n| n.checked_add(1))
    {
        minimum.push(next);
    }
    assert_eq!(MAX_HEIGHT, minimum.len() - 1);
    assert_eq!(MAX_HEIGHT, if usize::BITS == 64 { 91 } else { 45 });
    // 无法实际分配接近 usize::MAX 的分量；在精确的准入算术边界独立验证溢出。
    assert_eq!(joined_size(usize::MAX - 3, 1), Some(usize::MAX));
    assert_eq!(joined_size(usize::MAX - 2, 1), None);
    assert_eq!(joined_size(usize::MAX, 1), None);
    assert_eq!(joined_size(1, 1), Some(4));
}
