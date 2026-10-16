use super::*;
use crate::audit::AuditArgs;
use molt_runtime_platform::socket_constants::{collect_errno_constants, socket_constants};

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_getnode() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match uuid_node() {
            Ok(node) => MoltObject::from_int(node as i64).bits(),
            Err(err) => raise_exception::<_>(_py, "RuntimeError", &err),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid4_bytes() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let payload = match uuid_v4_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return raise_exception::<_>(_py, "RuntimeError", &err),
        };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid1_bytes(node_bits: u64, clock_seq_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = crate::operation_allowed(_py, crate::OperationId::TimeUuid1, AuditArgs::None);
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing time.wall capability");
        }
        let node_override = if obj_from_bits(node_bits).is_none() {
            None
        } else {
            let value = index_i64_from_obj(_py, node_bits, "node must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !(0..=0xFFFF_FFFF_FFFF_i64).contains(&value) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "node is out of range (need a 48-bit value)",
                );
            }
            Some(value as u64)
        };
        let clock_seq_override = if obj_from_bits(clock_seq_bits).is_none() {
            None
        } else {
            let value = index_i64_from_obj(_py, clock_seq_bits, "clock_seq must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !(0..=0x3FFF_i64).contains(&value) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "clock_seq is out of range (need a 14-bit value)",
                );
            }
            Some(value as u16)
        };
        let payload = match uuid_v1_bytes(node_override, clock_seq_override) {
            Ok(bytes) => bytes,
            Err(err) => return raise_exception::<_>(_py, "RuntimeError", &err),
        };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid3_bytes(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let namespace = match bytes_arg_from_bits(_py, namespace_bits, "namespace") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if namespace.len() != 16 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "namespace must be a 16-byte UUID payload",
            );
        }
        let name = match bytes_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = uuid_v3_bytes(&namespace, &name);
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid5_bytes(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let namespace = match bytes_arg_from_bits(_py, namespace_bits, "namespace") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if namespace.len() != 16 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "namespace must be a 16-byte UUID payload",
            );
        }
        let name = match bytes_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = uuid_v5_bytes(&namespace, &name);
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_name() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        init_platform_cached_owned_bits(_py, &platform_state(_py).os_name_cache, || {
            let ptr = alloc_string(_py, os_name_str().as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_platform() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        init_platform_cached_owned_bits(_py, &platform_state(_py).sys_platform_cache, || {
            let ptr = alloc_string(_py, sys_platform_str().as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_setlocale(category_bits: u64, locale_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(category) = crate::builtins::numbers::index_c_int_from_obj(_py, category_bits)
        else {
            return MoltObject::none().bits();
        };
        let locale = if obj_from_bits(locale_bits).is_none() {
            None
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(locale_bits)) else {
                return raise_exception::<u64>(_py, "TypeError", "locale must be str or None");
            };
            if value.contains('\0') {
                return raise_exception::<u64>(_py, "ValueError", "embedded null character");
            }
            Some(value)
        };
        match set_locale(category, locale.as_deref()) {
            Ok(value) => alloc_str_bits(_py, &value).unwrap_or_else(|error| error),
            // _locale owns its public Error class. A refused libc transition
            // leaves the current locale intact and has no Python value.
            Err(_) => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_getencoding() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match locale_encoding() {
            Ok(value) => alloc_str_bits(_py, &value).unwrap_or_else(|error| error),
            Err(error) => raise_exception::<u64>(_py, "RuntimeError", &error),
        }
    })
}

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_getdefaultlocale() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let legacy_lcid = !crate::object::ops_sys::runtime_target_at_least(py, 3, 15);
        let (language, encoding) =
            molt_runtime_platform::env_support::windows_default_locale(legacy_lcid);
        let language_bits = match language {
            Some(value) => match alloc_str_bits(py, &value) {
                Ok(bits) => bits,
                Err(error) => return error,
            },
            None => MoltObject::none().bits(),
        };
        let encoding_bits = match alloc_str_bits(py, &encoding) {
            Ok(bits) => bits,
            Err(error) => {
                dec_ref_bits(py, language_bits);
                return error;
            }
        };
        let ptr = alloc_tuple_owned(py, &[language_bits, encoding_bits]);
        if ptr.is_null() {
            dec_ref_bits(py, language_bits);
            dec_ref_bits(py, encoding_bits);
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_constants() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let entries = [
            ("LC_CTYPE", libc::LC_CTYPE),
            ("LC_NUMERIC", libc::LC_NUMERIC),
            ("LC_TIME", libc::LC_TIME),
            ("LC_COLLATE", libc::LC_COLLATE),
            ("LC_MONETARY", libc::LC_MONETARY),
            ("LC_ALL", libc::LC_ALL),
            ("CHAR_MAX", libc::c_char::MAX as i32),
            #[cfg(not(windows))]
            ("LC_MESSAGES", libc::LC_MESSAGES),
        ];
        let mut pairs = Vec::with_capacity(entries.len() * 2);
        for (name, value) in entries {
            let key = match alloc_str_bits(_py, name) {
                Ok(bits) => bits,
                Err(error) => {
                    for owned in pairs {
                        dec_ref_bits(_py, owned);
                    }
                    return error;
                }
            };
            pairs.push(key);
            pairs.push(MoltObject::from_int(i64::from(value)).bits());
        }
        let ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in pairs {
            dec_ref_bits(_py, bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gettext_gettext(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, message_bits);
        message_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gettext_ngettext(singular_bits: u64, plural_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let one = MoltObject::from_int(1);
        let result_bits = match crate::object::ops_compare::compare_object_eq_bool(
            _py,
            obj_from_bits(n_bits),
            one,
        ) {
            crate::object::ops_compare::CompareBoolOutcome::True => singular_bits,
            crate::object::ops_compare::CompareBoolOutcome::False => plural_bits,
            crate::object::ops_compare::CompareBoolOutcome::Error
            | crate::object::ops_compare::CompareBoolOutcome::NotComparable => {
                return MoltObject::none().bits();
            }
        };
        inc_ref_bits(_py, result_bits);
        result_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_errno_constants() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        init_platform_cached_owned_bits(_py, &platform_state(_py).errno_constants_cache, || {
            let constants = collect_errno_constants();
            let mut pairs = Vec::with_capacity(constants.len() * 2);
            let mut reverse_pairs = Vec::with_capacity(constants.len() * 2);
            let mut owned_bits = Vec::with_capacity(constants.len() * 2);
            for (name, value) in constants {
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = MoltObject::from_int(value).bits();
                pairs.push(name_bits);
                pairs.push(value_bits);
                reverse_pairs.push(value_bits);
                reverse_pairs.push(name_bits);
                owned_bits.push(name_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let reverse_ptr = alloc_dict_with_pairs(_py, &reverse_pairs);
            if reverse_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(dict_ptr).bits());
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let reverse_bits = MoltObject::from_ptr(reverse_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[dict_bits, reverse_bits]);
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, reverse_bits);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socket_constants() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        init_platform_cached_owned_bits(_py, &platform_state(_py).socket_constants_cache, || {
            let constants = socket_constants();
            let mut pairs = Vec::with_capacity(constants.len() * 2);
            let mut owned_bits = Vec::with_capacity(constants.len() * 2);
            for (name, value) in constants {
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = MoltObject::from_int(value).bits();
                pairs.push(name_bits);
                pairs.push(value_bits);
                owned_bits.push(name_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            dict_bits
        })
    })
}

fn env_key_audit_args(bits: u64) -> AuditArgs {
    let Some(pointer) = obj_from_bits(bits).as_ptr() else {
        return AuditArgs::None;
    };
    // Inspect only exact owned string/bytes storage. Permission checks must
    // precede codecs, __fspath__, descriptors and all inspection of values.
    let raw = unsafe {
        match object_type_id(pointer) {
            TYPE_ID_STRING => {
                std::slice::from_raw_parts(string_bytes(pointer), string_len(pointer))
            }
            TYPE_ID_BYTES => std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)),
            _ => return AuditArgs::None,
        }
    };
    AuditArgs::Env {
        key: String::from_utf8_lossy(raw).into_owned(),
    }
}

#[cfg(windows)]
fn env_string_from_bits(py: &PyToken<'_>, bits: u64) -> Result<std::ffi::OsString, u64> {
    let Some(pointer) = obj_from_bits(bits)
        .as_ptr()
        .filter(|pointer| unsafe { object_type_id(*pointer) == TYPE_ID_STRING })
    else {
        return Err(raise_exception::<u64>(py, "TypeError", "str expected"));
    };
    let text = unsafe { std::slice::from_raw_parts(string_bytes(pointer), string_len(pointer)) };
    crate::builtins::io::os_string_from_text(py, text, Some(bits))
}

#[cfg(windows)]
fn env_string_bits(py: &PyToken<'_>, value: &std::ffi::OsStr) -> u64 {
    let text = match crate::builtins::io::os_string_text(py, value) {
        Ok(text) => text,
        Err(error) => return error,
    };
    let pointer = alloc_string(py, &text);
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

#[cfg(not(windows))]
fn checked_process_env_string(
    py: &PyToken<'_>,
    value: std::ffi::OsString,
) -> Result<std::ffi::OsString, u64> {
    if value.as_encoded_bytes().contains(&0) {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            "embedded null byte",
        ));
    }
    Ok(value)
}

fn process_env_string_from_bits(py: &PyToken<'_>, bits: u64) -> Result<std::ffi::OsString, u64> {
    #[cfg(windows)]
    {
        env_string_from_bits(py, bits)
    }
    #[cfg(not(windows))]
    {
        // CPython's POSIX putenv/unsetenv use FSConverter, accepting the same
        // str/bytes/PathLike protocol and filesystem codec as path consumers.
        let value = crate::builtins::io::path_from_bits(py, bits)?.into_os_string();
        checked_process_env_string(py, value)
    }
}

fn raise_environment_error(
    py: &PyToken<'_>,
    error: molt_runtime_platform::env_support::EnvironmentError,
) -> u64 {
    match error {
        molt_runtime_platform::env_support::EnvironmentError::Value(message) => {
            raise_exception::<u64>(py, "ValueError", message)
        }
        molt_runtime_platform::env_support::EnvironmentError::Errno(errno) => {
            raise_os_error_errno::<u64>(py, i64::from(errno), "environment")
        }
    }
}

// The platform root owns one exact Python dict. No raw map mirrors it and no
// intrinsic publishes its handle. Every borrower retains it across callbacks.
fn environment_mapping<'a>(
    py: &'a PyToken<'_>,
) -> Result<molt_runtime_core::OwnedRuntimeValue<'a>, u64> {
    let slot = &platform_state(py).environment_mapping;
    let initializing = MoltObject::from_bool(true).bits();
    let retired = MoltObject::none().bits();
    let current = slot.load(Ordering::Acquire);
    if current == retired {
        return Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            "environment mapping is retired",
        ));
    }
    if current == initializing {
        return Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            "reentrant environment initialization",
        ));
    }
    if current != 0 {
        return Ok(molt_runtime_core::OwnedRuntimeValue::retain(
            py.core_token(),
            current,
        ));
    }
    if slot
        .compare_exchange(0, initializing, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            "environment initialization state changed",
        ));
    }
    struct Reset<'a>(&'a AtomicU64, u64);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            let _ = self
                .0
                .compare_exchange(self.1, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
    let _reset = Reset(slot, initializing);
    // The provider releases its process lock before any Python allocation.
    let snapshot = process_env_snapshot();
    let dict = crate::object::builders::alloc_internal_dict(py);
    if dict.is_null() {
        return Err(MoltObject::none().bits());
    }
    let root = unsafe {
        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
            py.core_token(),
            MoltObject::from_ptr(dict).bits(),
        )
    };
    for (key, value) in snapshot {
        #[cfg(windows)]
        let (key_bits, value_bits) = {
            let key_bits = env_string_bits(py, &key);
            let key_owner = unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), key_bits)
            };
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            // Exact startup strings use the same versioned Unicode operation as
            // the Python key encoder; do not uppercase UTF-16 or Rust chars.
            let upper = crate::object::ops_string::molt_string_upper(key_owner.bits());
            (upper, env_string_bits(py, &value))
        };
        #[cfg(not(windows))]
        let (key_bits, value_bits) = {
            let key = crate::builtins::io::os_string_bytes(py, &key)?;
            let value = crate::builtins::io::os_string_bytes(py, &value)?;
            let key = alloc_bytes(py, &key);
            if key.is_null() {
                return Err(MoltObject::none().bits());
            }
            let key_bits = MoltObject::from_ptr(key).bits();
            let value = alloc_bytes(py, &value);
            if value.is_null() {
                dec_ref_bits(py, key_bits);
                return Err(MoltObject::none().bits());
            }
            (key_bits, MoltObject::from_ptr(value).bits())
        };
        let key = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), key_bits)
        };
        let value = unsafe {
            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), value_bits)
        };
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let displaced =
            unsafe { crate::object::ops::dict_set_deferred(py, dict, key.bits(), value.bits()) }
                .map_err(|()| MoltObject::none().bits())?;
        drop(displaced);
    }
    if slot
        .compare_exchange(
            initializing,
            root.bits(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            "environment mapping retired during initialization",
        ));
    }
    let bits = root.into_bits(); // Published reference belongs to PlatformRuntimeState.
    Ok(molt_runtime_core::OwnedRuntimeValue::retain(
        py.core_token(),
        bits,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_init() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        match environment_mapping(py) {
            Ok(_) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_get(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) =
            require_operation::<u64>(py, OperationId::EnvGet, env_key_audit_args(key_bits))
        {
            return error;
        }
        let root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let found = unsafe {
            dict_get_in_place(py, obj_from_bits(root.bits()).as_ptr().unwrap(), key_bits)
        };
        match found {
            Ok(Some(value)) => {
                molt_runtime_core::OwnedRuntimeValue::retain(py.core_token(), value).into_bits()
            }
            Ok(None) => crate::raise_key_error_with_key(py, key_bits),
            Err(()) => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_set(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) =
            require_operation::<u64>(py, OperationId::EnvSet, env_key_audit_args(key_bits))
        {
            return error;
        }
        let root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let key = match process_env_string_from_bits(py, key_bits) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let value = match process_env_string_from_bits(py, value_bits) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if let Err(error) = process_env_set(&key, Some(&value)) {
            return raise_environment_error(py, error);
        }
        // CPython mutates the process before hashing/storing the exact encoded
        // objects. A subsequent Python hash/allocation error is observable.
        match unsafe {
            crate::object::ops::dict_set_deferred(
                py,
                obj_from_bits(root.bits()).as_ptr().unwrap(),
                key_bits,
                value_bits,
            )
        } {
            Ok(displaced) => {
                drop(displaced);
                MoltObject::none().bits()
            }
            Err(()) => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_delete(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) =
            require_operation::<u64>(py, OperationId::EnvUnset, env_key_audit_args(key_bits))
        {
            return error;
        }
        let root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let key = match process_env_string_from_bits(py, key_bits) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if let Err(error) = process_env_set(&key, None) {
            return raise_environment_error(py, error);
        }
        match unsafe {
            crate::object::ops::dict_del_deferred(
                py,
                obj_from_bits(root.bits()).as_ptr().unwrap(),
                key_bits,
            )
        } {
            Ok(Some(detached)) => {
                drop(detached);
                MoltObject::none().bits()
            }
            Ok(None) => crate::raise_key_error_with_key(py, key_bits),
            Err(()) => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_keys() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation::<u64>(py, OperationId::EnvKeys, AuditArgs::None) {
            return error;
        }
        let root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let keys = match unsafe {
            crate::object::ops_dict::dict_snapshot(
                py,
                obj_from_bits(root.bits()).as_ptr().unwrap(),
                crate::object::ops_dict::DictSnapshotKind::Keys,
            )
        } {
            Some(keys) => keys,
            None => return MoltObject::none().bits(),
        };
        let list = alloc_list(py, &keys);
        if list.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_len() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation::<u64>(py, OperationId::EnvLen, AuditArgs::None) {
            return error;
        }
        let root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        MoltObject::from_int(
            unsafe { dict_len(obj_from_bits(root.bits()).as_ptr().unwrap()) } as i64,
        )
        .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_mapping_items_iter() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation::<u64>(py, OperationId::EnvSnapshot, AuditArgs::None)
        {
            return error;
        }
        let root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let view = crate::object::ops_dict::molt_dict_items(root.bits());
        let view =
            unsafe { molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), view) };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        crate::molt_iter(view.bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_putenv(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) =
            require_operation::<u64>(py, OperationId::EnvPutenv, env_key_audit_args(key_bits))
        {
            return error;
        }
        let _root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let key = match process_env_string_from_bits(py, key_bits) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let value = match process_env_string_from_bits(py, value_bits) {
            Ok(value) => value,
            Err(error) => return error,
        };
        match process_env_set(&key, Some(&value)) {
            Ok(()) => MoltObject::none().bits(),
            Err(error) => raise_environment_error(py, error),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_unsetenv(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) =
            require_operation::<u64>(py, OperationId::EnvUnsetenv, env_key_audit_args(key_bits))
        {
            return error;
        }
        let _root = match environment_mapping(py) {
            Ok(root) => root,
            Err(error) => return error,
        };
        let key = match process_env_string_from_bits(py, key_bits) {
            Ok(value) => value,
            Err(error) => return error,
        };
        match process_env_set(&key, None) {
            Ok(()) => MoltObject::none().bits(),
            Err(error) => raise_environment_error(py, error),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_need_current_directory_for_exe_path(command_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(error) = require_operation::<u64>(
            py,
            OperationId::OsNeedCurrentDirectoryForExePath,
            AuditArgs::Env {
                key: "NoDefaultCurrentDirectoryInExePath".to_owned(),
            },
        ) {
            return error;
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let command = match env_string_from_bits(py, command_bits) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let mut wide: Vec<u16> = command.encode_wide().collect();
            if wide.contains(&0) {
                return raise_exception::<u64>(py, "ValueError", "embedded null character");
            }
            wide.push(0);
            MoltObject::from_bool(
                unsafe {
                    molt_runtime_platform::windows_abi::NeedCurrentDirectoryForExePathW(
                        wide.as_ptr(),
                    )
                } != 0,
            )
            .bits()
        }
        #[cfg(not(windows))]
        {
            let _ = command_bits;
            raise_exception::<u64>(
                py,
                "RuntimeError",
                "Windows executable directory policy is unavailable on this target",
            )
        }
    })
}
