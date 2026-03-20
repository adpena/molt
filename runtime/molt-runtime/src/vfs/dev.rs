//! Pseudo-device filesystem for /dev/stdin, /dev/stdout, /dev/stderr.

use std::sync::Mutex;
use crate::vfs::{VfsBackend, VfsError, VfsStat};

pub struct DevFs {
    stdout_buffer: Mutex<Vec<u8>>,
    stderr_buffer: Mutex<Vec<u8>>,
    stdin_data: Vec<u8>,
}

impl DevFs {
    pub fn new() -> Self {
        Self {
            stdout_buffer: Mutex::new(Vec::new()),
            stderr_buffer: Mutex::new(Vec::new()),
            stdin_data: Vec::new(),
        }
    }

    pub fn set_stdin(&mut self, data: Vec<u8>) {
        self.stdin_data = data;
    }

    pub fn take_stdout(&self) -> Vec<u8> {
        let mut buf = self.stdout_buffer.lock().unwrap();
        std::mem::take(&mut *buf)
    }

    pub fn take_stderr(&self) -> Vec<u8> {
        let mut buf = self.stderr_buffer.lock().unwrap();
        std::mem::take(&mut *buf)
    }
}

impl VfsBackend for DevFs {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        match path {
            "stdin" => Ok(self.stdin_data.clone()),
            "stdout" | "stderr" => Err(VfsError::PermissionDenied),
            _ => Err(VfsError::NotFound),
        }
    }

    fn open_write(&self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        match path {
            "stdout" => {
                let mut buf = self.stdout_buffer.lock().unwrap();
                buf.extend_from_slice(data);
                Ok(())
            }
            "stderr" => {
                let mut buf = self.stderr_buffer.lock().unwrap();
                buf.extend_from_slice(data);
                Ok(())
            }
            "stdin" => Err(VfsError::ReadOnly),
            _ => Err(VfsError::NotFound),
        }
    }

    fn open_append(&self, path: &str, data: &[u8]) -> Result<(), VfsError> {
        self.open_write(path, data)
    }

    fn stat(&self, path: &str) -> Result<VfsStat, VfsError> {
        match path {
            "stdin" | "stdout" | "stderr" => Ok(VfsStat {
                is_file: true,
                is_dir: false,
                size: 0,
                readonly: path == "stdin",
                mtime: 0,
            }),
            "" => Ok(VfsStat {
                is_file: false,
                is_dir: true,
                size: 0,
                readonly: true,
                mtime: 0,
            }),
            _ => Err(VfsError::NotFound),
        }
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError> {
        if path.is_empty() {
            Ok(vec!["stdin".into(), "stdout".into(), "stderr".into()])
        } else {
            Err(VfsError::NotDirectory)
        }
    }

    fn mkdir(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::ReadOnly) }
    fn unlink(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::PermissionDenied) }
    fn rename(&self, _from: &str, _to: &str) -> Result<(), VfsError> { Err(VfsError::PermissionDenied) }

    fn exists(&self, path: &str) -> bool {
        matches!(path, "" | "stdin" | "stdout" | "stderr")
    }

    fn is_readonly(&self) -> bool {
        false // stdout/stderr are writable
    }
}
