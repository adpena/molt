use crate::intrinsics::generated::{INTRINSICS, resolve_symbol};
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_MODULE, TYPE_ID_STRING,
    alloc_dict_with_pairs, alloc_string, builtin_classes, dec_ref_bits, dict_get_in_place,
    dict_set_in_place, inc_ref_bits, module_dict_bits, obj_from_bits,
    object_set_class_bits, object_type_id, raise_exception, string_bytes, string_len,
};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

const REGISTRY_NAME: &str = "_molt_intrinsics";
const LOOKUP_HELPER_NAME: &str = "_molt_intrinsic_lookup";
const STRICT_FLAG: &str = "_molt_intrinsics_strict";
const RUNTIME_FLAG: &str = "_molt_runtime";

/// Pointer to the builtins module, stored so the lazy resolver can locate the
/// intrinsics registry dict without re-traversing the module hierarchy.
/// Uses `AtomicPtr` so that concurrent calls on native multi-threaded targets
/// cannot race on the null check (on wasm32 single-threaded atomics are no-ops).
static BUILTINS_MODULE_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

/// Per-app intrinsic manifest for WASM tree shaking.
static INTRINSIC_MANIFEST_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static INTRINSIC_MANIFEST_LEN: AtomicU32 = AtomicU32::new(0);

/// One-shot guard: only the first call (compiler-generated bootstrap) takes
/// effect.  Uses compare_exchange to eliminate the TOCTOU race between
/// load and store that a plain load+store pair would have on native
/// multi-threaded targets.
static MANIFEST_SET: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intrinsic_manifest(ptr: u64, len: u64) -> u64 {
    // Claim the one-shot slot FIRST.  Only the winner stores PTR/LEN.
    // This avoids the race where a loser's pre-CAS stores clobber the
    // winner's values before the loser discovers it lost.
    if MANIFEST_SET
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return 0; // another caller already set the manifest
    }
    // Winner: store PTR and LEN.  The Release edge from the CAS above
    // ensures any reader that observes MANIFEST_SET=true will also see
    // these stores (via an Acquire load on MANIFEST_SET).
    INTRINSIC_MANIFEST_PTR.store(ptr as u32 as *mut u8, Ordering::Release);
    INTRINSIC_MANIFEST_LEN.store(len as u32, Ordering::Release);
    0
}

#[cfg(target_arch = "wasm32")]
fn parse_manifest() -> Option<std::collections::BTreeSet<&'static str>> {
    // Gate on MANIFEST_SET (Acquire) to ensure we see the winner's
    // PTR/LEN stores that were published with Release ordering.
    if !MANIFEST_SET.load(Ordering::Acquire) {
        return None;
    }
    let ptr = INTRINSIC_MANIFEST_PTR.load(Ordering::Acquire);
    let len = INTRINSIC_MANIFEST_LEN.load(Ordering::Acquire) as usize;
    if ptr.is_null() || len == 0 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    let mut set = std::collections::BTreeSet::new();
    for chunk in bytes.split(|&b| b == 0) {
        if let Ok(name) = core::str::from_utf8(chunk) {
            if !name.is_empty() {
                set.insert(name);
            }
        }
    }
    Some(set)
}

