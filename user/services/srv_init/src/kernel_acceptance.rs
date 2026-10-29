//! 内核验收 task：init 启动 `test_kernel` 并等其正常完成，再以它为输入执行根监督自检，
//! 最后核其付款域归零、held Pool 自报告后不变。任何失败由 root 账本与 services 收集兜底。
use super::*;
use libkernel::shared::memory_pool::{METADATA_SLOT_BYTES, MemoryPoolBudget};
use libprocess::image::IMAGE_RIGHTS;
use test_kernel::birth::{Frame, Input, Mode, Role};
use test_kernel::report;

/// 受托平台根别名的权利；与 test_kernel 采用时核验的权利一致。
const PLATFORM_ROOT_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
const POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
/// held Pool 由 init 持有，别名只给 test_kernel 付款，不再外传。
const HELD_POOL_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw() | Rights::CREATE.raw() | Rights::DUPLICATE.raw() | Rights::GRANT.raw(),
);

/// 本部署的验收模式；feature 编码是组态接通前的登记债务。
fn mode() -> Mode {
    let flags = [
        (cfg!(feature = "acceptance-stress"), Mode::STRESS),
        (cfg!(feature = "acceptance-memory"), Mode::MEMORY),
        (
            cfg!(feature = "acceptance-mmio-publication"),
            Mode::MMIO_PUBLICATION,
        ),
        (cfg!(feature = "acceptance-mmio-forced"), Mode::MMIO_FORCED),
        (cfg!(feature = "acceptance-dma-races"), Mode::DMA_RACES),
        (cfg!(feature = "acceptance-dma-shared"), Mode::DMA_SHARED),
        (cfg!(feature = "acceptance-dma-quota"), Mode::DMA_QUOTA),
        (
            cfg!(feature = "acceptance-dma-install-failure"),
            Mode::DMA_INSTALL_FAILURE,
        ),
        (cfg!(feature = "acceptance-irq-exit"), Mode::IRQ_EXIT),
    ];
    Mode(flags.into_iter().fold(0, |bits, (enabled, flag)| {
        bits | if enabled { flag } else { 0 }
    }))
}

/// test_kernel 主 Pool 的份数：按各场景同时存活进程的峰值取 max，每份一个单进程工作集。
/// core 系为主进程、pm 与 Once 独立 2 MiB metadata 域（折两份）；stress 为主进程、pm、
/// 两 hammer 与一并发目标；memory 的线程 storm 借用 stress 组合（见组态计划债务）。
fn kernel_pool_budget(mode: Mode) -> MemoryPoolBudget {
    let shares = if mode.stress() { 5 } else { 4 };
    let worker = libdriver::boot::WORKER_POOL;
    MemoryPoolBudget {
        pages: shares * worker.pages,
        metadata: shares * worker.metadata,
    }
}

/// 跨 reset 保留 fixture 的付款域：保留子进程与其目标各一份单进程工作集。
fn held_pool_budget() -> MemoryPoolBudget {
    let worker = libdriver::boot::WORKER_POOL;
    MemoryPoolBudget {
        pages: 2 * worker.pages,
        metadata: 2 * worker.metadata,
    }
}

/// test_fp 在本部署的结局；决定根监督自检能否以 [test_kernel, test_fp] 为输入。
enum Companion {
    Completed(u64),
    /// 平台无兼容 D64 域，spawn 明确返回 NotSupported。
    Unsupported,
    /// memory 部署不运行 test_fp 与根监督自检。
    Skipped,
}

/// 已正常完成、Control 仍在 root 账本中的内核验收 task。
pub(super) struct KernelAcceptance {
    pid: u64,
    pool: Handle,
    /// held Pool 与 test_kernel 报告的最终快照。
    held: Option<(Handle, [u8; report::BYTES])>,
    companion: Companion,
}

