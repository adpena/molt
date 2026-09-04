use super::*;
use crate::audit::{AuditArgs, audit_capability_decision};
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
            raise_exception::<_>(_py, "MemoryError", "out of memory")
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
            raise_exception::<_>(_py, "MemoryError", "out of memory")
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
            raise_exception::<_>(_py, "MemoryError", "out of memory")
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
            raise_exception::<_>(_py, "MemoryError", "out of memory")
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
pub extern "C" fn molt_locale_setlocale(_category_bits: u64, locale_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(locale_bits).is_none() {
            let current = locale_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            return match alloc_str_bits(_py, &current) {
                Ok(bits) => bits,
                Err(err_bits) => err_bits,
            };
        }
        let Some(mut locale) = string_obj_to_owned(obj_from_bits(locale_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "locale must be str or None");
        };
        if locale.is_empty() {
            // POSIX setlocale("") — resolve from environment variables in
            // priority order: LC_ALL, LC_<category>, LANG. Honors live OS
            // locale rather than the Rust-internal default.
            locale = std::env::var("LC_ALL")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var("LC_CTYPE").ok().filter(|s| !s.is_empty()))
                .or_else(|| std::env::var("LANG").ok().filter(|s| !s.is_empty()))
                .unwrap_or_else(|| String::from("C"));
        }
        if locale == "POSIX" {
            locale = String::from("C");
        }
        *locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = locale.clone();
        match alloc_str_bits(_py, &locale) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_getpreferredencoding(_do_setlocale_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let current = locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match alloc_str_bits(_py, locale_encoding_label(&current)) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locale_getlocale(_category_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let current = locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if current == "C" || current == "POSIX" {
            let tuple_ptr =
                alloc_tuple(_py, &[MoltObject::none().bits(), MoltObject::none().bits()]);
            if tuple_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let locale_bits = match alloc_str_bits(_py, &current) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let encoding_bits = match alloc_str_bits(_py, locale_encoding_label(&current)) {
            Ok(bits) => bits,
            Err(err_bits) => {
                dec_ref_bits(_py, locale_bits);
                return err_bits;
            }
        };
        let tuple_ptr = alloc_tuple(_py, &[locale_bits, encoding_bits]);
        dec_ref_bits(_py, locale_bits);
        dec_ref_bits(_py, encoding_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
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
        let result_bits = if obj_eq(_py, obj_from_bits(n_bits), one) {
            singular_bits
        } else {
            plural_bits
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_get(key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return default_bits,
        };
        let allowed = has_capability(_py, "env.read");
        audit_capability_decision(
            "env.get",
            "env.read",
            AuditArgs::Env { key: key.clone() },
            allowed,
        );
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing env.read capability");
        }
        let value = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.get(&key).cloned()
        };
        match value {
            Some(val) => {
                if trace_env_get() {
                    eprintln!("molt_env_get key={key} hit=true");
                }
                let ptr = alloc_string(_py, val.as_bytes());
                if ptr.is_null() {
                    default_bits
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            None => {
                if trace_env_get() {
                    eprintln!("molt_env_get key={key} hit=false");
                }
                default_bits
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_set(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let allowed = has_capability(_py, "env.write");
        audit_capability_decision(
            "env.set",
            "env.write",
            AuditArgs::Env { key: key.clone() },
            allowed,
        );
        if !allowed {
            return raise_capability_denied(_py, "env.write");
        }
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(value) => value,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.insert(key, value);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_unset(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::from_bool(false).bits(),
        };
        let allowed = has_capability(_py, "env.write");
        audit_capability_decision(
            "env.unset",
            "env.write",
            AuditArgs::Env { key: key.clone() },
            allowed,
        );
        if !allowed {
            return raise_capability_denied(_py, "env.write");
        }
        let removed = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.remove(&key).is_some()
        };
        MoltObject::from_bool(removed).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_len() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "env.read");
        audit_capability_decision("env.len", "env.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing env.read capability");
        }
        let len = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.len()
        };
        MoltObject::from_int(len as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_contains(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::from_bool(false).bits(),
        };
        let allowed = has_capability(_py, "env.read");
        audit_capability_decision(
            "env.contains",
            "env.read",
            AuditArgs::Env { key: key.clone() },
            allowed,
        );
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing env.read capability");
        }
        let contains = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.contains_key(&key)
        };
        MoltObject::from_bool(contains).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_snapshot() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "env.read");
        audit_capability_decision("env.snapshot", "env.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<u64>(_py, "PermissionError", "missing env.read capability");
        }
        let env_pairs: Vec<(String, String)> = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .iter()
                .map(|(key, val)| (key.clone(), val.clone()))
                .collect()
        };
        let mut pairs = Vec::with_capacity(env_pairs.len() * 2);
        let mut owned_bits = Vec::with_capacity(env_pairs.len() * 2);
        for (key, val) in env_pairs {
            let key_ptr = alloc_string(_py, key.as_bytes());
            let val_ptr = alloc_string(_py, val.as_bytes());
            if key_ptr.is_null() || val_ptr.is_null() {
                if !key_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(key_ptr).bits());
                }
                if !val_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(val_ptr).bits());
                }
                continue;
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let val_bits = MoltObject::from_ptr(val_ptr).bits();
            pairs.push(key_bits);
            pairs.push(val_bits);
            owned_bits.push(key_bits);
            owned_bits.push(val_bits);
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_popitem() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "env.write");
        audit_capability_decision("env.popitem", "env.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_capability_denied(_py, "env.write");
        }
        let (key, value) = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some((key, value)) = guard
                .iter()
                .next_back()
                .map(|(key, value)| (key.clone(), value.clone()))
            else {
                return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
            };
            guard.remove(&key);
            (key, value)
        };
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(key_ptr).bits());
            return MoltObject::none().bits();
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[key_bits, value_bits]);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, value_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_clear() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "env.write");
        audit_capability_decision("env.clear", "env.write", AuditArgs::None, allowed);
        if !allowed {
            return raise_capability_denied(_py, "env.write");
        }
        {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.clear();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_putenv(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let allowed = has_capability(_py, "env.write");
        audit_capability_decision(
            "env.putenv",
            "env.write",
            AuditArgs::Env { key: key.clone() },
            allowed,
        );
        if !allowed {
            return raise_capability_denied(_py, "env.write");
        }
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(value) => value,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = process_env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.insert(key, value);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_env_unsetenv(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let allowed = has_capability(_py, "env.write");
        audit_capability_decision(
            "env.unsetenv",
            "env.write",
            AuditArgs::Env { key: key.clone() },
            allowed,
        );
        if !allowed {
            return raise_capability_denied(_py, "env.write");
        }
        {
            let mut guard = process_env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.remove(&key);
        }
        MoltObject::none().bits()
    })
}
