// 复用 Runtime/FakeSet 夹具，只验证实际费用与来源责任；不模拟领域或 Kernel capability。
fn funding_accounts(
    global_tasks: usize,
    a_tasks: usize,
    b_tasks: usize,
) -> (
    AccountView<ExecutionResource>,
    AccountView<ExecutionResource>,
    AccountView<ExecutionResource>,
) {
    let limits = [global_tasks, 1024 * 1024];
    let budget = Budget::new(&limits, 3).unwrap();
    let slots = [budget.slot(0).unwrap(), budget.slot(1).unwrap()];
    let h = budget
        .account(&[global_tasks, limits[1]])
        .unwrap()
        .view(&slots)
        .unwrap();
    let a = budget
        .account(&[a_tasks, 64 * 1024])
        .unwrap()
        .view(&slots)
        .unwrap();
    let b = budget
        .account(&[b_tasks, 64 * 1024])
        .unwrap()
        .view(&slots)
        .unwrap();
    (h, a, b)
}

#[test]
fn explicit_tasks_isolate_local_quota_and_obey_shared_global_quota() {
    let shared = fake_shared();
    let (h, a, b) = funding_accounts(2, 1, 2);
    let assembly =
        Assembly::new(FakeSet::new(&shared), &h).unwrap_or_else(|_| panic!("assembly failed"));
    let mut rt = Runtime::<ToyTask, FakeSet>::new(assembly.bind().unwrap(), 8, 8, &h).unwrap();
    let baseline = h.usage(ExecutionResource::InputBytes);
    let first = rt
        .spawn(TaskSubmission::new(ToyTask { turns: 1 }, 0, a.clone()))
        .unwrap();
    let local = rt
        .spawn(TaskSubmission::new(ToyTask { turns: 7 }, 2, a.clone()))
        .unwrap_err();
    assert_eq!(local.error, SystemCallError::QuotaExceeded);
    assert_eq!(local.submission.funding().account_id(), a.account_id());
    let (returned, max_sources, returned_funding) = local.submission.into_parts();
    assert_eq!((returned.turns, max_sources), (7, 2));
    assert_eq!(returned_funding.account_id(), a.account_id());
    rt.spawn(TaskSubmission::new(ToyTask { turns: 1 }, 0, b.clone()))
        .unwrap();
    assert_eq!(
        (
            h.usage(ExecutionResource::Task).0,
            a.usage(ExecutionResource::Task).0,
            b.usage(ExecutionResource::Task).0
        ),
        (0, 1, 1)
    );
    let global = rt
        .spawn(TaskSubmission::new(ToyTask { turns: 9 }, 0, b.clone()))
        .unwrap_err();
    assert_eq!(global.error, SystemCallError::QuotaExceeded);
    assert_eq!(b.usage(ExecutionResource::Task).0, 1);
    assert_eq!(h.usage(ExecutionResource::InputBytes), baseline);
    rt.queue.begin_finalizing(first);
    let mut world = World::default();
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if rt.is_empty() {
            break;
        }
    }
    assert!(rt.is_empty());
    let recovered = rt.spawn(global.submission).unwrap();
    assert!(recovered > first);
    rt.queue.begin_finalizing(recovered);
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if rt.is_empty() {
            break;
        }
    }
    assert_eq!(a.usage(ExecutionResource::Task).0, 0);
    assert_eq!(b.usage(ExecutionResource::Task).0, 0);
    assert_eq!(h.usage(ExecutionResource::InputBytes), baseline);
    assert!(rt.close().is_ok());
    assert!(
        h.usage(ExecutionResource::InputBytes).0 > 0,
        "Assembly remains a live infrastructure owner"
    );
    assert!(assembly.close().is_ok());
    assert_eq!(h.usage(ExecutionResource::InputBytes).0, 0);
}

