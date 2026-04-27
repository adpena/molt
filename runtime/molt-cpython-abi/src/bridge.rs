//! Object bridge: bidirectional translation between `*mut PyObject` and `MoltHandle`.
//!
//! ## Design
//!
//! Every time Molt passes an argument to a C extension, or a C extension
//! returns a value to Molt, we need to translate:
//!
//! - `MoltHandle` → `*mut PyObject`: allocate a `PyObject` header on a bridge
//!   arena, fill `ob_type` from the static type registry, cache the mapping.
//!
//! - `*mut PyObject` → `MoltHandle`: look up the reverse mapping in the
//!   bridge's pointer table.
//!
//! ## SIMD-accelerated type-tag lookup
//!
//! When translating handles to PyObject pointers, we need to find the
//! corresponding `PyTypeObject*` for the Molt type tag embedded in the handle.
//! The tag table has at most 16 entries (see `MoltTypeTag`), fitting in one
//! SIMD register.
//!
//! - **x86_64 + SSE4.1**: `_mm_cmpeq_epi8` on a 16-byte tag→index table.
//! - **aarch64 + NEON**: `vceqq_u8` equivalent.
//! - **Scalar fallback**: linear scan of a 16-entry array.
//!
//! The SIMD paths reduce branch mispredictions on the argument dispatch loop
//! in `PyArg_ParseTuple`, which is called on every C extension function entry.

use crate::abi_types::{
    MoltTypeTag, Py_False, Py_None, Py_True, PyBool_Type, PyBytes_Type, PyDict_Type, PyFloat_Type,
    PyList_Type, PyLong_Type, PyModule_Type, PyObject, PySet_Type, PyTuple_Type, PyTypeObject,
    PyUnicode_Type,
};
use molt_lang_obj_model::MoltObject;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Once;

/// A MoltHandle cast to u64, used as bridge map key.
pub type AbiHandle = u64;

/// Mapping from MoltHandle bits → allocated PyObject header.
/// Entries live until the extension signals dealloc via Py_DECREF → 0.
///
/// Each entry allocates a `PyObject` header *followed by* the 64-bit Molt
/// handle bits in a single boxed memory block.  C extensions only see the
/// `PyObject` prefix (matching CPython's binary layout), but our bridge can
/// recover the original Molt handle by reading the trailing u64 — even from
/// a separately loaded copy of the bridge that has no entry in its in-memory
/// map.  This is the contract that makes the rlib/dylib split safe across
/// the loader's `pyobj_to_handle` boundary.
#[repr(C)]
struct BridgeHeader {
    py_obj: PyObject,
    molt_bits: u64,
}

struct BridgeEntry {
    /// The CPython-layout header C code sees, plus the trailing handle bits.
    header: Box<BridgeHeader>,
}

/// Global bridge — one per process (extensions are global singletons).
/// Protected by a parking_lot Mutex for minimal lock overhead.
pub static GLOBAL_BRIDGE: once_cell::sync::Lazy<Mutex<ObjectBridge>> =
    once_cell::sync::Lazy::new(|| Mutex::new(ObjectBridge::new()));

/// Global bridge state. One instance per Molt runtime context.
pub struct ObjectBridge {
    /// handle → CPython pointer
    to_py: HashMap<AbiHandle, Box<BridgeEntry>>,
    /// CPython raw pointer (usize) → handle
    from_py: HashMap<usize, AbiHandle>,
}

/// SIMD tag→type lookup table.
/// Index is `MoltTypeTag as u8`, value is `*mut PyTypeObject`.
/// Fits in exactly 16 entries (one SIMD lane on SSE/NEON).
struct TypeTagTable {
    tags: [u8; 16],
    types: [*mut PyTypeObject; 16],
    len: usize,
}

unsafe impl Send for TypeTagTable {}
unsafe impl Sync for TypeTagTable {}

static TAG_TABLE: OnceCell<TypeTagTable> = OnceCell::new();

