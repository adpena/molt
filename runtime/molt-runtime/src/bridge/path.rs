#[cfg(not(feature = "stdlib_path"))]
use std::path::PathBuf;

use crate::audit::AuditArgs;
use molt_runtime_core::prelude::*;

pub enum AuditArg {
    #[cfg(not(feature = "stdlib_path"))]
    None,
    Path(String),
}

/// Direct counterpart of the extracted path crate's operation bridge.
pub fn require_operation(_py: &CoreGilToken, operation: &str, arg: AuditArg) -> Result<(), u64> {
    let args = match arg {
        #[cfg(not(feature = "stdlib_path"))]
        AuditArg::None => AuditArgs::None,
        AuditArg::Path(path) => AuditArgs::Path(path),
    };
    crate::with_gil_entry_nopanic!(py, {
        crate::require_named_operation::<u64>(py, operation, args)
    })
}

#[cfg(not(feature = "stdlib_path"))]
pub fn path_from_bits(_py: &CoreGilToken, bits: u64) -> Result<PathBuf, u64> {
    crate::with_gil_entry_nopanic!(py, { crate::path_from_bits(py, bits) })
}

/// Return the canonical owned Python path without converting its str/bytes type.
pub fn fspath_bits(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        match crate::builtins::io_path_utils::fspath_bits_with_flavor(py, bits) {
            Ok((path, _)) => path,
            Err(error) => error,
        }
    })
}
