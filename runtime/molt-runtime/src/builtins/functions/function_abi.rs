// Runtime callable, function, code-object, bound-method, and closure ABI.
// This module owns function pointer canonicalization together with the
// exported constructors that consume it, so call target identity has one
// authority instead of a registry/function split across functions.rs.

use super::wasm_callables_generated as wasm_callables;
use super::*;
use crate::ClassEdgeOwnership;
use crate::object::{object_init_class_edge_unpublished, object_replace_class_edge};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

#[derive(Copy, Clone)]
struct NativeCallableTarget(*const ());

// Native callable targets are immutable code pointers published once from Rust.
unsafe impl Send for NativeCallableTarget {}
unsafe impl Sync for NativeCallableTarget {}

fn native_callable_targets() -> &'static Mutex<HashMap<u64, NativeCallableTarget>> {
    static TARGETS: OnceLock<Mutex<HashMap<u64, NativeCallableTarget>>> = OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn runtime_callable_name_suffix(symbol_path: &str) -> &str {
    symbol_path.rsplit("::").next().unwrap_or(symbol_path)
}

pub(crate) fn runtime_fn_addr(symbol_path: &str, raw_ptr: *const ()) -> u64 {
    let key = runtime_callable_key_from_symbol_name(runtime_callable_name_suffix(symbol_path))
        .unwrap_or_else(|| crate::provenance::abi::expose_function_address(raw_ptr));
    if !raw_ptr.is_null() {
        let mut guard = native_callable_targets().lock().unwrap();
        guard.entry(key).or_insert(NativeCallableTarget(raw_ptr));
    }
    key
}

fn runtime_callable_key_from_symbol_name(symbol_name: &str) -> Option<u64> {
    wasm_callables::runtime_callable_key_from_symbol_name(symbol_name)
}

