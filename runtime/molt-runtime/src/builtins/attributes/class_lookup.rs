use super::*;
use crate::builtins::attr::descriptor_call1;

/// One normal attribute transaction for custom and default lookup, on ordinary
/// objects, dataclasses, dictless instances and metaclass receivers alike.
/// Capture __getattr__ before any getattribute/descriptor callback can replace
/// it. Explicit object/type.__getattribute__ entrypoints bypass this transaction.
pub(super) unsafe fn attribute_lookup_transaction<F>(
    py: &PyToken<'_>,
    object: *mut u8,
    owner: *mut u8,
    name: u64,
    default: Option<u64>,
    default_lookup: F,
) -> Option<u64>
where
    F: FnOnce() -> Option<u64>,
{
    unsafe {
        let receiver_bits = MoltObject::from_ptr(object).bits();
        let owner_bits = MoltObject::from_ptr(owner).bits();
        inc_ref_bits(py, receiver_bits);
        inc_ref_bits(py, owner_bits);
        inc_ref_bits(py, name);
        let fallback_name =
            intern_static_name(py, &runtime_state(py).interned.getattr_name, b"__getattr__");
        let fallback = if exception_pending(py) {
            None
        } else {
            class_attr_lookup_raw_mro(py, owner, fallback_name)
        };
        if let Some(bits) = fallback {
            inc_ref_bits(py, bits);
        }
        let getattribute_name = intern_static_name(
            py,
            &runtime_state(py).interned.getattribute_name,
            b"__getattribute__",
        );
        let raw = if exception_pending(py) {
            None
        } else {
            class_attr_lookup_raw_mro(py, owner, getattribute_name)
        };
        if let Some(bits) = raw {
            inc_ref_bits(py, bits);
        }
        let result = (|| {
            if exception_pending(py) {
                return None;
            }
            if fallback.is_some() {
                traceback_suppress_enter();
            }
            exception_stack_push();
            let result = match raw {
                Some(raw) if Some(raw) != default => {
                    descriptor_call1(py, raw, owner, Some(receiver_bits), name)
                }
                _ => default_lookup(),
            };
            if fallback.is_some() {
                traceback_suppress_exit();
            }
            // A default lookup reports a missing attribute as None without a
            // pending exception. A Python None value is Some(tagged None).
            let use_fallback = if exception_pending(py) {
                if let Some(bits) = result {
                    dec_ref_bits(py, bits);
                }
                let error = molt_exception_last_pending();
                let admitted = fallback.is_some()
                    && exception_matches_builtin_name(py, error, "AttributeError");
                if admitted {
                    molt_exception_clear();
                }
                dec_ref_bits(py, error);
                admitted
            } else if result.is_some() {
                exception_stack_pop(py);
                return result;
            } else {
                fallback.is_some()
            };
            exception_stack_pop(py);
            if !use_fallback {
                return None;
            }
            exception_stack_push();
            let result = descriptor_call1(py, fallback.unwrap(), owner, Some(receiver_bits), name);
            let result = if exception_pending(py) {
                if let Some(bits) = result {
                    dec_ref_bits(py, bits);
                }
                None
            } else {
                result
            };
            exception_stack_pop(py);
            result
        })();
        if let Some(bits) = raw {
            dec_ref_bits(py, bits);
        }
        if let Some(bits) = fallback {
            dec_ref_bits(py, bits);
        }
        dec_ref_bits(py, name);
        dec_ref_bits(py, owner_bits);
        dec_ref_bits(py, receiver_bits);
        result
    }
}

pub(crate) unsafe fn type_attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, true) }
}

pub(crate) unsafe fn type_attr_lookup_ptr_default(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, false) }
}

unsafe fn type_attr_lookup_ptr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    allow_meta_custom_getattribute: bool,
) -> Option<u64> {
    unsafe {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        inc_ref_bits(_py, class_bits);
        let _class_guard = crate::PtrDropGuard::new(obj_ptr);
        let meta_bits = object_class_bits(obj_ptr);
        let meta_ptr = if meta_bits != 0 {
            obj_from_bits(meta_bits).as_ptr()
        } else {
            obj_from_bits(builtin_classes(_py).type_obj).as_ptr()
        };
        let meta_ptr = match meta_ptr {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TYPE => Some(ptr),
            _ => None,
        };
        let _meta_guard = meta_ptr.map(|ptr| {
            inc_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            crate::PtrDropGuard::new(ptr)
        });
        if let Some(meta_ptr) = meta_ptr {
            if allow_meta_custom_getattribute {
                return attribute_lookup_transaction(
                    _py,
                    obj_ptr,
                    meta_ptr,
                    attr_bits,
                    type_method_bits(_py, "__getattribute__"),
                    || type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, false),
                );
            }
            if let Some(meta_bits) = class_attr_lookup_raw_mro(_py, meta_ptr, attr_bits)
                && descriptor_is_data(_py, meta_bits)
            {
                return descriptor_bind(
                    _py,
                    meta_bits,
                    Some(MoltObject::from_ptr(meta_ptr).bits()),
                    Some(class_bits),
                );
            }
        }
        // Intrinsic metadata is represented by the metaclass data descriptors
        // above. Local namespace entries use ordinary descriptor precedence.
        let dict_bits = class_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
            && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits)
        {
            return descriptor_bind(
                _py,
                val_bits,
                Some(MoltObject::from_ptr(obj_ptr).bits()),
                None,
            );
        }

        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            let builtins = builtin_classes(_py);
            if class_bits == builtins.object
                && (name == "__getattribute__" || name == "__setattr__" || name == "__delattr__")
                && let Some(func_bits) = object_method_bits(_py, name.as_str())
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            if name == "__init_subclass__"
                && matches!(
                    std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                    Some("1")
                )
            {
                let builtins = builtin_classes(_py);
                eprintln!(
                    "molt init_subclass lookup class_bits=0x{:x} builtins.object=0x{:x} is_builtin={}",
                    class_bits,
                    builtins.object,
                    is_builtin_class_bits(_py, class_bits),
                );
            }
            if class_bits == builtins.tuple
                && name == "__new__"
                && let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__new__")
            {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }

            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if name == "fromkeys" {
                let builtins = builtin_classes(_py);
                if issubclass_bits(class_bits, builtins.dict)
                    && let Some(func_bits) = dict_method_bits(_py, name.as_str())
                {
                    let bound_bits = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound_bits);
                }
            }
        }
        if let Some(class_bits) = class_attr_lookup(_py, obj_ptr, obj_ptr, None, attr_bits) {
            return Some(class_bits);
        }
        if let Some(meta_ptr) = meta_ptr
            && let Some(meta_bits) = class_attr_lookup_raw_mro(_py, meta_ptr, attr_bits)
        {
            return descriptor_bind(
                _py,
                meta_bits,
                Some(MoltObject::from_ptr(meta_ptr).bits()),
                Some(class_bits),
            );
        }
        None
    }
}
