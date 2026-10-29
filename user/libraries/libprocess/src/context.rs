//! Process-lifetime startup context and one-shot initial grant ownership.

use alloc::vec::Vec;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandleDescription, ObjectKind, Rights},
    proc::Pid,
    startup::EntryThreadDescriptor,
};
use libkernel::{ipc::capability::Capability, startup::StartupInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    InvalidGrant(SystemCallError),
}

/// The single affine source proof created by the platform entry.
#[derive(Debug)]
pub struct StartupSeed {
    input: StartupInput,
}

impl StartupSeed {
    /// # Safety
    ///
    /// `input` must be the value returned by the current process platform
    /// entry. No other caller may construct a seed.
    pub unsafe fn from_input(input: StartupInput) -> Self {
        Self { input }
    }
}

#[derive(Clone, Copy)]
pub struct StartupPayload {
    bytes: *const u8,
    len: usize,
}

unsafe impl Send for StartupPayload {}
unsafe impl Sync for StartupPayload {}

impl StartupPayload {
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: the StartupSeed source keeps the validated mapping alive for
        // the process lifetime; the returned slice is tied to this borrow.
        unsafe { core::slice::from_raw_parts(self.bytes, self.len) }
    }

    /// 进程生命期内的 payload 视图；StartupSeed 来源保证映射不被释放。
    pub fn as_static_bytes(&self) -> &'static [u8] {
        // SAFETY: 同 as_bytes；启动块映射在进程生命期内有效且只读。
        unsafe { core::slice::from_raw_parts(self.bytes, self.len) }
    }
}

/// Provenance supplied by the creator of the current process.
///
/// The private fields prevent ordinary callers from fabricating startup
/// identity. The view is copyable because it grants no kernel authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentProcessInfo {
    pid: Pid,
    parent_pid: Pid,
}

impl CurrentProcessInfo {
    pub(crate) const fn from_parts(pid: Pid, parent_pid: Pid) -> Self {
        Self { pid, parent_pid }
    }

    fn from_header(header: &erhino_shared::startup::StartupBlockHeader) -> Self {
        Self::from_parts(header.pid, header.parent_pid)
    }

    pub const fn pid(self) -> Pid {
        self.pid
    }

    pub const fn parent_pid(self) -> Pid {
        self.parent_pid
    }
}

pub struct InitialGrants {
    slots: Vec<Option<InitialGrant>>,
}

struct InitialGrant {
    kind: ObjectKind,
    capability: Capability,
}

impl InitialGrants {
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 尚由应用拥有的实际能力数；不含已由 runtime 采用的入口线程控制槽。
    /// 原始索引保持不变，完整启动数组的几何校验仍使用 len。
    pub fn remaining(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn take(&mut self, index: usize) -> Option<Capability> {
        self.slots
            .get_mut(index)
            .and_then(Option::take)
            .map(|grant| grant.capability)
    }

    pub fn try_adopt(&mut self, kind: ObjectKind) -> Option<Capability> {
        self.slots
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|grant| grant.kind == kind))
            .and_then(Option::take)
            .map(|grant| grant.capability)
    }
}

pub struct StartupContext {
    process: CurrentProcessInfo,
    payload: StartupPayload,
    grants: InitialGrants,
}

/// Result of adopting the startup block: the entry-thread descriptor and its
/// Control go to the runtime, the context goes to the application. Reaching
/// this value requires an unsafe `StartupSeed`, so safe code never observes
/// the entry descriptor.
pub struct StartupAdoption {
    pub entry: EntryThreadDescriptor,
    pub entry_control: Capability,
    pub context: StartupContext,
}

impl StartupContext {
    /// Verify every entry before consuming the affine StartupSeed.
    pub fn adopt(seed: StartupSeed) -> Result<StartupAdoption, (StartupSeed, ContextError)> {
        Self::adopt_with_query(seed, libkernel::ipc::object::query)
    }

