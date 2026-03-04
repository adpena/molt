use crate::intrinsics::generated::{INTRINSICS, resolve_symbol};
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_MODULE, alloc_dict_with_pairs, alloc_function_obj,
    alloc_string, builtin_classes, dec_ref_bits, dict_get_in_place, dict_set_in_place,
    function_set_trampoline_ptr, inc_ref_bits, module_dict_bits, obj_from_bits,
    object_set_class_bits, object_type_id,
};
use std::sync::atomic::{AtomicU64, Ordering};

const REGISTRY_NAME: &str = "_molt_intrinsics";
const STRICT_FLAG: &str = "_molt_intrinsics_strict";
const RUNTIME_FLAG: &str = "_molt_runtime";
const LOOKUP_HELPER_NAME: &str = "_molt_intrinsic_lookup";

static INTRINSICS_REGISTRY_BITS: AtomicU64 = AtomicU64::new(0);

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

    if let Some(existing_registry_bits) = registry_bits_in_builtins_dict(_py, dict_ptr) {
        INTRINSICS_REGISTRY_BITS.store(existing_registry_bits, Ordering::Release);
        install_lookup_helper(_py, dict_ptr);
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
    INTRINSICS_REGISTRY_BITS.store(registry_bits, Ordering::Release);
    install_lookup_helper(_py, dict_ptr);
    set_dict_bool(_py, dict_ptr, STRICT_FLAG, true);
    set_dict_bool(_py, dict_ptr, RUNTIME_FLAG, true);

    for spec in INTRINSICS {
        let Some(fn_ptr) = resolve_symbol(spec.symbol) else {
            panic!("intrinsics registry missing symbol: {}", spec.symbol);
        };
        let Some(func_bits) = build_intrinsic_func(_py, spec.name, fn_ptr, spec.arity) else {
            continue;
        };
        let mut registered = false;
        // Keep intrinsics confined to the private registry; they must not pollute
        // the public `builtins` API surface (CPython parity).
        if set_intrinsic_entry(_py, registry_ptr, spec.name, func_bits) {
            registered = true;
        }
        if let Some(alias) = alias_name(spec.name)
            && set_intrinsic_entry(_py, registry_ptr, &alias, func_bits)
        {
            registered = true;
        }
        if registered {
            dec_ref_bits(_py, func_bits);
        }
    }
    dec_ref_bits(_py, registry_bits);
}

fn registry_bits_in_builtins_dict(_py: &PyToken<'_>, dict_ptr: *mut u8) -> Option<u64> {
    let key_ptr = alloc_string(_py, REGISTRY_NAME.as_bytes());
    if key_ptr.is_null() {
        return None;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let existing = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    let bits = existing?;
    match obj_from_bits(bits).as_ptr() {
        Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Some(bits),
        _ => None,
    }
}

fn install_lookup_helper(_py: &PyToken<'_>, builtins_dict_ptr: *mut u8) {
    let helper_fn_ptr = molt_intrinsic_lookup as *const () as usize as u64;
    let Some(helper_bits) = build_intrinsic_func(_py, "molt_intrinsic_lookup", helper_fn_ptr, 1)
    else {
        return;
    };
    let _ = set_dict_entry(_py, builtins_dict_ptr, LOOKUP_HELPER_NAME, helper_bits);
    dec_ref_bits(_py, helper_bits);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_intrinsic_lookup(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let registry_bits = INTRINSICS_REGISTRY_BITS.load(Ordering::Acquire);
        if registry_bits == 0 {
            return MoltObject::none().bits();
        }
        let Some(registry_ptr) = obj_from_bits(registry_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if unsafe { object_type_id(registry_ptr) } != TYPE_ID_DICT {
            return MoltObject::none().bits();
        }
        let Some(value_bits) = (unsafe { dict_get_in_place(_py, registry_ptr, name_bits) }) else {
            return MoltObject::none().bits();
        };
        inc_ref_bits(_py, value_bits);
        value_bits
    })
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

fn build_intrinsic_func(_py: &PyToken<'_>, name: &str, fn_ptr: u64, arity: u8) -> Option<u64> {
    if cfg!(target_arch = "wasm32")
        && std::env::var("MOLT_WASM_INTRINSIC_DEBUG").as_deref() == Ok("1")
    {
        eprintln!("molt wasm intrinsic_new: name={name} fn=0x{fn_ptr:x} arity={arity}");
    }
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
