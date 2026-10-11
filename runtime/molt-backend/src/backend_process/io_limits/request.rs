use std::io::{self, Read};

#[derive(Debug)]
pub(crate) struct RequestBoundedRead<R> {
    pub(crate) inner: R,
    pub(crate) remaining: usize,
    pub(crate) limit_bytes: usize,
    pub(crate) context: &'static str,
}

impl<R: Read> RequestBoundedRead<R> {
    pub(crate) fn new(inner: R, limit_bytes: usize, context: &'static str) -> Self {
        Self {
            inner,
            remaining: limit_bytes,
            limit_bytes,
            context,
        }
    }
}

impl<R: Read> Read for RequestBoundedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe) {
                Ok(0) => Ok(0),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} exceeded {} byte limit", self.context, self.limit_bytes),
                )),
                Err(err) => Err(err),
            };
        }

        let read_len = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..read_len])?;
        self.remaining = self.remaining.saturating_sub(n);
        Ok(n)
    }
}

pub(crate) fn read_bounded_request_bytes<R: Read>(
    reader: R,
    limit_bytes: usize,
    context: &'static str,
) -> io::Result<Vec<u8>> {
    let mut bounded = RequestBoundedRead::new(reader, limit_bytes, context);
    let mut bytes = Vec::new();
    bounded.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Admit a regular artifact descriptor before any read or mmap. Stream inputs
/// use RequestBoundedRead directly and deliberately do not use this owner.
pub(crate) fn open_regular_artifact(path: &std::path::Path) -> io::Result<std::fs::File> {
    #[cfg(any(unix, windows))]
    {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A no-writer FIFO must not block before descriptor admission.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
            // Reject pipes/devices before requesting ordinary file metadata.
            if unsafe { GetFileType(file.as_raw_handle() as _) } != FILE_TYPE_DISK {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "artifact input must be a regular disk file",
                ));
            }
        }
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "artifact input must be a regular file",
            ));
        }
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "regular artifact descriptor admission is unavailable on this host",
        ))
    }
}