struct FundingSources {
    registrations: [crate::SourceRegistration; 3],
    mask: ObjectSignals,
    remove: bool,
    finish: bool,
}
impl Task<World> for FundingSources {
    type Family = Self;
    fn advance(
        &mut self,
        _: u64,
        _: &mut World,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        _: usize,
    ) -> Result<Advance, SystemCallError> {
        while input.pull().is_some() {}
        for (n, registration) in self.registrations.iter_mut().enumerate() {
            if self.remove || self.finish {
                registration.remove(requests)?;
            } else {
                registration.install(
                    requests,
                    SourcePlan::new(Handle::from_parts(n as u32 + 20, 1), self.mask),
                    n as u64 + 1,
                )?;
            }
        }
        Ok(Advance {
            work_done: 1,
            step: if self.finish
                && self
                    .registrations
                    .iter()
                    .all(crate::SourceRegistration::is_empty)
            {
                Step::Complete
            } else if self
                .registrations
                .iter()
                .any(|r| r.is_installing() || r.is_removing())
            {
                Step::Runnable
            } else {
                Step::Parked
            },
        })
    }
    fn registered(&mut self, _: &mut World, kind: u64, source: SourceId) {
        self.registrations[kind as usize - 1].registered(kind, source);
    }
    fn unregistered(&mut self, _: &mut World, kind: u64, source: SourceId) {
        self.registrations[kind as usize - 1].unregistered(source);
    }
    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        match failure {
            RequestFailure::Source { kind, error } => {
                self.registrations[kind as usize - 1].refused(kind);
                world.refusals.push(error);
            }
            _ => panic!("funding task only submits source commands"),
        }
    }
    fn stop(&mut self, _: &mut World) {
        self.finish = true;
    }
}

#[test]
fn one_host_task_keeps_bound_sources_on_original_accounts_through_mask_reinstallation() {
    let shared = fake_shared();
    let (h, a, b) = funding_accounts(4, 1, 1);
    let assembly =
        Assembly::new(FakeSet::new(&shared), &h).unwrap_or_else(|_| panic!("assembly failed"));
    let mut rt =
        Runtime::<FundingSources, FakeSet>::new(assembly.bind().unwrap(), 4, 8, &h).unwrap();
    let baseline = h.usage(ExecutionResource::InputBytes).0;
    let task = FundingSources {
        registrations: [
            crate::SourceRegistration::funded(a.clone()),
            crate::SourceRegistration::funded(b.clone()),
            crate::SourceRegistration::new(),
        ],
        mask: ObjectSignals::CLOSED,
        remove: false,
        finish: false,
    };
    let id = rt
        .spawn(TaskSubmission::new(task, 3, h.clone()))
        .unwrap_or_else(|_| panic!("task admission failed"));
    let mut world = World::default();
    shared
        .register_error
        .set(Some(SystemCallError::RightsDenied));
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if !world.refusals.is_empty() {
            break;
        }
    }
    assert_eq!(world.refusals, [SystemCallError::RightsDenied]);
    assert_eq!(
        a.usage(ExecutionResource::InputBytes).0,
        0,
        "failed SourceOps does not retain a new charge"
    );
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if shared.registered().len() == 3 {
            break;
        }
    }
    let source_bytes = Runtime::<FundingSources, FakeSet>::source_input_budget(1).unwrap();
    assert_eq!(
        h.usage(ExecutionResource::InputBytes).0,
        baseline + source_bytes
    );
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, source_bytes);
    assert_eq!(b.usage(ExecutionResource::InputBytes).0, source_bytes);
    shared.set_remove_busy(2);
    rt.get_task_mut(id).unwrap().remove = true;
    rt.wake(id).unwrap();
    for _ in 0..100 {
        shared.now.set(shared.now.get().saturating_add(1_000_000));
        rt.turn(&mut world, 1).unwrap();
        if shared.removes().len() == 3 {
            break;
        }
    }
    assert_eq!(shared.removes().len(), 3);
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, 0);
    assert_eq!(b.usage(ExecutionResource::InputBytes).0, 0);
    let task = rt.get_task_mut(id).unwrap();
    task.remove = false;
    task.mask = ObjectSignals::WRITABLE | ObjectSignals::CLOSED;
    rt.wake(id).unwrap();
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if shared.registered().len() == 6 {
            break;
        }
    }
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, source_bytes);
    assert_eq!(b.usage(ExecutionResource::InputBytes).0, source_bytes);
    assert_eq!(
        h.usage(ExecutionResource::InputBytes).0,
        baseline + source_bytes
    );
    assert_eq!(world.refusals, [SystemCallError::RightsDenied]);
    rt.get_task_mut(id).unwrap().finish = true;
    rt.wake(id).unwrap();
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if rt.is_empty() {
            break;
        }
    }
    assert!(rt.is_empty());
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, 0);
    assert_eq!(b.usage(ExecutionResource::InputBytes).0, 0);
    assert_eq!(h.usage(ExecutionResource::InputBytes).0, baseline);
    assert!(rt.close().is_ok());
    assert!(assembly.close().is_ok());
    assert_eq!(h.usage(ExecutionResource::InputBytes).0, 0);
}

