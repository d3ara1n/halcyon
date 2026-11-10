//! 出生视图只描述 prefix → 命名输入，不携带 handle 或服务政策。
use crate::path::validate_absolute;
use alloc::{string::String, vec::Vec};

pub const SECTION_NAME: &str = "oal.namespace";
pub const INPUT_PREFIX: &str = "oal.entry.";
const MAGIC: [u8; 8] = *b"EROALNS\0";
const VERSION: u32 = 1;

pub fn input_name(index: usize) -> String {
    alloc::format!("{INPUT_PREFIX}{index}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub prefix: String,
    pub input: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Description {
    pub bindings: Vec<Binding>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Format,
    InvalidPath,
    InvalidInput,
    DuplicatePrefix,
    DuplicateInput,
    LengthOverflow,
}

fn valid_input(input: &str) -> bool {
    let Some(index) = input.strip_prefix(INPUT_PREFIX) else {
        return false;
    };
    !index.is_empty()
        && index.bytes().all(|b| b.is_ascii_digit())
        && (index.len() == 1 || !index.starts_with('0'))
        && index.parse::<usize>().is_ok()
}
impl Description {
    pub fn validate(&self) -> Result<(), Error> {
        for (i, binding) in self.bindings.iter().enumerate() {
            validate_absolute(&binding.prefix).map_err(|_| Error::InvalidPath)?;
            if !valid_input(&binding.input) {
                return Err(Error::InvalidInput);
            }
            for earlier in &self.bindings[..i] {
                if earlier.prefix == binding.prefix {
                    return Err(Error::DuplicatePrefix);
                }
                if earlier.input == binding.input {
                    return Err(Error::DuplicateInput);
                }
            }
        }
        Ok(())
    }
    /// 任意输入顺序产生同一规范编码；解码只接受严格递增的 prefix。
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut bindings: Vec<_> = self.bindings.iter().collect();
        bindings.sort_unstable_by(|a, b| a.prefix.cmp(&b.prefix));
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        word(&mut out, bindings.len())?;
        for b in bindings {
            for s in [&b.prefix, &b.input] {
                word(&mut out, s.len())?;
                out.extend_from_slice(s.as_bytes());
            }
        }
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(8)? != MAGIC || r.word()? != VERSION as usize {
            return Err(Error::Format);
        }
        let count = r.word()?;
        if count > (bytes.len() - r.pos) / 8 {
            return Err(Error::Format);
        }
        let mut bindings: Vec<Binding> = Vec::new();
        for _ in 0..count {
            let prefix = r.string()?;
            let input = r.string()?;
            if bindings.last().is_some_and(|prev| prev.prefix >= prefix) {
                return Err(Error::Format);
            }
            bindings.push(Binding { prefix, input });
        }
        if r.pos != bytes.len() {
            return Err(Error::Format);
        }
        let result = Self { bindings };
        result.validate()?;
        Ok(result)
    }
}
fn word(out: &mut Vec<u8>, n: usize) -> Result<(), Error> {
    out.extend_from_slice(
        &u32::try_from(n)
            .map_err(|_| Error::LengthOverflow)?
            .to_le_bytes(),
    );
    Ok(())
}
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Format)?;
        let result = self.bytes.get(self.pos..end).ok_or(Error::Format)?;
        self.pos = end;
        Ok(result)
    }
    fn word(&mut self) -> Result<usize, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| Error::Format)?) as usize)
    }
    fn string(&mut self) -> Result<String, Error> {
        let n = self.word()?;
        core::str::from_utf8(self.take(n)?)
            .map(String::from)
            .map_err(|_| Error::Format)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn binding(prefix: &str, i: usize) -> Binding {
        Binding {
            prefix: prefix.into(),
            input: input_name(i),
        }
    }
    #[test]
    fn canonical_unicode_roundtrip_and_truncations() {
        let d = Description {
            bindings: alloc::vec![binding("/雪", 1), binding("/", 0)],
        };
        let encoded = d.encode().unwrap();
        let decoded = Description::decode(&encoded).unwrap();
        assert_eq!(decoded.bindings[0].prefix, "/");
        assert_eq!(decoded.encode().unwrap(), encoded);
        for n in 0..encoded.len() {
            assert!(Description::decode(&encoded[..n]).is_err());
        }
        let mut trailing = encoded;
        trailing.push(0);
        assert!(Description::decode(&trailing).is_err());
    }
    #[test]
    fn reject_ambiguous_bindings_and_noncanonical_paths() {
        for bindings in [
            alloc::vec![binding("/", 0), binding("/", 1)],
            alloc::vec![binding("/", 0), binding("/a", 0)],
            alloc::vec![binding("/a/..", 0)],
        ] {
            assert!(Description { bindings }.encode().is_err());
        }
        for input in ["ordinary", "oal.entry.00", "oal.entry.", "oal.entry.-1"] {
            assert!(
                Description {
                    bindings: alloc::vec![Binding {
                        prefix: "/".into(),
                        input: input.into()
                    }]
                }
                .encode()
                .is_err()
            );
        }
        assert!(
            Description {
                bindings: alloc::vec![binding(&alloc::format!("/{}", "a".repeat(1000)), 0)]
            }
            .encode()
            .is_ok()
        );
    }
}
