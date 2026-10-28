//! init 的固定部署选择与停止聚合；公共库不认识基础服务名单。
use super::*;
use supervision::Instance;

pub(super) fn instances() -> &'static [Instance] {
    if cfg!(feature = "system") {
        &[Instance::Device, Instance::Driver, Instance::Filesystem]
    } else {
        &[
            Instance::Device,
            Instance::Driver,
            Instance::Filesystem,
            Instance::Secondary,
        ]
    }
}
pub(super) fn file_hosts() -> usize {
    instances()
        .iter()
        .filter(|role| role.is_filesystem())
        .count()
}

pub(super) fn system(root: &mut RootSupervisor, services: Handle) -> Result<(), &'static str> {
    // 发布任何进程前先核完整输入集合；重复和缺失均拒绝。
    let mut images = [None; 6];
    let mut invalid = false;
    with_init_payload(|payload| {
        tar::walk(payload, |entry| {
            let index = match entry.name {
                "bin/srv_device" => 0,
                "bin/srv_driver" => 1,
                "bin/srv_fs" => 2,
                "bin/drv_pci_generic" => 3,
                "bin/drv_usb_xhci" => 4,
                "bin/drv_usb_hid" => 5,
                _ => {
                    invalid = true;
                    return;
                }
            };
            if images[index]
                .replace(StartupImage::from_slice(entry.data))
                .is_some()
            {
                invalid = true;
            }
        })
    })
    .map_err(|_| "system boot archive invalid")?;
    if invalid || images.iter().any(Option::is_none) {
        return Err("system image set invalid");
    }
    let images = images.map(|image| image.expect("complete image set checked"));
    root.begin_supervision()?;
    let mut names = TopologyNames::new();
    root.devices = Some(device::Manager::launch(
        root,
        services,
        &mut names,
        images[0].as_slice(),
    )?);
    root.drivers = Some(driver::Manager::launch(
        root,
        services,
        &mut names,
        images[1].as_slice(),
        [
            images[3].as_slice(),
            images[4].as_slice(),
            images[5].as_slice(),
        ],
    )?);
    root.system_host = Some(Host::launch(
        root,
        services,
        &mut names,
        images[2].as_slice(),
        srv_fs::startup::Profile::System,
        None,
    )?);
    libkernel::debug!(
        "SYSTEM foundation ready: device, driver, filesystem and registry; no acceptance consumers"
    );
    // host 持实际授权根，不新增占位消费者。空闲由同一 Assembly 的真实来源停驻。
    loop {
        root.turn_and_wait(None)
            .map_err(|_| "system supervision failed")?;
    }
}

/// 全系统停止意图由 root 稳定持有；局部验收宿主的停止不封整个部署。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum StopPhase {
    Consumers,
    Files,
    Devices,
    Root,
    Completed,
}
pub(super) struct Stop {
    pub action: ResetAction,
    pub deadline: Deadline,
    pub phase: StopPhase,
}
impl RootSupervisor {
    pub(super) fn stopping(&self) -> bool {
        self.stop.is_some()
    }
    pub(super) fn check_admission(&self) -> Result<(), &'static str> {
        if self.stopping() {
            Err("system assembly admission closed")
        } else {
            Ok(())
        }
    }
    pub(super) fn stop_deadline(&self, deadline: Deadline) -> Deadline {
        self.stop.as_ref().map_or(deadline, |stop| {
            Deadline::at(
                deadline
                    .instant()
                    .ok()
                    .flatten()
                    .map_or(stop.deadline.at_ns, |at| at.min(stop.deadline.at_ns)),
            )
        })
    }
    pub(super) fn request_stop(
        &mut self,
        services: Handle,
        action: ResetAction,
    ) -> Result<StopPhase, &'static str> {
        if let Some(stop) = &self.stop {
            return if stop.action == action {
                Ok(stop.phase)
            } else {
                Err("system stop action already committed")
            };
        }
        // 预算来自实际原记录集合，冻结后每轮总期限仍覆盖 detach 与 Collector。
        let fs_budget = self
            .active_filesystems()
            .checked_mul(srv_fs::deployment::STOP_TIMEOUT_NS)
            .ok_or("file stop budget overflow")?;
        let device_budget = if self.devices.is_some() {
            super::device::STOP_TIMEOUT_NS
        } else {
            0
        };
        let driver_budget = if self.drivers.is_some() {
            super::driver::STOP_TIMEOUT_NS
        } else {
            0
        };
        let budget = Self::root_stop_budget()?
            .checked_add(self.consumer_stop_budget()?)
            .ok_or("system stop budget overflow")?
            .checked_add(fs_budget)
            .and_then(|ns| ns.checked_add(device_budget))
            .and_then(|n| n.checked_add(driver_budget))
            .ok_or("system stop budget overflow")?;
        let deadline = libkernel::time::snapshot()
            .map_err(|_| "system stop clock failed")?
            .now_ns
            .checked_add(budget)
            .map(Deadline::at)
            .ok_or("system stop deadline overflow")?;
        self.stop = Some(Stop {
            action,
            deadline,
            phase: StopPhase::Consumers,
        });
        process::seal_job(services).map_err(|_| "system services seal failed")?;
        debug!(
            "SYSTEM stop committed: services sealed, deadline={}",
            deadline.at_ns
        );
        self.stop_consumers()?;
        // 正式部署的根由 root 稳定持有，在 provider 停止前实际释放。
        if let Some(mut host) = self.system_host.take() {
            host.release_roots();
        }
        self.stop.as_mut().expect("stop intent remains owned").phase = StopPhase::Files;
        self.stop_filesystems()?;
        self.stop.as_mut().expect("stop intent remains owned").phase = StopPhase::Devices;
        self.stop_drivers()?;
        self.stop_devices()?;
        self.stop.as_mut().expect("stop intent remains owned").phase = StopPhase::Root;
        debug!("SYSTEM providers stopped: file completion and device safety retained separately");
        Ok(StopPhase::Root)
    }
    pub(super) fn finish_stop(&mut self, services: Handle) -> Result<(), &'static str> {
        if self.stop.is_none() {
            self.request_stop(services, ResetAction::Shutdown)?;
        }
        self.collect_job(services)?;
        self.close_control(services)?;
        self.verify_idle_ownership()?;
        self.stop.as_mut().expect("stop intent remains owned").phase = StopPhase::Completed;
        debug!("SYSTEM stop completed: consumers, providers, original process and Job collectors");
        Ok(())
    }
}
