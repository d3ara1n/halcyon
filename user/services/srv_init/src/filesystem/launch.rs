//! 文件宿主的实际出生、付款和停止；所有 profile 共用此装配出口。
use super::hosting::Bootstrap;
use super::*;
use crate::supervision::Instance;
use crate::supervisor::Await;
use crate::{SUPERVISOR_RIGHTS, SpawnRequest, TopologyNames, init_process_info, root_memory_pool};
use erhino_shared::{
    memory_pool::{METADATA_SLOT_BYTES, MemoryPoolBudget},
    proc::*,
};
use srv_fs::{deployment, startup::Profile};

fn end_after(ns: u64) -> Result<Deadline, &'static str> {
    libkernel::time::snapshot()
        .map_err(|_| "filesystem clock failed")?
        .now_ns
        .checked_add(ns)
        .map(Deadline::at)
        .ok_or("filesystem deadline overflow")
}
impl Host {
    pub(crate) fn launch(
        root: &mut RootSupervisor,
        services: Handle,
        names: &mut TopologyNames,
        image: &[u8],
        profile: Profile,
        authority: Option<MailboxSender>,
    ) -> Result<Self, &'static str> {
        root.check_admission()?;
        let start = end_after(deployment::START_TIMEOUT_NS)?;
        // 同一 ELF 验证器逐步规划，按实际 load runs 计页，不把文件长度当 BSS 大小。
        let mut validation = elf::Validation::new(
            image,
            elf::LoadLimits {
                page_size: PROCESS_PAGE_SIZE as u64,
                image_limit: (PROCESS_USER_TOP - PROCESS_MAIN_STACK_SIZE) as u64,
            },
        )
        .map_err(|_| "filesystem ELF input invalid")?;
        let plan = loop {
            root.turn_and_wait(Some(Await::Turn))
                .map_err(|_| "filesystem layout observation failed")?;
            if libkernel::time::snapshot()
                .map_err(|_| "filesystem clock failed")?
                .now_ns
                >= start.at_ns
            {
                return Err("filesystem layout deadline expired");
            }
            if let Some(plan) = validation.advance().map_err(|_| "filesystem ELF invalid")? {
                break plan;
            }
        };
        let pages = plan
            .runs()
            .iter()
            .try_fold(0u64, |sum, run| {
                sum.checked_add(run.memsz / PROCESS_PAGE_SIZE as u64)
            })
            .and_then(|pages| {
                pages.checked_add(
                    (PROCESS_MAIN_STACK_SIZE / PROCESS_PAGE_SIZE + 1 + deployment::HEAP_PAGES)
                        as u64,
                )
            })
            .ok_or("filesystem page budget overflow")?;
        let pool = root
            .derive_pool(
                root_memory_pool(),
                MemoryPoolBudget {
                    pages,
                    metadata: (deployment::METADATA_BYTES as u64).div_ceil(METADATA_SLOT_BYTES),
                },
                Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::GRANT,
            )
            .map_err(|error| {
                libkernel::debug!("filesystem Pool admission rejected: {:?}", error);
                "filesystem Pool admission failed"
            })?;
        libkernel::debug!(
            "FS host Pool admitted: profile={:?}, image_runs={}, pages={}, metadata_bytes={}",
            profile,
            plan.runs().len(),
            pages,
            deployment::METADATA_BYTES
        );
        drop(plan);
        let bootstrap = root
            .create_mailbox(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT,
            )
            .map_err(|_| "filesystem bootstrap creation failed")?;
        let context = libkernel::ipc::object::query(bootstrap.peer)
            .map_err(|_| "filesystem bootstrap context query failed")?
            .object_id;
        let release = root
            .create_notification(Rights::READ | Rights::WAIT | Rights::GRANT, Rights::SIGNAL)
            .map_err(|_| "filesystem stop notification failed")?;
        let route = root
            .create_mailbox_owner(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
            .map_err(|_| "filesystem management mailbox failed")?;
        let registration = root
            .create_mailbox(
                Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            )
            .map_err(|_| "filesystem registration mailbox failed")?;
        if profile.registry() {
            root.close_control(registration.peer)?;
        }
        let pool_grant = root
            .duplicate(pool, Rights::CREATE | Rights::GRANT)
            .map_err(|_| "filesystem Pool grant failed")?;
        let instance = if profile.registry() {
            Instance::Filesystem
        } else {
            Instance::Secondary
        };
        let bootstrap_owner = Bootstrap::new(
            instance,
            bootstrap.owner,
            release.peer,
            pool,
            context,
            profile,
            start,
        )?;
        root.attach_filesystem(instance, bootstrap_owner)?;
        let grants = [
            HandleGrant {
                handle: bootstrap.peer,
                rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            },
            HandleGrant {
                handle: release.owner,
                rights: Rights::READ | Rights::WAIT,
            },
            HandleGrant {
                handle: route,
                rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
            },
            HandleGrant {
                handle: registration.owner,
                rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
            },
            HandleGrant {
                handle: pool_grant,
                rights: Rights::CREATE,
            },
        ];
        let prepared = root.prepare_process_until(
            SpawnRequest {
                memory_pool: pool,
                parent: init_process_info(),
                job: services,
                image,
                payload: &profile.encode(),
                grants: &grants,
                control_rights: SUPERVISOR_RIGHTS,
            },
            start,
        );
        if prepared.is_ok()
            || matches!(&prepared, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed)
        {
            for grant in grants {
                root.transferred(grant.handle);
            }
        }
        let prepared = prepared.map_err(|failure| {
            libkernel::debug!("filesystem preparation failed: {:?}", failure);
            "filesystem preparation failed"
        })?;
        let publication = (|| -> Result<(), &'static str> {
            if let Some(authority) = authority {
                root.check_admission()?;
                let rights = authority
                    .description()
                    .map_err(|_| "publication authority query failed")?
                    .rights;
                let mut packet =
                    libkernel::ipc::packet::Packet::new(service::AUTHORITY_GRANT_KIND, &[])
                        .map_err(|_| "publication packet failed")?;
                packet
                    .push(authority.into_capability(), rights)
                    .map_err(|_| "publication packet capability failed")?;
                packet
                    .try_send(root.sender(registration.peer), start)
                    .map_err(|_| "publication handoff failed")?;
                root.close_control(registration.peer)?;
            } else if !profile.registry() {
                return Err("secondary publication authority missing");
            }
            Ok(())
        })();
        if let Err(error) = publication {
            root.cancel_prepared(prepared)?;
            return Err(error);
        }
        let pid = root.start_instance(instance, prepared)?;
        names.register_process(
            pid,
            if profile.registry() {
                "bin/srv_fs"
            } else {
                "bin/srv_fs@second"
            },
        );
        let host = root.await_filesystem(instance)?;
        super::hosting::pool_sample(root.filesystem_bootstrap(instance)?.pool, "Ready");
        libkernel::debug!(
            "FS host ready authenticated: profile={:?}, pid={}, context={}",
            profile,
            pid,
            context
        );
        Ok(host)
    }
}