/// Build the tag table once at init time.
pub fn init_tag_table() {
    TAG_TABLE.get_or_init(|| {
        let mut table = TypeTagTable {
            tags: [0u8; 16],
            types: [std::ptr::null_mut(); 16],
            len: 0,
        };
        macro_rules! push {
            ($tag:expr, $ty:expr) => {{
                let i = table.len;
                table.tags[i] = $tag as u8;
                table.types[i] = &raw mut $ty;
                table.len += 1;
            }};
        }
        push!(MoltTypeTag::None, PyUnicode_Type); // NoneType → placeholder
        push!(MoltTypeTag::Bool, PyBool_Type);
        push!(MoltTypeTag::Int, PyLong_Type);
        push!(MoltTypeTag::Float, PyFloat_Type);
        push!(MoltTypeTag::Str, PyUnicode_Type);
        push!(MoltTypeTag::Bytes, PyBytes_Type);
        push!(MoltTypeTag::List, PyList_Type);
        push!(MoltTypeTag::Tuple, PyTuple_Type);
        push!(MoltTypeTag::Dict, PyDict_Type);
        push!(MoltTypeTag::Set, PySet_Type);
        push!(MoltTypeTag::Module, PyModule_Type);
        table
    });
}

/// Resolve a Molt type tag to its static `PyTypeObject*` using the fastest
/// available SIMD instruction set.
///
/// # Safety
/// `init_tag_table()` must have been called before first use.
#[inline]
pub unsafe fn tag_to_type(tag: MoltTypeTag) -> *mut PyTypeObject {
    let needle = tag as u8;

    #[cfg(all(target_arch = "x86_64", feature = "simd"))]
    unsafe {
        return simd_x86::lookup_type(needle);
    }

    #[cfg(all(target_arch = "aarch64", feature = "simd"))]
    unsafe {
        return simd_neon::lookup_type(needle);
    }

    // Scalar fallback — 16-entry linear scan, branch predictor handles well.
    #[allow(unreachable_code)]
    {
        let table = TAG_TABLE.get().expect("init_tag_table not called");
        for i in 0..table.len {
            if table.tags[i] == needle {
                return table.types[i];
            }
        }
        // SAFETY: PyUnicode_Type is a valid static with the same lifetime as the program.
        &raw mut PyUnicode_Type
    }
}

#[cfg(all(target_arch = "x86_64", feature = "simd"))]
mod simd_x86 {
    use super::*;
    use std::arch::x86_64::*;

    /// SSE4.1 path: compare 16 tag bytes in one instruction.
    #[target_feature(enable = "sse4.1")]
    pub unsafe fn lookup_type(needle: u8) -> *mut PyTypeObject {
        let table = TAG_TABLE.get().expect("init_tag_table not called");

        let tags_vec = unsafe { _mm_loadu_si128(table.tags.as_ptr().cast()) };
        let needle_vec = unsafe { _mm_set1_epi8(needle as i8) };
        let cmp = unsafe { _mm_cmpeq_epi8(tags_vec, needle_vec) };
        let mask = unsafe { _mm_movemask_epi8(cmp) } as u32;

        if mask != 0 {
            let idx = mask.trailing_zeros() as usize;
            if idx < table.len {
                return table.types[idx];
            }
        }
        &raw mut PyUnicode_Type
    }
}

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
mod simd_neon {
    use super::*;
    use std::arch::aarch64::*;

    /// NEON path: vceqq_u8 + first-set-bit extraction.
    pub unsafe fn lookup_type(needle: u8) -> *mut PyTypeObject {
        let table = TAG_TABLE.get().expect("init_tag_table not called");

        let tags_vec = unsafe { vld1q_u8(table.tags.as_ptr()) };
        let needle_vec = unsafe { vdupq_n_u8(needle) };
        let cmp = unsafe { vceqq_u8(tags_vec, needle_vec) };

        // Extract match positions via u64 lanes.
        let lo = unsafe { vgetq_lane_u64(vreinterpretq_u64_u8(cmp), 0) };
        let hi = unsafe { vgetq_lane_u64(vreinterpretq_u64_u8(cmp), 1) };

        let idx = if lo != 0 {
            lo.trailing_zeros() as usize / 8
        } else if hi != 0 {
            8 + hi.trailing_zeros() as usize / 8
        } else {
            return &raw mut PyUnicode_Type;
        };

        if idx < table.len {
            table.types[idx]
        } else {
            &raw mut PyUnicode_Type
        }
    }
}

