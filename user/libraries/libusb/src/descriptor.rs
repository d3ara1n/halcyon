//! USB 2.0 §9.3/§9.6、HID 1.11 Appendix B.1 的首批 boot keyboard 描述。
use libdevice::Error;
pub const BOOT_KEYBOARD_IDENTITY: &[u8] = b"usb:03:01:01";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}
impl Setup {
    pub fn encode(self) -> [u8; 8] {
        let mut bytes = [self.request_type, self.request, 0, 0, 0, 0, 0, 0];
        bytes[2..4].copy_from_slice(&self.value.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.index.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.length.to_le_bytes());
        bytes
    }
    pub fn descriptor(kind: u8, length: u16) -> Self {
        Self {
            request_type: 0x80,
            request: 6,
            value: u16::from(kind) << 8,
            index: 0,
            length,
        }
    }
    pub fn configuration(value: u8) -> Self {
        Self {
            request_type: 0,
            request: 9,
            value: u16::from(value),
            index: 0,
            length: 0,
        }
    }
    pub fn boot_protocol(interface: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 11,
            value: 0,
            index: u16::from(interface),
            length: 0,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyboard {
    pub configuration: u8,
    pub interface: u8,
    pub endpoint: u8,
    pub max_packet: u16,
    pub interval: u8,
}
impl Keyboard {
    pub const BYTES: usize = 8;
    pub fn encode(self) -> [u8; 8] {
        let size = self.max_packet.to_le_bytes();
        [
            1,
            self.configuration,
            self.interface,
            self.endpoint,
            size[0],
            size[1],
            self.interval,
            0,
        ]
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::BYTES || bytes[0] != 1 || bytes[7] != 0 {
            return Err(Error::Invalid);
        }
        let value = Self {
            configuration: bytes[1],
            interface: bytes[2],
            endpoint: bytes[3],
            max_packet: u16::from_le_bytes([bytes[4], bytes[5]]),
            interval: bytes[6],
        };
        if value.configuration == 0
            || value.endpoint & 0xf0 != 0x80
            || value.endpoint & 15 == 0
            || value.max_packet < 8
            || value.interval == 0
        {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
    pub fn dci(self) -> u8 {
        (self.endpoint & 15) * 2 + 1
    }
}
pub fn keyboard_configuration(bytes: &[u8]) -> Result<Keyboard, Error> {
    if bytes.len() < 9 || bytes[0] != 9 || bytes[1] != 2 {
        return Err(Error::Invalid);
    }
    let total = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
    if total != bytes.len() || bytes[5] == 0 {
        return Err(Error::Invalid);
    }
    let mut at = 9;
    let mut selected = None;
    let mut interface = None;
    while at < total {
        if total - at < 2 {
            return Err(Error::Invalid);
        }
        let size = usize::from(bytes[at]);
        if size < 2 || size > total - at {
            return Err(Error::Invalid);
        }
        let part = &bytes[at..at + size];
        match part[1] {
            4 => {
                if size < 9 {
                    return Err(Error::Invalid);
                }
                interface = if part[3] == 0 && part[5..8] == [3, 1, 1] {
                    Some(part[2])
                } else {
                    None
                };
            }
            5 => {
                if size < 7 {
                    return Err(Error::Invalid);
                }
                if let Some(number) = interface
                    && part[2] & 0xf0 == 0x80
                    && part[2] & 15 != 0
                    && part[3] & 3 == 3
                {
                    let max_packet = u16::from_le_bytes([part[4], part[5]]);
                    if max_packet & !0x7ff != 0 || max_packet < 8 || part[6] == 0 {
                        return Err(Error::Unsupported);
                    }
                    if selected.is_some() {
                        return Err(Error::Unsupported);
                    }
                    selected = Some(Keyboard {
                        configuration: bytes[5],
                        interface: number,
                        endpoint: part[2],
                        max_packet,
                        interval: part[6],
                    });
                }
            }
            _ => {}
        }
        at += size;
    }
    selected.ok_or(Error::Unsupported)
}
#[cfg(test)]
mod tests {
    use super::*;
    // 标准配置+boot接口+interrupt IN；长度、class与packet预期来自USB/HID固定字段合同。
    #[test]
    fn boot_interface_and_malformed_descriptor_progress() {
        let bytes = [
            9, 2, 25, 0, 1, 1, 0, 0x80, 50, 9, 4, 2, 0, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10,
        ];
        let keyboard = keyboard_configuration(&bytes).unwrap();
        assert_eq!(keyboard.interface, 2);
        assert_eq!(keyboard.dci(), 3);
        assert_eq!(keyboard.interval, 10);
        let mut bad = bytes;
        bad[18] = 0;
        assert_eq!(keyboard_configuration(&bad), Err(Error::Invalid));
        let mut bad = bytes;
        bad[2] = 24;
        assert_eq!(keyboard_configuration(&bad), Err(Error::Invalid));
        let mut bad = bytes;
        bad[15] = 2;
        assert_eq!(keyboard_configuration(&bad), Err(Error::Unsupported));
        assert_eq!(
            Setup::boot_protocol(2).encode(),
            [0x21, 11, 0, 0, 2, 0, 0, 0]
        );
    }
}
