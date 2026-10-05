use crate::intrinsics::generated::{INTRINSICS, IntrinsicDefaultValue, IntrinsicSpec};
use crate::object::ops::{
    DICT_STRING_BINDING_LIMIT, ExactStringLookup, dict_bind_string_entries,
    dict_exact_string_lookup, hash_string_bytes,
};
// `resolve_symbol` address-takes every intrinsic via `resolve_core_symbol`, so
// production artifacts must keep it unreachable. Unit tests are the only direct
// resolver users; shipped native and wasm artifacts install a per-app resolver.
#[cfg(test)]
use crate::intrinsics::generated::resolve_symbol;
use crate::{
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_MODULE, TYPE_ID_STRING,
    TYPE_ID_TUPLE, alloc_dict_with_pairs, alloc_string, dec_ref_bits, dict_get_in_place,
    dict_set_in_place, exception_pending, inc_ref_bits, module_dict_bits, obj_from_bits,
    object_type_id, raise_exception, runtime_state, string_bytes, string_len,
};
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, AtomicUsize, Ordering};

const REGISTRY_NAME: &str = "_molt_intrinsics";
const LOOKUP_HELPER_NAME: &str = "_molt_intrinsic_lookup";
const STRICT_FLAG: &str = "_molt_intrinsics_strict";
const RUNTIME_FLAG: &str = "_molt_runtime";
#[cfg(not(target_arch = "wasm32"))]
const LAZY_RESOLVE_NAME: &str = "_molt_lazy_resolve";

/// Per-app intrinsic manifest for WASM tree shaking.
static INTRINSIC_MANIFEST_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static INTRINSIC_MANIFEST_LEN: AtomicU32 = AtomicU32::new(0);

const PUBLICATION_EMPTY: u8 = 0;
const PUBLICATION_INITIALIZING: u8 = 1;
const PUBLICATION_READY: u8 = 2;

/// Publication state for the split manifest payload. The compiler-generated
/// bootstrap is the sole winner, but embedders may race it. Readers may consume
/// PTR/LEN only after observing READY with Acquire ordering.
static MANIFEST_STATE: AtomicU8 = AtomicU8::new(PUBLICATION_EMPTY);

/// Per-app runtime-callable resolver function pointer.
///
/// Backends emit a per-app function/table resolver
/// `molt_app_resolve_callable(name_ptr, name_len) -> u64` into the user object
/// covering exactly the runtime callables the app reaches by name. The app
/// bootstrap registers it here (via `molt_set_app_callable_resolver`) before
/// runtime initialization. Intrinsic resolution and dynamic builtin-function
/// materialization both consume this one executable authority, which keeps
/// monolithic generated resolvers native-unreachable so the linker dead-strips
/// every unused intrinsic and builtin callable.
static APP_CALLABLE_RESOLVER_ADDRESS: AtomicUsize = AtomicUsize::new(0);

/// Register the per-app runtime-callable resolver. One-shot: only the first
/// call (the compiler-generated main stub, run before `molt_runtime_init`)
/// takes effect.
///
/// `fn_ptr` is the address of the backend-emitted app resolver, an
/// `extern "C" fn(*const u8, usize) -> u64` that returns the function
/// pointer/table index for `name[..len]` or 0 when the app does not reference
/// that callable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_app_callable_resolver(fn_ptr: u64) -> u64 {
    let Some(resolver_address) = crate::provenance::abi::address(fn_ptr) else {
        return 1;
    };
    if resolver_address == 0 {
        return 1;
    }
    // The executable carrier is itself the one-shot state: zero is empty and
    // the single successful CAS publishes the complete value atomically.
    let _ = APP_CALLABLE_RESOLVER_ADDRESS.compare_exchange(
        0,
        resolver_address,
        Ordering::Release,
        Ordering::Acquire,
    );
    0
}

pub(crate) fn try_app_resolve_runtime_callable(symbol: &str) -> Option<u64> {
    let resolver_address = APP_CALLABLE_RESOLVER_ADDRESS.load(Ordering::Acquire);
    if resolver_address == 0 {
        return None;
    }
    let name_bytes = symbol.as_bytes();
    let fn_ptr: u64 = unsafe {
        // Function addresses and wasm table indices are not data pointers. Keep
        // the payload integer-valued until the one typed executable conversion.
        let resolver_ptr = crate::provenance::abi::function_ptr(
            u64::try_from(resolver_address).expect("supported function carriers fit in u64"),
        )?;
        let resolver: extern "C" fn(*const u8, usize) -> u64 = core::mem::transmute(resolver_ptr);
        resolver(name_bytes.as_ptr(), name_bytes.len())
    };
    if fn_ptr == 0 { None } else { Some(fn_ptr) }
}