pub(crate) fn canonicalize_runtime_callable_key(fn_ptr: u64) -> u64 {
    fn_ptr
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_info(
    fn_ptr: u64,
) -> Option<(u64, &'static str, &'static str, usize)> {
    wasm_callables::reserved_wasm_runtime_callable_info(fn_ptr)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_info_for_table_idx(
    table_idx: u64,
) -> Option<(u64, &'static str, &'static str, usize)> {
    let base = crate::wasm_table_base();
    let start = base + wasm_callables::RESERVED_WASM_RUNTIME_CALLABLE_BASE;
    let idx = table_idx.checked_sub(start)?;
    if idx >= wasm_callables::RESERVED_WASM_RUNTIME_CALLABLE_COUNT {
        return None;
    }
    wasm_callables::RESERVED_RUNTIME_CALLABLES
        .iter()
        .find(|entry| entry.index == idx)
        .map(|entry| {
            (
                entry.index,
                entry.runtime_name,
                entry.import_name,
                entry.arity,
            )
        })
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_direct_callable_info_for_table_idx(
    table_idx: u64,
) -> Option<(u64, &'static str, &'static str, usize)> {
    let info = reserved_wasm_runtime_callable_info_for_table_idx(table_idx)?;
    let dispatch = wasm_callables::reserved_wasm_runtime_callable_dispatch_for_index(info.0)?;
    if dispatch != wasm_callables::ReservedRuntimeCallableDispatch::Direct {
        return None;
    }
    Some(info)
}

/// Map a reserved-callable table index — in either the direct region OR the
/// trampoline region — to its canonical *direct* table slot, but only when the
/// callable uses `Direct` dispatch.
///
/// The fixed-arity call lanes lower a Python call of a reserved callable to
/// `molt_call_indirectN(<slot>, <N positional args>)`. The host resolves the
/// slot's region: a *direct*-region slot is invoked as an N-argument direct
/// call; a *trampoline*-region slot is invoked with the closure/argv/argc
/// convention (exactly 3 host arguments). A fixed-arity lane therefore MUST
/// target the direct region for a `Direct`-dispatch reserved callable; handing
/// it the trampoline slot makes the host reject the call as a malformed
/// trampoline. `reserved_wasm_runtime_callable_info_for_table_idx` only
/// recognizes the direct region, so a function object whose stored `fn_ptr`
/// already is the trampoline slot was silently treated as a non-reserved
/// pointer and routed straight back to the trampoline region. This closes that
/// gap by recognizing both regions and always returning the direct slot.
/// Pure region math for [`reserved_wasm_runtime_direct_slot_for_any_reserved_slot`],
/// factored out so the direct/trampoline-region reconciliation is unit-testable
/// on native targets. Given a table index that may sit in the direct region
/// (`[direct_start, direct_start + count)`) or the trampoline region
/// (`[trampoline_start, trampoline_start + count)`), returns the reserved
/// callable index it belongs to, or `None` when it is outside both regions.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn reserved_slot_index_for_any_region(
    table_idx: u64,
    direct_start: u64,
    trampoline_start: u64,
    count: u64,
) -> Option<u64> {
    table_idx
        .checked_sub(direct_start)
        .filter(|o| *o < count)
        .or_else(|| {
            table_idx
                .checked_sub(trampoline_start)
                .filter(|o| *o < count)
        })
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_direct_slot_for_any_reserved_slot(
    table_idx: u64,
) -> Option<u64> {
    let base = crate::wasm_table_base();
    let direct_start = base + wasm_callables::RESERVED_WASM_RUNTIME_CALLABLE_BASE;
    let trampoline_start = base + wasm_callables::RESERVED_WASM_RUNTIME_TRAMPOLINE_BASE;
    let count = wasm_callables::RESERVED_WASM_RUNTIME_CALLABLE_COUNT;
    let idx = reserved_slot_index_for_any_region(table_idx, direct_start, trampoline_start, count)?;
    let dispatch = wasm_callables::reserved_wasm_runtime_callable_dispatch_for_index(idx)?;
    if dispatch != wasm_callables::ReservedRuntimeCallableDispatch::Direct {
        return None;
    }
    Some(direct_start + idx)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_ptr(fn_ptr: u64) -> Option<u64> {
    let base = crate::wasm_table_base();
    reserved_wasm_runtime_callable_info(fn_ptr).map(|(idx, _sym, _import, _arity)| {
        base + wasm_callables::RESERVED_WASM_RUNTIME_CALLABLE_BASE + idx
    })
}

#[cfg(target_arch = "wasm32")]
#[inline]
pub(crate) fn normalize_runtime_callable_ptr(fn_ptr: u64) -> u64 {
    reserved_wasm_runtime_callable_ptr(fn_ptr).unwrap_or(fn_ptr)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_arity(fn_ptr: u64) -> Option<usize> {
    reserved_wasm_runtime_callable_info(fn_ptr).map(|(_idx, _sym, _import, arity)| arity)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_trampoline_ptr(fn_ptr: u64) -> Option<u64> {
    let base = crate::wasm_table_base();
    reserved_wasm_runtime_callable_info(fn_ptr).map(|(idx, _sym, _import, _arity)| {
        base + wasm_callables::RESERVED_WASM_RUNTIME_TRAMPOLINE_BASE + idx
    })
}

#[inline]
pub(crate) fn normalize_runtime_trampoline_ptr(fn_ptr: u64, tramp_ptr: u64) -> u64 {
    let _ = fn_ptr;
    #[cfg(target_arch = "wasm32")]
    {
        return reserved_wasm_runtime_trampoline_ptr(fn_ptr).unwrap_or(tramp_ptr);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tramp_ptr
    }
}

#[inline]
pub(crate) fn runtime_callable_represents_symbol(
    fn_ptr: u64,
    tramp_ptr: u64,
    symbol_fn_ptr: u64,
) -> bool {
    let _ = tramp_ptr;
    if canonicalize_runtime_callable_key(fn_ptr) == canonicalize_runtime_callable_key(symbol_fn_ptr)
    {
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if normalize_runtime_callable_ptr(fn_ptr) == normalize_runtime_callable_ptr(symbol_fn_ptr) {
            return true;
        }
        let expected_tramp_ptr = normalize_runtime_trampoline_ptr(symbol_fn_ptr, 0);
        if expected_tramp_ptr != 0
            && normalize_runtime_trampoline_ptr(fn_ptr, tramp_ptr) == expected_tramp_ptr
        {
            return true;
        }
    }
    false
}

pub(crate) fn runtime_callable_target_ptr(fn_ptr: u64) -> Option<*const ()> {
    if let Some(target) = native_callable_targets()
        .lock()
        .unwrap()
        .get(&fn_ptr)
        .copied()
    {
        return Some(target.0);
    }
    wasm_callables::runtime_callable_target_ptr(fn_ptr)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn runtime_callable_returns_void(fn_ptr: u64) -> bool {
    wasm_callables::runtime_callable_returns_void_from_target_ptr(fn_ptr)
}

#[inline]
unsafe fn init_runtime_callable_function_obj(
    ptr: *mut u8,
    fn_key: u64,
    raw_fn_ptr: u64,
    trampoline_ptr: u64,
    native_direct_target: bool,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(call_target) = runtime_callable_target_ptr(fn_key) {
        unsafe {
            function_set_call_target_ptr(ptr, call_target);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = fn_key;
    #[cfg(target_arch = "wasm32")]
    let _ = native_direct_target;
    #[cfg(not(target_arch = "wasm32"))]
    if native_direct_target
        && unsafe { function_call_target_ptr(ptr) }.is_null()
        && raw_fn_ptr != 0
        && let Some(raw_target) = crate::provenance::abi::function_ptr(raw_fn_ptr)
    {
        unsafe {
            function_set_call_target_ptr(ptr, raw_target);
        }
    }
    let normalized_trampoline_ptr = normalize_runtime_trampoline_ptr(raw_fn_ptr, trampoline_ptr);
    if normalized_trampoline_ptr != 0 {
        unsafe {
            function_set_trampoline_ptr(ptr, normalized_trampoline_ptr);
        }
    }
}

pub(crate) fn alloc_runtime_function_obj(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    arity: u64,
) -> *mut u8 {
    let fn_key = canonicalize_runtime_callable_key(fn_ptr);
    #[cfg(not(miri))]
    if fn_key == fn_ptr
        && fn_ptr != 0
        && let Some(raw_target) = crate::provenance::abi::function_ptr(fn_ptr)
    {
        let mut guard = native_callable_targets().lock().unwrap();
        guard
            .entry(fn_key)
            .or_insert(NativeCallableTarget(raw_target));
    }
    let ptr = alloc_function_obj(_py, fn_key, arity);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        init_runtime_callable_function_obj(ptr, fn_key, fn_ptr, 0, false);
    }
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fn_key = canonicalize_runtime_callable_key(fn_ptr);
        let trace = matches!(
            std::env::var("MOLT_TRACE_FUNC_NEW").ok().as_deref(),
            Some("1")
        );
        if trace {
            eprintln!(
                "molt func new: fn_ptr={fn_ptr} fn_key={fn_key} tramp_ptr={trampoline_ptr} arity={arity}"
            );
        }
        let ptr = alloc_function_obj(_py, fn_key, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                init_runtime_callable_function_obj(ptr, fn_key, fn_ptr, trampoline_ptr, true);
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_builtin(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        molt_func_new_builtin_raw_impl(_py, fn_ptr, trampoline_ptr, arity)
    })
}

fn molt_func_new_builtin_raw_impl(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
) -> u64 {
    let fn_key = canonicalize_runtime_callable_key(fn_ptr);
    #[cfg(not(miri))]
    if fn_key == fn_ptr
        && fn_ptr != 0
        && let Some(raw_target) = crate::provenance::abi::function_ptr(fn_ptr)
    {
        let mut guard = native_callable_targets().lock().unwrap();
        guard
            .entry(fn_key)
            .or_insert(NativeCallableTarget(raw_target));
    }
    let trace = matches!(
        std::env::var("MOLT_TRACE_BUILTIN_FUNC").ok().as_deref(),
        Some("1")
    );
    let trace_enter_ptr = fn_addr!(molt_trace_enter_slot);
    if trace {
        eprintln!(
            "molt builtin_func new: fn_ptr=0x{fn_ptr:x} tramp_ptr=0x{trampoline_ptr:x} arity={arity}"
        );
    }
    if fn_ptr == 0 || trampoline_ptr == 0 {
        let msg = format!(
            "builtin func pointer missing: fn=0x{fn_ptr:x} tramp=0x{trampoline_ptr:x} arity={arity}"
        );
        return raise_exception::<_>(_py, "RuntimeError", &msg);
    }
    let ptr = alloc_function_obj(_py, fn_key, arity);
    if ptr.is_null() {
        return raise_exception::<_>(_py, "RuntimeError", "builtin func alloc failed");
    }
    unsafe {
        init_runtime_callable_function_obj(ptr, fn_key, fn_ptr, trampoline_ptr, false);
        let builtin_bits = builtin_classes(_py).builtin_function_or_method;
        if !object_init_class_edge_unpublished(_py, ptr, builtin_bits, ClassEdgeOwnership::Owned) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return MoltObject::none().bits();
        }
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    if trace && fn_ptr == trace_enter_ptr {
        eprintln!(
            "molt builtin_func trace_enter_slot bits=0x{bits:x} ptr=0x{:x}",
            ptr as usize
        );
    }
    bits
}

pub(crate) fn python_builtin_function_bits(
    _py: &crate::PyToken<'_>,
    python_name: &str,
) -> Option<u64> {
    let info = wasm_callables::python_builtin_function_info(python_name)?;
    let slots = python_builtin_function_slots(_py);
    let slot = slots.get(info.index)?;
    let cached_bits = slot.load(AtomicOrdering::Acquire);
    if cached_bits != 0 {
        inc_ref_bits(_py, cached_bits);
        return Some(cached_bits);
    }
    let bits = alloc_python_builtin_function_bits(_py, info)?;
    match slot.compare_exchange(0, bits, AtomicOrdering::AcqRel, AtomicOrdering::Acquire) {
        Ok(_) => {
            inc_ref_bits(_py, bits);
            Some(bits)
        }
        Err(existing_bits) => {
            dec_ref_bits(_py, bits);
            if existing_bits == 0 {
                return None;
            }
            inc_ref_bits(_py, existing_bits);
            Some(existing_bits)
        }
    }
}

pub(crate) fn python_builtin_functions_clear_runtime_state(
    _py: &crate::PyToken<'_>,
    state: &crate::state::RuntimeState,
) {
    if let Some(slots) = state.python_builtin_function_slots.get() {
        let slot_refs: Vec<&AtomicU64> = slots.iter().collect();
        crate::state::cache::clear_atomic_slots(_py, &slot_refs);
    }
}

fn python_builtin_function_slots(_py: &crate::PyToken<'_>) -> &'static [AtomicU64] {
    crate::runtime_state(_py)
        .python_builtin_function_slots
        .get_or_init(|| {
            (0..wasm_callables::PYTHON_BUILTIN_FUNCTION_COUNT)
                .map(|_| AtomicU64::new(0))
                .collect()
        })
        .as_slice()
}

fn alloc_python_builtin_function_bits(
    _py: &crate::PyToken<'_>,
    info: wasm_callables::PythonBuiltinFunctionInfo,
) -> Option<u64> {
    let fn_ptr = python_builtin_function_target(info.runtime_name)?;
    let ptr = alloc_runtime_function_obj(_py, fn_ptr, info.arity);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let builtin_bits = builtin_classes(_py).builtin_function_or_method;
        if !object_init_class_edge_unpublished(_py, ptr, builtin_bits, ClassEdgeOwnership::Owned) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return None;
        }
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    if !init_python_builtin_function_metadata(_py, bits, info) {
        dec_ref_bits(_py, bits);
        return None;
    }
    Some(bits)
}

fn python_builtin_function_target(runtime_name: &str) -> Option<u64> {
    #[cfg(target_arch = "wasm32")]
    {
        crate::intrinsics::registry::try_app_resolve_runtime_callable(runtime_name)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let raw_ptr = wasm_callables::python_builtin_function_target_ptr(runtime_name)?;
        Some(runtime_fn_addr(runtime_name, raw_ptr))
    }
}

fn init_python_builtin_function_metadata(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    info: wasm_callables::PythonBuiltinFunctionInfo,
) -> bool {
    let Some(metadata_bits) = alloc_python_builtin_metadata_tuple_bits(_py, info) else {
        return false;
    };
    let bind_kind_bits = info
        .bind_kind
        .map(|kind| MoltObject::from_int(kind).bits())
        .unwrap_or_else(|| MoltObject::none().bits());
    let result = molt_function_init_metadata_packed(
        func_bits,
        metadata_bits,
        MoltObject::none().bits(),
        bind_kind_bits,
    );
    dec_ref_bits(_py, metadata_bits);
    obj_from_bits(result).is_none() && !exception_pending(_py)
}

struct OwnedMetadataBits<'a, 'py> {
    py: &'a crate::PyToken<'py>,
    bits: Vec<u64>,
}

impl<'a, 'py> OwnedMetadataBits<'a, 'py> {
    fn new(py: &'a crate::PyToken<'py>) -> Self {
        Self {
            py,
            bits: Vec::new(),
        }
    }

    fn push(&mut self, bits: u64) {
        self.bits.push(bits);
    }
}

impl Drop for OwnedMetadataBits<'_, '_> {
    fn drop(&mut self) {
        for bits in self.bits.drain(..) {
            dec_ref_bits(self.py, bits);
        }
    }
}

fn alloc_python_builtin_metadata_tuple_bits(
    _py: &crate::PyToken<'_>,
    info: wasm_callables::PythonBuiltinFunctionInfo,
) -> Option<u64> {
    let mut owned = OwnedMetadataBits::new(_py);

    let name_bits = alloc_owned_static_str_bits(_py, info.python_name, &mut owned)?;
    let qualname_bits = alloc_owned_static_str_bits(_py, info.python_name, &mut owned)?;
    let module_bits = alloc_owned_static_str_bits(_py, "builtins", &mut owned)?;
    let arg_names_bits = alloc_static_str_tuple_bits(
        _py,
        info.posonly_params
            .iter()
            .chain(info.pos_or_kw_params.iter())
            .copied(),
        &mut owned,
    )?;
    let posonly_bits = MoltObject::from_int(info.posonly_params.len() as i64).bits();
    let kwonly_bits =
        alloc_static_str_tuple_bits(_py, info.kwonly_params.iter().copied(), &mut owned)?;
    let vararg_bits = alloc_optional_static_str_bits(_py, info.vararg, &mut owned)?;
    let varkw_bits = alloc_optional_static_str_bits(_py, info.varkw, &mut owned)?;
    let defaults_bits = alloc_generated_defaults_tuple_bits(_py, info.defaults, &mut owned)?;
    let kwdefaults_bits = alloc_generated_kwdefaults_dict_bits(_py, info.kw_defaults, &mut owned)?;
    let doc_bits = MoltObject::none().bits();
    let metadata_ptr = alloc_tuple(
        _py,
        &[
            name_bits,
            qualname_bits,
            module_bits,
            arg_names_bits,
            posonly_bits,
            kwonly_bits,
            vararg_bits,
            varkw_bits,
            defaults_bits,
            kwdefaults_bits,
            doc_bits,
        ],
    );
    if metadata_ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(metadata_ptr).bits())
    }
}

fn alloc_owned_static_str_bits(
    _py: &crate::PyToken<'_>,
    value: &str,
    owned: &mut OwnedMetadataBits<'_, '_>,
) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    owned.push(bits);
    Some(bits)
}

fn alloc_optional_static_str_bits(
    _py: &crate::PyToken<'_>,
    value: Option<&'static str>,
    owned: &mut OwnedMetadataBits<'_, '_>,
) -> Option<u64> {
    match value {
        Some(value) => alloc_owned_static_str_bits(_py, value, owned),
        None => Some(MoltObject::none().bits()),
    }
}

fn alloc_static_str_tuple_bits<'a>(
    _py: &crate::PyToken<'_>,
    values: impl Iterator<Item = &'a str>,
    owned: &mut OwnedMetadataBits<'_, '_>,
) -> Option<u64> {
    let mut elems: Vec<u64> = Vec::new();
    for value in values {
        elems.push(alloc_owned_static_str_bits(_py, value, owned)?);
    }
    let ptr = alloc_tuple(_py, &elems);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    owned.push(bits);
    Some(bits)
}

fn generated_default_value_bits(
    _py: &crate::PyToken<'_>,
    value: wasm_callables::GeneratedBuiltinDefaultValue,
    owned: &mut OwnedMetadataBits<'_, '_>,
) -> Option<u64> {
    match value {
        wasm_callables::GeneratedBuiltinDefaultValue::Missing => Some(missing_bits(_py)),
        wasm_callables::GeneratedBuiltinDefaultValue::None => Some(MoltObject::none().bits()),
        wasm_callables::GeneratedBuiltinDefaultValue::Bool(value) => {
            Some(MoltObject::from_bool(value).bits())
        }
        wasm_callables::GeneratedBuiltinDefaultValue::Int(value) => {
            Some(MoltObject::from_int(value).bits())
        }
        wasm_callables::GeneratedBuiltinDefaultValue::Str(value) => {
            alloc_owned_static_str_bits(_py, value, owned)
        }
    }
}

fn alloc_generated_defaults_tuple_bits(
    _py: &crate::PyToken<'_>,
    defaults: &[wasm_callables::GeneratedBuiltinDefaultValue],
    owned: &mut OwnedMetadataBits<'_, '_>,
) -> Option<u64> {
    if defaults.is_empty() {
        return Some(MoltObject::none().bits());
    }
    let mut elems: Vec<u64> = Vec::with_capacity(defaults.len());
    for value in defaults {
        elems.push(generated_default_value_bits(_py, *value, owned)?);
    }
    let ptr = alloc_tuple(_py, &elems);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    owned.push(bits);
    Some(bits)
}

fn alloc_generated_kwdefaults_dict_bits(
    _py: &crate::PyToken<'_>,
    defaults: &[(&'static str, wasm_callables::GeneratedBuiltinDefaultValue)],
    owned: &mut OwnedMetadataBits<'_, '_>,
) -> Option<u64> {
    if defaults.is_empty() {
        return Some(MoltObject::none().bits());
    }
    let mut pairs: Vec<u64> = Vec::with_capacity(defaults.len() * 2);
    for (name, value) in defaults {
        pairs.push(alloc_owned_static_str_bits(_py, name, owned)?);
        pairs.push(generated_default_value_bits(_py, *value, owned)?);
    }
    let ptr = alloc_dict_with_pairs(_py, &pairs);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    owned.push(bits);
    Some(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_builtin_named(
    name_bits: u64,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace = matches!(
            std::env::var("MOLT_TRACE_BUILTIN_FUNC").ok().as_deref(),
            Some("1")
        );
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            if let Some(func_bits) = python_builtin_function_bits(_py, &name) {
                if trace {
                    eprintln!("molt builtin_func named: resolved python builtin {}", name);
                }
                return func_bits;
            }
            if let Some(func_bits) =
                crate::intrinsics::registry::try_resolve_intrinsic_func(_py, &name, false)
            {
                if trace {
                    eprintln!("molt builtin_func named: resolved intrinsic {}", name);
                }
                return func_bits;
            }
        }
        if trace {
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<non-str>".to_string());
            eprintln!("molt builtin_func named: fallback {}", name);
        }
        molt_func_new_builtin_raw_impl(_py, fn_ptr, trampoline_ptr, arity)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_closure(
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fn_key = canonicalize_runtime_callable_key(fn_ptr);
        let trace = matches!(
            std::env::var("MOLT_TRACE_FUNC_NEW").ok().as_deref(),
            Some("1")
        );
        if trace {
            eprintln!(
                "molt func new closure: fn_ptr={fn_ptr} tramp_ptr={trampoline_ptr} arity={arity} closure_bits={closure_bits}"
            );
        }
        let ptr = alloc_function_obj(_py, fn_key, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
            let cell_bits = cell_class(_py);
            if cell_bits != 0 && !obj_from_bits(cell_bits).is_none() {
                let closure_obj = obj_from_bits(closure_bits);
                if let Some(closure_ptr) = closure_obj.as_ptr() {
                    unsafe {
                        if object_type_id(closure_ptr) == TYPE_ID_TUPLE {
                            let Some(closure) = crate::object::seq_access::snapshot(
                                _py,
                                closure_ptr,
                                "sequence snapshot allocation failed",
                            ) else {
                                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                                return MoltObject::none().bits();
                            };
                            for &entry_bits in closure.iter() {
                                let entry_obj = obj_from_bits(entry_bits);
                                let Some(entry_ptr) = entry_obj.as_ptr() else {
                                    continue;
                                };
                                if object_type_id(entry_ptr) != TYPE_ID_LIST {
                                    continue;
                                }
                                if crate::object::seq_access::len(entry_ptr) != 1 {
                                    continue;
                                }
                                let old_class_bits = object_class_bits(entry_ptr);
                                if old_class_bits == cell_bits {
                                    continue;
                                }
                                if !object_replace_class_edge(
                                    _py,
                                    entry_ptr,
                                    cell_bits,
                                    ClassEdgeOwnership::Owned,
                                ) {
                                    dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                                    return MoltObject::none().bits();
                                }
                            }
                        }
                    }
                }
            }
        }
        unsafe {
            function_set_closure_bits(_py, ptr, closure_bits);
            init_runtime_callable_function_obj(ptr, fn_key, fn_ptr, trampoline_ptr, true);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub(crate) unsafe fn function_type_new_from_args(_py: &PyToken<'_>, args: &[u64]) -> u64 {
    unsafe {
        if args.len() < 2 || args.len() > 5 {
            let msg = format!("function expected 2 to 5 arguments, got {}", args.len());
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let Some(code_ptr) = obj_from_bits(args[0]).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "arg 1 (code) must be code");
        };
        if object_type_id(code_ptr) != TYPE_ID_CODE {
            return raise_exception::<_>(_py, "TypeError", "arg 1 (code) must be code");
        }
        let Some(globals_ptr) = obj_from_bits(args[1]).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "arg 2 (globals) must be dict");
        };
        if object_type_id(globals_ptr) != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "arg 2 (globals) must be dict");
        }

        let none_bits = MoltObject::none().bits();
        let name_bits = match args.get(2).copied() {
            Some(bits) if !obj_from_bits(bits).is_none() => {
                let Some(name_ptr) = obj_from_bits(bits).as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "arg 3 (name) must be None or string",
                    );
                };
                if object_type_id(name_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "arg 3 (name) must be None or string",
                    );
                }
                bits
            }
            _ => code_name_bits(code_ptr),
        };

        let defaults_bits = args.get(3).copied().unwrap_or(none_bits);
        if !obj_from_bits(defaults_bits).is_none() {
            let Some(defaults_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "arg 4 (defaults) must be None or tuple",
                );
            };
            if object_type_id(defaults_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "arg 4 (defaults) must be None or tuple",
                );
            }
        }

        if let Some(closure_bits) = args.get(4).copied()
            && !obj_from_bits(closure_bits).is_none()
        {
            let Some(closure_ptr) = obj_from_bits(closure_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "arg 5 (closure) must be None or tuple",
                );
            };
            if object_type_id(closure_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "arg 5 (closure) must be None or tuple",
                );
            }
            if crate::object::seq_access::len(closure_ptr) != 0 {
                return raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    "FunctionType closure cells require compiled freevar lowering",
                );
            }
        }

        let fn_ptr = code_callable_fn_ptr(code_ptr);
        if fn_ptr == 0 {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "code object has no Molt callable target",
            );
        }
        let arity = code_callable_arity(code_ptr);
        let func_ptr = alloc_function_obj(_py, fn_ptr, arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        init_runtime_callable_function_obj(
            func_ptr,
            fn_ptr,
            fn_ptr,
            code_callable_trampoline_ptr(code_ptr),
            true,
        );
        function_set_globals_bits(_py, func_ptr, args[1]);
        function_set_globals_override_enabled(func_ptr, true);
        function_set_code_bits(_py, func_ptr, args[0]);

        let set_attr = |name: &'static [u8], value_bits: u64| -> Result<(), u64> {
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, name) else {
                return Err(MoltObject::none().bits());
            };
            crate::call::class_init::function_set_attr_bits(_py, func_ptr, attr_bits, value_bits);
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        };

        let module_bits = {
            let Some(module_name_bits) = attr_name_bits_from_bytes(_py, b"__name__") else {
                return MoltObject::none().bits();
            };
            let bits = dict_get_in_place(_py, globals_ptr, module_name_bits).unwrap_or(none_bits);
            dec_ref_bits(_py, module_name_bits);
            bits
        };

        let arg_names_bits = code_arg_names_bits(code_ptr);
        let posonly_bits = code_signature_posonly_bits(code_ptr);
        let kwonly_bits = code_kwonly_names_bits(code_ptr);
        let vararg_bits = code_vararg_bits(code_ptr);
        let varkw_bits = code_varkw_bits(code_ptr);

        if set_attr(b"__name__", name_bits).is_err()
            || set_attr(b"__qualname__", name_bits).is_err()
            || set_attr(b"__module__", module_bits).is_err()
            || set_attr(b"__molt_arg_names__", arg_names_bits).is_err()
            || set_attr(b"__molt_posonly__", posonly_bits).is_err()
            || set_attr(b"__molt_kwonly_names__", kwonly_bits).is_err()
            || set_attr(b"__molt_vararg__", vararg_bits).is_err()
            || set_attr(b"__molt_varkw__", varkw_bits).is_err()
            || set_attr(b"__defaults__", defaults_bits).is_err()
            || set_attr(b"__kwdefaults__", none_bits).is_err()
            || set_attr(b"__doc__", none_bits).is_err()
        {
            return MoltObject::none().bits();
        }

        crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);

        MoltObject::from_ptr(func_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_set_builtin(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits
                && !object_replace_class_edge(
                    _py,
                    func_ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                )
            {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "function class metadata is immutable after publication",
                );
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_init_metadata(
    func_bits: u64,
    name_bits: u64,
    qualname_bits: u64,
    module_bits: u64,
    arg_names_bits: u64,
    posonly_bits: u64,
    kwonly_bits: u64,
    vararg_bits: u64,
    varkw_bits: u64,
    defaults_bits: u64,
    kwdefaults_bits: u64,
    doc_bits: u64,
    code_bits: u64,
    bind_kind_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
        }

        let set_attr = |name: &'static [u8], value_bits: u64| -> Result<(), u64> {
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, name) else {
                return Err(MoltObject::none().bits());
            };
            unsafe {
                crate::call::class_init::function_set_attr_bits(
                    _py, func_ptr, attr_bits, value_bits,
                );
            }
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        };

        if set_attr(b"__name__", name_bits).is_err()
            || set_attr(b"__qualname__", qualname_bits).is_err()
            || set_attr(b"__module__", module_bits).is_err()
            || set_attr(b"__molt_arg_names__", arg_names_bits).is_err()
            || set_attr(b"__molt_posonly__", posonly_bits).is_err()
            || set_attr(b"__molt_kwonly_names__", kwonly_bits).is_err()
            || set_attr(b"__molt_vararg__", vararg_bits).is_err()
            || set_attr(b"__molt_varkw__", varkw_bits).is_err()
            || set_attr(b"__defaults__", defaults_bits).is_err()
            || set_attr(b"__kwdefaults__", kwdefaults_bits).is_err()
            || set_attr(b"__doc__", doc_bits).is_err()
        {
            return MoltObject::none().bits();
        }

        unsafe {
            function_set_globals_from_module_name(_py, func_ptr, module_bits);
        }

        if !obj_from_bits(code_bits).is_none() {
            unsafe {
                function_set_code_bits(_py, func_ptr, code_bits);
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() {
                unsafe {
                    if object_type_id(code_ptr) == TYPE_ID_CODE {
                        code_set_signature_bits(
                            _py,
                            code_ptr,
                            arg_names_bits,
                            posonly_bits,
                            kwonly_bits,
                            vararg_bits,
                            varkw_bits,
                        );
                    }
                }
            }
        }

        if !obj_from_bits(bind_kind_bits).is_none()
            && set_attr(b"__molt_bind_kind__", bind_kind_bits).is_err()
        {
            return MoltObject::none().bits();
        }

        unsafe {
            crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
        }

        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_init_metadata_packed(
    func_bits: u64,
    metadata_bits: u64,
    code_bits: u64,
    bind_kind_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
        }

        let Some(metadata_ptr) = obj_from_bits(metadata_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected metadata tuple");
        };
        unsafe {
            if object_type_id(metadata_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "expected metadata tuple");
            }
        }
        let Some(metadata) = (unsafe {
            crate::object::seq_access::snapshot(
                _py,
                metadata_ptr,
                "sequence snapshot allocation failed",
            )
        }) else {
            return MoltObject::none().bits();
        };
        if metadata.len() != 11 {
            return raise_exception::<_>(_py, "TypeError", "metadata tuple must contain 11 items");
        }

        let set_attr = |name: &'static [u8], value_bits: u64| -> Result<(), u64> {
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, name) else {
                return Err(MoltObject::none().bits());
            };
            unsafe {
                crate::call::class_init::function_set_attr_bits(
                    _py, func_ptr, attr_bits, value_bits,
                );
            }
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        };

        let name_bits = metadata[0];
        let qualname_bits = metadata[1];
        let module_bits = metadata[2];
        let arg_names_bits = metadata[3];
        let posonly_bits = metadata[4];
        let kwonly_bits = metadata[5];
        let vararg_bits = metadata[6];
        let varkw_bits = metadata[7];
        let defaults_bits = metadata[8];
        let kwdefaults_bits = metadata[9];
        let doc_bits = metadata[10];

        if set_attr(b"__name__", name_bits).is_err()
            || set_attr(b"__qualname__", qualname_bits).is_err()
            || set_attr(b"__module__", module_bits).is_err()
            || set_attr(b"__molt_arg_names__", arg_names_bits).is_err()
            || set_attr(b"__molt_posonly__", posonly_bits).is_err()
            || set_attr(b"__molt_kwonly_names__", kwonly_bits).is_err()
            || set_attr(b"__molt_vararg__", vararg_bits).is_err()
            || set_attr(b"__molt_varkw__", varkw_bits).is_err()
            || set_attr(b"__defaults__", defaults_bits).is_err()
            || set_attr(b"__kwdefaults__", kwdefaults_bits).is_err()
            || set_attr(b"__doc__", doc_bits).is_err()
        {
            return MoltObject::none().bits();
        }

        unsafe {
            function_set_globals_from_module_name(_py, func_ptr, module_bits);
        }

        if !obj_from_bits(code_bits).is_none() {
            unsafe {
                function_set_code_bits(_py, func_ptr, code_bits);
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }

        if !obj_from_bits(bind_kind_bits).is_none()
            && set_attr(b"__molt_bind_kind__", bind_kind_bits).is_err()
        {
            return MoltObject::none().bits();
        }

        unsafe {
            crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
        }

        MoltObject::none().bits()
    })
}

