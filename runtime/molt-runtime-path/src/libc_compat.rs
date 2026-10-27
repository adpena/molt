#![allow(non_camel_case_types)]

// Minimal libc constants/types for wasm builds where libc isn't available.

pub type c_char = i8;
pub type c_int = i32;
pub type c_long = i32;

pub use molt_runtime_platform::synthetic_errno::{
    EACCES, EEXIST, EINVAL, EIO, EISDIR, ENOENT, ENOSYS, ENOTDIR, EPERM,
};