pub(crate) fn install_into_builtins(_py: &PyToken<'_>, module_ptr: *mut u8) {
    if module_ptr.is_null() {
        return;
    }
    // Install an __intrinsics__ registry dict into the module so the lazy
    // resolver can cache intrinsic function objects.  The `registry_installed`
    // check prevents double-installation on re-entry.
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

    // Store builtins module pointer for the lazy resolver (native only).
    // AtomicPtr ensures thread safety on native multi-threaded targets.
    //
    // Only set BUILTINS_MODULE_PTR once.  Previous code swapped it on every
    // molt_module_new, which dec-ref'd the prior module.  On native builds the
    // dec-ref cascaded into a use-after-free: the "builtins" module was freed
    // when the next module (e.g. _sitebuiltins) overwrote the pointer, but
    // the module cache still held (now-dangling) bits for "builtins", causing
    // "module attribute access expects module, got type_id=..." on every
    // subsequent MODULE_GET_ATTR.
    //
    // The lazy resolver only needs *some* module's __intrinsics__ registry
    // dict to cache resolved intrinsics; it does not matter which module.
    // Locking to the first one avoids the refcount imbalance entirely.
    let prev = BUILTINS_MODULE_PTR.load(Ordering::Acquire);
    if prev.is_null() {
        BUILTINS_MODULE_PTR.store(module_ptr, Ordering::Release);
        inc_ref_bits(_py, MoltObject::from_ptr(module_ptr).bits());
    }

    // On wasm32, `call_indirect` with lazily-resolved function pointers
    // causes "out of bounds table access" traps because the indirect
    // function table indices become invalid after wasm-ld linking.
    // Use eager registration on wasm32 for correctness; lazy on native
    // for the cold-start performance benefit (~7100 fewer allocations).
    #[cfg(not(target_arch = "wasm32"))]
    {
        let resolver_fn_ptr = molt_intrinsic_resolve as *const () as usize as u64;
        if let Some(helper_bits) = build_intrinsic_func(_py, resolver_fn_ptr, 1) {
            set_dict_entry(_py, dict_ptr, LOOKUP_HELPER_NAME, helper_bits);
            dec_ref_bits(_py, helper_bits);
        }
        if let Some(resolver_bits) = build_intrinsic_func(_py, resolver_fn_ptr, 1) {
            set_intrinsic_entry(_py, registry_ptr, "_molt_lazy_resolve", resolver_bits);
            dec_ref_bits(_py, resolver_bits);
        }
    }

    // On WASM with a manifest, eagerly register only the referenced
    // intrinsics.  The manifest already filters to only the functions the
    // compiled module uses, so this is safe and enables dead stripping.
    // On native, skip eager registration — the lazy resolver handles it.
    #[cfg(target_arch = "wasm32")]
    {
        let manifest = parse_manifest();
        if let Some(ref m) = manifest {
            for spec in INTRINSICS {
                if !m.contains(spec.name) {
                    continue;
                }
                let Some(fn_ptr) = resolve_symbol(spec.symbol) else {
                    continue;
                };
                let Some(func_bits) = build_intrinsic_func(_py, fn_ptr, spec.arity) else {
                    continue;
                };
                set_intrinsic_entry(_py, registry_ptr, spec.name, func_bits);
                if let Some(alias) = alias_name(spec.name) {
                    set_intrinsic_entry(_py, registry_ptr, &alias, func_bits);
                }
                dec_ref_bits(_py, func_bits);
            }
        }
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
        let trace = matches!(
            std::env::var("MOLT_TRACE_REQUIRE_INTRINSIC")
                .ok()
                .as_deref(),
            Some("1")
        );
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            if trace {
                eprintln!("molt intrinsic_resolve: non-pointer arg bits=0x{name_bits:x}");
            }
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                if trace {
                    eprintln!(
                        "molt intrinsic_resolve: arg type={} bits=0x{name_bits:x}",
                        crate::type_name(_py, name_obj),
                    );
                }
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
        let resolved = resolve_intrinsic_func(_py, name_str, true);
        if trace {
            eprintln!(
                "molt intrinsic_resolve: name={} status={:?}",
                name_str, resolved
            );
        }
        resolved.unwrap_or_else(|_| MoltObject::none().bits())
    })
}

/// Python builtin name -> intrinsic name mapping for builtins that have
/// non-standard intrinsic names (e.g. `globals` -> `molt_globals_builtin`).
static PYTHON_BUILTIN_ALIASES: &[(&str, &str)] = &[("globals", "molt_globals_builtin")];

fn find_spec_by_name(name: &str) -> Option<&'static crate::intrinsics::generated::IntrinsicSpec> {
    INTRINSICS.iter().find(|spec| spec.name == name)
}