impl KernelAcceptance {
    /// 启动 test_kernel（及非 memory 部署的 test_fp）并等其正常完成；Control 保留给 finish。
    pub(super) fn run(
        root: &mut RootSupervisor,
        services: Handle,
        names: &mut TopologyNames,
        images: &AcceptanceImages,
    ) -> Result<Self, &'static str> {
        let mode = mode();
        let mut frame = Frame::new(mode, Role::Main);
        let mut grants = alloc::vec::Vec::new();
        let mut add = |frame: &mut Frame, input: Input, handle: Handle, rights: Rights| {
            frame.set_slot(input, grants.len() as u8);
            grants.push(HandleGrant { handle, rights });
        };
        let mut sealed = alloc::vec::Vec::new();
        let full = !mode.memory();
        for (input, image) in [
            (Input::Pm, full.then_some(images.pm)),
            (Input::Target, full.then_some(images.target)),
            (Input::Hammer, images.hammer),
            (Input::SelfImage, full.then_some(images.kernel)),
        ] {
            let Some(image) = image else { continue };
            let object = root
                .copy_image(image.as_slice())
                .map_err(|_| "kernel acceptance image copy failed")?;
            let grant = root
                .duplicate(object.handle(), IMAGE_RIGHTS)
                .map_err(|_| "kernel acceptance image duplicate failed")?;
            add(&mut frame, input, grant, IMAGE_RIGHTS);
            frame.set_image_length(input, image.len as u64);
            sealed.push(object);
        }
        let job = root
            .create_job(services, root_memory_pool(), JOB_FULL_RIGHTS)
            .map_err(|_| "kernel acceptance Job create failed")?;
        let job_grant = root
            .duplicate(job, JOB_FULL_RIGHTS)
            .map_err(|_| "kernel acceptance Job duplicate failed")?;
        add(&mut frame, Input::Job, job_grant, JOB_FULL_RIGHTS);
        let reset = root
            .duplicate(
                root.reset_authority().ok_or("reset authority missing")?,
                Rights::DUPLICATE | Rights::GRANT,
            )
            .map_err(|_| "kernel acceptance reset duplicate failed")?;
        add(&mut frame, Input::Reset, reset, Rights::DUPLICATE);
        let roots = {
            let guard = INIT_STARTUP.lock();
            let platform = &guard
                .as_ref()
                .expect("init startup remains owned")
                ._platform;
            [
                platform.ecam_handle(),
                platform.irq_handle(),
                platform.dma_handle(),
            ]
        };
        if full {
            for (input, handle) in [Input::Mmio, Input::Irq, Input::Dma].into_iter().zip(roots) {
                if let Some(handle) = handle {
                    let alias = root
                        .duplicate(handle, PLATFORM_ROOT_RIGHTS)
                        .map_err(|_| "kernel acceptance platform duplicate failed")?;
                    add(&mut frame, input, alias, PLATFORM_ROOT_RIGHTS);
                }
            }
        }
        // 只有会运行跨 reset 保留 fixture 的部署才需要 held Pool。
        let held = if full && !mode.has(Mode::DMA_INSTALL_FAILURE) && roots[2].is_some() {
            let pool = root.hold_pool(held_pool_budget(), HELD_POOL_RIGHTS)?;
            let alias = root
                .duplicate(pool, HELD_POOL_RIGHTS)
                .map_err(|_| "held Pool grant failed")?;
            add(&mut frame, Input::Retention, alias, HELD_POOL_RIGHTS);
            Some(pool)
        } else {
            None
        };
        let pool = root
            .derive_pool(root_memory_pool(), kernel_pool_budget(mode), POOL_RIGHTS)
            .map_err(|error| {
                debug!("kernel acceptance Pool admission rejected: {:?}", error);
                "kernel acceptance Pool admission failed"
            })?;
        let pool_grant = root
            .duplicate(pool, POOL_RIGHTS)
            .map_err(|_| "kernel acceptance Pool duplicate failed")?;
        add(&mut frame, Input::Pool, pool_grant, POOL_RIGHTS);
        let report = root
            .create_mailbox(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::WRITE | Rights::GRANT,
            )
            .map_err(|_| "kernel acceptance report creation failed")?;
        add(&mut frame, Input::Report, report.peer, Rights::WRITE);
        let payload = frame.encode();
        root.reserve_process()
            .map_err(|_| "kernel acceptance supervision admission failed")?;
        let prepared = root.prepare_process(SpawnRequest {
            memory_pool: pool,
            parent: init_process_info(),
            job: services,
            image: images.kernel.as_slice(),
            payload: &payload,
            grants: &grants,
            control_rights: SUPERVISOR_RIGHTS,
        });
        if prepared.is_ok()
            || matches!(&prepared, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed)
        {
            for grant in grants {
                root.transferred(grant.handle);
            }
        }
        let prepared = prepared.map_err(|_| "kernel acceptance preparation failed")?;
        debug!(
            "kernel acceptance prepared before Start: pid={}",
            prepared.pid()
        );
        let started = root
            .start_prepared(prepared)
            .map_err(|_| "kernel acceptance Start failed")?;
        root.track_process(Supervised {
            pid: started.pid,
            control: started.control,
        });
        names.register_process(started.pid, "bin/test_kernel");
        let companion = if full {
            run_companion(root, services, names, images.fp)?
        } else {
            Companion::Skipped
        };
        if !completed(root, started.control)? {
            return Err("kernel acceptance did not complete normally");
        }
        let held = match held {
            Some(pool) => Some((pool, receive_report(root, report.owner, started.pid)?)),
            None => None,
        };
        root.close_control(report.owner)?;
        root.collect_job(job)?;
        root.close_control(job)?;
        drop(sealed);
        debug!(
            "required service topology complete: ordinary acceptance consumers admitted; foundation images validated"
        );
        Ok(Self {
            pid: started.pid,
            pool,
            held,
            companion,
        })
    }

    /// 收集内核验收 task 并执行根监督自检，再核主 Pool 归零与 held Pool 不变。
    /// 完成后 srv_device 才可准入，平台根别名与设备使用由此串行。
    pub(super) fn finish(
        self,
        root: &mut RootSupervisor,
        acceptance: Handle,
    ) -> Result<(), &'static str> {
        match self.companion {
            Companion::Completed(fp) => {
                root.verify_failure_isolation(acceptance, [self.pid, fp])?
            }
            Companion::Unsupported => {
                root.collect_process(self.pid)?;
                debug!(
                    "root failure isolation not applicable: no compatible D64 domain (test_fp NotSupported)"
                );
            }
            Companion::Skipped => root.collect_process(self.pid)?,
        }
        let snapshot = libkernel::memory_pool::query(self.pool)
            .map_err(|_| "kernel acceptance collected Pool query failed")?;
        if snapshot.allocated != 0
            || snapshot.reserved != 0
            || snapshot.delegated != 0
            || snapshot.metadata.allocated != 0
            || snapshot.metadata.reserved != 0
            || snapshot.metadata.delegated != 0
        {
            return Err("kernel acceptance Pool remained charged after collection");
        }
        debug!("kernel acceptance Pool refunded after Collector: pages=0, metadata=0");
        root.close_control(self.pool)?;
        if let Some((pool, reported)) = self.held {
            let snapshot =
                libkernel::memory_pool::query(pool).map_err(|_| "held Pool query failed")?;
            if report::encode_pool(snapshot) != reported {
                return Err("held Pool changed after kernel acceptance report");
            }
            debug!(
                "held Pool unchanged after kernel acceptance collection: pages={}, metadata_slots={}, metadata_bytes={}",
                snapshot.allocated,
                snapshot.metadata.allocated,
                snapshot.metadata.allocated * METADATA_SLOT_BYTES
            );
        }
        Ok(())
    }
}

