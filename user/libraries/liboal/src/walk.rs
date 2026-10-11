//! 不解释领域响应的接续工具；帧和入口上下文是单次调用的私有状态。

use crate::{Boundary, FollowBudget, LocalError, Namespace, view};
use alloc::{string::String, sync::Arc, vec::Vec};
use libkernel::ipc::message::MailboxSender;

struct Frame<E> {
    entry: E,
    root: String,
}

/// 泛型只抽象入口句柄，生产与 host 用例使用同一份纯逻辑算法。
struct State<E> {
    frames: Vec<Frame<E>>,
    path: String,
    budget: FollowBudget,
}

enum Advance {
    Local,
    Reroute,
}

impl<E> State<E> {
    fn new(
        root: &str,
        entry: E,
        path: String,
        mut budget: FollowBudget,
    ) -> Result<Self, LocalError> {
        let frame = Self::prepare_frame(root, entry, &mut budget)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(1)
            .map_err(|_| LocalError::Resource)?;
        frames.push(frame);
        Ok(Self {
            frames,
            path,
            budget,
        })
    }

    fn prepare_frame(
        root: &str,
        entry: E,
        budget: &mut FollowBudget,
    ) -> Result<Frame<E>, LocalError> {
        budget.consume_bytes(core::mem::size_of::<Frame<E>>())?;
        Ok(Frame {
            entry,
            root: view::copy_text(root, budget)?,
        })
    }

    fn target(&self) -> Result<(&E, &str), LocalError> {
        let frame = self.frames.last().ok_or(LocalError::Protocol)?;
        let relative = relative_path(&self.path, &frame.root)?;
        Ok((&frame.entry, relative))
    }

    fn follow(&mut self, boundary: Boundary<'_>, entry: Option<E>) -> Result<Advance, LocalError> {
        let frame = self.frames.last().ok_or(LocalError::Protocol)?;
        let relative = relative_path(&self.path, &frame.root)?;
        boundary
            .validate_for(relative)
            .map_err(|_| LocalError::Protocol)?;
        match boundary {
            Boundary::Provider(value) => {
                let entry = entry.ok_or(LocalError::Protocol)?;
                // consumed 已与原 relative 路径完整核对，切片落在 UTF-8 分量末尾。
                let consumed =
                    usize::try_from(value.consumed_bytes).map_err(|_| LocalError::Protocol)?;
                let frame = self.frames.last().ok_or(LocalError::Protocol)?;
                let root =
                    view::expand(&frame.root, &[&relative[..consumed]], &mut self.budget)?.path;
                let frame = Self::prepare_frame(&root, entry, &mut self.budget)?;
                self.frames
                    .try_reserve_exact(1)
                    .map_err(|_| LocalError::Resource)?;
                self.frames.push(frame);
                Ok(Advance::Local)
            }
            Boundary::Link(value) => {
                if entry.is_some() {
                    return Err(LocalError::Protocol);
                }
                let parent =
                    usize::try_from(value.parent_bytes).map_err(|_| LocalError::Protocol)?;
                let frame = self.frames.last().ok_or(LocalError::Protocol)?;
                let base =
                    view::expand(&frame.root, &[&relative[..parent]], &mut self.budget)?.path;
                let absolute = value.target.starts_with('/');
                let expanded = view::expand(
                    if absolute { "/" } else { &base },
                    &[value.target, value.remaining],
                    &mut self.budget,
                )?;
                self.path = expanded.path;
                if absolute {
                    self.frames.clear();
                    return Ok(Advance::Reroute);
                }
                // 曾越过某个入口根就回上层；不按最终字符串把旧入口偷偷重新放回。
                while self
                    .frames
                    .last()
                    .is_some_and(|frame| view::depth(&frame.root) > expanded.retained_depth)
                {
                    self.frames.pop();
                }
                Ok(if self.frames.is_empty() {
                    Advance::Reroute
                } else {
                    Advance::Local
                })
            }
        }
    }