impl ObjectBridge {
    pub fn new() -> Self {
        Self {
            to_py: HashMap::new(),
            from_py: HashMap::new(),
        }
    }

    /// Translate a Molt handle to a `*mut PyObject` that a C extension can use.
    ///
    /// Allocates a `PyObject` header on the heap (cached on repeat calls),
    /// registers the bidirectional mapping, and returns the raw pointer.
    ///
    /// The returned pointer's `ob_refcnt` starts at 1 (new reference).
    ///
    /// # Safety
    /// The returned pointer is valid until `release_pyobj` is called.
    pub unsafe fn handle_to_pyobj(&mut self, bits: AbiHandle) -> *mut PyObject {
        // Fast path: already in map.
        if let Some(entry) = self.to_py.get(&bits) {
            let ptr = &entry.header.py_obj as *const PyObject as *mut PyObject;
            unsafe {
                (*ptr).ob_refcnt += 1;
            }
            return ptr;
        }

        let obj = MoltObject::from_bits(bits);

        // Singletons: None, True, False — return static pointers, no allocation.
        if obj.is_none() {
            return &raw mut Py_None;
        }
        if obj.is_bool() {
            return if obj.as_bool().unwrap_or(false) {
                &raw mut Py_True
            } else {
                &raw mut Py_False
            };
        }

        let tag = self.classify_handle(bits);
        let ob_type = unsafe { tag_to_type(tag) };

        let mut entry = Box::new(BridgeEntry {
            header: Box::new(BridgeHeader {
                py_obj: PyObject {
                    ob_refcnt: 1,
                    ob_type,
                },
                molt_bits: bits,
            }),
        });

        let raw_ptr = &mut entry.header.py_obj as *mut PyObject;
        self.from_py.insert(raw_ptr as usize, bits);
        self.to_py.insert(bits, entry);
        raw_ptr
    }

    /// Translate a `*mut PyObject` back to a Molt handle.
    ///
    /// Returns `None` for static singletons or unknown pointers.
    pub fn pyobj_to_handle(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        pyobj_to_handle_static(ptr).or_else(|| self.from_py.get(&(ptr as usize)).copied())
    }

    /// Called by `Py_DECREF` when ref count reaches zero — release bridge entry.
    pub fn release_pyobj(&mut self, ptr: *mut PyObject) {
        if let Some(bits) = self.from_py.remove(&(ptr as usize)) {
            self.to_py.remove(&bits);
        }
    }

    fn classify_handle(&self, bits: AbiHandle) -> MoltTypeTag {
        let obj = MoltObject::from_bits(bits);
        if obj.is_none() {
            return MoltTypeTag::None;
        }
        if obj.is_bool() {
            return MoltTypeTag::Bool;
        }
        if obj.is_int() {
            return MoltTypeTag::Int;
        }
        if obj.is_float() {
            return MoltTypeTag::Float;
        }

        if obj.is_ptr() {
            // Heap type: ask the runtime via the registered classify hook.
            let h = crate::hooks::hooks_or_stubs();
            let tag_u8 = unsafe { (h.classify_heap)(bits) };
            match tag_u8 {
                t if t == MoltTypeTag::Str as u8 => MoltTypeTag::Str,
                t if t == MoltTypeTag::Bytes as u8 => MoltTypeTag::Bytes,
                t if t == MoltTypeTag::List as u8 => MoltTypeTag::List,
                t if t == MoltTypeTag::Tuple as u8 => MoltTypeTag::Tuple,
                t if t == MoltTypeTag::Dict as u8 => MoltTypeTag::Dict,
                t if t == MoltTypeTag::Set as u8 => MoltTypeTag::Set,
                t if t == MoltTypeTag::Module as u8 => MoltTypeTag::Module,
                _ => MoltTypeTag::Other,
            }
        } else {
            MoltTypeTag::Other
        }
    }
}

