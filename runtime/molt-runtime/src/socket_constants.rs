//! Platform socket constants shared by stdlib exports and runtime helpers.

pub(crate) const AF_INET: i32 = 2;

#[cfg(target_arch = "wasm32")]
pub(crate) const AF_INET6: i32 = crate::libc_compat::AF_INET6;

#[cfg(target_os = "macos")]
pub(crate) const AF_INET6: i32 = 30;

#[cfg(windows)]
pub(crate) const AF_INET6: i32 = 23;

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos"), not(windows)))]
pub(crate) const AF_INET6: i32 = libc::AF_INET6;