    fn reroute(&mut self, root: &str, entry: E) -> Result<(), LocalError> {
        let frame = Self::prepare_frame(root, entry, &mut self.budget)?;
        self.frames
            .try_reserve_exact(1)
            .map_err(|_| LocalError::Resource)?;
        self.frames.clear();
        self.frames.push(frame);
        Ok(())
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.path.clear();
    }
}

fn relative_path<'a>(path: &'a str, root: &str) -> Result<&'a str, LocalError> {
    let suffix = path.strip_prefix(root).ok_or(LocalError::Protocol)?;
    if root == "/" || suffix.is_empty() {
        return Ok(suffix);
    }
    suffix.strip_prefix('/').ok_or(LocalError::Protocol)
}

enum NamespaceOwner<'a> {
    Borrowed(&'a Namespace),
    Owned(Arc<Namespace>),
}
impl core::ops::Deref for NamespaceOwner<'_> {
    type Target = Namespace;
    fn deref(&self) -> &Namespace {
        match self {
            Self::Borrowed(namespace) => namespace,
            Self::Owned(namespace) => namespace,
        }
    }
}
/// 领域客户端用 target 直接发自己的请求，再把已验证 Boundary 交给 follow。
/// 不产生 Position、ObjectInfo 前置或公共帧；整个调用固定同一个 Namespace。
pub struct Walk<'a> {
    namespace: NamespaceOwner<'a>,
    state: State<Arc<MailboxSender>>,
}

impl<'a> Walk<'a> {
    pub fn new(
        namespace: &'a Namespace,
        path: &str,
        cwd: &str,
        budget: FollowBudget,
    ) -> Result<Self, LocalError> {
        Self::prepare(NamespaceOwner::Borrowed(namespace), path, cwd, budget)
    }
    fn prepare(
        namespace: NamespaceOwner<'a>,
        path: &str,
        cwd: &str,
        mut budget: FollowBudget,
    ) -> Result<Self, LocalError> {
        let path = view::normalize(path, cwd, &mut budget)?;
        let (root, entry, _) = namespace.route(&path)?;
        let state = State::new(root, entry, path, budget)?;
        Ok(Self { namespace, state })
    }

    pub fn target(&self) -> Result<(&Arc<MailboxSender>, &str), LocalError> {
        self.state.target()
    }
    pub fn budget(&self) -> &FollowBudget {
        &self.state.budget
    }
    /// Rename/Copy 的其它阶段也可借用原余额，不创建新的 FollowBudget。
    pub fn budget_mut(&mut self) -> &mut FollowBudget {
        &mut self.state.budget
    }

    /// 结束本次路径接续并释放帧，把同一余额交给 Rename/Copy 的下一个路径阶段。
    pub fn into_budget(self) -> FollowBudget {
        self.state.budget
    }

    pub fn before_call(&mut self, now_ns: u64) -> Result<(), LocalError> {
        let result = self.state.budget.before_call(now_ns);
        if result.is_err() {
            self.state.clear();
        }
        result
    }

    pub fn follow(
        &mut self,
        boundary: Boundary<'_>,
        entry: Option<Arc<MailboxSender>>,
        now_ns: u64,
    ) -> Result<(), LocalError> {
        let result = (|| {
            self.state.budget.check_deadline(now_ns)?;
            match self.state.follow(boundary, entry)? {
                Advance::Local => {}
                Advance::Reroute => {
                    let (root, entry, _) = self.namespace.route(&self.state.path)?;
                    self.state.reroute(root, entry)?;
                }
            }
            Ok(())
        })();
        // 失败即清空所有临时 Arc，合法 typed sender 的叶关闭由 libkernel Drop 保证。
        if result.is_err() {
            self.state.clear();
        }
        result
    }
}

impl Walk<'static> {
    /// 分步任务持有同一不可变 Namespace；复用与借用调用相同的接续状态。
    pub fn owned(
        namespace: Arc<Namespace>,
        path: &str,
        cwd: &str,
        budget: FollowBudget,
    ) -> Result<Self, LocalError> {
        Self::prepare(NamespaceOwner::Owned(namespace), path, cwd, budget)
    }
}

#[cfg(test)]
mod tests;
