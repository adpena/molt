use crate::PyToken;
use molt_obj_model::MoltObject;

use crate::builtins::attr::module_attr_lookup;
use crate::{
    alloc_dict_with_pairs, alloc_module_obj, alloc_string, class_name_for_error, dec_ref_bits,
    dict_del_in_place, dict_get_in_place, dict_order, dict_set_in_place, exception_pending,
    inc_ref_bits, intern_static_name, is_truthy, module_dict_bits, module_name_bits,
    molt_is_callable, molt_iter, molt_iter_next, obj_eq, obj_from_bits, object_type_id,
    raise_exception, runtime_state, seq_vec_ref, string_bytes, string_len, string_obj_to_owned,
    type_of_bits, TYPE_ID_DICT, TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE,
};

#[no_mangle]
pub extern "C" fn molt_module_new(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "module name must be str");
            }
        }
        let ptr = alloc_module_obj(_py, name_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let dict_bits = module_dict_bits(ptr);
            let dict_obj = obj_from_bits(dict_bits);
            if let Some(dict_ptr) = dict_obj.as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let key_ptr = alloc_string(_py, b"__name__");
                    if !key_ptr.is_null() {
                        let key_bits = MoltObject::from_ptr(key_ptr).bits();
                        dict_set_in_place(_py, dict_ptr, key_bits, name_bits);
                        dec_ref_bits(_py, key_bits);
                    }
                }
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_module_cache_get(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        if let Some(bits) = guard.get(&name) {
            inc_ref_bits(_py, *bits);
            return *bits;
        }
        MoltObject::none().bits()
    })
}

fn sys_modules_dict_ptr(_py: &PyToken<'_>, sys_bits: u64) -> Option<*mut u8> {
    let sys_obj = obj_from_bits(sys_bits);
    let sys_ptr = sys_obj.as_ptr()?;
    unsafe {
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return None;
        }
        let dict_bits = module_dict_bits(sys_ptr);
        let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return None,
        };
        let modules_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.modules_name, b"modules");
        if obj_from_bits(modules_name_bits).is_none() {
            return None;
        }
        let mut modules_bits = dict_get_in_place(_py, dict_ptr, modules_name_bits);
        if modules_bits.is_none() {
            let new_ptr = alloc_dict_with_pairs(_py, &[]);
            if new_ptr.is_null() {
                return None;
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            dict_set_in_place(_py, dict_ptr, modules_name_bits, new_bits);
            modules_bits = Some(new_bits);
            dec_ref_bits(_py, new_bits);
        }
        let modules_ptr = match obj_from_bits(modules_bits?).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "sys.modules must be dict"),
        };
        Some(modules_ptr)
    }
}

