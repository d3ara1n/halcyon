//! 固定预付存储的确定性 Euler-tour / AVL 森林。
//!
//! 每个域内嵌一个顶点标记和自己父边的两个方向 token；无注册、分配、全局计数、
//! 随机优先级或逻辑父链扫描。token 归属由域内固定字段确定，不另存重复 owner。
//! 新域天然是单点，只有第一次连接时才产生跨域指针。
//! `parent` 是唯一业务父边，AVL parent 仅为序列索引，不是第二份业务关系。
//!
//! # 同步与存储合同
//!
//! 本模块不拥有锁或存储引用。所有操作（包括查询）必须在同一个外部排他锁下进行。
//! 从首次使用到所有 incident edge 切除，域必须处于稳定地址；稳定堆对象可以直接
//! 内嵌 [`Domain`]，不需要注册或认识其分配器。调用者必须用真实存储 owner 保持
//! 所有可达域存活：特别是拆父边必须先于来源 owner 的最后释放。销毁前必须成为
//! 单点；Drop 不加锁、不扫描、不自动拆边。裸身份和索引本身不拥有存储。
//!
//! [`transfer`] 用固定 `[Option<(DomainId, Option<DomainId>)>; K]` 记录旧父边，
//! 先切全批再连；失败时切掉新增边并恢复全部旧边，包括永久根上保留的既存子边。
//! 恢复无需分配；原森林无环，故恢复顺序不影响准入。上层锁必须覆盖父边快照、
//! 全部图变更和实际能力提交或回滚；不得在锁内调用可能析构域的代码。
//!
//! # 工作与空间界
//!
//! 空树高 0，单点高 1。AVL 最小节点数 `m(0)=0, m(1)=1,
//! m(h+2)=1+m(h+1)+m(h)`；[`MAX_HEIGHT`] 是 checked usize 递推可表示的最高 h
//! （64 位为 91，32 位为 45），不是能力链深限额。每个分量 size 都 checked。
//! JOIN 沿较高树的一条脊下降/上升；SPLIT 沿查找路径下降，回程 JOIN 的高度差
//! 望远镜求和为 O(h)，不是每层 O(h)：分裂片段的高度不超过原路径子树；
//! 原 AVL 兄弟高度与路径子树相差至多 1，所以每个 JOIN 的下降层数不超过
//! 本侧累积结果的高度增加量。左右两侧分别望远镜求和，合计至多 2h。
//! 两者均无递归。SPLIT scratch 为
//! `MAX_HEIGHT * (size_of::<Option<Token>>() + size_of::<bool>())`，内层 JOIN
//! 另用 `MAX_HEIGHT * size_of::<Option<Token>>()`，不随进程数或逻辑链深增长。
//! RV64 同时存活的两组数组共 1547 字节，transfer::<64> 的记录另占 1024 字节；
//! 尚不含编译器局部变量/调用帧，精确机器栈由内核 ELF 审计核定。
//! connected/rank 仅走 AVL 父指针，业务 parent 查询 O(1)。link 最多 4 SPLIT + 4 JOIN，cut
//! 最多 5 SPLIT + 1 JOIN；这是最坏 O(log N)，不是摊还界。
//!
//! 粗保守的源码步数证书：以一次 Node 读/写为一个 token 访问，单层 JOIN
//! （含双旋）不超过 128 次；SPLIT 的脊和各 JOIN 脊高度差总和不超过 4(h+1)，
//! 因而 SPLIT 不超过 1024(h+1) 次访问。每次公开 link/cut 可统一以
//! `8192 * (MAX_HEIGHT + 1)` 次 token 访问作上界；批量成功/失败恢复至多每项
//! 两次 cut 和两次 link，加线性固定记录扫描。不将此宽松界解释为性能测量。

use core::{cell::UnsafeCell, marker::PhantomPinned, ptr::NonNull};

/// size 使用 usize 时 AVL 可表示的最大高度，直接由最小节点递推导出。
pub const MAX_HEIGHT: usize = max_height();

const fn max_height() -> usize {
    let (mut previous, mut current, mut height) = (0usize, 1usize, 1usize);
    loop {
        let Some(sum) = previous.checked_add(current) else {
            return height;
        };
        let Some(next) = sum.checked_add(1) else {
            return height;
        };
        previous = current;
        current = next;
        height += 1;
    }
}

type Link = Option<Token>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Token(NonNull<Node>);

