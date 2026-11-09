// Runtime callable, function, code-object, bound-method, and closure ABI.
// This module owns function pointer canonicalization together with the
// exported constructors that consume it, so call target identity has one
// authority instead of a registry/function split across functions.rs.

use super::wasm_callables_generated as wasm_callables;
use super::*;
use crate::BuiltinExc;
use crate::ClassEdgeOwnership;
use crate::object::layout::function_call_abi;
use crate::object::object_init_class_edge_unpublished;
use crate::runtime_flags::{self, RuntimeFlag};

#[derive(Copy, Clone)]
struct NativeCallableTarget(*const ());

// Native callable targets are immutable code pointers published once from Rust.
unsafe impl Send for NativeCallableTarget {}
unsafe impl Sync for NativeCallableTarget {}

fn native_callable_targets() -> &'static Mutex<HashMap<u64, NativeCallableTarget>> {
    static TARGETS: OnceLock<Mutex<HashMap<u64, NativeCallableTarget>>> = OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The named key of a `stringify!` symbol path: its name after the last `::`.
/// `fn_key!` and `fn_addr!` evaluate this in a `const` block, so identity
/// predicates on the call path compare constants instead of scanning names.
pub(crate) const fn runtime_callable_key_from_symbol_path(symbol_path: &str) -> Option<u64> {
    let path = symbol_path.as_bytes();
    let mut start = path.len();
    while start >= 2 {
        if path[start - 2] == b':' && path[start - 1] == b':' {
            break;
        }
        start -= 1;
    }
    let name = if start >= 2 {
        path.split_at(start).1
    } else {
        path
    };
    wasm_callables::runtime_callable_key_from_symbol_name(name)
}

/// Resolve semantic callable identity without registering a native call target.
/// Metadata readers use this path without allocation or registry side effects.
#[inline]
pub(crate) fn runtime_fn_key(named_key: Option<u64>, raw_ptr: *const ()) -> u64 {
    named_key.unwrap_or_else(|| crate::provenance::abi::expose_function_address(raw_ptr))
}

/// Resolve identity and publish its executable target for callable construction.
pub(crate) fn runtime_fn_publish(named_key: Option<u64>, raw_ptr: *const ()) -> u64 {
    let key = runtime_fn_key(named_key, raw_ptr);
    register_address_target(key, raw_ptr);
    key
}

/// Named keys sit in a reserved range above every function address. The
/// generated table owns their targets.
fn runtime_callable_key_is_named(key: u64) -> bool {
    key >= wasm_callables::RUNTIME_CALLABLE_KEY_BASE
}

/// The registry's one writer. It admits address keys only: a named key's
/// value is not an address, so it must never become a call target.
fn register_address_target(key: u64, raw_target: *const ()) {
    if raw_target.is_null() || runtime_callable_key_is_named(key) {
        return;
    }
    let mut guard = native_callable_targets().lock().unwrap();
    guard.entry(key).or_insert(NativeCallableTarget(raw_target));
}

/// Generated intrinsic resolvers name their symbol at run time.
pub(crate) fn runtime_fn_addr(symbol_path: &str, raw_ptr: *const ()) -> u64 {
    runtime_fn_publish(runtime_callable_key_from_symbol_path(symbol_path), raw_ptr)
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
#[cfg(any(test, target_arch = "wasm32"))]
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
        reserved_wasm_runtime_trampoline_ptr(fn_ptr).unwrap_or(tramp_ptr)
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
    if runtime_callable_key_is_named(fn_ptr) {
        return wasm_callables::runtime_callable_target_ptr(fn_ptr);
    }
    native_callable_targets()
        .lock()
        .unwrap()
        .get(&fn_ptr)
        .map(|target| target.0)
}

/// What a task's stored poll identity calls (HF-699 K2, K4). The variant also
/// fixes the argument convention, so one resolution serves both.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PollTarget {
    /// A runtime poll, resolved from its key. It takes the attached context
    /// and then the task's object word.
    /// On wasm32 the pointer is the runtime's own table slot, which wasm-ld
    /// assigns and the runtime's element segment publishes in every layout.
    Runtime(unsafe extern "C" fn(u64, u64) -> i64),
    /// A compiled poll: the identity the app stored, which is its table index
    /// on wasm32 and its code address on native. It takes the raw payload
    /// address of the task.
    AppTableIndex(u64),
}

/// Resolve a stored poll identity: a named key through the generated table,
/// otherwise the app's table index. Requires: `poll_fn` is a key or an app
/// table index (K1). Returns `None` for a named key that names no runtime
/// poll in this build, and for zero or a value wider than the target's
/// machine word, which no table index or code address can be.
/// This runs on every resume; it takes no lock and allocates nothing.
#[inline]
pub(crate) fn runtime_poll_call_target(poll_fn: u64) -> Option<PollTarget> {
    if runtime_callable_key_is_named(poll_fn) {
        let target = wasm_callables::runtime_poll_callable_target_ptr(poll_fn)?;
        // SAFETY: the generated poll table holds only manifest runtime polls,
        // and the manifest validator admits a poll only with one object-word
        // argument. Its generated export prepends the attached execution context.
        let poll = unsafe {
            std::mem::transmute::<*const (), unsafe extern "C" fn(u64, u64) -> i64>(target)
        };
        return Some(PollTarget::Runtime(poll));
    }
    if poll_fn == 0 || crate::provenance::abi::address(poll_fn).is_none() {
        return None;
    }
    Some(PollTarget::AppTableIndex(poll_fn))
}

#[inline]
unsafe fn init_runtime_callable_function_obj(
    py: &PyToken<'_>,
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
            function_set_trampoline_ptr(py, ptr, normalized_trampoline_ptr);
        }
    }
}

/// Atomically retarget a Python function to the executable identity published
/// by a compatible code object. Signature consumers project directly from the
/// new code object, while defaults, globals, annotations, and the lexical
/// closure remain owned by the function.
pub(crate) unsafe fn function_replace_code_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    code_bits: u64,
) -> bool {
    unsafe {
        let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
            raise_exception::<u64>(
                _py,
                BuiltinExc::TypeError,
                "function __code__ must be a code object",
            );
            return false;
        };
        if object_type_id(code_ptr) != TYPE_ID_CODE {
            raise_exception::<u64>(
                _py,
                BuiltinExc::TypeError,
                "function __code__ must be a code object",
            );
            return false;
        }
        let Some(identity) = crate::object::layout::code_callable_identity(code_ptr) else {
            raise_exception::<u64>(
                _py,
                BuiltinExc::ValueError,
                "function __code__ must have a published callable identity",
            );
            return false;
        };
        if !identity.call_abi.is_reconstructible()
            || function_call_abi(func_ptr) != identity.call_abi
        {
            raise_exception::<u64>(
                _py,
                BuiltinExc::ValueError,
                "function and code object use different callable context ABIs",
            );
            return false;
        }

        #[cfg(not(target_arch = "wasm32"))]
        let call_target = runtime_callable_target_ptr(identity.fn_ptr)
            .or_else(|| crate::provenance::abi::function_ptr(identity.fn_ptr));
        #[cfg(target_arch = "wasm32")]
        let call_target = Some(std::ptr::null());
        let Some(call_target) = call_target else {
            raise_exception::<u64>(
                _py,
                BuiltinExc::SystemError,
                "replacement code has no resolvable native callable target",
            );
            return false;
        };

        let prepared = match crate::object::layout::prepare_function_code_bits(
            _py,
            func_ptr,
            code_bits,
            Some(identity),
            None,
        ) {
            Ok(prepared) => prepared,
            Err(()) => return false,
        };
        crate::object::layout::function_replace_callable_identity(func_ptr, identity, call_target);
        publish_function_metadata_code(_py, func_ptr, prepared, true);
        !exception_pending(_py)
    }
}

pub(crate) fn alloc_runtime_function_obj(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    arity: u64,
) -> *mut u8 {
    let fn_key = canonicalize_runtime_callable_key(fn_ptr);
    #[cfg(not(miri))]
    if let Some(raw_target) = crate::provenance::abi::function_ptr(fn_key) {
        register_address_target(fn_key, raw_target);
    }
    let ptr = alloc_function_obj(_py, fn_key, arity);
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        init_runtime_callable_function_obj(_py, ptr, fn_key, fn_ptr, 0, false);
    }
    ptr
}

/// Complete a compiled function object's creation with the entry custody its
/// code generator derived from the function's own parameter declaration
/// (`molt_codegen_abi::EntryCustodyDeclaration`): 0 borrows every argument,
/// `ENTRY_CUSTODY_ADOPTS` adopts every Python parameter. Any other word, or an
/// adopting entry the function cannot honor, is a code generation defect: the
/// new function is released and `SystemError` raised.
unsafe fn finish_compiled_function(py: &PyToken<'_>, ptr: *mut u8, entry_custody: u64) -> u64 {
    let custody = match entry_custody {
        0 => Ok(crate::object::layout::EntryCustody::Borrowing),
        molt_codegen_abi::ENTRY_CUSTODY_ADOPTS => Ok(crate::object::layout::EntryCustody::Adopting),
        _ => Err("compiled function entry custody must be 0 (borrowing) or 1 (adopting)"),
    };
    let bits = MoltObject::from_ptr(ptr).bits();
    match custody.and_then(|custody| unsafe {
        crate::object::layout::function_publish_entry_custody(ptr, custody)
    }) {
        Ok(()) => bits,
        Err(message) => {
            dec_ref_bits(py, bits);
            raise_exception::<_>(py, BuiltinExc::SystemError, message)
        }
    }
}

