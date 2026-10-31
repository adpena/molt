//! Anonymous, dedicated multiprocessing pipes. No program stdio is a channel.

#[cfg(unix)]
use super::inheritance::{self, ChildDescriptor};
use os_pipe::{PipeReader, PipeWriter};
use std::io;
use std::process::{Child, Command};

const BOOTSTRAP_PREFIX: &str = "--molt-ipc=";
#[cfg(windows)]
const READY: u8 = 0xa7;

struct ChildPipes {
    reader: PipeReader,
    writer: PipeWriter,
}

pub(super) struct PreparedIpc {
    reader: PipeReader,
    writer: PipeWriter,
    child: ChildPipes,
}

impl PreparedIpc {
    pub(super) fn new(command: &mut Command) -> io::Result<Self> {
        let (child_reader, writer) = os_pipe::pipe()?;
        let (reader, child_writer) = os_pipe::pipe()?;
        let child = ChildPipes {
            reader: normalize_reader(child_reader)?,
            writer: normalize_writer(child_writer)?,
        };
        let reader = normalize_reader(reader)?;
        let writer = normalize_writer(writer)?;
        let bootstrap = Bootstrap {
            parent: std::process::id(),
            reader: pipe_identity(&child.reader),
            writer: pipe_identity(&child.writer),
        };
        command
            .arg("--multiprocessing-fork")
            .arg(bootstrap.argument());
        #[cfg(unix)]
        inheritance::configure(
            command,
            true,
            vec![
                ChildDescriptor(bootstrap.reader as i32),
                ChildDescriptor(bootstrap.writer as i32),
            ],
        )?;
        Ok(Self {
            reader,
            writer,
            child,
        })
    }

    pub(super) fn finish(mut self, child: &mut Child) -> io::Result<(PipeReader, PipeWriter)> {
        #[cfg(windows)]
        {
            use molt_runtime_platform::windows_abi::PeekNamedPipe;
            use std::io::Read;
            use std::os::windows::io::AsRawHandle;
            use std::time::{Duration, Instant};
            let deadline = Instant::now() + Duration::from_secs(30);
            // The child acknowledges only after both non-inheritable source
            // handles have been duplicated and converted to owned CRT fds.
            // Retain the sources until then; dropping them earlier races the
            // child, retaining them afterwards prevents EOF and broken pipe.
            loop {
                let mut available = 0;
                if unsafe {
                    PeekNamedPipe(
                        self.reader.as_raw_handle(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if available != 0 {
                    let mut ready = [0];
                    self.reader.read_exact(&mut ready)?;
                    if ready != [READY] {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid multiprocessing pipe acknowledgement",
                        ));
                    }
                    break;
                }
                if child.try_wait()?.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "multiprocessing child exited before accepting its IPC pipes",
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "multiprocessing child did not accept its IPC pipes",
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        #[cfg(not(windows))]
        let _ = (&mut self, child);
        drop(self.child);
        Ok((self.reader, self.writer))
    }
}

#[derive(Debug)]
struct Bootstrap {
    parent: u32,
    reader: usize,
    writer: usize,
}

impl Bootstrap {
    fn argument(&self) -> String {
        format!(
            "{BOOTSTRAP_PREFIX}{}:{}:{}",
            self.parent, self.reader, self.writer
        )
    }

    fn current() -> io::Result<Self> {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--multiprocessing-fork")) {
            return Err(invalid_bootstrap());
        }
        let argument = args.next().ok_or_else(invalid_bootstrap)?;
        if args.next().is_some() {
            return Err(invalid_bootstrap());
        }
        Self::parse(argument.to_str().ok_or_else(invalid_bootstrap)?)
    }

    fn parse(argument: &str) -> io::Result<Self> {
        let mut fields = argument
            .strip_prefix(BOOTSTRAP_PREFIX)
            .ok_or_else(invalid_bootstrap)?
            .split(':');
        let parent = fields
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value != 0)
            .ok_or_else(invalid_bootstrap)?;
        let reader = fields
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(invalid_bootstrap)?;
        let writer = fields
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(invalid_bootstrap)?;
        if fields.next().is_some() || reader == writer {
            return Err(invalid_bootstrap());
        }
        Ok(Self {
            parent,
            reader,
            writer,
        })
    }
}

fn invalid_bootstrap() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "missing or invalid dedicated multiprocessing IPC bootstrap",
    )
}

/// A child CRT/POSIX descriptor owns exactly one pipe endpoint. This guard
/// also closes partial success if creating the pair or its Python tuple fails.
pub(super) struct ChildPipeDescriptor(i32);

impl ChildPipeDescriptor {
    pub(super) fn raw(&self) -> i32 {
        self.0
    }
    pub(super) fn into_raw(self) -> i32 {
        let fd = self.0;
        std::mem::forget(self);
        fd
    }
}