unsafe fn function_set_globals_from_module_name(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    module_bits: u64,
) {
    unsafe {
        let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
            return;
        };
        let module_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            guard.get(&module_name).copied().unwrap_or(0)
        };
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            return;
        };
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return;
        }
        let globals_bits = module_dict_bits(module_ptr);
        if globals_bits != 0 && !obj_from_bits(globals_bits).is_none() {
            function_set_globals_bits(_py, func_ptr, globals_bits);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_set_defaults(
    func_bits: u64,
    defaults_bits: u64,
    kwdefaults_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
        }

        let set_attr = |name: &'static [u8], value_bits: u64| -> Result<(), u64> {
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, name) else {
                return Err(MoltObject::none().bits());
            };
            unsafe {
                crate::call::class_init::function_set_attr_bits(
                    _py, func_ptr, attr_bits, value_bits,
                );
            }
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        };

        if set_attr(b"__defaults__", defaults_bits).is_err()
            || set_attr(b"__kwdefaults__", kwdefaults_bits).is_err()
        {
            return MoltObject::none().bits();
        }

        unsafe {
            crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
        }

        MoltObject::none().bits()
    })
}

/// Read a function object's `__defaults__`/`__kwdefaults__` mutation version
/// stamp as an inline int.  The compile-time defaults-devirt deopt guard calls
/// this once per direct call site and branches on `version == 0` (the baked
/// literal default is still observably correct) vs `!= 0` (a runtime
/// reassignment occurred → read the live tuple/dict).  A non-function or null
/// argument yields 0 (treated as "pristine" — those call sites never bake a
/// guarded default against a non-function, so the value is inert).
#[unsafe(no_mangle)]
pub extern "C" fn molt_function_defaults_version(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let version = obj_from_bits(func_bits)
            .as_ptr()
            .map(|func_ptr| unsafe {
                if object_type_id(func_ptr) == TYPE_ID_FUNCTION {
                    function_defaults_version(func_ptr)
                } else {
                    0
                }
            })
            .unwrap_or(0);
        // The counter is a small monotonic value; an inline int is exact and
        // the guard only ever compares it against 0.
        MoltObject::from_int(version as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_get_code(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let code_bits = ensure_function_code_bits(_py, func_ptr);
            if obj_from_bits(code_bits).is_none() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, code_bits);
            code_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_get_globals(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let globals_bits = function_globals_bits(func_ptr);
            if globals_bits == 0 || obj_from_bits(globals_bits).is_none() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, globals_bits);
            globals_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_new(
    filename_bits: u64,
    name_bits: u64,
    firstlineno_bits: u64,
    linetable_bits: u64,
    varnames_bits: u64,
    names_bits: u64,
    argcount_bits: u64,
    posonlyargcount_bits: u64,
    kwonlyargcount_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let filename_obj = obj_from_bits(filename_bits);
        let Some(filename_ptr) = filename_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code filename must be str");
        };
        unsafe {
            if object_type_id(filename_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code filename must be str");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code name must be str");
            }
        }
        if !crate::object::builders::acyclic_slot_edge(
            crate::object::heap_kinds_generated::HeapAcyclicSlot::CodeLinetable,
            linetable_bits,
        ) {
            return raise_exception::<_>(_py, "TypeError", "code linetable must be bytes or None");
        }
        let Some(argcount) = to_i64(obj_from_bits(argcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code argcount must be int");
        };
        let Some(posonlyargcount) = to_i64(obj_from_bits(posonlyargcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code posonlyargcount must be int");
        };
        let Some(kwonlyargcount) = to_i64(obj_from_bits(kwonlyargcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code kwonlyargcount must be int");
        };
        if argcount < 0 || posonlyargcount < 0 || kwonlyargcount < 0 {
            return raise_exception::<_>(_py, "ValueError", "code arg counts must be >= 0");
        }
        let mut varnames_bits = varnames_bits;
        let mut varnames_owned = false;
        if obj_from_bits(varnames_bits).is_none() {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            varnames_bits = MoltObject::from_ptr(tuple_ptr).bits();
            varnames_owned = true;
        } else {
            if !crate::object::builders::acyclic_slot_edge(
                crate::object::heap_kinds_generated::HeapAcyclicSlot::CodeVarnames,
                varnames_bits,
            ) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code varnames must be a tuple of str or None",
                );
            }
        }
        let mut names_bits = names_bits;
        let mut names_owned = false;
        if obj_from_bits(names_bits).is_none() {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                if varnames_owned {
                    dec_ref_bits(_py, varnames_bits);
                }
                return MoltObject::none().bits();
            }
            names_bits = MoltObject::from_ptr(tuple_ptr).bits();
            names_owned = true;
        } else {
            if !crate::object::builders::acyclic_slot_edge(
                crate::object::heap_kinds_generated::HeapAcyclicSlot::CodeNames,
                names_bits,
            ) {
                if varnames_owned {
                    dec_ref_bits(_py, varnames_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code names must be a tuple of str or None",
                );
            }
        }
        let firstlineno = to_i64(obj_from_bits(firstlineno_bits)).unwrap_or(0);
        let ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            firstlineno,
            linetable_bits,
            varnames_bits,
            names_bits,
            argcount as u64,
            posonlyargcount as u64,
            kwonlyargcount as u64,
        );
        if names_owned {
            dec_ref_bits(_py, names_bits);
        }
        if varnames_owned {
            dec_ref_bits(_py, varnames_bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let debug_bound = crate::builtins::attributes::debug_bound_method_enabled();
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            if debug_bound {
                let self_obj = obj_from_bits(self_bits);
                let self_label = self_obj
                    .as_ptr()
                    .map(|_| type_name(_py, self_obj).into_owned())
                    .unwrap_or_else(|| format!("immediate:{:#x}", self_bits));
                let self_type_id = self_obj
                    .as_ptr()
                    .map(|ptr| unsafe { object_type_id(ptr) })
                    .unwrap_or(0);
                eprintln!(
                    "molt_bound_method_new: non-object func_bits={:#x} self={} self_type_id={}",
                    func_bits, self_label, self_type_id
                );
                if let Some(name) = crate::builtins::attr::debug_last_attr_name() {
                    eprintln!("molt_bound_method_new last_attr={}", name);
                }
            }
            return raise_exception::<_>(_py, "TypeError", "bound method expects function object");
        };
        unsafe {
            // If func_bits is already a BOUND_METHOD, unwrap to its inner function
            // so we don't fail the TYPE_ID_FUNCTION check below. This happens when
            // inline int/float/bool attribute fallback passes a bound method through
            // the builtin_class_method_bits path.
            if object_type_id(func_ptr) == TYPE_ID_BOUND_METHOD {
                let inner_func_bits = bound_method_func_bits(func_ptr);
                return molt_bound_method_new(inner_func_bits, self_bits);
            }
            if !is_callable_impl(_py, func_bits) {
                if debug_bound {
                    let type_label = type_name(_py, func_obj).into_owned();
                    let self_label = obj_from_bits(self_bits)
                        .as_ptr()
                        .map(|_| type_name(_py, obj_from_bits(self_bits)).into_owned())
                        .unwrap_or_else(|| format!("immediate:{:#x}", self_bits));
                    eprintln!(
                        "molt_bound_method_new: expected callable got type_id={} type={} self={}",
                        object_type_id(func_ptr),
                        type_label,
                        self_label
                    );
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bound method expects callable object",
                );
            }
        }
        let ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            let method_bits = {
                let func_class_bits = unsafe { object_class_bits(func_ptr) };
                if func_class_bits == builtin_classes(_py).builtin_function_or_method {
                    func_class_bits
                } else {
                    crate::builtins::types::method_class(_py)
                }
            };
            if method_bits != 0 {
                unsafe {
                    let old_bits = object_class_bits(ptr);
                    if old_bits != method_bits
                        && !object_init_class_edge_unpublished(
                            _py,
                            ptr,
                            method_bits,
                            ClassEdgeOwnership::Owned,
                        )
                    {
                        dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                        return MoltObject::none().bits();
                    }
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// # Safety
/// `self_ptr_bits` must encode a valid closure storage pointer and `offset`
/// must be within the allocated payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_closure_load(self_ptr_bits: u64, offset: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(self_ptr) = crate::provenance::abi::mut_ptr::<u8>(self_ptr_bits) else {
                return MoltObject::none().bits();
            };
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let Some(offset) = usize::try_from(offset).ok() else {
                return MoltObject::none().bits();
            };
            let slot = self_ptr.add(offset) as *mut u64;
            let bits = *slot;
            inc_ref_bits(_py, bits);
            bits
        })
    }
}

