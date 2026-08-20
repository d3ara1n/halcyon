use core::{fmt::Display, ops::Div, str::Split};

use alloc::{
    borrow::ToOwned,
    string::{String, ToString},
    vec::Vec,
};

/// The separator of path for nodes
pub const PATH_SEPARATOR: char = '/';
/// Refused characters for path
pub const INVALID_CHARACTERS: [char; 1] = ['\0'];

/// Parts of path
#[derive(Debug)]
pub enum Component<'a> {
    /// /
    Root,
    /// .
    Current,
    /// ..
    Parent,
    /// Valid filename
    Normal(&'a str),
}

impl<'a> Component<'a> {
    pub fn as_str(&self) -> &str {
        match self {
            Component::Root => "",
            Component::Current => ".",
            Component::Parent => "..",
            Component::Normal(it) => it,
        }
    }
}

/// Path parsing error
#[derive(Debug)]
pub enum PathError {
    /// Containing zero byte
    InvalidCharacters,
    /// Making absolute path requires root node
    MissingRoot,
    /// Reaching the end of root node
    OverflowRoot,
}

/// Representing a file or directory
#[derive(Debug, Clone)]
pub struct Path {
    inner: String,
}

impl Path {
    /// Construct a path from string
    pub fn from(s: &str) -> Result<Self, PathError> {
        if Self::is_valid(s) {
            Ok(Self::from_string_unchecked(s.to_owned()))
        } else {
            Err(PathError::InvalidCharacters)
        }
    }

    /// Is string a valid file name(containing no /)
    pub fn is_filename(s: &str) -> bool {
        !s.contains(PATH_SEPARATOR)
    }

    /// Is string a valid path(containing no zero-byte)
    pub fn is_valid(s: &str) -> bool {
        !INVALID_CHARACTERS.iter().any(|c| s.contains(*c))
    }

    /// Is path starting from root
    pub fn is_absolute(&self) -> bool {
        self.inner.starts_with(PATH_SEPARATOR)
    }

    /// Is path contains no ./..
    pub fn is_qualified(&self) -> bool {
        !self.inner.contains(".") && !self.inner.contains("..")
    }

    /// Construct an absolute path with no . or ..
    pub fn qualify(&self) -> Result<Path, PathError> {
        if self.is_absolute() {
            let is_dir = self.inner.ends_with(PATH_SEPARATOR);
            let mut buffer = Vec::<&str>::new();
            for c in self.iter() {
                match c {
                    Component::Current => {}
                    Component::Root => buffer.push(""),
                    Component::Parent => {
                        if buffer.len() > 1 {
                            buffer.pop();
                        } else {
                            return Err(PathError::OverflowRoot);
                        }
                    }
                    Component::Normal(normal) => buffer.push(normal),
                }
            }
            if is_dir {
                buffer.push("");
            }
            let path = buffer.join(&PATH_SEPARATOR.to_string());
            Path::from(&path)
        } else {
            Err(PathError::MissingRoot)
        }
    }

    /// Get its filename with no SEPARATOR char and parent
    pub fn filename(&self) -> &str {
        let non_separator_terminated = self.get_non_separated_terminated();
        if let Some(p) = Self::get_break_position(non_separator_terminated) {
            &non_separator_terminated[(p + 1)..]
        } else {
            &non_separator_terminated
        }
    }

    /// Get its parent full path regardless it's directory or file
    pub fn parent(&self) -> Option<Path> {
        let non_separator_terminated = self.get_non_separated_terminated();
        if let Some(p) = Self::get_break_position(non_separator_terminated) {
            // its a dir so must be containing / at the end
            Some(Path::from_string_unchecked(
                (&non_separator_terminated[..(p + 1)]).to_owned(),
            ))
        } else {
            None
        }
    }