impl Default for ObjectBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Stateless `*mut PyObject` → Molt handle translation for static singletons.
///
/// Recognises `Py_None` / `Py_True` / `Py_False` directly.  Returns `None`
/// for non-singleton pointers; callers fall back to either the per-bridge
/// map or the trailing-bits read in `read_bridge_header_bits`.
///
/// Pointer-equality only — no dereference — so this function is safe to
/// call with any `*mut PyObject` value (including dangling).
fn pyobj_to_handle_static(ptr: *mut PyObject) -> Option<AbiHandle> {
    if ptr.is_null() {
        return None;
    }
    if std::ptr::eq(ptr, &raw const Py_None as *const _) {
        return Some(MoltObject::none().bits());
    }
    if std::ptr::eq(ptr, &raw const Py_True as *const _) {
        return Some(MoltObject::from_bool(true).bits());
    }
    if std::ptr::eq(ptr, &raw const Py_False as *const _) {
        return Some(MoltObject::from_bool(false).bits());
    }
    None
}

/// Read the Molt handle bits encoded in a `*mut PyObject`.
///
/// Recognises bridge-static singletons (`Py_None`, `Py_True`, `Py_False`)
/// directly.  For all other pointers the function reads the trailing u64
/// stored immediately after the `PyObject` header in `BridgeHeader`, the
/// layout used by every PyObject the bridge mints.
///
/// # Safety
/// `ptr` must either be null, a `&Py_None` / `&Py_True` / `&Py_False` static,
/// or a non-null pointer minted by `ObjectBridge::handle_to_pyobj` (in any
/// copy of the bridge crate — the layout is `#[repr(C)]` and stable).
pub unsafe fn read_bridge_header_bits(ptr: *mut PyObject) -> u64 {
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    if let Some(bits) = pyobj_to_handle_static(ptr) {
        return bits;
    }
    let trailer = unsafe {
        let raw = ptr as *const u8;
        raw.add(std::mem::size_of::<PyObject>()) as *const u64
    };
    unsafe { *trailer }
}

// ─── Exported ABI initialiser ─────────────────────────────────────────────

/// Initialize the Molt CPython ABI bridge (type-tag table + static type objects).
///
/// Exposed as a `#[no_mangle]` C symbol so callers can `dlopen`
/// `libmolt_cpython_abi.dylib`, resolve this symbol, and call it before
/// loading any C extensions.  Idempotent — safe to call multiple times.
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        unsafe { crate::abi_types::init_static_types() };
        init_tag_table();
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Direct CPython C API bridge — fast path where PyObject* IS NaN-boxed u64.
//
// In Molt's NaN-boxing scheme the 64-bit `MoltObject` bit pattern can be
// round-tripped through a pointer-width integer.  On 64-bit platforms,
// `*mut PyObject` carries the same 64 bits as `MoltObject::bits()`.
//
// This gives us zero-cost conversion between the C extension world
// (PyObject*) and the Molt world (u64 bits): the pointer IS the bits.
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a `*mut PyObject` to Molt NaN-boxed u64 bits.
///
/// On 64-bit platforms the pointer IS the bit pattern — no allocation,
/// no bridge lookup, just a cast.
#[inline(always)]
pub fn pyobject_to_bits(obj: *mut PyObject) -> u64 {
    obj as u64
}

/// Convert Molt NaN-boxed u64 bits back to a `*mut PyObject`.
#[inline(always)]
pub fn bits_to_pyobject(bits: u64) -> *mut PyObject {
    bits as *mut PyObject
}

// ─── Tier 1: Reference Counting ──────────────────────────────────────────

/// `Py_IncRef(obj)` — increment Molt reference count for a NaN-boxed object.
///
/// Only heap-pointer objects (is_ptr) need ref-counting. Inline values
/// (int, float, bool, None) are value types with no allocation — skip them.
#[unsafe(no_mangle)]
pub extern "C" fn Py_IncRef(obj: *mut PyObject) {
    if obj.is_null() {
        return;
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        unsafe { (h.inc_ref)(bits) };
    }
}

