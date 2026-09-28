//! Process-lifetime startup context and one-shot initial grant ownership.

use alloc::vec::Vec;

use erhino_shared::{
    call::SystemCallError,
    object::Handle,
    proc::Pid,
    startup::EntryThreadDescriptor,
};
use libkernel::{ipc::capability::Capability, startup};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    NotInitialized,
    MissingGrant,
    InvalidGrant(SystemCallError),
}

pub struct InitialGrants {
    slots: Vec<Option<Capability>>,
}

impl InitialGrants {
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn take(&mut self, index: usize) -> Option<Capability> {
        self.slots.get_mut(index).and_then(Option::take)
    }

    pub fn duplicate(&self, index: usize) -> Result<Option<Capability>, SystemCallError> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map(Capability::duplicate_all)
            .transpose()
    }
}

pub struct StartupContext {
    pid: Pid,
    parent_pid: Pid,
    payload: &'static [u8],
    entry: EntryThreadDescriptor,
    grants: InitialGrants,
}

impl StartupContext {
    /// Claim every StartupBlock grant exactly once and make the resulting
    /// capability owners available by index.
    ///
    /// # Safety
    ///
    /// The runtime must call this only after `libkernel::startup::init` has
    /// validated and published the process-lifetime StartupBlock.
    pub unsafe fn adopt() -> Result<Self, ContextError> {
        let count = startup::grant_count();
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(count)
            .map_err(|_| ContextError::InvalidGrant(SystemCallError::OutOfMemory))?;
        for index in 0..count {
            let handle = startup::startup_handle(index).ok_or(ContextError::MissingGrant)?;
            // SAFETY: StartupBlock grants are unique entries installed in this
            // process; claiming each index transfers its close responsibility.
            let capability = unsafe { Capability::from_raw(handle) };
            slots.push(Some(capability));
        }
        Ok(Self {
            pid: startup::pid(),
            parent_pid: startup::parent_pid(),
            payload: startup::startup_payload(),
            entry: startup::entry_thread(),
            grants: InitialGrants { slots },
        })
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn parent_pid(&self) -> Pid {
        self.parent_pid
    }

    pub fn payload(&self) -> &'static [u8] {
        self.payload
    }

    pub fn entry(&self) -> EntryThreadDescriptor {
        self.entry
    }

    pub fn grants(&mut self) -> &mut InitialGrants {
        &mut self.grants
    }

    pub fn take_handle(&mut self, index: usize) -> Option<Handle> {
        self.grants.take(index).map(Capability::into_raw)
    }

    pub fn take_or_duplicate_handle(&mut self, index: usize) -> Option<Handle> {
        if let Ok(Some(capability)) = self.grants.duplicate(index) {
            return Some(capability.into_raw());
        }
        self.take_handle(index)
    }

    pub fn duplicate_handle(&self, index: usize) -> Result<Option<Handle>, SystemCallError> {
        Ok(self.grants.duplicate(index)?.map(Capability::into_raw))
    }
}