pub(crate) fn molt_func_new_attached_body(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    entry_custody: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let fn_key = canonicalize_runtime_callable_key(fn_ptr);
        let trace = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_FUNC_NEW);
        if trace {
            eprintln!(
                "molt func new: fn_ptr={fn_ptr} fn_key={fn_key} tramp_ptr={trampoline_ptr} arity={arity} entry_custody={entry_custody}"
            );
        }
        let ptr = alloc_function_obj(_py, fn_key, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                init_runtime_callable_function_obj(_py, ptr, fn_key, fn_ptr, trampoline_ptr, true);
                finish_compiled_function(_py, ptr, entry_custody)
            }
        }
    })
}

pub(crate) fn molt_func_new_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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
    if let Some(raw_target) = crate::provenance::abi::function_ptr(fn_key) {
        register_address_target(fn_key, raw_target);
    }
    let trace = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_BUILTIN_FUNC);
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
        return raise_exception::<_>(_py, BuiltinExc::RuntimeError, &msg);
    }
    let ptr = alloc_function_obj(_py, fn_key, arity);
    if ptr.is_null() {
        return raise_exception::<_>(_py, BuiltinExc::RuntimeError, "builtin func alloc failed");
    }
    unsafe {
        init_runtime_callable_function_obj(_py, ptr, fn_key, fn_ptr, trampoline_ptr, false);
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

/// Resolve a public builtin name from the executing/canonical builtins
/// namespace. Provider declarations construct objects only during publication;
/// deletion or replacement of a public alias never triggers materialization.
pub(crate) fn lookup_builtin_name(
    py: &crate::PyToken<'_>,
    python_name: &str,
) -> Result<Option<u64>, ()> {
    let namespace = crate::builtins::frames::frame_effective_builtins_bits(py, 0)?;
    // A custom mapping callback may replace the interpreter publication. Keep
    // the selected namespace alive through lookup and result acquisition.
    inc_ref_bits(py, namespace);
    let owner = obj_from_bits(namespace)
        .as_ptr()
        .map(|__ptr| crate::PtrDropGuard::new(py, __ptr));
    let name = dynamic_attr_name_bits(py, python_name.as_bytes()).ok_or(())?;
    let value = crate::builtins::modules::lookup_builtin_global(py, name, namespace);
    dec_ref_bits(py, name);
    drop(owner);
    value
}

/// The exact provider module owns the published callable and its immutable
/// receiver edge. This allocator is consumed only by namespace publication.
pub(crate) fn alloc_python_builtin_function_bits(
    _py: &crate::PyToken<'_>,
    info: wasm_callables::PythonBuiltinFunctionInfo,
    module_bits: u64,
) -> Option<u64> {
    if info.public == crate::intrinsics::public_objects_generated::PublicObjectId::BuildClassBuiltin
    {
        // Publication and LOAD_BUILD_CLASS share this runtime-owned singleton,
        // independent of later public or intrinsic-registry dictionary edits.
        let bits = crate::intrinsics::registry::class_builder_bits(_py)?;
        let ptr = obj_from_bits(bits)
            .as_ptr()
            .expect("public native callable");
        let spec = super::native_callable::NativeCallableSpec {
            identity: Some(info.public.identity()),
            self_bits: Some(module_bits),
            ..super::native_callable::NativeCallableSpec::uncached_function()
        };
        if !unsafe { super::native_callable::configure_native_callable(_py, ptr, spec) } {
            dec_ref_bits(_py, bits);
            return None;
        }
        return Some(bits);
    }
    let fn_ptr = crate::intrinsics::registry::try_app_resolve_symbol(info.runtime_name)?;
    let ptr = alloc_runtime_function_obj(_py, fn_ptr, info.arity);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    if !init_python_builtin_function_metadata(_py, bits, info) {
        dec_ref_bits(_py, bits);
        return None;
    }
    let spec = super::native_callable::NativeCallableSpec {
        identity: Some(info.public.identity()),
        text_signature: info.text_signature,
        self_bits: Some(module_bits),
        ..super::native_callable::NativeCallableSpec::uncached_function()
    };
    if !unsafe { super::native_callable::configure_native_callable(_py, ptr, spec) } {
        dec_ref_bits(_py, bits);
        return None;
    }
    Some(bits)
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
        .map(|kind| MoltObject::from_int(kind as i64).bits())
        .unwrap_or_else(|| MoltObject::none().bits());
    let result = molt_function_init_metadata_packed_attached_body(
        _py,
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

    let identity = info.public.identity();
    let name_bits = identity.name.owned_bits(_py)?;
    owned.push(name_bits);
    let qualname_bits = identity.qualname.owned_bits(_py)?;
    owned.push(qualname_bits);
    let module_bits = identity.module.owned_bits(_py)?;
    owned.push(module_bits);
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
    let execution_kind_bits =
        MoltObject::from_int(crate::object::layout::CodeExecutionKind::Direct as i64).bits();
    let freevars_bits = alloc_static_str_tuple_bits(_py, std::iter::empty(), &mut owned)?;
    let cellvars_bits = alloc_static_str_tuple_bits(_py, std::iter::empty(), &mut owned)?;
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
            execution_kind_bits,
            freevars_bits,
            cellvars_bits,
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
        wasm_callables::GeneratedBuiltinDefaultValue::EmptyTuple => {
            alloc_static_str_tuple_bits(_py, std::iter::empty(), owned)
        }
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

pub(crate) fn molt_func_new_builtin_named_attached_body(
    _py: &crate::PyToken<'_>,
    name_bits: u64,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let trace = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_BUILTIN_FUNC);
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "builtin name must be str");
        };
        // The frontend distinguishes a public declaration name from an
        // explicit executable runtime symbol. Only the former resolves the
        // active namespace; raw targets cannot refill a missing public alias.
        if wasm_callables::python_builtin_function_info(&name).is_some() {
            let resolved = match lookup_builtin_name(_py, &name) {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            };
            if let Some(func_bits) = resolved {
                if trace {
                    eprintln!("molt builtin_func named: resolved python builtin {}", name);
                }
                return func_bits;
            }
            return raise_exception(
                _py,
                BuiltinExc::NameError,
                &format!("name '{name}' is not defined"),
            );
        }
        {
            match crate::intrinsics::registry::try_resolve_intrinsic_func(_py, &name, false) {
                Ok(Some(func_bits)) => {
                    if trace {
                        eprintln!("molt builtin_func named: resolved intrinsic {}", name);
                    }
                    return func_bits;
                }
                Ok(None) => {}
                Err(()) => return MoltObject::none().bits(),
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

pub(crate) fn molt_func_new_closure_attached_body(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
    entry_custody: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let fn_key = canonicalize_runtime_callable_key(fn_ptr);
        let trace = runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_FUNC_NEW);
        if trace {
            eprintln!(
                "molt func new closure: fn_ptr={fn_ptr} tramp_ptr={trampoline_ptr} arity={arity} closure_bits={closure_bits} entry_custody={entry_custody}"
            );
        }
        let call_abi = if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
            let Some(closure) = (unsafe { crate::object::cells::inspect_cell_tuple(closure_bits) })
            else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "compiled closure is not a tuple",
                );
            };
            if closure.first_non_cell.is_some() {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "compiled closure contains a non-cell value",
                );
            }
            if closure.len == 0 {
                FunctionCallAbi::Positional
            } else {
                FunctionCallAbi::LexicalClosureFirst
            }
        } else {
            FunctionCallAbi::Positional
        };
        let ptr = alloc_function_obj(_py, fn_key, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            function_set_closure_bits(_py, ptr, closure_bits, call_abi);
            init_runtime_callable_function_obj(_py, ptr, fn_key, fn_ptr, trampoline_ptr, true);
            finish_compiled_function(_py, ptr, entry_custody)
        }
    })
}

/// CPython's function.__new__ Clinic declaration: every parameter can be
/// positional or named; 3.13 added kwdefaults as the sixth optional parameter.
fn function_type_parameter_names(py: &PyToken<'_>) -> &'static [&'static str] {
    const NAMES: [&str; 6] = [
        "code",
        "globals",
        "name",
        "argdefs",
        "closure",
        "kwdefaults",
    ];
    if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
        &NAMES
    } else {
        &NAMES[..5]
    }
}

pub(crate) unsafe fn function_type_new_from_call(
    py: &PyToken<'_>,
    args: &[u64],
    keywords: crate::builtins::native_arguments::NativeKeywords<'_>,
) -> u64 {
    unsafe fn bind<const N: usize>(
        py: &PyToken<'_>,
        args: &[u64],
        keywords: crate::builtins::native_arguments::NativeKeywords<'_>,
        names: [&str; N],
    ) -> u64 {
        let Some(bound) =
            crate::builtins::native_arguments::bind_named(py, "function", args, keywords, names, 2)
        else {
            return MoltObject::none().bits();
        };
        let values: [u64; N] =
            std::array::from_fn(|index| bound[index].unwrap_or_else(|| MoltObject::none().bits()));
        // Binding pins admitted values across mapping callbacks and keeps them
        // alive until the constructor has acquired its own metadata/closure.
        unsafe { function_type_new_from_args(py, &values) }
    }

    let names = function_type_parameter_names(py);
    // Valid positional construction retains its allocation-free argument path.
    if keywords.is_empty() && (2..=names.len()).contains(&args.len()) {
        return unsafe { function_type_new_from_args(py, args) };
    }
    unsafe {
        if names.len() == 5 {
            bind::<5>(
                py,
                args,
                keywords,
                names.try_into().expect("3.12 function signature"),
            )
        } else {
            bind::<6>(
                py,
                args,
                keywords,
                names.try_into().expect("3.13+ function signature"),
            )
        }
    }
}