/// `Py_DecRef(obj)` — decrement Molt reference count for a NaN-boxed object.
///
/// Only heap-pointer objects need ref-counting. When the Molt-side count
/// reaches zero the runtime deallocates the backing storage.
#[unsafe(no_mangle)]
pub extern "C" fn Py_DecRef(obj: *mut PyObject) {
    if obj.is_null() {
        return;
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        unsafe { (h.dec_ref)(bits) };
    }
}

// ─── Tier 1: Object Protocol — Repr / Str ────────────────────────────────

/// `PyObject_Repr(obj)` — return the string representation of a Molt object.
///
/// Dispatches by NaN-box tag to produce a Python-style repr:
///   int   → "123"
///   float → "1.5"
///   bool  → "True" / "False"
///   None  → "None"
///   str   → "'hello'"  (quoted)
///   other → "<molt object>"
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IncRef_Repr(obj: *mut PyObject) -> *mut PyObject {
    // Note: this is exposed as molt_bridge_repr; the canonical PyObject_Repr
    // in api/typeobj.rs delegates here when using the direct path.
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    let bits = pyobject_to_bits(obj);
    let repr = molt_repr_string(bits);
    let h = crate::hooks::hooks_or_stubs();
    let repr_bits = unsafe { (h.alloc_str)(repr.as_ptr(), repr.len()) };
    if repr_bits == 0 {
        return std::ptr::null_mut();
    }
    bits_to_pyobject(repr_bits)
}

/// `PyObject_Str(obj)` — return the str() of a Molt object.
///
/// For most types str() == repr(), except strings which return themselves
/// unquoted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IncRef_Str(obj: *mut PyObject) -> *mut PyObject {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);

    // Strings return themselves — no allocation needed.
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(bits) };
        if tag == MoltTypeTag::Str as u8 {
            unsafe { (h.inc_ref)(bits) };
            return obj; // already a string, return as-is
        }
    }

    let s = molt_str_string(bits);
    let h = crate::hooks::hooks_or_stubs();
    let str_bits = unsafe { (h.alloc_str)(s.as_ptr(), s.len()) };
    if str_bits == 0 {
        return std::ptr::null_mut();
    }
    bits_to_pyobject(str_bits)
}

// ─── Tier 1: Object Protocol — Attr Access ───────────────────────────────

/// `PyObject_GetAttrString(obj, name)` — get attribute by C string name.
///
/// Converts both the object and name to NaN-boxed bits, then delegates
/// to the existing bridge attribute lookup path. Returns NULL on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bridge_get_attr_string(
    obj: *mut PyObject,
    name: *const std::os::raw::c_char,
) -> *mut PyObject {
    if obj.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }
    // Allocate the name as a Molt string object.
    let name_bytes = unsafe { std::ffi::CStr::from_ptr(name).to_bytes() };
    let h = crate::hooks::hooks_or_stubs();
    let name_bits = unsafe { (h.alloc_str)(name_bytes.as_ptr(), name_bytes.len()) };
    if name_bits == 0 {
        return std::ptr::null_mut();
    }

    // Use the bridge's existing attribute resolution via the ObjectBridge
    // for objects that have PyObject headers, and fall back to NULL for
    // direct NaN-boxed objects (which don't have attribute dicts).
    let obj_bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(obj_bits);

    // Primitive types (int, float, bool, None) have no attributes.
    if !mo.is_ptr() {
        unsafe { (h.dec_ref)(name_bits) };
        return std::ptr::null_mut();
    }

    // For heap objects, delegate to the existing full-bridge path which
    // handles tp_getattro and tp_dict lookup. The full bridge has more
    // context about type slots.
    unsafe { (h.dec_ref)(name_bits) };
    // Fall through to existing PyObject_GetAttrString in api/object.rs.
    unsafe { crate::api::object::PyObject_GetAttrString(obj, name) }
}

/// `PyObject_SetAttrString(obj, name, val)` — set attribute by C string name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bridge_set_attr_string(
    obj: *mut PyObject,
    name: *const std::os::raw::c_char,
    val: *mut PyObject,
) -> std::os::raw::c_int {
    if obj.is_null() || name.is_null() {
        return -1;
    }
    // Delegate to the existing implementation which handles type slots.
    unsafe { crate::api::object::PyObject_SetAttrString(obj, name, val) }
}