impl Drop for ChildPipeDescriptor {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

pub(super) fn accept_child() -> io::Result<(ChildPipeDescriptor, ChildPipeDescriptor)> {
    let bootstrap = Bootstrap::current()?;
    // Endpoint ownership is transferred once, including when a partial claim
    // fails. Retrying could adopt a descriptor reused by unrelated program I/O.
    static CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if CLAIMED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "multiprocessing IPC endpoints have already been claimed",
        ));
    }
    #[cfg(unix)]
    {
        fn adopt(raw: usize, access: i32) -> io::Result<ChildPipeDescriptor> {
            let fd = i32::try_from(raw)
                .ok()
                .filter(|fd| *fd >= 3)
                .ok_or_else(invalid_bootstrap)?;
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if flags & libc::O_ACCMODE != access
                || unsafe { stat.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO
            {
                return Err(invalid_bootstrap());
            }
            let owned = ChildPipeDescriptor(fd);
            inheritance::set_cloexec(fd, true)?;
            Ok(owned)
        }
        let reader = adopt(bootstrap.reader, libc::O_RDONLY)?;
        let writer = adopt(bootstrap.writer, libc::O_WRONLY)?;
        Ok((reader, writer))
    }
    #[cfg(windows)]
    {
        use molt_runtime_platform::windows_abi::{OpenProcess, PROCESS_DUP_HANDLE};
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        let process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, bootstrap.parent) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        let reader = pull_handle(&process, bootstrap.reader, libc::O_RDONLY)?;
        let writer = pull_handle(&process, bootstrap.writer, libc::O_WRONLY)?;
        if unsafe { libc::write(writer.0, (&READY as *const u8).cast(), 1) } != 1 {
            return Err(io::Error::last_os_error());
        }
        Ok((reader, writer))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = bootstrap;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "dedicated multiprocessing pipes are unavailable on this host",
        ))
    }
}

#[cfg(windows)]
fn pull_handle(
    parent: &std::os::windows::io::OwnedHandle,
    source: usize,
    access: i32,
) -> io::Result<ChildPipeDescriptor> {
    use molt_runtime_platform::windows_abi::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, FILE_TYPE_PIPE, GetCurrentProcess, GetFileType,
        HANDLE_FLAG_INHERIT, SetHandleInformation,
    };
    use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
    let mut handle = std::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            parent.as_raw_handle(),
            source as *mut _,
            GetCurrentProcess(),
            &mut handle,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { GetFileType(handle.as_raw_handle()) } != FILE_TYPE_PIPE {
        return Err(invalid_bootstrap());
    }
    let fd = unsafe {
        libc::open_osfhandle(
            handle.as_raw_handle() as isize,
            access | libc::O_BINARY | libc::O_NOINHERIT,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let _ = handle.into_raw_handle(); // ownership moved to the CRT
    let mut descriptor = ChildPipeDescriptor(fd);
    // A closed program stdio slot is still not an IPC slot. Temporarily hold
    // low CRT slots while dup finds a descriptor outside 0..=2, then close them.
    let mut low = Vec::new();
    while descriptor.0 < 3 {
        let next = unsafe { libc::dup(descriptor.0) };
        if next < 0 {
            return Err(io::Error::last_os_error());
        }
        low.push(descriptor);
        descriptor = ChildPipeDescriptor(next);
    }
    // CRT dup need not preserve O_NOINHERIT. The transferred endpoint must
    // remain private even when it had to move out of a closed stdio slot.
    let raw = unsafe { libc::get_osfhandle(descriptor.0) };
    if raw == -1 || unsafe { SetHandleInformation(raw as *mut _, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(descriptor)
}

#[cfg(unix)]
fn pipe_identity(pipe: &impl std::os::fd::AsRawFd) -> usize {
    pipe.as_raw_fd() as usize
}
#[cfg(windows)]
fn pipe_identity(pipe: &impl std::os::windows::io::AsRawHandle) -> usize {
    pipe.as_raw_handle() as usize
}

#[cfg(unix)]
fn normalize_reader(pipe: PipeReader) -> io::Result<PipeReader> {
    use std::os::fd::{AsRawFd, FromRawFd};
    if pipe.as_raw_fd() >= 3 {
        return Ok(pipe);
    }
    let fd = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { PipeReader::from_raw_fd(fd) })
}
#[cfg(unix)]
fn normalize_writer(pipe: PipeWriter) -> io::Result<PipeWriter> {
    use std::os::fd::{AsRawFd, FromRawFd};
    if pipe.as_raw_fd() >= 3 {
        return Ok(pipe);
    }
    let fd = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { PipeWriter::from_raw_fd(fd) })
}
#[cfg(not(unix))]
fn normalize_reader(pipe: PipeReader) -> io::Result<PipeReader> {
    Ok(pipe)
}
#[cfg(not(unix))]
fn normalize_writer(pipe: PipeWriter) -> io::Result<PipeWriter> {
    Ok(pipe)
}