#[derive(Clone, Copy)]
struct Node {
    left: Link,
    right: Link,
    parent: Link,
    height: usize,
    size: usize,
}

impl Node {
    const SINGLE: Self = Self {
        left: None,
        right: None,
        parent: None,
        height: 1,
        size: 1,
    };
}

struct State {
    parent: Option<DomainId>,
    controlled: bool,
}

/// 可直接内嵌在稳定对象中的全部图存储；三 token 总是预付，不单独分配。
///
/// 业务根与索引根不同：controlled=false 的域永久是业务根，但仍可留有子边。
/// 字段私有；除 new 外的一切访问均经过下述带外部同步合同的 unsafe API。
pub struct Domain {
    state: UnsafeCell<State>,
    marker: UnsafeCell<Node>,
    forward: UnsafeCell<Node>,
    backward: UnsafeCell<Node>,
    _pinned: PhantomPinned,
}

// SAFETY: safe API 只构造未连通值；读写均为 unsafe，要求统一外部排他锁。
// 首次使用后的地址稳定及销毁前单点要求由该 unsafe 合同负责，不依赖线程身份。
unsafe impl Send for Domain {}
// SAFETY: 同上；共享引用不会提供未经外部同步的 safe 字段访问。
unsafe impl Sync for Domain {}

impl Domain {
    /// controlled=true 是尚受唯一 gate 控制的域；false 为永久独立业务根。
    pub const fn new(controlled: bool) -> Self {
        Self {
            state: UnsafeCell::new(State {
                parent: None,
                controlled,
            }),
            marker: UnsafeCell::new(Node::SINGLE),
            forward: UnsafeCell::new(Node::SINGLE),
            backward: UnsafeCell::new(Node::SINGLE),
            _pinned: PhantomPinned,
        }
    }
}

impl Drop for Domain {
    fn drop(&mut self) {
        debug_assert!(
            self.state.get_mut().parent.is_none(),
            "Domain still has a custody parent"
        );
        for node in [&mut self.marker, &mut self.forward, &mut self.backward] {
            let node = node.get_mut();
            debug_assert!(
                node.left.is_none() && node.right.is_none() && node.parent.is_none(),
                "Domain storage dropped before isolation"
            );
        }
    }
}

/// 可复制的非拥有域身份，供上层固定事务记录使用；不是存储 pin。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DomainId(NonNull<Domain>);

// SAFETY: 身份只有比较与指针输出是 safe；解引用仅在外部锁/pin 的 unsafe 合同下。
unsafe impl Send for DomainId {}
// SAFETY: 与 Send 相同，身份自身没有可被 safe API 修改的内容。
unsafe impl Sync for DomainId {}

impl DomainId {
    /// 在稳定位置取得身份，不注册、不写自指针。
    ///
    /// # Safety
    /// 后续所有使用期间地址必须不变；有 incident edge 时不得释放域。身份不延长
    /// 存储寿命，保存身份的回滚记录也必须由调用者保证在其使用前存储仍存活。
    pub unsafe fn new(domain: &Domain) -> Self {
        Self(NonNull::from(domain))
    }

    /// 仅输出地址，不能据此绕过外部同步或存储生命周期合同。
    pub fn as_ptr(self) -> *const Domain {
        self.0.as_ptr()
    }

    fn domain<'a>(self) -> &'a Domain {
        // SAFETY: 仅由本模块 unsafe 入口使用；入口要求整个操作期间域仍存活。
        // 该引用不向调用者逸出，也不用于延长真实存储生命期。
        unsafe { self.0.as_ref() }
    }

    fn state(self) -> State {
        // SAFETY: unsafe 入口要求统一排他锁；只复制纯值，不产生可逸出的可变引用。
        unsafe {
            let state = &*self.domain().state.get();
            State {
                parent: state.parent,
                controlled: state.controlled,
            }
        }
    }

    fn set_state(self, parent: Option<DomainId>, controlled: bool) {
        // SAFETY: 同一外部排他锁保证唯一写入；没有跨该写入保留 State 引用。
        unsafe {
            self.domain()
                .state
                .get()
                .write(State { parent, controlled });
        }
    }

    fn marker(self) -> Token {
        Token(NonNull::new(self.domain().marker.get()).expect("UnsafeCell address is non-null"))
    }
    fn forward(self) -> Token {
        Token(NonNull::new(self.domain().forward.get()).expect("UnsafeCell address is non-null"))
    }
    fn backward(self) -> Token {
        Token(NonNull::new(self.domain().backward.get()).expect("UnsafeCell address is non-null"))
    }
}