pub(crate) unsafe fn function_type_new_from_args(_py: &PyToken<'_>, args: &[u64]) -> u64 {
    unsafe {
        if !(2..=function_type_parameter_names(_py).len()).contains(&args.len()) {
            return function_type_new_from_call(
                _py,
                args,
                crate::builtins::native_arguments::NativeKeywords::empty(),
            );
        }
        let Some(code_ptr) = obj_from_bits(args[0])
            .as_ptr()
            .filter(|&ptr| object_type_id(ptr) == TYPE_ID_CODE)
        else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                &format!(
                    "function() argument 'code' must be code, not {}",
                    type_name(_py, obj_from_bits(args[0])),
                ),
            );
        };
        let Some(callable_identity) = crate::object::layout::code_callable_identity(code_ptr)
        else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "code object has no Molt callable target",
            );
        };
        if !callable_identity.call_abi.is_reconstructible() {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "cannot create a Python function from opaque runtime-context code",
            );
        }
        let globals = match crate::builtins::frames::globals_namespace_storage_ptr(_py, args[1]) {
            Ok(storage) => storage,
            Err(()) => return MoltObject::none().bits(),
        };
        let Some(globals_ptr) = globals else {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                &format!(
                    "function() argument 'globals' must be dict, not {}",
                    type_name(_py, obj_from_bits(args[1])),
                ),
            );
        };

        let none_bits = MoltObject::none().bits();
        let name_bits = match args.get(2).copied() {
            Some(bits) if !obj_from_bits(bits).is_none() => {
                let Some(name_ptr) = obj_from_bits(bits).as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "arg 3 (name) must be None or string",
                    );
                };
                if object_type_id(name_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
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
                    BuiltinExc::TypeError,
                    "arg 4 (defaults) must be None or tuple",
                );
            };
            if object_type_id(defaults_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "arg 4 (defaults) must be None or tuple",
                );
            }
        }

        let freevars_bits = code_freevars_bits(code_ptr);
        let Some(freevars_ptr) = obj_from_bits(freevars_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::SystemError,
                "code freevars metadata is not a tuple",
            );
        };
        if object_type_id(freevars_ptr) != TYPE_ID_TUPLE {
            return raise_exception::<_>(
                _py,
                BuiltinExc::SystemError,
                "code freevars metadata is not a tuple",
            );
        }
        let required_cells = crate::object::seq_access::len(freevars_ptr);
        match callable_identity.call_abi {
            FunctionCallAbi::Positional if required_cells != 0 => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "positional code object has lexical free variables",
                );
            }
            FunctionCallAbi::LexicalClosureFirst if required_cells == 0 => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "lexical-closure code object has no free variables",
                );
            }
            FunctionCallAbi::OpaqueContextFirst => unreachable!("opaque code rejected above"),
            _ => {}
        }
        let closure_bits = args.get(4).copied().unwrap_or(none_bits);
        let closure = if obj_from_bits(closure_bits).is_none() {
            if required_cells != 0 {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "arg 5 (closure) must be tuple",
                );
            }
            None
        } else {
            let Some(closure) = crate::object::cells::inspect_cell_tuple(closure_bits) else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "arg 5 (closure) must be None or tuple",
                );
            };
            Some(closure)
        };
        let kwdefaults_bits = args.get(5).copied().unwrap_or(none_bits);
        if !obj_from_bits(kwdefaults_bits).is_none() {
            match crate::builtins::frames::globals_namespace_storage_ptr(_py, kwdefaults_bits) {
                Ok(Some(_)) => {}
                Err(()) => return MoltObject::none().bits(),
                Ok(None) => {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "arg 6 (kwdefaults) must be None or dict",
                    );
                }
            }
        }
        let closure_to_store = if let Some(closure) = closure {
            let supplied_cells = closure.len;
            if supplied_cells != required_cells {
                let code_name = string_obj_to_owned(obj_from_bits(code_name_bits(code_ptr)))
                    .unwrap_or_else(|| "<code>".to_string());
                let msg = format!(
                    "{code_name} requires closure of length {required_cells}, not {supplied_cells}"
                );
                return raise_exception::<_>(_py, BuiltinExc::ValueError, &msg);
            }
            if let Some(invalid_bits) = closure.first_non_cell {
                let found = type_name(_py, obj_from_bits(invalid_bits));
                let msg = format!("arg 5 (closure) expected cell, found {found}");
                return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
            }
            closure_bits
        } else {
            0
        };

        let func_ptr = alloc_function_obj(_py, callable_identity.fn_ptr, callable_identity.arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        init_runtime_callable_function_obj(
            _py,
            func_ptr,
            callable_identity.fn_ptr,
            callable_identity.fn_ptr,
            callable_identity.trampoline_ptr,
            true,
        );
        if function_set_globals_bits(_py, func_ptr, args[1]).is_err() {
            dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
            return MoltObject::none().bits();
        }
        if closure_to_store != 0 {
            function_set_closure_bits(_py, func_ptr, closure_to_store, callable_identity.call_abi);
        }
        // The new function runs the code's compiled entry, so it adopts exactly
        // as that entry was compiled; the code pairing below verifies it.
        crate::object::layout::function_publish_entry_custody(func_ptr, callable_identity.custody)
            .expect("an unpaired reconstructible function publishes its entry custody");
        if !function_set_code_bits(_py, func_ptr, args[0]) {
            dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
            return MoltObject::none().bits();
        }

        let set_attr = |name: crate::NameId, value_bits: u64| {
            crate::call::class_init::function_set_attr_name(_py, func_ptr, name, value_bits)
        };

        let module_bits = {
            let Some(module_name_bits) = crate::NameId::DunderName.owned_bits(_py) else {
                dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
                return MoltObject::none().bits();
            };
            let bits = dict_get_in_place(_py, globals_ptr, module_name_bits);
            dec_ref_bits(_py, module_name_bits);
            match bits {
                Ok(bits) => bits.unwrap_or(none_bits),
                Err(()) => {
                    dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
                    return MoltObject::none().bits();
                }
            }
        };

        if !set_attr(crate::NameId::DunderName, name_bits)
            || !set_attr(
                crate::NameId::DunderQualname,
                crate::object::layout::code_qualname_bits(code_ptr),
            )
            || !set_attr(crate::NameId::DunderModule, module_bits)
            || !set_attr(crate::NameId::DunderDefaults, defaults_bits)
            || !set_attr(crate::NameId::DunderKwdefaults, kwdefaults_bits)
            || !set_attr(
                crate::NameId::DunderDoc,
                crate::object::layout::code_doc_bits(code_ptr),
            )
        {
            dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
            return MoltObject::none().bits();
        }
        crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
        if exception_pending(_py) {
            dec_ref_bits(_py, MoltObject::from_ptr(func_ptr).bits());
            return MoltObject::none().bits();
        }

        MoltObject::from_ptr(func_ptr).bits()
    }
}

pub(crate) fn molt_function_set_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        function_set_builtin_class(_py, func_bits, false)
    })
}

/// Explicit METH_NOARGS/METH_O module-function promotion. The positional count
/// declares the public convention; it is checked against the executable before
/// any callable state is published.
pub(crate) fn molt_function_set_fixed_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    positional_count: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(count) = obj_from_bits(positional_count).as_int() else {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "fixed builtin positional count must be an integer",
            );
        };
        let Some(convention) = u64::try_from(count).ok().and_then(
            molt_cpython_abi::api::cfunction::CFunctionConvention::from_fixed_positional_count,
        ) else {
            return raise_exception::<_>(
                py,
                BuiltinExc::ValueError,
                "fixed builtin positional count must be zero or one",
            );
        };
        function_set_builtin_class_with_convention(py, func_bits, false, Some(convention))
    })
}

/// Assign the native callable class before publication. The defining-class
/// calling convention is a distinct subtype, not a second function layout.
pub(crate) fn function_set_builtin_class(
    _py: &PyToken<'_>,
    func_bits: u64,
    has_defining_class: bool,
) -> u64 {
    function_set_builtin_class_with_convention(_py, func_bits, has_defining_class, None)
}

fn function_set_builtin_class_with_convention(
    _py: &PyToken<'_>,
    func_bits: u64,
    has_defining_class: bool,
    fixed_convention: Option<molt_cpython_abi::api::cfunction::CFunctionConvention>,
) -> u64 {
    let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
        return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
    };
    unsafe {
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
        }
        let globals = function_globals_bits(func_ptr);
        let module = if globals == 0 || obj_from_bits(globals).is_none() {
            None
        } else {
            crate::builtins::module_table::existing_module_dictionary(_py).and_then(|dictionary| {
                crate::builtins::containers::dict_entries(crate::ptr_from_bits(dictionary))
                    .iter()
                    .filter(|row| row.hash.is_some())
                    .map(|row| row.value)
                    .find(|bits| {
                        obj_from_bits(*bits).as_ptr().is_some_and(|module| {
                            object_type_id(module) == TYPE_ID_MODULE
                                && module_dict_bits(module) == globals
                        })
                    })
                    .inspect(|bits| inc_ref_bits(_py, *bits))
            })
        };
        let _module_owner = module
            .and_then(|bits| obj_from_bits(bits).as_ptr())
            .map(|__ptr| crate::PtrDropGuard::new(_py, __ptr));
        let spec = super::native_callable::NativeCallableSpec {
            kind: if has_defining_class {
                super::native_callable::NativeCallableKind::CMethod
            } else {
                super::native_callable::NativeCallableKind::Function
            },
            self_bits: module,
            fixed_convention,
            ..super::native_callable::NativeCallableSpec::uncached_function()
        };
        if !super::native_callable::configure_native_callable(_py, func_ptr, spec) {
            return MoltObject::none().bits();
        }
    }
    MoltObject::none().bits()
}

