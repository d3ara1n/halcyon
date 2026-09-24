use super::*;

pub const RNL2_PROTOCOL: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamDirection {
    Read = 1,
    Write = 2,
}

impl StreamDirection {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Read),
            2 => Some(Self::Write),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamState {
    Offered = 1,
    Active = 2,
    Terminal = 3,
}

impl StreamState {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Offered),
            2 => Some(Self::Active),
            3 => Some(Self::Terminal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamReason {
    None = 0,
    Completed = 1,
    Cancelled = 2,
    Expired = 3,
    PeerClosed = 4,
    Backend = 5,
    ProviderStopped = 6,
}

impl StreamReason {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::None),
            1 => Some(Self::Completed),
            2 => Some(Self::Cancelled),
            3 => Some(Self::Expired),
            4 => Some(Self::PeerClosed),
            5 => Some(Self::Backend),
            6 => Some(Self::ProviderStopped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamOffer {
    pub identity: u64,
    pub direction: StreamDirection,
    pub tunnel_bytes: u32,
    pub start: u64,
    pub read_end: u64,
    pub offer_deadline: Deadline,
}

impl StreamOffer {
    pub const ENCODED_LEN: usize = 48;

    pub(super) fn write(self, writer: &mut Writer<'_>) {
        writer.u64(self.identity);
        writer.u32(self.direction as u32);
        writer.u32(self.tunnel_bytes);
        writer.u64(self.start);
        writer.u64(self.read_end);
        writer.u32(self.offer_deadline.kind);
        writer.u32(self.offer_deadline.reserved);
        writer.u64(self.offer_deadline.at_ns);
    }

    pub(super) fn read(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offer = Self {
            identity: reader.u64()?,
            direction: StreamDirection::from_raw(reader.u32()?).ok_or(DecodeError)?,
            tunnel_bytes: reader.u32()?,
            start: reader.u64()?,
            read_end: reader.u64()?,
            offer_deadline: Deadline {
                kind: reader.u32()?,
                reserved: reader.u32()?,
                at_ns: reader.u64()?,
            },
        };
        if offer.identity == 0
            || offer.tunnel_bytes == 0
            || offer
                .offer_deadline
                .instant()
                .map_err(|_| DecodeError)?
                .is_none()
            || (offer.direction == StreamDirection::Read && offer.read_end < offer.start)
            || (offer.direction == StreamDirection::Write && offer.read_end != 0)
        {
            return Err(DecodeError);
        }
        Ok(offer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub identity: u64,
    pub state: StreamState,
    pub outcome: Status,
    pub reason: StreamReason,
    pub accepted: u64,
    pub transported: u64,
}

impl StreamInfo {
    pub const ENCODED_LEN: usize = 40;

    pub(super) fn write(self, writer: &mut Writer<'_>) {
        writer.u64(self.identity);
        writer.u32(self.state as u32);
        writer.u32(self.outcome as u32);
        writer.u32(self.reason as u32);
        writer.u32(0);
        writer.u64(self.accepted);
        writer.u64(self.transported);
    }

    pub(super) fn read(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let identity = reader.u64()?;
        let state = StreamState::from_raw(reader.u32()?).ok_or(DecodeError)?;
        let outcome = Status::from_raw(reader.u32()?).ok_or(DecodeError)?;
        let reason = StreamReason::from_raw(reader.u32()?).ok_or(DecodeError)?;
        let reserved = reader.u32()?;
        let accepted = reader.u64()?;
        let transported = reader.u64()?;
        if identity == 0
            || reserved != 0
            || accepted > transported
            || (state != StreamState::Terminal
                && (outcome != Status::Ok || reason != StreamReason::None))
            || (state == StreamState::Terminal && reason == StreamReason::None)
        {
            return Err(DecodeError);
        }
        Ok(Self {
            identity,
            state,
            outcome,
            reason,
            accepted,
            transported,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_control_wire_reject_malformed_ranges_and_trailing_bytes() {
        let open = Request::Open {
            path: "data",
            expected_identity: core::num::NonZeroU64::new(42),
            direction: StreamDirection::Read,
            offset: 3,
            length: Some(8),
            session_deadline: Deadline::at(200),
            stream_protocol: RNL2_PROTOCOL,
            tunnel_bytes: 0,
        };
        let mut buffer = [0; 128];
        let used = encode_request(&open, Deadline::at(100), &mut buffer).unwrap();
        assert_eq!(decode_request(&buffer[..used]).unwrap().1, open);
        let current_name = Request::Open {
            path: "data",
            expected_identity: None,
            direction: StreamDirection::Read,
            offset: 3,
            length: Some(8),
            session_deadline: Deadline::at(200),
            stream_protocol: RNL2_PROTOCOL,
            tunnel_bytes: 0,
        };
        let current_used = encode_request(&current_name, Deadline::at(100), &mut buffer).unwrap();
        assert_eq!(
            decode_request(&buffer[..current_used]).unwrap().1,
            current_name
        );
        let used = encode_request(&open, Deadline::at(100), &mut buffer).unwrap();
        buffer[HEADER_LEN + 4] = 2;
        assert!(decode_request(&buffer[..used]).is_err());
        let invalid = Request::Open {
            path: "data",
            expected_identity: None,
            direction: StreamDirection::Read,
            offset: u64::MAX,
            length: Some(1),
            session_deadline: Deadline::at(200),
            stream_protocol: RNL2_PROTOCOL,
            tunnel_bytes: 0,
        };
        assert!(encode_request(&invalid, Deadline::at(100), &mut buffer).is_none());
        for request in [
            Request::Start,
            Request::QueryStream,
            Request::FinishStream,
            Request::CancelStream,
        ] {
            let used = encode_request(&request, Deadline::at(100), &mut buffer).unwrap();
            assert_eq!(decode_request(&buffer[..used]).unwrap().1, request);
            buffer[used] = 1;
            assert!(decode_request(&buffer[..used + 1]).is_err());
        }
    }

    #[test]
    fn offer_and_terminal_result_have_distinct_capability_contracts() {
        let offer = StreamOffer {
            identity: 17,
            direction: StreamDirection::Read,
            tunnel_bytes: 3 * 4096,
            start: 4,
            read_end: 20,
            offer_deadline: Deadline::at(100),
        };
        let mut buffer = [0; 128];
        let used = encode_response(
            Op::Open,
            Status::Ok,
            Deadline::at(100),
            &Response::StreamOffer(offer),
            &mut buffer,
        )
        .unwrap();
        let (_, response) = decode_response(&buffer[..used]).unwrap();
        assert_eq!(response, Response::StreamOffer(offer));
        assert_eq!(response.capability_count(Op::Open), Some(2));
        assert_eq!(response.capability_count(Op::Start), None);
        buffer[HEADER_LEN] = 0;
        assert!(decode_response(&buffer[..used]).is_err());

        let info = StreamInfo {
            identity: 17,
            state: StreamState::Terminal,
            outcome: Status::Cancelled,
            reason: StreamReason::Expired,
            accepted: 2,
            transported: 3,
        };
        let used = encode_response(
            Op::FinishStream,
            Status::Ok,
            Deadline::at(100),
            &Response::StreamInfo(info),
            &mut buffer,
        )
        .unwrap();
        assert_eq!(
            decode_response(&buffer[..used]).unwrap().1,
            Response::StreamInfo(info)
        );
        buffer[HEADER_LEN + 24..HEADER_LEN + 32].copy_from_slice(&4u64.to_le_bytes());
        assert!(decode_response(&buffer[..used]).is_err());
    }
}