// ─── Tier 1: Object Protocol — Call ──────────────────────────────────────

/// `PyObject_Call(callable, args, kwargs)` — call a Molt callable.
///
/// Delegates to the existing call protocol which checks tp_call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bridge_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe { crate::api::object::PyObject_Call(callable, args, kwargs) }
}

// ─── Tier 1: Object Protocol — Truthiness / Hash / Length ────────────────

/// `PyObject_IsTrue(obj)` — test truthiness via NaN-boxed bits.
///
/// Direct fast-path that avoids bridge lookup for inline values:
///   None, False, 0, 0.0 → 0 (falsy)
///   True, nonzero int, nonzero float → 1 (truthy)
///   Heap objects → check length (empty containers are falsy)
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_is_true(obj: *mut PyObject) -> std::os::raw::c_int {
    if obj.is_null() {
        return 0;
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);

    if mo.is_none() {
        return 0;
    }
    if mo.is_bool() {
        return mo.as_bool().unwrap_or(false) as std::os::raw::c_int;
    }
    if mo.is_int() {
        return (mo.as_int().unwrap_or(0) != 0) as std::os::raw::c_int;
    }
    if mo.is_float() {
        return (mo.as_float().unwrap_or(0.0) != 0.0) as std::os::raw::c_int;
    }
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(bits) };
        // Empty containers are falsy.
        match tag {
            t if t == MoltTypeTag::Str as u8 => {
                let mut len: usize = 0;
                unsafe { (h.str_data)(bits, &raw mut len) };
                return (len > 0) as std::os::raw::c_int;
            }
            t if t == MoltTypeTag::List as u8 => {
                let len = unsafe { (h.list_len)(bits) };
                return (len > 0) as std::os::raw::c_int;
            }
            t if t == MoltTypeTag::Tuple as u8 => {
                let len = unsafe { (h.tuple_len)(bits) };
                return (len > 0) as std::os::raw::c_int;
            }
            t if t == MoltTypeTag::Dict as u8 => {
                let len = unsafe { (h.dict_len)(bits) };
                return (len > 0) as std::os::raw::c_int;
            }
            t if t == MoltTypeTag::Bytes as u8 => {
                let mut len: usize = 0;
                unsafe { (h.bytes_data)(bits, &raw mut len) };
                return (len > 0) as std::os::raw::c_int;
            }
            _ => return 1, // non-null heap object is truthy by default
        }
    }
    1
}

/// `PyObject_Hash(obj)` — compute hash from NaN-boxed bits.
///
/// Inline values hash directly from their bit representation.
/// Heap objects use the bit pattern as a pointer-based hash (identity hash).
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_hash(obj: *mut PyObject) -> isize {
    if obj.is_null() {
        return -1;
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);

    if mo.is_int() {
        // CPython: hash(n) == n for small ints.
        return mo.as_int().unwrap_or(0) as isize;
    }
    if mo.is_float() {
        // CPython: hash(float) follows a specific protocol.
        // For the common case, use the bit pattern.
        let f = mo.as_float().unwrap_or(0.0);
        if f == (f as i64 as f64) && f.is_finite() {
            // Integer-valued float: hash matches the int hash.
            return f as i64 as isize;
        }
        return bits as isize;
    }
    if mo.is_bool() {
        return mo.as_bool().unwrap_or(false) as isize;
    }
    if mo.is_none() {
        // CPython 3.12: hash(None) == 0xFCA86420
        return 0x0FCA86420_isize;
    }
    if mo.is_ptr() {
        // Heap objects: use the address portion of the NaN-boxed bits.
        // This gives a stable identity hash for the object's lifetime.
        return (bits & 0x0000_FFFF_FFFF_FFFF) as isize;
    }
    bits as isize
}

