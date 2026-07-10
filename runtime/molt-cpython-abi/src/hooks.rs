//! Runtime hooks vtable — pluggable object allocators from `molt-lang-runtime`.
//!
//! `molt-lang-cpython-abi` cannot depend on `molt-lang-runtime` (that would
//! create a circular dependency). Instead, the runtime registers concrete
//! implementations at startup via [`set_runtime_hooks`].
//!
//! Every hook function uses `extern "C"` with primitive types so the
//! registration call works across crate boundaries without monomorphisation.
//!
//! ## Handle encoding
//!
//! All `u64` parameters and return values are raw `MoltObject` bit patterns
//! (QNAN-boxed). `0` is reserved for "null / not found / error".

use std::sync::OnceLock;

pub const MOLT_BUFFER_MAX_NDIM: usize = 64;
pub const MOLT_BUFFER_FORMAT_CAP: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MoltBufferView {
    pub data: *mut u8,
    pub len: u64,
    pub backing_capacity: u64,
    pub readonly: u32,
    pub ndim: u32,
    pub itemsize: u64,
    pub offset: isize,
    pub owner: u64,
    pub base: u64,
    pub shape: [isize; MOLT_BUFFER_MAX_NDIM],
    pub strides: [isize; MOLT_BUFFER_MAX_NDIM],
    pub format: [u8; MOLT_BUFFER_FORMAT_CAP],
}

impl Default for MoltBufferView {
    fn default() -> Self {
        let mut format = [0; MOLT_BUFFER_FORMAT_CAP];
        format[0] = b'B';
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            backing_capacity: 0,
            readonly: 1,
            ndim: 1,
            itemsize: 1,
            offset: 0,
            owner: 0,
            base: 0,
            shape: [0; MOLT_BUFFER_MAX_NDIM],
            strides: [0; MOLT_BUFFER_MAX_NDIM],
            format,
        }
    }
}

