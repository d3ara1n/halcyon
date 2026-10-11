use super::*;

#[test]
fn property_wire_is_fixed_scalar_array_count_and_little_endian_payload() {
    // 独立线形：Signed64=-2，不使用生产解码来构造编码预期。
    let mut bytes = [0; 64];
    let used = Value::Signed64(-2).encode(&mut bytes).unwrap();
    assert_eq!(
        &bytes[..used],
        &[
            2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 254, 255, 255, 255, 255, 255, 255, 255
        ]
    );
    let value = Property::decode(&bytes[..used]).unwrap();
    assert_eq!(value.get::<i64>(), Ok(-2));
    assert_eq!(value.get::<usize>(), Err(Status::Range));
    let used = Value::Unsigned64Array(&[]).encode(&mut bytes).unwrap();
    let value = Property::decode(&bytes[..used]).unwrap();
    assert_eq!(value.scalar(), Scalar::Unsigned64);
    assert!(value.is_array());
    assert_eq!(value.count(), 0);
    assert_eq!(value.get::<usize>(), Err(Status::TypeMismatch));
}
#[test]
fn property_rejects_foreign_shapes_counts_truncation_utf8_and_tail() {
    // capability/Record 不是纯值种类；每个字段与元素必须完整且同质。
    let mut bytes = [0; 64];
    let used = Value::String("雪").encode(&mut bytes).unwrap();
    for n in 0..used {
        assert!(Property::decode(&bytes[..n]).is_err());
    }
    assert!(Property::decode(&bytes[..used + 1]).is_err());
    for scalar in [0, 7, 8, u32::MAX] {
        let mut bad = bytes;
        bad[..4].copy_from_slice(&scalar.to_le_bytes());
        assert!(Property::decode(&bad[..used]).is_err());
    }
    let mut bad = bytes;
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Property::decode(&bad[..used]).is_err());
    let used = Value::Bool(true).encode(&mut bytes).unwrap();
    bytes[12] = 2;
    assert!(Property::decode(&bytes[..used]).is_err());
    let used = Value::String("a").encode(&mut bytes).unwrap();
    bytes[16] = 0xff;
    assert!(Property::decode(&bytes[..used]).is_err());
}
#[test]
fn homogeneous_arrays_and_ieee_bits_are_preserved() {
    // Get<T> 只在本地转换，Float 的 NaN 位模式不能被字符串/数值规范化。
    let mut bytes = [0; 128];
    let bits = 0x7ff8_0000_0000_1234;
    let used = Value::Float64(f64::from_bits(bits))
        .encode(&mut bytes)
        .unwrap();
    assert_eq!(
        Property::decode(&bytes[..used])
            .unwrap()
            .get::<f64>()
            .unwrap()
            .to_bits(),
        bits
    );
    let used = Value::StringArray(&["", "é", "雪"])
        .encode(&mut bytes)
        .unwrap();
    let value = Property::decode(&bytes[..used]).unwrap();
    assert_eq!(value.count(), 3);
    let texts = value
        .items()
        .map(|item| match item {
            Item::String(s) => s,
            _ => panic!("unexpected scalar"),
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["", "é", "雪"]);
}