    fn adopt_with_query(
        seed: StartupSeed,
        mut query: impl FnMut(
            Handle,
        )
            -> Result<erhino_shared::object::HandleDescription, SystemCallError>,
    ) -> Result<StartupAdoption, (StartupSeed, ContextError)> {
        let count = seed.input.grant_count();
        let mut handles = Vec::new();
        if handles.try_reserve_exact(count).is_err() {
            return Err((
                seed,
                ContextError::InvalidGrant(SystemCallError::OutOfMemory),
            ));
        }
        let entry = seed.input.entry_thread();
        for index in 0..count {
            let Some(handle) = seed.input.handle(index) else {
                return Err((
                    seed,
                    ContextError::InvalidGrant(SystemCallError::IllegalArgument),
                ));
            };
            if !handle.is_valid() {
                return Err((
                    seed,
                    ContextError::InvalidGrant(SystemCallError::IllegalArgument),
                ));
            }
            handles.push(handle);
        }
        let mut sorted_handles = Vec::new();
        if sorted_handles.try_reserve_exact(count).is_err() {
            return Err((
                seed,
                ContextError::InvalidGrant(SystemCallError::OutOfMemory),
            ));
        }
        sorted_handles.extend_from_slice(&handles);
        sorted_handles.sort_unstable_by_key(|handle| handle.raw());
        if has_duplicate_handles(&sorted_handles) {
            return Err((
                seed,
                ContextError::InvalidGrant(SystemCallError::IllegalArgument),
            ));
        }
        let control_index = entry.control_index as usize;
        let mut slots = Vec::new();
        if slots.try_reserve_exact(count).is_err() {
            return Err((
                seed,
                ContextError::InvalidGrant(SystemCallError::OutOfMemory),
            ));
        }
        let descriptions = match query_descriptions(&handles, &mut query) {
            Ok(descriptions) => descriptions,
            Err(error) => return Err((seed, ContextError::InvalidGrant(error))),
        };
        if let Err(error) = validate_descriptions(&descriptions, control_index) {
            return Err((seed, ContextError::InvalidGrant(error)));
        }

        // All fallible work is complete. Capability construction only installs
        // the already-validated close responsibilities into the affine table.
        let mut entry_control = None;
        for (index, (handle, description)) in handles.into_iter().zip(descriptions).enumerate() {
            if index == control_index {
                // SAFETY: 已验证这是唯一的 WAIT ThreadControl 观察 owner。
                entry_control = Some(unsafe { Capability::from_raw(handle) });
                // The slot stays empty so grant indices keep launcher order.
                slots.push(None);
                continue;
            }
            let kind =
                adoptable_kind(description.kind).expect("validated startup kind is not adoptable");
            // SAFETY: the platform StartupSeed proves unique ownership of
            // every validated non-mapping entry.
            slots.push(Some(InitialGrant {
                kind,
                capability: unsafe { Capability::from_raw(handle) },
            }));
        }
        let payload = seed.input.payload();
        let context = StartupContext {
            process: CurrentProcessInfo::from_header(&seed.input.header()),
            payload: StartupPayload {
                bytes: payload.as_ptr(),
                len: payload.len(),
            },
            grants: InitialGrants { slots },
        };
        Ok(StartupAdoption {
            entry,
            entry_control: entry_control
                .expect("validate_descriptions proved the entry control index"),
            context,
        })
    }

    pub fn into_parts(self) -> (CurrentProcessInfo, StartupPayload, InitialGrants) {
        (self.process, self.payload, self.grants)
    }
}

