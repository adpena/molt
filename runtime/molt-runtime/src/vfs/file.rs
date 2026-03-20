//! File handle wrapper bridging VFS Vec<u8> operations with the runtime's
//! file object expectations (cursor-based read, buffered write, close flush).

use crate::vfs::{VfsBackend, VfsError};
use std::sync::Arc;

pub struct MoltVfsFile {
    content: Vec<u8>,
    cursor: usize,
    path: String,
    writable: bool,
    dirty: bool,
    backend: Arc<dyn VfsBackend>,
}

impl MoltVfsFile {
    pub fn open_read(backend: Arc<dyn VfsBackend>, path: &str) -> Result<Self, VfsError> {
        let content = backend.open_read(path)?;
        Ok(Self {
            content,
            cursor: 0,
            path: path.to_string(),
            writable: false,
            dirty: false,
            backend,
        })
    }

    pub fn open_write(backend: Arc<dyn VfsBackend>, path: &str) -> Result<Self, VfsError> {
        // Start with empty content for write mode
        Ok(Self {
            content: Vec::new(),
            cursor: 0,
            path: path.to_string(),
            writable: true,
            dirty: false,
            backend,
        })
    }

    pub fn open_append(backend: Arc<dyn VfsBackend>, path: &str) -> Result<Self, VfsError> {
        let content = backend.open_read(path).unwrap_or_default();
        let cursor = content.len();
        Ok(Self {
            content,
            cursor,
            path: path.to_string(),
            writable: true,
            dirty: false,
            backend,
        })
    }

    pub fn read(&mut self, n: usize) -> Vec<u8> {
        let end = (self.cursor + n).min(self.content.len());
        let data = self.content[self.cursor..end].to_vec();
        self.cursor = end;
        data
    }

    pub fn read_all(&mut self) -> Vec<u8> {
        let data = self.content[self.cursor..].to_vec();
        self.cursor = self.content.len();
        data
    }

    pub fn write(&mut self, data: &[u8]) -> Result<usize, VfsError> {
        if !self.writable {
            return Err(VfsError::ReadOnly);
        }
        self.content.extend_from_slice(data);
        self.cursor = self.content.len();
        self.dirty = true;
        Ok(data.len())
    }

    pub fn flush(&mut self) -> Result<(), VfsError> {
        if self.dirty && self.writable {
            self.backend.open_write(&self.path, &self.content)?;
            self.dirty = false;
        }
        Ok(())
    }

    pub fn close(mut self) -> Result<(), VfsError> {
        self.flush()
    }

    pub fn tell(&self) -> usize {
        self.cursor
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}
