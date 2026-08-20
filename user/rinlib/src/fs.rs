use core::mem::size_of;

use alloc::{vec, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    fal::{DentryAttribute, DentryObject, DentryType, Mid, PropertyKind},
    path::Path,
};
use flagset::FlagSet;

use crate::call::{sys_access, sys_create, sys_inspect};

use self::components::{Dentry, Directory, Link, MountPoint, Property, Stream};

pub mod components;

#[derive(Debug, Clone, Copy)]
pub enum FileSystemError {
    Unknown,
    SerializationFailure,
    InvalidPath,
    NotFound,
    UnsupportedOperation,
    NotAccessible,
    // Filesystem mountpoint does not exist
    NotAvailable,
    SystemError,
}

impl From<SystemCallError> for FileSystemError {
    fn from(value: SystemCallError) -> Self {
        match value {
            SystemCallError::IllegalArgument => FileSystemError::InvalidPath,
            SystemCallError::ObjectNotAccessible => FileSystemError::NotAccessible,
            SystemCallError::ObjectNotFound => FileSystemError::NotFound,
            SystemCallError::NotSupported => FileSystemError::UnsupportedOperation,
            SystemCallError::InternalError => FileSystemError::SystemError,
            _ => FileSystemError::Unknown,
        }
    }
}

unsafe fn read_dentry_from_object_bytes(
    bytes: &[u8],
    count: usize,
    request_path: &str,
) -> Result<Dentry, FileSystemError> {
    unsafe {
    if count > 0 {
        let size = size_of::<DentryObject>();
        let first = &*(bytes.as_ptr() as *const DentryObject);
        let first_name =
            core::str::from_utf8_unchecked(&bytes[size..(size + first.name_length as usize)]);
        match first.kind {
            DentryType::Directory => {
                if count > 1 {
                    let mut children = Vec::<Dentry>::with_capacity(count - 1);
                    let mut pointer = size + (first.name_length as usize + 8 - 1) & !(8 - 1);
                    for _ in 0..(count - 1) {
                        let this = &*(bytes.as_ptr().add(pointer) as *const DentryObject);
                        let mut dir_path = Path::from(request_path).unwrap();
                        let name = core::str::from_utf8_unchecked(
                            &bytes[(pointer + size)..(pointer + size + this.name_length as usize)],
                        );
                        dir_path.append(name).unwrap();
                        let child =
                            read_dentry_from_object_bytes(&bytes[pointer..], 1, dir_path.as_str())?;
                        children.push(child);
                        pointer += size + ((this.name_length as usize + 8 - 1) & !(8 - 1));
                    }
                    Ok(Dentry::Directory(Directory::new(
                        first_name,
                        request_path,
                        first.created_at,
                        first.modified_at,
                        FlagSet::new(first.attr).unwrap(),
                        children,
                    )))
                } else {
                    Ok(Dentry::Directory(Directory::new(
                        first_name,
                        request_path,
                        first.created_at,
                        first.modified_at,
                        FlagSet::new(first.attr).unwrap(),
                        Vec::with_capacity(0),
                    )))
                }
            }
            DentryType::MountPoint => {
                if count > 1 {
                    let mounted = read_dentry_from_object_bytes(
                        &bytes[(size + ((first.name_length as usize + 8 - 1) & !(8 - 1)))..],
                        count - 1,
                        request_path,
                    )?;
                    Ok(Dentry::MountPoint(MountPoint::new(
                        first_name,
                        request_path,
                        first.created_at,
                        first.modified_at,
                        Some(mounted),
                    )))
                } else {
                    Ok(Dentry::MountPoint(MountPoint::new(
                        first_name,
                        request_path,
                        first.created_at,
                        first.modified_at,
                        None,
                    )))
                }
            }
            DentryType::Link => Ok(Dentry::Link(Link::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                FlagSet::new(first.attr).unwrap(),
            ))),
            DentryType::Stream => Ok(Dentry::Stream(Stream::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
            ))),
            DentryType::Boolean => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                1,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::Boolean,
            ))),
            DentryType::Integer => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::Integer,
            ))),
            DentryType::Integers => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::Integers,
            ))),
            DentryType::Decimal => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::Decimal,
            ))),
            DentryType::Decimals => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::Decimals,
            ))),
            DentryType::String => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::String,
            ))),
            DentryType::Blob => Ok(Dentry::Property(Property::new(
                first_name,
                request_path,
                first.created_at,
                first.modified_at,
                first.size as usize,
                FlagSet::new(first.attr).unwrap(),
                PropertyKind::Blob,
            ))),
        }
    } else {
        Err(FileSystemError::NotAvailable)
    }
    }
}