fn adoptable_kind(kind: u32) -> Option<ObjectKind> {
    Some(match kind {
        value if value == ObjectKind::Job as u32 => ObjectKind::Job,
        value if value == ObjectKind::MemoryPool as u32 => ObjectKind::MemoryPool,
        value if value == ObjectKind::MemoryObject as u32 => ObjectKind::MemoryObject,
        value if value == ObjectKind::ProcessBuilder as u32 => ObjectKind::ProcessBuilder,
        value if value == ObjectKind::ProcessControl as u32 => ObjectKind::ProcessControl,
        value if value == ObjectKind::ThreadControl as u32 => ObjectKind::ThreadControl,
        value if value == ObjectKind::MailboxReceiver as u32 => ObjectKind::MailboxReceiver,
        value if value == ObjectKind::MailboxSender as u32 => ObjectKind::MailboxSender,
        value if value == ObjectKind::MailboxOnceSender as u32 => ObjectKind::MailboxOnceSender,
        value if value == ObjectKind::NotificationOwner as u32 => ObjectKind::NotificationOwner,
        value if value == ObjectKind::NotificationSignaler as u32 => {
            ObjectKind::NotificationSignaler
        }
        value if value == ObjectKind::NotificationObserver as u32 => {
            ObjectKind::NotificationObserver
        }
        value if value == ObjectKind::TunnelInvitation as u32 => ObjectKind::TunnelInvitation,
        value if value == ObjectKind::SystemReset as u32 => ObjectKind::SystemReset,
        value if value == ObjectKind::LifetimeObserver as u32 => ObjectKind::LifetimeObserver,
        value if value == ObjectKind::Delivery as u32 => ObjectKind::Delivery,
        value if value == ObjectKind::WaitSet as u32 => ObjectKind::WaitSet,
        value if value == ObjectKind::ReplyObserver as u32 => ObjectKind::ReplyObserver,
        value if value == ObjectKind::MmioControl as u32 => ObjectKind::MmioControl,
        value if value == ObjectKind::MmioUse as u32 => ObjectKind::MmioUse,
        value if value == ObjectKind::IrqControl as u32 => ObjectKind::IrqControl,
        value if value == ObjectKind::IrqUse as u32 => ObjectKind::IrqUse,
        value if value == ObjectKind::DmaDomainControl as u32 => ObjectKind::DmaDomainControl,
        value if value == ObjectKind::DmaUse as u32 => ObjectKind::DmaUse,
        value if value == ObjectKind::DmaMapping as u32 => ObjectKind::DmaMapping,
        _ => return None,
    })
}

fn has_duplicate_handles(handles: &[Handle]) -> bool {
    handles.windows(2).any(|pair| pair[0] == pair[1])
}

