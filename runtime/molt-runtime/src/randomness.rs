use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OsRandomError {
    Unsupported(&'static str),
    Backend(&'static str),
}

impl fmt::Display for OsRandomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(message) | Self::Backend(message) => f.write_str(message),
        }
    }
}

#[cfg(all(target_arch = "wasm32", feature = "wasm_freestanding"))]
pub(crate) fn fill_os_random(_buf: &mut [u8]) -> Result<(), OsRandomError> {
    Err(OsRandomError::Unsupported(
        "os randomness unavailable on wasm-freestanding",
    ))
}

#[cfg(not(all(target_arch = "wasm32", feature = "wasm_freestanding")))]
pub(crate) fn fill_os_random(buf: &mut [u8]) -> Result<(), OsRandomError> {
    getrandom::fill(buf).map_err(|err| {
        let rendered = err.to_string();
        let leaked: &'static str = Box::leak(rendered.into_boxed_str());
        OsRandomError::Backend(leaked)
    })
}

pub(crate) fn os_random_supported() -> bool {
    !cfg!(all(target_arch = "wasm32", feature = "wasm_freestanding"))
}