/// # Safety
/// `self_ptr_bits` must encode a valid closure storage pointer and `offset`
/// must be within the allocated payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_closure_store(self_ptr_bits: u64, offset: u64, bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(self_ptr) = crate::provenance::abi::mut_ptr::<u8>(self_ptr_bits) else {
                return MoltObject::none().bits();
            };
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let Some(offset) = usize::try_from(offset).ok() else {
                return MoltObject::none().bits();
            };
            let slot = self_ptr.add(offset) as *mut u64;
            let old_bits = *slot;
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, bits);
            *slot = bits;
            MoltObject::none().bits()
        })
    }
}

#[cfg(test)]
mod wasm_runtime_callable_tests {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn raw_core_function_address_does_not_impersonate_named_runtime_key() {
        let raw_type_init = crate::molt_type_init as *const () as usize as u64;
        let named_type_init = fn_addr!(crate::molt_type_init);

        assert_ne!(named_type_init, raw_type_init);
        assert_eq!(
            canonicalize_runtime_callable_key(raw_type_init),
            raw_type_init
        );
        assert_eq!(
            runtime_callable_target_ptr(named_type_init),
            Some(crate::molt_type_init as *const ())
        );
    }

    #[test]
    fn wasm_runtime_callable_symbols_resolve_in_functions_scope() {
        wasm_callables::assert_reserved_runtime_symbols_resolve();
    }

