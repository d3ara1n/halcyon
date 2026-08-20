//! ustar 遍历测试（host）：构造最小归档验证解析。

use tar::{walk, TarError};

/// 构造一个 ustar 头块。
fn header(name: &str, size: usize, typeflag: u8) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    let oct = format!("{:011o}", size);
    h[124..124 + oct.len()].copy_from_slice(oct.as_bytes());
    h[156] = typeflag;
    h[257..262].copy_from_slice(b"ustar");
    h[263..265].copy_from_slice(b"00");
    h
}

fn file_entry(name: &str, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&header(name, data.len(), b'0'));
    v.extend_from_slice(data);
    v.resize(v.len().div_ceil(512) * 512, 0);
    v
}

#[test]
fn walk_files_and_dirs() {
    let mut archive = Vec::new();
    archive.extend(file_entry("bin/", &[])); // 目录（'5' 应走目录分支）
    archive[156] = b'5'; // 首块头部的 typeflag
    archive.extend(file_entry("bin/srv_init", b"\x7fELF payload"));
    archive.extend([0u8; 1024]); // 终止块

    let mut names = Vec::new();
    walk(&archive, |e| names.push((e.name.to_string(), e.data.len()))).unwrap();
    assert_eq!(
        names,
        vec![("bin/".to_string(), 0), ("bin/srv_init".to_string(), 12)],
        "目录零长、文件按 size"
    );
}

#[test]
fn empty_archive() {
    let archive = vec![0u8; 1024];
    walk(&archive, |_| panic!("空归档不应有项")).unwrap();
}

#[test]
fn bad_magic_rejected() {
    let mut archive = file_entry("a", b"x");
    archive[257..262].copy_from_slice(b"bstar");
    assert_eq!(walk(&archive, |_| {}), Err(TarError::BadMagic));
}

#[test]
fn truncated_data_rejected() {
    let mut archive = file_entry("big", &[0u8; 2048]);
    archive.truncate(512 + 512); // 头 + 首块，数据被截断
    assert_eq!(walk(&archive, |_| {}), Err(TarError::BadBlock));
}
