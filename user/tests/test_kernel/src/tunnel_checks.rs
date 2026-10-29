//! 内核 Tunnel 契约：失败 owner、映射几何与能力生命周期。
use super::*;

const TUNNEL_BYTES: usize = 3 * 4096;

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
