#![allow(dead_code)]
#![allow(non_camel_case_types)]

// Minimal libc constants/types for wasm builds where libc isn't available.

pub type c_char = i8;
pub type c_int = i32;
pub type c_long = i32;
pub type socklen_t = u32;
#[allow(clippy::upper_case_acronyms)]
pub type SOCKET = i32;

pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 10;

pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_RAW: i32 = 3;

pub const SOL_SOCKET: i32 = 1;
pub const SO_ACCEPTCONN: i32 = 30;

pub const MSG_DONTWAIT: i32 = 0x40;

pub const NI_MAXHOST: i32 = 1025;
pub const NI_MAXSERV: i32 = 32;

pub const O_RDONLY: i32 = 0;
pub const O_WRONLY: i32 = 1;
pub const O_RDWR: i32 = 2;
pub const O_APPEND: i32 = 0x400;
pub const O_CREAT: i32 = 0x40;
pub const O_TRUNC: i32 = 0x200;
pub const O_EXCL: i32 = 0x80;
pub const O_NONBLOCK: i32 = 0x800;

// fcntl commands and flags (Linux values for WASM compat).
pub const F_GETFD: i32 = 1;
pub const F_SETFD: i32 = 2;
pub const F_GETFL: i32 = 3;
pub const F_SETFL: i32 = 4;
pub const FD_CLOEXEC: i32 = 1;

// Synthetic signal numbers used by wasm/native-lite fallback paths.
pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;
pub const SIGQUIT: i32 = 3;
pub const SIGILL: i32 = 4;
pub const SIGTRAP: i32 = 5;
pub const SIGABRT: i32 = 6;
pub const SIGBUS: i32 = 7;
pub const SIGFPE: i32 = 8;
pub const SIGKILL: i32 = 9;
pub const SIGUSR1: i32 = 10;
pub const SIGSEGV: i32 = 11;
pub const SIGUSR2: i32 = 12;
pub const SIGPIPE: i32 = 13;
pub const SIGALRM: i32 = 14;
pub const SIGTERM: i32 = 15;
pub const SIGCHLD: i32 = 17;
pub const SIGCONT: i32 = 18;
pub const SIGTSTP: i32 = 20;
pub const SIGSTOP: i32 = 19;
pub const SIGTTIN: i32 = 21;
pub const SIGTTOU: i32 = 22;
pub const SIGXCPU: i32 = 24;
pub const SIGXFSZ: i32 = 25;
pub const SIGVTALRM: i32 = 26;
pub const SIGPROF: i32 = 27;
pub const SIGWINCH: i32 = 28;
pub const SIGSYS: i32 = 31;

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

pub const EACCES: i32 = 13;
pub const EAGAIN: i32 = 11;
pub const EALREADY: i32 = 114;
pub const EAFNOSUPPORT: i32 = 97;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const ECONNABORTED: i32 = 103;
pub const ECONNREFUSED: i32 = 111;
pub const ECONNRESET: i32 = 104;
pub const EEXIST: i32 = 17;
pub const EHOSTUNREACH: i32 = 113;
pub const EINPROGRESS: i32 = 115;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const EISCONN: i32 = 106;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENOENT: i32 = 2;
pub const ENOMEM: i32 = 12;
pub const ENOSYS: i32 = 38;
pub const ENOTDIR: i32 = 20;
pub const EPERM: i32 = 1;
pub const EPIPE: i32 = 32;
pub const EPROTOTYPE: i32 = 91;
pub const ESRCH: i32 = 3;
pub const ETIMEDOUT: i32 = 110;
pub const EWOULDBLOCK: i32 = 11;

pub const EAI_NONAME: i32 = 8;

pub const ESHUTDOWN: i32 = 108;

// Deterministic sysconf identifiers used by the wasm-side `os.sysconf` subset.
// Match the canonical values exposed through `os.sysconf_names` in the current
// host/runtime contract.
pub const _SC_PAGE_SIZE: c_int = 29;
pub const _SC_PAGESIZE: c_int = 29;
pub const _SC_IOV_MAX: c_int = 56;
pub const _SC_NPROCESSORS_CONF: c_int = 57;
pub const _SC_NPROCESSORS_ONLN: c_int = 58;