    /// Regression for the reserved-callable fixed-arity dispatch bug: a `Direct`
    /// reserved callable (e.g. `molt_type_new`) whose function object stored the
    /// *trampoline* table slot as its identity pointer was routed back onto the
    /// trampoline lane, emitting `molt_call_indirectN` with N positional args
    /// into the trampoline region — which the host rejects because that region
    /// expects the 3-argument closure/argv/argc convention. The region math must
    /// recognize a reserved slot from EITHER region so the fixed-arity lanes can
    /// canonicalize it back to the direct slot.
    #[test]
    fn reserved_slot_index_resolves_from_direct_and_trampoline_regions() {
        // Mirror the wasm layout: direct region [1000, 1024), trampoline region
        // [1024, 1048), 24 reserved callables.
        let direct_start = 1000u64;
        let count = 24u64;
        let trampoline_start = direct_start + count;

        // molt_type_new is reserved index 1.
        let direct_slot = direct_start + 1;
        let trampoline_slot = trampoline_start + 1;

        // A pointer in the direct region resolves to its index.
        assert_eq!(
            reserved_slot_index_for_any_region(direct_slot, direct_start, trampoline_start, count),
            Some(1)
        );
        // The bug shape: a pointer in the TRAMPOLINE region must also resolve to
        // the same reserved index (previously returned None → dispatch fell back
        // to the trampoline lane and produced the arg-count mismatch).
        assert_eq!(
            reserved_slot_index_for_any_region(
                trampoline_slot,
                direct_start,
                trampoline_start,
                count
            ),
            Some(1)
        );
        // Out-of-range indices (below both regions, at the boundary, and above
        // the trampoline region) resolve to nothing.
        assert_eq!(
            reserved_slot_index_for_any_region(
                direct_start - 1,
                direct_start,
                trampoline_start,
                count
            ),
            None
        );
        assert_eq!(
            reserved_slot_index_for_any_region(
                trampoline_start + count,
                direct_start,
                trampoline_start,
                count
            ),
            None
        );
    }
}
