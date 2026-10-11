//! 同一 Dispatcher 的独立付款验收；复用 RootSupervisor 的运输责任与失败收束。
use super::*;
use libkernel::ipc::{capability::Capability, message::MailboxSender};
use librpc::dispatcher::{Dispatcher, ServiceOwner};
use libservice::protocol::{self as service, Request, Response, Status};

impl RootSupervisor {
    pub(crate) fn verify_dispatcher_funding(
        &mut self,
        directories: [&MailboxSender; 2],
    ) -> Result<(), &'static str> {
        let sources =
            Dispatcher::source_capacity(2).map_err(|_| "funding source capacity invalid")?;
        let fixed = Runtime::<InitTask, WaitSet>::fixed_input_budget(sources)
            .map_err(|_| "funding fixed capacity invalid")?;
        let private = Runtime::<InitTask, WaitSet>::source_input_budget(2)
            .map_err(|_| "funding private capacity invalid")?;
        let all = Runtime::<InitTask, WaitSet>::source_input_budget(sources)
            .map_err(|_| "funding total capacity invalid")?;
        let limits = [
            1,
            fixed.checked_add(all).ok_or("funding capacity overflow")?,
        ];
        let budget = Budget::new(&limits, 3).map_err(|_| "funding budget failed")?;
        let slots = [
            budget.slot(0).ok_or("funding task slot missing")?,
            budget.slot(1).ok_or("funding input slot missing")?,
        ];
        let bind = |limits: &[usize]| budget.account(limits)?.view::<ExecutionResource>(&slots);
        let host = bind(&limits).map_err(|_| "funding host account failed")?;
        let payers = [
            bind(&[0, private]).map_err(|_| "funding account A failed")?,
            bind(&[0, private]).map_err(|_| "funding account B failed")?,
        ];
        self.start_clients(
            Vec::new(),
            2,
            0,
            ReadWorld {
                client_quiescent: true,
                ..ReadWorld::default()
            },
            Some(host.clone()),
        )?;
        // 此后失败由原 ReadDuty 保持运输 owner，RootSupervisor 的失败路径继续退休。
        let deadline =
            libkernel::time::timeout_millis(5000).map_err(|_| "funding call deadline failed")?;
        let command = Request::GetEntry {
            path: "acceptance.unpublished",
        };
        let mut bytes = [0; service::HEADER_LEN + service::BODY_MAX];
        let used = service::encode_request(&command, deadline, &mut bytes)
            .map_err(|_| "funding request encoding failed")?;
        let mut calls = [None; 2];
        for (index, directory) in directories.into_iter().enumerate() {
            let handle = libkernel::ipc::object::duplicate(
                directory.as_handle(),
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            )
            .map_err(|_| "funding directory duplicate failed")?;
            // SAFETY: duplicate 刚交付独占新表项，此处创建其唯一关闭 owner。
            let capability = unsafe { Capability::from_raw(handle) };
            let sender = MailboxSender::from_capability(capability)
                .map_err(|_| "funding directory role invalid")?
                .0;
            let rpc = self
                .read_world()
                .rpc
                .as_mut()
                .expect("funding transport remains owned");
            calls[index] = Some(
                rpc.begin(
                    ServiceOwner::Owned(sender),
                    deadline,
                    librpc::Request::new(service::ID, &bytes[..used])
                        .map_err(|_| "funding request preparation failed")?,
                    &payers[index],
                )
                .map_err(|_| "funding call admission failed")?,
            );
        }
        assert_eq!(
            self.read_world()
                .rpc
                .as_ref()
                .expect("funding transport remains owned")
                .pending_count(),
            2
        );
        let mut observed = [false; 2];
        while calls.iter().any(Option::is_some) {
            self.turn_and_wait(Some(Await::RpcReply))
                .map_err(|_| "funding transport wait failed")?;
            let duty = self.read.as_mut().expect("funding duty remains owned");
            if duty.error.is_some() || duty.world.failed || duty.complete {
                return Err("funding transport stopped before completion");
            }
            for (index, call) in calls.iter_mut().enumerate() {
                observed[index] |= payers[index].usage(ExecutionResource::InputBytes).0 != 0;
                if let Some(txid) = *call
                    && let Some(completion) = duty
                        .world
                        .rpc
                        .as_mut()
                        .expect("funding transport remains owned")
                        .take(txid)
                {
                    let reply = completion.result.map_err(|_| "funding RPC failed")?;
                    let response = service::decode_response(
                        &reply.payload,
                        &command,
                        deadline,
                        reply.handles.remaining() as u64,
                    )
                    .map_err(|_| "funding response invalid")?;
                    if !matches!(response, Response::Error(Status::NotFound)) {
                        return Err("funding lookup unexpectedly found entry");
                    }
                    *call = None;
                }
            }
        }
        assert!(
            observed.into_iter().all(|seen| seen),
            "private RPC source charges missing"
        );
        for payer in &payers {
            assert_eq!(
                payer.usage(ExecutionResource::InputBytes).0,
                0,
                "private RPC sources not refunded"
            );
            assert_eq!(
                payer.usage(ExecutionResource::Task).0,
                0,
                "shared RPC task charged to caller"
            );
        }
        assert_eq!(
            host.usage(ExecutionResource::Task).0,
            1,
            "RPC task sponsor changed"
        );
        assert_eq!(
            host.usage(ExecutionResource::InputBytes).0,
            fixed
                + Runtime::<InitTask, WaitSet>::source_input_budget(1)
                    .map_err(|_| "funding shared capacity invalid")?,
            "shared reply source sponsor changed",
        );
        let duty = self.read.as_mut().expect("funding duty remains owned");
        duty.world
            .rpc
            .as_mut()
            .expect("funding transport remains owned")
            .stop(&mut ());
        duty.runtime
            .as_mut()
            .expect("funding runtime remains owned")
            .wake(duty.world.rpc_task.expect("funding task remains owned"))
            .map_err(|_| "funding stop wake failed")?;
        self.await_read()?;
        self.close_read()?;
        assert!(
            budget.is_empty(),
            "RPC funding remained after owner retirement"
        );
        debug!("RPC Dispatcher independent payer sources and refund passed");
        Ok(())
    }
}
