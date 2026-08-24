use alloc::{borrow::ToOwned, boxed::Box, string::String, vec, vec::Vec};
use erhino_shared::{
    fal::{DentryAttribute, PropertyKind},
    time::Timestamp,
};
use flagset::FlagSet;

use crate::{
    call::{sys_read, sys_write},
};

use super::FileSystemError;

macro_rules! dentry_method {
    ($method: ident, $result: ty) => {
        pub fn $method(&self) -> $result {
            match self {
                Dentry::Directory(directory) => directory.$method(),
                Dentry::Link(link) => link.$method(),
                Dentry::MountPoint(mountpoint) => mountpoint.$method(),
                Dentry::Property(property) => property.$method(),
                Dentry::Stream(stream) => stream.$method(),
            }
        }
    };
}

macro_rules! dentry_sub_methods {
    () => {
        pub fn name(&self) -> &str {
            &self.name
        }

        pub fn fullname(&self) -> &str {
            &self.fullname
        }

        pub fn created_at(&self) -> Timestamp {
            self.created_at
        }

        pub fn modified_at(&self) -> Timestamp {
            self.modified_at
        }

        pub fn r#move(&mut self, to: &str) -> Result<(), FileSystemError> {
            super::r#move(&self.fullname, to)?;
            self.fullname = to.to_owned();
            Ok(())
        }

        pub fn delete(self) -> Result<(), FileSystemError> {
            super::delete(&self.fullname)
        }
    };
}

pub enum Dentry {
    Directory(Directory),
    Link(Link),
    Stream(Stream),
    Property(Property),
    MountPoint(MountPoint),
}

impl Dentry {
    dentry_method!(name, &str);

    dentry_method!(fullname, &str);

    dentry_method!(created_at, Timestamp);

    dentry_method!(modified_at, Timestamp);
}

#[derive(Debug)]
pub enum PropertyValue {
    Boolean(bool),
    Integer(i64),
    Integers(Vec<i64>),
    Decimal(f64),
    Decimals(Vec<f64>),
    String(String),
    Blob(Vec<u8>),
}

impl PropertyValue {
    fn from_bytes(kind: PropertyKind, bytes: Vec<u8>) -> Result<Self, ()> {
        match kind {
            PropertyKind::Boolean => {
                if bytes.len() == 1 {
                    Ok(PropertyValue::Boolean(bytes[0] > 0))
                } else {
                    Err(())
                }
            }
            PropertyKind::Integer => {
                if bytes.len() == 8 {
                    Ok(PropertyValue::Integer(i64::from_ne_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ])))
                } else {
                    Err(())
                }
            }
            PropertyKind::Integers => {
                if bytes.len() % 8 == 0 {
                    let count = bytes.len() / 8;
                    let mut ints = Vec::<i64>::with_capacity(count);
                    for i in 0..count {
                        let int = i64::from_ne_bytes([
                            bytes[i * 8 + 0],
                            bytes[i * 8 + 1],
                            bytes[i * 8 + 2],
                            bytes[i * 8 + 3],
                            bytes[i * 8 + 4],
                            bytes[i * 8 + 5],
                            bytes[i * 8 + 6],
                            bytes[i * 8 + 7],
                        ]);
                        ints.push(int);
                    }
                    ints.truncate(count);
                    Ok(PropertyValue::Integers(ints))
                } else {
                    Err(())
                }
            }
            PropertyKind::Decimal => {
                if bytes.len() == 8 {
                    Ok(PropertyValue::Decimal(f64::from_ne_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ])))
                } else {
                    Err(())
                }
            }
            PropertyKind::Decimals => {
                if bytes.len() % 8 == 0 {
                    let count = bytes.len() / 8;
                    let mut decs = Vec::<f64>::with_capacity(count);
                    for i in 0..count {
                        let int = f64::from_ne_bytes([
                            bytes[i * 8 + 0],
                            bytes[i * 8 + 1],
                            bytes[i * 8 + 2],
                            bytes[i * 8 + 3],
                            bytes[i * 8 + 4],
                            bytes[i * 8 + 5],
                            bytes[i * 8 + 6],
                            bytes[i * 8 + 7],
                        ]);
                        decs.push(int);
                    }
                    decs.truncate(count);
                    Ok(PropertyValue::Decimals(decs))
                } else {
                    Err(())
                }
            }
            PropertyKind::String => {
                if let Ok(s) = String::from_utf8(bytes) {
                    Ok(PropertyValue::String(s))
                } else {
                    Err(())
                }
            }
            PropertyKind::Blob => Ok(PropertyValue::Blob(bytes)),
        }
    }

    fn to_bytes(self) -> Result<Vec<u8>, ()> {
        match self {
            PropertyValue::Boolean(it) => Ok(vec![if it { 1u8 } else { 0u8 }]),
            PropertyValue::Integer(it) => Ok(i64::to_ne_bytes(it).to_vec()),
            PropertyValue::Integers(it) => {
                Ok(it.iter().flat_map(|i| i64::to_ne_bytes(*i)).collect())
            }
            PropertyValue::Decimal(it) => Ok(f64::to_ne_bytes(it).to_vec()),
            PropertyValue::Decimals(it) => {
                Ok(it.iter().flat_map(|f| f64::to_ne_bytes(*f)).collect())
            }
            PropertyValue::String(it) => Ok(it.as_bytes().to_vec()),
            PropertyValue::Blob(it) => Ok(it),
        }
    }
}

