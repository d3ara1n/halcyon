//! Tunnel 数据面：失败 owner、几何、生命周期与阻塞流读取任务。
use super::*;

pub(super) fn wait_supervision_signal(
    tunnel: &mut blocking::Consumer,
    label: &'static str,
) -> Result<libkernel::shared::wait::WaitResult, &'static str> {
    for attempt in 1..=DEFAULT_SUPERVISION_POLICY.wait_attempts {
        match tunnel.wait_peer_closed(DEFAULT_SUPERVISION_POLICY.wait_timeout_ms) {
            Ok(result) if result.observed != ObjectSignals::NONE => return Ok(result),
            Ok(result) if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) => {}
            Ok(result) => {
                debug!(
                    "{} supervision wait returned no signal at attempt {}: {:?}",
                    label, attempt, result
                );
                return Err("supervision wait returned no signal");
            }
            Err(error) => {
                debug!(
                    "{} supervision wait failed at attempt {}: {:?}",
                    label, attempt, error
                );
                return Err("supervision wait failed");
            }
        }
    }
    debug!(
        "{} supervision wait exhausted {} attempts",
        label, DEFAULT_SUPERVISION_POLICY.wait_attempts
    );
    Err("supervision wait timed out")
}
/// 真实失败保存 owner；最后事件授权关闭不撤销已接受的普通 View。
pub(super) fn test_tunnel_failure_owners() {
    use libkernel::{
        ipc::invitation::{CreateFailure, Invitation},
        memory_pool::MemoryPool,
        mm::{MappedRegion, Placement},
        shared::mem::MemoryProtection,
    };
    let parent = duplicate(
        memory_pool(),
        Rights::CREATE | Rights::READ | Rights::DUPLICATE,
    )
    .unwrap();
    // SAFETY: duplicate 新建唯一 raw owner，交给 typed Pool 管理。
    let parent = unsafe { MemoryPool::from_handle(parent) };
    // 子域承担 Tunnel 存储；其余供给留给调用者域的 View/操作增长，不复制内核布局成本。
    let metadata = parent.query().unwrap().metadata.available / 2;
    let child = parent
        .derive(
            erhino_shared::memory_pool::MemoryPoolBudget { pages: 4, metadata },
            Rights::CREATE | Rights::READ | Rights::DUPLICATE,
        )
        .unwrap()
        .into_handle();
    let observation = duplicate(child, Rights::READ).unwrap();
    // SAFETY: 独立只读查询 owner；付款授权 child 由本函数在末尾关闭。
    let pool = unsafe { MemoryPool::from_handle(observation) };
    assert!(matches!(
        Invitation::create(child, 0),
        Err(CreateFailure::System(SystemCallError::IllegalArgument))
    ));
    assert!(matches!(
        Invitation::create(child, 5 * 4096),
        Err(CreateFailure::System(SystemCallError::QuotaExceeded))
    ));
    assert_eq!(
        pool.query().unwrap().allocated,
        0,
        "failed Tunnel retained page charge"
    );
    let (endpoint, invitation, bytes) = Invitation::create(child, 4096).unwrap();
    let occupied = MappedRegion::map_anonymous(
        bytes,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .unwrap();
    let address = occupied.usable().unwrap().start;
    let failure = librunnel::MappedEndpoint::map(
        endpoint,
        bytes,
        Placement::FixedEmpty {
            usable_start: address,
        },
    )
    .expect_err("Tunnel Map replaced an occupied range");
    assert_eq!(failure.error, SystemCallError::AddressConflict);
    occupied.unmap().unwrap();
    let endpoint = failure.endpoint;
    let mapping = MappedRegion::map_endpoint(
        &endpoint,
        bytes,
        MemoryProtection::ReadWrite,
        Placement::FixedEmpty {
            usable_start: address,
        },
    )
    .unwrap();
    {
        // SAFETY: 唯一 MappedRegion 持有已发布的 RW 页；本作用域不解除映射。
        let memory = unsafe { libkernel::shared_memory::SharedMemory::new(address, bytes) };
        memory.store_u64(0, 0x5120_fade, core::sync::atomic::Ordering::Release);
    }
    endpoint.close().unwrap();
    let failure = invitation
        .attach()
        .expect_err("Attach accepted a closed creator");
    assert_eq!(failure.error, SystemCallError::ObjectClosed);
    failure.invitation.close().unwrap();
    assert_eq!(
        pool.query().unwrap().allocated,
        1,
        "Endpoint Close revoked a live View"
    );
    {
        // SAFETY: 两端授权已关，但 mapping 独立持 use；它在本次访问后才消费。
        let memory = unsafe { libkernel::shared_memory::SharedMemory::new(address, bytes) };
        assert_eq!(
            memory.load_u64(0, core::sync::atomic::Ordering::Acquire),
            0x5120_fade
        );
    }
    mapping.unmap().unwrap();
    let deadline = libkernel::time::timeout_millis(5_000).unwrap();
    // root 放弃由出生记录自主推进；页退款须同时越过 root 与 View 两份退休责任。
    while {
        let snapshot = pool.query().unwrap();
        snapshot.allocated != 0 || snapshot.metadata.allocated != 0
    } {
        assert!(
            !libkernel::time::expired(deadline).unwrap(),
            "Tunnel View retirement did not refund"
        );
        libkernel::time::sleep_until(libkernel::time::timeout_millis(1).unwrap()).unwrap();
    }
    unsafe { close(child) }.unwrap();
    pool.close();
    debug!(
        "Tunnel failure owner checks passed: quota rollback, Map conflict, Attach rejection, independent View refund"
    );
}

/// 实际 Running 调用链验证取整、双端不同 VA、完整共享范围与关闭后存活端。
pub(super) fn test_tunnel_geometry() {
    use core::sync::atomic::Ordering;
    use libkernel::mm::Placement;
    for bytes in [1usize, 4096, 4097, 8192, 12288, 512 * 4096] {
        let (endpoint, invitation, mapped_bytes) =
            libkernel::ipc::invitation::Invitation::create(memory_pool(), bytes)
                .expect("Tunnel geometry Create failed");
        let creator = librunnel::MappedEndpoint::map(endpoint, mapped_bytes, Placement::Anywhere)
            .expect("Tunnel geometry Map failed");
        let geometry = creator.geometry();
        assert_eq!(geometry.bytes(), bytes.div_ceil(4096) * 4096);
        for page in 0..geometry.bytes() / 4096 {
            creator
                .memory()
                .store_u64(page * 4096, page as u64 + 1, Ordering::Release);
        }
        let endpoint = invitation.attach().expect("Tunnel geometry Attach failed");
        let peer = librunnel::MappedEndpoint::map(endpoint, mapped_bytes, Placement::Anywhere)
            .expect("Tunnel geometry peer Map failed");
        assert_ne!(geometry.base(), peer.geometry().base());
        assert_eq!(geometry.bytes(), peer.geometry().bytes());
        for page in 0..geometry.bytes() / 4096 {
            assert_eq!(
                peer.memory().load_u64(page * 4096, Ordering::Acquire),
                page as u64 + 1
            );
        }
        let peer_base = peer.geometry().base();
        creator
            .close()
            .expect("Tunnel geometry creator close failed");
        assert_eq!(
            peer.memory()
                .load_u64(geometry.bytes() - 4096, Ordering::Acquire),
            (geometry.bytes() / 4096) as u64
        );
        peer.close().expect("Tunnel geometry peer close failed");
        for address in [geometry.base(), peer_base] {
            let region = libkernel::mm::MappedRegion::map_anonymous(
                geometry.bytes(),
                0,
                0,
                libkernel::shared::mem::MemoryProtection::ReadWrite,
                Placement::FixedEmpty {
                    usable_start: address,
                },
            )
            .expect("Tunnel geometry left a mapping behind");
            region.unmap().expect("Tunnel geometry reuse Unmap failed");
        }
    }
    debug!("Tunnel Running geometry checks passed: six lengths");
}

pub(super) fn test_tunnel_lifecycle() {
    use libkernel::ipc::invitation::Invitation;
    for _ in 0..TUNNEL_STRESS {
        let (abandoned, invitation, _) = Invitation::create(memory_pool(), TUNNEL_BYTES)
            .expect("lifecycle Tunnel create failed");
        // SAFETY: 只借未消费的Invitation验证其不提供等待来源，不取得第二owner；
        // WAIT模板位约束Attach后Endpoint，不能把它当成Invitation自己的观察来源。
        assert_eq!(
            wait_until(
                unsafe { invitation.raw_handle() },
                ObjectSignals::CLOSED,
                libkernel::time::Deadline::at(0)
            ),
            Err(SystemCallError::WrongObjectType)
        );
        invitation.close().expect("Invitation close failed");
        assert!(
            abandoned
                .events()
                .wait(ObjectSignals::PEER_CLOSED, 0)
                .unwrap()
                .observed
                .intersects(ObjectSignals::PEER_CLOSED)
        );
        abandoned.close().expect("lifecycle Endpoint close failed");
        let (creator, invitation, _) = Invitation::create(memory_pool(), TUNNEL_BYTES).unwrap();
        creator.close().unwrap();
        let failure = invitation
            .attach()
            .expect_err("Attach accepted a closed creator");
        assert_eq!(failure.error, SystemCallError::ObjectClosed);
        failure
            .invitation
            .close()
            .expect("failed Attach lost Invitation owner");
    }
    debug!("Tunnel abandonment and closed-creator Attach checks passed");
}

pub(super) const KIND_STREAM_READ: SourceKind = 1;
#[derive(Default)]
pub(super) struct ReadWorld {
    pub(super) tunnel: Option<blocking::Consumer>,
    pub(super) buffer: alloc::vec::Vec<u8>,
    pub(super) filled: usize,
    pub(super) failed: bool,
}
/// 消费侧读取任务：arm/poll 三条件接入，按预算推进，EOF 即完成。
pub(super) struct StreamReadTask {
    source: Option<SourceId>,
    arm_failed: Option<SystemCallError>,
    stopping: bool,
    removing: bool,
}
impl StreamReadTask {
    pub(super) const fn new() -> Self {
        Self {
            source: None,
            arm_failed: None,
            stopping: false,
            removing: false,
        }
    }
}
impl Task<ReadWorld> for StreamReadTask {
    type Family = StreamReadTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut ReadWorld,
        requests: &mut Requests<'_, StreamReadTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(source) = self.source {
                if !self.removing {
                    requests.remove(source)?;
                    self.removing = true;
                }
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if let Some(tunnel) = world.tunnel.take() {
                match tunnel.close() {
                    Ok(()) => {}
                    Err((tunnel, error)) => {
                        world.tunnel = Some(tunnel);
                        debug!("stream read stop close failed: {:?}", error);
                        return Err(SystemCallError::InternalError);
                    }
                }
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        let tunnel = world
            .tunnel
            .as_mut()
            .expect("stream read world lost its tunnel");
        if let Some(error) = self.arm_failed.take() {
            debug!("stream read source registration failed: {:?}", error);
            world.failed = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        let mut source = None;
        let mut replace_source = false;
        while let Some(event) = input.pull() {
            if event.kind != KIND_STREAM_READ {
                continue;
            }
            source = Some(event.source);

            match tunnel.poll(event.observed) {
                Ok(Some(ConsumerReady::Started))
                | Ok(Some(ConsumerReady::Readable {
                    peer_started: true, ..
                })) => replace_source = true,
                Ok(Some(ConsumerReady::EofDrained)) => {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    debug!("stream read poll failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        let mut work = 0;
        while world.filled < world.buffer.len() && world.filled < STREAM_LEN && work < budget {
            match tunnel.read(&mut world.buffer[world.filled..]) {
                Ok(0) => break,
                Ok(count) => {
                    world.filled += count;
                    work += 1;
                }
                Err(error) => {
                    debug!("stream read failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if world.failed {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if world.filled >= STREAM_LEN {
            // 已读满目标量：EOF 判定后交付，否则等终末事件。
            match tunnel.eof_reached() {
                Ok(true) => {
                    return Ok(Advance {
                        work_done: work.max(1),
                        step: Step::Complete,
                    });
                }
                Ok(false) => {}
                Err(error) => {
                    debug!("stream eof check failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if replace_source {
            let source = source.expect("Started event lost its source");
            requests.remove(source)?;
            self.source = None;
        } else if let Some(source) = source {
            self.source = Some(source);
        }
        if self.source.is_none() {
            let plan = match tunnel.wait_plan() {
                Ok(plan) => plan,
                Err(error) => {
                    debug!("stream wait plan failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            };
            requests
                .register(plan, KIND_STREAM_READ)
                .map_err(|_| SystemCallError::ReachLimit)?;
        }
        Ok(Advance {
            work_done: work.max(1),
            step: Step::Parked,
        })
    }

    fn registered(&mut self, _world: &mut ReadWorld, kind: SourceKind, source: SourceId) {
        if kind == KIND_STREAM_READ {
            self.source = Some(source);
        }
    }
    fn unregistered(&mut self, _world: &mut ReadWorld, _kind: SourceKind, source: SourceId) {
        if self.source == Some(source) {
            self.source = None;
            self.removing = false;
        }
    }

    fn refused(&mut self, _world: &mut ReadWorld, failure: RequestFailure<StreamReadTask>) {
        let error = match failure {
            RequestFailure::Spawn { error, .. }
            | RequestFailure::Source { error, .. }
            | RequestFailure::Wake { error, .. } => error,
        };
        self.arm_failed = Some(error);
    }

    fn stop(&mut self, _world: &mut ReadWorld) {
        self.stopping = true;
    }
}
