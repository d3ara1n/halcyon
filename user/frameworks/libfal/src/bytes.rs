//! little-endian 定宽编解码游标：协议层的唯一字节出入通道。

/// 编码游标：顺序写入 LE 定宽字段与原始字节。
pub struct Writer<'a> {
    buffer: &'a mut [u8],
    position: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, position: 0 }
    }

    pub fn written(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.position
    }

    fn put(&mut self, bytes: &[u8]) -> DecodeResult<()> {
        if self.remaining() < bytes.len() {
            return Err(DecodeError);
        }
        self.buffer[self.position..self.position + bytes.len()].copy_from_slice(bytes);
        self.position += bytes.len();
        Ok(())
    }

    pub fn u8(&mut self, value: u8) -> DecodeResult<()> {
        self.put(&[value])
    }

    pub fn u16(&mut self, value: u16) -> DecodeResult<()> {
        self.put(&value.to_le_bytes())
    }

    pub fn u32(&mut self, value: u32) -> DecodeResult<()> {
        self.put(&value.to_le_bytes())
    }

    pub fn u64(&mut self, value: u64) -> DecodeResult<()> {
        self.put(&value.to_le_bytes())
    }

    pub fn bytes(&mut self, value: &[u8]) -> DecodeResult<()> {
        self.put(value)
    }

    /// 长度前缀（u16）的字节段；超长即协议错误。
    pub fn sized_bytes(&mut self, value: &[u8]) -> DecodeResult<()> {
        if value.len() > u16::MAX as usize {
            return Err(DecodeError);
        }
        self.u16(value.len() as u16)?;
        self.put(value)
    }
}

/// 解码游标：按协议顺序读取，任何越界/尾差都报告为协议错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError;

pub type DecodeResult<T> = Result<T, DecodeError>;

pub struct Reader<'a> {
    buffer: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer, position: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.position
    }

    /// 已消费的字节数（定位后续参数用）。
    pub fn consumed(&self) -> usize {
        self.position
    }

    fn take(&mut self, len: usize) -> DecodeResult<&'a [u8]> {
        if self.remaining() < len {
            return Err(DecodeError);
        }
        let slice = &self.buffer[self.position..self.position + len];
        self.position += len;
        Ok(slice)
    }

    pub fn u8(&mut self) -> DecodeResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> DecodeResult<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> DecodeResult<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> DecodeResult<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn bytes(&mut self, len: usize) -> DecodeResult<&'a [u8]> {
        self.take(len)
    }

    pub fn sized_bytes(&mut self) -> DecodeResult<&'a [u8]> {
        let len = self.u16()? as usize;
        self.take(len)
    }

    /// body 必须整体消费完毕：残尾即协议违约。
    pub fn finish(&self) -> DecodeResult<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(DecodeError)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_reader_roundtrip() {
        let mut buffer = [0u8; 32];
        let mut writer = Writer::new(&mut buffer);
        writer.u8(0xAB).unwrap();
        writer.u16(0x0102).unwrap();
        writer.u32(0x0304_0506).unwrap();
        writer.u64(0x07).unwrap();
        writer.sized_bytes(b"erhino").unwrap();
        let used = writer.written();

        let mut reader = Reader::new(&buffer[..used]);
        assert_eq!(reader.u8().unwrap(), 0xAB);
        assert_eq!(reader.u16().unwrap(), 0x0102);
        assert_eq!(reader.u32().unwrap(), 0x0304_0506);
        assert_eq!(reader.u64().unwrap(), 7);
        assert_eq!(reader.sized_bytes().unwrap(), b"erhino");
        assert_eq!(reader.finish(), Ok(()));
    }

    #[test]
    fn writer_rejects_overflow() {
        let mut buffer = [0u8; 4];
        let mut writer = Writer::new(&mut buffer);
        assert_eq!(writer.bytes(&[0; 5]), Err(DecodeError));
        let mut tight = [0u8; 3];
        let mut writer = Writer::new(&mut tight);
        assert_eq!(writer.sized_bytes(b"abcd"), Err(DecodeError));
        // 定宽写同样拒绝短缓冲。
        let mut narrow = [0u8; 3];
        let mut writer = Writer::new(&mut narrow);
        assert_eq!(writer.u32(1), Err(DecodeError));
    }

    #[test]
    fn reader_reports_truncation_and_tail() {
        let mut reader = Reader::new(&[0; 3]);
        assert_eq!(reader.u32(), Err(DecodeError));
        let mut reader = Reader::new(&[1, 0, 0, 0, 9]);
        assert_eq!(reader.u32(), Ok(1));
        assert_eq!(reader.finish(), Err(DecodeError));
    }
}
