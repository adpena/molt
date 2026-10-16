#[cfg(any(not(feature = "stdlib_path"), all(test, unix)))]
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
    path_from_bits_with_flavor(_py, bits).map(|(path, _)| path)
}

#[cfg(any(not(feature = "stdlib_path"), all(test, unix)))]
pub fn path_from_bits_with_flavor(_py: &CoreGilToken, bits: u64) -> Result<(PathBuf, bool), u64> {
    crate::with_gil_entry_nopanic!(py, {
        crate::builtins::io::path_from_bits_with_flavor(py, bits)
            .map(|(path, flavor)| (path, flavor == crate::builtins::io::PathFlavor::Bytes))
    })
}

#[cfg(any(not(feature = "stdlib_path"), all(test, unix)))]
pub fn path_result(_py: &CoreGilToken, value: &std::path::Path, bytes: bool) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::io::PathFlavor;
        crate::builtins::io::path_result_bits(
            py,
            value,
            if bytes {
                PathFlavor::Bytes
            } else {
                PathFlavor::Str
            },
        )
    })
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn direct_path_transport_preserves_type_and_non_utf8_bytes() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (raw, bytes) in [(b"name".as_slice(), false), (b"name-\xff".as_slice(), true)] {
                let pointer = if bytes {
                    crate::alloc_bytes(py, raw)
                } else {
                    crate::alloc_string(py, raw)
                };
                assert!(!pointer.is_null());
                let _input = crate::PtrDropGuard::new(pointer);
                let bits = MoltObject::from_ptr(pointer).bits();
                molt_runtime_core::with_core_gil!(core, {
                    let (path, actual_bytes) = path_from_bits_with_flavor(core, bits).unwrap();
                    assert_eq!(path.as_os_str().as_bytes(), raw);
                    assert_eq!(actual_bytes, bytes);
                    let result = path_result(core, &path, actual_bytes);
                    let _result = unsafe {
                        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(core, result)
                    };
                    let result = crate::obj_from_bits(result).as_ptr().unwrap();
                    unsafe {
                        if bytes {
                            assert_eq!(crate::object_type_id(result), crate::TYPE_ID_BYTES);
                            assert_eq!(
                                std::slice::from_raw_parts(
                                    crate::bytes_data(result),
                                    crate::bytes_len(result)
                                ),
                                raw
                            );
                        } else {
                            assert_eq!(crate::object_type_id(result), crate::TYPE_ID_STRING);
                            assert_eq!(
                                std::slice::from_raw_parts(
                                    crate::string_bytes(result),
                                    crate::string_len(result)
                                ),
                                raw
                            );
                        }
                    }
                });
                assert!(!crate::exception_pending(py));
            }
        });
    }
}
