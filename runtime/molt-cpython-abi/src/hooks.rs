//! Runtime hooks vtable — pluggable object allocators from `molt-lang-runtime`.
//!
//! `molt-lang-cpython-abi` cannot depend on `molt-lang-runtime` (that would
//! create a circular dependency). Instead, the runtime registers concrete
//! implementations at startup via [`try_set_runtime_hooks`].
//!
//! Every hook function uses `extern "C"` with primitive types so the
//! registration call works across crate boundaries without monomorphisation.
//!
//! ## Handle encoding
//!
//! All `u64` parameters and return values are raw `MoltObject` bit patterns
//! (QNAN-boxed). `0` is reserved for "null / not found / error".

use std::sync::OnceLock;

pub const HANDLE_RESULT_ERROR: i32 = -1;
pub const HANDLE_RESULT_MISSING: i32 = 0;
pub const HANDLE_RESULT_OK: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OwnedHandleResult {
    status: i32,
    _reserved: u32,
    bits: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BorrowedHandleResult {
    status: i32,
    _reserved: u32,
    bits: u64,
}

pub const EXCEPTION_SNAPSHOT_DICT: u32 = 1 << 0;
pub const EXCEPTION_SNAPSHOT_ARGS: u32 = 1 << 1;
pub const EXCEPTION_SNAPSHOT_NOTES: u32 = 1 << 2;
pub const EXCEPTION_SNAPSHOT_TRACEBACK: u32 = 1 << 3;
pub const EXCEPTION_SNAPSHOT_CONTEXT: u32 = 1 << 4;
pub const EXCEPTION_SNAPSHOT_CAUSE: u32 = 1 << 5;

/// One atomic runtime/ABI transaction for the complete public
/// `PyBaseExceptionObject` field set.  Every bit whose mask is present is a
/// non-zero handle.  A capture owns one reference to each present handle; a
/// commit borrows every handle.  Missing optional fields are represented only
/// by their absent mask bit, never by `Ok(0)`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ExceptionSnapshot {
    pub present_mask: u32,
    pub suppress_context: u32,
    pub dict: u64,
    pub args: u64,
    pub notes: u64,
    pub traceback: u64,
    pub context: u64,
    pub cause: u64,
}

pub enum DecodedHandleResult {
    Ok(u64),
    Missing,
    Error,
}

