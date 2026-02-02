#![allow(dead_code)]
#![allow(non_camel_case_types)]

// Minimal libc constants/types for wasm builds where libc isn't available.

pub type c_char = i8;
pub type c_int = i32;
pub type c_long = i32;
pub type socklen_t = u32;
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