/// Vtable of runtime-provided object-allocation and inspection hooks.
/// All function pointers are `extern "C"` for ABI stability across crate boundaries.
#[derive(Clone, Copy)]
#[allow(dead_code)]
#[repr(C)]
pub struct RuntimeHooks {
    // ── Allocation ────────────────────────────────────────────────────────────
    /// Allocate a UTF-8 string object. Returns handle bits, 0 on failure.
    pub alloc_str: unsafe extern "C" fn(data: *const u8, len: usize) -> u64,
    /// Allocate a bytes object. Returns handle bits, 0 on failure.
    pub alloc_bytes: unsafe extern "C" fn(data: *const u8, len: usize) -> u64,
    /// Allocate an int object from a signed 64-bit value. Returns handle bits, 0 on failure.
    pub int_from_i64: unsafe extern "C" fn(value: i64) -> u64,
    /// Allocate an int object from an unsigned 64-bit value. Returns handle bits, 0 on failure.
    pub int_from_u64: unsafe extern "C" fn(value: u64) -> u64,
    /// Convert an int-compatible object to i64. Returns -1 on failure.
    pub int_as_i64: unsafe extern "C" fn(bits: u64) -> i64,
    /// Checked int-compatible object conversion to i64. Returns 0 on success, -1 on failure.
    pub int_as_i64_checked: unsafe extern "C" fn(bits: u64, out: *mut i64) -> std::os::raw::c_int,
    /// Checked int-compatible object conversion to u64. Returns 0 on success, -1 on failure.
    pub int_as_u64_checked: unsafe extern "C" fn(bits: u64, out: *mut u64) -> std::os::raw::c_int,
    /// Allocate an empty list. Returns handle bits.
    pub alloc_list: unsafe extern "C" fn() -> u64,
    /// Append `item_bits` to the list at `list_bits`.
    pub list_append: unsafe extern "C" fn(list_bits: u64, item_bits: u64),
    /// Return the number of items in a list.
    pub list_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return the bits of item `i` in the list, or 0 if out of range.
    pub list_item: unsafe extern "C" fn(bits: u64, i: usize) -> u64,
    /// Store `val_bits` at index `i` of the list, writing the previous occupant's
    /// bits into `*out_old`. Returns 1 on success, 0 when `i` is out of range or
    /// `list_bits` is not a list. Backs the indexed `PyList_SetItem`/`SET_ITEM`
    /// store: CPython stores directly (`Py_SETREF`), stealing the new reference
    /// and releasing the old — the ABI releases `*out_old` and honors the steal.
    pub list_set: unsafe extern "C" fn(
        list_bits: u64,
        i: usize,
        val_bits: u64,
        out_old: *mut u64,
    ) -> std::os::raw::c_int,
    /// Insert `item_bits` before (clamped) index `where_` in the list, shifting
    /// subsequent elements right. Returns 0 on success, -1 on a non-list.
    /// Routes to the runtime `PyList_Insert` authority (`ins1` semantics).
    pub list_insert:
        unsafe extern "C" fn(list_bits: u64, where_: isize, item_bits: u64) -> std::os::raw::c_int,
    /// Sort the list in place via the runtime comparison authority. Returns 0 on
    /// success, -1 with a pending exception on error (uncomparable elements).
    pub list_sort: unsafe extern "C" fn(list_bits: u64) -> std::os::raw::c_int,
    /// Reverse the list in place. Returns 0 on success, -1 on a non-list.
    pub list_reverse: unsafe extern "C" fn(list_bits: u64) -> std::os::raw::c_int,
    /// Replace `list[ilow:ihigh]` with the elements of the list `itemlist_bits`
    /// (or delete the slice when `itemlist_bits == 0`), growing/shrinking the
    /// backing store. Returns 0 on success, -1 on a non-list receiver. Backs
    /// `PyList_SetSlice` (`list_ass_slice`).
    pub list_set_slice: unsafe extern "C" fn(
        list_bits: u64,
        ilow: isize,
        ihigh: isize,
        itemlist_bits: u64,
    ) -> std::os::raw::c_int,
    /// Allocate a tuple of exactly `n` slots. Slots are uninitialized (None).
    pub alloc_tuple: unsafe extern "C" fn(n: usize) -> u64,
    /// Set slot `i` of tuple `bits` to `val_bits`.
    pub tuple_set: unsafe extern "C" fn(bits: u64, i: usize, val_bits: u64),
    /// Return the number of items in a tuple.
    pub tuple_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return the bits of item `i` in the tuple, or 0 if out of range.
    pub tuple_item: unsafe extern "C" fn(bits: u64, i: usize) -> u64,
    /// Allocate an empty dict. Returns handle bits.
    pub alloc_dict: unsafe extern "C" fn() -> u64,
    /// Insert or overwrite a key→value pair in the dict.
    pub dict_set: unsafe extern "C" fn(dict_bits: u64, key_bits: u64, val_bits: u64),
    /// Lookup `key_bits` in the dict. Returns 0 if not found.
    pub dict_get: unsafe extern "C" fn(dict_bits: u64, key_bits: u64) -> u64,
    /// Delete `key_bits` from the dict. Returns 0 on success, -1 on failure.
    pub dict_del: unsafe extern "C" fn(dict_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    /// Return the number of entries in a dict.
    pub dict_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Read the dict entry at insertion-order `index`, writing borrowed key/value
    /// bits into `*out_key`/`*out_val`. Returns 1 when an entry exists at `index`,
    /// 0 at end-of-dict or when `dict_bits` is not a dict. Allocation-free O(1)
    /// cursor step backing `PyDict_Next` (mirrors CPython's `ppos` index into the
    /// entry table); it must NOT set an exception (CPython `PyDict_Next` contract).
    pub dict_entry: unsafe extern "C" fn(
        dict_bits: u64,
        index: usize,
        out_key: *mut u64,
        out_val: *mut u64,
    ) -> std::os::raw::c_int,
    // ── Data access ───────────────────────────────────────────────────────────
    /// Return a pointer to the UTF-8 bytes of a string handle, writing the
    /// length into `*out_len`. Pointer is valid until next GC cycle.
    /// Returns null on error.
    pub str_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *const u8,
    /// Return a pointer to the raw bytes of a bytes handle.
    pub bytes_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *const u8,
    /// Acquire a typed strided buffer export owned by the runtime.
    pub buffer_acquire:
        unsafe extern "C" fn(bits: u64, out_view: *mut MoltBufferView) -> std::os::raw::c_int,
    /// Release a typed strided buffer export previously acquired from the runtime.
    pub buffer_release: unsafe extern "C" fn(view: *mut MoltBufferView) -> std::os::raw::c_int,
    /// Return obj.name using the runtime object model. Returns 0 when absent or unavailable.
    pub object_get_attr: unsafe extern "C" fn(obj_bits: u64, name_bits: u64) -> u64,
    /// Set obj.name using the runtime object model. Returns 0 on success, -1 on failure.
    pub object_set_attr:
        unsafe extern "C" fn(obj_bits: u64, name_bits: u64, value_bits: u64) -> std::os::raw::c_int,
    /// Return format(obj, spec) using the runtime object model. Returns 0 on error.
    pub object_format: unsafe extern "C" fn(obj_bits: u64, spec_bits: u64) -> u64,
    /// Format an `f64` as CPython's `repr(float)` / `str(float)` using the
    /// runtime's single float-format authority (`object::float_repr`). Writes
    /// up to `cap` UTF-8 bytes into `out` and returns the total byte length of
    /// the formatted string. When the return value exceeds `cap`, `out` is left
    /// untouched and the caller must retry with a buffer of at least that size.
    /// The ABI MUST NOT reimplement float formatting; Rust's own `{f}` breaks
    /// round-half-to-even ties differently from CPython.
    pub float_repr: unsafe extern "C" fn(value: f64, out: *mut u8, cap: usize) -> usize,
    pub sys_get_object_borrowed: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    // ── Type classification ───────────────────────────────────────────────────
    /// Classify a heap-pointer handle into a `MoltTypeTag` discriminant (u8).
    /// Used by `classify_handle` to fill in the SIMD type-tag table for heap types.
    pub classify_heap: unsafe extern "C" fn(bits: u64) -> u8,
    // ── Reference counting ────────────────────────────────────────────────────
    /// Increment the Molt reference count for a heap object.
    pub inc_ref: unsafe extern "C" fn(bits: u64),
    /// Decrement the Molt reference count; deallocate if it reaches zero.
    pub dec_ref: unsafe extern "C" fn(bits: u64),
    // ── Module / C-extension support ─────────────────────────────────────────
    /// Allocate a new Molt module object whose `__name__` is the UTF-8 string
    /// in `name_data[..name_len]`.  Returns module handle bits, 0 on failure.
    pub alloc_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Return the runtime-owned module dict handle for a Molt module object.
    pub module_get_dict: unsafe extern "C" fn(module_bits: u64) -> u64,
    /// Set `module_bits.__dict__[name_data[..name_len]] = value_bits`.
    /// `module_bits` must be a Molt module handle.  Returns 0 on success, -1 on failure.
    pub module_set_attr: unsafe extern "C" fn(
        module_bits: u64,
        name_data: *const u8,
        name_len: usize,
        value_bits: u64,
    ) -> std::os::raw::c_int,
    /// Register C-API module metadata and allocate per-module state when
    /// `module_state_size` is non-zero.
    pub module_capi_register: unsafe extern "C" fn(
        module_bits: u64,
        module_def_ptr: usize,
        module_state_size: u64,
    ) -> std::os::raw::c_int,
    /// Return the runtime-owned C-API module state pointer for a module.
    pub module_capi_get_state: unsafe extern "C" fn(module_bits: u64) -> *mut u8,
    /// Add `def -> module` to the process module-state registry.
    pub module_state_add:
        unsafe extern "C" fn(module_bits: u64, module_def_ptr: usize) -> std::os::raw::c_int,
    /// Find the module handle registered for a module definition pointer.
    pub module_state_find: unsafe extern "C" fn(module_def_ptr: usize) -> u64,
    /// Remove a module definition pointer from the module-state registry.
    pub module_state_remove: unsafe extern "C" fn(module_def_ptr: usize) -> std::os::raw::c_int,
    /// Register a `PyCFunction`-style C function pointer (`meth_addr`) as a
    /// callable Molt function.  `flags` follows CPython's `METH_*` bitmask.
    /// `name_data[..name_len]` is the function's `__name__`.  Returns the bits
    /// of the resulting Molt callable, 0 on failure (e.g. unsupported flags).
    pub register_c_function: unsafe extern "C" fn(
        meth_addr: u64,
        flags: std::os::raw::c_int,
        self_bits: u64,
        name_data: *const u8,
        name_len: usize,
    ) -> u64,
    /// Import the module named by the UTF-8 dotted path in
    /// `name_data[..name_len]` through the runtime import pipeline (package
    /// custody, static extension registry, sys.modules cache).  Returns an
    /// owned module handle, or 0 on failure with the import error left in
    /// the runtime pending-exception state.
    pub import_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Return non-zero when the runtime holds a pending Python exception.
    /// Lets ABI-side fallbacks avoid masking a real runtime error with a
    /// synthetic "without setting an exception" message.
    pub exception_pending: unsafe extern "C" fn() -> std::os::raw::c_int,
    // ── Numeric protocol (PyNumber_*) ─────────────────────────────────────────
    //
    // The runtime owns the single numeric authority: arbitrary-precision int
    // promotion, float coercion, operator-overload dispatch, and CPython-shaped
    // exception raising all live in `molt-lang-runtime`. The ABI MUST NOT
    // reimplement arithmetic (that silently wraps at 64 bits and masks the
    // exceptions CPython raises). These hooks route `PyNumber_*` straight to
    // that authority. Each returns result handle bits, or `0` with a pending
    // runtime exception on error (the ABI turns `0` into a NULL PyObject*).
    /// Binary numeric op. `op` is a [`NumberBinaryOp`] discriminant. Returns
    /// result bits, or 0 with a pending exception on error.
    pub number_binary_op: unsafe extern "C" fn(op: u32, a_bits: u64, b_bits: u64) -> u64,
    /// Unary numeric op. `op` is a [`NumberUnaryOp`] discriminant. Returns
    /// result bits, or 0 with a pending exception on error.
    pub number_unary_op: unsafe extern "C" fn(op: u32, a_bits: u64) -> u64,
    /// Ternary power `pow(base, exp, modulus)`. When `mod_bits` is `0` or None,
    /// computes two-argument `base ** exp`. Returns result bits, or 0 with a
    /// pending exception on error.
    pub number_power: unsafe extern "C" fn(a_bits: u64, b_bits: u64, mod_bits: u64) -> u64,
    // ── Mapping protocol (PyDict_*) ───────────────────────────────────────────
    //
    // The runtime owns dict iteration (copy / keys / values / items). The ABI
    // MUST NOT return an empty dict/list ignoring its argument — that is silent
    // data loss. This hook routes to the runtime dict authority. `op` is a
    // [`DictOp`] discriminant. Returns result bits, or 0 with a pending exception
    // on error.
    pub dict_op: unsafe extern "C" fn(op: u32, dict_bits: u64) -> u64,
    // ── Set protocol (PySet_*) ────────────────────────────────────────────────
    //
    // The runtime owns the single set authority (hash table, dedup, membership,
    // frozenset immutability, CPython-shaped exceptions) in
    // `molt-lang-runtime`. The ABI MUST NOT fake a set with a list (no dedup, no
    // hashed membership) or report every membership test as absent — both are
    // silent-wrong-answer. These hooks route `PySet_*` to that authority.
    /// Allocate a new set, optionally populated from `iterable_bits` (0 = empty
    /// set). Returns set handle bits, or 0 with a pending exception on error
    /// (e.g. a non-iterable argument → TypeError).
    pub set_new: unsafe extern "C" fn(iterable_bits: u64) -> u64,
    /// Return the number of elements in a set/frozenset, or -1 with a pending
    /// exception (SystemError) when `set_bits` is not a set/frozenset.
    pub set_size: unsafe extern "C" fn(set_bits: u64) -> std::os::raw::c_int,
    /// Membership test. Returns 1 (present) / 0 (absent) / -1 with a pending
    /// exception on error (TypeError for an unhashable key, SystemError for a
    /// non-set).
    pub set_contains: unsafe extern "C" fn(set_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    /// Add `key_bits` to the set. Returns 0 on success, -1 with a pending
    /// exception on error (TypeError for an unhashable key, SystemError for a
    /// non-set).
    pub set_add: unsafe extern "C" fn(set_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    /// Remove `key_bits` from the set if present. Returns 1 (found and removed)
    /// / 0 (absent) / -1 with a pending exception on error. Never raises
    /// KeyError (unlike `set.discard`).
    pub set_discard: unsafe extern "C" fn(set_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    // ── Object introspection (PyObject_Dir) ───────────────────────────────────
    //
    // The runtime owns `dir(obj)` (MRO walk, `__dict__`, `__dir__`). The ABI MUST
    // NOT return an empty list ignoring its argument. Returns a list handle, or 0
    // with a pending exception on error.
    pub object_dir: unsafe extern "C" fn(obj_bits: u64) -> u64,
    // ── Call protocol (PyObject_Call) ─────────────────────────────────────────
    //
    // The runtime owns the single call authority (`molt_call_bind`): compiled
    // functions, types, bound methods, kwargs binding, and CPython-shaped
    // exceptions all live there. Bridge proxies for Molt objects carry no
    // `tp_call`, so `PyObject_Call` on a bridge-managed callable (e.g. numpy's
    // `numpy.dtypes._add_dtype_helper`, a Molt-compiled function fetched via
    // `PyObject_GetAttrString`) MUST route through this hook instead of failing
    // "'<proxy-type>' object is not callable".
    /// Call a Molt callable. `args_bits` is a Molt tuple handle of positional
    /// arguments (0 = no positional args); `kwargs_bits` is a Molt dict handle
    /// (0 = no keyword args). Returns the result handle bits, or 0 with the
    /// error left in the runtime pending-exception state.
    pub object_call:
        unsafe extern "C" fn(callable_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64,
    // ── Foreign-object custody (C-extension objects into Molt) ────────────────
    //
    // When a genuine C-extension `PyObject*` (a numpy static type, an extension
    // instance, a descriptor, …) crosses *into* compiled Python, the bridge
    // wraps it in a first-class Molt heap object (`TYPE_ID_FOREIGN`) so that
    // Molt-side attribute access / calls resolve — the previous synthetic
    // `0xA11C…` identity token was not a valid `MoltObject` bit pattern, so
    // `DType.__name__` and friends failed to decode the handle. This hook
    // allocates the wrapper; the runtime owns the `TYPE_ID_FOREIGN` heap type,
    // its drop custody, and the getattr/setattr/call routing back through the
    // object's own CPython type slots (via `molt-cpython-abi` bridge functions).
    /// Allocate a `TYPE_ID_FOREIGN` wrapper around the C `PyObject*` at address
    /// `c_ptr`. Returns the wrapper handle bits, or 0 on failure. The strong
    /// reference custody (`Py_INCREF` on the C object) is handled by the bridge
    /// caller, not this hook.
    pub foreign_new: unsafe extern "C" fn(c_ptr: usize) -> u64,
}

/// Discriminants for [`RuntimeHooks::dict_op`]. Kept in sync with the match in
/// the runtime hook implementation (`hook_dict_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DictOp {
    Copy = 0,
    Keys = 1,
    Values = 2,
    Items = 3,
}

/// Discriminants for [`RuntimeHooks::number_binary_op`]. Kept in sync with the
/// match in the runtime hook implementation (`hook_number_binary_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NumberBinaryOp {
    Add = 0,
    Subtract = 1,
    Multiply = 2,
    TrueDivide = 3,
    FloorDivide = 4,
    Remainder = 5,
    Lshift = 6,
    Rshift = 7,
    And = 8,
    Or = 9,
    Xor = 10,
    MatrixMultiply = 11,
}

/// Discriminants for [`RuntimeHooks::number_unary_op`]. Kept in sync with the
/// match in the runtime hook implementation (`hook_number_unary_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NumberUnaryOp {
    Negative = 0,
    Positive = 1,
    Absolute = 2,
    Invert = 3,
}

/// Global hook table, set once by `molt-lang-runtime` at init time.
static RUNTIME_HOOKS: OnceLock<RuntimeHooks> = OnceLock::new();

/// Register the runtime hook vtable. Must be called exactly once, before any
/// C extension function is invoked. Panics if called more than once.
///
/// # Safety
/// All function pointers in `hooks` must remain valid for the lifetime of the process.
pub unsafe fn set_runtime_hooks(hooks: RuntimeHooks) {
    RUNTIME_HOOKS
        .set(hooks)
        .unwrap_or_else(|_| panic!("molt_cpython_abi: runtime hooks already registered"));
}

/// Idempotent variant of [`set_runtime_hooks`] that silently no-ops when the
/// hook vtable has already been registered.
///
/// Intended for test setup paths where multiple integration tests in the same
/// crate each call `init()` on a shared `OnceLock` — the first wins, the rest
/// observe the already-registered state without panicking.  Production hosts
/// should prefer [`set_runtime_hooks`] so that double-registration with
/// mismatched hooks is caught loudly.
///
/// Returns `true` if this call installed the hooks, or `false` if a prior
/// registration was already in effect (the passed-in `hooks` is dropped).
///
/// # Safety
/// Same as [`set_runtime_hooks`]: every function pointer in `hooks` must
/// remain valid for the lifetime of the process.
pub unsafe fn try_set_runtime_hooks(hooks: RuntimeHooks) -> bool {
    RUNTIME_HOOKS.set(hooks).is_ok()
}

/// C-callable registration entry point for `molt-lang-runtime`.
///
/// # Safety
/// Same as `set_runtime_hooks`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_register_hooks(hooks: *const RuntimeHooks) {
    if hooks.is_null() {
        return;
    }
    unsafe {
        set_runtime_hooks(*hooks);
    }
}

/// Access the runtime hooks. Returns `None` if hooks have not been registered
/// (pre-init or test contexts). Callers must degrade gracefully (return None/0).
#[inline]
pub fn hooks() -> Option<&'static RuntimeHooks> {
    RUNTIME_HOOKS.get()
}

