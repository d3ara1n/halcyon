//! 文件宿主私有出生协议；profile 选择部署，不授予权限。
use erhino_shared::call::SystemCallError;

pub const GRANTS: usize = 5;
pub const PAYLOAD_LEN: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Profile {
    System = 0,
    AcceptancePrimary = 1,
    AcceptanceSecondary = 2,
}
impl Profile {
    pub const fn encode(self) -> [u8; PAYLOAD_LEN] {
        let version = 1u32.to_le_bytes();
        let profile = (self as u32).to_le_bytes();
        [
            version[0], version[1], version[2], version[3], profile[0], profile[1], profile[2],
            profile[3],
        ]
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, SystemCallError> {
        if bytes.len() != PAYLOAD_LEN || bytes[..4] != 1u32.to_le_bytes() {
            return Err(SystemCallError::IllegalArgument);
        }
        match &bytes[4..] {
            [0, 0, 0, 0] => Ok(Self::System),
            [1, 0, 0, 0] => Ok(Self::AcceptancePrimary),
            [2, 0, 0, 0] => Ok(Self::AcceptanceSecondary),
            _ => Err(SystemCallError::IllegalArgument),
        }
    }
    pub const fn registry(self) -> bool {
        !matches!(self, Self::AcceptanceSecondary)
    }
    pub const fn entries(self) -> usize {
        match self {
            Self::AcceptancePrimary => 2,
            _ => 1,
        }
    }
    pub const fn root_messages(self) -> usize {
        if self.registry() { 4 } else { 1 }
    }
    pub const fn bootstrap_messages(self) -> usize {
        self.root_messages() + 2
    }
}
const _: () = {
    assert!(Profile::System.bootstrap_messages() <= erhino_shared::message::MAILBOX_CAPACITY);
    assert!(
        Profile::AcceptancePrimary.bootstrap_messages() <= erhino_shared::message::MAILBOX_CAPACITY
    );
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn birth_wire_is_versioned_and_rejects_old_payloads() {
        for (profile, tag) in [
            (Profile::System, 0),
            (Profile::AcceptancePrimary, 1),
            (Profile::AcceptanceSecondary, 2),
        ] {
            let wire = [1, 0, 0, 0, tag, 0, 0, 0];
            assert_eq!(profile.encode(), wire);
            assert_eq!(Profile::decode(&wire), Ok(profile));
        }
        for bytes in [
            &b""[..],
            &b"secondary"[..],
            &[0, 0, 0, 0, 0, 0, 0, 0][..],
            &[1, 0, 0, 0, 3, 0, 0, 0][..],
            &[1, 0, 0, 0, 0, 0, 0][..],
        ] {
            assert!(Profile::decode(bytes).is_err());
        }
    }
    #[test]
    fn system_has_one_full_entry_and_acceptance_preserves_shared_quotas() {
        use crate::deployment::*;
        let system = Capacity::new(Profile::System).unwrap();
        let acceptance = Capacity::new(Profile::AcceptancePrimary).unwrap();
        assert_eq!(Profile::System.bootstrap_messages(), 6);
        assert_eq!(Profile::AcceptanceSecondary.bootstrap_messages(), 3);
        assert_eq!(system.entry_watches(0), WATCHES);
        assert_eq!(system.entry_sessions(0), SESSIONS);
        assert_eq!(acceptance.entry_watches(0), WATCHES / 2);
        assert_eq!(acceptance.entry_sessions(0), SESSIONS / 2);
        assert_eq!(acceptance.entry_watches(1), WATCHES);
        assert_eq!(acceptance.entry_sessions(1), SESSIONS);
    }
}