/// Find an `IntrinsicSpec` by primary name or `_molt_` alias.
fn find_spec(name: &str) -> Option<&'static crate::intrinsics::generated::IntrinsicSpec> {
    // Try primary name first.
    if let Some(spec) = find_spec_by_name(name) {
        return Some(spec);
    }
    // Try alias: `_molt_foo` -> `molt_foo`.
    if let Some(rest) = name.strip_prefix("_molt_") {
        let primary = {
            let mut s = String::with_capacity(5 + rest.len());
            s.push_str("molt_");
            s.push_str(rest);
            s
        };
        if let Some(spec) = find_spec_by_name(&primary) {
            return Some(spec);
        }
    }
    // Try generic Python builtin spellings first as `molt_<name>` and then
    // `molt_<name>_builtin`. This keeps compiler-generated builtin calls off
    // the fragile builtins-module bootstrap path when a direct runtime
    // intrinsic exists.
    if !name.starts_with("molt_") {
        let prefixed = format!("molt_{name}");
        if let Some(spec) = find_spec_by_name(&prefixed) {
            return Some(spec);
        }
        let builtin = format!("molt_{name}_builtin");
        if let Some(spec) = find_spec_by_name(&builtin) {
            return Some(spec);
        }
    }
    // Try Python builtin aliases (e.g. `globals` -> `molt_globals_builtin`).
    for &(py_name, intrinsic_name) in PYTHON_BUILTIN_ALIASES {
        if name == py_name {
            if let Some(spec) = find_spec_by_name(intrinsic_name) {
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

fn build_runtime_function(
    _py: &PyToken<'_>,
    fn_ptr: u64,
    arity: u8,
) -> Option<u64> {
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity as u64);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let builtin_bits = builtin_classes(_py).builtin_function_or_method;
        object_set_class_bits(_py, ptr, builtin_bits);
        inc_ref_bits(_py, builtin_bits);
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

fn build_bootstrap_function(
    _py: &PyToken<'_>,
    fn_ptr: u64,
    arity: u8,
    defaults: &[u64],
) -> Option<u64> {
    let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity as u64);
    if ptr.is_null() {
        return None;
    }
    if !defaults.is_empty() {
        unsafe {
            let defaults_name = crate::intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.defaults_name,
                b"__defaults__",
            );
            let defaults_ptr = crate::alloc_tuple(_py, defaults);
            if !defaults_ptr.is_null() {
                let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
                crate::function_set_attr_bits(_py, ptr, defaults_name, defaults_bits);
            }
        }
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

fn build_intrinsic_func(_py: &PyToken<'_>, fn_ptr: u64, arity: u8) -> Option<u64> {
    build_runtime_function(_py, fn_ptr, arity)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntrinsicResolveError {
    Unknown,
    MissingSymbol,
    AllocFailed,
}

fn cache_resolved_intrinsic(
    _py: &PyToken<'_>,
    requested_name: &str,
    canonical_name: &str,
    func_bits: u64,
) {
    let builtins_ptr = BUILTINS_MODULE_PTR.load(Ordering::Acquire);
    if builtins_ptr.is_null() {
        return;
    }
    let dict_bits = unsafe { module_dict_bits(builtins_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    let reg_key_ptr = alloc_string(_py, REGISTRY_NAME.as_bytes());
    if reg_key_ptr.is_null() {
        return;
    }
    let reg_key_bits = MoltObject::from_ptr(reg_key_ptr).bits();
    let reg_opt = unsafe { dict_get_in_place(_py, dict_ptr, reg_key_bits) };
    dec_ref_bits(_py, reg_key_bits);
    let Some(reg_bits) = reg_opt else {
        return;
    };
    let Some(registry_ptr) = obj_from_bits(reg_bits).as_ptr() else {
        return;
    };
    set_intrinsic_entry(_py, registry_ptr, canonical_name, func_bits);
    if requested_name != canonical_name {
        set_intrinsic_entry(_py, registry_ptr, requested_name, func_bits);
    }
    if let Some(alias) = alias_name(canonical_name) {
        set_intrinsic_entry(_py, registry_ptr, &alias, func_bits);
    }
}

fn resolve_intrinsic_func(
    _py: &PyToken<'_>,
    requested_name: &str,
    cache_result: bool,
) -> Result<u64, IntrinsicResolveError> {
    let Some(spec) = find_spec(requested_name) else {
        return Err(IntrinsicResolveError::Unknown);
    };
    let Some(fn_ptr) = resolve_symbol(spec.symbol) else {
        return Err(IntrinsicResolveError::MissingSymbol);
    };
    let Some(func_bits) = build_intrinsic_func(_py, fn_ptr, spec.arity) else {
        return Err(IntrinsicResolveError::AllocFailed);
    };
    if cache_result {
        cache_resolved_intrinsic(_py, requested_name, spec.name, func_bits);
    }
    Ok(func_bits)
}

pub(crate) fn try_resolve_intrinsic_func(
    _py: &PyToken<'_>,
    requested_name: &str,
    cache_result: bool,
) -> Option<u64> {
    resolve_intrinsic_func(_py, requested_name, cache_result).ok()
}

/// Register a synthetic `_intrinsics` module in the module cache so that
/// stdlib Python files can `from _intrinsics import require_intrinsic`.
/// The module contains a `require_intrinsic` function that delegates to
/// the runtime's intrinsic lookup.
pub(crate) fn register_intrinsics_module(_py: &PyToken<'_>) {
    use crate::object::builders::alloc_module_obj;
    use crate::{alloc_string, dict_set_in_place, module_dict_bits};

    // Create the _intrinsics module
    let name_ptr = alloc_string(_py, b"_intrinsics");
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();

    let module_ptr = alloc_module_obj(_py, name_bits);
    if module_ptr.is_null() {
        dec_ref_bits(_py, name_bits);
        return;
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();

    // Add require_intrinsic function to the module dict
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        // Avoid builtin_classes() here: register_intrinsics_module runs during
        // bootstrap while init_builtin_classes still holds its mutex.
        let none = MoltObject::none().bits();
        if let Some(fn_bits) = build_bootstrap_function(
            _py,
            molt_require_intrinsic_runtime as *const () as usize as u64,
            2,
            &[none],
        ) {
            let key_ptr = alloc_string(_py, b"require_intrinsic");
            if !key_ptr.is_null() {
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                unsafe {
                    dict_set_in_place(_py, dict_ptr, key_bits, fn_bits);
                }
                dec_ref_bits(_py, key_bits);
            }
            dec_ref_bits(_py, fn_bits);
        }
    }

    // Register in module cache
    crate::builtins::modules::molt_module_cache_set(name_bits, module_bits);
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
}

/// Reset all one-shot globals in the intrinsic registry.
///
/// Called by `molt_runtime_reset_for_testing` to clear dangling pointers
/// after runtime shutdown.  Without this, `BUILTINS_MODULE_PTR` holds a
/// dangling pointer to the destroyed builtins module and `MANIFEST_SET`
/// prevents re-setting the manifest on the next init.
#[cfg(test)]
pub(crate) fn reset_for_testing() {
    MANIFEST_SET.store(false, Ordering::SeqCst);
    INTRINSIC_MANIFEST_PTR.store(core::ptr::null_mut(), Ordering::SeqCst);
    INTRINSIC_MANIFEST_LEN.store(0, Ordering::SeqCst);
    BUILTINS_MODULE_PTR.store(core::ptr::null_mut(), Ordering::SeqCst);
}

// Expose internals for testing.
#[cfg(test)]
pub(crate) fn test_manifest_set() -> &'static AtomicBool {
    &MANIFEST_SET
}
#[cfg(test)]
pub(crate) fn test_manifest_ptr() -> &'static AtomicPtr<u8> {
    &INTRINSIC_MANIFEST_PTR
}

/// Runtime implementation of require_intrinsic(name, namespace=None) -> function.
///
/// The optional namespace is accepted for API compatibility with
/// `src/molt/stdlib/_intrinsics.py`; resolution is runtime-global today.
#[unsafe(no_mangle)]
pub extern "C" fn molt_require_intrinsic_runtime(name_bits: u64, namespace_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = namespace_bits;
        let trace = matches!(
            std::env::var("MOLT_TRACE_REQUIRE_INTRINSIC")
                .ok()
                .as_deref(),
            Some("1")
        );
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            if trace {
                eprintln!("molt require_intrinsic: non-pointer arg bits=0x{name_bits:x}");
            }
            return raise_exception::<u64>(_py, "TypeError", "intrinsic name must be str");
        };
        let name = unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return if trace {
                    eprintln!(
                        "molt require_intrinsic: arg type={} bits=0x{name_bits:x}",
                        crate::type_name(_py, name_obj),
                    );
                    raise_exception::<u64>(_py, "TypeError", "intrinsic name must be str")
                } else {
                    raise_exception::<u64>(_py, "TypeError", "intrinsic name must be str")
                };
            }
            let len = string_len(name_ptr);
            let bytes = std::slice::from_raw_parts(string_bytes(name_ptr), len);
            std::str::from_utf8(bytes).unwrap_or("")
        };
        if trace {
            let resolved = find_spec(name).and_then(|spec| resolve_symbol(spec.symbol));
            eprintln!(
                "molt require_intrinsic: name={} resolved={}",
                name,
                resolved
                    .map(|addr| format!("0x{addr:x}"))
                    .unwrap_or_else(|| "<none>".to_string())
            );
        }
        match resolve_intrinsic_func(_py, name, true) {
            Ok(func_bits) => {
                inc_ref_bits(_py, name_bits);
                func_bits
            }
            Err(IntrinsicResolveError::AllocFailed) => {
                if trace {
                    eprintln!(
                        "molt require_intrinsic: alloc_function_obj failed for {}",
                        name
                    );
                }
                raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    &format!("failed to allocate intrinsic function: {name}"),
                )
            }
            Err(IntrinsicResolveError::Unknown | IntrinsicResolveError::MissingSymbol) => {
                raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("intrinsic unavailable: {name}"),
                )
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering;

    /// Test one-shot manifest guard: first call sets, second is ignored.
    ///
    /// Because MANIFEST_SET is a process-global static, this test must run
    /// in a single function to avoid ordering issues with parallel test
    /// execution.  We reset the flag at the start (safe in test-only code)
    /// so the test is idempotent even if other tests ran first.
    #[test]
    fn manifest_one_shot_guard() {
        // Reset globals so this test is self-contained.
        test_manifest_set().store(false, Ordering::SeqCst);
        test_manifest_ptr().store(core::ptr::null_mut(), Ordering::SeqCst);

        // First call should succeed and latch the guard.
        let ret = molt_set_intrinsic_manifest(0x1000, 10);
        assert_eq!(ret, 0, "first call should return 0 (success)");
        assert!(
            test_manifest_set().load(Ordering::SeqCst),
            "MANIFEST_SET should be true after first call"
        );
        assert_eq!(
            test_manifest_ptr().load(Ordering::SeqCst) as usize,
            0x1000,
            "INTRINSIC_MANIFEST_PTR should be 0x1000 after first call"
        );

        // Second call should be silently ignored.
        let ret2 = molt_set_intrinsic_manifest(0x2000, 20);
        assert_eq!(ret2, 0, "second call should also return 0");
        assert_eq!(
            test_manifest_ptr().load(Ordering::SeqCst) as usize,
            0x1000,
            "INTRINSIC_MANIFEST_PTR must still be 0x1000, NOT 0x2000"
        );
    }
}
