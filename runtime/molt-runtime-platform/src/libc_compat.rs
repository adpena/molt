#![allow(non_camel_case_types)]

// Minimal libc constants/types for wasm builds where libc isn't available.

pub type c_char = i8;
pub type c_double = f64;
pub type c_float = f32;
pub type c_int = i32;
pub type c_long = i32;
pub type c_longlong = i64;
pub type c_short = i16;
pub type c_uchar = u8;
pub type c_uint = u32;
pub type c_ulong = u32;
pub type c_ulonglong = u64;
pub type c_ushort = u16;
pub type socklen_t = u32;
#[allow(clippy::upper_case_acronyms)]
pub type SOCKET = i32;

pub const SO_ACCEPTCONN: i32 = 30;

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

pub use crate::wasm_socket_abi::{
    AF_INET, AF_INET6, AF_UNIX, MSG_DONTWAIT, SOCK_DGRAM, SOCK_RAW, SOCK_STREAM, SOL_SOCKET,
};

// The actual Molt host/Python domain, generated from the ABI declaration.
pub use crate::synthetic_errno::*;

// Deterministic sysconf identifiers used by the wasm-side `os.sysconf` subset.
// Match the canonical values exposed through `os.sysconf_names` in the current
// host/runtime contract.
pub const _SC_PAGE_SIZE: c_int = 29;
pub const _SC_PAGESIZE: c_int = 29;
pub const _SC_IOV_MAX: c_int = 56;
pub const _SC_NPROCESSORS_CONF: c_int = 57;
pub const _SC_NPROCESSORS_ONLN: c_int = 58;

// Rust's WASI libc object emits relocatable references to these underscored
// clock-id symbols when CPU-time support is linked into the runtime staticlib.
// Define the canonical WASI values here so the relocatable runtime artifact
// closes over them before the final app link.
#[cfg(target_arch = "wasm32")]
#[used]
#[unsafe(no_mangle)]
pub static _CLOCK_PROCESS_CPUTIME_ID: u32 = 2;

#[cfg(target_arch = "wasm32")]
#[used]
#[unsafe(no_mangle)]
pub static _CLOCK_THREAD_CPUTIME_ID: u32 = 3;

/// WASI stand-in for C `_exit`: stop the instance at once by trapping.
///
/// # Safety
///
/// The signature mirrors `libc::_exit` so the runtime calls one spelling on
/// every target. Like the C function it never returns and never unwinds, so
/// no destructor or buffered I/O flush runs after it; the caller must have
/// finished every write it needs before calling it.
#[cfg(target_arch = "wasm32")]
pub unsafe fn _exit(_code: c_int) -> ! {
    core::arch::wasm32::unreachable()
}

// wasi-libc locale and environment ABI, supplied by the selected SDK libc.a.
pub const LC_CTYPE: c_int = 0;
pub const LC_NUMERIC: c_int = 1;
pub const LC_TIME: c_int = 2;
pub const LC_COLLATE: c_int = 3;
pub const LC_MONETARY: c_int = 4;
pub const LC_MESSAGES: c_int = 5;
pub const LC_ALL: c_int = 6;
pub const CODESET: c_int = 14;
unsafe extern "C" {
    pub fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    pub fn nl_langinfo(item: c_int) -> *mut c_char;
    pub fn getenv(name: *const c_char) -> *mut c_char;
    pub fn __wasilibc_get_environ() -> *mut *mut c_char;
    pub fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
    pub fn unsetenv(name: *const c_char) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_errno_exports_cover_runtime_errors_and_alias_precedence() {
        use std::collections::HashMap;
        use std::io::{Error, ErrorKind};

        let names: HashMap<_, _> = ERRNO_CONSTANTS.iter().copied().collect();
        assert_eq!(names.len(), ERRNO_CONSTANTS.len(), "duplicate errno name");
        // Independent synthetic POSIX values; raw WASI numbers differ.
        for (name, value) in [
            ("EIO", 5),
            ("ENOMEM", 12),
            ("ENOSYS", 38),
            ("EPROTOTYPE", 91),
            ("EAFNOSUPPORT", 97),
            ("EISCONN", 106),
            ("ESHUTDOWN", 108),
        ] {
            assert_eq!(names.get(name), Some(&value));
        }
        for kind in [
            ErrorKind::Other,
            ErrorKind::Unsupported,
            ErrorKind::NotFound,
        ] {
            let code = i64::from(crate::synthetic_errno::from_io_error(&Error::from(kind)).code());
            assert!(names.values().any(|value| *value == code));
        }
        assert!(names.keys().all(|name| !name.starts_with("EAI_")));
        assert_eq!(names["EWOULDBLOCK"], names["EAGAIN"]);
        let reverse: HashMap<_, _> = ERRNO_CONSTANTS
            .iter()
            .map(|&(name, code)| (code, name))
            .collect();
        assert_eq!(reverse[&11], "EAGAIN");
    }
}