/// `PyObject_Length(obj)` — return length of a container via NaN-boxed bits.
///
/// Dispatches to the appropriate runtime hook based on the heap type tag.
/// Returns -1 for objects that don't support len().
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_length(obj: *mut PyObject) -> isize {
    if obj.is_null() {
        return -1;
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);

    if !mo.is_ptr() {
        return -1; // inline values don't have length
    }

    let h = crate::hooks::hooks_or_stubs();
    let tag = unsafe { (h.classify_heap)(bits) };

    match tag {
        t if t == MoltTypeTag::List as u8 => unsafe { (h.list_len)(bits) as isize },
        t if t == MoltTypeTag::Tuple as u8 => unsafe { (h.tuple_len)(bits) as isize },
        t if t == MoltTypeTag::Dict as u8 => unsafe { (h.dict_len)(bits) as isize },
        t if t == MoltTypeTag::Str as u8 => {
            let mut len: usize = 0;
            unsafe { (h.str_data)(bits, &raw mut len) };
            len as isize
        }
        t if t == MoltTypeTag::Bytes as u8 => {
            let mut len: usize = 0;
            unsafe { (h.bytes_data)(bits, &raw mut len) };
            len as isize
        }
        _ => -1,
    }
}

// ─── Tier 1: List Operations ─────────────────────────────────────────────

/// `PyList_New(size)` — allocate a new empty Molt list, return as NaN-boxed ptr.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_list_new() -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    let bits = unsafe { (h.alloc_list)() };
    if bits == 0 {
        return std::ptr::null_mut();
    }
    bits_to_pyobject(bits)
}

/// `PyList_Append(list, item)` — append item to list, both as NaN-boxed ptrs.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_list_append(
    list: *mut PyObject,
    item: *mut PyObject,
) -> std::os::raw::c_int {
    if list.is_null() || item.is_null() {
        return -1;
    }
    let list_bits = pyobject_to_bits(list);
    let item_bits = pyobject_to_bits(item);
    let h = crate::hooks::hooks_or_stubs();
    unsafe { (h.list_append)(list_bits, item_bits) };
    0
}

// ─── Tier 1: Dict Operations ─────────────────────────────────────────────

/// `PyDict_New()` — allocate a new empty Molt dict, return as NaN-boxed ptr.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_dict_new() -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    let bits = unsafe { (h.alloc_dict)() };
    if bits == 0 {
        return std::ptr::null_mut();
    }
    bits_to_pyobject(bits)
}

/// `PyDict_SetItem(dict, key, val)` — insert key-value pair into dict.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_dict_set(
    dict: *mut PyObject,
    key: *mut PyObject,
    val: *mut PyObject,
) -> std::os::raw::c_int {
    if dict.is_null() || key.is_null() || val.is_null() {
        return -1;
    }
    let dict_bits = pyobject_to_bits(dict);
    let key_bits = pyobject_to_bits(key);
    let val_bits = pyobject_to_bits(val);
    let h = crate::hooks::hooks_or_stubs();
    unsafe { (h.dict_set)(dict_bits, key_bits, val_bits) };
    0
}

// ─── Tier 1: Numeric Constructors ────────────────────────────────────────

/// `PyLong_FromLong(val)` — create a NaN-boxed int from a C long.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_long_from_long(val: std::os::raw::c_long) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    let bits = MoltObject::from_int(val as i64).bits();
    bits_to_pyobject(bits)
}

/// `PyLong_AsLong(obj)` — extract a C long from a NaN-boxed int.
///
/// Returns -1 if the object is not an integer (matches CPython error convention).
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_long_as_long(obj: *mut PyObject) -> std::os::raw::c_long {
    if obj.is_null() {
        return -1;
    }
    let bits = pyobject_to_bits(obj);
    let mo = MoltObject::from_bits(bits);
    if mo.is_int() {
        mo.as_int_unchecked() as std::os::raw::c_long
    } else if mo.is_bool() {
        mo.as_bool().unwrap_or(false) as std::os::raw::c_long
    } else {
        -1
    }
}

/// `PyFloat_FromDouble(val)` — create a NaN-boxed float from a C double.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_float_from_double(val: std::os::raw::c_double) -> *mut PyObject {
    let bits = MoltObject::from_float(val).bits();
    bits_to_pyobject(bits)
}

