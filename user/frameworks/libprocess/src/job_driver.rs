//! Job 收束机器的 Runtime 接缝：观察注销回执先于机器恢复，所有事件完整分派。

use crate::{JobCollector, JobCollectorError, JobCollectorEvent, observation::ObservationSlot};
use erhino_shared::{call::SystemCallError, time::Deadline};
use libsrv::runtime::{Advance, Input, Requests, SourceId, SourceKind, Step};

pub struct JobDriver {
    machine: JobCollector,
    observation: ObservationSlot,
    retry_at: Option<u64>,
    failure: Option<JobCollectorError>,
}

impl JobDriver {
    pub fn new(machine: JobCollector) -> Self {
        Self {
            machine,
            observation: ObservationSlot::default(),
            retry_at: None,
            failure: None,
        }
    }
    pub fn machine(&self) -> &JobCollector {
        &self.machine
    }
    pub fn replenish(&mut self, policy: crate::SupervisionPolicy) {
        self.machine.replenish(policy);
        self.failure = None;
        self.retry_at = None;
    }
    pub fn failure(&self) -> Option<JobCollectorError> {
        self.failure
    }
    pub fn deadline(&self) -> Deadline {
        if self.observation.is_active() {
            self.observation.deadline()
        } else {
            self.retry_at.map_or(Deadline::INFINITE, Deadline::at)
        }
    }
    pub fn registered(&mut self, kind: SourceKind, source: SourceId) {
        self.observation.registered(kind, source);
    }
    pub fn unregistered(&mut self, kind: SourceKind, source: SourceId) {
        self.observation.unregistered(kind, source);
    }
    pub fn refused(&mut self, kind: SourceKind, error: SystemCallError) {
        self.observation.refused(kind, error);
    }

    pub fn advance<T>(
        &mut self,
        requests: &mut Requests<T>,
        input: &mut Input<'_>,
        now: u64,
    ) -> Result<Advance, SystemCallError> {
        if let Some((request, result)) = self
            .observation
            .poll(input, requests, now, |request| self.machine.probe(request))?
        {
            if let Err(error) = self.machine.observe(request, result, now) {
                self.failure = Some(error);
                return Err(SystemCallError::InternalError);
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.observation.is_active() || self.retry_at.is_some_and(|at| now < at) {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let _ = input.take_timeout();
        self.retry_at = None;
        self.failure = None;
        let event = self.machine.step(now).map_err(|error| {
            self.failure = Some(error);
            SystemCallError::InternalError
        })?;
        let step = match event {
            JobCollectorEvent::Progress => Step::Runnable,
            JobCollectorEvent::Done => Step::Complete,
            JobCollectorEvent::RetryAt(at) => {
                self.retry_at = Some(at);
                Step::Parked
            }
            JobCollectorEvent::Observe(request) => {
                self.observation.begin(request, requests)?;
                Step::Parked
            }
        };
        Ok(Advance { work_done: 1, step })
    }
}