/// Preflight is repeated after attribute writes because dictionary replacement
/// may reenter Python. Only the final preparation is used for publication.
unsafe fn prepare_function_metadata_code(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    code_bits: u64,
    signature: Option<crate::object::layout::PreparedCodeSignature>,
    lexical: Option<(u64, u64, crate::object::layout::CodeExecutionKind)>,
    declaration: [u64; 2],
) -> Result<crate::object::layout::PreparedFunctionCode, ()> {
    unsafe {
        let prepared = crate::object::layout::prepare_function_code_bits(
            py, func_ptr, code_bits, None, signature,
        )?
        .with_declaration_metadata(py, declaration[0], declaration[1])?;
        match lexical {
            Some((freevars, cellvars, kind)) => {
                prepared.with_lexical_metadata(py, freevars, cellvars, kind)
            }
            None => Ok(prepared),
        }
    }
}

unsafe fn publish_function_metadata_code(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    prepared: crate::object::layout::PreparedFunctionCode,
    user_mutation: bool,
) {
    unsafe {
        let replacing = crate::object::layout::function_code_bits(func_ptr) != 0;
        let displaced = prepared.publish(py, func_ptr);
        // Fresh construction must retain the zero epoch used by baked-default
        // guards. Reentry may already have attached code during initialization.
        if user_mutation || replacing {
            crate::object::layout::bump_function_mutation_version(func_ptr);
        }
        let detached = crate::call::bind::detach_callable_ic_caches();
        // All scalar, code, version, and cache-visible derived state is coherent
        // before either detached cache edges or displaced metadata can finalize.
        detached.release(py);
        displaced.release(py);
    }
}

/// Borrowed input views share one initializer regardless of the transport ABI.
/// The caller keeps every edge alive across attribute writes and publication.
struct FunctionMetadata {
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
    lexical: Option<(u64, u64, crate::object::layout::CodeExecutionKind)>,
}

fn function_metadata_target(py: &PyToken<'_>, func_bits: u64) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(func_bits).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_FUNCTION
    {
        return Some(ptr);
    }
    raise_exception::<u64>(py, BuiltinExc::TypeError, "expected function");
    None
}

fn initialize_function_metadata(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    metadata: FunctionMetadata,
    code_bits: u64,
    bind_kind_bits: u64,
) -> u64 {
    let FunctionMetadata {
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
        lexical,
    } = metadata;

    let set_attr = |name: crate::NameId, value_bits: u64| unsafe {
        crate::call::class_init::function_set_attr_name(_py, func_ptr, name, value_bits)
    };

    let code_ptr = if obj_from_bits(code_bits).is_none() {
        if lexical
            .is_some_and(|(_, _, kind)| kind != crate::object::layout::CodeExecutionKind::Direct)
        {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "task execution kind requires a code object",
            );
        }
        None
    } else {
        let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected code object");
        };
        if unsafe { object_type_id(code_ptr) } != TYPE_ID_CODE {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected code object");
        }
        Some(code_ptr)
    };

    let signature = if code_ptr.is_some() {
        match crate::object::layout::prepare_code_signature(
            _py,
            [
                arg_names_bits,
                posonly_bits,
                kwonly_bits,
                vararg_bits,
                varkw_bits,
            ],
        ) {
            Ok(signature) => Some(signature),
            Err(()) => return MoltObject::none().bits(),
        }
    } else {
        None
    };
    if code_ptr.is_some()
        && unsafe {
            prepare_function_metadata_code(
                _py,
                func_ptr,
                code_bits,
                signature,
                lexical,
                [qualname_bits, doc_bits],
            )
        }
        .is_err()
    {
        return MoltObject::none().bits();
    }

    if code_ptr.is_none()
        && let Some((freevars, cellvars, _)) = lexical
        && unsafe {
            crate::object::layout::validate_code_lexical_metadata(_py, None, freevars, cellvars)
        }
        .is_err()
    {
        return MoltObject::none().bits();
    }

    if !set_attr(crate::NameId::DunderName, name_bits)
        || !set_attr(crate::NameId::DunderQualname, qualname_bits)
        || !set_attr(crate::NameId::DunderModule, module_bits)
        || !set_attr(crate::NameId::DunderDefaults, defaults_bits)
        || !set_attr(crate::NameId::DunderKwdefaults, kwdefaults_bits)
        || !set_attr(crate::NameId::DunderDoc, doc_bits)
    {
        return MoltObject::none().bits();
    }
    if code_ptr.is_none()
        && (!set_attr(crate::NameId::DunderMoltArgNames, arg_names_bits)
            || !set_attr(crate::NameId::DunderMoltPosonly, posonly_bits)
            || !set_attr(crate::NameId::DunderMoltKwonlyNames, kwonly_bits)
            || !set_attr(crate::NameId::DunderMoltVararg, vararg_bits)
            || !set_attr(crate::NameId::DunderMoltVarkw, varkw_bits))
    {
        return MoltObject::none().bits();
    }

    unsafe {
        if function_set_globals_from_module_name(_py, func_ptr, module_bits).is_err() {
            return MoltObject::none().bits();
        }
    }

    if code_ptr.is_some() {
        let prepared = match unsafe {
            prepare_function_metadata_code(
                _py,
                func_ptr,
                code_bits,
                signature,
                lexical,
                [qualname_bits, doc_bits],
            )
        } {
            Ok(prepared) => prepared,
            Err(()) => return MoltObject::none().bits(),
        };
        unsafe { publish_function_metadata_code(_py, func_ptr, prepared, false) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }

    if !obj_from_bits(bind_kind_bits).is_none()
        && !set_attr(crate::NameId::DunderMoltBindKind, bind_kind_bits)
    {
        return MoltObject::none().bits();
    }

    unsafe {
        crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
    }

    MoltObject::none().bits()
}

#[allow(
    clippy::too_many_arguments,
    reason = "the body takes the intrinsic ABI's parameters one for one"
)]
pub(crate) fn molt_function_init_metadata_attached_body(
    _py: &crate::PyToken<'_>,
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
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(func_ptr) = function_metadata_target(_py, func_bits) else {
            return MoltObject::none().bits();
        };
        initialize_function_metadata(
            _py,
            func_ptr,
            FunctionMetadata {
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
                lexical: None,
            },
            code_bits,
            bind_kind_bits,
        )
    })
}

pub(crate) fn molt_function_init_metadata_packed_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    metadata_bits: u64,
    code_bits: u64,
    bind_kind_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(func_ptr) = function_metadata_target(_py, func_bits) else {
            return MoltObject::none().bits();
        };

        let Some(metadata_ptr) = obj_from_bits(metadata_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected metadata tuple");
        };
        unsafe {
            if object_type_id(metadata_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected metadata tuple");
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
        if metadata.len() != 14 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "metadata tuple must contain 14 items",
            );
        }

        let execution_kind = match obj_from_bits(metadata[11]).as_int() {
            Some(0) => crate::object::layout::CodeExecutionKind::Direct,
            Some(1) => crate::object::layout::CodeExecutionKind::Generator,
            Some(2) => crate::object::layout::CodeExecutionKind::Coroutine,
            Some(3) => crate::object::layout::CodeExecutionKind::AsyncGenerator,
            _ => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "metadata execution kind must be an int in range 0..=3",
                );
            }
        };

        initialize_function_metadata(
            _py,
            func_ptr,
            FunctionMetadata {
                name_bits: metadata[0],
                qualname_bits: metadata[1],
                module_bits: metadata[2],
                arg_names_bits: metadata[3],
                posonly_bits: metadata[4],
                kwonly_bits: metadata[5],
                vararg_bits: metadata[6],
                varkw_bits: metadata[7],
                defaults_bits: metadata[8],
                kwdefaults_bits: metadata[9],
                doc_bits: metadata[10],
                lexical: Some((metadata[12], metadata[13], execution_kind)),
            },
            code_bits,
            bind_kind_bits,
        )
    })
}

unsafe fn function_set_globals_from_module_name(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    module_bits: u64,
) -> Result<(), ()> {
    unsafe {
        let active_globals = crate::builtins::frames::frame_stack_active_globals_bits();
        if active_globals != 0 {
            function_set_globals_bits(_py, func_ptr, active_globals)?;
            return Ok(());
        }
        let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
            return Ok(());
        };
        let module_bits = crate::builtins::module_table::module_lookup(_py, &module_name)
            .map_err(|_| ())?
            .unwrap_or(0);
        let _module_owner = obj_from_bits(module_bits)
            .as_ptr()
            .map(|__ptr| crate::PtrDropGuard::new(_py, __ptr));
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            return Ok(());
        };
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return Ok(());
        }
        let globals_bits = module_dict_bits(module_ptr);
        if globals_bits != 0 && !obj_from_bits(globals_bits).is_none() {
            function_set_globals_bits(_py, func_ptr, globals_bits)?;
        }
    }
    Ok(())
}

pub(crate) fn molt_function_set_defaults_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    defaults_bits: u64,
    kwdefaults_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
            }
        }

        let set_attr = |name: crate::NameId, value_bits: u64| unsafe {
            crate::call::class_init::function_set_attr_name(_py, func_ptr, name, value_bits)
        };

        if !set_attr(crate::NameId::DunderDefaults, defaults_bits)
            || !set_attr(crate::NameId::DunderKwdefaults, kwdefaults_bits)
        {
            return MoltObject::none().bits();
        }

        unsafe {
            crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
        }

        MoltObject::none().bits()
    })
}