#[test]
fn task_default_source_and_busy_retirement_keep_actual_charge_until_remove_receipt() {
    let shared = fake_shared();
    let (h, a, b) = funding_accounts(4, 1, 1);
    let assembly =
        Assembly::new(FakeSet::new(&shared), &h).unwrap_or_else(|_| panic!("assembly failed"));
    let mut rt = Runtime::<SourceTask, FakeSet>::new(assembly.bind().unwrap(), 4, 8, &h).unwrap();
    let baseline = h.usage(ExecutionResource::InputBytes);
    let id = rt
        .spawn(TaskSubmission::new(
            SourceTask {
                handle: Handle::from_parts(9, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            1,
            a.clone(),
        ))
        .unwrap();
    let mut world = World::default();
    for _ in 0..30 {
        rt.turn(&mut world, 1).unwrap();
        if shared.registered().len() == 1 {
            break;
        }
    }
    let paid = a.usage(ExecutionResource::InputBytes).0;
    assert!(paid > 0);
    assert_eq!(h.usage(ExecutionResource::InputBytes), baseline);
    assert_eq!(b.usage(ExecutionResource::InputBytes).0, 0);
    shared.set_remove_busy(2);
    rt.queue.begin_finalizing(id);
    for _ in 0..20 {
        rt.turn(&mut world, 1).unwrap();
        if shared.inner.borrow().remove_busy < 2 {
            break;
        }
    }
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, paid);
    assert_eq!(a.usage(ExecutionResource::Task).0, 1);
    shared.clock_error.set(Some(SystemCallError::InternalError));
    let failure = rt.turn(&mut world, 1).unwrap_err();
    assert_eq!(failure.error, SystemCallError::InternalError);
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, paid);
    shared.clock_error.set(None);
    for _ in 0..100 {
        shared.now.set(shared.now.get().saturating_add(1_000_000));
        rt.turn(&mut world, 1).unwrap();
        if rt.is_empty() {
            break;
        }
    }
    assert!(rt.is_empty());
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, 0);
    assert_eq!(a.usage(ExecutionResource::Task).0, 0);
    assert_eq!(h.usage(ExecutionResource::InputBytes), baseline);
    assert!(rt.close().is_ok());
    assert!(assembly.close().is_ok());
}

#[test]
fn scoped_requests_refuse_capacity_with_complete_frozen_submission() {
    let (_, a, b) = funding_accounts(4, 1, 1);
    let mut commands = CommandBuffer::<ToyTask>::new().unwrap();
    {
        let mut requests = Requests::new(&a, &mut commands.operations);
        for _ in 0..REQUEST_CAPACITY {
            requests.wake(1).unwrap();
        }
        let returned = requests
            .spawn(TaskSubmission::new(ToyTask { turns: 7 }, 3, b.clone()))
            .unwrap_err();
        assert_eq!(returned.funding().account_id(), b.account_id());
        let (task, maximum, _) = returned.into_parts();
        assert_eq!((task.turns, maximum), (7, 3));
    }
    commands.operations.clear();
    {
        let mut requests = Requests::new(&a, &mut commands.operations);
        assert!(
            requests
                .register(
                    SourcePlan::new(Handle::from_parts(20, 1), ObjectSignals::CLOSED),
                    7
                )
                .is_ok()
        );
    }
    // 再借给 B 不改变 A 已提交命令；Gate 会收到 A 的拥有快照。
    {
        let _requests = Requests::new(&b, &mut commands.operations);
    }
    let RequestOperation::Source(SourceRequest::Register { funding, .. }) =
        commands.pop_front().unwrap()
    else {
        panic!("expected registration");
    };
    assert_eq!(funding.account_id(), a.account_id());
}

