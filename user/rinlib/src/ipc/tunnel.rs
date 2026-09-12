//! Endpoint 独占本地映射；Invitation 仍由内核一次性消费。

use crate::{call, mm::Placement, shared_memory::SharedMemory};
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{
    call::SystemCallError,
    mem::MemoryPlacement,
    object::{Handle, ObjectSignals},
    proc::{PROCESS_PAGE_SIZE, PROCESS_USER_TOP},
    tunnel::{
        TUNNEL_MAX_PAGES, TunnelAttachRequest, TunnelCreateRequest, TunnelCreateResult,
        TunnelEndpointResult,
    },
    wait::{WaitItem, WaitResult},
};

static ABANDONED_ENDPOINTS: AtomicUsize = AtomicUsize::new(0);
static LAST_CLEANUP_ERROR: AtomicUsize = AtomicUsize::new(SystemCallError::NoError as usize);

#[derive(Debug, Clone, Copy)]
pub struct EndpointCleanupSnapshot {
    pub abandoned: usize,
    pub last_error: SystemCallError,
}

pub fn cleanup_snapshot() -> EndpointCleanupSnapshot {
    EndpointCleanupSnapshot {
        abandoned: ABANDONED_ENDPOINTS.load(Ordering::Acquire),
        last_error: num_traits::FromPrimitive::from_usize(
            LAST_CLEANUP_ERROR.load(Ordering::Acquire),
        )
        .unwrap_or(SystemCallError::Unknown),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MappingGeometry {
    base: usize,
    bytes: usize,
}
impl MappingGeometry {
    pub fn base(&self) -> usize {
        self.base
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[derive(Debug)]
#[must_use = "an Endpoint owns its mapping until close or process drain"]
pub struct Endpoint {
    handle: Option<Handle>,
    geometry: MappingGeometry,
}

impl Endpoint {
    fn from_result(result: TunnelEndpointResult) -> Result<Self, SystemCallError> {
        let base = usize::try_from(result.base).map_err(|_| SystemCallError::InternalError)?;
        let bytes = usize::try_from(result.bytes).map_err(|_| SystemCallError::InternalError)?;
        if !result.endpoint.is_valid()
            || bytes == 0
            || bytes > TUNNEL_MAX_PAGES as usize * PROCESS_PAGE_SIZE
            || !base.is_multiple_of(PROCESS_PAGE_SIZE)
            || !bytes.is_multiple_of(PROCESS_PAGE_SIZE)
            || base
                .checked_add(bytes)
                .is_none_or(|end| end > PROCESS_USER_TOP)
        {
            if result.endpoint.is_valid() {
                // SAFETY: 本 syscall 新生成的 entry 尚未形成任何安全 owner。
                let _ = unsafe { super::object::close(result.endpoint) };
            }
            return Err(SystemCallError::InternalError);
        }
        Ok(Self {
            handle: Some(result.endpoint),
            geometry: MappingGeometry { base, bytes },
        })
    }

    pub fn geometry(&self) -> MappingGeometry {
        self.geometry
    }
    /// ```compile_fail
    /// let (endpoint, _) = rinlib::ipc::tunnel::create(4096, rinlib::mm::Placement::Anywhere).unwrap();
    /// let memory = endpoint.memory();
    /// let _ = endpoint.close();
    /// let _ = memory.len();
    /// ```
    pub fn memory(&self) -> SharedMemory<'_> {
        // SAFETY: owner 借用阻止 close；内核普通 Unmap/Protect 不得解除 object lease。
        unsafe { SharedMemory::new(self.geometry.base, self.geometry.bytes) }
    }
    pub fn events(&self) -> EndpointEvents<'_> {
        EndpointEvents(self)
    }

    /// # Safety
    /// 仅供原始 ABI 诊断；不得在 owner 存活期消费该 Handle 或构造另一个 owner。
    pub unsafe fn raw_handle(&self) -> Handle {
        self.handle.expect("Endpoint already closed")
    }

    /// 失败原样交还 owner；不隐含重试或把未关闭误认为成功。
    pub fn close(self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: 本 owner 唯一持有 entry，消费 self 排除仍存活的 memory 借用。
        let result = unsafe { super::object::close(self.handle.expect("Endpoint already closed")) };
        self.finish_close(result)
    }

    fn finish_close(
        mut self,
        result: Result<(), SystemCallError>,
    ) -> Result<(), (Self, SystemCallError)> {
        match result {
            Ok(()) => {
                self.handle = None;
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: 析构时已无 owner 的安全借用；失败 entry 留给进程 drain。
            if let Err(error) = unsafe { super::object::close(handle) } {
                LAST_CLEANUP_ERROR.store(error as usize, Ordering::Release);
                let _ =
                    ABANDONED_ENDPOINTS.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        Some(count.saturating_add(1))
                    });
            }
        }
    }
}

pub struct EndpointEvents<'a>(&'a Endpoint);
impl EndpointEvents<'_> {
    pub fn notify(&self) -> Result<(), SystemCallError> {
        // SAFETY: 借用 owner 保证 entry 存活，不消费映射。
        unsafe { call::sys_tunnel_notify(self.0.handle.expect("Endpoint already closed")) }
    }
    pub fn acknowledge_data(&self) -> Result<(), SystemCallError> {
        // SAFETY: 同 notify，仅确认本端 DATA。
        unsafe {
            call::sys_tunnel_acknowledge_data(self.0.handle.expect("Endpoint already closed"))
        }
    }
    pub fn wait(
        &self,
        signals: ObjectSignals,
        timeout: u64,
    ) -> Result<WaitResult, SystemCallError> {
        super::wait::wait_many(
            &[WaitItem::new(
                self.0.handle.expect("Endpoint already closed"),
                signals,
                0,
            )],
            timeout,
        )
    }
}