// ─── No-op stubs for pre-init or test use ────────────────────────────────────

unsafe extern "C" fn stub_alloc_str(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_bytes(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_int_from_i64(_value: i64) -> u64 {
    0
}
unsafe extern "C" fn stub_int_from_u64(_value: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_int_as_i64(_bits: u64) -> i64 {
    -1
}
unsafe extern "C" fn stub_int_as_i64_checked(_bits: u64, _out: *mut i64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_int_as_u64_checked(_bits: u64, _out: *mut u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_alloc_list() -> u64 {
    0
}
unsafe extern "C" fn stub_list_append(_list_bits: u64, _item_bits: u64) {}
unsafe extern "C" fn stub_list_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_list_item(_bits: u64, _i: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_list_set(
    _list_bits: u64,
    _i: usize,
    _val_bits: u64,
    _out_old: *mut u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_list_insert(
    _list_bits: u64,
    _where_: isize,
    _item_bits: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_sort(_list_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_reverse(_list_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_set_slice(
    _list_bits: u64,
    _ilow: isize,
    _ihigh: isize,
    _itemlist_bits: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_alloc_tuple(_n: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_tuple_set(_bits: u64, _i: usize, _val: u64) {}
unsafe extern "C" fn stub_tuple_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_tuple_item(_bits: u64, _i: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_dict() -> u64 {
    0
}
unsafe extern "C" fn stub_dict_set(_d: u64, _k: u64, _v: u64) {}
unsafe extern "C" fn stub_dict_get(_d: u64, _k: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_dict_del(_d: u64, _k: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_dict_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_dict_entry(
    _dict_bits: u64,
    _index: usize,
    _out_key: *mut u64,
    _out_val: *mut u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_str_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    c"".as_ptr().cast()
}
unsafe extern "C" fn stub_bytes_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    std::ptr::null()
}
unsafe extern "C" fn stub_buffer_acquire(
    _bits: u64,
    out_view: *mut MoltBufferView,
) -> std::os::raw::c_int {
    if !out_view.is_null() {
        unsafe {
            *out_view = MoltBufferView::default();
        }
    }
    -1
}
unsafe extern "C" fn stub_buffer_release(view: *mut MoltBufferView) -> std::os::raw::c_int {
    if !view.is_null() {
        unsafe {
            *view = MoltBufferView::default();
        }
    }
    0
}
unsafe extern "C" fn stub_object_get_attr(_obj: u64, _name: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_object_set_attr(
    _obj: u64,
    _name: u64,
    _value: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_format(_obj: u64, _spec: u64) -> u64 {
    0
}
/// Degraded fallback used only when the runtime float-format authority is not
/// installed (pure-ABI unit tests without `molt-lang-runtime` linked). This is
/// NOT CPython-exact — the real authority lives in the runtime and is wired
/// through the `float_repr` hook. It exists solely so ABI-only tests do not
/// crash; production always installs the runtime hook.
unsafe extern "C" fn stub_float_repr(value: f64, out: *mut u8, cap: usize) -> usize {
    let s = if value.is_nan() {
        "nan".to_string()
    } else if value.is_infinite() {
        if value < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        }
    } else {
        let raw = format!("{value}");
        if raw.contains('.') || raw.contains('e') || raw.contains('E') {
            raw
        } else {
            format!("{raw}.0")
        }
    };
    let bytes = s.as_bytes();
    if bytes.len() <= cap && !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    }
    bytes.len()
}
unsafe extern "C" fn stub_sys_get_object_borrowed(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_classify_heap(_bits: u64) -> u8 {
    crate::abi_types::MoltTypeTag::Other as u8
}
unsafe extern "C" fn stub_inc_ref(_bits: u64) {}
unsafe extern "C" fn stub_dec_ref(_bits: u64) {}
unsafe extern "C" fn stub_alloc_module(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_module_get_dict(_module_bits: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_module_set_attr(
    _m: u64,
    _data: *const u8,
    _len: usize,
    _v: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_module_capi_register(
    _module_bits: u64,
    _module_def_ptr: usize,
    _module_state_size: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_module_capi_get_state(_module_bits: u64) -> *mut u8 {
    std::ptr::null_mut()
}
unsafe extern "C" fn stub_module_state_add(
    _module_bits: u64,
    _module_def_ptr: usize,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_module_state_find(_module_def_ptr: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_module_state_remove(_module_def_ptr: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_register_c_function(
    _meth: u64,
    _flags: std::os::raw::c_int,
    _self_bits: u64,
    _data: *const u8,
    _len: usize,
) -> u64 {
    0
}
unsafe extern "C" fn stub_import_module(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_exception_pending() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_number_binary_op(_op: u32, _a: u64, _b: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_number_unary_op(_op: u32, _a: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_number_power(_a: u64, _b: u64, _mod_bits: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_dict_op(_op: u32, _dict: u64) -> u64 {
    0
}
// Set stubs fail closed with the CPython error sentinel (0 / -1). Without the
// runtime set authority registered, returning a fake success would silently
// corrupt set semantics; the API wrappers turn these sentinels into NULL / -1
// with an exception set.
unsafe extern "C" fn stub_set_new(_iterable: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_set_size(_set: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_set_contains(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_set_add(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_set_discard(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_dir(_obj: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_object_call(_callable: u64, _args: u64, _kwargs: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_foreign_new(_c_ptr: usize) -> u64 {
    0
}

/// A no-op hooks table used when the runtime hasn't registered yet.
pub const STUB_HOOKS: RuntimeHooks = RuntimeHooks {
    alloc_str: stub_alloc_str,
    alloc_bytes: stub_alloc_bytes,
    int_from_i64: stub_int_from_i64,
    int_from_u64: stub_int_from_u64,
    int_as_i64: stub_int_as_i64,
    int_as_i64_checked: stub_int_as_i64_checked,
    int_as_u64_checked: stub_int_as_u64_checked,
    alloc_list: stub_alloc_list,
    list_append: stub_list_append,
    list_len: stub_list_len,
    list_item: stub_list_item,
    list_set: stub_list_set,
    list_insert: stub_list_insert,
    list_sort: stub_list_sort,
    list_reverse: stub_list_reverse,
    list_set_slice: stub_list_set_slice,
    alloc_tuple: stub_alloc_tuple,
    tuple_set: stub_tuple_set,
    tuple_len: stub_tuple_len,
    tuple_item: stub_tuple_item,
    alloc_dict: stub_alloc_dict,
    dict_set: stub_dict_set,
    dict_get: stub_dict_get,
    dict_del: stub_dict_del,
    dict_len: stub_dict_len,
    dict_entry: stub_dict_entry,
    str_data: stub_str_data,
    bytes_data: stub_bytes_data,
    buffer_acquire: stub_buffer_acquire,
    buffer_release: stub_buffer_release,
    object_get_attr: stub_object_get_attr,
    object_set_attr: stub_object_set_attr,
    object_format: stub_object_format,
    float_repr: stub_float_repr,
    sys_get_object_borrowed: stub_sys_get_object_borrowed,
    classify_heap: stub_classify_heap,
    inc_ref: stub_inc_ref,
    dec_ref: stub_dec_ref,
    alloc_module: stub_alloc_module,
    module_get_dict: stub_module_get_dict,
    module_set_attr: stub_module_set_attr,
    module_capi_register: stub_module_capi_register,
    module_capi_get_state: stub_module_capi_get_state,
    module_state_add: stub_module_state_add,
    module_state_find: stub_module_state_find,
    module_state_remove: stub_module_state_remove,
    register_c_function: stub_register_c_function,
    import_module: stub_import_module,
    exception_pending: stub_exception_pending,
    number_binary_op: stub_number_binary_op,
    number_unary_op: stub_number_unary_op,
    number_power: stub_number_power,
    dict_op: stub_dict_op,
    set_new: stub_set_new,
    set_size: stub_set_size,
    set_contains: stub_set_contains,
    set_add: stub_set_add,
    set_discard: stub_set_discard,
    object_dir: stub_object_dir,
    object_call: stub_object_call,
    foreign_new: stub_foreign_new,
};

/// Return the registered hooks or fall back to the no-op stubs.
/// Use this in API functions where a partial result is better than a panic.
#[inline]
pub fn hooks_or_stubs() -> &'static RuntimeHooks {
    RUNTIME_HOOKS.get().unwrap_or(&STUB_HOOKS)
}
