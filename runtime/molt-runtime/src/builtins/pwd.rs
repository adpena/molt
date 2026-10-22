//! POSIX user-database projection. Native storage and cursor custody belong to
//! molt-runtime-platform::userdb; this boundary owns admission and Python values.

use crate::audit::AuditArgs;
use crate::*;

// Provider absence is independent of capability denial and of import resolution.
// Keep the exception's Python name owned through canonical construction.
fn raise_provider_absence(py: &PyToken<'_>) -> u64 {
    let Some(name) = attr_name_bits_from_bytes(py, b"pwd") else {
        return MoltObject::none().bits();
    };
    let name = crate::builtins::exceptions::ExceptionValue::adopt(py, name);
    crate::builtins::exceptions::raise_module_not_found(py, b"No module named 'pwd'", name.bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pwd_module_init() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !crate::async_rt::channels::operation_supported_here(py, OperationId::PwdGetpwuid) {
            return raise_provider_absence(py);
        }
        MoltObject::none().bits()
    })
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod native {
    use super::*;
    use crate::builtins::io_path_utils::{filesystem_decode, os_string_from_text};
    use crate::builtins::numbers::{index_from_object, int_bits_from_i64, integer_storage_bigint};
    use crate::object::ops::protocol_type_name;
    use crate::object::ops_format::{FormatError, FormatOutput, FormatWriter};
    use molt_runtime_core::OwnedRuntimeValue;
    use molt_runtime_platform::userdb::{self, Passwd};
    use num_traits::ToPrimitive;

    fn field(py: &PyToken<'_>, raw: Option<&[u8]>) -> u64 {
        let Some(raw) = raw else {
            return MoltObject::none().bits();
        };
        let text = match filesystem_decode(py, raw) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let pointer = alloc_string(py, &text);
        if pointer.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(pointer).bits()
        }
    }

    pub(super) fn entry(py: &PyToken<'_>, class: u64, value: Passwd) -> u64 {
        let mut fields = Vec::new();
        if fields.try_reserve_exact(7).is_err() {
            return raise_exception::<u64>(py, "MemoryError", "");
        }
        for index in 0..7 {
            let bits = match index {
                0 => field(py, value.name.as_deref()),
                1 => field(py, value.passwd.as_deref()),
                2 => int_bits_from_i64(py, molt_runtime_platform::utils::uid_integer(value.uid)),
                3 => int_bits_from_i64(py, molt_runtime_platform::utils::gid_integer(value.gid)),
                4 => field(py, value.gecos.as_deref()),
                5 => field(py, value.dir.as_deref()),
                6 => field(py, value.shell.as_deref()),
                _ => unreachable!(),
            };
            let owner = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) };
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            fields.push(owner);
        }
        // The shared factory validates the actual class declaration and owns
        // visible/hidden tuple allocation. No callback or mutable type-name test.
        crate::builtins::structseq::from_values(
            py,
            class,
            OwnedRuntimeValue::as_bits_slice(&fields),
        )
    }

    pub(super) fn uid(py: &PyToken<'_>, bits: u64) -> Option<libc::uid_t> {
        let index = index_from_object(py, bits);
        let owner = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), index) };
        if exception_pending(py) {
            // _Py_Uid_Converter deliberately replaces *every* failed index
            // conversion, including exceptions raised inside __index__.
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
            raise_exception::<()>(
                py,
                "TypeError",
                &format!(
                    "uid should be integer, not {}",
                    protocol_type_name(py, bits, 200)
                ),
            );
            return None;
        }
        let value = match integer_storage_bigint(py, owner.bits()) {
            Ok(Some(value)) => value,
            _ => return None,
        };
        let signed = value.to_i64();
        if signed == Some(-1) {
            return Some(libc::uid_t::MAX);
        }
        // Linux/macOS uid_t is unsigned32. The positive sentinel is accepted
        // when it fits signed C long (LP64); on ILP32 its unsigned-long route
        // rejects the sentinel, exactly as _Py_Uid_Converter does.
        let value = value
            .to_u64()
            .and_then(|value| libc::uid_t::try_from(value).ok());
        if let Some(value) = value
            && (value != libc::uid_t::MAX
                || std::mem::size_of::<libc::c_long>() > std::mem::size_of::<libc::uid_t>())
        {
            return Some(value);
        }
        raise_exception::<()>(py, "KeyError", "getpwuid(): uid not found");
        None
    }

    pub(super) fn name(py: &PyToken<'_>, bits: u64) -> Result<std::ffi::CString, u64> {
        let converted = unsafe {
            crate::object::ops_format::with_string_bytes(obj_from_bits(bits), |bytes| {
                os_string_from_text(py, bytes, Some(bits))
            })
        };
        let Some(converted) = converted else {
            let message = (|| {
                let mut output = FormatWriter::new(py);
                output.append_bytes(b"getpwnam() argument must be str, not ")?;
                // Clinic uses %T in 3.13/3.14; 3.12 and 3.15 use the distinct
                // _PyArg_BadArgument contract (native name precision and None).
                let actual = if crate::object::ops_sys::runtime_target_at_least(py, 3, 13)
                    && !crate::object::ops_sys::runtime_target_at_least(py, 3, 15)
                {
                    crate::object::ops_format::format_diagnostic_type_name_bytes(
                        py,
                        obj_from_bits(bits),
                    )
                } else if obj_from_bits(bits).is_none() {
                    b"None".to_vec()
                } else {
                    protocol_type_name(py, bits, 50).into_bytes()
                };
                if exception_pending(py) {
                    return Err(FormatError::Pending);
                }
                output.append_bytes(&actual)?;
                output.finish().into_bytes_result(py)
            })();
            return Err(match message {
                Ok(message) => {
                    crate::builtins::exceptions::raise_exception_bytes(py, "TypeError", &message)
                }
                Err(error) => error.raise(py),
            });
        };
        let mut bytes = converted?.into_encoded_bytes();
        if bytes.contains(&0) {
            return Err(raise_exception::<u64>(
                py,
                "ValueError",
                "embedded null byte",
            ));
        }
        if bytes.try_reserve(1).is_err() {
            return Err(raise_exception::<u64>(py, "MemoryError", ""));
        }
        Ok(std::ffi::CString::new(bytes).expect("NUL checked before native query"))
    }

    pub(super) fn lookup_uid(py: &PyToken<'_>, uid: libc::uid_t, class: u64) -> u64 {
        let result = {
            let _release = molt_runtime_core::GilReleaseGuard::suspend();
            userdb::by_uid(uid)
        };
        match result {
            Ok(Some(value)) => entry(py, class, value),
            Ok(None) => raise_exception::<u64>(
                py,
                "KeyError",
                &format!(
                    "getpwuid(): uid not found: {}",
                    molt_runtime_platform::utils::uid_integer(uid)
                ),
            ),
            Err(_) => raise_exception::<u64>(py, "MemoryError", ""),
        }
    }

    pub(super) fn lookup_name(
        py: &PyToken<'_>,
        original: u64,
        name: &std::ffi::CStr,
        class: u64,
    ) -> u64 {
        let result = {
            let _release = molt_runtime_core::GilReleaseGuard::suspend();
            userdb::by_name(name)
        };
        match result {
            Ok(Some(value)) => entry(py, class, value),
            Ok(None) => {
                let message = (|| {
                    let mut output = FormatWriter::new(py);
                    output.append_bytes(b"getpwnam(): name not found: ")?;
                    output.append_output(
                        FormatOutput::OwnedString(crate::molt_repr_from_obj(original)),
                        false,
                    )?;
                    output.finish().into_bytes_result(py)
                })();
                match message {
                    Ok(message) => {
                        crate::builtins::exceptions::raise_exception_bytes(py, "KeyError", &message)
                    }
                    Err(error) => error.raise(py),
                }
            }
            Err(_) => raise_exception::<u64>(py, "MemoryError", ""),
        }
    }

    pub(super) fn all(py: &PyToken<'_>, class: u64) -> u64 {
        let entries = {
            let _release = molt_runtime_core::GilReleaseGuard::suspend();
            userdb::all()
        };
        let entries = match entries {
            Ok(entries) => entries,
            Err(_) => return raise_exception::<u64>(py, "MemoryError", ""),
        };
        let mut values = Vec::new();
        if values.try_reserve_exact(entries.len()).is_err() {
            return raise_exception::<u64>(py, "MemoryError", "");
        }
        for value in entries {
            let bits = entry(py, class, value);
            let owner = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) };
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            values.push(owner);
        }
        let pointer = alloc_list(py, OwnedRuntimeValue::as_bits_slice(&values));
        if pointer.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(pointer).bits()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pwd_getpwuid(uid: u64, class: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if require_operation::<u64>(py, OperationId::PwdGetpwuid, AuditArgs::None).is_err() {
            return MoltObject::none().bits();
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let Some(uid) = native::uid(py, uid) else {
                return MoltObject::none().bits();
            };
            native::lookup_uid(py, uid, class)
        }
        #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
        {
            let _ = (uid, class);
            raise_provider_absence(py)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pwd_getpwnam(name: u64, class: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if require_operation::<u64>(py, OperationId::PwdGetpwnam, AuditArgs::None).is_err() {
            return MoltObject::none().bits();
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let name_bytes = match native::name(py, name) {
                Ok(name) => name,
                Err(error) => return error,
            };
            native::lookup_name(py, name, &name_bytes, class)
        }
        #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
        {
            let _ = (name, class);
            raise_provider_absence(py)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pwd_getpwall(class: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if require_operation::<u64>(py, OperationId::PwdGetpwall, AuditArgs::None).is_err() {
            return MoltObject::none().bits();
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            native::all(py, class)
        }
        #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
        {
            let _ = class;
            raise_provider_absence(py)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::exceptions::{
        ExceptionValue, exception_matches_builtin_name, molt_exception_last_pending,
    };

    #[test]
    fn provider_absence_preserves_module_name_after_exception_retirement() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let original_name =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"pwd").unwrap());
            let name_header = unsafe {
                header_from_obj_ptr(obj_from_bits(original_name.bits()).as_ptr().unwrap())
            };
            let references = unsafe { (*name_header).ref_count_snapshot() };
            assert_eq!(raise_provider_absence(py), MoltObject::none().bits());
            assert!(exception_pending(py));
            let exception = ExceptionValue::adopt(py, molt_exception_last_pending());
            assert!(exception_matches_builtin_name(
                py,
                exception.bits(),
                "ModuleNotFoundError"
            ));
            clear_exception(py);
            let message = ExceptionValue::adopt(
                py,
                crate::builtins::exceptions::molt_exception_message(exception.bits()),
            );
            assert_eq!(
                string_obj_to_owned(obj_from_bits(message.bits())).as_deref(),
                Some("No module named 'pwd'")
            );
            let key = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"name").unwrap());
            let name = ExceptionValue::adopt(
                py,
                crate::molt_getattr_builtin(exception.bits(), key.bits(), missing_bits(py)),
            );
            let path_key =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"path").unwrap());
            let path = ExceptionValue::adopt(
                py,
                crate::molt_getattr_builtin(exception.bits(), path_key.bits(), missing_bits(py)),
            );
            assert_eq!(name.bits(), original_name.bits());
            assert!(obj_from_bits(path.bits()).is_none());
            assert!(!exception_pending(py));
            drop(exception);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(name.bits())).as_deref(),
                Some("pwd")
            );
            drop(name);
            assert_eq!(unsafe { (*name_header).ref_count_snapshot() }, references);
        });
    }
}
