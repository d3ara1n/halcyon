//! HID boot keyboard状态；不拥有USB端点、DMA或执行运行体。
#![no_std]
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod protocol;
/// HID键盘面貌；领域marker并不授予功能操作权。
pub const KEYBOARD: liboal::MarkerId = liboal::MarkerId {
    namespace: 0x48414c43594f4e000000004849440000,
    local: 1,
};
use libdevice::Error;

/// keys按usage排序且不重复；无效报告/断开保留最后有效键集，并以valid=false明确失效。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub sequence: u64,
    pub modifiers: u8,
    pub keys: [u8; 6],
    pub valid: bool,
    pub connected: bool,
}
pub struct Keyboard(Snapshot);
impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}
impl Keyboard {
    pub const fn new() -> Self {
        Self(Snapshot {
            sequence: 1,
            modifiers: 0,
            keys: [0; 6],
            valid: false,
            connected: true,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.0
    }
    fn changed(&mut self) -> Result<(), Error> {
        if self.0.sequence == u64::MAX {
            return Err(Error::Exhausted);
        }
        self.0.sequence += 1;
        if self.0.sequence == u64::MAX {
            self.0.valid = false;
            self.0.connected = false;
            return Err(Error::Exhausted);
        }
        Ok(())
    }
    /// USB HID 1.11 Appendix B boot keyboard八字节输入；1..3为错误usage，不是全部释放。
    pub fn report(&mut self, bytes: [u8; 8]) -> Result<(), Error> {
        if !self.0.connected {
            return Err(Error::Closed);
        }
        if bytes[1] != 0 || bytes[2..].iter().any(|k| matches!(k, 1..=3)) {
            self.0.valid = false;
        } else {
            let mut keys = [0; 6];
            let mut count = 0;
            for &key in &bytes[2..] {
                if key != 0 && !keys[..count].contains(&key) {
                    keys[count] = key;
                    count += 1;
                }
            }
            keys[..count].sort_unstable();
            self.0.modifiers = bytes[0];
            self.0.keys = keys;
            self.0.valid = true;
        }
        self.changed()
    }
    /// 终态不是设备送来的空报告；不清最后有效键集。
    pub fn disconnect(&mut self) -> Result<(), Error> {
        if self.0.connected {
            self.0.connected = false;
            self.0.valid = false;
            self.changed()?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_rollover_and_disconnect_do_not_invent_release() {
        let mut keyboard = Keyboard::new();
        keyboard.report([2, 0, 7, 4, 4, 0, 0, 0]).unwrap();
        let valid = keyboard.snapshot();
        assert_eq!(valid.keys, [4, 7, 0, 0, 0, 0]);
        assert_eq!(valid.modifiers, 2);
        keyboard.report([0, 0, 1, 1, 1, 1, 1, 1]).unwrap();
        let invalid = keyboard.snapshot();
        assert!(!invalid.valid);
        assert_eq!(invalid.keys, valid.keys);
        keyboard.disconnect().unwrap();
        let stopped = keyboard.snapshot();
        assert!(!stopped.connected);
        assert!(!stopped.valid);
        assert_eq!(stopped.keys, valid.keys);
        assert!(stopped.sequence > invalid.sequence);
        assert_eq!(keyboard.report([0; 8]), Err(Error::Closed));
    }
    #[test]
    fn real_zero_report_releases_keys() {
        let mut keyboard = Keyboard::new();
        keyboard.report([0, 0, 4, 0, 0, 0, 0, 0]).unwrap();
        keyboard.report([0; 8]).unwrap();
        assert!(keyboard.snapshot().valid);
        assert_eq!(keyboard.snapshot().keys, [0; 6]);
    }
}
