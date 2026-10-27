//! 同一真实HID进程的多Binding功能消费者；仅从原Devices取得入口。
use super::*;
struct Keyboard {
    reference: BindingRef,
    function: MailboxSender,
    keys: libhid::client::Watch,
    instance: libdevice::client::Watch,
}
impl Keyboard {
    fn find(caller: &mut Caller, source: &MailboxSender, excluded: &[DeviceId]) -> Self {
        let root = libdevice::client::Watch::from_reply(
            call(caller, source, Command::WatchScope { scope: Scope::Root }),
            source
                .description()
                .expect("Devices identity failed")
                .related_object_id,
            15,
        )
        .expect("reuse discovery watch failed");
        let (reference, function) = loop {
            if let Some(found) = find_excluding(caller, source, excluded) {
                break found;
            }
            let reply = call(caller, root.control(), Command::QueryWatch);
            let state = libdevice::client::Watch::decode_reply(&reply, 17)
                .expect("reuse discovery query failed");
            if state.pending != 0 {
                call(
                    caller,
                    root.control(),
                    Command::AckWatch {
                        sequence: state.sequence,
                    },
                );
                continue;
            }
            wait::wait_until(
                root.observer().as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                end(),
            )
            .expect("reuse device appearance deadline exceeded");
        };
        let instance = libdevice::client::Watch::from_reply(
            call(
                caller,
                source,
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
        .expect("reuse instance observation failed");
        oal(caller, source, reference, &function);
        let keys = libhid::client::Watch::from_reply(
            keycall(caller, &function, libhid::protocol::Command::Watch),
            &function,
        )
        .expect("reuse keyboard Watch invalid");
        Self {
            reference,
            function,
            keys,
            instance,
        }
    }
    fn state(&self, caller: &mut Caller) -> libhid::client::Observation {
        let reply = keycall(
            caller,
            self.keys.control(),
            libhid::protocol::Command::Query,
        );
        libhid::client::Watch::decode_reply(&reply, 2, 0).expect("reuse keyboard query failed")
    }
    fn input(&self, caller: &mut Caller, usage: u8, after: u64) -> u64 {
        loop {
            let state = self.state(caller);
            assert!(state.state.connected, "live reuse keyboard disconnected");
            let mut expected = [0; 6];
            expected[0] = usage;
            if state.state.valid && state.state.sequence > after && state.state.keys == expected {
                return state.state.sequence;
            }
            keycall(
                caller,
                self.keys.control(),
                libhid::protocol::Command::Ack(state.state.sequence),
            );
            wait::wait_until(
                self.keys.observer().as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                end(),
            )
            .expect("reuse keyboard input deadline exceeded");
        }
    }
    fn retired(&self, caller: &mut Caller, source: &MailboxSender) {
        loop {
            let reply = call(
                caller,
                source,
                Command::QueryDevice {
                    id: self.reference.device(),
                },
            );
            let current = info(&reply, self.reference.device());
            if current.binding.is_none() {
                break;
            }
            let reply = call(caller, self.instance.control(), Command::QueryWatch);
            let state = libdevice::client::Watch::decode_reply(&reply, 17)
                .expect("reuse retirement watch query failed");
            if state.pending != 0 {
                call(
                    caller,
                    self.instance.control(),
                    Command::AckWatch {
                        sequence: state.sequence,
                    },
                );
                continue;
            }
            wait::wait_until(
                self.instance.observer().as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                end(),
            )
            .expect("reuse binding release deadline exceeded");
        }
    }
    fn old(&self, caller: &mut Caller) {
        let state = self.state(caller);
        assert!(
            !state.state.connected && !state.state.valid,
            "old Watch reached new binding"
        );
        let reply = keycall(caller, &self.function, libhid::protocol::Command::Query);
        let (header, body) = libhid::protocol::Header::decode(&reply.payload)
            .expect("old reuse function response invalid");
        assert_eq!(header.op, 2, "old reuse function opcode changed");
        assert_eq!(
            reply.handles.remaining(),
            0,
            "old reuse function acquired capabilities"
        );
        assert!(
            header.error == Some(Error::Closed)
                || (header.error.is_none()
                    && !libhid::Snapshot::decode(body)
                        .expect("old reuse state invalid")
                        .0
                        .connected),
            "old function reached new binding"
        );
    }
}
pub(super) fn exercise(caller: &mut Caller, source: &MailboxSender, drivers: &MailboxSender) {
    libkernel::debug!("DEVICE reuse waiting A");
    let a = Keyboard::find(caller, source, &[]);
    let process_a = drivers::locate(caller, drivers, a.reference);
    let independent = libhid::client::Watch::from_reply(
        keycall(caller, &a.function, libhid::protocol::Command::Watch),
        &a.function,
    )
    .expect("independent keyboard Watch failed");
    let state = a.state(caller);
    keycall(
        caller,
        a.keys.control(),
        libhid::protocol::Command::Ack(state.state.sequence),
    );
    let reply = keycall(
        caller,
        independent.control(),
        libhid::protocol::Command::Query,
    );
    let other =
        libhid::client::Watch::decode_reply(&reply, 2, 0).expect("independent Watch query failed");
    assert!(
        other.pending,
        "one observer acknowledgement cleared another cursor"
    );
    drop(independent);
    libkernel::debug!(
        "DEVICE reuse A ready: serial={}",
        a.reference.device().serial()
    );
    let b = Keyboard::find(caller, source, &[a.reference.device()]);
    assert_eq!(
        drivers::locate(caller, drivers, b.reference),
        process_a,
        "A/B did not share Drivers instance"
    );
    assert_ne!(
        a.function
            .description()
            .expect("reuse A identity failed")
            .object_id,
        b.function
            .description()
            .expect("reuse B identity failed")
            .object_id,
        "reuse contexts aliased"
    );
    libkernel::debug!(
        "DEVICE reuse B ready: serial={}",
        b.reference.device().serial()
    );
    let first = b.input(caller, 4, 0);
    libkernel::debug!("DEVICE reuse B press observed: sequence={}", first);
    let first = b.input(caller, 0, first);
    libkernel::debug!("DEVICE reuse B release observed: sequence={}", first);
    a.retired(caller, source);
    drivers::detached(caller, drivers, process_a, a.reference);
    assert_eq!(
        drivers::locate(caller, drivers, b.reference),
        process_a,
        "B left original instance"
    );
    a.old(caller);
    libkernel::debug!("DEVICE reuse A Released with old aliases retained");
    let second = b.input(caller, 4, first);
    libkernel::debug!("DEVICE reuse B after A press observed: sequence={}", second);
    let second = b.input(caller, 0, second);
    libkernel::debug!(
        "DEVICE reuse B after A release observed: sequence={}",
        second
    );
    let c = Keyboard::find(
        caller,
        source,
        &[a.reference.device(), b.reference.device()],
    );
    assert_eq!(
        drivers::locate(caller, drivers, c.reference),
        process_a,
        "C did not reuse Drivers instance"
    );
    a.old(caller);
    let still = b.state(caller);
    assert!(
        still.state.connected && still.state.sequence >= second && still.state.keys == [0; 6],
        "B changed across C activation"
    );
    libkernel::debug!(
        "DEVICE reuse C ready; A terminal and B live: serial={}",
        c.reference.device().serial()
    );
    let pressed = c.input(caller, 5, 0);
    libkernel::debug!("DEVICE reuse C press observed: sequence={}", pressed);
    let released = c.input(caller, 0, pressed);
    a.old(caller);
    let unchanged = b.state(caller);
    assert!(
        unchanged.state.connected && unchanged.state.keys == [0; 6],
        "C input changed B keyboard state"
    );
    libkernel::debug!(
        "DEVICE reuse C release observed; waiting HID process fault: sequence={}",
        released
    );
    b.retired(caller, source);
    c.retired(caller, source);
    drivers::detached(caller, drivers, process_a, b.reference);
    drivers::detached(caller, drivers, process_a, c.reference);
    // 进程死亡关闭入口不证明硬件结束；上面各自原Seat才是供应安全Released的证据。
    libkernel::debug!("DEVICE reuse B/C Released after process fault");
    drop(a);
    drop(b);
    drop(c);
    libkernel::debug!("DEVICE multi-binding function acceptance passed");
}