fn query_descriptions(
    handles: &[Handle],
    query: &mut impl FnMut(Handle) -> Result<HandleDescription, SystemCallError>,
) -> Result<Vec<HandleDescription>, SystemCallError> {
    let mut descriptions = Vec::new();
    descriptions
        .try_reserve_exact(handles.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for handle in handles.iter().copied() {
        descriptions.push(query(handle)?);
    }
    Ok(descriptions)
}

fn validate_descriptions(
    descriptions: &[HandleDescription],
    control_index: usize,
) -> Result<(), SystemCallError> {
    for (index, description) in descriptions.iter().enumerate() {
        if !description.rights.is_known()
            || description.reserved0 != 0
            || adoptable_kind(description.kind).is_none()
        {
            return Err(SystemCallError::WrongObjectType);
        }
        if index == control_index
            && (description.kind != ObjectKind::ThreadControl as u32
                || !description.rights.contains(Rights::WAIT))
        {
            return Err(SystemCallError::WrongObjectType);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use erhino_shared::{
        call::SystemCallError,
        object::{Handle, HandleDescription, ObjectKind, Rights},
        startup::{EntryThreadDescriptor, build_startup_block_with_entry},
    };

    use super::{
        StartupContext, StartupSeed, adoptable_kind, has_duplicate_handles, query_descriptions,
        validate_descriptions,
    };

    #[test]
    fn adopts_65_grants_from_unaligned_startup_block_after_full_query() {
        let handles: Vec<_> = (1..=65)
            .map(|slot| erhino_shared::object::Handle::from_parts(slot, 1))
            .collect();
        let block = build_startup_block_with_entry(
            7,
            3,
            &handles,
            EntryThreadDescriptor {
                kind: erhino_shared::startup::ENTRY_THREAD_SECTION_KIND,
                version: erhino_shared::startup::ENTRY_THREAD_SECTION_VERSION,
                section_len: core::mem::size_of::<EntryThreadDescriptor>() as u32,
                control_index: 64,
                reserved: 0,
                stack_reservation_start: 0x10000,
                stack_reservation_len: 0x5000,
                usable_start: 0x11000,
                usable_len: 0x3000,
            },
            b"payload",
        )
        .unwrap();
        let mut unaligned = Vec::with_capacity(block.len() + 1);
        unaligned.push(0);
        unaligned.extend_from_slice(&block);
        let input = unsafe { libkernel::startup::init(unaligned.as_ptr().add(1), block.len()) };
        // SAFETY: input came from the unique test invocation of platform init.
        let seed = unsafe { StartupSeed::from_input(input) };
        let query_count = AtomicUsize::new(0);
        let admission = StartupContext::adopt_with_query(seed, |handle| {
            query_count.fetch_add(1, Ordering::Relaxed);
            let kind = if handle == handles[64] {
                ObjectKind::ThreadControl
            } else if handle == handles[0] {
                ObjectKind::MailboxSender
            } else {
                ObjectKind::MemoryObject
            };
            Ok(HandleDescription {
                object_id: handle.raw(),
                related_object_id: 0,
                kind: kind as u32,
                reserved0: 0,
                rights: if handle == handles[64] {
                    erhino_shared::object::Rights::WAIT
                } else {
                    erhino_shared::object::Rights::READ
                },
                badge: 0,
                reserved: 0,
            })
        })
        .unwrap();

        assert_eq!(query_count.load(Ordering::Relaxed), 65);
        // 入口 Control 已由采用结果单独交付，不再留在应用 grants 中。
        core::mem::forget(admission.entry_control);
        let (process, payload, mut grants) = admission.context.into_parts();
        assert_eq!(process.pid(), 7);
        assert_eq!(process.parent_pid(), 3);
        assert_eq!(payload.as_bytes(), b"payload");
        assert_eq!(grants.len(), 65);
        assert_eq!(grants.remaining(), 64);
        assert!(
            grants.take(64).is_none(),
            "entry control must not be exposed to the application"
        );
        let sender = grants
            .try_adopt(ObjectKind::MailboxSender)
            .expect("kind adoption must consume the matching grant");
        assert!(grants.try_adopt(ObjectKind::MailboxSender).is_none());
        assert_eq!(grants.remaining(), 63);
        core::mem::forget(sender);
        core::mem::forget(grants);
    }

    #[test]
    fn startup_adoption_whitelist_rejects_tunnel_endpoint() {
        assert_eq!(adoptable_kind(ObjectKind::TunnelEndpoint as u32), None);
        assert_eq!(adoptable_kind(ObjectKind::ThreadStartTicket as u32), None);
    }

    #[test]
    fn startup_adoption_accepts_mmio_authority_roles() {
        assert_eq!(
            adoptable_kind(ObjectKind::MmioControl as u32),
            Some(ObjectKind::MmioControl)
        );
        assert_eq!(
            adoptable_kind(ObjectKind::MmioUse as u32),
            Some(ObjectKind::MmioUse)
        );
    }

    #[test]
    fn startup_adoption_rejects_duplicate_and_invalid_kinds_before_owners() {
        let handles = [Handle::from_parts(1, 1), Handle::from_parts(1, 1)];
        assert!(has_duplicate_handles(&handles));

        let endpoint = HandleDescription {
            object_id: 1,
            related_object_id: 0,
            kind: ObjectKind::TunnelEndpoint as u32,
            reserved0: 0,
            rights: Rights::READ,
            badge: 0,
            reserved: 0,
        };
        assert_eq!(
            validate_descriptions(&[endpoint], usize::MAX),
            Err(SystemCallError::WrongObjectType)
        );
    }

    #[test]
    fn late_startup_query_failure_returns_before_owner_table_construction() {
        let handles = [
            Handle::from_parts(1, 1),
            Handle::from_parts(2, 1),
            Handle::from_parts(3, 1),
        ];
        let query_count = AtomicUsize::new(0);
        let result = query_descriptions(&handles, &mut |handle| {
            query_count.fetch_add(1, Ordering::Relaxed);
            if handle == handles[2] {
                Err(SystemCallError::ObjectClosed)
            } else {
                Ok(HandleDescription {
                    object_id: handle.raw(),
                    related_object_id: 0,
                    kind: ObjectKind::MemoryObject as u32,
                    reserved0: 0,
                    rights: Rights::READ,
                    badge: 0,
                    reserved: 0,
                })
            }
        });
        assert_eq!(result, Err(SystemCallError::ObjectClosed));
        assert_eq!(query_count.load(Ordering::Relaxed), 3);
    }
}