impl OwnedHandleResult {
    pub const fn ok(bits: u64) -> Self {
        if bits == 0 {
            return Self::error();
        }
        Self {
            status: HANDLE_RESULT_OK,
            _reserved: 0,
            bits,
        }
    }
    pub const fn missing() -> Self {
        Self {
            status: HANDLE_RESULT_MISSING,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn error() -> Self {
        Self {
            status: HANDLE_RESULT_ERROR,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn decode(self) -> DecodedHandleResult {
        match self.status {
            HANDLE_RESULT_OK if self.bits != 0 => DecodedHandleResult::Ok(self.bits),
            HANDLE_RESULT_MISSING if self.bits == 0 => DecodedHandleResult::Missing,
            _ => DecodedHandleResult::Error,
        }
    }
}

impl BorrowedHandleResult {
    pub const fn ok(bits: u64) -> Self {
        if bits == 0 {
            return Self::error();
        }
        Self {
            status: HANDLE_RESULT_OK,
            _reserved: 0,
            bits,
        }
    }
    pub const fn missing() -> Self {
        Self {
            status: HANDLE_RESULT_MISSING,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn error() -> Self {
        Self {
            status: HANDLE_RESULT_ERROR,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn decode(self) -> DecodedHandleResult {
        match self.status {
            HANDLE_RESULT_OK if self.bits != 0 => DecodedHandleResult::Ok(self.bits),
            HANDLE_RESULT_MISSING if self.bits == 0 => DecodedHandleResult::Missing,
            _ => DecodedHandleResult::Error,
        }
    }
}

pub const INT_BYTES_OK: std::os::raw::c_int = 0;
pub const INT_BYTES_OVERFLOW: std::os::raw::c_int = 1;
pub const INT_BYTES_NEGATIVE_UNSIGNED: std::os::raw::c_int = 2;
pub const INT_BYTES_INVALID: std::os::raw::c_int = -1;

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
    pub abi_magic: u64,
    pub abi_version: u32,
    pub struct_size: u32,
    pub gil_ensure: unsafe extern "C" fn() -> std::os::raw::c_int,
    pub gil_leave: unsafe extern "C" fn(state: std::os::raw::c_int),
    pub gil_release: unsafe extern "C" fn(),
    pub gil_restore: unsafe extern "C" fn(),
    pub gil_check: unsafe extern "C" fn() -> std::os::raw::c_int,
    pub runtime_is_initialized: unsafe extern "C" fn() -> std::os::raw::c_int,
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
    /// Return the low `width` bits of an int-compatible object. Returns 0 on success.
    pub int_as_u64_mask:
        unsafe extern "C" fn(bits: u64, width: u32, out: *mut u64) -> std::os::raw::c_int,
    /// Allocate an arbitrary-width int from a fixed-width byte string.
    pub int_from_bytes: unsafe extern "C" fn(
        data: *const u8,
        len: usize,
        little_endian: std::os::raw::c_int,
        signed: std::os::raw::c_int,
    ) -> u64,
    /// Encode an arbitrary-width int. Returns `INT_BYTES_*`; overflow still
    /// fills the output with the low bytes.
    pub int_to_bytes: unsafe extern "C" fn(
        bits: u64,
        data: *mut u8,
        len: usize,
        little_endian: std::os::raw::c_int,
        signed: std::os::raw::c_int,
    ) -> std::os::raw::c_int,
    /// Absolute bit length. Returns 0 on success, -1 on invalid input.
    pub int_num_bits: unsafe extern "C" fn(bits: u64, out: *mut usize) -> std::os::raw::c_int,
    /// Current `sys.get_int_max_str_digits()` authority; zero disables it.
    pub int_max_str_digits: unsafe extern "C" fn() -> usize,
    /// Allocate an empty list. Returns handle bits.
    pub alloc_list: unsafe extern "C" fn() -> u64,
    /// Allocate a list with its logical length established in one backing-store
    /// allocation. The ABI bridge owns the uninitialized-slot contract.
    pub alloc_list_presized: unsafe extern "C" fn(len: usize) -> u64,
    /// Append `item_bits` to the list at `list_bits`. `item_ptr` is the exact
    /// originating C object when the append crossed from CPython, or NULL for
    /// runtime-only appends. Preserving this origin makes `lst[-1] is item`
    /// exact and avoids rematerializing scalar carriers.
    pub list_append: unsafe extern "C" fn(
        list_bits: u64,
        item_bits: u64,
        item_ptr: *mut crate::abi_types::PyObject,
    ) -> std::os::raw::c_int,
    /// Return the number of items in a list.
    pub list_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return the bits of item `i` in the list, or 0 if out of range.
    pub list_item: unsafe extern "C" fn(bits: u64, i: usize) -> BorrowedHandleResult,
    /// Store `val_bits` at index `i` of the list, writing the previous occupant's
    /// bits into `*out_old`. Returns 1 on success, 0 when `i` is out of range or
    /// `list_bits` is not a list. Backs the indexed `PyList_SetItem`/`SET_ITEM`
    /// store: CPython stores directly (`Py_SETREF`), stealing the new reference
    /// and releasing the old — the ABI releases `*out_old` and honors the steal.
    pub list_set:
        unsafe extern "C" fn(list_bits: u64, i: usize, val_bits: u64) -> OwnedHandleResult,
    /// Insert `item_bits` before (clamped) index `where_` in the list, shifting
    /// subsequent elements right. `item_ptr` preserves the exact originating C
    /// object just like append. Returns 0 on success, -1 on any failed runtime
    /// or physical publication.
    pub list_insert: unsafe extern "C" fn(
        list_bits: u64,
        where_: isize,
        item_bits: u64,
        item_ptr: *mut crate::abi_types::PyObject,
    ) -> std::os::raw::c_int,
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
        future_pointers: *const *mut crate::abi_types::PyObject,
        future_len: usize,
    ) -> std::os::raw::c_int,
    /// Allocate a tuple of exactly `n` uninitialized slots. A zero handle is
    /// the only uninitialized sentinel; finalized tuples never contain it.
    pub alloc_tuple: unsafe extern "C" fn(n: usize) -> u64,
    /// Set the fixed slot `i` of an open, uniquely-owned tuple. `exact_pointer`
    /// is the physical object whose reference was stolen by PyTuple_SetItem.
    /// The hook never grows the tuple. `Missing` is the successful transition
    /// from an uninitialized zero slot; `Ok(bits)` replaces an initialized
    /// slot and transfers its old runtime edge; `Error` is failure.
    pub tuple_set: unsafe extern "C" fn(
        bits: u64,
        i: usize,
        val_bits: u64,
        exact_pointer: *mut crate::abi_types::PyObject,
    ) -> OwnedHandleResult,
    /// Return the number of items in a tuple.
    pub tuple_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return the bits of item `i` in the tuple, or 0 if out of range.
    pub tuple_item: unsafe extern "C" fn(bits: u64, i: usize) -> BorrowedHandleResult,
    /// Allocate an empty dict. Returns handle bits.
    pub alloc_dict: unsafe extern "C" fn() -> u64,
    /// Insert or overwrite a key→value pair in the dict.
    pub dict_set:
        unsafe extern "C" fn(dict_bits: u64, key_bits: u64, val_bits: u64) -> std::os::raw::c_int,
    /// Lookup `key_bits` in the dict. Returns 0 if not found.
    pub dict_get: unsafe extern "C" fn(dict_bits: u64, key_bits: u64) -> BorrowedHandleResult,
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
    pub object_get_attr: unsafe extern "C" fn(obj_bits: u64, name_bits: u64) -> OwnedHandleResult,
    /// Set obj.name using the runtime object model. Returns 0 on success, -1 on failure.
    pub object_set_attr:
        unsafe extern "C" fn(obj_bits: u64, name_bits: u64, value_bits: u64) -> std::os::raw::c_int,
    /// Return format(obj, spec) using the runtime object model. Returns 0 on error.
    pub object_format: unsafe extern "C" fn(obj_bits: u64, spec_bits: u64) -> OwnedHandleResult,
    /// Format an `f64` as CPython's `repr(float)` / `str(float)` using the
    /// runtime's single float-format authority (`object::float_repr`). Writes
    /// up to `cap` UTF-8 bytes into `out` and returns the total byte length of
    /// the formatted string. When the return value exceeds `cap`, `out` is left
    /// untouched and the caller must retry with a buffer of at least that size.
    /// The ABI MUST NOT reimplement float formatting; Rust's own `{f}` breaks
    /// round-half-to-even ties differently from CPython.
    pub float_repr: unsafe extern "C" fn(value: f64, out: *mut u8, cap: usize) -> usize,
    pub sys_get_object_borrowed:
        unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> BorrowedHandleResult,
    /// Resolve the current frame's effective builtins dict, or the interpreter
    /// default when no frame-specific override exists. Returns borrowed.
    pub eval_get_builtins_borrowed: unsafe extern "C" fn() -> BorrowedHandleResult,
    // ── Type classification ───────────────────────────────────────────────────
    /// Classify a heap-pointer handle into a `MoltTypeTag` discriminant (u8).
    /// Used by `classify_handle` to fill in the SIMD type-tag table for heap types.
    pub classify_heap: unsafe extern "C" fn(bits: u64) -> u8,
    /// Compute the CPython hash for a managed heap object. Returns `-1` only
    /// with a pending exception; every real `-1` hash is normalized to `-2`.
    pub object_hash: unsafe extern "C" fn(bits: u64) -> i64,
    // ── Reference counting ────────────────────────────────────────────────────
    /// Increment the Molt reference count for a heap object.
    pub inc_ref: unsafe extern "C" fn(bits: u64),
    /// Decrement the Molt reference count; deallocate if it reaches zero.
    pub dec_ref: unsafe extern "C" fn(bits: u64),
    /// Return the current runtime strong-reference count for a heap object.
    pub ref_count: unsafe extern "C" fn(bits: u64) -> usize,
    /// Mark or clear the runtime header's canonical ABI-view membership bit.
    /// Runtime refcount and GC hot paths use this as the lock-free negative
    /// test before consulting bridge state.
    /// Publish or retire the canonical ABI-view fact. Publication fails when
    /// terminal deallocation has begun; retirement always succeeds.
    pub try_mark_abi_view:
        unsafe extern "C" fn(bits: u64, present: std::os::raw::c_int) -> std::os::raw::c_int,
    // ── Module / C-extension support ─────────────────────────────────────────
    /// Allocate a new Molt module object whose `__name__` is the UTF-8 string
    /// in `name_data[..name_len]`.  Returns module handle bits, 0 on failure.
    pub alloc_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Return the runtime-owned module dict handle as a borrowed result.
    pub module_get_dict_borrowed: unsafe extern "C" fn(module_bits: u64) -> BorrowedHandleResult,
    /// Atomically get or create `sys.modules[name]`, replacing an existing
    /// non-module value with a fresh empty module. Returns borrowed.
    pub import_add_module_borrowed:
        unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> BorrowedHandleResult,
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
    /// Find the borrowed module handle registered for a module definition
    /// pointer. The registry retains its own strong reference.
    pub module_state_find: unsafe extern "C" fn(module_def_ptr: usize) -> BorrowedHandleResult,
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
    pub number_binary_op:
        unsafe extern "C" fn(op: u32, a_bits: u64, b_bits: u64) -> OwnedHandleResult,
    /// Unary numeric op. `op` is a [`NumberUnaryOp`] discriminant. Returns
    /// result bits, or 0 with a pending exception on error.
    pub number_unary_op: unsafe extern "C" fn(op: u32, a_bits: u64) -> OwnedHandleResult,
    /// Ternary power `pow(base, exp, modulus)`. When `mod_bits` is `0` or None,
    /// computes two-argument `base ** exp`. Returns result bits, or 0 with a
    /// pending exception on error.
    pub number_power:
        unsafe extern "C" fn(a_bits: u64, b_bits: u64, mod_bits: u64) -> OwnedHandleResult,
    // ── Mapping protocol (PyDict_*) ───────────────────────────────────────────
    //
    // The runtime owns dict iteration (copy / keys / values / items). The ABI
    // MUST NOT return an empty dict/list ignoring its argument — that is silent
    // data loss. This hook routes to the runtime dict authority. `op` is a
    // [`DictOp`] discriminant. Returns result bits, or 0 with a pending exception
    // on error.
    pub dict_op: unsafe extern "C" fn(op: u32, dict_bits: u64) -> u64,
    pub set_op: unsafe extern "C" fn(op: u32, set_bits: u64) -> OwnedHandleResult,
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
    pub object_call: unsafe extern "C" fn(
        callable_bits: u64,
        args_bits: u64,
        kwargs_bits: u64,
    ) -> OwnedHandleResult,
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
    /// Append-only L7 tail: construct one arbitrary-width integer from
    /// validated numeric digits in one owned allocation.
    pub int_from_digits: unsafe extern "C" fn(
        digits: *const u8,
        len: usize,
        base: u32,
        negative: std::os::raw::c_int,
    ) -> u64,
    pub int_from_f64_trunc: unsafe extern "C" fn(value: f64) -> u64,
    pub int_sign: unsafe extern "C" fn(bits: u64) -> std::os::raw::c_int,
    pub complex_parts:
        unsafe extern "C" fn(bits: u64, real: *mut f64, imag: *mut f64) -> std::os::raw::c_int,
    pub complex_from_doubles: unsafe extern "C" fn(real: f64, imag: f64) -> OwnedHandleResult,
    pub int_signed_byte_width:
        unsafe extern "C" fn(bits: u64, out: *mut usize) -> std::os::raw::c_int,
    /// Report an already-captured C-API exception through the runtime's sole
    /// unraisable transaction. `message` is UTF-8 and borrowed for this call.
    pub report_unraisable: unsafe extern "C" fn(
        context_bits: u64,
        type_bits: u64,
        value_bits: u64,
        traceback_bits: u64,
        message: *const u8,
        message_len: usize,
        err_msg: *const u8,
        err_msg_len: usize,
        has_err_msg: std::os::raw::c_int,
    ),
    /// Normalize a C-API exception through the runtime's canonical class-call
    /// authority. All handles are borrowed. `args_bits` is the already-shaped
    /// positional tuple; `value_bits` lets the runtime preserve an already
    /// normalized matching exception instance by identity. On
    /// success the result owns one exception-instance handle and
    /// `actual_class_bits` receives a borrowed handle for the instance's exact
    /// class (including managed user subclasses and OSError subtype selection),
    /// valid while the owned instance result remains alive.
    pub normalize_exception: unsafe extern "C" fn(
        requested_class_bits: u64,
        args_bits: u64,
        value_bits: u64,
        has_value: std::os::raw::c_int,
        traceback_bits: u64,
        has_traceback: std::os::raw::c_int,
        actual_class_bits: *mut u64,
    ) -> OwnedHandleResult,
    /// Replace one managed runtime exception field. Both handles are borrowed;
    /// `has_value == 0` means C NULL (clear cause/context/traceback). Returns
    /// zero on success and -1 when the exception or field value violates the
    /// field contract. Cause/context C wrappers separately honor their stolen
    /// input-reference contract.
    pub exception_set_field: unsafe extern "C" fn(
        exception_bits: u64,
        field: u32,
        value_bits: u64,
        has_value: std::os::raw::c_int,
    ) -> std::os::raw::c_int,
    /// Return one managed exception field as an owned handle. `Missing` means
    /// a cleared cause/context/traceback; args always returns its tuple.
    pub exception_get_field:
        unsafe extern "C" fn(exception_bits: u64, field: u32) -> OwnedHandleResult,
    /// Borrow the exact runtime class handle of a managed exception instance.
    /// Used when materializing its C view so `Py_TYPE(value)` is the same class
    /// returned by Fetch/Occurred, never the neutral managed-view type.
    pub exception_class_borrowed: unsafe extern "C" fn(exception_bits: u64) -> BorrowedHandleResult,
    /// Capture and pin the complete runtime exception field state in one GIL
    /// transaction.  On success every present field owns one handle reference.
    pub exception_snapshot: unsafe extern "C" fn(
        exception_bits: u64,
        out: *mut ExceptionSnapshot,
    ) -> std::os::raw::c_int,
    /// Validate then publish the complete C sidecar state in one GIL
    /// transaction.  No runtime field changes if validation fails.
    pub exception_commit_snapshot: unsafe extern "C" fn(
        exception_bits: u64,
        snapshot: *const ExceptionSnapshot,
    ) -> std::os::raw::c_int,
    /// Runtime MRO authority for managed type handles, including multiple
    /// inheritance that cannot be represented by a single C `tp_base` edge.
    pub type_is_subtype:
        unsafe extern "C" fn(subclass_bits: u64, class_bits: u64) -> std::os::raw::c_int,
    /// Detach the exact runtime-pending exception into the C indicator domain.
    /// The result owns the exception instance; `actual_class_bits` and
    /// `traceback_bits` are borrowed from that instance and remain valid while
    /// the owned result is live. The separate traceback out-param uses zero as
    /// its intentional no-traceback sentinel.
    pub take_pending_exception: unsafe extern "C" fn(
        actual_class_bits: *mut u64,
        traceback_bits: *mut u64,
    ) -> OwnedHandleResult,
    /// Return the runtime's active handled exception (`sys.exception()`) as an
    /// owned handle. `Missing` means no exception is being handled.
    pub handled_exception_get: unsafe extern "C" fn() -> OwnedHandleResult,
    /// Replace the runtime's active handled exception. Zero clears it; a
    /// non-zero handle is owned by and always consumed by this call.
    pub handled_exception_set:
        unsafe extern "C" fn(owned_exception_bits: u64) -> std::os::raw::c_int,
}

pub const RUNTIME_HOOKS_ABI_MAGIC: u64 = 0x4d4f_4c54_484f_4f4b;
pub const RUNTIME_HOOKS_ABI_VERSION: u32 = 16;

/// Discriminants for [`RuntimeHooks::dict_op`]. Kept in sync with the match in
/// the runtime hook implementation (`hook_dict_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DictOp {
    Copy = 0,
    Keys = 1,
    Values = 2,
    Items = 3,
    Clear = 4,
}

#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SetOp {
    FrozenNew = 0,
    Pop = 1,
    Clear = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ExceptionField {
    Cause = 0,
    Context = 1,
    Traceback = 2,
    Args = 3,
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

/// Register the exact runtime hook vtable without panicking on host input.
///
/// Returns `true` if this call installed the hooks, or `false` if a prior
/// registration was already in effect or the table is incompatible. The
/// passed-in table is dropped in either failure case.
///
/// # Safety
/// Every function pointer in `hooks` must remain valid for the lifetime of the
/// process.
pub unsafe fn try_set_runtime_hooks(hooks: RuntimeHooks) -> bool {
    if hooks.abi_magic != RUNTIME_HOOKS_ABI_MAGIC
        || hooks.abi_version != RUNTIME_HOOKS_ABI_VERSION
        || hooks.struct_size as usize != std::mem::size_of::<RuntimeHooks>()
    {
        return false;
    }
    RUNTIME_HOOKS.set(hooks).is_ok()
}

/// C-callable registration entry point for `molt-lang-runtime`.
///
/// # Safety
/// Every function pointer in the table must remain valid for the lifetime of
/// the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_register_hooks(hooks: *const RuntimeHooks) -> i32 {
    if hooks.is_null() {
        return -1;
    }
    // Registration accepts exactly the current table.  There is no shorter
    // append-only/legacy ABI lane: producer and consumer are rebuilt together.
    let hooks = unsafe { std::ptr::read_unaligned(hooks) };
    if hooks.abi_magic != RUNTIME_HOOKS_ABI_MAGIC
        || hooks.abi_version != RUNTIME_HOOKS_ABI_VERSION
        || hooks.struct_size as usize != std::mem::size_of::<RuntimeHooks>()
    {
        return -1;
    }
    if unsafe { try_set_runtime_hooks(hooks) } {
        0
    } else {
        -1
    }
}

/// Access the runtime hooks. Returns `None` if hooks have not been registered
/// (pre-init or test contexts). Callers must degrade gracefully (return None/0).
#[inline]
pub fn hooks() -> Option<&'static RuntimeHooks> {
    RUNTIME_HOOKS.get()
}

/// Whether the registered runtime owns managed tuple construction. Before
/// runtime initialization (and in intentionally partial ABI fixtures), exact
/// C tuples use their native `PyTupleObject` allocation authority instead.
#[inline]
pub(crate) fn managed_tuple_construction_available() -> bool {
    hooks().is_some_and(|runtime| {
        !std::ptr::fn_addr_eq(
            runtime.alloc_tuple,
            stub_alloc_tuple as unsafe extern "C" fn(usize) -> u64,
        ) && !std::ptr::fn_addr_eq(
            runtime.tuple_set,
            stub_tuple_set
                as unsafe extern "C" fn(
                    u64,
                    usize,
                    u64,
                    *mut crate::abi_types::PyObject,
                ) -> OwnedHandleResult,
        ) && !std::ptr::fn_addr_eq(
            runtime.tuple_len,
            stub_tuple_len as unsafe extern "C" fn(u64) -> usize,
        ) && !std::ptr::fn_addr_eq(
            runtime.tuple_item,
            stub_tuple_item as unsafe extern "C" fn(u64, usize) -> BorrowedHandleResult,
        ) && !std::ptr::fn_addr_eq(
            runtime.ref_count,
            stub_ref_count as unsafe extern "C" fn(u64) -> usize,
        ) && !std::ptr::fn_addr_eq(
            runtime.classify_heap,
            stub_classify_heap as unsafe extern "C" fn(u64) -> u8,
        )
    })
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
unsafe extern "C" fn stub_int_as_u64_mask(
    _bits: u64,
    _width: u32,
    _out: *mut u64,
) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_int_from_bytes(
    _data: *const u8,
    _len: usize,
    _little_endian: std::os::raw::c_int,
    _signed: std::os::raw::c_int,
) -> u64 {
    0
}

unsafe extern "C" fn stub_int_from_digits(
    _digits: *const u8,
    _len: usize,
    _base: u32,
    _negative: std::os::raw::c_int,
) -> u64 {
    0
}

unsafe extern "C" fn stub_int_from_f64_trunc(_value: f64) -> u64 {
    0
}

unsafe extern "C" fn stub_int_sign(_bits: u64) -> std::os::raw::c_int {
    0
}

unsafe extern "C" fn stub_int_signed_byte_width(
    _bits: u64,
    _out: *mut usize,
) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_int_to_bytes(
    _bits: u64,
    _data: *mut u8,
    _len: usize,
    _little_endian: std::os::raw::c_int,
    _signed: std::os::raw::c_int,
) -> std::os::raw::c_int {
    INT_BYTES_INVALID
}

unsafe extern "C" fn stub_int_num_bits(_bits: u64, _out: *mut usize) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_int_max_str_digits() -> usize {
    4300
}

unsafe extern "C" fn stub_complex_parts(_bits: u64, _real: *mut f64, _imag: *mut f64) -> i32 {
    -1
}
unsafe extern "C" fn stub_complex_from_doubles(_real: f64, _imag: f64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_alloc_list() -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_list_presized(_len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_list_append(
    _list_bits: u64,
    _item_bits: u64,
    _item_ptr: *mut crate::abi_types::PyObject,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_list_item(_bits: u64, _i: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_list_set(
    _list_bits: u64,
    _i: usize,
    _val_bits: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_list_insert(
    _list_bits: u64,
    _where_: isize,
    _item_bits: u64,
    _item_ptr: *mut crate::abi_types::PyObject,
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
    _future_pointers: *const *mut crate::abi_types::PyObject,
    _future_len: usize,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_alloc_tuple(_n: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_tuple_set(
    _bits: u64,
    _i: usize,
    _val: u64,
    _exact_pointer: *mut crate::abi_types::PyObject,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_tuple_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_tuple_item(_bits: u64, _i: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_alloc_dict() -> u64 {
    0
}
unsafe extern "C" fn stub_dict_set(_d: u64, _k: u64, _v: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_dict_get(_d: u64, _k: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
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
unsafe extern "C" fn stub_object_get_attr(_obj: u64, _name: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_set_attr(
    _obj: u64,
    _name: u64,
    _value: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_format(_obj: u64, _spec: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_float_repr(_value: f64, _out: *mut u8, _cap: usize) -> usize {
    0
}
unsafe extern "C" fn stub_sys_get_object_borrowed(
    _data: *const u8,
    _len: usize,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_eval_get_builtins_borrowed() -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_classify_heap(_bits: u64) -> u8 {
    crate::abi_types::MoltTypeTag::Other as u8
}
unsafe extern "C" fn stub_object_hash(_bits: u64) -> i64 {
    -1
}
unsafe extern "C" fn stub_inc_ref(_bits: u64) {}
unsafe extern "C" fn stub_dec_ref(_bits: u64) {}
unsafe extern "C" fn stub_ref_count(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_alloc_module(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_module_get_dict_borrowed(_module_bits: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_import_add_module_borrowed(
    _data: *const u8,
    _len: usize,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
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
unsafe extern "C" fn stub_module_state_find(_module_def_ptr: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
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
unsafe extern "C" fn stub_number_binary_op(_op: u32, _a: u64, _b: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_number_unary_op(_op: u32, _a: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_number_power(_a: u64, _b: u64, _mod_bits: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_dict_op(_op: u32, _dict: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_set_op(_op: u32, _set: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
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
unsafe extern "C" fn stub_object_call(
    _callable: u64,
    _args: u64,
    _kwargs: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_foreign_new(_c_ptr: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_gil_ensure() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_gil_leave(_state: std::os::raw::c_int) {}
unsafe extern "C" fn stub_gil_release() {}
unsafe extern "C" fn stub_gil_restore() {}
unsafe extern "C" fn stub_gil_check() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_runtime_is_initialized() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_try_mark_abi_view(
    _bits: u64,
    _present: std::os::raw::c_int,
) -> std::os::raw::c_int {
    1
}
unsafe extern "C" fn stub_report_unraisable(
    _context_bits: u64,
    _type_bits: u64,
    _value_bits: u64,
    _traceback_bits: u64,
    message: *const u8,
    message_len: usize,
    _err_msg: *const u8,
    _err_msg_len: usize,
    _has_err_msg: std::os::raw::c_int,
) {
    let text = if message.is_null() {
        "<unraisable exception>".into()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, message_len) })
            .into_owned()
    };
    eprintln!("[molt-cpython-abi] unraisable exception: {text}");
}
unsafe extern "C" fn stub_normalize_exception(
    _requested_class_bits: u64,
    _args_bits: u64,
    _value_bits: u64,
    _has_value: std::os::raw::c_int,
    _traceback_bits: u64,
    _has_traceback: std::os::raw::c_int,
    _actual_class_bits: *mut u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_exception_set_field(
    _exception_bits: u64,
    _field: u32,
    _value_bits: u64,
    _has_value: std::os::raw::c_int,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_exception_get_field(
    _exception_bits: u64,
    _field: u32,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_exception_class_borrowed(_exception_bits: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_exception_snapshot(
    _exception_bits: u64,
    _out: *mut ExceptionSnapshot,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_exception_commit_snapshot(
    _exception_bits: u64,
    _snapshot: *const ExceptionSnapshot,
) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_type_is_subtype(
    _subclass_bits: u64,
    _class_bits: u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_take_pending_exception(
    _actual_class_bits: *mut u64,
    _traceback_bits: *mut u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_handled_exception_get() -> OwnedHandleResult {
    OwnedHandleResult::missing()
}
unsafe extern "C" fn stub_handled_exception_set(_owned_exception_bits: u64) -> std::os::raw::c_int {
    -1
}

/// A no-op hooks table used when the runtime hasn't registered yet.
pub const STUB_HOOKS: RuntimeHooks = RuntimeHooks {
    abi_magic: RUNTIME_HOOKS_ABI_MAGIC,
    abi_version: RUNTIME_HOOKS_ABI_VERSION,
    struct_size: std::mem::size_of::<RuntimeHooks>() as u32,
    gil_ensure: stub_gil_ensure,
    gil_leave: stub_gil_leave,
    gil_release: stub_gil_release,
    gil_restore: stub_gil_restore,
    gil_check: stub_gil_check,
    runtime_is_initialized: stub_runtime_is_initialized,
    alloc_str: stub_alloc_str,
    alloc_bytes: stub_alloc_bytes,
    int_from_i64: stub_int_from_i64,
    int_from_u64: stub_int_from_u64,
    int_as_i64: stub_int_as_i64,
    int_as_i64_checked: stub_int_as_i64_checked,
    int_as_u64_checked: stub_int_as_u64_checked,
    int_as_u64_mask: stub_int_as_u64_mask,
    int_from_digits: stub_int_from_digits,
    int_from_f64_trunc: stub_int_from_f64_trunc,
    int_sign: stub_int_sign,
    int_signed_byte_width: stub_int_signed_byte_width,
    int_from_bytes: stub_int_from_bytes,
    int_to_bytes: stub_int_to_bytes,
    int_num_bits: stub_int_num_bits,
    int_max_str_digits: stub_int_max_str_digits,
    complex_parts: stub_complex_parts,
    complex_from_doubles: stub_complex_from_doubles,
    alloc_list: stub_alloc_list,
    alloc_list_presized: stub_alloc_list_presized,
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
    eval_get_builtins_borrowed: stub_eval_get_builtins_borrowed,
    classify_heap: stub_classify_heap,
    object_hash: stub_object_hash,
    inc_ref: stub_inc_ref,
    dec_ref: stub_dec_ref,
    ref_count: stub_ref_count,
    try_mark_abi_view: stub_try_mark_abi_view,
    alloc_module: stub_alloc_module,
    module_get_dict_borrowed: stub_module_get_dict_borrowed,
    import_add_module_borrowed: stub_import_add_module_borrowed,
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
    set_op: stub_set_op,
    set_new: stub_set_new,
    set_size: stub_set_size,
    set_contains: stub_set_contains,
    set_add: stub_set_add,
    set_discard: stub_set_discard,
    object_dir: stub_object_dir,
    object_call: stub_object_call,
    foreign_new: stub_foreign_new,
    report_unraisable: stub_report_unraisable,
    normalize_exception: stub_normalize_exception,
    exception_set_field: stub_exception_set_field,
    exception_get_field: stub_exception_get_field,
    exception_class_borrowed: stub_exception_class_borrowed,
    exception_snapshot: stub_exception_snapshot,
    exception_commit_snapshot: stub_exception_commit_snapshot,
    type_is_subtype: stub_type_is_subtype,
    take_pending_exception: stub_take_pending_exception,
    handled_exception_get: stub_handled_exception_get,
    handled_exception_set: stub_handled_exception_set,
};

/// Return the registered hooks or the typed fail-closed bootstrap table.
/// Stubs provide ABI-safe sentinels, never alternate runtime semantics.
#[inline]
pub fn hooks_or_stubs() -> &'static RuntimeHooks {
    RUNTIME_HOOKS.get().unwrap_or(&STUB_HOOKS)
}

/// Pins the managed-runtime GIL for a complete ABI bridge transaction.
///
/// Acquiring once before any bridge shard lock preserves the global lock order
/// (`runtime GIL -> bridge locks`) and avoids the inversion that would result
/// from acquiring inside `try_mark_abi_view` while address/handle shards are
/// held. Calls made from an existing Molt execution frame are a zero-acquire
/// fast path: `gil_check` observes the current owner and Drop does nothing.
pub(crate) struct RuntimeGilGuard {
    state: std::os::raw::c_int,
    acquired: bool,
}

impl RuntimeGilGuard {
    #[inline]
    pub(crate) fn ensure() -> Self {
        let hooks = hooks_or_stubs();
        if unsafe { (hooks.gil_check)() } != 0 {
            Self {
                state: 0,
                acquired: false,
            }
        } else {
            Self {
                state: unsafe { (hooks.gil_ensure)() },
                acquired: true,
            }
        }
    }
}

impl Drop for RuntimeGilGuard {
    #[inline]
    fn drop(&mut self) {
        if self.acquired {
            unsafe { (hooks_or_stubs().gil_leave)(self.state) };
        }
    }
}