fn placement(placement: Placement) -> (u64, MemoryPlacement) {
    match placement {
        Placement::Anywhere => (0, MemoryPlacement::Anywhere),
        Placement::FixedEmpty { usable_start } => {
            (usable_start as u64, MemoryPlacement::FixedEmpty)
        }
    }
}

pub fn create(bytes: usize, policy: Placement) -> Result<(Endpoint, Handle), SystemCallError> {
    let mut output = TunnelCreateResult::empty();
    let (address, policy) = placement(policy);
    let request = TunnelCreateRequest::new(
        bytes as u64,
        address,
        core::ptr::addr_of_mut!(output) as u64,
        policy,
    );
    // SAFETY: 请求与结果在包括等待的整个 syscall 期间有效。
    unsafe { call::sys_tunnel_create(&request)? };
    let endpoint = Endpoint::from_result(output.local).inspect_err(|_| {
        // SAFETY: 尚未交付的本次创建 Invitation，失败时不能遗留其引用。
        if output.invitation.is_valid() {
            let _ = unsafe { super::object::close(output.invitation) };
        }
    })?;
    if !output.invitation.is_valid()
        || bytes
            .checked_add(PROCESS_PAGE_SIZE - 1)
            .map(|value| value / PROCESS_PAGE_SIZE * PROCESS_PAGE_SIZE)
            != Some(endpoint.geometry.bytes)
    {
        // SAFETY: Invitation 是本次创建、尚未运输的 entry。
        if output.invitation.is_valid() {
            let _ = unsafe { super::object::close(output.invitation) };
        }
        return Err(SystemCallError::InternalError);
    }
    Ok((endpoint, output.invitation))
}

/// 内核提交失败不消费 Invitation；成功后格式结果错误不恢复已消费的邀请。
pub fn attach(invitation: Handle, policy: Placement) -> Result<Endpoint, SystemCallError> {
    let mut output = TunnelEndpointResult::empty();
    let (address, policy) = placement(policy);
    let request = TunnelAttachRequest::new(
        invitation,
        address,
        core::ptr::addr_of_mut!(output) as u64,
        policy,
    );
    // SAFETY: 同 create；内核按 Invitation role 验证并一次性消费。
    unsafe { call::sys_tunnel_attach(&request)? };
    Endpoint::from_result(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_close_returns_exact_owner_for_retry() {
        // 不调用 host syscall 或访问映射；只验证关闭结果的 affine 状态转换。
        let endpoint = Endpoint {
            handle: Some(Handle::from_parts(1, 1)),
            geometry: MappingGeometry {
                base: 0x1000,
                bytes: 3 * PROCESS_PAGE_SIZE,
            },
        };
        let (returned, error) = endpoint
            .finish_close(Err(SystemCallError::ObjectBusy))
            .unwrap_err();
        assert_eq!(error, SystemCallError::ObjectBusy);
        assert_eq!(returned.handle, Some(Handle::from_parts(1, 1)));
        assert_eq!(returned.geometry.base(), 0x1000);
        assert_eq!(returned.geometry.bytes(), 3 * PROCESS_PAGE_SIZE);
        assert!(returned.finish_close(Ok(())).is_ok());
    }
}