// ─── Tier 1: String Construction ─────────────────────────────────────────

/// `PyUnicode_FromString(str)` — create a Molt string from a null-terminated
/// C string, returned as a NaN-boxed pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bridge_string_from_cstr(
    s: *const std::os::raw::c_char,
) -> *mut PyObject {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(s).to_bytes() };
    let h = crate::hooks::hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        return std::ptr::null_mut();
    }
    bits_to_pyobject(bits)
}

// ─── Tier 1: Error Handling ──────────────────────────────────────────────

/// `PyErr_SetString(type, msg)` — set the thread-local exception state.
///
/// In the direct bridge, exception type is identified by its NaN-boxed bits
/// (which encode the exception singleton pointer). The message is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bridge_err_set_string(
    exc_type: *mut PyObject,
    message: *const std::os::raw::c_char,
) {
    // Delegate to the existing error machinery which stores in thread-local.
    unsafe { crate::api::errors::PyErr_SetString(exc_type, message) };
}

/// `PyErr_Occurred()` — check if an exception is pending.
///
/// Returns non-null if an exception is set, null otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_err_occurred() -> *mut PyObject {
    unsafe { crate::api::errors::PyErr_Occurred() }
}

// ─── Internal helpers for repr/str formatting ────────────────────────────

/// Produce a Python-style repr string for a NaN-boxed value.
fn molt_repr_string(bits: u64) -> Vec<u8> {
    let mo = MoltObject::from_bits(bits);

    if mo.is_none() {
        return b"None".to_vec();
    }
    if mo.is_bool() {
        return if mo.as_bool().unwrap_or(false) {
            b"True".to_vec()
        } else {
            b"False".to_vec()
        };
    }
    if mo.is_int() {
        let i = mo.as_int_unchecked();
        return i.to_string().into_bytes();
    }
    if mo.is_float() {
        let f = mo.as_float().unwrap_or(f64::NAN);
        return format_float_repr(f);
    }
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(bits) };
        if tag == MoltTypeTag::Str as u8 {
            // Strings get quoted in repr.
            let mut len: usize = 0;
            let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
            if !ptr.is_null() && len > 0 {
                let s = unsafe { std::slice::from_raw_parts(ptr, len) };
                let mut out = Vec::with_capacity(len + 2);
                out.push(b'\'');
                out.extend_from_slice(s);
                out.push(b'\'');
                return out;
            }
            return b"''".to_vec();
        }
    }
    b"<molt object>".to_vec()
}

/// Produce a Python-style str() string for a NaN-boxed value.
fn molt_str_string(bits: u64) -> Vec<u8> {
    let mo = MoltObject::from_bits(bits);

    if mo.is_none() {
        return b"None".to_vec();
    }
    if mo.is_bool() {
        return if mo.as_bool().unwrap_or(false) {
            b"True".to_vec()
        } else {
            b"False".to_vec()
        };
    }
    if mo.is_int() {
        let i = mo.as_int_unchecked();
        return i.to_string().into_bytes();
    }
    if mo.is_float() {
        let f = mo.as_float().unwrap_or(f64::NAN);
        return format_float_repr(f);
    }
    if mo.is_ptr() {
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(bits) };
        if tag == MoltTypeTag::Str as u8 {
            // str() of a string returns the string itself (unquoted).
            let mut len: usize = 0;
            let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
            if !ptr.is_null() && len > 0 {
                return unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
            }
            return Vec::new();
        }
    }
    b"<molt object>".to_vec()
}

/// Format a float for repr/str, matching CPython's convention:
/// - Integer-valued floats show ".0" (e.g. "1.0")
/// - NaN → "nan", Inf → "inf", -Inf → "-inf"
fn format_float_repr(f: f64) -> Vec<u8> {
    if f.is_nan() {
        return b"nan".to_vec();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            b"inf".to_vec()
        } else {
            b"-inf".to_vec()
        };
    }
    // Use Rust's display formatting which matches Python for most cases.
    let s = format!("{f}");
    // Ensure there's always a decimal point (CPython convention).
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0").into_bytes()
    } else {
        s.into_bytes()
    }
}
