//! Tunnel 授权与事件的安全封装；内存视图由独立 MapToken 拥有。

use crate::call;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    proc::PROCESS_PAGE_SIZE,
    tunnel::{TunnelCreateRequest, TunnelCreateResult},
    wait::WaitResult,
    wait_set::MemberRequest,
};

#[derive(Debug)]
#[must_use = "an Endpoint owns its capability until close"]
pub struct Endpoint {
    handle: Option<Handle>,
}
impl Endpoint {
    pub(crate) fn from_handle(handle: Handle) -> Option<Self> {
        handle.is_valid().then_some(Self {
            handle: Some(handle),
        })
    }
    pub fn events(&self) -> EndpointEvents<'_> {
        EndpointEvents(self)
    }
    pub(crate) fn handle(&self) -> Handle {
        self.handle.expect("Endpoint already closed")
    }
    /// # Safety
    /// 不得消费此借用句柄或构造第二份 owner。
    pub unsafe fn raw_handle(&self) -> Handle {
        self.handle.expect("Endpoint already closed")
    }
    pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: self 唯一拥有该表项；关闭不撤销任何普通 View。
        match unsafe { super::object::close(self.handle.expect("Endpoint already closed")) } {
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
            // SAFETY: 唯一 owner 的最终释放；若失效，进程表回收负责剩余责任。
            let _ = unsafe { super::object::close(handle) };
        }
    }
}
pub struct EndpointEvents<'a>(&'a Endpoint);
impl EndpointEvents<'_> {
    pub fn notify(&self) -> Result<(), SystemCallError> {
        // SAFETY: Endpoint owner 保证来源表项寿命；内核验证 SIGNAL。
        unsafe { call::sys_tunnel_notify(self.0.handle.expect("Endpoint already closed")) }
    }
    pub fn acknowledge_data(&self) -> Result<(), SystemCallError> {
        // SAFETY: 同 notify。
        unsafe {
            call::sys_tunnel_acknowledge_data(self.0.handle.expect("Endpoint already closed"))
        }
    }
    pub fn register(
        &self,
        set: &super::wait_set::WaitSet,
        signals: ObjectSignals,
        cookie: u64,
    ) -> Result<u64, SystemCallError> {
        set.register(self.wait_item(signals, cookie))
    }
    pub fn wait_item(&self, signals: ObjectSignals, cookie: u64) -> MemberRequest {
        MemberRequest::new(
            self.0.handle.expect("Endpoint already closed"),
            signals,
            cookie,
        )
    }
    pub fn wait_until(
        &self,
        signals: ObjectSignals,
        deadline: crate::time::Deadline,
    ) -> Result<WaitResult, SystemCallError> {
        super::wait::wait_until(
            self.0.handle.expect("Endpoint already closed"),
            signals,
            deadline,
        )
    }
    pub fn wait(
        &self,
        signals: ObjectSignals,
        timeout: u64,
    ) -> Result<WaitResult, SystemCallError> {
        self.wait_until(signals, crate::time::timeout_millis(timeout)?)
    }
}

#[derive(Debug)]
pub(crate) struct CreateFailure {
    pub error: SystemCallError,
    pub endpoint: Option<Endpoint>,
    pub invitation: Option<Handle>,
}
pub(crate) fn create_owned(
    pool: Handle,
    bytes: usize,
) -> Result<(Endpoint, Handle, usize), CreateFailure> {
    let mut output = TunnelCreateResult::empty();
    let request =
        TunnelCreateRequest::new(pool, bytes as u64, core::ptr::addr_of_mut!(output) as u64);
    // SAFETY: 完整请求和结果在调用期间有效；输出失败时内核未公开表项。
    unsafe { call::sys_tunnel_create(&request) }.map_err(|error| CreateFailure {
        error,
        endpoint: None,
        invitation: None,
    })?;
    let endpoint = Endpoint::from_handle(output.endpoint);
    let invitation = output.invitation.is_valid().then_some(output.invitation);
    let expected = bytes
        .checked_add(PROCESS_PAGE_SIZE - 1)
        .map(|bytes| bytes / PROCESS_PAGE_SIZE * PROCESS_PAGE_SIZE);
    if endpoint.is_none()
        || invitation.is_none()
        || expected != usize::try_from(output.bytes).ok()
        || output.reserved != 0
    {
        return Err(CreateFailure {
            error: SystemCallError::InternalError,
            endpoint,
            invitation,
        });
    }
    Ok((
        endpoint.expect("validated endpoint disappeared"),
        output.invitation,
        output.bytes as usize,
    ))
}
/// # Safety
/// 必须唯一拥有 Invitation；成功消费它，失败保留。
pub(crate) unsafe fn attach(invitation: Handle) -> Result<Endpoint, SystemCallError> {
    // SAFETY: 所有权由调用者承担，内核只返回已出生 Endpoint 的表项。
    let handle = unsafe { call::sys_tunnel_attach(invitation)? };
    Endpoint::from_handle(handle).ok_or(SystemCallError::InternalError)
}
