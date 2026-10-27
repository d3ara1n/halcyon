//! Watch 的内核通知投影；领域序号仍由 Registry 唯一拥有。
use super::*;
use libdevice::watch::Info;
use libkernel::ipc::notification::{
    self, NotificationObserver, NotificationOwner, NotificationSignaler,
};

pub(crate) struct Signal {
    owner: NotificationOwner,
    signaler: NotificationSignaler,
    signaled: u64,
}
impl Signal {
    pub fn create() -> Result<(Self, NotificationObserver), Error> {
        let pair =
            notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)?;
        let observer = pair
            .owner
            .mint_observer(Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE)?;
        Ok((
            Self {
                owner: pair.owner,
                signaler: pair.peer,
                signaled: 0,
            },
            observer,
        ))
    }
    pub fn update(&mut self, info: Info) -> Result<(), SystemCallError> {
        if info.pending == 0 {
            match self.owner.take(u64::MAX) {
                Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                Err(error) => return Err(error),
            }
            self.signaled = 0;
        } else if self.signaled != info.sequence {
            self.signaler.signal(1)?;
            self.signaled = info.sequence;
        }
        Ok(())
    }
}
impl World {
    pub(super) fn signal_watch(&mut self, context: u64) -> Result<Info, Error> {
        let info = self.registry.query_watch(context)?;
        let role = self.roles.get_mut(context).ok_or(Error::Permission)?;
        let RoleKind::Watch { signal, .. } = &mut role.kind else {
            return Err(Error::Permission);
        };
        signal.update(info)?;
        Ok(info)
    }
}