/// Resolve a runtime callable through the installed app authority.
///
/// When the per-app resolver is registered, delegate to it. Otherwise return
/// `None` in production builds so the full generated resolver stays
/// dead-strippable. Without an app resolver, unit tests use generated intrinsic
/// and builtin fixtures; neither address-taking fixture is a production root.
pub(crate) fn try_app_resolve_symbol(symbol: &str) -> Option<u64> {
    if APP_CALLABLE_RESOLVER_ADDRESS.load(Ordering::Acquire) != 0 {
        // An installed resolver's miss is authoritative, including in tests.
        return try_app_resolve_runtime_callable(symbol);
    }
    #[cfg(test)]
    {
        resolve_symbol(symbol)
            .or_else(|| crate::builtins::functions::resolve_test_python_builtin_symbol(symbol))
    }
    #[cfg(not(test))]
    {
        // Non-test native: the app resolver must be registered before any
        // resolution. `resolve_symbol` is intentionally not referenced here so it
        // (and the address-of expressions for every intrinsic in
        // `resolve_core_symbol`) are dead-stripped from the final binary.
        None
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intrinsic_manifest(ptr: u64, len: u64) -> u64 {
    let Some(ptr) = crate::provenance::abi::mut_ptr::<u8>(ptr) else {
        return 1;
    };
    let Ok(len) = u32::try_from(len) else {
        return 1;
    };
    if crate::provenance::abi::checked_slice_len(ptr.cast_const(), u64::from(len)).is_none() {
        return 1;
    }
    publish_manifest(ptr, len, || {}, || {})
}

fn publish_manifest(
    ptr: *mut u8,
    len: u32,
    after_claim: impl FnOnce(),
    on_competing_initialization: impl FnOnce(),
) -> u64 {
    // Validate before claiming the one-shot slot: malformed input must not
    // permanently prevent the generated app bootstrap from installing the
    // real manifest.
    match MANIFEST_STATE.compare_exchange(
        PUBLICATION_EMPTY,
        PUBLICATION_INITIALIZING,
        Ordering::Acquire,
        Ordering::Acquire,
    ) {
        Ok(_) => after_claim(),
        Err(mut state) => {
            // A competing setter must not return while the winner's split
            // payload is only half initialized. Observe its Release publication.
            if state == PUBLICATION_INITIALIZING {
                on_competing_initialization();
            }
            while state == PUBLICATION_INITIALIZING {
                core::hint::spin_loop();
                state = MANIFEST_STATE.load(Ordering::Acquire);
            }
            debug_assert_eq!(state, PUBLICATION_READY);
            return 0;
        }
    }
    INTRINSIC_MANIFEST_PTR.store(ptr, Ordering::Relaxed);
    INTRINSIC_MANIFEST_LEN.store(len, Ordering::Relaxed);
    MANIFEST_STATE.store(PUBLICATION_READY, Ordering::Release);
    0
}

#[allow(dead_code)]
fn manifest_snapshot() -> Option<(*mut u8, usize)> {
    if MANIFEST_STATE.load(Ordering::Acquire) != PUBLICATION_READY {
        return None;
    }
    let ptr = INTRINSIC_MANIFEST_PTR.load(Ordering::Relaxed);
    let len = INTRINSIC_MANIFEST_LEN.load(Ordering::Relaxed) as usize;
    Some((ptr, len))
}

#[allow(dead_code)]
fn manifest_bytes() -> Option<&'static [u8]> {
    let (ptr, len) = manifest_snapshot()?;
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

#[cfg(target_arch = "wasm32")]
fn parse_manifest() -> Option<std::collections::BTreeSet<&'static str>> {
    let bytes = manifest_bytes()?;
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

fn module_dict_ptr(module_ptr: *mut u8) -> Option<*mut u8> {
    if module_ptr.is_null() {
        return None;
    }
    unsafe {
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return None;
        }
    }
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    match obj_from_bits(dict_bits).as_ptr() {
        Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Some(ptr),
        _ => None,
    }
}

fn ensure_registry_memory_error(py: &PyToken<'_>, message: &'static str) {
    if !exception_pending(py) {
        raise_exception::<()>(py, "MemoryError", message);
    }
}

struct OwnedExactDict {
    ptr: *mut u8,
    _owner: crate::PtrDropGuard,
}

impl OwnedExactDict {
    fn empty(py: &PyToken<'_>) -> Option<Self> {
        let ptr = alloc_dict_with_pairs(py, &[]);
        if ptr.is_null() {
            ensure_registry_memory_error(py, "intrinsic registry dictionary allocation failed");
            return None;
        }
        Some(Self {
            ptr,
            _owner: crate::PtrDropGuard::new(ptr),
        })
    }

    fn copy(py: &PyToken<'_>, live: *mut u8) -> Option<Self> {
        if exception_pending(py) {
            return None;
        }
        let staged_bits =
            crate::object::ops_dict::molt_dict_copy(MoltObject::from_ptr(live).bits());
        let Some(ptr) = obj_from_bits(staged_bits).as_ptr() else {
            ensure_registry_memory_error(py, "intrinsic registry dictionary copy failed");
            return None;
        };
        let owner = crate::PtrDropGuard::new(ptr);
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT || exception_pending(py) {
            if !exception_pending(py) {
                raise_exception::<()>(py, "RuntimeError", "dictionary copy returned a non-dict");
            }
            return None;
        }
        Some(Self { ptr, _owner: owner })
    }

    /// Retain a live dictionary for the duration of one transition.
    fn retain(py: &PyToken<'_>, live: *mut u8) -> Self {
        inc_ref_bits(py, MoltObject::from_ptr(live).bits());
        Self {
            ptr: live,
            _owner: crate::PtrDropGuard::new(live),
        }
    }
}

/// Whole-namespace staging, reserved for the unbounded seeding of the `builtins`
/// namespace. Bounded registry and namespace bindings never copy a live
/// namespace: they commit through one `dict_bind_string_entries` transition.
struct StagedDictPublication {
    live: *mut u8,
    staged: OwnedExactDict,
}

impl StagedDictPublication {
    fn prepare(py: &PyToken<'_>, live: *mut u8) -> Option<Self> {
        Some(Self {
            live,
            staged: OwnedExactDict::copy(py, live)?,
        })
    }

    fn staged_ptr(&self) -> *mut u8 {
        self.staged.ptr
    }

    unsafe fn publish(&self, py: &PyToken<'_>) {
        unsafe { crate::object::ops::dict_publish_staged(py, self.live, self.staged.ptr) };
    }
}

fn named_dict_value(py: &PyToken<'_>, dict_ptr: *mut u8, name: &[u8]) -> Result<Option<u64>, ()> {
    let key_ptr = alloc_string(py, name);
    if key_ptr.is_null() {
        ensure_registry_memory_error(py, "intrinsic registry key allocation failed");
        return Err(());
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let value = unsafe { dict_get_in_place(py, dict_ptr, key_bits) };
    dec_ref_bits(py, key_bits);
    if exception_pending(py) {
        Err(())
    } else {
        Ok(value)
    }
}

/// Checked insertion into a dictionary no other code observes yet: the staged
/// `builtins` namespace or a new runtime registry. A failed write can never leak
/// a partial namespace; live namespaces bind through `bind_names`.
fn checked_dict_insert(py: &PyToken<'_>, dict_ptr: *mut u8, name: &[u8], value_bits: u64) -> bool {
    if value_bits == 0 {
        ensure_registry_memory_error(py, "intrinsic registry value allocation failed");
        return false;
    }
    let key_ptr = alloc_string(py, name);
    if key_ptr.is_null() {
        ensure_registry_memory_error(py, "intrinsic registry key allocation failed");
        return false;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    unsafe {
        dict_set_in_place(py, dict_ptr, key_bits, value_bits);
    }
    let inserted = if exception_pending(py) {
        false
    } else {
        (unsafe { dict_get_in_place(py, dict_ptr, key_bits) }) == Some(value_bits)
    };
    dec_ref_bits(py, key_bits);
    if !inserted {
        ensure_registry_memory_error(py, "intrinsic registry dictionary update failed");
    }
    inserted
}

/// Publish the runtime-backed Python namespace before the compiler's module
/// metadata can recursively import Python code. The caller must hold the
/// canonical module initialization transaction. There is no lazy refill after
/// publication: dictionary edits and deletions remain authoritative.
pub(crate) fn publish_python_native_namespace(
    py: &PyToken<'_>,
    provider: &str,
    module_bits: u64,
) -> bool {
    if provider != "builtins"
        && !crate::builtins::functions::PYTHON_BUILTIN_FUNCTIONS
            .iter()
            .any(|spec| spec.python_module == provider)
    {
        return true;
    }
    let Some(live) = obj_from_bits(module_bits)
        .as_ptr()
        .and_then(module_dict_ptr)
    else {
        raise_exception::<()>(
            py,
            "TypeError",
            "native provider initializer must publish a module",
        );
        return false;
    };
    let Some(publication) = StagedDictPublication::prepare(py, live) else {
        return false;
    };
    let staged = publication.staged_ptr();
    if provider == "builtins" {
        for (name, bits) in crate::builtins::classes::public_builtin_classes(py) {
            if !checked_dict_insert(py, staged, name.as_bytes(), bits) {
                return false;
            }
        }
        let minor = crate::object::ops_sys::runtime_target_minor(py) as u32;
        for spec in molt_obj_model::builtin_exception_specs() {
            let Some(name) = spec.public_name(minor, cfg!(target_os = "windows")) else {
                continue;
            };
            let Some(bits) =
                crate::builtins::exceptions::builtin_exception_type_bits_from_name(py, name)
            else {
                ensure_registry_memory_error(py, "builtin exception materialization failed");
                return false;
            };
            let inserted = checked_dict_insert(py, staged, name.as_bytes(), bits);
            dec_ref_bits(py, bits);
            if !inserted {
                return false;
            }
        }
    }
    for spec in crate::builtins::functions::PYTHON_BUILTIN_FUNCTIONS {
        // The per-app resolver is the executable/profile authority. Excluded
        // providers stay unavailable; allocation errors must never be skipped.
        if spec.python_module != provider
            || crate::builtins::classes::is_public_builtin_class_name(spec.python_name)
            || try_app_resolve_symbol(spec.runtime_name).is_none()
        {
            continue;
        }
        let Some(bits) =
            crate::builtins::functions::alloc_python_builtin_function_bits(py, *spec, module_bits)
        else {
            ensure_registry_memory_error(py, "builtin callable materialization failed");
            return false;
        };
        let inserted = checked_dict_insert(py, staged, spec.python_name.as_bytes(), bits);
        dec_ref_bits(py, bits);
        if !inserted {
            return false;
        }
    }
    if provider == "builtins" {
        for (name, bits) in [
            ("None", MoltObject::none().bits()),
            ("False", MoltObject::from_bool(false).bits()),
            ("True", MoltObject::from_bool(true).bits()),
            ("Ellipsis", crate::ellipsis_bits(py)),
            ("NotImplemented", crate::not_implemented_bits(py)),
        ] {
            if !checked_dict_insert(py, staged, name.as_bytes(), bits) {
                return false;
            }
        }
    }
    unsafe {
        publication.publish(py);
    }
    !exception_pending(py)
}

/// Finalize foreign-provider aliases after the builtins initializer has
/// published its base namespace and cache/table projections. Recursive provider
/// imports can then use that namespace. Admission precedes module execution;
/// an excluded provider is never initialized merely to fill a public alias.
pub(crate) fn publish_python_builtin_aliases(py: &PyToken<'_>, module_bits: u64) -> bool {
    let Some(builtins) = obj_from_bits(module_bits)
        .as_ptr()
        .and_then(module_dict_ptr)
    else {
        raise_exception::<()>(
            py,
            "TypeError",
            "builtins initializer must publish a module",
        );
        return false;
    };
    for spec in crate::builtins::functions::PYTHON_BUILTIN_FUNCTIONS {
        if spec.python_module == "builtins"
            || crate::builtins::classes::is_public_builtin_class_name(spec.python_name)
            || try_app_resolve_symbol(spec.runtime_name).is_none()
        {
            continue;
        }
        let Some(id) = crate::builtins::module_table::module_id_of(spec.python_module) else {
            raise_exception::<()>(
                py,
                "SystemError",
                "admitted native provider is absent from the module registry",
            );
            return false;
        };
        let provider = crate::builtins::module_table::module_ensure(py, id);
        let _provider_owner = obj_from_bits(provider)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        if exception_pending(py) {
            return false;
        }
        let Some(namespace) = obj_from_bits(provider).as_ptr().and_then(module_dict_ptr) else {
            raise_exception::<()>(
                py,
                "TypeError",
                "native provider initializer returned a non-module",
            );
            return false;
        };
        let value = match named_dict_value(py, namespace, spec.python_name.as_bytes()) {
            Ok(Some(value)) => value,
            Ok(None) => {
                raise_exception::<()>(
                    py,
                    "ImportError",
                    "admitted native provider did not publish its callable",
                );
                return false;
            }
            Err(()) => return false,
        };
        inc_ref_bits(py, value);
        let displaced = bind_names(py, builtins, &[(spec.python_name, value)]);
        dec_ref_bits(py, value);
        let Some(displaced) = displaced else {
            return false;
        };
        drop(displaced);
        if exception_pending(py) {
            return false;
        }
    }
    !exception_pending(py)
}

pub(crate) fn install_into_builtins(_py: &PyToken<'_>, module_ptr: *mut u8) {
    let Some(module_dict) = module_dict_ptr(module_ptr) else {
        return;
    };
    // One runtime registry serves every module namespace. The first namespace,
    // or one created after the anchor's binding was edited away, materializes
    // it; every later namespace binds that dictionary instead of rebuilding it.
    let shared = match published_registry(_py) {
        Ok(shared) => shared,
        Err(()) => return,
    };
    // Key allocation can collect; no finalizer may release the registry this
    // namespace is about to bind.
    let _shared_owner = shared.map(|registry| OwnedExactDict::retain(_py, registry));
    let created = match shared {
        Some(_) => None,
        None => match new_runtime_registry(_py) {
            Some(registry) => Some(registry),
            None => return,
        },
    };
    let Some(registry) = shared.or_else(|| created.as_ref().map(|registry| registry.ptr)) else {
        return;
    };
    let registry_bits = MoltObject::from_ptr(registry).bits();
    let enabled = MoltObject::from_bool(true).bits();
    // Native namespaces share the registry's one lazy resolver callable.
    #[cfg(not(target_arch = "wasm32"))]
    let lookup = match created.as_ref() {
        Some(created) => {
            unsafe { exact_name_value(_py, created.ptr, LAZY_RESOLVE_NAME.as_bytes()) }
                .ok()
                .flatten()
                .inspect(|&bits| inc_ref_bits(_py, bits))
        }
        None => registry_lookup_helper(_py, registry),
    };
    #[cfg(not(target_arch = "wasm32"))]
    let Some(lookup) = lookup else {
        ensure_registry_memory_error(_py, "intrinsic resolver allocation failed");
        return;
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _lookup_owner = obj_from_bits(lookup).as_ptr().map(crate::PtrDropGuard::new);
    #[cfg(not(target_arch = "wasm32"))]
    let bindings = [
        (REGISTRY_NAME, registry_bits),
        (STRICT_FLAG, enabled),
        (RUNTIME_FLAG, enabled),
        (LOOKUP_HELPER_NAME, lookup),
    ];
    #[cfg(target_arch = "wasm32")]
    let bindings = [
        (REGISTRY_NAME, registry_bits),
        (STRICT_FLAG, enabled),
        (RUNTIME_FLAG, enabled),
    ];
    let Some(displaced) = bind_names(_py, module_dict, &bindings) else {
        return;
    };
    // Only the first namespace anchors the registry. The anchor lives in
    // RuntimeState, not a process global, because tests and embedders
    // re-initialize runtime state in-process. It is never swapped: releasing a
    // prior anchor that the module cache still referenced was a use-after-free.
    let registry_module = &runtime_state(_py).intrinsic_registry_module;
    if registry_module.load(Ordering::Acquire).is_null() {
        inc_ref_bits(_py, MoltObject::from_ptr(module_ptr).bits());
        registry_module.store(module_ptr, Ordering::Release);
    }
    // A replaced value may reenter module construction from its finalizer.
    // Both the namespace and its runtime anchor must be visible by then.
    drop(displaced);
}

/// Materialize the runtime registry for the first module namespace. The
/// dictionary stays private until a namespace binds it.
fn new_runtime_registry(_py: &PyToken<'_>) -> Option<OwnedExactDict> {
    let registry = OwnedExactDict::empty(_py)?;

    // The Python intrinsic-loader interface on wasm32 reads this dictionary;
    // native instead publishes its lazy lookup helper below. Both targets'
    // runtime resolver can materialize callables through the app-owned resolver.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let resolver_fn_ptr = molt_intrinsic_resolve as *const () as usize as u64;
        let Some(resolver_bits) = build_intrinsic_func(_py, resolver_fn_ptr, 1, &[]) else {
            ensure_registry_memory_error(_py, "intrinsic resolver allocation failed");
            return None;
        };
        let installed = checked_dict_insert(
            _py,
            registry.ptr,
            LAZY_RESOLVE_NAME.as_bytes(),
            resolver_bits,
        );
        dec_ref_bits(_py, resolver_bits);
        if !installed {
            return None;
        }
    }

    // On WASM with a manifest, eagerly register only the referenced
    // intrinsics through the app resolver, once per runtime. The manifest
    // already filters to only the functions the compiled module uses, and the
    // resolver address-takes only those symbols.
    #[cfg(target_arch = "wasm32")]
    {
        let manifest = parse_manifest();
        if let Some(ref m) = manifest {
            for spec in INTRINSICS {
                if !m.contains(spec.name) {
                    continue;
                }
                if crate::builtins::functions::runtime_callable_symbol_is_non_callable(spec.symbol)
                {
                    continue;
                }
                let Some(fn_ptr) = try_app_resolve_symbol(spec.symbol) else {
                    continue;
                };
                let Some(func_bits) = build_intrinsic_func(_py, fn_ptr, spec.arity, spec.defaults)
                else {
                    ensure_registry_memory_error(_py, "intrinsic function allocation failed");
                    return None;
                };
                let installed =
                    checked_dict_insert(_py, registry.ptr, spec.name.as_bytes(), func_bits)
                        && match alias_name(spec.name) {
                            Some(alias) => {
                                checked_dict_insert(_py, registry.ptr, alias.as_bytes(), func_bits)
                            }
                            None => true,
                        };
                dec_ref_bits(_py, func_bits);
                if !installed {
                    return None;
                }
            }
        }
    }
    Some(registry)
}

/// The lookup helper a native namespace binds when it shares the registry: the
/// registry's resolver materialization while it is intact, otherwise a fresh
/// one. The shared registry itself is never rewritten here.
#[cfg(not(target_arch = "wasm32"))]
fn registry_lookup_helper(_py: &PyToken<'_>, registry: *mut u8) -> Option<u64> {
    let resolver_fn_ptr = molt_intrinsic_resolve as *const () as usize as u64;
    if let Ok(Some(bits)) = unsafe { exact_name_value(_py, registry, LAZY_RESOLVE_NAME.as_bytes()) }
        && is_runtime_materialization(_py, bits, resolver_fn_ptr, 1, &[])
    {
        inc_ref_bits(_py, bits);
        return Some(bits);
    }
    build_intrinsic_func(_py, resolver_fn_ptr, 1, &[])
}

/// Bind a bounded set of names in a live namespace as one transition. Keys are
/// allocated before any probe and values are borrowed. The caller receives
/// displaced ownership and releases it after its dependent metadata commits.
fn bind_names<'a, 'py>(
    _py: &'a PyToken<'py>,
    namespace: *mut u8,
    bindings: &[(&str, u64)],
) -> Option<crate::object::ops::DetachedDictReferences<'a, 'py, [u64; DICT_STRING_BINDING_LIMIT]>> {
    if bindings.len() > DICT_STRING_BINDING_LIMIT {
        raise_exception::<()>(_py, "SystemError", "namespace binding exceeds its bound");
        return None;
    }
    let mut key_owners: [Option<crate::PtrDropGuard>; DICT_STRING_BINDING_LIMIT] =
        Default::default();
    let mut entries = [(0, 0); DICT_STRING_BINDING_LIMIT];
    for ((entry, owner), &(name, value)) in entries.iter_mut().zip(&mut key_owners).zip(bindings) {
        let key = alloc_string(_py, name.as_bytes());
        if key.is_null() {
            ensure_registry_memory_error(_py, "intrinsic registry key allocation failed");
            return None;
        }
        *owner = Some(crate::PtrDropGuard::new(key));
        *entry = (MoltObject::from_ptr(key).bits(), value);
    }
    match unsafe { dict_bind_string_entries(_py, namespace, &entries[..bindings.len()]) } {
        Ok(displaced) => {
            drop(key_owners);
            Some(displaced)
        }
        Err(()) => {
            ensure_registry_memory_error(_py, "intrinsic registry dictionary update failed");
            None
        }
    }
}

/// Resolve a single intrinsic by name through the runtime registry: reuse its
/// published materialization or publish one, and return the function bits.
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

        inc_ref_bits(_py, name_bits);
        let _name_owner = crate::PtrDropGuard::new(name_ptr);
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
    if let Some(primary) = name
        .strip_prefix('_')
        .filter(|name| name.starts_with("molt_"))
    {
        return find_spec_by_name(primary);
    }
    // Python builtin spellings belong to python_builtin_function_info, not the
    // intrinsic namespace. Some valid ABI-backed builtins have no IntrinsicSpec.
    None
}

fn install_intrinsics_module_exports(_py: &PyToken<'_>, module_ptr: *mut u8) -> bool {
    let Some(namespace) = module_dict_ptr(module_ptr) else {
        return false;
    };
    let exports: [(&str, &str, u64); 3] = [
        (
            "require_intrinsic",
            "molt_require_intrinsic_runtime",
            molt_require_intrinsic_runtime as *const () as usize as u64,
        ),
        (
            "load_intrinsic",
            "molt_load_intrinsic_runtime",
            molt_load_intrinsic_runtime as *const () as usize as u64,
        ),
        (
            "runtime_active",
            "molt_runtime_active_runtime",
            molt_runtime_active_runtime as *const () as usize as u64,
        ),
    ];
    let mut owners: [Option<crate::PtrDropGuard>; 3] = Default::default();
    let mut bindings = [("", 0); 3];
    for ((binding, owner), &(name, symbol, fn_ptr)) in
        bindings.iter_mut().zip(&mut owners).zip(&exports)
    {
        let spec = find_spec_by_name(symbol).expect("bootstrap intrinsic must be manifested");
        let Some(bits) = build_bootstrap_function(_py, fn_ptr, spec) else {
            return false;
        };
        *owner = obj_from_bits(bits).as_ptr().map(crate::PtrDropGuard::new);
        *binding = (name, bits);
    }
    bind_names(_py, namespace, &bindings).is_some()
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

fn materialize_intrinsic_defaults(
    _py: &PyToken<'_>,
    defaults: &[IntrinsicDefaultValue],
) -> Option<Vec<u64>> {
    Some(
        defaults
            .iter()
            .map(|&default| intrinsic_default_bits(default))
            .collect(),
    )
}

/// The canonical boxed value of one generated intrinsic default.
fn intrinsic_default_bits(default: IntrinsicDefaultValue) -> u64 {
    match default {
        IntrinsicDefaultValue::None => MoltObject::none().bits(),
        IntrinsicDefaultValue::Bool(value) => MoltObject::from_bool(value).bits(),
        IntrinsicDefaultValue::Int(value) => MoltObject::from_int(value).bits(),
    }
}

fn build_runtime_function(
    py: &PyToken<'_>,
    fn_ptr: u64,
    arity: u8,
    defaults: &[u64],
) -> Option<u64> {
    let bits = if defaults.is_empty() {
        crate::builtins::methods::alloc_builtin_function(py, fn_ptr, u64::from(arity))
    } else {
        crate::builtins::methods::alloc_builtin_function_with_defaults(
            py,
            fn_ptr,
            u64::from(arity),
            defaults,
        )
    };
    (bits != 0).then_some(bits)
}

fn build_bootstrap_function(_py: &PyToken<'_>, fn_ptr: u64, spec: &IntrinsicSpec) -> Option<u64> {
    // Bootstrap precedes builtin-class publication, but consumes the same
    // signature/default authority as later compiled and registry callables.
    let defaults = materialize_intrinsic_defaults(_py, spec.defaults)?;
    let ptr =
        crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, u64::from(spec.arity));
    if ptr.is_null() {
        ensure_registry_memory_error(_py, "bootstrap callable allocation failed");
        return None;
    }
    let fn_bits = MoltObject::from_ptr(ptr).bits();
    if !defaults.is_empty()
        && !unsafe { crate::builtins::methods::set_function_defaults(_py, ptr, &defaults) }
    {
        dec_ref_bits(_py, fn_bits);
        return None;
    }
    Some(fn_bits)
}

fn build_intrinsic_func(
    _py: &PyToken<'_>,
    fn_ptr: u64,
    arity: u8,
    default_values: &[IntrinsicDefaultValue],
) -> Option<u64> {
    let defaults = materialize_intrinsic_defaults(_py, default_values)?;
    build_runtime_function(_py, fn_ptr, arity, &defaults)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntrinsicResolveError {
    Unknown,
    NotCallable,
    MissingSymbol,
    AllocFailed,
}

/// Registry and namespace reads decided without allocation or Python equality.
/// `Err(())` means a same-hash key of another kind leaves the decision to
/// ordinary lookup.
unsafe fn exact_name_value(
    py: &PyToken<'_>,
    dict: *mut u8,
    name: &[u8],
) -> Result<Option<u64>, ()> {
    let hash = hash_string_bytes(py, name) as u64;
    match unsafe { dict_exact_string_lookup(py, dict, name, hash) } {
        ExactStringLookup::Found(index) => {
            Ok(Some(unsafe { crate::dict_order(dict)[index * 2 + 1] }))
        }
        ExactStringLookup::Absent => Ok(None),
        ExactStringLookup::Undecided => Err(()),
    }
}

/// The dictionary the registry anchor module binds as `_molt_intrinsics`, or
/// `None` when no registry is published and resolution neither reads nor
/// writes one.
fn published_registry(py: &PyToken<'_>) -> Result<Option<*mut u8>, ()> {
    let anchor = runtime_state(py)
        .intrinsic_registry_module
        .load(Ordering::Acquire);
    let Some(namespace) = module_dict_ptr(anchor) else {
        return Ok(None);
    };
    let bound = match unsafe { exact_name_value(py, namespace, REGISTRY_NAME.as_bytes()) } {
        Ok(bound) => bound,
        Err(()) => named_dict_value(py, namespace, REGISTRY_NAME.as_bytes())?,
    };
    Ok(bound
        .and_then(|bits| obj_from_bits(bits).as_ptr())
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_DICT))
}

/// Whether `bits` is the runtime's materialization of one callable: an exact
/// builtin function with executable identity `fn_ptr`, this arity, the
/// positional call ABI and exactly the canonical `__defaults__`, whose callable
/// shape user code never mutated. A name, type name or callability alone never
/// admits reuse.
fn is_runtime_materialization(
    py: &PyToken<'_>,
    bits: u64,
    fn_ptr: u64,
    arity: u8,
    defaults: &[IntrinsicDefaultValue],
) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe {
        object_type_id(ptr) == TYPE_ID_FUNCTION
            && crate::object_class_bits(ptr)
                == crate::builtin_classes(py).builtin_function_or_method
            && crate::function_fn_ptr(ptr)
                == crate::builtins::functions::canonicalize_runtime_callable_key(fn_ptr)
            && crate::function_arity(ptr) == u64::from(arity)
            && crate::function_call_abi(ptr) == crate::FunctionCallAbi::Positional
            && crate::function_mutation_version(ptr) == 0
            && crate::call::function::function_has_default_only_binding_metadata(py, ptr)
            && function_defaults_are(py, ptr, defaults)
    }
}

/// Whether a function binds exactly these canonical `__defaults__`, decided
/// without allocation or Python equality.
unsafe fn function_defaults_are(
    _py: &PyToken<'_>,
    function: *mut u8,
    defaults: &[IntrinsicDefaultValue],
) -> bool {
    unsafe {
        let defaults_bits = crate::object::function_metadata::FunctionMetadataField::Defaults
            .load(function)
            .unwrap_or(MoltObject::none().bits());
        if obj_from_bits(defaults_bits).is_none() {
            return defaults.is_empty();
        }
        let Some(tuple) = obj_from_bits(defaults_bits).as_ptr() else {
            return false;
        };
        if object_type_id(tuple) != TYPE_ID_TUPLE {
            return false;
        }
        crate::object::seq_access::with_immutable_tuple_slice(tuple, |items| {
            items.len() == defaults.len()
                && items
                    .iter()
                    .zip(defaults)
                    .all(|(&item, &default)| item == intrinsic_default_bits(default))
        })
        .unwrap_or(false)
    }
}

/// Canonical spelling first, then each distinct requested or `_molt_` alias
/// spelling one registry materialization is bound under.
fn intrinsic_binding_names<'n>(
    canonical: &'n str,
    requested: &'n str,
    alias: Option<&'n str>,
) -> ([&'n str; 3], usize) {
    let mut names = [canonical; 3];
    let mut count = 1;
    for name in [Some(requested), alias].into_iter().flatten() {
        if !names[..count].contains(&name) {
            names[count] = name;
            count += 1;
        }
    }
    (names, count)
}

/// An allocation- and Python-free registry hit: the canonical spelling binds
/// this intrinsic's materialization and every other spelling binds that same
/// object, so nothing needs publishing.
unsafe fn bound_materialization(
    py: &PyToken<'_>,
    registry: *mut u8,
    names: &[&str],
    spec: &IntrinsicSpec,
    fn_ptr: u64,
) -> Option<u64> {
    let (canonical, spellings) = names.split_first()?;
    let bits = unsafe { exact_name_value(py, registry, canonical.as_bytes()) }.ok()??;
    if !is_runtime_materialization(py, bits, fn_ptr, spec.arity, spec.defaults) {
        return None;
    }
    for name in spellings {
        if unsafe { exact_name_value(py, registry, name.as_bytes()) } != Ok(Some(bits)) {
            return None;
        }
    }
    Some(bits)
}

/// Bind one materialization under every spelling in a single bounded registry
/// transition; the registry is never copied. A valid canonical materialization
/// keeps its identity, otherwise the app resolver's callable is materialized
/// afresh. Displaced values are released only after the binding commits.
fn publish_materialization(
    py: &PyToken<'_>,
    registry: *mut u8,
    names: &[&str],
    spec: &IntrinsicSpec,
    fn_ptr: u64,
) -> Result<u64, IntrinsicResolveError> {
    // Key allocation can collect and a same-hash key's equality can run
    // Python; neither may release the registry under this transition.
    let registry = OwnedExactDict::retain(py, registry);
    let canonical = names[0].as_bytes();
    let bound = match unsafe { exact_name_value(py, registry.ptr, canonical) } {
        Ok(bound) => bound,
        Err(()) => named_dict_value(py, registry.ptr, canonical)
            .map_err(|()| IntrinsicResolveError::AllocFailed)?,
    };
    let reusable = bound
        .filter(|&bits| is_runtime_materialization(py, bits, fn_ptr, spec.arity, spec.defaults));
    let value = match reusable {
        Some(bits) => {
            inc_ref_bits(py, bits);
            bits
        }
        None => build_intrinsic_func(py, fn_ptr, spec.arity, spec.defaults)
            .ok_or(IntrinsicResolveError::AllocFailed)?,
    };
    let mut value_owner = obj_from_bits(value).as_ptr().map(crate::PtrDropGuard::new);
    let mut bindings = [("", value); 3];
    for (binding, &name) in bindings.iter_mut().zip(names) {
        binding.0 = name;
    }
    let Some(displaced) = bind_names(py, registry.ptr, &bindings[..names.len()]) else {
        return Err(IntrinsicResolveError::AllocFailed);
    };
    drop(displaced);
    // The registry retains its own references; the caller owns this one.
    if let Some(owner) = value_owner.as_mut() {
        owner.release();
    }
    Ok(value)
}

fn resolve_intrinsic_func(
    _py: &PyToken<'_>,
    requested_name: &str,
    cache_result: bool,
) -> Result<u64, IntrinsicResolveError> {
    let Some(spec) = find_spec(requested_name) else {
        return Err(IntrinsicResolveError::Unknown);
    };
    if crate::builtins::functions::runtime_callable_symbol_is_non_callable(spec.symbol) {
        return Err(IntrinsicResolveError::NotCallable);
    }
    let Some(fn_ptr) = try_app_resolve_symbol(spec.symbol) else {
        return Err(IntrinsicResolveError::MissingSymbol);
    };
    // Materialization refuses to start under a pending exception; reuse
    // follows the same admission.
    if exception_pending(_py) {
        return Err(IntrinsicResolveError::AllocFailed);
    }
    // Only the registry's materialization is shared. Compiled BUILTIN_FUNC
    // construction receives a private callable with the same manifest-owned
    // defaults; no post-construction public metadata writes are needed.
    let registry = if cache_result {
        published_registry(_py).map_err(|()| IntrinsicResolveError::AllocFailed)?
    } else {
        None
    };
    let Some(registry) = registry else {
        return build_intrinsic_func(_py, fn_ptr, spec.arity, spec.defaults)
            .ok_or(IntrinsicResolveError::AllocFailed);
    };
    let alias = alias_name(spec.name);
    let (names, count) = intrinsic_binding_names(spec.name, requested_name, alias.as_deref());
    let names = &names[..count];
    if let Some(bits) = unsafe { bound_materialization(_py, registry, names, spec, fn_ptr) } {
        inc_ref_bits(_py, bits);
        return Ok(bits);
    }
    publish_materialization(_py, registry, names, spec, fn_ptr)
}

pub(crate) fn try_resolve_intrinsic_func(
    _py: &PyToken<'_>,
    requested_name: &str,
    cache_result: bool,
) -> Result<Option<u64>, ()> {
    if exception_pending(_py) {
        return Err(());
    }
    match resolve_intrinsic_func(_py, requested_name, cache_result) {
        Ok(bits) => Ok(Some(bits)),
        Err(
            IntrinsicResolveError::Unknown
            | IntrinsicResolveError::NotCallable
            | IntrinsicResolveError::MissingSymbol,
        ) => Ok(None),
        Err(IntrinsicResolveError::AllocFailed) => {
            ensure_registry_memory_error(_py, "intrinsic resolution failed");
            Err(())
        }
    }
}

/// Register a synthetic `_intrinsics` module in the module cache so that
/// stdlib Python files can `from _intrinsics import require_intrinsic`.
/// The module contains a `require_intrinsic` function that delegates to
/// the runtime's intrinsic lookup.
pub(crate) fn register_intrinsics_module(_py: &PyToken<'_>) {
    use crate::object::builders::alloc_module_obj;

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
        ensure_registry_memory_error(_py, "_intrinsics module name allocation failed");
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();

    let existing_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
    if exception_pending(_py) {
        if !obj_from_bits(existing_bits).is_none() {
            dec_ref_bits(_py, existing_bits);
        }
        dec_ref_bits(_py, name_bits);
        return;
    }
    if let Some(existing_ptr) = obj_from_bits(existing_bits).as_ptr() {
        unsafe {
            if object_type_id(existing_ptr) == TYPE_ID_MODULE {
                let _ = install_intrinsics_module_exports(_py, existing_ptr);
                dec_ref_bits(_py, existing_bits);
                dec_ref_bits(_py, name_bits);
                return;
            }
        }
        dec_ref_bits(_py, existing_bits);
        raise_exception::<()>(
            _py,
            "RuntimeError",
            "_intrinsics cache entry is not a module",
        );
        dec_ref_bits(_py, name_bits);
        return;
    }
    if !obj_from_bits(existing_bits).is_none() {
        raise_exception::<()>(
            _py,
            "RuntimeError",
            "_intrinsics cache entry is not a module",
        );
        dec_ref_bits(_py, name_bits);
        return;
    }

    let module_ptr = alloc_module_obj(_py, name_bits);
    if module_ptr.is_null() {
        ensure_registry_memory_error(_py, "_intrinsics module allocation failed");
        dec_ref_bits(_py, name_bits);
        return;
    }
    let module_bits = MoltObject::from_ptr(module_ptr).bits();

    // Mirror the public helpers exposed by src/_intrinsics.py so module-form
    // imports (`import _intrinsics as mod`) and from-imports see the same API.
    // If `_intrinsics` was already constructed by generic module creation,
    // the cached branch above repairs that module in place instead of trying
    // to replace the first-init-wins cache entry.
    if !install_intrinsics_module_exports(_py, module_ptr) {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return;
    }

    // Register in module cache
    let _ = crate::builtins::modules::molt_module_cache_set(name_bits, module_bits);
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
    MANIFEST_STATE.store(PUBLICATION_EMPTY, Ordering::SeqCst);
    INTRINSIC_MANIFEST_PTR.store(core::ptr::null_mut(), Ordering::SeqCst);
    INTRINSIC_MANIFEST_LEN.store(0, Ordering::SeqCst);
    APP_CALLABLE_RESOLVER_ADDRESS.store(0, Ordering::SeqCst);
}

// Expose internals for testing.
#[cfg(test)]
pub(crate) fn test_manifest_state() -> &'static AtomicU8 {
    &MANIFEST_STATE
}
#[cfg(test)]
pub(crate) fn test_manifest_ptr() -> &'static AtomicPtr<u8> {
    &INTRINSIC_MANIFEST_PTR
}
#[cfg(test)]
pub(crate) fn test_manifest_len() -> &'static AtomicU32 {
    &INTRINSIC_MANIFEST_LEN
}
#[cfg(test)]
pub(crate) fn test_app_resolver_address() -> &'static AtomicUsize {
    &APP_CALLABLE_RESOLVER_ADDRESS
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
        inc_ref_bits(_py, name_bits);
        let _name_owner = crate::PtrDropGuard::new(name_ptr);
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
            let resolved = find_spec(name).and_then(|spec| try_app_resolve_symbol(spec.symbol));
            eprintln!(
                "molt require_intrinsic: name={} resolved={}",
                name,
                resolved
                    .map(|addr| format!("0x{addr:x}"))
                    .unwrap_or_else(|| "<none>".to_string())
            );
        }
        match resolve_intrinsic_func(_py, name, true) {
            Ok(func_bits) => func_bits,
            Err(IntrinsicResolveError::AllocFailed) => {
                if trace {
                    eprintln!(
                        "molt require_intrinsic: alloc_function_obj failed for {}",
                        name
                    );
                }
                if exception_pending(_py) {
                    MoltObject::none().bits()
                } else {
                    raise_exception::<u64>(
                        _py,
                        "MemoryError",
                        &format!("failed to allocate intrinsic function: {name}"),
                    )
                }
            }
            Err(IntrinsicResolveError::NotCallable) => raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("intrinsic is a raw non-callable ABI: {name}"),
            ),
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
        inc_ref_bits(_py, name_bits);
        let _name_owner = crate::PtrDropGuard::new(name_ptr);
        let name = unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<u64>(_py, "TypeError", "intrinsic name must be str");
            }
            let len = string_len(name_ptr);
            let bytes = std::slice::from_raw_parts(string_bytes(name_ptr), len);
            std::str::from_utf8(bytes).unwrap_or("")
        };
        match resolve_intrinsic_func(_py, name, true) {
            Ok(func_bits) => func_bits,
            Err(
                IntrinsicResolveError::Unknown
                | IntrinsicResolveError::NotCallable
                | IntrinsicResolveError::MissingSymbol,
            ) => MoltObject::none().bits(),
            Err(IntrinsicResolveError::AllocFailed) => {
                if exception_pending(_py) {
                    MoltObject::none().bits()
                } else {
                    raise_exception::<u64>(
                        _py,
                        "MemoryError",
                        &format!("failed to allocate intrinsic function: {name}"),
                    )
                }
            }
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

    /// Cold runtime bootstrap exports the synthetic `_intrinsics` API only.
    /// The compiled module-construction path publishes the owning registry.
    fn construct_registry_owner(_py: &PyToken<'_>) -> u64 {
        assert!(
            runtime_state(_py)
                .intrinsic_registry_module
                .load(Ordering::Acquire)
                .is_null()
        );
        let name_ptr = alloc_string(_py, b"builtins");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_bits = crate::builtins::modules::molt_module_new(name_bits);
        dec_ref_bits(_py, name_bits);
        assert!(
            !exception_pending(_py),
            "compiled module construction must install the intrinsic registry"
        );
        let module_ptr = obj_from_bits(module_bits)
            .as_ptr()
            .expect("constructed builtins module");
        assert_eq!(
            runtime_state(_py)
                .intrinsic_registry_module
                .load(Ordering::Acquire),
            module_ptr,
            "module construction must publish this exact registry owner",
        );
        module_bits
    }

    static OBSERVED_ANCHOR: AtomicUsize = AtomicUsize::new(0);
    static OBSERVED_SHARED_REGISTRY: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn observe_anchor_during_reentry(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let anchor = runtime_state(py)
                .intrinsic_registry_module
                .load(Ordering::Acquire);
            OBSERVED_ANCHOR.store(anchor as usize, Ordering::Release);
            let name = alloc_string(py, b"registry_finalizer_peer");
            let name_bits = MoltObject::from_ptr(name).bits();
            let peer = crate::object::builders::alloc_module_obj(py, name_bits);
            assert!(!peer.is_null());
            install_into_builtins(py, peer);
            let peer_registry =
                named_dict_value(py, module_dict_ptr(peer).unwrap(), REGISTRY_NAME.as_bytes())
                    .unwrap();
            let shared = published_registry(py)
                .unwrap()
                .map(|ptr| MoltObject::from_ptr(ptr).bits());
            OBSERVED_SHARED_REGISTRY.store(
                usize::from(!anchor.is_null() && peer_registry == shared),
                Ordering::Release,
            );
            dec_ref_bits(py, MoltObject::from_ptr(peer).bits());
            dec_ref_bits(py, name_bits);
            MoltObject::none().bits()
        })
    }

    #[test]
    fn namespace_anchor_precedes_displaced_value_finalizer_reentry() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                OBSERVED_ANCHOR.store(0, Ordering::Release);
                OBSERVED_SHARED_REGISTRY.store(0, Ordering::Release);
                let name = alloc_string(py, b"RegistryAnchorFinalizer");
                let name_bits = MoltObject::from_ptr(name).bits();
                let class = crate::molt_class_new(name_bits);
                crate::molt_class_set_base(class, crate::builtin_classes(py).object);
                let method_name = alloc_string(py, b"__del__");
                let method_bits = MoltObject::from_ptr(method_name).bits();
                let function = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "observe_anchor_during_reentry",
                        observe_anchor_during_reentry as *const (),
                    ),
                    1,
                );
                let function_bits = MoltObject::from_ptr(function).bits();
                crate::molt_set_attr_name(class, method_bits, function_bits);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                unsafe {
                    crate::object::class_finish_definition(py, class_ptr).unwrap();
                }
                let size =
                    unsafe { crate::object::layout::class_cached_layout_size(class_ptr).unwrap() };
                let value = crate::object::builders::alloc_class_instance(py, size, class);
                unsafe {
                    crate::object::gc::gc_publish_initialized(
                        py,
                        obj_from_bits(value).as_ptr().unwrap(),
                    );
                }
                let module = crate::object::builders::alloc_module_obj(py, name_bits);
                assert!(!module.is_null());
                assert!(checked_dict_insert(
                    py,
                    module_dict_ptr(module).unwrap(),
                    RUNTIME_FLAG.as_bytes(),
                    value
                ));
                dec_ref_bits(py, value);
                assert!(
                    runtime_state(py)
                        .intrinsic_registry_module
                        .load(Ordering::Acquire)
                        .is_null()
                );

                install_into_builtins(py, module);
                assert!(!exception_pending(py));
                assert_eq!(OBSERVED_ANCHOR.load(Ordering::Acquire), module as usize);
                assert_eq!(OBSERVED_SHARED_REGISTRY.load(Ordering::Acquire), 1);
                for bits in [
                    MoltObject::from_ptr(module).bits(),
                    class,
                    function_bits,
                    method_bits,
                    name_bits,
                ] {
                    dec_ref_bits(py, bits);
                }
            });
        });
    }

    #[test]
    fn compiled_intrinsic_handles_bind_manifest_defaults_without_public_attributes() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                for symbol in [
                    "molt_require_intrinsic_runtime",
                    "molt_load_intrinsic_runtime",
                ] {
                    let name = string_bits(py, symbol);
                    let function =
                        crate::builtins::functions::molt_func_new_builtin_named(name, 0, 0, 2);
                    assert!(!exception_pending(py));
                    let ptr = obj_from_bits(function).as_ptr().unwrap();
                    unsafe {
                        assert_eq!(
                            crate::object_class_bits(ptr),
                            crate::builtin_classes(py).builtin_function_or_method
                        );
                        let defaults =
                            crate::call::function::function_metadata_bits(py, ptr, b"__defaults__");
                        let tuple = obj_from_bits(defaults).as_ptr().unwrap();
                        assert_eq!(
                            crate::object::seq_access::with_immutable_tuple_slice(tuple, |items| {
                                items == [MoltObject::none().bits()]
                            }),
                            Some(true),
                        );
                    }
                    let request = string_bits(py, "molt_stdlib_probe");
                    let resolved =
                        unsafe { crate::call::function::call_function_obj1(py, function, request) };
                    assert!(!exception_pending(py));
                    assert!(obj_from_bits(resolved).as_ptr().is_some());
                    let defaults_name = string_bits(py, "__defaults__");
                    crate::molt_get_attr_name(function, defaults_name);
                    assert!(exception_pending(py));
                    assert!(crate::builtins::attr::exception_is_attribute_error(
                        py,
                        crate::exception_last_bits_noinc(py).unwrap(),
                    ));
                    crate::clear_exception(py);
                    for bits in [name, function, request, resolved, defaults_name] {
                        dec_ref_bits(py, bits);
                    }
                }
            });
        });
    }

    #[test]
    fn intrinsic_apis_borrow_names_without_leaking_a_reference() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let owner = construct_registry_owner(py);
                let name =
                    crate::object::builders::alloc_string_nointern(py, b"molt_capabilities_has");
                assert!(!name.is_null());
                let bits = MoltObject::from_ptr(name).bits();
                let owners = unsafe { (*crate::header_from_obj_ptr(name)).ref_count_snapshot() };
                for resolve in [
                    molt_require_intrinsic_runtime as extern "C" fn(u64, u64) -> u64,
                    molt_load_intrinsic_runtime,
                ] {
                    let function = resolve(bits, MoltObject::none().bits());
                    assert!(!exception_pending(py));
                    assert!(obj_from_bits(function).as_ptr().is_some());
                    assert_eq!(
                        unsafe { (*crate::header_from_obj_ptr(name)).ref_count_snapshot() },
                        owners
                    );
                    dec_ref_bits(py, function);
                }
                let function = molt_intrinsic_resolve(bits);
                assert!(!exception_pending(py));
                assert!(obj_from_bits(function).as_ptr().is_some());
                assert_eq!(
                    unsafe { (*crate::header_from_obj_ptr(name)).ref_count_snapshot() },
                    owners
                );
                dec_ref_bits(py, function);
                dec_ref_bits(py, bits);
                dec_ref_bits(py, owner);
            });
        });
    }

    #[test]
    fn optional_intrinsic_resolution_distinguishes_failure_from_absence() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                assert_eq!(
                    try_resolve_intrinsic_func(py, "molt_not_an_intrinsic", false),
                    Ok(None)
                );
                let denied = deny_allocation();
                assert_eq!(
                    try_resolve_intrinsic_func(py, "molt_capabilities_has", false),
                    Err(())
                );
                drop(denied);
                assert!(exception_pending(py));
                crate::clear_exception(py);
            });
        });
    }

    #[cfg(not(feature = "stdlib_net"))]
    #[test]
    fn ssl_intrinsics_resolve_when_stdlib_net_is_disabled() {
        assert!(resolve_symbol("molt_ssl_cert_none").is_some());
        assert!(resolve_symbol("molt_ssl_context_new").is_some());
        assert!(resolve_symbol("molt_ssl_wrap_socket").is_some());
        assert!(resolve_symbol("molt_ssl_socket_read").is_some());
    }

    #[cfg(not(feature = "stdlib_http"))]
    #[test]
    fn http_intrinsics_do_not_resolve_when_stdlib_http_is_disabled() {
        assert!(resolve_symbol("molt_http_client_execute").is_none());
    }

    #[cfg(not(feature = "stdlib_regex"))]
    #[test]
    fn regex_engine_intrinsics_do_not_resolve_when_stdlib_regex_is_disabled() {
        // The byte-matching primitives moved into the `molt-runtime-regex` leaf
        // crate alongside the compiled-regex engine, so they are now gated by
        // `stdlib_regex` too — nothing `molt_re_*` links when the feature is off.
        assert!(resolve_symbol("molt_re_literal_advance").is_none());
        assert!(resolve_symbol("molt_re_charclass_advance").is_none());

        assert!(resolve_symbol("molt_re_compile").is_none());
        assert!(resolve_symbol("molt_re_execute").is_none());
        assert!(resolve_symbol("molt_re_match_group").is_none());
    }

    /// Intrinsic names are the linker symbols the runtime resolves. Keeping this
    /// invariant global prevents a second name-remapping authority from creeping
    /// back into backend manifest generation.
    #[test]
    fn intrinsic_specs_use_public_name_as_runtime_symbol() {
        for spec in INTRINSICS {
            assert_eq!(spec.name, spec.symbol, "intrinsic name/symbol drift");
        }
    }

    #[test]
    fn builtin_spellings_and_intrinsic_names_have_distinct_metadata_authorities() {
        for (python_name, runtime_name) in [
            ("len", "molt_len"),
            ("globals", "molt_globals_builtin"),
            ("locals", "molt_locals_builtin"),
            ("vars", "molt_vars_builtin"),
            ("__import__", "molt_importlib_import_transaction"),
        ] {
            let info = crate::builtins::functions::python_builtin_function_info(python_name)
                .unwrap_or_else(|| panic!("missing generated builtin mapping for {python_name}"));
            assert_eq!(info.runtime_name, runtime_name);
            assert!(find_spec(python_name).is_none());
            // Intrinsic membership is independent of builtin membership.
            if let Some(spec) = find_spec(runtime_name) {
                let alias = format!("_molt_{}", &runtime_name[5..]);
                assert!(core::ptr::eq(
                    spec,
                    find_spec(&alias).expect("canonical alias")
                ));
            }
        }

        assert!(
            find_spec("getframe").is_none(),
            "an unlisted Python spelling must not prefix-guess molt_getframe"
        );
        assert_eq!(
            find_spec("_molt_getframe").map(|spec| spec.name),
            Some("molt_getframe"),
            "explicit _molt_ intrinsic aliases remain supported"
        );

        let import_info = crate::builtins::functions::python_builtin_function_info("__import__")
            .expect("generated __import__ callable metadata");
        assert_eq!(import_info.arity, 5);
        assert_eq!(
            import_info.pos_or_kw_params,
            &["name", "globals", "locals", "fromlist", "level"]
        );
        assert_eq!(
            format!("{:?}", import_info.defaults),
            "[Missing, None, EmptyTuple, Int(0)]"
        );
    }

    #[test]
    fn non_callable_intrinsics_cannot_be_materialized_through_runtime_lookup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let registry_before = runtime_state(_py)
                .intrinsic_registry_module
                .load(Ordering::Acquire);
            for name in [
                "molt_dict_getitem_borrowed",
                "molt_list_getitem_borrowed",
                "molt_tuple_getitem_borrowed",
                "molt_exception_pending",
                "molt_async_work_poll_and_exception_pending",
                "molt_frame_invocation_enter",
                "molt_frame_invocation_exit",
                "molt_gpu_prim_realize",
                "molt_gpu_prim_dtype",
                "molt_gpu_prim_nbytes",
                "molt_gpu_prim_free",
                "molt_gpu_prim_contiguous",
                "molt_gpu_prim_numel",
            ] {
                let alias = alias_name(name).expect("intrinsic alias");
                for requested in [name, alias.as_str()] {
                    assert_eq!(
                        resolve_intrinsic_func(_py, requested, false),
                        Err(IntrinsicResolveError::NotCallable),
                        "{requested} must remain a compiler/C-ABI-only primitive"
                    );
                    let name_ptr = alloc_string(_py, requested.as_bytes());
                    assert!(!name_ptr.is_null());
                    let name_bits = MoltObject::from_ptr(name_ptr).bits();
                    let required =
                        molt_require_intrinsic_runtime(name_bits, MoltObject::none().bits());
                    assert_eq!(required, MoltObject::none().bits());
                    assert!(exception_pending(_py));
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        _py,
                        error,
                        "RuntimeError"
                    ));
                    crate::clear_exception(_py);
                    dec_ref_bits(_py, error);

                    let loaded = molt_load_intrinsic_runtime(name_bits, MoltObject::none().bits());
                    assert_eq!(loaded, MoltObject::none().bits());
                    assert!(!exception_pending(_py));
                    dec_ref_bits(_py, name_bits);
                }
            }
            assert_eq!(
                runtime_state(_py)
                    .intrinsic_registry_module
                    .load(Ordering::Acquire),
                registry_before,
                "rejected lookups must not publish an intrinsic registry"
            );
        });
    }

    #[test]
    fn staged_registry_failure_leaves_live_dictionary_unchanged() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let live = OwnedExactDict::empty(_py).expect("live dictionary allocation");
            assert!(checked_dict_insert(
                _py,
                live.ptr,
                b"stable",
                MoltObject::from_bool(true).bits(),
            ));
            let stable_before = named_dict_value(_py, live.ptr, b"stable").unwrap();

            let publication =
                StagedDictPublication::prepare(_py, live.ptr).expect("staged dictionary copy");
            assert!(checked_dict_insert(
                _py,
                publication.staged_ptr(),
                b"partial",
                MoltObject::from_bool(true).bits(),
            ));
            assert!(!checked_dict_insert(
                _py,
                publication.staged_ptr(),
                b"rejected",
                0,
            ));
            drop(publication);

            assert!(exception_pending(_py));
            let _ = crate::molt_exception_clear();
            assert_eq!(
                named_dict_value(_py, live.ptr, b"stable").unwrap(),
                stable_before
            );
            assert_eq!(named_dict_value(_py, live.ptr, b"partial").unwrap(), None);
            assert_eq!(named_dict_value(_py, live.ptr, b"rejected").unwrap(), None);
        });
    }

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            crate::resource::set_tracker(Box::new(crate::resource::UnlimitedTracker));
        }
    }

    /// Deny every allocation and container growth until the guard drops.
    fn deny_allocation() -> TrackerReset {
        crate::resource::set_tracker(Box::new(crate::resource::LimitedTracker::new(
            &crate::resource::ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            },
        )));
        TrackerReset
    }

    fn runtime_registry(py: &PyToken<'_>) -> *mut u8 {
        published_registry(py)
            .expect("registry lookup")
            .expect("published runtime registry")
    }

    fn bound_value(py: &PyToken<'_>, dict: *mut u8, name: &str) -> Option<u64> {
        named_dict_value(py, dict, name.as_bytes()).expect("ordinary lookup")
    }

    fn string_bits(py: &PyToken<'_>, text: &str) -> u64 {
        let ptr = alloc_string(py, text.as_bytes());
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    /// The executable identity the installed app resolver assigns an intrinsic.
    fn app_fn_ptr(name: &str) -> u64 {
        try_app_resolve_symbol(find_spec(name).expect("intrinsic spec").symbol)
            .expect("test callable fixture")
    }

    fn fn_ptr_of(bits: u64) -> u64 {
        unsafe { crate::function_fn_ptr(obj_from_bits(bits).as_ptr().expect("function")) }
    }

    fn mutation_version(bits: u64) -> u64 {
        unsafe { crate::function_mutation_version(obj_from_bits(bits).as_ptr().expect("function")) }
    }

    /// Public builtin metadata is immutable; rejected writes preserve binding.
    fn reject_user_defaults(py: &PyToken<'_>, function: u64, default: i64) {
        let name = string_bits(py, "__defaults__");
        let tuple = crate::alloc_tuple(py, &[MoltObject::from_int(default).bits()]);
        assert!(!tuple.is_null());
        let tuple = MoltObject::from_ptr(tuple).bits();
        let before = mutation_version(function);
        let _ = crate::molt_set_attr_name(function, name, tuple);
        assert!(exception_pending(py));
        assert!(crate::builtins::attr::exception_is_attribute_error(
            py,
            crate::exception_last_bits_noinc(py).unwrap(),
        ));
        crate::clear_exception(py);
        assert_eq!(mutation_version(function), before);
        dec_ref_bits(py, tuple);
        dec_ref_bits(py, name);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reuse compares callable addresses from separate fn-pointer casts, which Miri's provenance model exposes as distinct"
    )]
    fn published_materialization_is_reused_without_allocation() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let registry = runtime_registry(_py);
                // The WASM bootstrap binds eager materializations exactly so.
                let spec = find_spec("molt_operator_length_hint").expect("intrinsic with defaults");
                let eager =
                    build_intrinsic_func(_py, app_fn_ptr(spec.name), spec.arity, spec.defaults)
                        .expect("eager materialization");
                assert!(checked_dict_insert(
                    _py,
                    registry,
                    spec.name.as_bytes(),
                    eager
                ));
                assert!(checked_dict_insert(
                    _py,
                    registry,
                    b"_molt_operator_length_hint",
                    eager,
                ));
                dec_ref_bits(_py, eager);
                // Native resolution publishes on the first request.
                let lazy = resolve_intrinsic_func(_py, "_molt_capabilities_has", true)
                    .expect("first resolution publishes");
                dec_ref_bits(_py, lazy);
                for (canonical, alias, expected) in [
                    (
                        "molt_operator_length_hint",
                        "_molt_operator_length_hint",
                        eager,
                    ),
                    ("molt_capabilities_has", "_molt_capabilities_has", lazy),
                ] {
                    assert_eq!(bound_value(_py, registry, canonical), Some(expected));
                    assert_eq!(bound_value(_py, registry, alias), Some(expected));
                    let entries = unsafe { crate::dict_len(registry) };
                    // Every allocation and growth is denied, so copying,
                    // rebuilding or rebinding the registry would fail here.
                    let denial = deny_allocation();
                    for requested in [canonical, alias] {
                        assert_eq!(resolve_intrinsic_func(_py, requested, true), Ok(expected));
                        assert!(!exception_pending(_py));
                        dec_ref_bits(_py, expected);
                    }
                    drop(denial);
                    assert_eq!(unsafe { crate::dict_len(registry) }, entries);
                    assert_eq!(bound_value(_py, registry, canonical), Some(expected));
                }
                dec_ref_bits(_py, owner_bits);
            });
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reuse compares callable addresses from separate fn-pointer casts, which Miri's provenance model exposes as distinct"
    )]
    fn registry_edits_remain_authoritative_over_reuse() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let registry = runtime_registry(_py);
                let (canonical, alias) = ("molt_capabilities_has", "_molt_capabilities_has");
                let fn_ptr = app_fn_ptr(canonical);
                let binds = |value: u64| {
                    assert_eq!(bound_value(_py, registry, canonical), Some(value));
                    assert_eq!(bound_value(_py, registry, alias), Some(value));
                };
                let canonical_key = string_bits(_py, canonical);
                let alias_key = string_bits(_py, alias);
                let first = resolve_intrinsic_func(_py, canonical, true).expect("publication");
                binds(first);

                // A deleted spelling is rebound to the surviving materialization.
                assert!(unsafe { crate::object::ops::dict_del_in_place(_py, registry, alias_key) });
                assert_eq!(resolve_intrinsic_func(_py, alias, true), Ok(first));
                dec_ref_bits(_py, first);
                binds(first);

                // A non-callable replacement is never returned.
                unsafe {
                    dict_set_in_place(_py, registry, canonical_key, MoltObject::from_int(7).bits());
                }
                let second = resolve_intrinsic_func(_py, canonical, true).expect("republication");
                assert_ne!(second, first);
                assert_eq!(fn_ptr_of(second), fn_ptr);
                binds(second);

                // Another executable identity is never admitted under this name.
                let foreign = resolve_intrinsic_func(_py, "molt_capabilities_trusted", true)
                    .expect("foreign materialization");
                unsafe { dict_set_in_place(_py, registry, canonical_key, foreign) };
                let third = resolve_intrinsic_func(_py, alias, true).expect("republication");
                assert!(third != foreign && third != second);
                assert_eq!(fn_ptr_of(third), fn_ptr);
                binds(third);

                // Public metadata writes reject before changing the native
                // callable, so the existing valid materialization is retained.
                reject_user_defaults(_py, third, 1);
                let fourth = resolve_intrinsic_func(_py, canonical, true).expect("reuse");
                assert_eq!(fourth, third);
                assert_eq!(fn_ptr_of(fourth), fn_ptr);
                binds(fourth);

                for bits in [
                    first,
                    second,
                    third,
                    fourth,
                    foreign,
                    canonical_key,
                    alias_key,
                    owner_bits,
                ] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            });
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "reuse compares callable executable addresses")]
    fn public_dictionary_cannot_forge_intrinsic_binding_metadata() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner = construct_registry_owner(_py);
                let name = "molt_capabilities_has";
                // Public dictionary spellings never replace the actual typed
                // binder owner or invalidate an unchanged intrinsic callable.
                for field in [
                    "__defaults__",
                    "__kwdefaults__",
                    "__molt_arg_names__",
                    "__molt_posonly__",
                    "__molt_kwonly_names__",
                    "__molt_vararg__",
                    "__molt_varkw__",
                    "__molt_bind_kind__",
                ] {
                    let original = resolve_intrinsic_func(_py, name, true).unwrap();
                    let ptr = obj_from_bits(original).as_ptr().unwrap();
                    unsafe {
                        assert!(crate::call::class_init::function_set_attr_name(
                            _py,
                            ptr,
                            b"note",
                            MoltObject::from_int(42).bits(),
                        ));
                        // An unrelated attribute must not invalidate reuse.
                        assert_eq!(resolve_intrinsic_func(_py, name, true), Ok(original));
                        dec_ref_bits(_py, original);
                        let dictionary = obj_from_bits(crate::function_dict_bits(ptr))
                            .as_ptr()
                            .unwrap();
                        let key = string_bits(_py, field);
                        dict_set_in_place(_py, dictionary, key, MoltObject::from_int(1).bits());
                        assert_eq!(mutation_version(original), 0, "{field}");
                        let replacement = resolve_intrinsic_func(_py, name, true).unwrap();
                        assert_eq!(replacement, original, "{field}");
                        assert_eq!(fn_ptr_of(replacement), fn_ptr_of(original));
                        assert_eq!(
                            bound_value(_py, runtime_registry(_py), name),
                            Some(replacement)
                        );
                        dec_ref_bits(_py, key);
                        dec_ref_bits(_py, replacement);
                    }
                    dec_ref_bits(_py, original);
                }
                dec_ref_bits(_py, owner);
                assert!(!exception_pending(_py));
            });
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reuse compares callable addresses from separate fn-pointer casts, which Miri's provenance model exposes as distinct"
    )]
    fn failed_registry_binding_is_atomic_and_preserves_identity() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let module_dict = module_dict_ptr(obj_from_bits(owner_bits).as_ptr().unwrap())
                    .expect("anchor namespace");
                let registry = runtime_registry(_py);
                let registry_bits = MoltObject::from_ptr(registry).bits();
                let alias = "_molt_capabilities_has";
                let first = resolve_intrinsic_func(_py, alias, true).expect("publication");
                let alias_key = string_bits(_py, alias);
                assert!(unsafe { crate::object::ops::dict_del_in_place(_py, registry, alias_key) });
                // Exhaust entry storage so rebinding the alias must grow it.
                let spare = || unsafe {
                    crate::dict_order(registry).capacity() - crate::dict_order(registry).len()
                };
                let mut filler = 0;
                while spare() >= 2 {
                    let key = string_bits(_py, &format!("registry_filler_{filler}"));
                    unsafe { dict_set_in_place(_py, registry, key, MoltObject::none().bits()) };
                    dec_ref_bits(_py, key);
                    filler += 1;
                }
                let entries = unsafe { crate::dict_order(registry).clone() };
                let denial = deny_allocation();
                let denied = resolve_intrinsic_func(_py, alias, true);
                drop(denial);
                assert_eq!(denied, Err(IntrinsicResolveError::AllocFailed));
                assert!(exception_pending(_py));
                let _ = crate::molt_exception_clear();
                assert_eq!(
                    unsafe { crate::dict_order(registry).as_slice() },
                    entries.as_slice()
                );
                assert_eq!(
                    bound_value(_py, module_dict, REGISTRY_NAME),
                    Some(registry_bits)
                );
                assert_eq!(resolve_intrinsic_func(_py, alias, true), Ok(first));
                assert_eq!(bound_value(_py, registry, alias), Some(first));
                for bits in [first, first, alias_key, owner_bits] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            });
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reuse compares callable addresses from separate fn-pointer casts, which Miri's provenance model exposes as distinct"
    )]
    fn same_hash_registry_key_leaves_the_decision_to_ordinary_lookup() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let registry = runtime_registry(_py);
                let canonical = "molt_capabilities_has";
                // A key of another kind stored under the canonical spelling's
                // hash: the allocation-free read cannot decide past it.
                let forged = MoltObject::from_int(9).bits();
                let marker = MoltObject::from_int(42).bits();
                unsafe {
                    crate::object::ops::dict_set_with_hash_in_place(
                        _py,
                        registry,
                        forged,
                        marker,
                        hash_string_bytes(_py, canonical.as_bytes()) as u64,
                    );
                    assert_eq!(
                        exact_name_value(_py, registry, canonical.as_bytes()),
                        Err(())
                    );
                }
                let first = resolve_intrinsic_func(_py, canonical, true).expect("publication");
                assert_eq!(
                    resolve_intrinsic_func(_py, canonical, true),
                    Ok(first),
                    "ordinary lookup keeps the published identity"
                );
                assert_eq!(bound_value(_py, registry, canonical), Some(first));
                assert_eq!(
                    bound_value(_py, registry, "_molt_capabilities_has"),
                    Some(first)
                );
                let order = unsafe { crate::dict_order(registry) };
                assert!(
                    order
                        .chunks_exact(2)
                        .any(|entry| entry[0] == forged && entry[1] == marker)
                );
                for bits in [first, first, owner_bits] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            });
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reuse compares callable addresses from separate fn-pointer casts, which Miri's provenance model exposes as distinct"
    )]
    fn uncached_resolution_is_private_to_its_caller() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let registry = runtime_registry(_py);
                let canonical = "molt_operator_length_hint";
                let shared = resolve_intrinsic_func(_py, canonical, true).expect("publication");
                let private =
                    resolve_intrinsic_func(_py, canonical, false).expect("private callable");
                assert_ne!(private, shared);
                assert_eq!(bound_value(_py, registry, canonical), Some(shared));
                // Private and published handles use the same default authority.
                // Neither permits public writes to its internal binder metadata.
                reject_user_defaults(_py, private, 3);
                let spec = find_spec(canonical).unwrap();
                for function in [private, shared] {
                    assert!(unsafe {
                        function_defaults_are(
                            _py,
                            obj_from_bits(function).as_ptr().unwrap(),
                            spec.defaults,
                        )
                    });
                    assert_eq!(mutation_version(function), 0);
                }
                assert_eq!(resolve_intrinsic_func(_py, canonical, true), Ok(shared));
                for bits in [shared, shared, private, owner_bits] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            });
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reuse compares callable addresses from separate fn-pointer casts, which Miri's provenance model exposes as distinct"
    )]
    fn module_namespaces_bind_one_runtime_registry() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let anchor = obj_from_bits(owner_bits).as_ptr().unwrap();
                let anchor_dict = module_dict_ptr(anchor).expect("anchor namespace");
                let registry = runtime_registry(_py);
                let registry_bits = MoltObject::from_ptr(registry).bits();
                let entries = unsafe { crate::dict_len(registry) };
                let name_bits = string_bits(_py, "registry_consumer");
                let module_bits = crate::builtins::modules::molt_module_new(name_bits);
                assert!(!exception_pending(_py));
                let module_dict = module_dict_ptr(obj_from_bits(module_bits).as_ptr().unwrap())
                    .expect("consumer namespace");
                // Later namespaces reference the one registry; nothing is rebuilt.
                assert_eq!(
                    bound_value(_py, module_dict, REGISTRY_NAME),
                    Some(registry_bits)
                );
                assert_eq!(unsafe { crate::dict_len(registry) }, entries);
                for flag in [STRICT_FLAG, RUNTIME_FLAG] {
                    assert_eq!(
                        bound_value(_py, module_dict, flag),
                        Some(MoltObject::from_bool(true).bits())
                    );
                }
                #[cfg(not(target_arch = "wasm32"))]
                assert_eq!(
                    bound_value(_py, module_dict, LOOKUP_HELPER_NAME),
                    bound_value(_py, registry, LAZY_RESOLVE_NAME),
                    "native namespaces share the registry's resolver callable"
                );
                assert_eq!(
                    runtime_state(_py)
                        .intrinsic_registry_module
                        .load(Ordering::Acquire),
                    anchor
                );

                // Unbinding the anchor's registry is authoritative: resolution
                // stops publishing and the anchor namespace is never refilled.
                let registry_key = string_bits(_py, REGISTRY_NAME);
                assert!(unsafe {
                    crate::object::ops::dict_del_in_place(_py, anchor_dict, registry_key)
                });
                let private = resolve_intrinsic_func(_py, "molt_capabilities_has", true)
                    .expect("private callable");
                assert_eq!(bound_value(_py, registry, "molt_capabilities_has"), None);
                assert_eq!(bound_value(_py, anchor_dict, REGISTRY_NAME), None);
                for bits in [private, registry_key, module_bits, name_bits, owner_bits] {
                    dec_ref_bits(_py, bits);
                }
                assert!(!exception_pending(_py));
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_registry_reuses_one_lazy_resolver_callable() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let owner_bits = construct_registry_owner(_py);
                let registry_module = runtime_state(_py)
                    .intrinsic_registry_module
                    .load(Ordering::Acquire);
                assert!(!registry_module.is_null());
                let module_dict = module_dict_ptr(registry_module).expect("module dictionary");
                let lookup_bits = named_dict_value(_py, module_dict, LOOKUP_HELPER_NAME.as_bytes())
                    .unwrap()
                    .expect("module lazy resolver");
                let registry_bits = named_dict_value(_py, module_dict, REGISTRY_NAME.as_bytes())
                    .unwrap()
                    .expect("intrinsic registry");
                let registry_ptr = obj_from_bits(registry_bits).as_ptr().unwrap();
                let resolver_bits = named_dict_value(_py, registry_ptr, b"_molt_lazy_resolve")
                    .unwrap()
                    .expect("registry lazy resolver");

                assert_eq!(
                    lookup_bits, resolver_bits,
                    "module lookup and registry fallback must share one callable",
                );
                assert!(!exception_pending(_py));
                dec_ref_bits(_py, owner_bits);
            });
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(
        miri,
        ignore = "function pointer identity assertion via `as *const () as usize as u64` is not supported under Miri's pointer-provenance model: separate casts of the same fn-pointer expose distinct addresses, so the stored payload won't equal a freshly-cast comparator"
    )]
    fn register_intrinsics_module_exports_public_helpers() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        // Pending exceptions from prior parallel tests cause
        // `molt_load_intrinsic_runtime`'s wrapper to early-return None via
        // the GIL macro's exception fast-path; explicitly clear before
        // exercising the resolver to keep this test independent of order.
        let _ = crate::molt_exception_clear();
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
            let intrinsic_name_ptr = alloc_string(_py, b"molt_capabilities_has");
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
            #[cfg(feature = "molt_gpu_primitives")]
            {
                let split_ptr = obj_from_bits(split_bits)
                    .as_ptr()
                    .expect("split intrinsic should resolve to a function");
                assert_eq!(
                    unsafe { object_type_id(split_ptr) },
                    crate::TYPE_ID_FUNCTION
                );
                assert_eq!(
                    unsafe { crate::function_fn_ptr(split_ptr) },
                    crate::molt_gpu_tensor__tensor_linear_split_last_dim as *const () as usize
                        as u64
                );
            }
            #[cfg(not(feature = "molt_gpu_primitives"))]
            assert!(
                obj_from_bits(split_bits).is_none(),
                "disabled GPU intrinsic must not resolve"
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

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn register_intrinsics_module_repairs_existing_cache_entry() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let _ = crate::molt_exception_clear();
        crate::with_gil_entry_nopanic!(_py, {
            let module_name_ptr = alloc_string(_py, b"_intrinsics");
            assert!(!module_name_ptr.is_null());
            let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
            let _ = crate::builtins::modules::molt_module_cache_del(module_name_bits);
            let _ = crate::molt_exception_clear();

            let generic_module_bits = crate::builtins::modules::molt_module_new(module_name_bits);
            let generic_module_ptr = obj_from_bits(generic_module_bits)
                .as_ptr()
                .expect("generic _intrinsics module allocation should succeed");
            let generic_dict_bits = unsafe { module_dict_bits(generic_module_ptr) };
            assert_eq!(
                unsafe { object_type_id(generic_module_ptr) },
                TYPE_ID_MODULE
            );
            let set_bits = crate::builtins::modules::molt_module_cache_set(
                module_name_bits,
                generic_module_bits,
            );
            assert!(obj_from_bits(set_bits).is_none());
            assert!(!crate::exception_pending(_py));

            register_intrinsics_module(_py);

            let cached_bits = crate::builtins::modules::molt_module_cache_get(module_name_bits);
            assert_eq!(cached_bits, generic_module_bits);
            assert_eq!(
                unsafe { module_dict_bits(generic_module_ptr) },
                generic_dict_bits,
                "repair must preserve the existing module dictionary identity",
            );

            let runtime_active_name_ptr = alloc_string(_py, b"runtime_active");
            assert!(!runtime_active_name_ptr.is_null());
            let runtime_active_name_bits = MoltObject::from_ptr(runtime_active_name_ptr).bits();
            let runtime_active_bits =
                crate::molt_get_attr_name(cached_bits, runtime_active_name_bits);
            let runtime_active_ptr = obj_from_bits(runtime_active_bits)
                .as_ptr()
                .expect("cached _intrinsics module should be repaired in place");
            assert_eq!(
                unsafe { object_type_id(runtime_active_ptr) },
                crate::TYPE_ID_FUNCTION
            );
            assert!(!crate::exception_pending(_py));

            dec_ref_bits(_py, runtime_active_bits);
            dec_ref_bits(_py, runtime_active_name_bits);
            dec_ref_bits(_py, cached_bits);
            dec_ref_bits(_py, generic_module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }

    fn reset_manifest_publication_for_test() {
        test_manifest_state().store(PUBLICATION_EMPTY, Ordering::SeqCst);
        test_manifest_ptr().store(core::ptr::null_mut(), Ordering::SeqCst);
        test_manifest_len().store(0, Ordering::SeqCst);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "molt_set_intrinsic_manifest stores a wasm linear-memory address represented as u64; native Miri strict provenance cannot model this wasm-only pointer contract"
    )]
    fn manifest_one_shot_guard() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        reset_manifest_publication_for_test();

        let malformed_len = u64::from(u32::MAX) + 1;
        assert_eq!(molt_set_intrinsic_manifest(0x1000, malformed_len), 1);
        assert_eq!(
            test_manifest_state().load(Ordering::SeqCst),
            PUBLICATION_EMPTY,
            "invalid input must not poison the one-shot slot"
        );

        let ret = molt_set_intrinsic_manifest(0x1000, 10);
        assert_eq!(ret, 0, "first call should return 0 (success)");
        assert_eq!(
            test_manifest_state().load(Ordering::SeqCst),
            PUBLICATION_READY,
            "manifest payload should be ready after first call"
        );
        assert_eq!(
            test_manifest_ptr().load(Ordering::SeqCst) as usize,
            0x1000,
            "INTRINSIC_MANIFEST_PTR should be 0x1000 after first call"
        );

        let ret2 = molt_set_intrinsic_manifest(0x2000, 20);
        assert_eq!(ret2, 0, "second call should also return 0");
        assert_eq!(
            test_manifest_ptr().load(Ordering::SeqCst) as usize,
            0x1000,
            "INTRINSIC_MANIFEST_PTR must still be 0x1000, NOT 0x2000"
        );
    }

    #[test]
    fn empty_manifest_is_ready_and_distinct_from_unset() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        reset_manifest_publication_for_test();
        assert_eq!(manifest_snapshot(), None);

        assert_eq!(molt_set_intrinsic_manifest(0, 0), 0);
        assert_eq!(
            manifest_snapshot().map(|(ptr, len)| (ptr.is_null(), len)),
            Some((true, 0))
        );
        assert_eq!(manifest_bytes(), Some(&[][..]));
        assert_eq!(
            test_manifest_state().load(Ordering::Acquire),
            PUBLICATION_READY
        );
    }

    #[test]
    fn manifest_payload_is_hidden_until_release_publication() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        reset_manifest_publication_for_test();
        let first_payload = Box::leak(Box::new([1_u8, 2, 3, 4]));
        let second_payload = Box::leak(Box::new([9_u8, 8]));
        let first_address = first_payload.as_mut_ptr() as usize;
        let second_address = second_payload.as_mut_ptr() as usize;
        let (winner_claimed_tx, winner_claimed_rx) = std::sync::mpsc::channel();
        let (publish_tx, publish_rx) = std::sync::mpsc::channel();
        let (loser_waiting_tx, loser_waiting_rx) = std::sync::mpsc::channel();
        let (loser_done_tx, loser_done_rx) = std::sync::mpsc::channel();

        let winner = std::thread::spawn(move || {
            publish_manifest(
                core::ptr::with_exposed_provenance_mut(first_address),
                4,
                || {
                    winner_claimed_tx.send(()).unwrap();
                    publish_rx.recv().unwrap();
                },
                || unreachable!("CAS winner cannot enter the competing path"),
            )
        });
        winner_claimed_rx.recv().unwrap();
        assert_eq!(
            test_manifest_state().load(Ordering::Acquire),
            PUBLICATION_INITIALIZING
        );
        assert_eq!(
            manifest_snapshot(),
            None,
            "readers must not see split payload"
        );

        let loser = std::thread::spawn(move || {
            let result = publish_manifest(
                core::ptr::with_exposed_provenance_mut(second_address),
                2,
                || unreachable!("competing setter cannot win after the claim"),
                || loser_waiting_tx.send(()).unwrap(),
            );
            loser_done_tx.send(()).unwrap();
            result
        });
        loser_waiting_rx.recv().unwrap();
        assert!(
            loser_done_rx.try_recv().is_err(),
            "loser returned before READY"
        );
        publish_tx.send(()).unwrap();
        assert_eq!(winner.join().unwrap(), 0);
        assert_eq!(loser.join().unwrap(), 0);
        assert_eq!(manifest_snapshot(), Some((first_payload.as_mut_ptr(), 4)));
    }

    /// A fake per-app resolver that returns a sentinel address for one known
    /// name and 0 (not found) for everything else, matching the ABI of the
    /// backend-emitted app callable resolver.
    extern "C" fn fake_app_resolver(name_ptr: *const u8, name_len: usize) -> u64 {
        let bytes = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
        if bytes == b"molt_known_intrinsic" {
            0xDEAD_BEEF
        } else {
            0
        }
    }

    /// When a resolver is registered, `try_app_resolve_symbol` must delegate to
    /// it: hits return the resolver's address, misses return `None`. It must NOT
    /// fall back to `resolve_symbol` on native (that path is left dead-strippable).
    #[test]
    #[cfg_attr(
        miri,
        ignore = "fn-pointer-as-u64 sentinel address comparison is not modelled under Miri's strict provenance"
    )]
    fn app_resolver_used_when_registered() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        test_app_resolver_address().store(0, Ordering::SeqCst);

        let ret = molt_set_app_callable_resolver(crate::provenance::abi::expose_function_address(
            fake_app_resolver as *const (),
        ));
        assert_eq!(ret, 0, "first registration should return 0 (success)");

        assert_eq!(
            try_app_resolve_symbol("molt_known_intrinsic"),
            Some(0xDEAD_BEEF),
            "registered resolver must be consulted for known names"
        );
        assert_eq!(
            try_app_resolve_symbol("molt_unknown_intrinsic_xyz"),
            None,
            "registered resolver returning 0 must surface as None"
        );
        for symbol in ["molt_len", "molt_vars_builtin", "molt_globals_builtin"] {
            assert_eq!(
                try_app_resolve_symbol(symbol),
                None,
                "installed resolver misses must not fall back to test fixtures: {symbol}"
            );
        }

        // Clean up for other tests sharing the process-global statics.
        test_app_resolver_address().store(0, Ordering::SeqCst);
    }

    /// The resolver registration is one-shot: a second call must be ignored so a
    /// later (e.g. attacker-controlled or re-entrant) registration cannot
    /// override the compiler-generated resolver installed by the main stub.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "fn-pointer-as-u64 sentinel address comparison is not modelled under Miri's strict provenance"
    )]
    fn app_resolver_one_shot_guard() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        test_app_resolver_address().store(0, Ordering::SeqCst);

        let first = molt_set_app_callable_resolver(0x1000);
        assert_eq!(first, 0, "first registration should return 0 (success)");
        assert_eq!(
            test_app_resolver_address().load(Ordering::SeqCst),
            0x1000,
            "resolver address should be 0x1000 after first registration"
        );

        // Second registration must be silently ignored.
        let second = molt_set_app_callable_resolver(0x2000);
        assert_eq!(second, 0, "second registration should also return 0");
        assert_eq!(
            test_app_resolver_address().load(Ordering::SeqCst),
            0x1000,
            "resolver address must still be 0x1000, NOT 0x2000"
        );

        // Clean up.
        test_app_resolver_address().store(0, Ordering::SeqCst);
    }
}
