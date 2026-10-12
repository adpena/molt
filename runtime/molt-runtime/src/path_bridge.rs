//! FFI bridge shims for `molt-runtime-path`.
//!
//! Each function here is a thin `#[no_mangle] extern "C"` wrapper around an
//! internal `pub(crate)` function.  The path crate declares matching
//! `extern "C"` imports and they are resolved at link time.

use crate::audit::AuditArgs;
use crate::object::ops::string_obj_to_owned as _string_obj_to_owned;
use crate::*;

// ---------------------------------------------------------------------------
// Exception / error handling
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_exception(
    type_ptr: *const u8,
    type_len: usize,
    msg_ptr: *const u8,
    msg_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(type_ptr, type_len))
        };
        let msg =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
        raise_exception::<u64>(_py, type_name, msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_exception_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_os_error(
    err_kind: u32,
    err_msg_ptr: *const u8,
    err_msg_len: usize,
    ctx_ptr: *const u8,
    ctx_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let err_msg = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(err_msg_ptr, err_msg_len))
        };
        let ctx =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ctx_ptr, ctx_len)) };
        let kind = match err_kind {
            0 => std::io::ErrorKind::NotFound,
            1 => std::io::ErrorKind::PermissionDenied,
            2 => std::io::ErrorKind::ConnectionRefused,
            3 => std::io::ErrorKind::ConnectionReset,
            4 => std::io::ErrorKind::ConnectionAborted,
            5 => std::io::ErrorKind::NotConnected,
            6 => std::io::ErrorKind::AddrInUse,
            7 => std::io::ErrorKind::AddrNotAvailable,
            8 => std::io::ErrorKind::BrokenPipe,
            9 => std::io::ErrorKind::AlreadyExists,
            10 => std::io::ErrorKind::WouldBlock,
            11 => std::io::ErrorKind::InvalidInput,
            12 => std::io::ErrorKind::InvalidData,
            13 => std::io::ErrorKind::TimedOut,
            14 => std::io::ErrorKind::WriteZero,
            15 => std::io::ErrorKind::Interrupted,
            16 => std::io::ErrorKind::Unsupported,
            17 => std::io::ErrorKind::UnexpectedEof,
            18 => std::io::ErrorKind::OutOfMemory,
            _ => std::io::ErrorKind::Other,
        };
        let err = std::io::Error::new(kind, err_msg.to_string());
        raise_os_error::<u64>(_py, err, ctx)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_raise_os_error_errno(
    errno: i64,
    ctx_ptr: *const u8,
    ctx_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ctx_ptr, ctx_len)) };
        raise_os_error_errno::<u64>(_py, errno, ctx)
    })
}

