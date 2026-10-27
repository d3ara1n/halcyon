//! 两种领域观察直接组合既有 RPC ConsumerTask；原 Managed 完成实际读取后才 Ack。
use super::*;
use librpc::subscription::ConsumerTask;
enum Watched {
    Binding(ConsumerTask<libdevice::client::Watch>),
    Activation(ConsumerTask<libdriver::client::Watch>),
}
pub(super) struct Subscription {
    run: Rc<RefCell<Run>>,
    watched: Watched,
    reading: bool,
}
impl Subscription {
    pub fn binding(run: Rc<RefCell<Run>>, watch: libdevice::client::Watch) -> Self {
        Self {
            run,
            watched: Watched::Binding(ConsumerTask::new(watch, Deadline::INFINITE)),
            reading: false,
        }
    }
    pub fn activation(run: Rc<RefCell<Run>>, watch: libdriver::client::Watch) -> Self {
        Self {
            run,
            watched: Watched::Activation(ConsumerTask::new(watch, Deadline::INFINITE)),
            reading: false,
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        match &mut self.watched {
            Watched::Binding(task) => task.registered(kind, id),
            Watched::Activation(task) => task.registered(kind, id),
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        match &mut self.watched {
            Watched::Binding(task) => task.unregistered(id),
            Watched::Activation(task) => task.unregistered(id),
        }
    }
    pub fn deadline(&self) -> Deadline {
        match &self.watched {
            Watched::Binding(task) => task.deadline(),
            Watched::Activation(task) => task.deadline(),
        }
    }
    pub fn stop(&mut self) {
        match &mut self.watched {
            Watched::Binding(task) => task.stop(),
            Watched::Activation(task) => task.stop(),
        }
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        let activation = matches!(self.watched, Watched::Activation(_));
        if self.run.borrow().watchers_stopping {
            self.stop();
        }
        let mut run = self.run.borrow_mut();
        if activation {
            run.activation_watch = id;
        } else {
            run.binding_watch = id;
        }
        let changed = if activation {
            run.activation_changed
        } else {
            run.binding_changed
        };
        if self.reading && !changed {
            match &mut self.watched {
                Watched::Binding(task) => task.acknowledge_read()?,
                Watched::Activation(task) => task.acknowledge_read()?,
            }
            self.reading = false;
        }
        drop(run);
        let result = match &mut self.watched {
            Watched::Binding(task) => {
                task.advance(id, world.transport_task, &mut world.rpc, requests, input, 1)
            }
            Watched::Activation(task) => {
                task.advance(id, world.transport_task, &mut world.rpc, requests, input, 1)
            }
        };
        let mut run = self.run.borrow_mut();
        let available = match &self.watched {
            Watched::Binding(task) => task.snapshot().is_some(),
            Watched::Activation(task) => {
                if let Some(snapshot) = task.snapshot() {
                    if snapshot.reference != run.reference {
                        fatal(Error::StaleEpoch);
                    }
                    run.activation_status = Some(snapshot);
                    true
                } else {
                    false
                }
            }
        };
        if available && !self.reading {
            if activation {
                run.activation_changed = true;
            } else {
                run.binding_changed = true;
            }
            self.reading = true;
            requests.wake(run.task)?;
        }
        let failed = match &self.watched {
            Watched::Binding(task) => task.failure().is_some(),
            Watched::Activation(task) => task.failure().is_some(),
        };
        if failed && !run.watchers_stopping {
            if !world.stopping && !run.stop_requested {
                let process = run.process.ok_or(SystemCallError::InternalError)?;
                let state = worker::cpu_state(world, process).unwrap_or_else(|e| fatal(e));
                let task = world
                    .processes
                    .get(process.serial())
                    .ok_or(SystemCallError::InternalError)?
                    .borrow()
                    .task;
                if state == erhino_shared::proc::ProcessState::Terminating as u32 {
                    // 先保留失败owner，等唯一进程DONE来源唤醒；不从endpoint关闭制造死亡/Released。
                    if task != 0 {
                        requests.wake(task)?;
                    }
                    return Ok(advance(Step::Parked));
                }
                if state != erhino_shared::proc::ProcessState::Dead as u32 {
                    fatal(Error::Unavailable);
                }
                run.fail(Error::Unavailable);
                if task != 0 {
                    requests.wake(task)?;
                }
            }
            run.stop_requested = true;
            requests.wake(run.task)?;
        }
        if result.step == Step::Complete {
            if activation {
                run.activation_watch = 0;
            } else {
                run.binding_watch = 0;
            }
            requests.wake(run.task)?;
        }
        Ok(result)
    }
}
