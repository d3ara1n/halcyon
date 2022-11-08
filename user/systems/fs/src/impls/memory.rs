use crate::fs::{Directory, FileSystem, Stream};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec::{self, Vec}, borrow::ToOwned,
};
use unix_path::{Ancestors, Component, Components, Path};

use crate::fs::FileSystemError;

pub struct MemoryFs {
    mounted_at: String,
    root: Node,
}

enum Node {
    MountPoint(Box<dyn FileSystem>),
    Directory(BTreeMap<String, Node>),
    File(Data),
}

struct Data {
    data: Vec<u8>,
}

struct FileEntry {
    data: Vec<u8>,
}

impl FileSystem for MemoryFs {
    fn make_directory(&mut self, path: &str) -> Result<(), FileSystemError> {
        
        Ok(())
    }

    fn remove_directory(&mut self, path: &str) -> Result<(), FileSystemError> {
        todo!()
    }
}

impl MemoryFs {
    pub fn new(mount_point: &str) -> Self {
        Self {
            mounted_at: mount_point.to_string(),
            root: Node::Directory(BTreeMap::new()),
        }
    }

    fn get_entry_mut(&mut self, path: &str) -> Result<(), FileSystemError>{
        let buffer = Path::new(path);
        let relative = if buffer.is_absolute() {
            if let Ok(r) = buffer.strip_prefix(&self.mounted_at) {
                r
            } else {
                return Err(FileSystemError::PathInvalid);
            }
        } else {
            buffer
        };
        let mut components = relative.components();
        Self::get_entry_mut_internal(&mut components, &mut self.root)?;
        Ok(())
    }

    fn get_entry_mut_internal(
        relative: &mut Components,
        node: &mut Node,
    ) -> Result<bool, FileSystemError> {
        match relative.next() {
            Some(Component::RootDir) => panic!("unreachable for relative is relative"),
            Some(Component::CurDir) => {
                Self::get_entry_mut_internal(relative, node)
            }
            Some(Component::ParentDir) => Ok(true),
            Some(Component::Normal(normal)) => {
                match node {
                    Node::File(_) => Err(FileSystemError::NotDirectory),
                    Node::MountPoint(_) => todo!(),
                    Node::Directory(it) => {
                        let name = normal.to_str().unwrap().to_owned();
                        if let Some(entry) = it.get_mut(&name){
                            Self::get_entry_mut_internal(relative, entry)
                            
                        }else{
                            Err(FileSystemError::DirectoryNotFound)
                        }
                    }
                }
            }
            None => {
                // do the action
                Ok(false)
            }
        }
    }
}
