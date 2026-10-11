//! 客户端本地入口绑定；前缀不创建目录，也不产生真实授权。

use crate::{LocalError, view::validate_absolute};
use alloc::{string::String, sync::Arc, vec::Vec};
use libkernel::ipc::message::MailboxSender;

#[derive(Debug)]
struct Binding {
    prefix: String,
    entry: Arc<MailboxSender>,
}

#[derive(Debug, Default)]
pub struct Namespace {
    bindings: Vec<Binding>,
}

/// 本地绑定失败完整返还输入 owner，不隐式丢弃调用方的入口。
#[derive(Debug)]
pub struct BindFailure {
    pub cause: LocalError,
    pub entry: Arc<MailboxSender>,
}

impl Namespace {
    pub const fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    pub fn bind(&mut self, prefix: &str, entry: Arc<MailboxSender>) -> Result<(), BindFailure> {
        let prepare = (|| {
            validate_absolute(prefix)?;
            if self.bindings.iter().any(|binding| binding.prefix == prefix) {
                return Err(LocalError::Exists);
            }
            let mut owned = String::new();
            owned
                .try_reserve_exact(prefix.len())
                .map_err(|_| LocalError::Resource)?;
            owned.push_str(prefix);
            self.bindings
                .try_reserve_exact(1)
                .map_err(|_| LocalError::Resource)?;
            Ok(owned)
        })();
        match prepare {
            Ok(prefix) => {
                self.bindings.push(Binding { prefix, entry });
                Ok(())
            }
            Err(cause) => Err(BindFailure { cause, entry }),
        }
    }

    /// 只替换已有精确绑定；缺失返回 NoBinding，旧 owner 交回调用方。
    pub fn replace_binding(
        &mut self,
        prefix: &str,
        entry: Arc<MailboxSender>,
    ) -> Result<Arc<MailboxSender>, BindFailure> {
        if let Err(cause) = validate_absolute(prefix) {
            return Err(BindFailure { cause, entry });
        }
        match self
            .bindings
            .iter_mut()
            .find(|binding| binding.prefix == prefix)
        {
            Some(binding) => Ok(core::mem::replace(&mut binding.entry, entry)),
            None => Err(BindFailure {
                cause: LocalError::NoBinding,
                entry,
            }),
        }
    }

    pub fn unbind(&mut self, prefix: &str) -> Result<Arc<MailboxSender>, LocalError> {
        validate_absolute(prefix)?;
        let index = self
            .bindings
            .iter()
            .position(|binding| binding.prefix == prefix)
            .ok_or(LocalError::NoBinding)?;
        Ok(self.bindings.remove(index).entry)
    }

    pub(crate) fn route<'a>(
        &self,
        path: &'a str,
    ) -> Result<(&str, Arc<MailboxSender>, &'a str), LocalError> {
        validate_absolute(path)?;
        let (index, suffix) = longest_prefix(
            self.bindings.iter().map(|binding| binding.prefix.as_str()),
            path,
        )
        .ok_or(LocalError::NoBinding)?;
        let binding = &self.bindings[index];
        Ok((&binding.prefix, Arc::clone(&binding.entry), &path[suffix..]))
    }
}

/// 返回索引及相对后缀起点；同一个算法用于真实绑定和纯字符串 host 用例。
fn longest_prefix<'a>(
    prefixes: impl IntoIterator<Item = &'a str>,
    path: &str,
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, usize)> = None;
    for (index, prefix) in prefixes.into_iter().enumerate() {
        let suffix = if prefix == "/" {
            if !path.starts_with('/') {
                continue;
            }
            1
        } else if path == prefix {
            path.len()
        } else if path.starts_with(prefix) && path.as_bytes().get(prefix.len()) == Some(&b'/') {
            prefix.len() + 1
        } else {
            continue;
        };
        if best.is_none_or(|(_, _, length)| prefix.len() > length) {
            best = Some((index, suffix, prefix.len()));
        }
    }
    best.map(|(index, suffix, _)| (index, suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_binding_uses_component_boundaries_and_does_not_require_root() {
        // /a 不匹配 /abc；只有应用显式绑定的前缀可用，不构造隐含父目录。
        let prefixes = ["/a", "/a/b", "/雪"];
        assert_eq!(longest_prefix(prefixes, "/a/b/c"), Some((1, 5)));
        assert_eq!(longest_prefix(prefixes, "/a/b"), Some((1, 4)));
        assert_eq!(longest_prefix(prefixes, "/a/x"), Some((0, 3)));
        assert_eq!(longest_prefix(prefixes, "/abc"), None);
        assert_eq!(longest_prefix(prefixes, "/"), None);
        assert_eq!(longest_prefix(["/", "/a"], "/abc"), Some((0, 1)));
        assert_eq!(longest_prefix(prefixes, "/雪/x"), Some((2, 5)));
    }
}
