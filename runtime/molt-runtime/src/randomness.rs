use std::borrow::Cow;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OsRandomError(Cow<'static, str>);

impl fmt::Display for OsRandomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_ref())
    }
}

#[cfg(all(target_arch = "wasm32", feature = "wasm_freestanding"))]
pub(crate) fn fill_os_random(_buf: &mut [u8]) -> Result<(), OsRandomError> {
    Err(OsRandomError(Cow::Borrowed(
        "os randomness unavailable on wasm-freestanding",
    )))
}

#[cfg(not(all(target_arch = "wasm32", feature = "wasm_freestanding")))]
pub(crate) fn fill_os_random(buf: &mut [u8]) -> Result<(), OsRandomError> {
    getrandom::fill(buf).map_err(|err| OsRandomError(Cow::Owned(err.to_string())))
}

pub(crate) fn os_random_supported() -> bool {
    !cfg!(all(target_arch = "wasm32", feature = "wasm_freestanding"))
}
