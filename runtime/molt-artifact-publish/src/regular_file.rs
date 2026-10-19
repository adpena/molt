use std::io;

/// Admit a regular artifact descriptor before any read or mmap. Stream inputs
/// use their bounded reader directly and deliberately do not use this owner.
/// Symlinks to regular files remain valid aliases; this admits the opened
/// descriptor type, while callers retain file-content and identity custody.
pub fn open_regular_artifact(path: &std::path::Path) -> io::Result<std::fs::File> {
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