/// test_fp 是失败隔离自检的第二个正常完成成员；仅明确 NotSupported 时降级。
fn run_companion(
    root: &mut RootSupervisor,
    services: Handle,
    names: &mut TopologyNames,
    image: StartupImage,
) -> Result<Companion, &'static str> {
    root.reserve_process()
        .map_err(|_| "FP supervision admission failed")?;
    match root.spawn_process(SpawnRequest {
        memory_pool: root_memory_pool(),
        parent: init_process_info(),
        job: services,
        image: image.as_slice(),
        payload: &[],
        grants: &[],
        control_rights: SUPERVISOR_RIGHTS,
    }) {
        Ok(fp) => {
            root.track_process(Supervised {
                pid: fp.pid,
                control: fp.control,
            });
            names.register_process(fp.pid, "bin/test_fp");
            if !completed(root, fp.control)? {
                return Err("required FP did not complete normally");
            }
            Ok(Companion::Completed(fp.pid))
        }
        Err(failure)
            if failure.error == libprocess::SpawnError::System(SystemCallError::NotSupported)
                && failure.grants == libprocess::GrantOutcome::Retained
                && failure.cleanup_error.is_none() =>
        {
            debug!("optional service bin/test_fp degraded: {:?}", failure);
            Ok(Companion::Unsupported)
        }
        Err(failure) => {
            debug!("required FP spawn failed: {:?}", failure);
            Err("required FP spawn failed")
        }
    }
}

/// 等进程 DONE，返回其是否以 Completed/NONE 正常结束。
fn completed(root: &mut RootSupervisor, control: Handle) -> Result<bool, &'static str> {
    let observed = root
        .wait_until(control, ObjectSignals::DONE, Deadline::INFINITE)
        .map_err(|_| "completion wait failed")?;
    let snapshot = process::query(control).map_err(|_| "terminal query failed")?;
    Ok(observed.observed.contains(ObjectSignals::DONE)
        && snapshot.state == ProcessState::Dead as u32
        && snapshot.cause == TerminationCause::Completed as u32
        && snapshot.detail == libkernel::shared::proc::TerminationDetail::NONE)
}

fn receive_report(
    root: &mut RootSupervisor,
    owner: Handle,
    pid: u64,
) -> Result<[u8; report::BYTES], &'static str> {
    let message = root
        .receive_until(owner, Deadline::INFINITE)
        .map_err(|_| "held Pool report missing")?;
    if message.header.sender_pid != pid
        || message.header.kind != report::KIND
        || !message.handles.is_empty()
    {
        return Err("held Pool report identity invalid");
    }
    message
        .payload
        .as_slice()
        .try_into()
        .map_err(|_| "held Pool report length invalid")
}
