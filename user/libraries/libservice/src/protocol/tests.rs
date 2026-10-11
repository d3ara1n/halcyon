use super::*;
fn header(op: u16, body: &[u8]) -> alloc::vec::Vec<u8> {
    // 按固定线形手工组帧；不以生产 encoder 作为独立预期。
    let mut out = alloc::vec::Vec::new();
    out.extend(2u16.to_le_bytes());
    out.extend(op.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(50u64.to_le_bytes());
    out.extend(body);
    out
}
fn descriptor() -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    out.extend(1u32.to_le_bytes());
    out.extend(0x4653_3031u64.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out
}
#[test]
fn register_golden_uses_full_transport_and_exact_slots() {
    // 捕获 u64 出口被截断、正文偏移和实际 input 槽位绕过。
    let mut body = alloc::vec::Vec::new();
    body.extend(1u32.to_le_bytes());
    body.push(b's');
    body.extend(0u32.to_le_bytes());
    let desc = descriptor();
    body.extend((desc.len() as u32).to_le_bytes());
    body.extend(desc);
    body.extend(
        (Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
            .raw()
            .to_le_bytes(),
    );
    assert_eq!(body.len(), 41);
    let frame = header(2, &body);
    let (h, r) = decode_request(&frame, 3).unwrap();
    assert_eq!(h.deadline, Deadline::at(50));
    assert!(matches!(
        r,
        Request::Register {
            name: "s",
            mode: RegisterMode::CreateOnly,
            ..
        }
    ));
    assert!(matches!(decode_request(&frame, 1), Err(Status::Invalid)));
    let last = body.len() - 8;
    body[last..].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        decode_request(&header(2, &body), 3),
        Err(Status::Invalid)
    ));
}
#[test]
fn descriptor_count_duplicates_and_version_range_are_rejected() {
    // Descriptor 集合是独立可验证的唯一 id 集合，恶意 count 不能驱动无界循环。
    let mut one = descriptor();
    assert_eq!(Descriptors::decode(&one).unwrap().count(), 1);
    one[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(Descriptors::decode(&one), Err(Status::Invalid)));
    let original = descriptor();
    let mut duplicate = alloc::vec::Vec::new();
    duplicate.extend(2u32.to_le_bytes());
    duplicate.extend(&original[4..]);
    duplicate.extend(&original[4..]);
    assert!(matches!(
        Descriptors::decode(&duplicate),
        Err(Status::Invalid)
    ));
    let mut zero = original;
    zero[12..14].copy_from_slice(&0u16.to_le_bytes());
    assert!(matches!(Descriptors::decode(&zero), Err(Status::Invalid)));
}
#[test]
fn registration_response_pairs_state_reason_and_echo() {
    // Active/None 与 Withdrawn/非 None 是公开状态合同；旧 ID 不是管理权限。
    let mut body = alloc::vec::Vec::new();
    body.extend(2u64.to_le_bytes());
    body.extend(4u64.to_le_bytes());
    body.extend(1u32.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    let request = Request::QueryRegistration;
    let frame = header(5, &body);
    assert!(matches!(
        decode_response(&frame, &request, Deadline::at(50), 0),
        Ok(Response::Registration(_))
    ));
    assert!(matches!(
        decode_response(&frame, &request, Deadline::at(51), 0),
        Err(Status::Invalid)
    ));
    assert!(matches!(
        decode_response(&frame, &request, Deadline::at(50), 1),
        Err(Status::Invalid)
    ));
    body[16..20].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        decode_response(&header(5, &body), &request, Deadline::at(50), 0),
        Err(Status::Invalid)
    ));
    body[20..24].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        decode_response(&header(5, &body), &request, Deadline::at(50), 0),
        Ok(Response::Registration(RegistrationInfo {
            reason: Reason::Replaced,
            ..
        }))
    ));
}
#[test]
fn frames_reject_trailing_reserved_and_malformed_paths() {
    // 拒绝完整帧外的尾差、保留位以及线上非规范路径，不替 provider 词法归一化。
    let mut frame = header(5, &[]);
    frame.push(0);
    assert!(matches!(decode_request(&frame, 1), Err(Status::Invalid)));
    let mut frame = header(5, &[]);
    frame[12] = 1;
    assert!(matches!(decode_request(&frame, 1), Err(Status::Invalid)));
    let body = [4, 0, 0, 0, b'a', b'/', b'.', b'.'];
    assert!(matches!(
        decode_request(&header(3, &body), 1),
        Err(Status::Invalid)
    ));
}
