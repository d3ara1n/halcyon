//! 本轮具名功能根及不透明激活观察关联；能力数据与其描述在同一记录保存。
use super::*;

pub struct FunctionRegistration<'a> {
    pub name: &'a str,
    pub description: &'a [u8],
    pub endpoint: EndpointIdentity,
}
pub struct FunctionInfo<'a, C> {
    pub name: &'a str,
    pub description: &'a [u8],
    pub entry: &'a C,
}
pub(super) struct Function<C> {
    reference: BindingRef,
    name: String,
    description: Vec<u8>,
    endpoint: EndpointIdentity,
    entry: C,
    _unit: Charge,
    _text: Charge,
}
pub(super) struct Activation<C> {
    pub(super) reference: BindingRef,
    pub(super) endpoint: EndpointIdentity,
    pub(super) protocol: u64,
    pub(super) entry: C,
}
impl<C> Registry<C> {
    /// 宿主先核实际 BindingControl，再使用原轮接口；重复只比较冻结身份与描述。
    pub fn install_function(
        &mut self,
        reference: BindingRef,
        request: FunctionRegistration<'_>,
        entry: C,
    ) -> Result<(), Error> {
        self.binding_usable(reference)?;
        liboal::path::validate_name(request.name).map_err(|_| Error::Invalid)?;
        if request.name.len() > super::oal::MAX_DISCOVERY_NAME {
            return Err(Error::Invalid);
        }
        if request
            .name
            .len()
            .checked_add(request.description.len())
            .is_none_or(|n| n > crate::protocol::BODY_MAX - 16)
        {
            return Err(Error::Invalid);
        }
        let mut after = 0;
        while let Some((id, function)) = self.functions.next_after(Some(&after)) {
            after = *id;
            let f = &function.value;
            if f.reference == reference && f.name == request.name {
                return if f.endpoint == request.endpoint && f.description == request.description {
                    Ok(())
                } else {
                    Err(Error::Conflict)
                };
            }
        }
        let source = self.binding_snapshot(reference)?.subject.context();
        let account = self.funding(source)?.domain().clone();
        let unit = account.acquire(DeviceResource::Function, 1)?;
        let mut text = account.acquire(
            DeviceResource::Bytes,
            request.name.len() + request.description.len(),
        )?;
        let mut name = String::new();
        name.try_reserve_exact(request.name.len())
            .map_err(|_| Error::Resource)?;
        name.push_str(request.name);
        let mut description = Vec::new();
        description
            .try_reserve_exact(request.description.len())
            .map_err(|_| Error::Resource)?;
        description.extend_from_slice(request.description);
        let extra = name.capacity() + description.capacity();
        if extra > request.name.len() + request.description.len() {
            // 用最终分配容量替换原预留；失败前尚未提交，临时存储仍由本调用释放。
            drop(text);
            text = account.acquire(DeviceResource::Bytes, extra)?;
        }
        let key = self.next_function.checked_add(1).ok_or(Error::Exhausted)?;
        let record = Function {
            reference,
            name,
            description,
            endpoint: request.endpoint,
            entry,
            _unit: unit,
            _text: text,
        };
        let record = prepare(&self.functions, key, record, &account)?;
        self.functions.insert_prepared(record);
        self.next_function = key;
        self.changed(reference.device());
        Ok(())
    }
    pub fn open_function(
        &self,
        context: u64,
        reference: BindingRef,
        name: &str,
    ) -> Result<FunctionInfo<'_, C>, Error> {
        self.authorize(
            context,
            Access::FUNCTIONS,
            Scope::Subtree(reference.device()),
            true,
        )?;
        self.binding_usable(reference)?;
        self.find_function(reference, name).ok_or(Error::NotFound)
    }
    fn find_function(&self, reference: BindingRef, name: &str) -> Option<FunctionInfo<'_, C>> {
        let mut after = 0;
        while let Some((id, f)) = self.functions.next_after(Some(&after)) {
            after = *id;
            if f.value.reference == reference && f.value.name == name {
                return Some(FunctionInfo {
                    name: &f.value.name,
                    description: &f.value.description,
                    entry: &f.value.entry,
                });
            }
        }
        None
    }
    pub(super) fn next_function(
        &self,
        reference: BindingRef,
        after: &str,
    ) -> Option<FunctionInfo<'_, C>> {
        if self.binding_usable(reference).is_err() {
            return None;
        }
        let mut cursor = 0;
        let mut found: Option<FunctionInfo<'_, C>> = None;
        while let Some((id, f)) = self.functions.next_after(Some(&cursor)) {
            cursor = *id;
            let f = &f.value;
            if f.reference == reference
                && f.name.as_str() > after
                && found.as_ref().is_none_or(|old| f.name.as_str() < old.name)
            {
                found = Some(FunctionInfo {
                    name: &f.name,
                    description: &f.description,
                    entry: &f.entry,
                });
            }
        }
        found
    }
    pub fn install_activation(
        &mut self,
        reference: BindingRef,
        endpoint: EndpointIdentity,
        protocol: u64,
        entry: C,
    ) -> Result<(), Error> {
        self.binding_usable(reference)?;
        if protocol == 0 {
            return Err(Error::Invalid);
        }
        if let Some(existing) = self.device(reference.device())?.activation.as_ref() {
            return if existing.reference == reference
                && existing.endpoint == endpoint
                && existing.protocol == protocol
            {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        self.device_mut(reference.device())?.activation = Some(Activation {
            reference,
            endpoint,
            protocol,
            entry,
        });
        self.changed(reference.device());
        Ok(())
    }
    pub fn activation(&self, context: u64, reference: BindingRef) -> Result<(u64, &C), Error> {
        self.authorize(
            context,
            Access::OBSERVE,
            Scope::Subtree(reference.device()),
            true,
        )?;
        self.binding_usable(reference)?;
        let value = self
            .device(reference.device())?
            .activation
            .as_ref()
            .ok_or(Error::NotReady)?;
        if value.reference != reference {
            return Err(Error::StaleEpoch);
        }
        Ok((value.protocol, &value.entry))
    }
    pub(super) fn remove_functions(&mut self, id: DeviceId) {
        let mut after = 0;
        while let Some((key, f)) = self.functions.next_after(Some(&after)) {
            let key = *key;
            if f.value.reference.device() == id {
                self.functions.remove(key);
            }
            after = key;
        }
        if let Ok(d) = self.device_mut(id) {
            d.activation = None;
        }
    }
}
