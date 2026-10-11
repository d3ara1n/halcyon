//! Registry 的公平维护任务；通知失败保留工作，已撤出的入口 owner 由原任务推进。
use crate::{provider::State, resource::ServiceResource};
use erhino_shared::call::SystemCallError;
use libbudget::Charge;
use libexecution::runtime::{Advance, Requests, Step};
pub struct Maintenance {
    phase: u8,
    after: Option<u64>,
    retry: bool,
    _bytes: Charge,
}
impl Maintenance {
    pub fn new(state: &State) -> Result<Self, SystemCallError> {
        Ok(Self {
            phase: 0,
            after: None,
            retry: false,
            _bytes: state
                .account
                .acquire(ServiceResource::Bytes, core::mem::size_of::<Self>())?,
        })
    }
    pub fn advance<F>(
        &mut self,
        state: &mut State,
        requests: &mut Requests<'_, F>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut done = 0;
        while done < budget {
            done += 1;
            match self.phase {
                0 => {
                    if !state.dirty && !self.retry {
                        return Ok(Advance {
                            work_done: done,
                            step: Step::Parked,
                        });
                    }
                    state.dirty = false;
                    self.retry = false;
                    self.after = None;
                    self.phase = 1;
                }
                1 => {
                    if let Some(context) = state.registry.watches.next_context(self.after) {
                        self.after = Some(context);
                        match state.registry.watches.signal(context) {
                            Err(SystemCallError::ObjectClosed) => {
                                state.registry.watches.notification_closed(context)
                            }
                            Err(_) => self.retry = true,
                            Ok(()) => {}
                        }
                    } else {
                        self.phase = 2;
                        self.after = None;
                    }
                }
                _ => {
                    if let Some(context) = state.registry.next_context(self.after) {
                        self.after = Some(context);
                        if let Some(task) = state.registry.retiring_task(context)
                            && requests.wake(task).is_err()
                        {
                            self.retry = true;
                        }
                    } else {
                        self.phase = 0;
                    }
                }
            }
        }
        Ok(Advance {
            work_done: done,
            step: Step::Runnable,
        })
    }
}