/// link 的所有失败均在任何写入之前返回，原森林不变。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkError {
    /// child 已有业务父边，必须先 cut。
    NotRoot,
    /// child 或 parent 已经永久转为独立业务根。
    Inactive,
    /// 同点或同一连通分量，加入边将形成环。
    Cycle,
    /// 合并后的 token 数无法用 usize 表示。
    Overflow,
}

/// 查询唯一业务父边；不走业务祖先链。
///
/// # Safety
/// 必须持有统一外部排他锁，id 及整个可达分量的存储必须稳定且存活；见模块合同。
pub unsafe fn parent(id: DomainId) -> Option<DomainId> {
    id.state().parent
}

/// 查询是否仍受 gate 控制，与 make_root 及图操作使用同一锁。
///
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同。
pub unsafe fn is_controlled(id: DomainId) -> bool {
    id.state().controlled
}

/// 连通查询只沿 AVL 父指针，最坏 O(log N)。同一单点与自身连通。
///
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同，适用于两个域。
pub unsafe fn connected(a: DomainId, b: DomainId) -> bool {
    root(a.marker()) == root(b.marker())
}

/// 是否已无 incident edge，允许最终销毁存储；不是“没有业务父边”的同义词。
///
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同。
pub unsafe fn is_isolated(id: DomainId) -> bool {
    id.state().parent.is_none()
        && [id.marker(), id.forward(), id.backward()]
            .into_iter()
            .all(|token| {
                let n = token.read();
                n.left.is_none() && n.right.is_none() && n.parent.is_none()
            })
}

/// 切除父边并返回其原身份；无父边时不变。下属域整树保留，无需扫描。
///
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同；返回身份不拥有旧父存储。
pub unsafe fn cut(child: DomainId) -> Option<DomainId> {
    cut_impl(child)
}

fn cut_impl(child: DomainId) -> Option<DomainId> {
    let state = child.state();
    let old = state.parent?;
    // 以 parent->child token 开始，环形 tour 必为 [forward, child tour, backward, parent tour]。
    let tour = reroot(child.forward());
    let (_, after_forward) = split(Some(tour), 1);
    let (_, backward_rank) = position(child.backward());
    let (_, suffix) = split(after_forward, backward_rank);
    let _ = split(suffix, 1);
    child.set_state(None, state.controlled);
    Some(old)
}

/// 将无父边的 child 连到 parent。不会隐式 cut，也不会把永久根重新受控。
///
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同，适用于两个域及其分量。
pub unsafe fn link(child: DomainId, parent: DomainId) -> Result<(), LinkError> {
    let state = child.state();
    if state.parent.is_some() {
        return Err(LinkError::NotRoot);
    }
    if !state.controlled || !parent.state().controlled {
        return Err(LinkError::Inactive);
    }
    link_topology(child, parent)
}

// 只恢复同锁保存的合法旧边时可略过 controlled 政策：永久根允许保留既存子边。
// 普通业务准入必须经过公开 link 或 transfer，不能调用这条私有路径新增依赖。
fn link_topology(child: DomainId, parent: DomainId) -> Result<(), LinkError> {
    let child_root = root(child.marker());
    let parent_root = root(parent.marker());
    if child_root == parent_root {
        return Err(LinkError::Cycle);
    }
    if joined_size(child_root.read().size, parent_root.read().size).is_none() {
        return Err(LinkError::Overflow);
    }
    let child_tour = reroot(child.marker());
    let parent_tour = reroot(parent.marker());
    let joined = join(Some(parent_tour), child.forward(), Some(child_tour));
    let _ = join(Some(joined), child.backward(), None);
    child.set_state(Some(parent), true);
    Ok(())
}