#[no_mangle]
pub extern "C" fn molt_module_cache_set(name_bits: u64, module_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "module name must be str"),
        };
        let is_sys = name == "sys";
        let (sys_bits, cached_modules) = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let mut guard = cache.lock().unwrap();
            if let Some(old) = guard.insert(name, module_bits) {
                dec_ref_bits(_py, old);
            }
            inc_ref_bits(_py, module_bits);
            if is_sys {
                let entries = guard
                    .iter()
                    .map(|(key, &bits)| (key.clone(), bits))
                    .collect::<Vec<_>>();
                (Some(module_bits), Some(entries))
            } else {
                (guard.get("sys").copied(), None)
            }
        };
        if let Some(sys_bits) = sys_bits {
            if let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits) {
                if let Some(entries) = cached_modules {
                    for (key, bits) in entries {
                        let key_ptr = alloc_string(_py, key.as_bytes());
                        if key_ptr.is_null() {
                            return raise_exception::<_>(_py, "MemoryError", "out of memory");
                        }
                        let key_bits = MoltObject::from_ptr(key_ptr).bits();
                        unsafe {
                            dict_set_in_place(_py, modules_ptr, key_bits, bits);
                        }
                        dec_ref_bits(_py, key_bits);
                    }
                } else {
                    unsafe {
                        dict_set_in_place(_py, modules_ptr, name_bits, module_bits);
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_module_get_attr(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module attribute access expects module",
                );
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let _dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            if let Some(val) = module_attr_lookup(_py, module_ptr, attr_bits) {
                return val;
            }
            let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr)))
                .unwrap_or_default();
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
            return raise_exception::<_>(_py, "AttributeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_module_get_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module attribute access expects module",
                );
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            if let Some(val) = dict_get_in_place(_py, dict_ptr, name_bits) {
                inc_ref_bits(_py, val);
                return val;
            }
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<name>".to_string());
            let msg = format!("name '{name}' is not defined");
            return raise_exception::<_>(_py, "NameError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_module_del_global(module_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "module attribute access expects module",
            );
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module attribute access expects module",
                );
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            if dict_del_in_place(_py, dict_ptr, name_bits) {
                return MoltObject::none().bits();
            }
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<name>".to_string());
            let msg = format!("name '{name}' is not defined");
            return raise_exception::<_>(_py, "NameError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_module_get_name(module_bits: u64, attr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Keep wasm import parity; module __name__ is stored in the module dict.
        molt_module_get_attr(module_bits, attr_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_module_set_attr(module_bits: u64, attr_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_obj = obj_from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module attribute set expects module");
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "module attribute set expects module",
                );
            }
            let dict_bits = module_dict_bits(module_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = match dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            let annotations_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotations_name,
                b"__annotations__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(annotations_bits),
            ) {
                dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                let annotate_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotate_name,
                    b"__annotate__",
                );
                let none_bits = MoltObject::none().bits();
                dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                return MoltObject::none().bits();
            }
            let annotate_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotate_name,
                b"__annotate__",
            );
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(annotate_bits)) {
                let val_obj = obj_from_bits(val_bits);
                if !val_obj.is_none() {
                    let callable_ok = is_truthy(obj_from_bits(molt_is_callable(val_bits)));
                    if !callable_ok {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotate__ must be callable or None",
                        );
                    }
                }
                dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                if !val_obj.is_none() {
                    dict_del_in_place(_py, dict_ptr, annotations_bits);
                }
                return MoltObject::none().bits();
            }
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_module_import_star(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let src_obj = obj_from_bits(src_bits);
        let Some(src_ptr) = src_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module import expects module");
        };
        let dst_obj = obj_from_bits(dst_bits);
        let Some(dst_ptr) = dst_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "module import expects module");
        };
        unsafe {
            if object_type_id(src_ptr) != TYPE_ID_MODULE
                || object_type_id(dst_ptr) != TYPE_ID_MODULE
            {
                return raise_exception::<_>(_py, "TypeError", "module import expects module");
            }
            let src_dict_bits = module_dict_bits(src_ptr);
            let dst_dict_bits = module_dict_bits(dst_ptr);
            let src_dict_obj = obj_from_bits(src_dict_bits);
            let dst_dict_obj = obj_from_bits(dst_dict_bits);
            let src_dict_ptr = match src_dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            let dst_dict_ptr = match dst_dict_obj.as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => return raise_exception::<_>(_py, "TypeError", "module dict missing"),
            };
            let module_name =
                string_obj_to_owned(obj_from_bits(module_name_bits(src_ptr))).unwrap_or_default();
            let all_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.all_name, b"__all__");
            if let Some(all_bits) = dict_get_in_place(_py, src_dict_ptr, all_name_bits) {
                let iter_bits = molt_iter(all_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec_ref(pair_ptr);
                    if elems.len() < 2 {
                        return MoltObject::none().bits();
                    }
                    let done_bits = elems[1];
                    if is_truthy(obj_from_bits(done_bits)) {
                        break;
                    }
                    let name_bits = elems[0];
                    let name_obj = obj_from_bits(name_bits);
                    if let Some(name_ptr) = name_obj.as_ptr() {
                        if object_type_id(name_ptr) != TYPE_ID_STRING {
                            let type_name = class_name_for_error(type_of_bits(_py, name_bits));
                            let msg = format!(
                                "Item in {module_name}.__all__ must be str, not {type_name}"
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    } else {
                        let type_name = class_name_for_error(type_of_bits(_py, name_bits));
                        let msg =
                            format!("Item in {module_name}.__all__ must be str, not {type_name}");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    let Some(val_bits) = dict_get_in_place(_py, src_dict_ptr, name_bits) else {
                        let name =
                            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                        let msg = format!("module '{module_name}' has no attribute '{name}'");
                        return raise_exception::<_>(_py, "AttributeError", &msg);
                    };
                    dict_set_in_place(_py, dst_dict_ptr, name_bits, val_bits);
                }
                return MoltObject::none().bits();
            }

            let order = dict_order(src_dict_ptr);
            for idx in (0..order.len()).step_by(2) {
                let name_bits = order[idx];
                let name_obj = obj_from_bits(name_bits);
                let Some(name_ptr) = name_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(name_ptr) != TYPE_ID_STRING {
                    continue;
                }
                let name_len = string_len(name_ptr);
                if name_len > 0 {
                    let name_bytes = std::slice::from_raw_parts(string_bytes(name_ptr), name_len);
                    if name_bytes[0] == b'_' {
                        continue;
                    }
                }
                let val_bits = order[idx + 1];
                dict_set_in_place(_py, dst_dict_ptr, name_bits, val_bits);
            }
        }
        MoltObject::none().bits()
    })
}
