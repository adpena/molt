use crate::intrinsics::generated::{INTRINSICS, resolve_symbol};
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_MODULE, TYPE_ID_STRING, alloc_dict_with_pairs,
    alloc_string, builtin_classes, dec_ref_bits, dict_get_in_place, dict_set_in_place,
    inc_ref_bits, module_dict_bits, obj_from_bits, object_set_class_bits, object_type_id,
    raise_exception, runtime_state, string_bytes, string_len,
};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

const REGISTRY_NAME: &str = "_molt_intrinsics";
const LOOKUP_HELPER_NAME: &str = "_molt_intrinsic_lookup";
const STRICT_FLAG: &str = "_molt_intrinsics_strict";
const RUNTIME_FLAG: &str = "_molt_runtime";

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

    // Store a runtime-owned module pointer for the lazy resolver.  This must
    // live in RuntimeState, not a process-global, because tests and embedders
    // can tear down and re-initialize runtime state in-process.
    //
    // Only set the anchor once per RuntimeState.  Previous code swapped it on
    // every molt_module_new, which dec-ref'd the prior module.  On native
    // builds the dec-ref cascaded into a use-after-free: the "builtins" module
    // was freed when the next module (e.g. _sitebuiltins) overwrote the pointer,
    // but the module cache still held (now-dangling) bits for "builtins".
    let registry_module = &runtime_state(_py).intrinsic_registry_module;
    let prev = registry_module.load(Ordering::Acquire);
    if prev.is_null() {
        registry_module.store(module_ptr, Ordering::Release);
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
    crate::with_gil_entry_nopanic!(_py, {
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
        if name == py_name
            && let Some(spec) = find_spec_by_name(intrinsic_name)
        {
            return Some(spec);
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

fn register_bootstrap_callable(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    export_name: &[u8],
    fn_ptr: u64,
    arity: u8,
    defaults: &[u64],
) {
    let Some(fn_bits) = build_bootstrap_function(_py, fn_ptr, arity, defaults) else {
        return;
    };
    let key_ptr = alloc_string(_py, export_name);
    if !key_ptr.is_null() {
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        unsafe {
            dict_set_in_place(_py, dict_ptr, key_bits, fn_bits);
        }
        dec_ref_bits(_py, key_bits);
    }
    dec_ref_bits(_py, fn_bits);
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

fn build_runtime_function(_py: &PyToken<'_>, fn_ptr: u64, arity: u8) -> Option<u64> {
    let _nursery_guard = crate::object::NurserySuspendGuard::new();
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
    let _nursery_guard = crate::object::NurserySuspendGuard::new();
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
                dec_ref_bits(_py, defaults_bits);
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
    let builtins_ptr = runtime_state(_py)
        .intrinsic_registry_module
        .load(Ordering::Acquire);
    if builtins_ptr.is_null() {
        return;
    }
    unsafe {
        if object_type_id(builtins_ptr) != TYPE_ID_MODULE {
            return;
        }
    }
    let dict_bits = unsafe { module_dict_bits(builtins_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let reg_key_ptr = alloc_string(_py, REGISTRY_NAME.as_bytes());
    if reg_key_ptr.is_null() {
        return;
    };
    let reg_key_bits = MoltObject::from_ptr(reg_key_ptr).bits();
    let reg_opt = unsafe { dict_get_in_place(_py, dict_ptr, reg_key_bits) };
    dec_ref_bits(_py, reg_key_bits);
    let Some(reg_bits) = reg_opt else {
        return;
    };
    let Some(registry_ptr) = obj_from_bits(reg_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(registry_ptr) != TYPE_ID_DICT {
            return;
        }
    }
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
    use crate::{alloc_string, module_dict_bits};

    #[cfg(target_arch = "wasm32")]
    {
        // WASM builds ship the compiled stdlib `_intrinsics` module in the
        // application artifact. Prefer that canonical Python module over the
        // legacy synthetic cache entry so direct-link hosts do not depend on
        // bootstrap-only wrapper function semantics.
        return;
    }

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

    // Mirror the public helpers exposed by src/_intrinsics.py so module-form
    // imports (`import _intrinsics as mod`) and from-imports see the same API.
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        // Avoid builtin_classes() here: register_intrinsics_module runs during
        // bootstrap while init_builtin_classes still holds its mutex.
        let none = MoltObject::none().bits();
        register_bootstrap_callable(
            _py,
            dict_ptr,
            b"require_intrinsic",
            molt_require_intrinsic_runtime as *const () as usize as u64,
            2u8,
            &[none],
        );
        register_bootstrap_callable(
            _py,
            dict_ptr,
            b"load_intrinsic",
            molt_load_intrinsic_runtime as *const () as usize as u64,
            2u8,
            &[none],
        );
        register_bootstrap_callable(
            _py,
            dict_ptr,
            b"runtime_active",
            molt_runtime_active_runtime as *const () as usize as u64,
            0u8,
            &[],
        );
    }

    // Register in module cache
    crate::builtins::modules::molt_module_cache_set(name_bits, module_bits);
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
}

/// Reset process-wide one-shot globals in the intrinsic registry.
///
/// Called by `molt_runtime_reset_for_testing` after runtime shutdown so a
/// same-process re-init can install a fresh manifest. The module/cache anchor
/// is stored on `RuntimeState`, so dropping the state is sufficient to clear it.
#[cfg(test)]
pub(crate) fn reset_for_testing() {
    MANIFEST_SET.store(false, Ordering::SeqCst);
    INTRINSIC_MANIFEST_PTR.store(core::ptr::null_mut(), Ordering::SeqCst);
    INTRINSIC_MANIFEST_LEN.store(0, Ordering::SeqCst);
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
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_load_intrinsic_runtime(name_bits: u64, namespace_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = namespace_bits;
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "intrinsic name must be str");
        };
        let name = unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<u64>(_py, "TypeError", "intrinsic name must be str");
            }
            let len = string_len(name_ptr);
            let bytes = std::slice::from_raw_parts(string_bytes(name_ptr), len);
            std::str::from_utf8(bytes).unwrap_or("")
        };
        match resolve_intrinsic_func(_py, name, true) {
            Ok(func_bits) => {
                inc_ref_bits(_py, name_bits);
                func_bits
            }
            Err(IntrinsicResolveError::Unknown | IntrinsicResolveError::MissingSymbol) => {
                MoltObject::none().bits()
            }
            Err(IntrinsicResolveError::AllocFailed) => raise_exception::<u64>(
                _py,
                "MemoryError",
                &format!("failed to allocate intrinsic function: {name}"),
            ),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_active_runtime() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering;

    #[test]
    fn register_intrinsics_module_exports_public_helpers() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            register_intrinsics_module(_py);

            let module_name_ptr = alloc_string(_py, b"_intrinsics");
            assert!(!module_name_ptr.is_null());
            let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
            let module_bits = crate::builtins::modules::molt_module_cache_get(module_name_bits);
            let module_ptr = obj_from_bits(module_bits)
                .as_ptr()
                .expect("_intrinsics module should exist");
            assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);

            let runtime_active_name_ptr = alloc_string(_py, b"runtime_active");
            let runtime_active_name_bits = MoltObject::from_ptr(runtime_active_name_ptr).bits();
            let runtime_active_bits =
                crate::molt_get_attr_name(module_bits, runtime_active_name_bits);
            assert!(obj_from_bits(runtime_active_bits).as_ptr().is_some());
            let runtime_active_out = molt_runtime_active_runtime();
            assert!(crate::is_truthy(_py, obj_from_bits(runtime_active_out)));

            let load_name_ptr = alloc_string(_py, b"load_intrinsic");
            let load_name_bits = MoltObject::from_ptr(load_name_ptr).bits();
            let load_bits = crate::molt_get_attr_name(module_bits, load_name_bits);
            assert!(obj_from_bits(load_bits).as_ptr().is_some());
            let intrinsic_name_ptr = alloc_string(_py, b"molt_gpu_buffer_to_list");
            let intrinsic_name_bits = MoltObject::from_ptr(intrinsic_name_ptr).bits();
            let resolved_bits =
                molt_load_intrinsic_runtime(intrinsic_name_bits, MoltObject::none().bits());
            let resolved_ptr = obj_from_bits(resolved_bits)
                .as_ptr()
                .expect("load_intrinsic should resolve known intrinsics");
            assert_eq!(
                unsafe { object_type_id(resolved_ptr) },
                crate::TYPE_ID_FUNCTION
            );

            let split_name_ptr =
                alloc_string(_py, b"molt_gpu_tensor__tensor_linear_split_last_dim");
            let split_name_bits = MoltObject::from_ptr(split_name_ptr).bits();
            let split_bits =
                molt_load_intrinsic_runtime(split_name_bits, MoltObject::none().bits());
            let split_ptr = obj_from_bits(split_bits)
                .as_ptr()
                .expect("split intrinsic should resolve to a function");
            assert_eq!(
                unsafe { object_type_id(split_ptr) },
                crate::TYPE_ID_FUNCTION
            );
            assert_eq!(
                unsafe { crate::function_fn_ptr(split_ptr) },
                crate::molt_gpu_tensor__tensor_linear_split_last_dim as *const () as usize as u64
            );

            let missing_name_ptr = alloc_string(_py, b"molt_missing_intrinsic");
            let missing_name_bits = MoltObject::from_ptr(missing_name_ptr).bits();
            let missing_bits =
                molt_load_intrinsic_runtime(missing_name_bits, MoltObject::none().bits());
            assert!(obj_from_bits(missing_bits).is_none());

            dec_ref_bits(_py, missing_bits);
            dec_ref_bits(_py, missing_name_bits);
            dec_ref_bits(_py, split_bits);
            dec_ref_bits(_py, split_name_bits);
            dec_ref_bits(_py, resolved_bits);
            dec_ref_bits(_py, intrinsic_name_bits);
            dec_ref_bits(_py, load_bits);
            dec_ref_bits(_py, load_name_bits);
            dec_ref_bits(_py, runtime_active_out);
            dec_ref_bits(_py, runtime_active_bits);
            dec_ref_bits(_py, runtime_active_name_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }

    /// Test one-shot manifest guard: first call sets, second is ignored.
    ///
    /// Because MANIFEST_SET is a process-global static, this test must run
    /// in a single function to avoid ordering issues with parallel test
    /// execution.  We reset the flag at the start (safe in test-only code)
    /// so the test is idempotent even if other tests ran first.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "molt_set_intrinsic_manifest stores a wasm linear-memory address represented as u64; native Miri strict provenance cannot model this wasm-only pointer contract"
    )]
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
