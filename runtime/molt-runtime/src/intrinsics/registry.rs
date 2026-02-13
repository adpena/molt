use crate::intrinsics::generated::{INTRINSICS, resolve_symbol};
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_MODULE, alloc_dict_with_pairs, alloc_function_obj,
    alloc_string, builtin_classes, dec_ref_bits, dict_get_in_place, dict_set_in_place,
    function_set_trampoline_ptr, inc_ref_bits, module_dict_bits, obj_from_bits,
    object_set_class_bits, object_type_id,
};

const REGISTRY_NAME: &str = "_molt_intrinsics";
const STRICT_FLAG: &str = "_molt_intrinsics_strict";
const RUNTIME_FLAG: &str = "_molt_runtime";

pub(crate) fn install_into_builtins(_py: &PyToken<'_>, module_ptr: *mut u8) {
    if module_ptr.is_null() {
        return;
    }
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return;
        }
    }
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    let dict_ptr = match obj_from_bits(dict_bits).as_ptr() {
        Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
        _ => return,
    };

    if registry_installed(_py, dict_ptr) {
        return;
    }

    let registry_ptr = alloc_dict_with_pairs(_py, &[]);
    if registry_ptr.is_null() {
        return;
    }
    let registry_bits = MoltObject::from_ptr(registry_ptr).bits();
    if !set_dict_entry(_py, dict_ptr, REGISTRY_NAME, registry_bits) {
        dec_ref_bits(_py, registry_bits);
        return;
    }
    set_dict_bool(_py, dict_ptr, STRICT_FLAG, true);
    set_dict_bool(_py, dict_ptr, RUNTIME_FLAG, true);

    for spec in INTRINSICS {
        let Some(fn_ptr) = resolve_symbol(spec.symbol) else {
            panic!("intrinsics registry missing symbol: {}", spec.symbol);
        };
        let Some(func_bits) = build_intrinsic_func(_py, fn_ptr, spec.arity) else {
            continue;
        };
        let mut registered = false;
        if set_intrinsic_entry(_py, dict_ptr, registry_ptr, spec.name, func_bits) {
            registered = true;
        }
        if let Some(alias) = alias_name(spec.name) {
            if set_intrinsic_entry(_py, dict_ptr, registry_ptr, &alias, func_bits) {
                registered = true;
            }
        }
        if registered {
            dec_ref_bits(_py, func_bits);
        }
    }
    dec_ref_bits(_py, registry_bits);
}

fn registry_installed(_py: &PyToken<'_>, dict_ptr: *mut u8) -> bool {
    let key_ptr = alloc_string(_py, REGISTRY_NAME.as_bytes());
    if key_ptr.is_null() {
        return false;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let existing = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    let Some(bits) = existing else {
        return false;
    };
    match obj_from_bits(bits).as_ptr() {
        Some(ptr) => unsafe { object_type_id(ptr) == TYPE_ID_DICT },
        None => false,
    }
}

fn set_dict_entry(_py: &PyToken<'_>, dict_ptr: *mut u8, name: &str, value_bits: u64) -> bool {
    let key_ptr = alloc_string(_py, name.as_bytes());
    if key_ptr.is_null() {
        return false;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    unsafe {
        dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
    }
    dec_ref_bits(_py, key_bits);
    true
}

fn set_dict_bool(_py: &PyToken<'_>, dict_ptr: *mut u8, name: &str, value: bool) {
    let key_ptr = alloc_string(_py, name.as_bytes());
    if key_ptr.is_null() {
        return;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let val_bits = MoltObject::from_bool(value).bits();
    unsafe {
        dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
    }
    dec_ref_bits(_py, key_bits);
}

fn set_intrinsic_entry(
    _py: &PyToken<'_>,
    module_dict_ptr: *mut u8,
    registry_ptr: *mut u8,
    name: &str,
    func_bits: u64,
) -> bool {
    let key_ptr = alloc_string(_py, name.as_bytes());
    if key_ptr.is_null() {
        return false;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    unsafe {
        dict_set_in_place(_py, module_dict_ptr, key_bits, func_bits);
        dict_set_in_place(_py, registry_ptr, key_bits, func_bits);
    }
    dec_ref_bits(_py, key_bits);
    true
}

fn alias_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("molt_")?;
    if rest.is_empty() {
        return None;
    }
    // Avoid `format!` here to keep wasm startup free of fmt call_indirect traffic.
    let mut alias = String::with_capacity(6 + rest.len());
    alias.push_str("_molt_");
    alias.push_str(rest);
    Some(alias)
}

fn build_intrinsic_func(_py: &PyToken<'_>, fn_ptr: u64, arity: u8) -> Option<u64> {
    let ptr = alloc_function_obj(_py, fn_ptr, arity as u64);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        function_set_trampoline_ptr(ptr, 0);
        let builtin_bits = builtin_classes(_py).builtin_function_or_method;
        object_set_class_bits(_py, ptr, builtin_bits);
        inc_ref_bits(_py, builtin_bits);
    }
    Some(MoltObject::from_ptr(ptr).bits())
}