/// Read the shared callable-shape mutation version as an inline int. The
/// compile-time defaults-devirt guard calls this once per direct call site and
/// branches on `version == 0` (the baked code/default shape is still current)
/// vs `!= 0` (a runtime defaults, signature, bind-kind, or `__code__` mutation
/// occurred, so the call must consult live authorities). A non-function or
/// null argument yields 0; such call sites never bake a function default.
pub(crate) fn molt_function_defaults_version_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let version = obj_from_bits(func_bits)
            .as_ptr()
            .map(|func_ptr| unsafe {
                if object_type_id(func_ptr) == TYPE_ID_FUNCTION {
                    function_mutation_version(func_ptr)
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

pub(crate) fn molt_function_get_code_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
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

pub(crate) fn molt_function_get_globals_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "expected function");
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

/// CPython code_replace's complete keyword/type/error boundary. Metadata
/// cloning keeps the original compiled entry, trampoline, lexical layout and
/// frame identity; execution changes fail before allocating or publishing code.
pub(crate) fn molt_code_replace_attached_body(
    _py: &crate::PyToken<'_>,
    args: u64,
    kwargs: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        use crate::builtins::native_arguments::{NativeArguments, bind_named_slots};
        use crate::object::layout::{
            CODE_REPLACE_FIELDS, CodeReplaceEffect, CodeReplaceField, CodeReplaceValue,
            CodeReplacementMetadata,
        };
        use crate::object::ops_compare::{CompareBoolOutcome, compare_object_eq_bool};
        use num_traits::ToPrimitive;
        let Some(call) = NativeArguments::read(py, "replace", args, kwargs) else {
            return MoltObject::none().bits();
        };
        let Some(receiver) = call.receiver(py) else {
            return MoltObject::none().bits();
        };
        let Some(source) = obj_from_bits(receiver)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_CODE })
        else {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "replace requires a code object",
            );
        };
        if !call.values().is_empty() {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "replace() takes no positional arguments",
            );
        }
        let names = CODE_REPLACE_FIELDS
            .each_ref()
            .map(|spec| spec.name.bits(py));
        if names.contains(&0) {
            return MoltObject::none().bits();
        }
        let Some(bound) = bind_named_slots(
            py,
            "replace",
            &[],
            call.vector_keyword_view(),
            &names,
            0,
            [None; 18],
        ) else {
            return MoltObject::none().bits();
        };
        let mut metadata = unsafe { CodeReplacementMetadata::from_code(source) };
        // Clinic converts in declaration order, then code_replace_impl checks
        // nonnegative integers in that same order, before constructing a clone.
        let mut integers = [None; 18];
        for (index, (spec, value)) in CODE_REPLACE_FIELDS.iter().zip(bound.iter()).enumerate() {
            let Some(value) = *value else { continue };
            match spec.value {
                CodeReplaceValue::Int => {
                    let error = format!(
                        "'{}' object cannot be interpreted as an integer",
                        type_name(py, obj_from_bits(value))
                    );
                    let Some(number) =
                        crate::builtins::numbers::index_bigint_from_obj(py, value, &error)
                    else {
                        return MoltObject::none().bits();
                    };
                    let Some(number) = number.to_i32() else {
                        return raise_exception::<_>(
                            py,
                            BuiltinExc::OverflowError,
                            "Python int too large to convert to C int",
                        );
                    };
                    integers[index] = Some(number);
                }
                kind => {
                    let (expected, class) = match kind {
                        CodeReplaceValue::Bytes => ("bytes", builtin_classes(py).bytes),
                        CodeReplaceValue::Tuple => ("tuple", builtin_classes(py).tuple),
                        CodeReplaceValue::String => ("str", builtin_classes(py).str),
                        CodeReplaceValue::Int => unreachable!(),
                    };
                    if !crate::isinstance_runtime(py, value, class).unwrap_or(false) {
                        if exception_pending(py) {
                            return MoltObject::none().bits();
                        }
                        let got = if obj_from_bits(value).is_none() {
                            std::borrow::Cow::Borrowed("None")
                        } else {
                            type_name(py, obj_from_bits(value))
                        };
                        return raise_exception::<_>(
                            py,
                            BuiltinExc::TypeError,
                            &format!(
                                "replace() argument '{}' must be {}, not {}",
                                spec.name.as_str(),
                                expected,
                                got
                            ),
                        );
                    }
                }
            }
        }
        for (spec, number) in CODE_REPLACE_FIELDS.iter().zip(integers) {
            if number.is_some_and(|number| number < 0) {
                return raise_exception::<_>(
                    py,
                    BuiltinExc::ValueError,
                    &format!("{} must be a positive integer", spec.name.as_str()),
                );
            }
        }
        for (index, (spec, value)) in CODE_REPLACE_FIELDS.iter().zip(bound.iter()).enumerate() {
            let Some(value) = *value else { continue };
            if spec.effect == CodeReplaceEffect::Execution {
                let Ok(current) = (unsafe { spec.field.read(py, source) }) else {
                    return MoltObject::none().bits();
                };
                let normalized = integers[index].map_or(value, |number| {
                    MoltObject::from_int(i64::from(number)).bits()
                });
                let same =
                    compare_object_eq_bool(py, obj_from_bits(current), obj_from_bits(normalized));
                dec_ref_bits(py, current);
                match same {
                    CompareBoolOutcome::True => continue,
                    CompareBoolOutcome::Error => return MoltObject::none().bits(),
                    CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {}
                }
                return raise_exception::<_>(
                    py,
                    BuiltinExc::TypeError,
                    &format!(
                        "code.replace() cannot change {}: compiled code has no CPython bytecode or interpreter frame; dynamic execution is unsupported (spec 0216)",
                        spec.name.as_str()
                    ),
                );
            }
            match spec.field {
                CodeReplaceField::Filename => metadata.filename = value,
                CodeReplaceField::Name => metadata.name = value,
                CodeReplaceField::Qualname => metadata.qualname = value,
                CodeReplaceField::Firstlineno => {
                    metadata.firstlineno =
                        i64::from(integers[index].expect("validated integer metadata"))
                }
                _ => unreachable!("execution fields were refused above"),
            }
        }
        let clone =
            unsafe { crate::object::builders::clone_code_obj_with_metadata(py, source, metadata) };
        if clone.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(clone).bits()
        }
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the body takes the intrinsic ABI's parameters one for one"
)]
pub(crate) fn molt_code_new_attached_body(
    _py: &crate::PyToken<'_>,
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
    crate::with_attached_entry_nopanic!(_py, _py, {
        let filename_obj = obj_from_bits(filename_bits);
        let Some(filename_ptr) = filename_obj.as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "code filename must be str");
        };
        unsafe {
            if object_type_id(filename_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "code filename must be str",
                );
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "code name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, BuiltinExc::TypeError, "code name must be str");
            }
        }
        if !crate::object::builders::code_metadata_slot_edge(
            crate::object::layout::CodeMetadataSlot::Linetable,
            linetable_bits,
        ) {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "code linetable must be bytes or None",
            );
        }
        let Some(argcount) = to_i64(obj_from_bits(argcount_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "code argcount must be int");
        };
        let Some(posonlyargcount) = to_i64(obj_from_bits(posonlyargcount_bits)) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "code posonlyargcount must be int",
            );
        };
        let Some(kwonlyargcount) = to_i64(obj_from_bits(kwonlyargcount_bits)) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "code kwonlyargcount must be int",
            );
        };
        if argcount < 0 || posonlyargcount < 0 || kwonlyargcount < 0 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "code arg counts must be >= 0",
            );
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
            if !crate::object::builders::code_metadata_slot_edge(
                crate::object::layout::CodeMetadataSlot::Varnames,
                varnames_bits,
            ) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
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
            if !crate::object::builders::code_metadata_slot_edge(
                crate::object::layout::CodeMetadataSlot::Names,
                names_bits,
            ) {
                if varnames_owned {
                    dec_ref_bits(_py, varnames_bits);
                }
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
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

pub(crate) fn molt_bound_method_new_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        bound_method_new(_py, func_bits, self_bits, true)
    })
}

pub(crate) fn bound_method_new(
    _py: &PyToken<'_>,
    func_bits: u64,
    self_bits: u64,
    native_binding: bool,
) -> u64 {
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
        return raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "bound method expects function object",
        );
    };
    unsafe {
        // If func_bits is already a BOUND_METHOD, unwrap to its inner function
        // so we don't fail the TYPE_ID_FUNCTION check below. This happens when
        // inline int/float/bool attribute fallback passes a bound method through
        // the builtin_class_method_bits path.
        if object_type_id(func_ptr) == TYPE_ID_BOUND_METHOD {
            let inner_func_bits = bound_method_func_bits(func_ptr);
            return bound_method_new(_py, inner_func_bits, self_bits, native_binding);
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
                BuiltinExc::TypeError,
                "bound method expects callable object",
            );
        }
    }
    allocate_bound_method(_py, func_bits, self_bits, native_binding)
}

/// Explicit binding preserves an already-bound function and does not apply
/// descriptor admission. Python MethodType and C PyMethod_New validate their
/// distinct public policies before reaching this one allocation authority.
pub(crate) fn explicit_bound_method_new(_py: &PyToken<'_>, func_bits: u64, self_bits: u64) -> u64 {
    allocate_bound_method(_py, func_bits, self_bits, false)
}

