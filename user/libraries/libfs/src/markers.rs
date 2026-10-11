//! 文件领域标记和 tag 编号；OAL 不校验这些标记的组合。
use liboal::MarkerId;
pub const NAMESPACE: u128 = 0x48414c43594f4e000000000046530000;
const fn marker(local: u32) -> MarkerId {
    MarkerId {
        namespace: NAMESPACE,
        local,
    }
}
pub const REGULAR_FILE: MarkerId = marker(1);
pub const PROPERTY: MarkerId = marker(2);
pub const STREAM: MarkerId = marker(3);
pub const SYMBOLIC_LINK: MarkerId = marker(4);
pub const BOOLEAN: MarkerId = marker(5);
pub const INTEGER: MarkerId = marker(6);
pub const FLOAT: MarkerId = marker(7);
pub const STRING: MarkerId = marker(8);
pub const BLOB: MarkerId = marker(9);
pub const ARRAY: MarkerId = marker(10);
pub const BYTE_LENGTH: MarkerId = marker(1);
