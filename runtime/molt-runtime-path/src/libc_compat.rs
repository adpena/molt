#![allow(dead_code)]
#![allow(non_camel_case_types)]

// Minimal libc constants/types for wasm builds where libc isn't available.

pub type c_char = i8;
pub type c_int = i32;
pub type c_long = i32;

pub const ENOSYS: i32 = 38;
pub const ENOENT: i32 = 2;
pub const EACCES: i32 = 13;
pub const EINVAL: i32 = 22;
pub const EISDIR: i32 = 21;
pub const ENOTDIR: i32 = 20;
pub const EEXIST: i32 = 17;
pub const EPERM: i32 = 1;
pub const EIO: i32 = 5;
