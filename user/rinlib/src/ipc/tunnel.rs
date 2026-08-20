use erhino_shared::{call::SystemCallError, mem::Address};

use crate::call::{sys_tunnel_build, sys_tunnel_dispose, sys_tunnel_link};

pub const TUNNEL_FIELD_SIZE: usize = 4096;

#[derive(Debug)]
pub enum TunnelError {
    Unknown,
    NotAccessible,
    IllegalAddress,
    OutOfMemory,
}

impl From<SystemCallError> for TunnelError {
    fn from(value: SystemCallError) -> Self {
        match value {
            SystemCallError::ObjectNotAccessible | SystemCallError::ObjectNotFound => {
                TunnelError::NotAccessible
            }
            SystemCallError::MemoryNotAccessible | SystemCallError::OutOfMemory => {
                TunnelError::OutOfMemory
            }
            _ => Self::Unknown,
        }
    }
}

#[expect(dead_code, reason = "Runnel 协议结构，隧道里程碑接入")]
pub struct Tunnel {
    key: usize,
    field: *mut u8,
}

impl Tunnel {
    fn from_address(key: usize, addr: Address) -> Result<Self, TunnelError> {
        if addr & (TUNNEL_FIELD_SIZE - 1) == 0 {
            Ok(Self {
                key,
                field: addr as *mut u8,
            })
        } else {
            Err(TunnelError::IllegalAddress)
        }
    }

    pub fn key(&self) -> usize {
        self.key
    }

    pub fn dispose(self) {
        // cleanup
        if let Ok(_) = unsafe { sys_tunnel_dispose(self.key) } {
            // ignore
        }
    }
}

pub fn make() -> Result<Tunnel, TunnelError> {
    match unsafe { sys_tunnel_build() } {
        Ok(id) => link(id),
        Err(err) => Err(err.into()),
    }
}

pub fn link(id: usize) -> Result<Tunnel, TunnelError> {
    match unsafe { sys_tunnel_link(id) } {
        Ok(addr) => Tunnel::from_address(id, addr),
        Err(err) => Err(err.into()),
    }
}

#[expect(dead_code, reason = "Runnel 协议结构，隧道里程碑接入")]
pub struct Runnel {
    inner: Tunnel,
}

impl Runnel {
}

impl From<Tunnel> for Runnel {
    fn from(inner: Tunnel) -> Self {
        Self { inner }
    }
}
