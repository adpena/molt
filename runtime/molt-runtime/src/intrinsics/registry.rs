use core::sync::atomic::{AtomicPtr, Ordering};
use crate::intrinsics::generated::{INTRINSICS, resolve_symbol};
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_MODULE, TYPE_ID_STRING, alloc_dict_with_pairs,
    alloc_function_obj, alloc_string, builtin_classes, dec_ref_bits,
    dict_get_in_place, dict_set_in_place, function_set_trampoline_ptr, inc_ref_bits,
    module_dict_bits, obj_from_bits, object_set_class_bits, object_type_id, string_bytes,
    string_len,
};

const REGISTRY_NAME: &str = "_molt_intrinsics";
const LOOKUP_HELPER_NAME: &str = "_molt_intrinsic_lookup";
const STRICT_FLAG: &str = "_molt_intrinsics_strict";
const RUNTIME_FLAG: &str = "_molt_runtime";

/// Pointer to the builtins module, stored so the lazy resolver can locate the
/// intrinsics registry dict without re-traversing the module hierarchy.
/// Uses `AtomicPtr` so that concurrent calls on native multi-threaded targets
/// cannot race on the null check (on wasm32 single-threaded atomics are no-ops).
static BUILTINS_MODULE_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

pub(crate) fn install_into_builtins(_py: &PyToken<'_>, module_ptr: *mut u8) {
    if module_ptr.is_null() {
        return;
    }
    // Only install the intrinsics registry once — into the first module
    // created (the builtins module).  Previously this ran for every module,
    // which overwrote BUILTINS_MODULE_PTR with the *last* module created
    // and left the lazy resolver unreachable from the actual builtins dict
    // that Python-side `_intrinsics.py` looks up via `__builtins__`.
    if !BUILTINS_MODULE_PTR.load(Ordering::Relaxed).is_null() {
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

    // Store builtins module pointer for the lazy resolver.
    // AtomicPtr ensures thread safety on native multi-threaded targets.
    inc_ref_bits(_py, MoltObject::from_ptr(module_ptr).bits());
    BUILTINS_MODULE_PTR.store(module_ptr, Ordering::Release);

    // Lazy intrinsics: do NOT eagerly build function objects for all 2377
    // intrinsics.  Instead, resolve on demand in `molt_intrinsic_resolve`.
    // This saves ~7100 allocations (3 per intrinsic) during _start, reducing
    // cold-start bootstrap time on Cloudflare Workers from ~8ms to <2ms.
    //
    // Install *only* the resolver itself into the registry so Python code can
    // call it as a fallback when a dict lookup misses.
    let resolver_fn_ptr = molt_intrinsic_resolve as *const () as usize as u64;
    if let Some(helper_bits) = build_intrinsic_func(_py, resolver_fn_ptr, 1) {
        set_dict_entry(_py, dict_ptr, LOOKUP_HELPER_NAME, helper_bits);
        dec_ref_bits(_py, helper_bits);
    }
    if let Some(resolver_bits) = build_intrinsic_func(_py, resolver_fn_ptr, 1) {
        set_intrinsic_entry(_py, registry_ptr, "_molt_lazy_resolve", resolver_bits);
        dec_ref_bits(_py, resolver_bits);
    }

    dec_ref_bits(_py, registry_bits);
}

/// Lazily resolve a single intrinsic by name, build the function object,
/// cache it in the registry dict, and return the function bits.
///
/// Called from Python-side `_intrinsics.py` as a fallback when a dict
/// lookup on the registry misses.
#[unsafe(no_mangle)]
pub extern "C" fn molt_intrinsic_resolve(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
        }

        // Extract the name as a &str.
        let name_str = unsafe {
            let len = string_len(name_ptr);
            let bytes = core::slice::from_raw_parts(string_bytes(name_ptr), len);
            match core::str::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => return MoltObject::none().bits(),
            }
        };

        // Look up the spec by name (primary name or alias).
        let spec = find_spec(name_str);
        let Some(spec) = spec else {
            return MoltObject::none().bits();
        };

        let Some(fn_ptr) = resolve_symbol(spec.symbol) else {
            return MoltObject::none().bits();
        };

        let Some(func_bits) = build_intrinsic_func(_py, fn_ptr, spec.arity) else {
            return MoltObject::none().bits();
        };

        // Cache in the registry dict so subsequent lookups hit the fast path.
        let builtins_ptr = BUILTINS_MODULE_PTR.load(Ordering::Acquire);
        if !builtins_ptr.is_null() {
            let dict_bits = unsafe { module_dict_bits(builtins_ptr) };
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                let reg_key_ptr = alloc_string(_py, REGISTRY_NAME.as_bytes());
                if !reg_key_ptr.is_null() {
                    let reg_key_bits = MoltObject::from_ptr(reg_key_ptr).bits();
                    let reg_opt = unsafe { dict_get_in_place(_py, dict_ptr, reg_key_bits) };
                    dec_ref_bits(_py, reg_key_bits);
                    if let Some(reg_bits) = reg_opt {
                        if let Some(registry_ptr) = obj_from_bits(reg_bits).as_ptr() {
                            // Cache primary name
                            set_intrinsic_entry(_py, registry_ptr, name_str, func_bits);
                            // Also cache alias if applicable
                            if let Some(alias) = alias_name(name_str) {
                                set_intrinsic_entry(_py, registry_ptr, &alias, func_bits);
                            }
                        }
                    }
                }
            }
        }

        // The caller takes ownership; the dict also holds a ref via
        // set_intrinsic_entry, so we do NOT dec_ref here.
        func_bits
    })
}

/// Find an `IntrinsicSpec` by primary name or `_molt_` alias.
fn find_spec(name: &str) -> Option<&'static crate::intrinsics::generated::IntrinsicSpec> {
    // Try primary name first.
    for spec in INTRINSICS {
        if spec.name == name {
            return Some(spec);
        }
    }
    // Try alias: `_molt_foo` -> `molt_foo`.
    if let Some(rest) = name.strip_prefix("_molt_") {
        let primary = {
            let mut s = String::with_capacity(5 + rest.len());
            s.push_str("molt_");
            s.push_str(rest);
            s
        };
        for spec in INTRINSICS {
            if spec.name == primary {
                return Some(spec);
            }
        }
    }
    None
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
