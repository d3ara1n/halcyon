//! root的装配入口：显式推进原loader，等待时继续服务所有已提交职责。

use super::*;
use libprocess::{
    GrantOutcome, Preparation, PreparedProcess, SpawnError, SpawnFailure, SpawnRequest, Spawned,
};

impl RootSupervisor {
    /// 取消先由装配 owner 提交，再把同一个 Collector 交给根收集账本。
    fn collect_cancelled(
        &mut self,
        cancellation: Result<Option<Collector>, SystemCallError>,
    ) -> Option<SystemCallError> {
        match cancellation {
            Ok(Some(collector)) => self
                .collect_machine(collector)
                .err()
                .map(|_| SystemCallError::ObjectBusy),
            Ok(None) => None,
            Err(_) => self.system_failure("root preparation cleanup unavailable"),
        }
    }

    pub(crate) fn cancel_prepared(
        &mut self,
        mut prepared: PreparedProcess,
    ) -> Result<(), &'static str> {
        let cancellation = prepared.cancel_to_collector();
        if self.collect_cancelled(cancellation).is_some() {
            return Err("prepared process collection failed");
        }
        Ok(())
    }
    pub(crate) fn prepare_process(
        &mut self,
        request: SpawnRequest<'_>,
    ) -> Result<PreparedProcess, SpawnFailure> {
        self.prepare_process_until(request, Deadline::INFINITE)
    }
    pub(crate) fn prepare_process_until(
        &mut self,
        request: SpawnRequest<'_>,
        deadline: Deadline,
    ) -> Result<PreparedProcess, SpawnFailure> {
        let mut preparing = Preparation::new(request).map_err(|error| SpawnFailure {
            error,
            grants: GrantOutcome::Retained,
            cleanup_error: None,
        })?;
        loop {
            let result = self
                .turn_and_wait(Some(Await::Turn))
                .map_err(SpawnError::System)
                .and_then(|()| {
                    if self.stopping()
                        || deadline
                            .instant()
                            .map_err(|_| SpawnError::System(SystemCallError::IllegalArgument))?
                            .is_some_and(|end| {
                                libkernel::time::snapshot().map_or(true, |now| now.now_ns >= end)
                            })
                    {
                        return Err(SpawnError::System(SystemCallError::DeadlineExpired));
                    }
                    preparing.advance()
                });
            match result {
                Ok(Some(prepared)) => return Ok(prepared),
                Ok(None) => {}
                Err(error) => {
                    let grants = preparing.grants();
                    let cleanup_error = self.collect_cancelled(preparing.cancel());
                    return Err(SpawnFailure {
                        error,
                        grants,
                        cleanup_error,
                    });
                }
            }
        }
    }

    pub(crate) fn start_prepared(
        &mut self,
        mut prepared: PreparedProcess,
    ) -> Result<Spawned, SpawnFailure> {
        match prepared.try_start() {
            Ok(spawned) => Ok(spawned),
            Err(error) => {
                let grants = prepared.grants();
                let cleanup_error = self.collect_cancelled(prepared.cancel_to_collector());
                Err(SpawnFailure {
                    error: error.into(),
                    grants,
                    cleanup_error,
                })
            }
        }
    }

    pub(crate) fn spawn_process(
        &mut self,
        request: SpawnRequest<'_>,
    ) -> Result<Spawned, SpawnFailure> {
        let prepared = self.prepare_process(request)?;
        self.start_prepared(prepared)
    }

    pub(crate) fn copy_image(
        &mut self,
        bytes: &[u8],
    ) -> Result<libprocess::image::Image, SystemCallError> {
        let mut copying = libprocess::image::ImageCopy::new(bytes)?;
        let result = (|| loop {
            self.turn_and_wait(Some(Await::Turn))?;
            if let Some(image) = copying.advance()? {
                break Ok(image);
            }
            if let Some(handle) = copying.waiting() {
                self.wait_until(handle, ObjectSignals::EXECUTABLE, Deadline::INFINITE)?;
            }
        })();
        if result.is_err() && copying.cancel().is_err() {
            self.system_failure("unpublished image mapping cleanup unavailable");
        }
        result
    }
}