// ---------------------------------------------------------------------------
// Object allocation
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_tuple(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_tuple(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_list(elems_ptr: *const u64, elems_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let elems = unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) };
        alloc_list(_py, elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_string(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_string(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_bytes(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
        alloc_bytes(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_alloc_dict_with_pairs(
    pairs_ptr: *const u64,
    pairs_len: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, pairs_len) };
        alloc_dict_with_pairs(_py, pairs)
    })
}

// ---------------------------------------------------------------------------
// Object inspection
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_object_type_id(ptr: *mut u8) -> u32 {
    unsafe { object_type_id(ptr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_string_obj_to_owned(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let obj = obj_from_bits(bits);
    match _string_obj_to_owned(obj) {
        Some(s) => {
            let bytes = s.into_bytes().into_boxed_slice();
            unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_is_truthy(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        if is_truthy(_py, obj) { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_bytes_like_slice(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    match unsafe { crate::object::memoryview::bytes_like_slice(ptr) } {
        Some(slice) => {
            unsafe {
                *out_ptr = slice.as_ptr();
                *out_len = slice.len();
            }
            1
        }
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Reference counting
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_dec_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_inc_ref_bits(bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, bits);
    })
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_to_i64(bits: u64, out: *mut i64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_i64(obj) {
        Some(v) => {
            unsafe {
                *out = v;
            }
            1
        }
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_to_f64(bits: u64, out: *mut f64) -> i32 {
    let obj = obj_from_bits(bits);
    match to_f64(obj) {
        Some(v) => {
            unsafe {
                *out = v;
            }
            1
        }
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Container helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_seq_snapshot(
    ptr: *mut u8,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

// ---------------------------------------------------------------------------
// Iteration helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_iter(bits: u64) -> u64 {
    crate::object::ops_iter::molt_iter(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_iter_next(iter_bits: u64, out: *mut u64) -> i32 {
    let result = crate::object::ops_iter::molt_iter_next(iter_bits);
    let none_bits = MoltObject::none().bits();
    if result == none_bits {
        crate::with_gil_entry_nopanic!(_py, {
            if exception_pending(_py) {
                0 // StopIteration or error
            } else {
                unsafe {
                    *out = result;
                }
                1
            }
        })
    } else {
        unsafe {
            *out = result;
        }
        1
    }
}

// ---------------------------------------------------------------------------
// Capability helpers
// ---------------------------------------------------------------------------

/// Require a borrowed operation name and audit arguments from the path crate.
/// Returns 1 on success; on refusal writes the exact exception sentinel and
/// returns 0. All pointers are borrowed for this call, as in the other bridges.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_require_operation(
    operation_ptr: *const u8,
    operation_len: usize,
    arg_kind: u32,
    arg_ptr: *const u8,
    arg_len: usize,
    out_error_bits: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let operation = unsafe {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(operation_ptr, operation_len))
        };
        let args = match arg_kind {
            0 => AuditArgs::None,
            1 => {
                let path = unsafe {
                    std::str::from_utf8_unchecked(std::slice::from_raw_parts(arg_ptr, arg_len))
                };
                AuditArgs::Path(path.to_string())
            }
            _ => {
                let error = raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "invalid path operation audit argument kind",
                );
                unsafe { *out_error_bits = error };
                return 0;
            }
        };
        match crate::require_named_operation::<u64>(_py, operation, args) {
            Ok(()) => 1,
            Err(error) => {
                unsafe { *out_error_bits = error };
                0
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Hash helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_molt_object_hash(bits: u64) -> u64 {
    molt_object_hash(bits)
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_path_from_bits(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        match path_from_bits(_py, bits) {
            Ok(path) => {
                // Same-build, in-process transport preserves the OS representation.
                // These bytes are never an external serialization format.
                let bytes = path
                    .into_os_string()
                    .into_encoded_bytes()
                    .into_boxed_slice();
                unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
            }
            Err(_) => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_type_name(
    bits: u64,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let name = crate::object::ops::type_name(_py, obj);
        let bytes = name.into_owned().into_bytes().into_boxed_slice();
        unsafe { crate::resource::bridge_buffer::export_u8_box(bytes, out_ptr, out_len) }
    })
}

/// Return the canonical owned Python path, preserving str/bytes identity.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_path_fspath_bits(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match crate::builtins::io_path_utils::fspath_bits_with_flavor(_py, bits) {
            Ok((path, _)) => path,
            Err(error) => error,
        }
    })
}

#[cfg(all(test, feature = "stdlib_path"))]
mod path_transport_tests {
    use super::*;

    #[test]
    fn canonical_pid_requires_exact_permission() {
        for grants in ["", "os.getpid"] {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(grants, |py| {
                let result = molt_os_getpid();
                if grants.is_empty() {
                    assert_eq!(result, MoltObject::none().bits());
                    let error = crate::molt_exception_last_pending();
                    let _owner = unsafe {
                        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            error,
                        )
                    };
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "PermissionError"
                    ));
                    crate::molt_exception_clear();
                } else {
                    let pid = to_i64(obj_from_bits(result)).expect("PID integer");
                    #[cfg(not(target_arch = "wasm32"))]
                    assert_eq!(pid, std::process::id() as i64);
                    #[cfg(target_arch = "wasm32")]
                    assert!(pid >= 0);
                    assert!(!exception_pending(py));
                }
            });
        }
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn cached_platform_host_facts_require_each_runtime_permission() {
        use crate::builtins::platform_mod::*;
        let readers: [fn() -> u64; 7] = [
            || molt_platform_system(),
            || molt_platform_machine(),
            || molt_platform_node(),
            || molt_platform_release(),
            || molt_platform_version(),
            || {
                molt_platform_platform(
                    MoltObject::from_bool(false).bits(),
                    MoltObject::from_bool(false).bits(),
                )
            },
            || molt_platform_uname(),
        ];
        // Prime the process-wide cache, then start a new runtime without grants.
        // Checking only cache initialization would leak every subsequent read.
        for grants in ["os.uname", ""] {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(grants, |py| {
                for read in readers {
                    let bits = read();
                    if grants.is_empty() {
                        assert_eq!(bits, MoltObject::none().bits());
                        let error = crate::molt_exception_last_pending();
                        let _owner = unsafe {
                            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                                py.core_token(),
                                error,
                            )
                        };
                        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                            py,
                            error,
                            "PermissionError"
                        ));
                        crate::molt_exception_clear();
                        assert_eq!(
                            crate::builtins::exceptions::format_exception_message(
                                py,
                                obj_from_bits(error).as_ptr().unwrap()
                            ),
                            "missing [os.uname] capabilities for 'os.uname'"
                        );
                    } else {
                        assert!(!obj_from_bits(bits).is_none());
                        assert!(!exception_pending(py));
                        dec_ref_bits(py, bits);
                    }
                }
                // Compile-time metadata does not disclose host uname state.
                let bits = molt_platform_processor();
                assert!(!obj_from_bits(bits).is_none());
                assert!(!exception_pending(py));
                dec_ref_bits(py, bits);
            });
        }
    }

    #[cfg(feature = "stdlib_logging_ext")]
    #[test]
    fn logging_record_formats_supplied_process_without_querying_host() {
        use molt_runtime_logging::logging_ext::*;
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("", |py| {
            let string = |text: &str| {
                let ptr = alloc_string(py, text.as_bytes());
                assert!(!ptr.is_null());
                unsafe {
                    molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        MoltObject::from_ptr(ptr).bits(),
                    )
                }
            };
            let name = string("pid-metadata");
            let format = string("%(process)s");
            let style = string("%");
            let none = MoltObject::none().bits();
            let formatter = molt_logging_formatter_new(
                format.bits(),
                none,
                style.bits(),
                MoltObject::from_bool(true).bits(),
            );
            assert!(!exception_pending(py));
            for (process, expected) in [(MoltObject::from_int(73).bits(), "73"), (none, "None")] {
                let record = molt_logging_record_new(
                    name.bits(),
                    MoltObject::from_int(20).bits(),
                    none,
                    MoltObject::from_int(1).bits(),
                    name.bits(),
                    none,
                    none,
                    process,
                );
                assert!(!exception_pending(py));
                let rendered = molt_logging_formatter_format(formatter, record);
                assert!(!exception_pending(py));
                let _owner = unsafe {
                    molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), rendered)
                };
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                    Some(expected)
                );
                molt_logging_record_drop(record);
            }
            molt_logging_formatter_drop(formatter);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn path_operation_bridges_preserve_exact_grants_and_complete_refusal() {
        for (grants, missing) in [
            ("os.cpu_count", Some("fs.read, fs.write")),
            ("os.cpu_count,fs.read", Some("fs.write")),
            ("os.cpu_count,fs.read,fs.write", None),
        ] {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(grants, |py| {
                assert!(!crate::has_capability(py, "process"));
                let cpu_count = molt_runtime_path::os_ext::molt_os_cpu_count();
                assert!(to_i64(obj_from_bits(cpu_count)).is_some_and(|count| count > 0));
                assert!(!exception_pending(py));

                // Both extracted and reduced-profile bridges must transport
                // the same canonical decision, including every missing grant.
                for extracted in [true, false] {
                    let result = molt_runtime_core::with_core_gil!(core, {
                        if extracted {
                            molt_runtime_path::bridge::require_operation(
                                core,
                                "shutil.copy",
                                molt_runtime_path::bridge::AuditArg::Path("source".into()),
                            )
                        } else {
                            crate::bridge::require_operation(
                                core,
                                "shutil.copy",
                                crate::bridge::AuditArg::Path("source".into()),
                            )
                        }
                    });
                    if let Some(missing) = missing {
                        assert_eq!(result, Err(MoltObject::none().bits()));
                        assert!(exception_pending(py));
                        let exception = crate::molt_exception_last_pending();
                        let _exception_owner = unsafe {
                            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                                py.core_token(),
                                exception,
                            )
                        };
                        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                            py,
                            exception,
                            "PermissionError"
                        ));
                        crate::molt_exception_clear();
                        assert_eq!(
                            crate::builtins::exceptions::format_exception_message(
                                py,
                                obj_from_bits(exception).as_ptr().unwrap(),
                            ),
                            format!("missing [{missing}] capabilities for 'shutil.copy'"),
                        );
                    } else {
                        assert_eq!(result, Ok(()));
                        assert!(!exception_pending(py));
                    }
                }

                // Successful FFI transport must leave failure-only output
                // untouched; an unknown operation must never be admitted.
                let operation = "os.cpu_count";
                let marker = MoltObject::from_int(73).bits();
                let mut error_bits = marker;
                assert_eq!(
                    __molt_path_require_operation(
                        operation.as_ptr(),
                        operation.len(),
                        0,
                        std::ptr::null(),
                        0,
                        &mut error_bits,
                    ),
                    1
                );
                assert_eq!(error_bits, marker);
                let operation = "path.operation.unknown";
                assert_eq!(
                    __molt_path_require_operation(
                        operation.as_ptr(),
                        operation.len(),
                        0,
                        std::ptr::null(),
                        0,
                        &mut error_bits,
                    ),
                    0
                );
                assert_eq!(error_bits, MoltObject::none().bits());
                let exception = crate::molt_exception_last_pending();
                let _exception_owner = unsafe {
                    molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        exception,
                    )
                };
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "ValueError"
                ));
                crate::molt_exception_clear();
            });
        }
    }

    #[test]
    fn fspath_transports_preserve_owned_identity_and_type_errors() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let transports: [(&str, fn(u64) -> u64); 3] = [
                ("direct", crate::bridge::fspath_bits),
                ("satellite", molt_runtime_path::bridge::fspath_bits),
                ("public", |bits| {
                    molt_runtime_path::os_ext::molt_os_fspath(bits)
                }),
            ];
            for (name, transport) in transports {
                for pointer in [
                    alloc_string(py, b"molt-fspath-owned-text"),
                    alloc_bytes(py, b"molt-fspath-owned-\xff"),
                ] {
                    assert!(!pointer.is_null());
                    let _input_owner = crate::PtrDropGuard::new(pointer);
                    let bits = MoltObject::from_ptr(pointer).bits();
                    let references =
                        || unsafe { (*crate::header_from_obj_ptr(pointer)).ref_count_snapshot() };
                    let before = references();
                    let result = transport(bits);
                    assert!(!exception_pending(py), "{name}");
                    assert_eq!(result, bits, "{name}: preserve str/bytes identity");
                    assert_eq!(references(), before + 1, "{name}: return an owned result");
                    dec_ref_bits(py, result);
                    assert_eq!(references(), before, "{name}: release exactly one result");
                }
                let result = transport(MoltObject::from_int(42).bits());
                assert_eq!(result, MoltObject::none().bits(), "{name}");
                assert!(exception_pending(py), "{name}");
                let exception = crate::molt_exception_last_pending();
                let _exception_owner = unsafe {
                    molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        exception,
                    )
                };
                assert!(
                    crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        exception,
                        "TypeError"
                    ),
                    "{name}"
                );
                crate::molt_exception_clear();
                assert_eq!(
                    crate::builtins::exceptions::format_exception_message(
                        py,
                        obj_from_bits(exception).as_ptr().unwrap()
                    ),
                    "expected str, bytes or os.PathLike object, not int",
                    "{name}",
                );
            }
        });
    }

    #[test]
    fn path_bridge_preserves_bytes_and_does_not_publish_failure_output() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let input = b"molt-path-transport";
            let ptr = alloc_bytes(py, input);
            assert!(!ptr.is_null());
            let _owner = crate::PtrDropGuard::new(ptr);
            let bits = MoltObject::from_ptr(ptr).bits();
            molt_runtime_core::with_core_gil!(core, {
                let path = molt_runtime_path::bridge::path_from_bits(core, bits).unwrap();
                assert_eq!(path, std::path::PathBuf::from("molt-path-transport"));
            });
            let result = molt_runtime_path::os_ext::molt_os_fspath(bits);
            let _result_owner = unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), result)
            };
            assert_eq!(
                result, bits,
                "public fspath must retain the original bytes result"
            );
            assert!(!exception_pending(py));

            let marker = b"unchanged output";
            let mut out = marker.as_ptr();
            let mut length = marker.len();
            assert_eq!(
                __molt_path_path_from_bits(MoltObject::from_int(42).bits(), &mut out, &mut length),
                0
            );
            assert_eq!(out, marker.as_ptr());
            assert_eq!(length, marker.len());
            assert!(exception_pending(py));
            let exception = crate::molt_exception_last_pending();
            let _exception_owner = unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), exception)
            };
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "TypeError"
            ));
            crate::molt_exception_clear();
        });
    }

    #[cfg(unix)]
    #[test]
    fn path_bridge_roundtrips_non_utf8_os_bytes_through_the_real_consumer() {
        use std::os::unix::ffi::OsStrExt;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let input = b"molt-path-\xff";
            let ptr = alloc_bytes(py, input);
            assert!(!ptr.is_null());
            let _owner = crate::PtrDropGuard::new(ptr);
            let bits = MoltObject::from_ptr(ptr).bits();
            molt_runtime_core::with_core_gil!(core, {
                let path = molt_runtime_path::bridge::path_from_bits(core, bits).unwrap();
                assert_eq!(path.as_os_str().as_bytes(), input);
            });
            assert!(!exception_pending(py));
        });
    }
}
