//! Runnel 对内核授权与普通 View 的组合 owner；事件关闭与撤映射分别结算。

use erhino_shared::{call::SystemCallError, mem::MemoryProtection};
use libkernel::{
    ipc::tunnel::{Endpoint, EndpointEvents},
    mm::{MappedRegion, Placement},
    shared_memory::SharedMemory,
};

#[derive(Clone, Copy, Debug)]
pub struct MappingGeometry {
    base: usize,
    bytes: usize,
}
impl MappingGeometry {
    pub fn base(self) -> usize {
        self.base
    }
    pub fn bytes(self) -> usize {
        self.bytes
    }
}
#[derive(Debug)]
pub struct MappedEndpoint {
    endpoint: Option<Endpoint>,
    mapping: Option<MappedRegion>,
    geometry: MappingGeometry,
}
#[derive(Debug)]
pub struct MapFailure {
    pub endpoint: Endpoint,
    pub error: SystemCallError,
}
impl MappedEndpoint {
    pub fn map(endpoint: Endpoint, bytes: usize, placement: Placement) -> Result<Self, MapFailure> {
        match MappedRegion::map_endpoint(&endpoint, bytes, MemoryProtection::ReadWrite, placement) {
            Ok(mapping) => {
                let usable = mapping
                    .usable()
                    .expect("Map succeeded without a usable range");
                let geometry = MappingGeometry {
                    base: usable.start,
                    bytes: usable.end - usable.start,
                };
                // 地址几何来自普通 Map 的已发布结果；不从事件状态推断。
                Ok(Self {
                    endpoint: Some(endpoint),
                    mapping: Some(mapping),
                    geometry,
                })
            }
            Err(error) => Err(MapFailure { endpoint, error }),
        }
    }
    pub fn geometry(&self) -> MappingGeometry {
        self.geometry
    }
    pub fn memory(&self) -> SharedMemory<'_> {
        assert!(self.mapping.is_some(), "Runnel mapping already released");
        // SAFETY: MappedRegion 唯一持有该 RW View，借用 self 排除 close/unmap；全段页对齐。
        unsafe { SharedMemory::new(self.geometry.base, self.geometry.bytes) }
    }
    pub fn events(&self) -> EndpointEvents<'_> {
        self.endpoint
            .as_ref()
            .expect("Endpoint already closed")
            .events()
    }
    pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
        if let Some(mapping) = self.mapping.take()
            && let Err((mapping, error)) = mapping.unmap()
        {
            self.mapping = Some(mapping);
            return Err((self, error));
        }
        if let Some(endpoint) = self.endpoint.take()
            && let Err((endpoint, error)) = endpoint.close()
        {
            self.endpoint = Some(endpoint);
            return Err((self, error));
        }
        Ok(())
    }
}
impl Drop for MappedEndpoint {
    fn drop(&mut self) {
        if let Some(mapping) = self.mapping.take() {
            // 析构时无安全借用；解除失败仍由进程空间回收持责任，不重试自旋。
            let _ = mapping.unmap();
        }
        drop(self.endpoint.take());
    }
}