/// 先切全批旧边，再向同一目标迁移；失败精确恢复全部旧父边。
///
/// None 表示非 gate 能力；永久根 child 被跳过。target 为 None 或已经永久成为
/// 业务根时仅 cut。固定 K 条记录在栈上预付，无分配，不拥有/释放任何存储引用。
/// 成功返回后调用者仍须在同一外部锁内提交实际能力位置，不允许先放锁后安装。
///
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同适用于所有输入与可达分量。
/// children 中非 None 身份必须唯一（由仿射 gate 保证），长度不得超过 K。
/// 外部锁必须覆盖真实提交/失败收束，期间不可让任何记录中的域或旧父失去 pin。
pub unsafe fn transfer<const K: usize>(
    children: &[Option<DomainId>],
    target: Option<DomainId>,
) -> Result<(), LinkError> {
    assert!(
        children.len() <= K,
        "Transfer batch exceeds prepaid record capacity"
    );
    let mut records = [None; K];
    let mut count = 0;
    let target = target.filter(|id| id.state().controlled);
    for &child in children.iter().flatten() {
        if child.state().controlled {
            let old = cut_impl(child);
            records[count] = Some((child, old));
            count += 1;
        }
    }
    if let Some(target) = target {
        for &(child, _) in records[..count].iter().flatten() {
            if let Err(error) = link_topology(child, target) {
                // 无并发修改且已切完新增边，恢复的只是原有无环图的子集。
                // 原图 size 可表示，故 Cycle/Overflow 均结构性不可达；恢复无需分配。
                for &(child, _) in records[..count].iter().flatten().rev() {
                    cut_impl(child);
                }
                for &(child, old) in records[..count].iter().flatten().rev() {
                    if let Some(old) = old {
                        link_topology(child, old)
                            .expect("Saved forest edges must restore without failure");
                    }
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

/// 同锁永久改为业务根并切父边，返回旧父身份。重复调用无副作用。
///
/// 保留旧子边至自然移动/释放，不扫描子树；之后 link 拒绝以该域为任一端。
/// # Safety
/// 与 [`parent`] 相同的同步、稳定地址及存储合同。
pub unsafe fn make_root(id: DomainId) -> Option<DomainId> {
    let old = cut_impl(id);
    id.set_state(None, false);
    old
}

impl Token {
    fn read(self) -> Node {
        // SAFETY: Token 只来自已 pin 域或其可达索引。unsafe 入口要求存储存活且同锁，
        // Node 为 Copy；读取不创建跨写入的引用。
        unsafe { self.0.as_ptr().read() }
    }
    fn write(self, node: Node) {
        // SAFETY: 与 read 相同；底层在 UnsafeCell 内，外部锁保证唯一写入。
        unsafe {
            self.0.as_ptr().write(node);
        }
    }
    fn set_parent(self, parent: Link) {
        let mut n = self.read();
        n.parent = parent;
        self.write(n);
    }
}

fn joined_size(a: usize, b: usize) -> Option<usize> {
    a.checked_add(b).and_then(|n| n.checked_add(2))
}

fn height(t: Link) -> usize {
    t.map_or(0, |t| t.read().height)
}
fn size(t: Link) -> usize {
    t.map_or(0, |t| t.read().size)
}
fn root(mut t: Token) -> Token {
    while let Some(parent) = t.read().parent {
        t = parent;
    }
    t
}

fn position(mut t: Token) -> (Token, usize) {
    let mut rank = size(t.read().left);
    while let Some(parent) = t.read().parent {
        let p = parent.read();
        if p.right == Some(t) {
            // 子序列 rank 严格小于整棵已校验 size，不可能溢出。
            rank = rank
                .checked_add(size(p.left))
                .and_then(|n| n.checked_add(1))
                .expect("AVL rank exceeds component size");
        }
        t = parent;
    }
    (t, rank)
}

fn assemble(t: Token, left: Link, right: Link) -> Token {
    // 内部 split/join 的临时序列不大于入口预检后的最终分量，checked 失败结构性不可达。
    let count = size(left)
        .checked_add(size(right))
        .and_then(|n| n.checked_add(1))
        .expect("AVL size exceeds checked component size");
    let h = height(left).max(height(right)) + 1;
    t.write(Node {
        left,
        right,
        parent: None,
        height: h,
        size: count,
    });
    if let Some(left) = left {
        left.set_parent(Some(t));
    }
    if let Some(right) = right {
        right.set_parent(Some(t));
    }
    t
}

fn rotate_left(t: Token) -> Token {
    let n = t.read();
    // 调用者已证明右子树至少高 1，故必存在。
    let r = n.right.expect("AVL left rotation requires right child");
    let rn = r.read();
    assemble(t, n.left, rn.left);
    assemble(r, Some(t), rn.right)
}

fn rotate_right(t: Token) -> Token {
    let n = t.read();
    // 调用者已证明左子树至少高 1，故必存在。
    let l = n.left.expect("AVL right rotation requires left child");
    let ln = l.read();
    assemble(t, ln.right, n.right);
    assemble(l, ln.left, Some(t))
}

fn balance(t: Token) -> Token {
    let n = t.read();
    if height(n.left) > height(n.right) + 1 {
        // 高度差证明该子树存在；JOIN 回程最多失衡 2。
        let l = n.left.expect("AVL imbalance requires left child");
        let ln = l.read();
        if height(ln.right) > height(ln.left) {
            let l = rotate_left(l);
            assemble(t, Some(l), n.right);
        }
        rotate_right(t)
    } else if height(n.right) > height(n.left) + 1 {
        let r = n.right.expect("AVL imbalance requires right child");
        let rn = r.read();
        if height(rn.left) > height(rn.right) {
            let r = rotate_right(r);
            assemble(t, n.left, Some(r));
        }
        rotate_left(t)
    } else {
        t
    }
}

/// AVL JOIN：pivot 不属于 left/right，二者都是平衡序列；结果 parent=None。
fn join(mut left: Link, pivot: Token, mut right: Link) -> Token {
    let mut path = [None; MAX_HEIGHT];
    let mut depth = 0;
    let left_tall = height(left) > height(right) + 1;
    if left_tall {
        while height(left) > height(right) + 1 {
            // 条件证明该子树非空；缓存脊严格缩短，数组由 AVL 最小节点数推导。
            let t = left.expect("Nonempty AVL spine required");
            path[depth] = Some(t);
            depth += 1;
            left = t.read().right;
        }
    } else {
        while height(right) > height(left) + 1 {
            let t = right.expect("Nonempty AVL spine required");
            path[depth] = Some(t);
            depth += 1;
            right = t.read().left;
        }
    }
    let mut result = assemble(pivot, left, right);
    while depth != 0 {
        depth -= 1;
        // 只有刚刚压入的槽位会被弹出。
        let t = path[depth].expect("AVL join path initialized");
        let n = t.read();
        result = if left_tall {
            assemble(t, n.left, Some(result))
        } else {
            assemble(t, Some(result), n.right)
        };
        result = balance(result);
    }
    result
}

/// 按序列前 k 个 token 分裂；回程 JOIN 的高度差望远镜求和，最坏 O(h)。
fn split(mut tree: Link, mut k: usize) -> (Link, Link) {
    let mut path = [None; MAX_HEIGHT];
    let mut went_left = [false; MAX_HEIGHT];
    let mut depth = 0;
    debug_assert!(k <= size(tree), "AVL split position exceeds size");
    while let Some(t) = tree {
        let n = t.read();
        let left_size = size(n.left);
        path[depth] = Some(t);
        went_left[depth] = k <= left_size;
        tree = if went_left[depth] {
            n.left
        } else {
            k -= left_size + 1;
            n.right
        };
        depth += 1;
    }
    let (mut a, mut b) = (None, None);
    while depth != 0 {
        depth -= 1;
        // 与 JOIN 相同，槽位由严格下降的 AVL 查找路径初始化。
        let t = path[depth].expect("AVL split path initialized");
        let n = t.read();
        t.write(Node::SINGLE);
        // 不触碰路径方向的旧 child：它可能已被回程 JOIN 移为另一结果的内部节点。
        if went_left[depth] {
            if let Some(other) = n.right {
                other.set_parent(None);
            }
            b = Some(join(b, t, n.right));
        } else {
            if let Some(other) = n.left {
                other.set_parent(None);
            }
            a = Some(join(n.left, t, a));
        }
    }
    (a, b)
}

fn concat(left: Link, right: Link) -> Link {
    match (left, right) {
        (None, other) | (other, None) => other,
        (Some(l), Some(r)) => {
            let (rest, pivot) = split(Some(l), l.read().size - 1);
            // split 最后一个 token 必然产生单点。
            Some(join(
                rest,
                pivot.expect("Nonempty AVL suffix required"),
                Some(r),
            ))
        }
    }
}

fn reroot(token: Token) -> Token {
    let (tree, rank) = position(token);
    if rank == 0 {
        return tree;
    }
    let (a, b) = split(Some(tree), rank);
    // 输入序列含 token，旋转后必然非空。
    concat(b, a).expect("Euler tour contains its anchor")
}

#[cfg(test)]
mod tests;
