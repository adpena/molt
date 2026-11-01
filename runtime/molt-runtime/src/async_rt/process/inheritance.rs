//! Descriptor inheritance for every launch through the process authority.
//!
//! Change flags only in the forked child. Mark unwanted descriptors CLOEXEC,
//! rather than closing Rust's exec-error pipe before it can report a failure.
//! No allocation, locks, or directory-library state is used after fork.

use std::io;
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct ChildDescriptor(pub(super) i32);

pub(super) fn configure(
    command: &mut Command,
    close_fds: bool,
    mut keep: Vec<ChildDescriptor>,
) -> io::Result<()> {
    keep.sort_unstable();
    keep.dedup();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        for descriptor in &keep {
            if descriptor.0 < 0 || unsafe { libc::fcntl(descriptor.0, libc::F_GETFD) } < 0 {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        if close_fds {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "descriptor-table inheritance is unavailable on this Unix host",
            ));
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let mut table = close_fds.then(DescriptorTable::prepare).transpose()?;
        if !close_fds && keep.is_empty() {
            return Ok(());
        }
        unsafe {
            command.pre_exec(move || {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                if let Some(table) = table.as_mut() {
                    table.close_on_exec()?;
                }
                for descriptor in &keep {
                    set_cloexec(descriptor.0, false)?;
                }
                Ok(())
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (command, close_fds);
        if !keep.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "pass_fds is supported only on POSIX hosts",
            ));
        }
        // Windows multiprocessing pulls non-inheritable handles from its
        // parent; it never puts them in the Command's inheritance set.
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn set_cloexec(fd: i32, enabled: bool) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let next = if enabled {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    if next != flags && unsafe { libc::fcntl(fd, libc::F_SETFD, next) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
struct DescriptorTable {
    buffer: Vec<std::mem::MaybeUninit<libc::proc_fdinfo>>,
}

#[cfg(target_os = "macos")]
impl DescriptorTable {
    fn prepare() -> io::Result<Self> {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let current = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        if current <= 0 {
            return Err(io::Error::last_os_error());
        }
        let item_size = std::mem::size_of::<libc::proc_fdinfo>();
        let count = usize::try_from(unsafe { limit.assume_init() }.rlim_cur)
            .ok()
            .and_then(|limit| limit.max(current as usize / item_size).checked_add(1))
            .filter(|count| *count <= i32::MAX as usize / item_size)
            .ok_or_else(|| {
                io::Error::other("descriptor table exceeds proc_pidinfo's size limit")
            })?;
        let mut buffer = Vec::new();
        buffer.try_reserve_exact(count).map_err(io::Error::other)?;
        // The kernel initializes only the returned entries. Spare capacity is
        // not read and allocating it does not touch every possible fd slot.
        buffer.resize_with(count, std::mem::MaybeUninit::uninit);
        Ok(Self { buffer })
    }

    fn close_on_exec(&mut self) -> io::Result<()> {
        let size = std::mem::size_of::<libc::proc_fdinfo>();
        let bytes = (self.buffer.len() * size) as i32;
        // Apple's libproc wrapper is a direct __proc_info syscall wrapper.
        // Query the child's table after fork, never a racy parent snapshot.
        let read = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                self.buffer.as_mut_ptr().cast(),
                bytes,
            )
        };
        if read <= 0 {
            return Err(io::Error::last_os_error());
        }
        if read >= bytes || !(read as usize).is_multiple_of(size) {
            return Err(io::Error::from_raw_os_error(libc::EOVERFLOW));
        }
        for entry in &self.buffer[..read as usize / size] {
            let fd = unsafe { entry.assume_init_ref() }.proc_fd;
            if fd >= 3 {
                set_cloexec(fd, true)?;
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
struct DescriptorTable;

#[cfg(target_os = "linux")]
impl DescriptorTable {
    fn prepare() -> io::Result<Self> {
        // Preflight the required kernel interface before fork. The child
        // opens its own /proc/self/fd, not a directory bound to the parent.
        std::fs::read_dir("/proc/self/fd")?;
        Ok(Self)
    }

    fn close_on_exec(&mut self) -> io::Result<()> {
        let dir = unsafe {
            libc::open(
                c"/proc/self/fd".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if dir < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut buffer = [0u8; 8192];
            loop {
                let bytes = unsafe {
                    libc::syscall(libc::SYS_getdents64, dir, buffer.as_mut_ptr(), buffer.len())
                };
                if bytes < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                if bytes == 0 {
                    return Ok(());
                }
                let mut offset = 0;
                while offset < bytes as usize {
                    // linux_dirent64: ino:u64, offset:i64, reclen:u16,
                    // type:u8, followed by the NUL-terminated decimal name.
                    if offset + 19 > bytes as usize {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    let length =
                        u16::from_ne_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
                    if length < 20 || offset + length > bytes as usize {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    let mut fd = Some(0i32);
                    for &byte in &buffer[offset + 19..offset + length] {
                        if byte == 0 {
                            break;
                        }
                        fd = fd.and_then(|value| {
                            byte.is_ascii_digit().then_some(())?;
                            value.checked_mul(10)?.checked_add(i32::from(byte - b'0'))
                        });
                    }
                    if let Some(fd) = fd.filter(|fd| *fd >= 3) {
                        set_cloexec(fd, true)?;
                    }
                    offset += length;
                }
            }
        })();
        unsafe {
            libc::close(dir);
        }
        result
    }
}