fn allocate_bound_method(
    _py: &PyToken<'_>,
    func_bits: u64,
    self_bits: u64,
    native_binding: bool,
) -> u64 {
    let ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        let method_bits = {
            let func_class_bits = obj_from_bits(func_bits)
                .as_ptr()
                .map(|ptr| unsafe { object_class_bits(ptr) })
                .unwrap_or(0);
            if native_binding
                && let Some(kind) =
                    crate::builtins::functions::native_callable::NativeCallableKind::from_class(
                        _py,
                        func_class_bits,
                    )
            {
                kind.bound_class(_py)
            } else {
                crate::builtins::types::method_class(_py)
            }
        };
        if method_bits == 0 || exception_pending(_py) {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return MoltObject::none().bits();
        }
        {
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
}

/// # Safety
/// `self_ptr_bits` must encode a valid closure storage pointer and `offset`
/// must be within the allocated payload.
pub(crate) unsafe fn molt_closure_load_attached_body(
    _py: &crate::PyToken<'_>,
    self_ptr_bits: u64,
    offset: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
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
/// `self_ptr_bits` must encode the base of a live Molt object's closure payload
/// (not an interior pointer or standalone word), and `offset` must be within it.
pub(crate) unsafe fn molt_closure_store_attached_body(
    _py: &crate::PyToken<'_>,
    self_ptr_bits: u64,
    offset: u64,
    bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(self_ptr) = crate::provenance::abi::mut_ptr::<u8>(self_ptr_bits) else {
                return MoltObject::none().bits();
            };
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let Some(offset) = usize::try_from(offset).ok() else {
                return MoltObject::none().bits();
            };
            crate::object::payload_refs::store_borrowed(_py, self_ptr, offset, bits);
            MoltObject::none().bits()
        })
    }
}

#[cfg(test)]
#[path = "closure_storage_tests.rs"]
mod closure_storage_tests;

#[cfg(test)]
mod function_type_execution_kind_tests {
    use super::*;
    use crate::builtins::exceptions::clear_exception;
    use crate::object::layout::{
        code_callable_identity, code_publish_lexical_metadata, code_set_signature_bits,
        function_closure_bits, function_code_bits, function_fn_ptr, function_trampoline_ptr,
    };
    use crate::{
        HEADER_FLAG_FUNC_REQUIRES_BINDER, dict_set_in_place, function_dict_bits,
        header_from_obj_ptr, ptr_from_bits,
    };

    extern "C" fn opaque_context_probe(__context: u64, _context: u64, value: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        value
    }

    extern "C" fn lexical_closure_probe(__context: u64, _closure: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        MoltObject::none().bits()
    }

