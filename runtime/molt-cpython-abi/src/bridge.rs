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
    MoltTypeTag, PyBool_Type, PyBytes_Type, PyDict_Type, PyFloat_Type, PyList_Type,
    PyLong_Type, PyModule_Type, PyObject, PySet_Type, PyTuple_Type, PyTypeObject,
    PyUnicode_Type, Py_False, Py_None, Py_True,
};
use molt_lang_obj_model::MoltObject;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::collections::HashMap;

/// A MoltHandle cast to u64, used as bridge map key.
pub type AbiHandle = u64;

/// Mapping from MoltHandle bits → allocated PyObject header.
/// Entries live until the extension signals dealloc via Py_DECREF → 0.
struct BridgeEntry {
    /// The CPython-layout header C code sees.
    py_obj: Box<PyObject>,
    /// Original Molt handle — used to recover the value on the return path.
    molt_bits: AbiHandle,
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
    tags:  [u8;             16],
    types: [*mut PyTypeObject; 16],
    len:   usize,
}

unsafe impl Send for TypeTagTable {}
unsafe impl Sync for TypeTagTable {}

static TAG_TABLE: OnceCell<TypeTagTable> = OnceCell::new();

/// Build the tag table once at init time.
pub fn init_tag_table() {
    TAG_TABLE.get_or_init(|| unsafe {
        let mut table = TypeTagTable {
            tags:  [0u8; 16],
            types: [std::ptr::null_mut(); 16],
            len:   0,
        };
        macro_rules! push {
            ($tag:expr, $ty:expr) => {{
                let i = table.len;
                table.tags[i]  = $tag as u8;
                table.types[i] = &raw mut $ty;
                table.len += 1;
            }};
        }
        push!(MoltTypeTag::None,   PyUnicode_Type); // NoneType → placeholder
        push!(MoltTypeTag::Bool,   PyBool_Type);
        push!(MoltTypeTag::Int,    PyLong_Type);
        push!(MoltTypeTag::Float,  PyFloat_Type);
        push!(MoltTypeTag::Str,    PyUnicode_Type);
        push!(MoltTypeTag::Bytes,  PyBytes_Type);
        push!(MoltTypeTag::List,   PyList_Type);
        push!(MoltTypeTag::Tuple,  PyTuple_Type);
        push!(MoltTypeTag::Dict,   PyDict_Type);
        push!(MoltTypeTag::Set,    PySet_Type);
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
        unsafe { &raw mut PyUnicode_Type }
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

        let tags_vec   = unsafe { _mm_loadu_si128(table.tags.as_ptr().cast()) };
        let needle_vec = unsafe { _mm_set1_epi8(needle as i8) };
        let cmp        = unsafe { _mm_cmpeq_epi8(tags_vec, needle_vec) };
        let mask       = unsafe { _mm_movemask_epi8(cmp) } as u32;

        if mask != 0 {
            let idx = mask.trailing_zeros() as usize;
            if idx < table.len {
                return table.types[idx];
            }
        }
        unsafe { &raw mut PyUnicode_Type }
    }
}

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
mod simd_neon {
    use super::*;
    use std::arch::aarch64::*;

    /// NEON path: vceqq_u8 + first-set-bit extraction.
    pub unsafe fn lookup_type(needle: u8) -> *mut PyTypeObject {
        let table = TAG_TABLE.get().expect("init_tag_table not called");

        let tags_vec   = unsafe { vld1q_u8(table.tags.as_ptr()) };
        let needle_vec = unsafe { vdupq_n_u8(needle) };
        let cmp        = unsafe { vceqq_u8(tags_vec, needle_vec) };

        // Extract match positions via u64 lanes.
        let lo = unsafe { vgetq_lane_u64(vreinterpretq_u64_u8(cmp), 0) };
        let hi = unsafe { vgetq_lane_u64(vreinterpretq_u64_u8(cmp), 1) };

        let idx = if lo != 0 {
            lo.trailing_zeros() as usize / 8
        } else if hi != 0 {
            8 + hi.trailing_zeros() as usize / 8
        } else {
            return unsafe { &raw mut PyUnicode_Type };
        };

        if idx < table.len {
            table.types[idx]
        } else {
            unsafe { &raw mut PyUnicode_Type }
        }
    }
}

impl ObjectBridge {
    pub fn new() -> Self {
        Self {
            to_py:   HashMap::new(),
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
            let ptr = entry.py_obj.as_ref() as *const PyObject as *mut PyObject;
            unsafe { (*ptr).ob_refcnt += 1; }
            return ptr;
        }

        let obj = MoltObject::from_bits(bits);

        // Singletons: None, True, False — return static pointers, no allocation.
        if obj.is_none() {
            return unsafe { &raw mut Py_None };
        }
        if obj.is_bool() {
            return if obj.as_bool().unwrap_or(false) {
                unsafe { &raw mut Py_True }
            } else {
                unsafe { &raw mut Py_False }
            };
        }

        let tag = self.classify_handle(bits);
        let ob_type = unsafe { tag_to_type(tag) };

        let mut entry = Box::new(BridgeEntry {
            py_obj: Box::new(PyObject { ob_refcnt: 1, ob_type }),
            molt_bits: bits,
        });

        let raw_ptr = entry.py_obj.as_mut() as *mut PyObject;
        self.from_py.insert(raw_ptr as usize, bits);
        self.to_py.insert(bits, entry);
        raw_ptr
    }

    /// Translate a `*mut PyObject` back to a Molt handle.
    ///
    /// Returns `None` for static singletons or unknown pointers.
    pub fn pyobj_to_handle(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        if ptr.is_null() {
            return None;
        }
        // Singletons — compare against static addresses.
        unsafe {
            if std::ptr::eq(ptr, &raw const Py_None as *const _ as *const PyObject) {
                return Some(MoltObject::none().bits());
            }
            if std::ptr::eq(ptr, &raw const Py_True as *const _ as *const PyObject) {
                return Some(MoltObject::from_bool(true).bits());
            }
            if std::ptr::eq(ptr, &raw const Py_False as *const _ as *const PyObject) {
                return Some(MoltObject::from_bool(false).bits());
            }
        }
        self.from_py.get(&(ptr as usize)).copied()
    }

    /// Called by `Py_DECREF` when ref count reaches zero — release bridge entry.
    pub fn release_pyobj(&mut self, ptr: *mut PyObject) {
        if let Some(bits) = self.from_py.remove(&(ptr as usize)) {
            self.to_py.remove(&bits);
        }
    }

    fn classify_handle(&self, bits: AbiHandle) -> MoltTypeTag {
        let obj = MoltObject::from_bits(bits);
        if obj.is_none()  { return MoltTypeTag::None; }
        if obj.is_bool()  { return MoltTypeTag::Bool; }
        if obj.is_int()   { return MoltTypeTag::Int;  }
        if obj.is_float() { return MoltTypeTag::Float; }

        if obj.is_ptr() {
            // Heap type: ask the runtime via the registered classify hook.
            let h = crate::hooks::hooks_or_stubs();
            let tag_u8 = unsafe { (h.classify_heap)(bits) };
            match tag_u8 {
                t if t == MoltTypeTag::Str    as u8 => MoltTypeTag::Str,
                t if t == MoltTypeTag::Bytes  as u8 => MoltTypeTag::Bytes,
                t if t == MoltTypeTag::List   as u8 => MoltTypeTag::List,
                t if t == MoltTypeTag::Tuple  as u8 => MoltTypeTag::Tuple,
                t if t == MoltTypeTag::Dict   as u8 => MoltTypeTag::Dict,
                t if t == MoltTypeTag::Set    as u8 => MoltTypeTag::Set,
                t if t == MoltTypeTag::Module as u8 => MoltTypeTag::Module,
                _                                   => MoltTypeTag::Other,
            }
        } else {
            MoltTypeTag::Other
        }
    }
}

// ─── Exported ABI initialiser ─────────────────────────────────────────────

/// Initialize the Molt CPython ABI bridge (type-tag table + static type objects).
///
/// Exposed as a `#[no_mangle]` C symbol so callers can `dlopen`
/// `libmolt_cpython_abi.dylib`, resolve this symbol, and call it before
/// loading any C extensions.  Idempotent — safe to call multiple times.
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_init() {
    unsafe { crate::abi_types::init_static_types() };
    init_tag_table();
}