    /// Append sub path into it
    pub fn append(&mut self, s: &str) -> Result<&str, PathError> {
        if Self::is_valid(s) {
            if self.inner == "" {
                self.inner.push_str(s);
                Ok(&self.inner)
            } else {
                if self.inner.ends_with(PATH_SEPARATOR) {
                    if s.starts_with(PATH_SEPARATOR) {
                        self.inner.pop();
                    }
                    self.inner.push_str(s);
                    Ok(&self.inner)
                } else {
                    if !s.starts_with(PATH_SEPARATOR) {
                        self.inner.push(PATH_SEPARATOR);
                    }
                    self.inner.push_str(s);
                    Ok(&self.inner)
                }
            }
        } else {
            Err(PathError::InvalidCharacters)
        }
    }

    /// Prepend sub path into it
    pub fn prepend(&mut self, s: &str) -> Result<&str, PathError> {
        if Self::is_valid(s) {
            let mut buffer = String::from(s);
            if self.inner == "" {
                self.inner = buffer;
                Ok(&self.inner)
            } else {
                if buffer.ends_with(PATH_SEPARATOR) {
                    if self.inner.starts_with(PATH_SEPARATOR) {
                        buffer.pop();
                    }
                    buffer.push_str(&self.inner);
                    self.inner = buffer;
                    Ok(&self.inner)
                } else {
                    if !self.inner.starts_with(PATH_SEPARATOR) {
                        buffer.push(PATH_SEPARATOR);
                    }
                    buffer.push_str(&self.inner);
                    self.inner = buffer;
                    Ok(&self.inner)
                }
            }
        } else {
            Err(PathError::InvalidCharacters)
        }
    }

    /// Get a iterator to iterate over path components
    pub fn iter(&self) -> PathIterator<'_> {
        PathIterator::from_path(self)
    }

    /// Get its str reference
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn make_root(&mut self) {
        if !self.is_absolute() {
            let mut buffer = String::new();
            buffer.push(PATH_SEPARATOR);
            buffer.push_str(&self.inner);
            self.inner = buffer
        }
    }

    fn get_non_separated_terminated(&self) -> &str {
        let offset = if self.inner.ends_with(PATH_SEPARATOR) {
            // must be directory
            1
        } else {
            // file
            0
        };
        &self.inner[..(self.inner.len() - offset)]
    }

    fn get_break_position(non_separator_terminated: &str) -> Option<usize> {
        let mut position = 0usize;
        for c in non_separator_terminated.chars().rev() {
            if c == PATH_SEPARATOR {
                return Some(non_separator_terminated.len() - 1 - position);
            } else {
                position += c.len_utf8();
            }
        }
        None
    }

    fn from_string_unchecked(string: String) -> Self {
        Self { inner: string }
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Div<&str> for &Path {
    type Output = Path;

    fn div(self, rhs: &str) -> Self::Output {
        if Path::is_valid(rhs) {
            let mut string = String::from(self.get_non_separated_terminated());
            if rhs.starts_with(PATH_SEPARATOR) {
                string.push_str(rhs);
            } else {
                string.push(PATH_SEPARATOR);
                string.push_str(rhs);
            }
            Path::from_string_unchecked(string)
        } else {
            panic!("string is not valid path");
        }
    }
}

/// Iterates over components
pub struct PathIterator<'a> {
    split: Split<'a, char>,
    has_root: bool,
}

impl<'a> PathIterator<'a> {
    fn from_path(path: &'a Path) -> Self {
        Self {
            split: path.get_non_separated_terminated().split(PATH_SEPARATOR),
            has_root: path.is_absolute(),
        }
    }

    pub fn collect_remaining(mut self) -> Path {
        let mut buffer = Vec::<String>::new();
        while let Some(next) = self.next() {
            buffer.push(next.as_str().to_owned());
        }
        Path::from_string_unchecked(buffer.join(&PATH_SEPARATOR.to_string()))
    }
}

impl<'a> Iterator for PathIterator<'a> {
    type Item = Component<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(next) = self.split.next() {
            Some(match next {
                "" => {
                    if self.has_root {
                        self.has_root = false;
                        Component::Root
                    } else {
                        Component::Normal("")
                    }
                }
                "." => Component::Current,
                ".." => Component::Parent,
                _ => Component::Normal(next),
            })
        } else {
            None
        }
    }
}
