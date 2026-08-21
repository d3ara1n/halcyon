//! 测试共享：手工构造 DTB blob 的最小构建器（token 文法直出）。

pub struct BlobBuilder {
    pub struct_block: Vec<u8>,
    pub strings: Vec<u8>,
}

impl BlobBuilder {
    pub fn new() -> Self {
        Self {
            struct_block: Vec::new(),
            strings: Vec::new(),
        }
    }

    /// 登记字符串，返回 strings 块内偏移。
    pub fn string(&mut self, s: &str) -> u32 {
        let off = self.strings.len();
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        off as u32
    }

    fn push_u32(&mut self, v: u32) {
        self.struct_block.extend_from_slice(&v.to_be_bytes());
    }

    pub fn begin(&mut self, name: &str) {
        self.push_u32(0x1);
        let mut b = name.as_bytes().to_vec();
        b.push(0);
        while b.len() % 4 != 0 {
            b.push(0);
        }
        self.struct_block.extend_from_slice(&b);
    }

    pub fn end(&mut self) {
        self.push_u32(0x2);
    }

    pub fn nop(&mut self) {
        self.push_u32(0x4);
    }

    pub fn prop(&mut self, name: &str, data: &[u8]) {
        let nameoff = self.string(name);
        self.push_u32(0x3);
        self.push_u32(data.len() as u32);
        self.push_u32(nameoff);
        self.struct_block.extend_from_slice(data);
        while self.struct_block.len() % 4 != 0 {
            self.struct_block.push(0xAA); // 填充字节任意，读取方必须跳过
        }
    }

    /// NUL 分隔的字符串列表属性。
    pub fn prop_str_list(&mut self, name: &str, items: &[&str]) {
        let mut data = Vec::new();
        for item in items {
            data.extend_from_slice(item.as_bytes());
            data.push(0);
        }
        self.prop(name, &data);
    }

    pub fn prop_u32(&mut self, name: &str, v: u32) {
        self.prop(name, &v.to_be_bytes());
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.push_u32(0x9); // FDT_END

        const HEADER: usize = 40;
        let strings_off = HEADER + self.struct_block.len().max(8).next_power_of_two();
        let totalsize = strings_off + self.strings.len();
        let mut out = Vec::with_capacity(totalsize);
        out.extend_from_slice(&0xD00D_FEEDu32.to_be_bytes());
        out.extend_from_slice(&(totalsize as u32).to_be_bytes());
        out.extend_from_slice(&(HEADER as u32).to_be_bytes()); // off_struct
        out.extend_from_slice(&(strings_off as u32).to_be_bytes());
        out.extend_from_slice(&((HEADER + 8) as u32).to_be_bytes()); // off_mem_rsvmap（未用）
        out.extend_from_slice(&17u32.to_be_bytes()); // version
        out.extend_from_slice(&16u32.to_be_bytes()); // last_comp
        out.extend_from_slice(&0u32.to_be_bytes()); // boot_cpuid
        out.extend_from_slice(&(self.strings.len() as u32).to_be_bytes());
        out.extend_from_slice(&(self.struct_block.len() as u32).to_be_bytes());
        debug_assert_eq!(out.len(), HEADER);
        out.resize(HEADER, 0);
        out.extend_from_slice(&self.struct_block);
        out.resize(strings_off, 0);
        out.extend_from_slice(&self.strings);
        out
    }
}

impl BlobBuilder {
    /// NUL 结尾单字符串属性。
    pub fn prop_str(&mut self, name: &str, value: &str) {
        let mut data = value.as_bytes().to_vec();
        data.push(0);
        self.prop(name, &data);
    }
}
