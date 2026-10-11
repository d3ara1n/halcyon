//! 同一 Runtime 的具体任务分派；文件与 registry 只借用各自实际 backing。
use super::{FileTask, World};
use erhino_shared::{call::SystemCallError, time::Deadline};
use libexecution::runtime::{Advance, Input, RequestFailure, Requests, SourceId, Step};
impl FileTask {
    pub(super) fn advance_extra(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::FileRoot(task) => task.advance(world, requests, input),
            Self::ServiceRoot(task) => task.advance(
                world
                    .service
                    .as_mut()
                    .expect("registry root backing missing"),
                requests,
                input,
            ),
            Self::ServiceIngress(task) => task.advance(id, world, requests, input, budget),
            Self::ServiceRequest(task) => task.advance(
                id,
                world
                    .service
                    .as_mut()
                    .expect("registry request backing missing"),
                requests,
                input,
                budget,
            ),
            Self::Maintenance(task) => {
                while input.pull().is_some() {}
                input.take_timeout();
                let state = world
                    .service
                    .as_mut()
                    .expect("registry maintenance backing missing");
                if world.stop
                    && state.registry.is_empty()
                    && state.accesses.is_empty()
                    && state.authorities.is_empty()
                {
                    Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    })
                } else {
                    task.advance(state, requests, budget)
                }
            }
            Self::Release(task) => task.advance(world, requests, input),
            Self::Retire(task) => task.advance(world, requests, input, budget),
            Self::Dispatcher(task) => task.advance(world, requests, input, budget),
            Self::Ingress(_) | Self::Request(_) => unreachable!("file task entered extra dispatch"),
        }
    }
    pub(super) fn registered_extra(&mut self, world: &mut World, kind: u64, source: SourceId) {
        match self {
            Self::FileRoot(task) => task.registered(kind, source),
            Self::ServiceRoot(task) => task.registered(kind, source),
            Self::ServiceIngress(task) => task.registered(kind, source),
            Self::ServiceRequest(task) => task.registered(kind, source),
            Self::Release(task) => task.registered(kind, source),
            Self::Retire(task) => task.registered(kind, source),
            Self::Dispatcher(task) => {
                let rpc = world.rpc_mut();
                rpc.registered(&mut (), kind, source);
                task.deadline = rpc.deadline();
            }
            Self::Maintenance(_) => unreachable!("registry maintenance requested source"),
            Self::Ingress(_) | Self::Request(_) => unreachable!("file task entered extra callback"),
        }
    }
    pub(super) fn unregistered_extra(&mut self, world: &mut World, kind: u64, source: SourceId) {
        match self {
            Self::FileRoot(task) => task.unregistered(source),
            Self::ServiceRoot(task) => task.unregistered(source),
            Self::ServiceIngress(task) => task.unregistered(
                world.service.as_mut().expect("registry backing missing"),
                kind,
                source,
            ),
            Self::ServiceRequest(task) => task.unregistered(
                world.service.as_mut().expect("registry backing missing"),
                kind,
                source,
            ),
            Self::Release(task) => task.unregistered(source),
            Self::Retire(task) => task.unregistered(source),
            Self::Dispatcher(task) => {
                let rpc = world.rpc_mut();
                rpc.unregistered(&mut (), kind, source);
                task.deadline = rpc.deadline();
            }
            Self::Maintenance(_) => unreachable!("registry maintenance removed source"),
            Self::Ingress(_) | Self::Request(_) => unreachable!("file task entered extra callback"),
        }
    }
    pub(super) fn refused_extra(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        match self {
            Self::FileRoot(task) => {
                if let RequestFailure::Source { kind, .. } = failure {
                    task.refused(kind);
                }
                world.stop = true;
            }
            Self::ServiceRoot(task) => {
                if let RequestFailure::Source { kind, .. } = failure {
                    task.refused(kind);
                }
                world.stop = true;
            }
            Self::ServiceIngress(task) => match failure {
                RequestFailure::Spawn { submission, .. } => {
                    let Self::ServiceRequest(request) = submission.into_parts().0 else {
                        unreachable!("registry ingress spawned another task kind")
                    };
                    task.refused_task(request);
                }
                failure => {
                    if matches!(failure, RequestFailure::Source { kind: 0, .. }) {
                        world.stop = true;
                    }
                    task.refused(failure);
                }
            },
            Self::ServiceRequest(task) => task.refused(failure),
            Self::Release(task) => {
                if let RequestFailure::Source { kind, .. } = failure {
                    task.refused(kind);
                }
                world.stop = true;
            }
            Self::Retire(task) => {
                if let RequestFailure::Source { kind, .. } = failure {
                    task.refused(kind);
                }
            }
            Self::Dispatcher(task) => {
                let rpc = world.rpc_mut();
                rpc.refused(&mut (), failure);
                task.deadline = rpc.deadline();
            }
            Self::Maintenance(_) => {
                if let Some(state) = world.service.as_mut() {
                    state.dirty = true;
                }
            }
            Self::Ingress(_) | Self::Request(_) => unreachable!("file task entered extra callback"),
        }
    }
    pub(super) fn stop_extra(&mut self, world: &mut World) {
        match self {
            Self::FileRoot(task) => task.stop(),
            Self::ServiceRoot(task) => task.stop(),
            Self::ServiceIngress(task) => task.stop(),
            Self::ServiceRequest(task) => task.stop(),
            Self::Release(task) => task.stop(),
            Self::Retire(task) => task.stop(),
            Self::Dispatcher(task) => task.stopping = true,
            Self::Maintenance(_) => {
                if let Some(state) = world.service.as_mut() {
                    state.dirty = true;
                }
            }
            Self::Ingress(_) | Self::Request(_) => unreachable!("file task entered extra stop"),
        }
    }
    pub(super) fn deadline_extra(&self) -> Deadline {
        match self {
            Self::Retire(task) => task.deadline(),
            Self::ServiceRequest(task) => task.deadline(),
            Self::ServiceIngress(task) => task.deadline(),
            Self::Dispatcher(task) => task.deadline,
            _ => Deadline::INFINITE,
        }
    }
}