pub fn check(path: &str) -> Result<Dentry, FileSystemError> {
    unsafe {
        match sys_access(path) {
            Ok(size) => {
                let buffer = vec![0u8; size];
                match sys_inspect(path, &buffer) {
                    Ok(count) => read_dentry_from_object_bytes(&buffer, count, path),
                    Err(err) => Err(FileSystemError::from(err)),
                }
            }
            Err(err) => Err(FileSystemError::from(err)),
        }
    }
}

pub fn create_directory<A: Into<FlagSet<DentryAttribute>>>(
    path: &str,
    attr: A,
) -> Result<Directory, FileSystemError> {
    match unsafe { sys_create(path, DentryType::Directory, attr.into()) } {
        Ok(_) => match check(path) {
            Ok(Dentry::Directory(dir)) => Ok(dir),
            Ok(_) => Err(FileSystemError::SystemError),
            Err(err) => Err(FileSystemError::from(err)),
        },
        Err(err) => Err(FileSystemError::from(err)),
    }
}

pub fn create_stream<A: Into<FlagSet<DentryAttribute>>>(
    path: &str,
    attr: A,
) -> Result<Stream, FileSystemError> {
    match unsafe { sys_create(path, DentryType::Stream, attr.into()) } {
        Ok(_) => match check(path) {
            Ok(Dentry::Stream(stream)) => Ok(stream),
            Ok(_) => Err(FileSystemError::SystemError),
            Err(err) => Err(FileSystemError::from(err)),
        },
        Err(err) => Err(FileSystemError::from(err)),
    }
}

pub fn create_property<A: Into<FlagSet<DentryAttribute>>>(
    path: &str,
    kind: PropertyKind,
    attr: A,
) -> Result<Property, FileSystemError> {
    match unsafe { sys_create(path, DentryType::from(&kind), attr.into()) } {
        Ok(_) => match check(path) {
            Ok(Dentry::Property(prop)) => Ok(prop),
            Ok(_) => Err(FileSystemError::SystemError),
            Err(err) => Err(FileSystemError::from(err)),
        },
        Err(err) => Err(FileSystemError::from(err)),
    }
}

// 桩接口：签名即 API 契约，实现随内核 FAL 里程碑接通（expect 在实现
// 后变为未满足期望，提醒移除属性）。
#[expect(unused_variables, reason = "桩：实现随 FAL 里程碑")]
pub fn link(path: &str, target: &str) -> Result<(), FileSystemError> {
    todo!()
}

#[expect(unused_variables, reason = "桩：实现随 FAL 里程碑")]
pub fn delete(path: &str) -> Result<(), FileSystemError> {
    todo!()
}

#[expect(unused_variables, reason = "桩：实现随 FAL 里程碑")]
pub fn r#move(from: &str, to: &str) -> Result<(), FileSystemError> {
    todo!()
}

#[expect(unused_variables, reason = "桩：实现随 FAL 里程碑")]
pub fn mount(path: &str, mid: Mid) -> Result<(), FileSystemError> {
    todo!()
}

#[expect(unused_variables, reason = "桩：实现随 FAL 里程碑")]
pub fn unmount(path: &str) -> Result<(), FileSystemError> {
    todo!()
}