    #[test]
    fn native_configuration_refusal_preserves_public_function_and_dictionary() {
        use crate::builtins::functions::native_callable::{
            NativeCallableKind as Kind, NativeCallableSpec, configure_native_callable,
        };
        use crate::object::function_metadata::FunctionMetadataField as Field;
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let classes = builtin_classes(py);
                let key = dynamic_attr_name_bits(py, b"__text_signature__").unwrap();
                let original = dynamic_attr_name_bits(py, b"original_callable").unwrap();
                for (index, kind) in [
                    Kind::Constructor,
                    Kind::Function,
                    Kind::CMethod,
                    Kind::MethodDescriptor,
                    Kind::WrapperDescriptor,
                    Kind::ClassMethodDescriptor,
                    Kind::MethodWrapper,
                ]
                .into_iter()
                .enumerate()
                {
                    let function = alloc_runtime_function_obj(
                        py,
                        lexical_closure_probe as *const () as usize as u64,
                        1,
                    );
                    assert!(!function.is_null());
                    let bits = MoltObject::from_ptr(function).bits();
                    let value = MoltObject::from_int(37).bits();
                    crate::molt_set_attr_name_attached_body(py, bits, key, value);
                    drop(Field::Name.replace_deferred(py, function, original));
                    let dictionary = function_dict_bits(function);
                    inc_ref_bits(py, dictionary);
                    let class = object_class_bits(function);
                    let flags = (*header_from_obj_ptr(function)).load_metadata_flags();
                    let name = format!("native_configuration_refusal_unique_{index}");
                    set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                        max_memory: Some(0),
                        ..Default::default()
                    })));
                    let reset = RestoreTracker;
                    assert!(!configure_native_callable(
                        py,
                        function,
                        NativeCallableSpec {
                            kind,
                            name: Some(super::native_callable::NativeCallableName::External(&name)),
                            owner: Some(classes.object),
                            self_bits: Some(classes.object),
                            cache: None,
                            text_signature: Some("($self, /)"),
                            fixed_convention: None,
                            identity: None,
                        }
                    ));
                    drop(reset);
                    assert!(exception_pending(py));
                    clear_exception(py);
                    assert_eq!(object_class_bits(function), class);
                    assert_eq!(function_dict_bits(function), dictionary);
                    assert_eq!(Field::Name.load(function), Some(original));
                    assert_eq!(Field::QualName.load(function), None);
                    assert_eq!(Field::TextSignature.load(function), None);
                    assert_eq!(Field::Owner.load(function), None);
                    assert_eq!(Field::SelfValue.load(function), None);
                    assert_eq!(
                        (*header_from_obj_ptr(function)).load_metadata_flags()
                            & HEADER_FLAG_FUNC_REQUIRES_BINDER,
                        flags & HEADER_FLAG_FUNC_REQUIRES_BINDER,
                    );
                    assert_eq!(
                        crate::dict_get_in_place(py, ptr_from_bits(dictionary), key,),
                        Ok(Some(value))
                    );

                    // A later preparation failure occurs after the new name
                    // exists. It still must not replace class or display fields.
                    assert!(!configure_native_callable(
                        py,
                        function,
                        NativeCallableSpec {
                            kind,
                            name: Some(super::native_callable::NativeCallableName::External(&name)),
                            owner: Some(original),
                            ..NativeCallableSpec::uncached_function()
                        }
                    ));
                    assert!(exception_pending(py));
                    clear_exception(py);
                    assert_eq!(object_class_bits(function), class);
                    assert_eq!(Field::Name.load(function), Some(original));
                    assert_eq!(Field::TextSignature.load(function), None);
                    assert_eq!(function_dict_bits(function), dictionary);
                    assert_eq!(
                        crate::dict_get_in_place(py, ptr_from_bits(dictionary), key,),
                        Ok(Some(value))
                    );
                    dec_ref_bits(py, dictionary);
                    dec_ref_bits(py, bits);
                }
                dec_ref_bits(py, original);
                dec_ref_bits(py, key);
            }
        });
    }

    static SIGNATURE_PROBE_MODE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static SIGNATURE_PROBE_FUNCTION: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static SIGNATURE_PROBE_DICTIONARY: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static SIGNATURE_PROBE_CALLS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    extern "C" fn signature_key_hash(__context: u64, _self_bits: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        {
            let py = __py;
            let key = dynamic_attr_name_bits(py, b"__text_signature__").unwrap();
            let result = crate::molt_hash_builtin_attached_body(py, key);
            dec_ref_bits(py, key);
            result
        }
    }

    extern "C" fn signature_key_eq(__context: u64, _left: u64, _right: u64) -> u64 {
        assert_eq!(
            __context,
            crate::state::molt_attached_context(),
            "Python callback context identity"
        );
        let __py = unsafe { crate::PyToken::from_attached_context(__context) };

        use std::sync::atomic::Ordering::SeqCst;
        {
            let py = __py;
            SIGNATURE_PROBE_CALLS.fetch_add(1, SeqCst);
            match SIGNATURE_PROBE_MODE.load(SeqCst) {
                0 => {
                    raise_exception::<u64>(py, BuiltinExc::RuntimeError, "signature lookup refused")
                }
                1 => {
                    unsafe {
                        crate::object::field_storage::replace_dictionary(
                            py,
                            ptr_from_bits(SIGNATURE_PROBE_FUNCTION.load(SeqCst)),
                            Some(SIGNATURE_PROBE_DICTIONARY.load(SeqCst)),
                        )
                        .unwrap();
                    }
                    MoltObject::from_bool(true).bits()
                }
                3 => {
                    SIGNATURE_PROBE_MODE.store(2, SeqCst);
                    function_set_builtin_class(py, SIGNATURE_PROBE_FUNCTION.load(SeqCst), false);
                    MoltObject::from_bool(true).bits()
                }
                _ => MoltObject::from_bool(true).bits(),
            }
        }
    }

    #[test]
    fn native_signature_transfer_uses_live_dictionary_and_actual_key_owner() {
        use crate::object::function_metadata::FunctionMetadataField as Field;
        use std::sync::atomic::Ordering::SeqCst;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let name = dynamic_attr_name_bits(py, b"SignatureKey").unwrap();
                let class = crate::molt_class_new_attached_body(py, name);
                crate::molt_class_set_base_attached_body(py, class, builtin_classes(py).object);
                let eq_name = dynamic_attr_name_bits(py, b"__eq__").unwrap();
                let eq = alloc_runtime_function_obj(
                    py,
                    signature_key_eq as *const () as usize as u64,
                    2,
                );
                assert!(!eq.is_null());
                let eq_bits = MoltObject::from_ptr(eq).bits();
                crate::molt_set_attr_name_attached_body(py, class, eq_name, eq_bits);
                let hash_name = dynamic_attr_name_bits(py, b"__hash__").unwrap();
                let hash_function = alloc_runtime_function_obj(
                    py,
                    signature_key_hash as *const () as usize as u64,
                    1,
                );
                assert!(!hash_function.is_null());
                let hash_bits = MoltObject::from_ptr(hash_function).bits();
                crate::molt_set_attr_name_attached_body(py, class, hash_name, hash_bits);
                let class_ptr = ptr_from_bits(class);
                crate::object::class_finish_definition(py, class_ptr).unwrap();
                let size = crate::object::layout::class_cached_layout_size(class_ptr).unwrap();
                let collision = crate::object::builders::alloc_class_instance(py, size, class);
                crate::object::gc::gc_publish_initialized(py, ptr_from_bits(collision));
                let key = dynamic_attr_name_bits(py, b"__text_signature__").unwrap();
                for mode in [1, 2, 3] {
                    let replace_dictionary = mode == 1;
                    let function = alloc_runtime_function_obj(
                        py,
                        lexical_closure_probe as *const () as usize as u64,
                        1,
                    );
                    assert!(!function.is_null());
                    let bits = MoltObject::from_ptr(function).bits();
                    let original_class = object_class_bits(function);
                    let original = crate::alloc_dict_with_pairs(py, &[]);
                    let replacement =
                        crate::alloc_dict_with_pairs(py, &[key, MoltObject::from_int(29).bits()]);
                    assert!(!original.is_null() && !replacement.is_null());
                    assert!(crate::dict_set_in_place(
                        py,
                        original,
                        collision,
                        MoltObject::from_int(17).bits(),
                    ));
                    crate::object::field_storage::replace_dictionary(
                        py,
                        function,
                        Some(MoltObject::from_ptr(original).bits()),
                    )
                    .unwrap();
                    SIGNATURE_PROBE_FUNCTION.store(bits, SeqCst);
                    SIGNATURE_PROBE_DICTIONARY
                        .store(MoltObject::from_ptr(replacement).bits(), SeqCst);
                    SIGNATURE_PROBE_MODE.store(0, SeqCst);
                    SIGNATURE_PROBE_CALLS.store(0, SeqCst);
                    function_set_builtin_class(py, bits, false);
                    assert!(exception_pending(py));
                    clear_exception(py);
                    assert_eq!(SIGNATURE_PROBE_CALLS.load(SeqCst), 1);
                    assert_eq!(object_class_bits(function), original_class);
                    assert_eq!(Field::TextSignature.load(function), None);
                    assert_eq!(
                        function_dict_bits(function),
                        MoltObject::from_ptr(original).bits()
                    );
                    assert_eq!(crate::dict_len(original), 1);

                    SIGNATURE_PROBE_MODE.store(mode, SeqCst);
                    function_set_builtin_class(py, bits, false);
                    assert!(!exception_pending(py));
                    assert_eq!(
                        object_class_bits(function),
                        builtin_classes(py).builtin_function_or_method
                    );
                    assert_eq!(
                        Field::TextSignature.load(function),
                        Some(MoltObject::from_int(if replace_dictionary { 29 } else { 17 }).bits())
                    );
                    // The displaced public alias keeps its binding; only the
                    // selected live dictionary loses the actual found key.
                    assert_eq!(crate::dict_len(original), usize::from(replace_dictionary));
                    assert_eq!(
                        crate::dict_len(replacement),
                        usize::from(!replace_dictionary)
                    );
                    dec_ref_bits(py, bits);
                    dec_ref_bits(py, MoltObject::from_ptr(original).bits());
                    dec_ref_bits(py, MoltObject::from_ptr(replacement).bits());
                }
                SIGNATURE_PROBE_FUNCTION.store(0, SeqCst);
                SIGNATURE_PROBE_DICTIONARY.store(0, SeqCst);
                for bits in [
                    key, collision, hash_bits, hash_name, eq_bits, eq_name, class, name,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn builtin_promotion_moves_public_text_signature_into_its_typed_owner() {
        use crate::object::function_metadata::FunctionMetadataField as Field;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let name = dynamic_attr_name_bits(py, b"__text_signature__").unwrap();
                let extra = dynamic_attr_name_bits(py, b"promotion_control").unwrap();
                let original = alloc_string(py, b"($module, value=19, /)");
                assert!(!original.is_null());
                let original_bits = MoltObject::from_ptr(original).bits();
                let owners = || (*crate::header_from_obj_ptr(original)).ref_count_snapshot();
                let baseline = owners();
                for signature in [Some(original_bits), Some(MoltObject::none().bits()), None] {
                    let function = alloc_runtime_function_obj(
                        py,
                        lexical_closure_probe as *const () as usize as u64,
                        1,
                    );
                    assert!(!function.is_null());
                    let bits = MoltObject::from_ptr(function).bits();
                    let executable = function_fn_ptr(function);
                    if let Some(signature) = signature {
                        let _ = crate::molt_set_attr_name_attached_body(py, bits, name, signature);
                    }
                    let _ = crate::molt_set_attr_name_attached_body(
                        py,
                        bits,
                        extra,
                        MoltObject::from_int(41).bits(),
                    );
                    assert!(!exception_pending(py));
                    assert_eq!(Field::TextSignature.load(function), None);
                    let dictionary = crate::object::field_storage::current_dictionary(py, function)
                        .unwrap()
                        .unwrap();
                    inc_ref_bits(py, dictionary); // independent public-dictionary observer
                    let dictionary_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
                    assert_eq!(dict_get_in_place(py, dictionary_ptr, name), Ok(signature));
                    if let Some(signature) = signature {
                        let observed = crate::molt_get_attr_name_attached_body(py, bits, name);
                        assert_eq!(observed, signature);
                        dec_ref_bits(py, observed);
                    }

                    let _ = function_set_builtin_class(py, bits, false);
                    assert!(!exception_pending(py));
                    assert_eq!(Field::TextSignature.load(function), signature);
                    assert_eq!(function_fn_ptr(function), executable);
                    assert_eq!(
                        crate::call_callable1(py, bits, MoltObject::from_int(17).bits()),
                        MoltObject::none().bits()
                    );
                    assert!(!exception_pending(py));
                    assert_eq!(dict_get_in_place(py, dictionary_ptr, name), Ok(None));
                    assert_eq!(
                        dict_get_in_place(py, dictionary_ptr, extra),
                        Ok(Some(MoltObject::from_int(41).bits()))
                    );
                    if signature == Some(original_bits) {
                        assert_eq!(
                            owners(),
                            baseline + 1,
                            "only the typed owner retains the signature"
                        );
                    }

                    // An old dictionary alias cannot rewrite sealed native
                    // metadata, including on a repeated builtin publication.
                    assert!(dict_set_in_place(
                        py,
                        dictionary_ptr,
                        name,
                        MoltObject::from_int(99).bits()
                    ));
                    let _ = function_set_builtin_class(py, bits, false);
                    let observed = crate::molt_get_attr_name_attached_body(py, bits, name);
                    assert_eq!(observed, signature.unwrap_or(MoltObject::none().bits()));
                    dec_ref_bits(py, observed);
                    let _ = crate::molt_set_attr_name_attached_body(
                        py,
                        bits,
                        name,
                        MoltObject::none().bits(),
                    );
                    assert!(exception_pending(py), "native text signature is readonly");
                    clear_exception(py);
                    assert_eq!(Field::TextSignature.load(function), signature);
                    dec_ref_bits(py, bits);
                    dec_ref_bits(py, dictionary);
                    assert_eq!(owners(), baseline);
                }
                for bits in [name, extra, original_bits] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    fn empty_globals(_py: &crate::PyToken<'_>) -> u64 {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    #[test]
    fn function_type_rejects_synthetic_opaque_context_code() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let function = alloc_runtime_function_obj(
                _py,
                opaque_context_probe as *const () as usize as u64,
                1,
            );
            assert!(!function.is_null());
            let context = alloc_tuple(_py, &[MoltObject::from_int(17).bits()]);
            assert!(!context.is_null());
            let context_bits = MoltObject::from_ptr(context).bits();
            unsafe {
                function_set_closure_bits(
                    _py,
                    function,
                    context_bits,
                    FunctionCallAbi::OpaqueContextFirst,
                );
            }
            dec_ref_bits(_py, context_bits);
            let code_bits = unsafe { ensure_function_code_bits(_py, function) };
            let code = obj_from_bits(code_bits).as_ptr().unwrap();
            assert_eq!(
                unsafe { code_callable_identity(code) }.unwrap().call_abi,
                FunctionCallAbi::OpaqueContextFirst
            );
            let globals_bits = empty_globals(_py);
            let rejected = unsafe { function_type_new_from_args(_py, &[code_bits, globals_bits]) };
            assert!(obj_from_bits(rejected).is_none());
            assert!(exception_pending(_py));
            clear_exception(_py);
            dec_ref_bits(_py, globals_bits);
            dec_ref_bits(_py, MoltObject::from_ptr(function).bits());
        });
    }

    #[test]
    fn function_type_preserves_genuine_lexical_code_abi() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name = alloc_string(_py, b"captured");
            let empty = alloc_tuple(_py, &[]);
            assert!(!name.is_null() && !empty.is_null());
            let name_bits = MoltObject::from_ptr(name).bits();
            let empty_bits = MoltObject::from_ptr(empty).bits();
            let freevars = alloc_tuple(_py, &[name_bits]);
            assert!(!freevars.is_null());
            let freevars_bits = MoltObject::from_ptr(freevars).bits();
            let code = alloc_code_obj(
                _py,
                name_bits,
                name_bits,
                1,
                MoltObject::none().bits(),
                empty_bits,
                empty_bits,
                0,
                0,
                0,
            );
            assert!(!code.is_null());
            let code_bits = MoltObject::from_ptr(code).bits();
            assert!(unsafe { code_publish_lexical_metadata(_py, code, freevars_bits, empty_bits) });

            let cell_bits = crate::object::cells::molt_cell_new_attached_body(
                _py,
                MoltObject::from_int(47).bits(),
            );
            let closure = alloc_tuple(_py, &[cell_bits]);
            assert!(!closure.is_null());
            let closure_bits = MoltObject::from_ptr(closure).bits();
            let source = alloc_runtime_function_obj(
                _py,
                lexical_closure_probe as *const () as usize as u64,
                0,
            );
            assert!(!source.is_null());
            unsafe {
                function_set_closure_bits(
                    _py,
                    source,
                    closure_bits,
                    FunctionCallAbi::LexicalClosureFirst,
                );
                assert!(function_set_code_bits(_py, source, code_bits));
            }
            assert_eq!(
                unsafe { code_callable_identity(code) }.unwrap().call_abi,
                FunctionCallAbi::LexicalClosureFirst
            );

            let globals_bits = empty_globals(_py);
            let rebound_bits = unsafe {
                function_type_new_from_args(
                    _py,
                    &[
                        code_bits,
                        globals_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        closure_bits,
                    ],
                )
            };
            assert!(!obj_from_bits(rebound_bits).is_none());
            assert!(!exception_pending(_py));
            let rebound = obj_from_bits(rebound_bits).as_ptr().unwrap();
            assert_eq!(unsafe { function_code_bits(rebound) }, code_bits);
            assert_eq!(
                unsafe { function_call_abi(rebound) },
                FunctionCallAbi::LexicalClosureFirst
            );
            assert_eq!(unsafe { function_closure_bits(rebound) }, closure_bits);

            // Vector binding, public type calls and the published __new__
            // descriptor lend the same tuple; no route may copy or steal it.
            for route in 0..3 {
                let before = unsafe { (*header_from_obj_ptr(closure)).ref_count_snapshot() };
                let class = builtin_classes(_py).function;
                let direct_new = route == 2;
                let callable = if direct_new {
                    let name = dynamic_attr_name_bits(_py, b"__new__").unwrap();
                    let method = crate::molt_get_attr_name_attached_body(_py, class, name);
                    dec_ref_bits(_py, name);
                    method
                } else {
                    class
                };
                let name = dynamic_attr_name_bits(_py, b"closure").unwrap();
                let result = if route == 0 {
                    unsafe {
                        function_type_new_from_call(
                            _py,
                            &[code_bits, globals_bits],
                            crate::builtins::native_arguments::NativeKeywords::vector(
                                &[name],
                                &[closure_bits],
                            ),
                        )
                    }
                } else {
                    unsafe {
                        let builder = crate::molt_callargs_new_attached_body(_py, 3, 1);
                        if direct_new {
                            crate::molt_callargs_push_pos_attached_body(_py, builder, class);
                        }
                        crate::molt_callargs_push_pos_attached_body(_py, builder, code_bits);
                        crate::molt_callargs_push_pos_attached_body(_py, builder, globals_bits);
                        crate::molt_callargs_push_kw_attached_body(
                            _py,
                            builder,
                            name,
                            closure_bits,
                        );
                        crate::molt_call_bind_attached_body(_py, callable, builder)
                    }
                };
                dec_ref_bits(_py, name);
                assert!(!exception_pending(_py));
                let function = obj_from_bits(result).as_ptr().unwrap();
                assert_eq!(unsafe { function_closure_bits(function) }, closure_bits);
                assert_eq!(
                    unsafe { function_call_abi(function) },
                    FunctionCallAbi::LexicalClosureFirst
                );
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(closure)).ref_count_snapshot() },
                    before + 1
                );
                dec_ref_bits(_py, result);
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(closure)).ref_count_snapshot() },
                    before
                );
                if direct_new {
                    dec_ref_bits(_py, callable);
                }
            }

            for bits in [
                rebound_bits,
                MoltObject::from_ptr(source).bits(),
                globals_bits,
                closure_bits,
                cell_bits,
                code_bits,
                freevars_bits,
                empty_bits,
                name_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn function_type_reconstructs_task_dispatch_from_code() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = alloc_string(_py, b"reconstructed_generator");
            let empty_tuple_ptr = alloc_tuple(_py, &[]);
            let builtins_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!name_ptr.is_null() && !empty_tuple_ptr.is_null() && !builtins_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let empty_tuple_bits = MoltObject::from_ptr(empty_tuple_ptr).bits();
            let builtins_bits = MoltObject::from_ptr(builtins_ptr).bits();
            let builtins_name = dynamic_attr_name_bits(_py, b"__builtins__").unwrap();
            let globals_ptr = alloc_dict_with_pairs(_py, &[builtins_name, builtins_bits]);
            assert!(!globals_ptr.is_null());
            let globals_bits = MoltObject::from_ptr(globals_ptr).bits();

            let code_ptr = alloc_code_obj(
                _py,
                name_bits,
                name_bits,
                1,
                MoltObject::none().bits(),
                empty_tuple_bits,
                empty_tuple_bits,
                0,
                0,
                0,
            );
            assert!(!code_ptr.is_null());
            let code_bits = MoltObject::from_ptr(code_ptr).bits();
            unsafe {
                code_set_signature_bits(
                    _py,
                    code_ptr,
                    empty_tuple_bits,
                    MoltObject::from_int(0).bits(),
                    empty_tuple_bits,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                )
                .expect("valid compiled signature");
                let source = alloc_function_obj(_py, 41, 0);
                assert!(!source.is_null());
                function_set_trampoline_ptr(_py, source, 73);
                assert!(function_set_code_bits(_py, source, code_bits));
                assert_eq!(
                    crate::object::layout::code_publish_execution_kind(
                        code_ptr,
                        crate::object::layout::CodeExecutionKind::Generator,
                    ),
                    Ok(())
                );

                let rebound_bits = function_type_new_from_args(_py, &[code_bits, globals_bits]);
                assert!(!exception_pending(_py));
                let rebound = obj_from_bits(rebound_bits)
                    .as_ptr()
                    .expect("reconstructed function");
                assert_eq!(function_fn_ptr(rebound), 41);
                assert_eq!(function_trampoline_ptr(rebound), 73);
                assert_eq!(function_code_bits(rebound), code_bits);
                assert_eq!(
                    crate::call::function::function_code_execution_kind(rebound),
                    Some(crate::object::layout::CodeExecutionKind::Generator),
                );
                dec_ref_bits(_py, rebound_bits);
                dec_ref_bits(_py, MoltObject::from_ptr(source).bits());
            }
            for bits in [
                code_bits,
                globals_bits,
                builtins_name,
                builtins_bits,
                empty_tuple_bits,
                name_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }
}