struct RejectFundedSpawn {
    submitted: bool,
    child: AccountView<ExecutionResource>,
}
impl core::fmt::Debug for RejectFundedSpawn {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RejectFundedSpawn")
            .field("submitted", &self.submitted)
            .finish()
    }
}
impl Task<World> for RejectFundedSpawn {
    type Family = Self;
    fn advance(
        &mut self,
        _: u64,
        _: &mut World,
        requests: &mut Requests<'_, Self>,
        _: &mut Input<'_>,
        _: usize,
    ) -> Result<Advance, SystemCallError> {
        if !self.submitted {
            self.submitted = true;
            requests
                .spawn(TaskSubmission::new(
                    Self {
                        submitted: true,
                        child: self.child.clone(),
                    },
                    2,
                    self.child.clone(),
                ))
                .unwrap();
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Complete,
        })
    }
    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        let RequestFailure::Spawn { submission, error } = failure else {
            panic!("expected complete submission refusal");
        };
        assert_eq!(submission.funding().account_id(), self.child.account_id());
        let (task, maximum, _) = submission.into_parts();
        assert!(task.submitted);
        assert_eq!(maximum, 2);
        world.refusals.push(error);
    }
    fn stop(&mut self, _: &mut World) {}
}
#[test]
fn rejected_gate_returns_child_funding_after_parent_advance_fails() {
    let shared = fake_shared();
    let (h, a, b) = funding_accounts(4, 1, 1);
    let assembly =
        Assembly::new(FakeSet::new(&shared), &h).unwrap_or_else(|_| panic!("assembly failed"));
    let mut rt =
        Runtime::<RejectFundedSpawn, FakeSet>::new(assembly.bind().unwrap(), 4, 8, &h).unwrap();
    let id = rt
        .spawn(TaskSubmission::new(
            RejectFundedSpawn {
                submitted: false,
                child: b.clone(),
            },
            0,
            a.clone(),
        ))
        .unwrap();
    let mut world = World::default();
    for _ in 0..50 {
        if let Err(failure) = rt.turn(&mut world, 1) {
            assert_eq!(failure.task, id);
            break;
        }
    }
    for _ in 0..50 {
        rt.turn(&mut world, 1).unwrap();
        if !world.refusals.is_empty() {
            break;
        }
    }
    assert_eq!(world.refusals, [SystemCallError::IllegalArgument]);
    assert_eq!(b.usage(ExecutionResource::Task).0, 0);
    assert_eq!(b.usage(ExecutionResource::InputBytes).0, 0);
    rt.queue.begin_finalizing(id);
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if rt.is_empty() {
            break;
        }
    }
    assert!(rt.is_empty());
    assert!(rt.close().is_ok());
    assert!(assembly.close().is_ok());
}

#[test]
fn active_source_clock_failure_after_kernel_remove_retains_charge_until_receipt_without_reacquiring()
 {
    let shared = fake_shared();
    let (h, a, _) = funding_accounts(4, 1, 1);
    let assembly =
        Assembly::new(FakeSet::new(&shared), &h).unwrap_or_else(|_| panic!("assembly failed"));
    let mut rt = Runtime::<SourceTask, FakeSet>::new(assembly.bind().unwrap(), 4, 8, &h).unwrap();
    let id = rt
        .spawn(TaskSubmission::new(
            SourceTask {
                handle: Handle::from_parts(9, 1),
                requested: false,
                events_seen: 0,
                complete_after: 100,
            },
            1,
            a.clone(),
        ))
        .unwrap();
    let mut world = World::default();
    for _ in 0..30 {
        rt.turn(&mut world, 1).unwrap();
        if shared.registered().len() == 1 {
            break;
        }
    }
    let (paid, limit) = a.usage(ExecutionResource::InputBytes);
    assert!(paid > 0);
    let exhausted = a
        .acquire(ExecutionResource::InputBytes, limit - paid)
        .unwrap();
    shared.remove_clock_error.set(true);
    assert_eq!(
        rt.drop_source(&mut world, 1),
        Err(SystemCallError::InternalError)
    );
    assert!(
        shared.inner.borrow().members.is_empty(),
        "real SourceOps removal already succeeded"
    );
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, limit);
    assert_eq!(rt.pending_sources(), 1);
    shared.clock_error.set(None);
    for _ in 0..100 {
        shared.now.set(shared.now.get().saturating_add(1_000_000));
        rt.turn(&mut world, 1).unwrap();
        if rt.pending_sources() == 0 {
            break;
        }
    }
    assert_eq!(rt.pending_sources(), 0);
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, limit - paid);
    assert_eq!(shared.removes(), [1]);
    drop(exhausted);
    assert_eq!(a.usage(ExecutionResource::InputBytes).0, 0);
    rt.queue.begin_finalizing(id);
    for _ in 0..100 {
        rt.turn(&mut world, 1).unwrap();
        if rt.is_empty() {
            break;
        }
    }
    assert!(rt.is_empty());
    assert!(rt.close().is_ok());
    assert!(assembly.close().is_ok());
}
