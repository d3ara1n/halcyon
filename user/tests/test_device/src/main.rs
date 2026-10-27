//! 正式设备功能消费者：只持Devices观察/FunctionAccess，不知道驱动PID或物理资源。
#![no_std]
extern crate alloc;
extern crate libruntime;
mod drivers;
mod reuse;
use alloc::{string::ToString, vec::Vec};
use erhino_shared::{object::ObjectSignals, time::Deadline};
use libdevice::{
    Error,
    identity::{BindingRef, DeviceId},
    protocol::{self as device, Command},
    registry::Scope,
};
use libkernel::ipc::{message::MailboxSender, wait};
use librpc::{Caller, Reply};
fn end() -> Deadline {
    libkernel::time::timeout_millis(30_000).expect("device client deadline failed")
}
fn call(caller: &mut Caller, source: &MailboxSender, command: Command<'_>) -> Reply {
    let deadline = end();
    let mut wire = [0; device::HEADER_LEN + device::BODY_MAX];
    let used = command
        .encode(deadline, &mut wire)
        .expect("device command invalid");
    caller
        .call(
            source,
            deadline,
            librpc::Request::new(device::ID, &wire[..used]).expect("device request failed"),
        )
        .expect("device RPC failed")
}
fn keytry(
    caller: &mut Caller,
    source: &MailboxSender,
    command: libhid::protocol::Command,
) -> Result<Reply, librpc::CallError> {
    let deadline = end();
    let mut wire = [0; libhid::protocol::HEADER_LEN + 8];
    let used = command
        .encode(deadline, &mut wire)
        .expect("HID command invalid");
    caller.call(
        source,
        deadline,
        librpc::Request::new(libhid::protocol::ID, &wire[..used]).expect("HID request failed"),
    )
}
fn keycall(
    caller: &mut Caller,
    source: &MailboxSender,
    command: libhid::protocol::Command,
) -> Reply {
    keytry(caller, source, command).expect("HID RPC failed")
}
fn info(reply: &Reply, id: DeviceId) -> libdevice::registry::DeviceInfo<'_> {
    device::decode_device(&reply.payload, id, reply.handles.remaining())
        .expect("device snapshot invalid")
}
fn find_excluding(
    caller: &mut Caller,
    source: &MailboxSender,
    excluded: &[DeviceId],
) -> Option<(BindingRef, MailboxSender)> {
    let mut after = 0;
    let mut last = 0;
    loop {
        let reply = call(
            caller,
            source,
            Command::Enumerate {
                scope: Scope::Root,
                after,
                end: last,
            },
        );
        let (header, body) = device::Header::decode(&reply.payload).expect("device page invalid");
        assert!(
            header.op == 14 && header.error.is_none() && reply.handles.remaining() == 0,
            "device page rejected"
        );
        let page = device::decode_scan(body, after, last).expect("device scan invalid");
        after = page.next;
        last = page.end;
        let id = page.device?;
        let reply = call(caller, source, Command::QueryDevice { id });
        let current = match device::decode_device(&reply.payload, id, reply.handles.remaining()) {
            Ok(i) => i,
            Err(Error::NotFound | Error::Closed | Error::StaleInstance) => continue,
            Err(e) => panic!("device query failed: {e:?}"),
        };
        if excluded.contains(&id)
            || !current.present
            || current.identity != libusb::descriptor::BOOT_KEYBOARD_IDENTITY
        {
            continue;
        }
        let Some(binding) = current.binding else {
            continue;
        };
        let mut reply = call(
            caller,
            source,
            Command::OpenFunction {
                reference: binding.reference,
                name: "keyboard",
            },
        );
        let (header, body) =
            device::Header::decode(&reply.payload).expect("function reply invalid");
        assert_eq!(header.op, 22, "function access opcode mismatch");
        if let Some(error) = header.error {
            assert!(
                body.is_empty() && reply.handles.remaining() == 0,
                "function access error carried output"
            );
            if matches!(
                error,
                Error::NotReady
                    | Error::Closed
                    | Error::NotFound
                    | Error::StaleEpoch
                    | Error::StaleInstance
            ) {
                libkernel::debug!(
                    "DEVICE discovery candidate changed: serial={}, error={:?}",
                    id.serial(),
                    error
                );
                continue;
            }
            panic!(
                "function access rejected: op={}, error={:?}, slots={}",
                header.op,
                error,
                reply.handles.remaining()
            );
        }
        assert_eq!(
            reply.handles.remaining(),
            1,
            "function access success slot mismatch"
        );
        let mut reader = librpc::bytes::Reader::new(body);
        assert_eq!(
            reader.u64().expect("function protocol missing"),
            liboal::protocol::ID
        );
        assert_eq!(
            reader.bytes32().expect("function description missing"),
            b"hid:boot-keyboard"
        );
        reader.finish().expect("function trailing bytes");
        let function =
            MailboxSender::from_capability(reply.handles.take(0).expect("function entry missing"))
                .map_err(|f| f.error)
                .expect("function entry invalid")
                .0;
        return Some((binding.reference, function));
    }
}
fn find(
    caller: &mut Caller,
    source: &MailboxSender,
    old: Option<DeviceId>,
) -> Option<(BindingRef, MailboxSender)> {
    find_excluding(caller, source, old.as_slice())
}