#[cfg(test)]
mod wasm_runtime_callable_tests {
    use super::*;

    #[test]
    fn resolved_constructor_identity_does_not_reenter_target_registry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Resolve and publish first. Only read-only predicates run while the
            // target registry is exclusively held: registration would deadlock.
            let new = crate::builtins::methods::object_method_bits(py, crate::NameId::DunderNew)
                .expect("lookup succeeds");
            let init = crate::builtins::methods::object_method_bits(py, crate::NameId::DunderInit)
                .expect("lookup succeeds");
            let call = crate::builtins::methods::type_method_bits(py, crate::NameId::DunderCall)
                .expect("lookup succeeds");
            let exception_init =
                crate::builtins::exceptions::exception_method_bits(py, crate::NameId::DunderInit)
                    .expect("lookup succeeds");
            let (default_new, policy, matches) = {
                let _targets = native_callable_targets().lock().unwrap();
                unsafe {
                    (
                        crate::call::type_policy::resolved_new_is_default_object_new(new),
                        crate::call::type_policy::resolved_constructor_init_policy(new, init),
                        [
                            (new, fn_key!(crate::molt_object_new_bound)),
                            (init, fn_key!(crate::molt_object_init)),
                            (call, fn_key!(crate::molt_type_call)),
                            (
                                exception_init,
                                fn_key!(crate::builtins::exceptions::molt_exception_init_owned),
                            ),
                        ]
                        .map(|(actual, expected)| {
                            crate::call::type_policy::callable_matches_runtime_symbol(
                                actual, expected,
                            )
                        }),
                    )
                }
            };
            // Report semantic failures after dropping the registry guard so a
            // failing assertion cannot poison unrelated callable consumers.
            assert!(default_new);
            assert_eq!(
                policy,
                crate::call::type_policy::InitArgPolicy::RejectConstructorArgs
            );
            assert!(matches.into_iter().all(|matched| matched));
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn named_key_construction_never_registers_the_key_as_an_address() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let target = crate::molt_type_init as *const ();
            let key = fn_addr!(crate::molt_type_init);
            let ptr = alloc_runtime_function_obj(py, key, 3);
            assert!(!ptr.is_null());
            // A named key is not an address: both the registry route and the
            // object's stored target must name the generated table's function.
            assert_eq!(runtime_callable_target_ptr(key), Some(target));
            assert_eq!(
                unsafe { crate::object::layout::function_call_target_ptr(ptr) },
                target
            );
            crate::dec_ref_bits(py, MoltObject::from_ptr(ptr).bits());
        });
    }

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
    fn poll_dispatch_never_treats_a_named_key_as_an_address() {
        let past_the_poll_interval = wasm_callables::RUNTIME_POLL_CALLABLE_KEY_BASE
            + wasm_callables::WASM_POLL_SLOT_MAX_OFFSET
            + 1;
        assert!(runtime_callable_key_is_named(past_the_poll_interval));
        assert!(runtime_poll_call_target(past_the_poll_interval).is_none());
        // A reserved runtime callable's key is a key, not a poll: poll
        // dispatch refuses it instead of calling a non-poll function.
        let reserved = fn_key!(crate::molt_type_init);
        assert!(runtime_callable_key_is_named(reserved));
        assert!(runtime_poll_call_target(reserved).is_none());
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

include!("../../attached_exports_generated/builtins/functions/function_abi.rs");