pub struct StreamValue {
    bytes: Vec<u8>,
}

impl StreamValue {
    fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub struct Directory {
    name: String,
    fullname: String,
    created_at: Timestamp,
    modified_at: Timestamp,
    attributes: FlagSet<DentryAttribute>,
    children: Vec<Dentry>,
}

impl Directory {
    pub(crate) fn new(
        name: &str,
        fullname: &str,
        created: Timestamp,
        modified: Timestamp,
        attr: FlagSet<DentryAttribute>,
        children: Vec<Dentry>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            fullname: fullname.to_owned(),
            created_at: created,
            modified_at: modified,
            attributes: attr,
            children,
        }
    }

    dentry_sub_methods!();

    pub fn attributes(&self) -> &FlagSet<DentryAttribute> {
        &self.attributes
    }

    pub fn children(&self) -> &[Dentry] {
        &self.children
    }
}

pub struct Link {
    name: String,
    fullname: String,
    created_at: Timestamp,
    modified_at: Timestamp,
    attributes: FlagSet<DentryAttribute>,
}

impl Link {
    pub(crate) fn new(
        name: &str,
        fullname: &str,
        created: Timestamp,
        modified: Timestamp,
        attr: FlagSet<DentryAttribute>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            fullname: fullname.to_owned(),
            created_at: created,
            modified_at: modified,
            attributes: attr,
        }
    }

    dentry_sub_methods!();

    pub fn attributes(&self) -> &FlagSet<DentryAttribute> {
        &self.attributes
    }
}

pub struct Stream {
    name: String,
    fullname: String,
    created_at: Timestamp,
    modified_at: Timestamp,
    size: usize,
    attributes: FlagSet<DentryAttribute>,
}

impl Stream {
    pub(crate) fn new(
        name: &str,
        fullname: &str,
        created: Timestamp,
        modified: Timestamp,
        size: usize,
        attr: FlagSet<DentryAttribute>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            fullname: fullname.to_owned(),
            created_at: created,
            modified_at: modified,
            size,
            attributes: attr,
        }
    }

    dentry_sub_methods!();

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn attributes(&self) -> &FlagSet<DentryAttribute> {
        &self.attributes
    }

    pub fn read(&self, length: usize) -> Result<StreamValue, FileSystemError> {
        let mut buffer = vec![0u8; length];
        unsafe {
            match sys_read(&self.fullname, &mut buffer) {
                Ok(read) => {
                    buffer.truncate(read);
                    Ok(StreamValue::from_bytes(buffer))
                }
                Err(err) => Err(FileSystemError::from(err)),
            }
        }
    }
}

pub struct Property {
    name: String,
    fullname: String,
    created_at: Timestamp,
    modified_at: Timestamp,
    size: usize,
    attributes: FlagSet<DentryAttribute>,
    kind: PropertyKind,
}

impl Property {
    pub(crate) fn new(
        name: &str,
        fullname: &str,
        created: Timestamp,
        modified: Timestamp,
        size: usize,
        attr: FlagSet<DentryAttribute>,
        kind: PropertyKind,
    ) -> Self {
        Self {
            name: name.to_owned(),
            fullname: fullname.to_owned(),
            created_at: created,
            modified_at: modified,
            size,
            attributes: attr,
            kind,
        }
    }

    dentry_sub_methods!();

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn attributes(&self) -> &FlagSet<DentryAttribute> {
        &self.attributes
    }

    pub fn kind(&self) -> PropertyKind {
        self.kind
    }

    pub fn read(&self) -> Result<PropertyValue, FileSystemError> {
        let mut buffer = vec![0u8; self.size];
        unsafe {
            match sys_read(&self.fullname, &mut buffer) {
                Ok(read) => {
                    buffer.truncate(read);
                    PropertyValue::from_bytes(self.kind, buffer)
                        .map_err(|_| FileSystemError::SerializationFailure)
                }
                Err(err) => Err(FileSystemError::from(err)),
            }
        }
    }

    pub fn write(&mut self, value: PropertyValue) -> Result<(), FileSystemError> {
        if let Ok(bytes) = value.to_bytes() {
            unsafe {
                match sys_write(&self.fullname, &bytes) {
                    Ok(()) => {
                        self.size = bytes.len();
                        Ok(())
                    }
                    Err(err) => Err(FileSystemError::from(err)),
                }
            }
        } else {
            Err(FileSystemError::SerializationFailure)
        }
    }
}

pub struct MountPoint {
    name: String,
    fullname: String,
    created_at: Timestamp,
    modified_at: Timestamp,
    mounted: Option<Box<Dentry>>,
}

impl MountPoint {
    pub(crate) fn new(
        name: &str,
        fullname: &str,
        created: Timestamp,
        modified: Timestamp,
        mounted: Option<Dentry>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            fullname: fullname.to_owned(),
            created_at: created,
            modified_at: modified,
            mounted: mounted.map(|d| Box::new(d)),
        }
    }

    dentry_sub_methods!();

    pub fn mounted(&self) -> Option<&Dentry> {
        self.mounted.as_ref().map(|d| d.as_ref())
    }
}