fn oal(
    caller: &mut Caller,
    source: &MailboxSender,
    reference: BindingRef,
    function: &MailboxSender,
) {
    let mut names = Vec::new();
    let mut next = Some(reference.device());
    while let Some(id) = next {
        let reply = call(caller, source, Command::QueryDevice { id });
        let current = info(&reply, id);
        names.push(current.name.to_string());
        next = current.parent;
    }
    names.reverse();
    let mut path = names.join("/children/");
    path.push_str("/functions/keyboard");
    let request = liboal::protocol::Request::Lookup { path: &path };
    let deadline = end();
    let mut wire = [0; erhino_shared::message::PAYLOAD_MAX];
    let used = liboal::protocol::encode_request(&request, deadline, &mut wire)
        .expect("OAL request invalid");
    let mut reply = caller
        .call(
            source,
            deadline,
            librpc::Request::new(liboal::protocol::ID, &wire[..used]).expect("OAL request failed"),
        )
        .expect("OAL lookup failed");
    let response = liboal::protocol::decode_response(&reply.payload, &request, deadline, 1)
        .expect("OAL response invalid");
    assert!(
        matches!(response,liboal::protocol::Response::Boundary(liboal::Boundary::Provider(value)) if value.remaining.is_empty()),
        "function boundary missing"
    );
    let boundary = reply.handles.take(0).expect("OAL function missing");
    let a = boundary.description().expect("OAL identity failed");
    let b = function.description().expect("function identity failed");
    assert_eq!(
        (a.object_id, a.related_object_id),
        (b.object_id, b.related_object_id),
        "OAL bypassed function gate"
    );
    let root = liboal::protocol::Request::Lookup { path: "" };
    let deadline = end();
    let used = liboal::protocol::encode_request(&root, deadline, &mut wire)
        .expect("function lookup invalid");
    let reply = caller
        .call(
            function,
            deadline,
            librpc::Request::new(liboal::protocol::ID, &wire[..used])
                .expect("function lookup failed"),
        )
        .expect("function OAL unavailable");
    match liboal::protocol::decode_response(&reply.payload, &root, deadline, 0)
        .expect("function info invalid")
    {
        liboal::protocol::Response::Found(info) => {
            assert!(info.contains(libhid::KEYBOARD), "keyboard marker missing")
        }
        _ => panic!("keyboard OAL face missing"),
    }
}
fn main() {
    let startup = libruntime::startup::take().expect("device client startup missing");
    let (_, payload, mut grants) = startup.into_parts();
    assert!(
        matches!(payload.as_bytes(), b"" | b"reuse") && grants.remaining() == test_device::GRANTS,
        "device client startup invalid"
    );
    let source = MailboxSender::from_capability(
        grants
            .take(test_device::DEVICES)
            .expect("Devices grant missing"),
    )
    .map_err(|f| f.error)
    .expect("Devices grant invalid")
    .0;
    let drivers = MailboxSender::from_capability(
        grants
            .take(test_device::DRIVERS)
            .expect("Drivers grant missing"),
    )
    .map_err(|f| f.error)
    .expect("Drivers grant invalid")
    .0;
    let mut caller = Caller::new();
    drivers::authenticate(&mut caller, &drivers);
    for index in 0..test_device::BACKLOG_PORTS {
        let source = MailboxSender::from_capability(
            grants
                .take(test_device::REPLY_BASE + index)
                .expect("Drivers backlog grant missing"),
        )
        .map_err(|f| f.error)
        .expect("Drivers backlog role invalid")
        .0;
        drivers::backlog(&drivers, &source);
    }
    if payload.as_bytes() == b"reuse" {
        assert!(
            find(&mut caller, &source, None).is_none(),
            "keyboard unexpectedly present before reuse hotplug"
        );
        libkernel::debug!("DEVICE reuse starting: no keyboard function present");
        reuse::exercise(&mut caller, &source, &drivers);
        return;
    }
    let reply = call(
        &mut caller,
        &source,
        Command::WatchScope { scope: Scope::Root },
    );
    let watch = libdevice::client::Watch::from_reply(
        reply,
        source
            .description()
            .expect("Devices identity failed")
            .related_object_id,
        15,
    )
    .expect("Devices watch failed");
    assert!(
        find(&mut caller, &source, None).is_none(),
        "keyboard unexpectedly present before hotplug"
    );
    libkernel::debug!("DEVICE waiting first insertion: no keyboard function present");
    let mut old: Option<(DeviceId, MailboxSender)> = None;
    for round in 1..=2 {
        let (reference, function) = loop {
            if let Some(found) = find(&mut caller, &source, old.as_ref().map(|old| old.0)) {
                break found;
            }
            let reply = call(&mut caller, watch.control(), Command::QueryWatch);
            let state = libdevice::client::Watch::decode_reply(&reply, 17)
                .expect("Devices watch query failed");
            if state.pending != 0 {
                call(
                    &mut caller,
                    watch.control(),
                    Command::AckWatch {
                        sequence: state.sequence,
                    },
                );
                continue;
            }
            wait::wait_until(
                watch.observer().as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                end(),
            )
            .expect("device appearance deadline exceeded");
        };
        if let Some((previous, entry)) = &old {
            assert_ne!(
                *previous,
                reference.device(),
                "DeviceId reused after reconnect"
            );
            let deadline = end();
            let mut wire = [0; libhid::protocol::HEADER_LEN];
            let used = libhid::protocol::Command::Query
                .encode(deadline, &mut wire)
                .expect("old function query invalid");
            match caller.call(
                entry,
                deadline,
                librpc::Request::new(libhid::protocol::ID, &wire[..used])
                    .expect("old function request failed"),
            ) {
                Ok(reply) => {
                    let (header, body) = libhid::protocol::Header::decode(&reply.payload)
                        .expect("old function reply invalid");
                    assert!(
                        header.op == 2 && reply.handles.remaining() == 0,
                        "old function reply mismatched"
                    );
                    assert!(
                        header.error == Some(Error::Closed)
                            || (header.error.is_none()
                                && !libhid::Snapshot::decode(body)
                                    .expect("old keyboard state invalid")
                                    .0
                                    .connected),
                        "old function reached new connection"
                    );
                }
                Err(error) => assert_eq!(
                    error.cause,
                    librpc::CallCause::ServiceClosed,
                    "old function result unknown"
                ),
            }
        }
        if let Some((previous, entry)) = old.take() {
            drop(entry);
            libkernel::debug!(
                "DEVICE old function alias closed: serial={}",
                previous.serial()
            );
        }
        // 真实Subtree Watch保留原实例身份到完成核验；不把消失/运输失败当安全Released。
        let instance_watch = libdevice::client::Watch::from_reply(
            call(
                &mut caller,
                &source,
                Command::WatchScope {
                    scope: Scope::Subtree(reference.device()),
                },
            ),
            source
                .description()
                .expect("Devices identity failed")
                .related_object_id,
            15,
        )
        .expect("instance observation failed");
        oal(&mut caller, &source, reference, &function);
        let reply = keycall(&mut caller, &function, libhid::protocol::Command::Watch);
        let keywatch =
            libhid::client::Watch::from_reply(reply, &function).expect("keyboard watch invalid");
        libkernel::debug!(
            "DEVICE keyboard ready: round={}, serial={}",
            round,
            reference.device().serial()
        );
        let mut pressed = false;
        let mut press_sequence = 0;
        let mut released = false;
        loop {
            let reply = match keytry(
                &mut caller,
                keywatch.control(),
                libhid::protocol::Command::Query,
            ) {
                Ok(reply) => reply,
                Err(error) if released && error.cause == librpc::CallCause::ServiceClosed => break,
                Err(error) => panic!("keyboard query transport failed: {error:?}"),
            };
            let state =
                libhid::client::Watch::decode_reply(&reply, 2, 0).expect("keyboard query failed");
            if !state.state.connected {
                assert!(released, "keyboard detached before input confirmation");
                break;
            }
            if state.state.valid {
                if !pressed && state.state.keys == [4, 0, 0, 0, 0, 0] {
                    pressed = true;
                    press_sequence = state.state.sequence;
                    libkernel::debug!(
                        "DEVICE press observed: round={}, sequence={}",
                        round,
                        press_sequence
                    );
                }
                if pressed
                    && !released
                    && state.state.sequence > press_sequence
                    && state.state.keys == [0; 6]
                {
                    released = true;
                    libkernel::debug!(
                        "DEVICE release observed: round={}, sequence={}",
                        round,
                        state.state.sequence
                    );
                }
            }
            if let Err(error) = keytry(
                &mut caller,
                keywatch.control(),
                libhid::protocol::Command::Ack(state.state.sequence),
            ) {
                assert!(
                    released && error.cause == librpc::CallCause::ServiceClosed,
                    "keyboard acknowledgement transport failed"
                );
                break;
            }
            wait::wait_until(
                keywatch.observer().as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                end(),
            )
            .expect("keyboard input deadline exceeded");
        }
        loop {
            let reply = call(
                &mut caller,
                &source,
                Command::QueryDevice {
                    id: reference.device(),
                },
            );
            match device::decode_device(
                &reply.payload,
                reference.device(),
                reply.handles.remaining(),
            ) {
                Ok(current) if !current.present && current.binding.is_none() => break,
                Ok(_) => {}
                Err(e) => panic!("device retirement query failed: {e:?}"),
            }
            libkernel::time::sleep_until(
                libkernel::time::timeout_millis(1).expect("retirement timer failed"),
            )
            .expect("device retirement wait failed");
        }
        libkernel::debug!(
            "DEVICE disconnected and binding released: round={}, serial={}",
            round,
            reference.device().serial()
        );
        drop(instance_watch);
        old = Some((reference.device(), function));
    }
    libkernel::debug!("DEVICE hotplug keyboard function acceptance passed");
    drop(old);
}
