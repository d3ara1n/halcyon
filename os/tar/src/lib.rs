//! ustar 归档就地游标：零拷贝遍历 initfs（与 os/dtb 同范式，host 可测）。
//!
//! 只覆盖 initfs 打包所用的 ustar 子集：普通文件与目录项、512 字节块对齐、
//! 双全零块终止。GNU/POSIX 扩展头（typeflag 'x'/'g'/…）按格式错误处理。

#![cfg_attr(not(test), no_std)]

/// 一个归档项。目录项 `data` 为空切片。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// 归档格式错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarError {
    /// 块长度不是 512 的倍数。
    BadBlock,
    /// ustar magic 不符。
    BadMagic,
    /// 字段不是合法八进制。
    BadOctal,
    /// 名字含非 UTF-8 字节。
    BadName,
}

const BLOCK: usize = 512;

/// 解析整个归档，逐项回调（就地切片，不拷贝）。
pub fn walk(data: &[u8], mut f: impl FnMut(Entry<'_>)) -> Result<(), TarError> {
    if data.len() % BLOCK != 0 {
        return Err(TarError::BadBlock);
    }
    let mut pos = 0;
    while pos + BLOCK <= data.len() {
        let head = &data[pos..pos + BLOCK];
        if head.iter().all(|&b| b == 0) {
            return Ok(()); // 终止块
        }
        if &head[257..262] != b"ustar" {
            return Err(TarError::BadMagic);
        }
        let name_len = head[..100]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(100);
        let name = core::str::from_utf8(&head[..name_len]).map_err(|_| TarError::BadName)?;
        let size = octal(&head[124..136])? as usize;
        let typeflag = head[156];
        pos += BLOCK;
        let data_end = pos + size;
        if data_end > data.len() {
            return Err(TarError::BadBlock);
        }
        // 目录（'5'）与普通文件（'0' 或 NUL）之外不认。
        if !matches!(typeflag, b'0' | b'\0' | b'5') {
            return Err(TarError::BadMagic);
        }
        if typeflag != b'5' {
            f(Entry { name, data: &data[pos..data_end] });
        } else {
            f(Entry { name, data: &[] });
        }
        // 数据补齐到块边界。
        pos = data_end.div_ceil(BLOCK) * BLOCK;
    }
    Ok(())
}

/// 解析 NUL 结尾的八进制字段。
fn octal(field: &[u8]) -> Result<u64, TarError> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    let s = core::str::from_utf8(&field[..end]).map_err(|_| TarError::BadOctal)?;
    u64::from_str_radix(s.trim_matches(' '), 8).map_err(|_| TarError::BadOctal)
}
