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
    MoltTypeTag, Py_False, Py_None, Py_True, PyBaseObject_Type, PyBool_Type, PyBytes_Type,
    PyDict_Type, PyFloat_Type, PyList_Type, PyLong_Type, PyModule_Type, PyObject, PySet_Type,
    PyTuple_Type, PyTypeObject, PyUnicode_Type,
};
use molt_lang_obj_model::MoltObject;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::cell::UnsafeCell;
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
    /// The CPython-layout `PyObject` header. C extensions and the bridge itself
    /// hold *aliasing* `*mut PyObject` pointers into this field and mutate
    /// `ob_refcnt` through them (that is what CPython refcounting is). Interior
    /// mutability (`UnsafeCell`) is therefore mandatory: without it, every fresh
    /// `&`/`&mut` reborrow of this field would pop previously-handed-out raw
    /// pointers off the aliasing model's borrow stack, making a later access
    /// through an earlier pointer undefined behaviour (a real miscompilation
    /// hazard — LLVM may cache/reorder around the reborrow). `UnsafeCell` is
    /// `#[repr(transparent)]`, so the `#[repr(C)]` layout the C ABI and the
    /// trailing-`molt_bits` read (`read_bridge_header_bits`) depend on is
    /// unchanged.
    py_obj: UnsafeCell<PyObject>,
    molt_bits: u64,
}

struct BridgeEntry {
    /// The CPython-layout header C code sees, plus the trailing handle bits.
    header: Box<BridgeHeader>,
    /// CPython-compatible, object-owned UTF-8 cache. The final byte is always
    /// NUL and the payload length excludes it. Dropping the bridge entry drops
    /// this cache, matching the `str` object's C-visible lifetime.
    utf8: Option<Box<[u8]>>,
}

/// Global bridge — one per process (extensions are global singletons).
/// Protected by a parking_lot Mutex for minimal lock overhead.
pub static GLOBAL_BRIDGE: once_cell::sync::Lazy<Mutex<ObjectBridge>> =
    once_cell::sync::Lazy::new(|| Mutex::new(ObjectBridge::new()));

/// Global bridge state. One instance per Molt runtime context.
pub struct ObjectBridge {
    /// handle → CPython pointer
    to_py: HashMap<AbiHandle, Box<BridgeEntry>>,
    raw_py: HashMap<AbiHandle, usize>,
    /// CPython raw pointer (usize) → handle
    from_py: HashMap<usize, AbiHandle>,
    next_raw_handle: AbiHandle,
    /// Genuine C-extension object pointer (usize) → its `TYPE_ID_FOREIGN` Molt
    /// wrapper handle bits. Populated lazily by [`ObjectBridge::molt_value_for_pyobj`]
    /// when a C object crosses *into* compiled Python; the wrapper's handle bits
    /// are also entered into `raw_py` so a wrapper handed back to C round-trips
    /// (via [`ObjectBridge::handle_to_pyobj`]) to the original C pointer. One
    /// entry ⇒ one strong reference the wrapper holds on the C object, released
    /// by `molt_foreign_object_release` at wrapper drop.
    foreign: HashMap<usize, AbiHandle>,
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
        // `None` never reaches proxy allocation (the `Py_None` singleton path
        // resolves first); the entry exists so every tag has a defined type.
        push!(MoltTypeTag::None, PyBaseObject_Type);
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
        // `Other` covers every Molt heap type without a dedicated static type
        // (functions, classes, bound methods, arbitrary instances). It MUST NOT
        // masquerade as a concrete builtin: mapping it to `PyUnicode_Type` made
        // a Molt-compiled function proxy fail `PyObject_Call` with the lying
        // diagnostic "'str' object is not callable" (numpy `_multiarray_umath`
        // init calling `numpy.dtypes._add_dtype_helper`). `PyBaseObject_Type`
        // ("object") is the honest neutral: no `tp_call`, no false type checks.
        push!(MoltTypeTag::Other, PyBaseObject_Type);
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
        // SAFETY: PyBaseObject_Type is a valid static with the same lifetime as the program.
        &raw mut PyBaseObject_Type
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
        &raw mut PyBaseObject_Type
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
            return &raw mut PyBaseObject_Type;
        };

        if idx < table.len {
            table.types[idx]
        } else {
            &raw mut PyBaseObject_Type
        }
    }
}

impl ObjectBridge {
    pub fn new() -> Self {
        Self {
            to_py: HashMap::new(),
            raw_py: HashMap::new(),
            from_py: HashMap::new(),
            next_raw_handle: 0xA11C_0000_0000_0000,
            foreign: HashMap::new(),
        }
    }

    fn singleton_pyobj(bits: AbiHandle) -> Option<*mut PyObject> {
        let obj = MoltObject::from_bits(bits);
        if obj.is_none() {
            return Some(&raw mut Py_None);
        }
        if obj.is_bool() {
            return Some(if obj.as_bool().unwrap_or(false) {
                (&raw mut Py_True).cast::<PyObject>()
            } else {
                (&raw mut Py_False).cast::<PyObject>()
            });
        }
        None
    }

    unsafe fn allocate_pyobj_entry(&mut self, bits: AbiHandle, ob_refcnt: isize) -> *mut PyObject {
        let tag = self.classify_handle(bits);
        let ob_type = unsafe { tag_to_type(tag) };

        let entry = Box::new(BridgeEntry {
            header: Box::new(BridgeHeader {
                py_obj: UnsafeCell::new(PyObject { ob_refcnt, ob_type }),
                molt_bits: bits,
            }),
            utf8: None,
        });

        // Derive the shared-mutable raw pointer through `UnsafeCell::get()` so
        // every pointer the bridge hands out for this object shares one
        // interior-mutable provenance and none invalidates the others.
        let raw_ptr = entry.header.py_obj.get();
        // Molt OWNS this allocation: `Box<BridgeHeader>` forces
        // `align_of::<BridgeHeader>() == 8` (the trailing `u64` handle), so the
        // header base — and hence the trailer at `base + size_of::<PyObject>()`
        // — is 8-byte aligned. Document + enforce that contract so iteration
        // mode flags any regression where molt's own allocation drifts. (A
        // *foreign* C `PyObject` carries no such guarantee — see
        // `read_bridge_header_bits`, which reads unaligned.)
        debug_assert_aligned!(raw_ptr, BridgeHeader);
        // `from_py` is an identity map (address → handle); it is never used to
        // reconstruct a pointer, so key it on the strict address, not an exposed
        // one.
        self.from_py.insert(raw_ptr.addr(), bits);
        self.to_py.insert(bits, entry);
        raw_ptr
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
        //
        // Derive the raw pointer through `UnsafeCell::get()` from a *shared*
        // borrow. `ob_refcnt` is written below, and the header the C side aliases
        // is interior-mutable, so this write is legal AND — unlike a fresh `&mut`
        // reborrow — it does not invalidate any raw pointer the bridge handed out
        // on an earlier call for the same object. (Verified by Miri: writing
        // through a shared-ref `*mut` cast, or re-`&mut`-reborrowing, is UB.)
        if let Some(entry) = self.to_py.get(&bits) {
            let ptr = entry.header.py_obj.get();
            unsafe {
                // Never mutate an immortal's refcount (mirrors CPython Py_INCREF,
                // which no-ops on immortals). Proxies are mortal so this normally
                // increments; the guard keeps the "bridge never touches an
                // immortal refcnt" invariant total.
                if !crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) {
                    (*ptr).ob_refcnt += 1;
                }
            }
            return ptr;
        }

        if let Some(addr) = self.raw_py.get(&bits).copied() {
            // `addr` is a C-extension pointer stored as its exposed address
            // (see `register_raw_pyobj` / `insert_foreign_for_test`); rebuild the
            // pointer with the exposed-provenance API so the reconstruction is
            // provenance-correct rather than a bare `usize as *mut` int→ptr cast.
            let ptr = core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
            unsafe {
                // `raw_py` holds BOTH the registered immortal singletons (exc
                // singletons, `Py*_Type`, sentinels) and foreign-object anchors.
                // Immortals must never be incremented — otherwise a registered
                // singleton's refcount creeps past `IMMORTAL_REFCNT` and (under
                // CPython-faithful detection) silently drops to MORTAL, exposing
                // a static-free/UAF. Mortal foreign objects still get the new ref.
                if !crate::abi_types::is_immortal_refcnt((*ptr).ob_refcnt) {
                    (*ptr).ob_refcnt += 1;
                }
            }
            return ptr;
        }

        if let Some(ptr) = Self::singleton_pyobj(bits) {
            return ptr;
        }

        unsafe { self.allocate_pyobj_entry(bits, 1) }
    }

    /// Translate a Molt handle to a borrowed `*mut PyObject`.
    ///
    /// Existing bridge entries are returned without incrementing the logical
    /// `ob_refcnt`. A missing non-singleton entry is materialized with refcount
    /// 1 as a bridge cache anchor representing the Molt-side owner; callers of
    /// borrowed C-API functions must not `Py_DECREF` it unless they first
    /// `Py_INCREF`.
    ///
    /// # Safety
    /// The Molt-side owner must keep `bits` live for the borrowed result.
    pub unsafe fn handle_to_borrowed_pyobj(&mut self, bits: AbiHandle) -> *mut PyObject {
        // Return an interior-mutable pointer via `UnsafeCell::get()`: the
        // borrowed-API contract lets the caller `Py_INCREF` through it, so the
        // pointer must carry write permission (a shared-ref `*mut` cast would be
        // read-only, and writing through it UB), while still aliasing cleanly
        // with any other outstanding pointer to the same object.
        if let Some(entry) = self.to_py.get(&bits) {
            return entry.header.py_obj.get();
        }

        if let Some(addr) = self.raw_py.get(&bits).copied() {
            return core::ptr::with_exposed_provenance_mut::<PyObject>(addr);
        }

        if let Some(ptr) = Self::singleton_pyobj(bits) {
            return ptr;
        }

        unsafe { self.allocate_pyobj_entry(bits, 1) }
    }

    /// Translate a `*mut PyObject` back to a Molt handle.
    ///
    /// Returns `None` for static singletons or unknown pointers.
    pub fn pyobj_to_handle(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        pyobj_to_handle_static(ptr).or_else(|| self.from_py.get(&ptr.addr()).copied())
    }

    /// Return the object-owned, NUL-terminated UTF-8 cache for a bridge string,
    /// creating it from `bytes` on first use. The returned pointer stays valid
    /// until `release_pyobj` removes the bridge entry.
    pub fn unicode_utf8_cache(
        &mut self,
        bits: AbiHandle,
        bytes: &[u8],
    ) -> Option<(*const u8, usize)> {
        let entry = self.to_py.get_mut(&bits)?;
        let cache = entry.utf8.get_or_insert_with(|| {
            let mut nul_terminated = Vec::with_capacity(bytes.len() + 1);
            nul_terminated.extend_from_slice(bytes);
            nul_terminated.push(0);
            nul_terminated.into_boxed_slice()
        });
        Some((cache.as_ptr(), cache.len() - 1))
    }

    /// Translate a `*mut PyObject` to a *genuine Molt object handle*, excluding
    /// raw-registered C objects (whose synthetic `0xA11C…` handles are bridge
    /// identity anchors, NOT valid `MoltObject` bit patterns). Use this when the
    /// handle will be passed to a runtime hook that interprets the bits as a
    /// `MoltObject` (e.g. the `object_call` call authority); use
    /// [`Self::pyobj_to_handle`] for pure identity resolution.
    pub fn molt_handle_for_pyobj(&self, ptr: *mut PyObject) -> Option<AbiHandle> {
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            return Some(bits);
        }
        let bits = self.from_py.get(&ptr.addr()).copied()?;
        if self.raw_py.contains_key(&bits) {
            return None;
        }
        Some(bits)
    }

    /// Translate a `*mut PyObject` to a Molt value usable *inside* compiled
    /// Python — the crossing converter for C-extension objects flowing INTO
    /// Molt (call arguments, attribute/call results). Unlike
    /// [`Self::pyobj_to_handle`] (identity only) and
    /// [`Self::molt_handle_for_pyobj`] (genuine Molt handles only), this mints a
    /// first-class `TYPE_ID_FOREIGN` wrapper for a genuine C-extension object so
    /// its attribute access / calls resolve through the object's own type slots.
    ///
    /// The returned handle is an *owned* reference (the caller owns one Molt
    /// reference): a singleton (immortal), a genuine Molt handle
    /// (`inc_ref`-ed), or a foreign wrapper (fresh at refcount 1, or `inc_ref`-ed
    /// on a cache hit). Returns `None` only when the wrapper cannot be minted
    /// (runtime hooks not yet installed) — callers must then fail loud.
    ///
    /// # Safety
    /// `ptr` must be null, a static singleton, a bridge-managed pointer, or a
    /// live C-extension `PyObject*`.
    pub unsafe fn molt_value_for_pyobj(&mut self, ptr: *mut PyObject) -> Option<u64> {
        if ptr.is_null() {
            return None;
        }
        if let Some(bits) = pyobj_to_handle_static(ptr) {
            // Singletons are immortal; no reference to take.
            return Some(bits);
        }
        // A genuine Molt object that previously crossed to C is a bridge proxy;
        // resolve it to its real Molt handle and hand the caller an owned ref.
        if let Some(bits) = self.molt_handle_for_pyobj(ptr) {
            let h = crate::hooks::hooks_or_stubs();
            unsafe { (h.inc_ref)(bits) };
            return Some(bits);
        }
        // Otherwise this is a genuine C-extension object (a raw-registered
        // identity anchor, or one not yet seen): give it a first-class foreign
        // wrapper the runtime can getattr/setattr/call through.
        unsafe { self.foreign_wrapper_for(ptr) }
    }

    /// Mint (or reuse) the `TYPE_ID_FOREIGN` wrapper for a genuine C-extension
    /// object `ptr`, returning an owned reference to it. On first mint the
    /// wrapper takes ONE strong reference on the C object (`Py_INCREF`), released
    /// by `molt_foreign_object_release` when the wrapper is dropped.
    ///
    /// # Safety
    /// `ptr` must be a live C-extension `PyObject*`.
    unsafe fn foreign_wrapper_for(&mut self, ptr: *mut PyObject) -> Option<u64> {
        // `key` flows into `raw_py` and into the runtime's `foreign_new` hook,
        // both of which reconstruct it back into a `*mut PyObject` later
        // (`handle_to_pyobj`, `molt_foreign_*`), so expose the provenance now.
        let key = ptr.expose_provenance();
        let h = crate::hooks::hooks_or_stubs();
        if let Some(&w) = self.foreign.get(&key) {
            // Cache hit: hand the caller its own reference to the live wrapper.
            unsafe { (h.inc_ref)(w) };
            return Some(w);
        }
        let w = unsafe { (h.foreign_new)(key) };
        if w == 0 {
            // Runtime hooks not installed (pre-init / pure-ABI tests): cannot
            // mint a wrapper. Fail loud rather than fabricate a handle.
            return None;
        }
        // Strong-ref custody: the wrapper holds one reference on the C object for
        // its lifetime; `molt_foreign_object_release` balances it at drop.
        unsafe { crate::api::refcount::Py_INCREF(ptr) };
        self.foreign.insert(key, w);
        // Round-trip: a wrapper handed back to C resolves to the original C
        // pointer via `handle_to_pyobj`'s `raw_py` lookup.
        self.raw_py.insert(w, key);
        Some(w)
    }

    /// Release the foreign wrapper identity for the C object at `c_ptr`: drop the
    /// `foreign` / `raw_py` entries and `Py_DECREF` the strong reference the
    /// wrapper held. Called by the runtime's `TYPE_ID_FOREIGN` drop hook.
    ///
    /// # Safety
    /// `c_ptr` must be the C pointer a live-until-now foreign wrapper held.
    pub unsafe fn release_foreign(&mut self, c_ptr: usize) {
        if let Some(w) = self.foreign.remove(&c_ptr) {
            self.raw_py.remove(&w);
        }
    }

    /// Install a `TYPE_ID_FOREIGN` wrapper identity for `ptr` exactly as
    /// [`Self::foreign_wrapper_for`] would after a first crossing. Test-only:
    /// the runtime `foreign_new` hook is not linked in pure-ABI unit tests, so
    /// cross-module tests that need to exercise foreign key/value custody install
    /// the cache entry directly instead of minting one.
    #[cfg(test)]
    pub(crate) fn insert_foreign_for_test(&mut self, ptr: *mut PyObject, handle: AbiHandle) {
        // `raw_py` values are later reconstructed into `*mut PyObject`
        // (`handle_to_pyobj`), so the address must be *exposed*; the `foreign`
        // key is lookup-only, so its address needs no provenance.
        let addr = ptr.expose_provenance();
        self.foreign.insert(ptr.addr(), handle);
        self.raw_py.insert(handle, addr);
    }

    pub unsafe fn register_raw_pyobj(&mut self, ptr: *mut PyObject) -> AbiHandle {
        if ptr.is_null() {
            return 0;
        }
        if let Some(bits) = self.from_py.get(&ptr.addr()).copied() {
            return bits;
        }
        // The address stored in `raw_py` is reconstructed into a pointer by
        // `handle_to_pyobj`, so expose the pointer's provenance here. The
        // `from_py` key is identity-only and needs no exposure.
        let addr = ptr.expose_provenance();
        loop {
            let bits = self.next_raw_handle;
            self.next_raw_handle = self.next_raw_handle.wrapping_add(0x10);
            if bits != 0 && !self.to_py.contains_key(&bits) && !self.raw_py.contains_key(&bits) {
                self.raw_py.insert(bits, addr);
                self.from_py.insert(addr, bits);
                return bits;
            }
        }
    }

    /// Called by `Py_DECREF` when ref count reaches zero — release bridge entry.
    pub fn release_pyobj(&mut self, ptr: *mut PyObject) -> bool {
        if let Some(bits) = self.from_py.remove(&ptr.addr()) {
            if self.raw_py.remove(&bits).is_some() {
                false
            } else {
                self.to_py.remove(&bits);
                true
            }
        } else {
            false
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
                t if t == MoltTypeTag::Int as u8 => MoltTypeTag::Int,
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
    // The canonical CPython data-symbol singletons resolve to the SAME handles as
    // their molt-header twins above, so `_Py_NoneStruct is None` / `is True` hold
    // across the header boundary (a real-CPython-header extension resolving
    // `_Py_NoneStruct` is no longer a foreign object). Matrix L1 #3/#4.
    if std::ptr::eq(
        ptr,
        &raw const crate::api::object::_Py_NoneStruct as *const _,
    ) {
        return Some(MoltObject::none().bits());
    }
    if std::ptr::eq(
        ptr,
        (&raw const crate::api::object::_Py_TrueStruct).cast::<PyObject>(),
    ) {
        return Some(MoltObject::from_bool(true).bits());
    }
    if std::ptr::eq(
        ptr,
        (&raw const crate::api::object::_Py_FalseStruct).cast::<PyObject>(),
    ) {
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
/// # Alignment
/// The read uses [`core::ptr::read_unaligned`] deliberately. Callers reach this
/// function *only when [`ObjectBridge::pyobj_to_handle`] returned `None`* — i.e.
/// `ptr` is not in this bridge copy's map. That pointer is either (a) a
/// `BridgeHeader` minted by a *separately loaded* copy of the bridge (8-byte
/// aligned — the `u64` field forces `align_of::<BridgeHeader>() == 8`) or (b) a
/// genuine C-extension object that happens to expose the same layout. Case (b)
/// carries **no** molt-side alignment guarantee: on `wasm32` a C `PyObject` has
/// struct alignment 4 (widest member is a 4-byte `Py_ssize_t`/pointer), so a
/// statically-declared C object lands on a 4-byte-aligned address and the
/// trailer at `base + size_of::<PyObject>()` is likewise only 4-aligned. An
/// aligned `*trailer` read is therefore UB (a misaligned dereference, caught by
/// the debug alignment check under `MOLT_WITNESS_ITERATION=1`); `read_unaligned`
/// is correct and — because wasm loads carry no alignment penalty — free.
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
    unsafe { std::ptr::read_unaligned(trailer) }
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
        unsafe { crate::api::typeobj::init_descriptor_slots() };
        // Give `PyType_Type` a `tp_getattro` that answers `type.__name__` /
        // `__qualname__` from `tp_name`, so metaclasses (numpy's `_DTypeMeta`)
        // inherit it and `DType.__name__` resolves once a DType crosses into
        // Molt as a foreign wrapper.
        unsafe { crate::api::typeobj::init_type_getattro() };
        init_tag_table();
        // Register the canonical CPython-ABI sentinel data objects (exception
        // singletons + Ellipsis/NotImplemented/UTC) in the bridge so a native
        // extension that hands them back resolves them via `pyobj_to_handle`.
        crate::abi_types::register_static_abi_objects();
        // Publish the `datetime.datetime_CAPI` capsule so a C extension's
        // `PyDateTime_IMPORT` (`PyCapsule_Import("datetime.datetime_CAPI", 0)`)
        // resolves the datetime C API — numpy's `_multiarray_umath` init does
        // this and returned NULL when the capsule was absent (silent-failure).
        crate::api::datetime::register_datetime_capi();
    });
}

// ─── Foreign-object custody: dispatch back through the C type slots ────────────
//
// A runtime `TYPE_ID_FOREIGN` wrapper stores a genuine C-extension `PyObject*`.
// When compiled Python performs `getattr` / `setattr` / a call on the wrapper,
// the runtime extracts the C pointer and calls one of these functions (a direct
// cross-crate Rust call — the ABI is statically linked into the runtime binary).
// Each routes through the wrapped object's OWN type slots and converts the
// C result back into an owned Molt value via `molt_value_for_pyobj`. They must
// NOT re-enter `PyObject_GetAttr`/`PyObject_SetAttr` (whose first branch is the
// bridge hook back into the runtime), or a foreign object would recurse forever;
// they go straight to `tp_getattro` / `tp_setattro` / `PyObject_Call`.

/// Release the bridge identity + strong reference a foreign wrapper held on the
/// C object at `c_ptr`. Called from the runtime's `TYPE_ID_FOREIGN` drop hook.
///
/// # Safety
/// `c_ptr` is the C pointer a now-dropping foreign wrapper held; the matching
/// `Py_INCREF` was taken in [`ObjectBridge::foreign_wrapper_for`].
pub unsafe fn molt_foreign_object_release(c_ptr: usize) {
    if c_ptr == 0 {
        return;
    }
    // Drop the identity mapping under the bridge lock, then release the strong
    // reference OUTSIDE the lock (Py_DECREF may run a C tp_dealloc that
    // re-enters the bridge).
    unsafe { GLOBAL_BRIDGE.lock().release_foreign(c_ptr) };
    unsafe {
        crate::api::refcount::Py_DECREF(core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr))
    };
}

/// `getattr` on a foreign wrapper: route through the wrapped C object's own
/// `tp_getattro` (else CPython generic getattr). `name_bits` is a Molt string
/// handle. Returns the attribute value as an owned Molt handle, or 0 with the C
/// slot's exception left pending.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_getattr(c_ptr: usize, name_bits: u64) -> u64 {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return 0;
    }
    let name_obj = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(name_bits) };
    if name_obj.is_null() {
        return 0;
    }
    let result = unsafe { foreign_slot_getattr(obj, name_obj) };
    unsafe { crate::api::refcount::Py_DECREF(name_obj) };
    if result.is_null() {
        return 0;
    }
    let bits = unsafe { GLOBAL_BRIDGE.lock().molt_value_for_pyobj(result) };
    // `molt_value_for_pyobj` minted its own owned reference for the Molt caller;
    // release the reference the C slot returned to us.
    unsafe { crate::api::refcount::Py_DECREF(result) };
    bits.unwrap_or(0)
}

/// Return the wrapped C object's type name (`tp_name`, a static C string) for
/// honest diagnostics of a foreign wrapper. Returns NULL when unavailable.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_type_name(c_ptr: usize) -> *const std::os::raw::c_char {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return std::ptr::null();
    }
    let tp = unsafe { (*obj).ob_type };
    if tp.is_null() {
        return std::ptr::null();
    }
    unsafe { (*tp).tp_name }
}

/// Is `c_ptr` a C type object? Detected identity-free by walking its metatype
/// (`ob_type`) chain for a `type`-named metatype — robust to a static
/// extension's `&PyType_Type` retargeting to a copy of `type`.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
unsafe fn foreign_obj_is_type(c_ptr: usize) -> bool {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return false;
    }
    let mut mt = unsafe { (*obj).ob_type };
    let mut steps = 0u32;
    while !mt.is_null() && steps < 32 {
        let n = unsafe { (*mt).tp_name };
        if !n.is_null() && unsafe { std::ffi::CStr::from_ptr(n) }.to_bytes() == b"type" {
            return true;
        }
        mt = unsafe { (*mt).tp_base };
        steps += 1;
    }
    false
}

/// Resolve a *type* object's `__name__` / `__qualname__` from its own `tp_name`,
/// stripping any module/qualifier prefix (up to and including the last '.') —
/// exactly what CPython's `type.__name__` getter does. Writes the resolved name
/// (no NUL) into `out[..cap]` and returns its byte length, or -1 when `c_ptr` is
/// not a type object or has no `tp_name`.
///
/// This is deliberately **hooks-free** (no `str_data`/`alloc_str`, no bridge
/// proxy round-trip): the runtime side owns the attribute name and allocates the
/// result string itself, so `Type.__name__` resolves regardless of which
/// split-runtime module the getattr lands in. Our static `PyType_Type` carries
/// no `__name__` getset, and a static extension's metaclass (numpy's
/// `_DTypeMeta`) can reach Molt with no usable getattro, so this direct path is
/// the honest, robust answer for the numpy `numpy.dtypes._add_dtype_helper`
/// `DType.__name__` frontier.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`; `out` valid for `cap` bytes.
pub unsafe fn molt_foreign_type_dunder_name(c_ptr: usize, out: *mut u8, cap: usize) -> isize {
    if !unsafe { foreign_obj_is_type(c_ptr) } {
        return -1;
    }
    let tp = c_ptr as *mut PyTypeObject;
    let name_ptr = unsafe { (*tp).tp_name };
    if name_ptr.is_null() {
        return -1;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(name_ptr) }.to_bytes();
    let short = match bytes.iter().rposition(|&b| b == b'.') {
        Some(dot) => &bytes[dot + 1..],
        None => bytes,
    };
    if short.len() <= cap && !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(short.as_ptr(), out, short.len()) };
    }
    short.len() as isize
}

unsafe fn foreign_slot_getattr(obj: *mut PyObject, name_obj: *mut PyObject) -> *mut PyObject {
    // Walk the metatype's `tp_base` chain to find the first `tp_getattro` slot,
    // replicating CPython slot inheritance at call time. A static extension's
    // metaclass (numpy's `_DTypeMeta`, whose `tp_base` is `type`) can reach this
    // path with a null `tp_getattro` — either because inherit-slots was skipped
    // or because its `&PyType_Type` retargeted to a copy without our
    // `type_getattro` — and the chain walk still finds `PyType_Type`'s slot,
    // so `DType.__name__` resolves.
    let mut tp = unsafe { (*obj).ob_type };
    let mut steps = 0u32;
    while !tp.is_null() && steps < 32 {
        if let Some(getattro) = unsafe { (*tp).tp_getattro } {
            return unsafe { getattro(obj, name_obj) };
        }
        tp = unsafe { (*tp).tp_base };
        steps += 1;
    }
    // No tp_getattro slot anywhere in the metatype chain: fall back to CPython's
    // generic instance getattr.
    unsafe { crate::api::object::PyObject_GenericGetAttr(obj, name_obj) }
}

/// `setattr` on a foreign wrapper: route through the wrapped C object's own
/// `tp_setattro`. `value_bits == 0` means delete the attribute. Returns 0 on
/// success, -1 with the C slot's exception left pending on failure.
///
/// # Safety
/// `c_ptr` must be a live C-extension `PyObject*`.
pub unsafe fn molt_foreign_setattr(
    c_ptr: usize,
    name_bits: u64,
    value_bits: u64,
) -> std::os::raw::c_int {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return -1;
    }
    let (name_obj, value_obj) = {
        let mut bridge = GLOBAL_BRIDGE.lock();
        let name_obj = unsafe { bridge.handle_to_pyobj(name_bits) };
        let value_obj = if value_bits == 0 {
            std::ptr::null_mut()
        } else {
            unsafe { bridge.handle_to_pyobj(value_bits) }
        };
        (name_obj, value_obj)
    };
    if name_obj.is_null() {
        return -1;
    }
    let tp = unsafe { (*obj).ob_type };
    let rc = if !tp.is_null() {
        if let Some(setattro) = unsafe { (*tp).tp_setattro } {
            unsafe { setattro(obj, name_obj, value_obj) }
        } else {
            -1
        }
    } else {
        -1
    };
    unsafe { crate::api::refcount::Py_DECREF(name_obj) };
    if !value_obj.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(value_obj) };
    }
    rc
}

/// Call a foreign wrapper: route through the wrapped C object's `tp_call` via
/// `PyObject_Call`, materializing a C-layout args tuple the callee can read.
/// `args_bits` is a Molt tuple handle (0 = no positional args); `kwargs_bits` a
/// Molt dict handle (0 = none). Returns the result as an owned Molt handle, or 0
/// with the C slot's exception left pending.
///
/// # Safety
/// `c_ptr` must be a live callable C-extension `PyObject*`.
pub unsafe fn molt_foreign_call(c_ptr: usize, args_bits: u64, kwargs_bits: u64) -> u64 {
    let obj = core::ptr::with_exposed_provenance_mut::<PyObject>(c_ptr);
    if obj.is_null() {
        return 0;
    }
    let args_obj = unsafe { c_layout_tuple_from_molt(args_bits) };
    if args_obj.is_null() {
        return 0;
    }
    let kwargs_obj = if kwargs_bits == 0 {
        std::ptr::null_mut()
    } else {
        unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(kwargs_bits) }
    };
    let result = unsafe { crate::api::object::PyObject_Call(obj, args_obj, kwargs_obj) };
    unsafe { crate::api::refcount::Py_DECREF(args_obj) };
    if !kwargs_obj.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(kwargs_obj) };
    }
    if result.is_null() {
        return 0;
    }
    let bits = unsafe { GLOBAL_BRIDGE.lock().molt_value_for_pyobj(result) };
    unsafe { crate::api::refcount::Py_DECREF(result) };
    bits.unwrap_or(0)
}

/// Build a C-layout `PyTupleObject` (readable by a C callee via
/// `PyTuple_GET_ITEM`) from a Molt tuple handle, translating each element to a
/// C-visible `*mut PyObject` via `handle_to_pyobj`. `PyTuple_SetItem` steals the
/// element references. Returns NULL on allocation failure.
unsafe fn c_layout_tuple_from_molt(args_bits: u64) -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    if args_bits == 0 {
        return unsafe { crate::api::sequences::PyTuple_New(0) };
    }
    let n = unsafe { (h.tuple_len)(args_bits) };
    let tuple = unsafe { crate::api::sequences::PyTuple_New(n as crate::abi_types::Py_ssize_t) };
    if tuple.is_null() {
        return std::ptr::null_mut();
    }
    for i in 0..n {
        let item_bits = unsafe { (h.tuple_item)(args_bits, i) };
        let item_obj = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) };
        // PyTuple_SetItem steals the reference on success.
        if unsafe {
            crate::api::sequences::PyTuple_SetItem(
                tuple,
                i as crate::abi_types::Py_ssize_t,
                item_obj,
            )
        } != 0
        {
            unsafe { crate::api::refcount::Py_DECREF(tuple) };
            return std::ptr::null_mut();
        }
    }
    tuple
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
    // Expose the pointer's provenance so the address can be reconstructed into a
    // valid pointer by `bits_to_pyobject`. On 64-bit the address is the full
    // NaN-box bit pattern; on 32-bit (wasm) it zero-extends, exactly as the
    // previous `obj as u64` cast did.
    obj.expose_provenance() as u64
}

/// Convert Molt NaN-boxed u64 bits back to a `*mut PyObject`.
#[inline(always)]
pub fn bits_to_pyobject(bits: u64) -> *mut PyObject {
    // Reconstruct with the exposed-provenance API (matching `pyobject_to_bits`)
    // instead of a bare `u64 as *mut` int→ptr cast, so the round-trip carries
    // provenance under Miri's strict/exposed model.
    core::ptr::with_exposed_provenance_mut(bits as usize)
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
const PY_NONE_HASH_BITS: u64 = 0x0FCA_86420;

#[inline]
fn py_hash_from_unsigned_bits(bits: u64) -> isize {
    if std::mem::size_of::<isize>() >= 8 {
        bits as isize
    } else {
        bits as u32 as i32 as isize
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bridge_hash(obj: *mut PyObject) -> isize {
    if obj.is_null() {
        return -1;
    }
    molt_hash_from_bits(pyobject_to_bits(obj))
}

/// Compute the runtime hash of a Molt value directly from its NaN-boxed bits.
/// The single authority behind both `molt_bridge_hash` (direct-boxing callers)
/// and the C-ABI `PyObject_Hash` (which resolves a bridge handle to its bits
/// first). Never returns the raw `-1` error sentinel for a real value: an
/// integer that hashes to `-1` is remapped to `-2`, matching CPython.
pub(crate) fn molt_hash_from_bits(bits: u64) -> isize {
    let mo = MoltObject::from_bits(bits);

    let h = if mo.is_int() {
        // CPython: hash(n) == n for small ints.
        mo.as_int().unwrap_or(0) as isize
    } else if mo.is_float() {
        // CPython: hash(float) follows a specific protocol.
        // For the common case, use the bit pattern.
        let f = mo.as_float().unwrap_or(0.0);
        if f == (f as i64 as f64) && f.is_finite() {
            // Integer-valued float: hash matches the int hash.
            f as i64 as isize
        } else {
            py_hash_from_unsigned_bits(bits)
        }
    } else if mo.is_bool() {
        mo.as_bool().unwrap_or(false) as isize
    } else if mo.is_none() {
        py_hash_from_unsigned_bits(PY_NONE_HASH_BITS)
    } else if mo.is_ptr() {
        // Heap objects: use the address portion of the NaN-boxed bits.
        // This gives a stable identity hash for the object's lifetime.
        py_hash_from_unsigned_bits(bits & 0x0000_FFFF_FFFF_FFFF)
    } else {
        py_hash_from_unsigned_bits(bits)
    };
    // CPython contract: a real value never hashes to the -1 error sentinel.
    if h == -1 { -2 } else { h }
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
pub(crate) fn molt_repr_string(bits: u64) -> Vec<u8> {
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
pub(crate) fn molt_str_string(bits: u64) -> Vec<u8> {
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

/// Format a float for `repr(float)` / `str(float)` through the runtime's single
/// float-format authority (`molt-lang-runtime`'s `object::float_repr`), exposed
/// over the `float_repr` runtime hook.
///
/// The ABI MUST NOT reimplement float formatting: Rust's own `{f}` produces the
/// correct number of digits but breaks round-half-to-even ties differently from
/// CPython's `_Py_dg_dtoa` (e.g. `137839762462415.625` renders as `...62` in
/// CPython but `...63` in Rust `std`). Routing through the runtime authority
/// keeps native `repr(float)` and the C-API path byte-for-byte identical.
fn format_float_repr(f: f64) -> Vec<u8> {
    let h = crate::hooks::hooks_or_stubs();
    // Max CPython float repr is "-1.7976931348623157e+308" (24 bytes); 32 is a
    // comfortable ceiling that avoids a second call in practice.
    let mut buf = [0u8; 32];
    let len = unsafe { (h.float_repr)(f, buf.as_mut_ptr(), buf.len()) };
    if len <= buf.len() {
        buf[..len].to_vec()
    } else {
        // Extremely defensive: authority reported a longer string than our
        // buffer. Re-run into an exactly-sized buffer.
        let mut big = vec![0u8; len];
        let written = unsafe { (h.float_repr)(f, big.as_mut_ptr(), big.len()) };
        big.truncate(written.min(big.len()));
        big
    }
}

#[cfg(test)]
mod bridge_handle_tests {
    use super::*;

    /// Regression for the numpy `_multiarray_umath` "'str' object is not
    /// callable" frontier: `PyObject_Call` routes bridge-managed Molt callables
    /// to the runtime `object_call` hook via `molt_handle_for_pyobj`, which must
    /// return genuine Molt handles for minted proxies and MUST NOT hand a
    /// raw-registry synthetic handle (not valid `MoltObject` bits) to the
    /// runtime.
    #[test]
    fn molt_handle_for_pyobj_excludes_raw_registered_pointers() {
        init_tag_table();
        let mut bridge = GLOBAL_BRIDGE.lock();
        // Minted proxy for a genuine Molt handle resolves through both paths.
        let int_bits = MoltObject::from_int(0x5EED).bits();
        let proxy = unsafe { bridge.handle_to_pyobj(int_bits) };
        assert_eq!(bridge.pyobj_to_handle(proxy), Some(int_bits));
        assert_eq!(bridge.molt_handle_for_pyobj(proxy), Some(int_bits));
        // Raw-registered C object: identity resolution still works, but the
        // synthetic handle is not a Molt object and must be excluded.
        let mut stray = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let stray_ptr = &raw mut stray;
        let _ = unsafe { bridge.register_raw_pyobj(stray_ptr) };
        assert!(bridge.pyobj_to_handle(stray_ptr).is_some());
        assert_eq!(bridge.molt_handle_for_pyobj(stray_ptr), None);
    }

    /// `Other`-tagged Molt objects (compiled functions, classes, arbitrary
    /// instances) must NOT masquerade as `str`: the old `PyUnicode_Type`
    /// fallback produced the lying diagnostic "'str' object is not callable"
    /// for numpy's `numpy.dtypes._add_dtype_helper` call. `object` is the
    /// honest neutral.
    #[test]
    fn other_tag_maps_to_base_object_not_str() {
        init_tag_table();
        let ty = unsafe { tag_to_type(MoltTypeTag::Other) };
        assert!(
            std::ptr::eq(ty.cast_const(), &raw const PyBaseObject_Type),
            "Other tag must map to PyBaseObject_Type"
        );
        assert!(
            !std::ptr::eq(ty.cast_const(), &raw const PyUnicode_Type),
            "Other tag must not masquerade as str"
        );
    }

    /// `molt_value_for_pyobj` resolves static singletons and genuine Molt
    /// proxies to their canonical Molt handles WITHOUT foreign-wrapping them —
    /// only genuine C-extension objects get a `TYPE_ID_FOREIGN` wrapper.
    #[test]
    fn molt_value_for_pyobj_resolves_singletons_and_proxies() {
        init_tag_table();
        let mut bridge = ObjectBridge::new();
        // Static singleton `None` → canonical NaN-boxed None, no wrapper.
        let none_ptr = &raw mut Py_None;
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(none_ptr) },
            Some(MoltObject::none().bits())
        );
        // A genuine Molt object that crossed to C (a bridge proxy) resolves back
        // to its own Molt handle, not a foreign wrapper.
        let int_bits = MoltObject::from_int(0x1234).bits();
        let proxy = unsafe { bridge.handle_to_pyobj(int_bits) };
        assert_eq!(
            unsafe { bridge.molt_value_for_pyobj(proxy) },
            Some(int_bits)
        );
        assert!(
            bridge.foreign.is_empty(),
            "no foreign wrapper for a Molt proxy"
        );
    }

    /// A foreign wrapper's identity round-trips: handed back to C it resolves to
    /// the ORIGINAL C pointer (via `raw_py`), and `release_foreign` drops both
    /// the `foreign` and `raw_py` identity entries.
    #[test]
    fn foreign_wrapper_round_trips_and_releases() {
        init_tag_table();
        let mut bridge = ObjectBridge::new();
        let mut fake = PyObject {
            ob_refcnt: 1,
            ob_type: std::ptr::null_mut(),
        };
        let c_ptr = &raw mut fake;
        // Stand in for a minted `TYPE_ID_FOREIGN` wrapper handle (the runtime
        // hook is not linked in a pure-ABI test); install the identity entries
        // exactly as `foreign_wrapper_for` would.
        let w_bits = 0xBEEF_0000_0000_0010u64;
        // Expose once (the address is reconstructed into a pointer by
        // `handle_to_pyobj` below), then use it for the identity entries exactly
        // as `foreign_wrapper_for` does.
        let addr = c_ptr.expose_provenance();
        bridge.foreign.insert(addr, w_bits);
        bridge.raw_py.insert(w_bits, addr);
        // The wrapper handed back to C resolves to the original C pointer.
        let back = unsafe { bridge.handle_to_pyobj(w_bits) };
        assert_eq!(
            back, c_ptr,
            "foreign wrapper must round-trip to its C object"
        );
        // Release drops the identity mapping so a fresh wrapper can be minted.
        unsafe { bridge.release_foreign(addr) };
        assert!(!bridge.foreign.contains_key(&addr));
        assert!(!bridge.raw_py.contains_key(&w_bits));
    }

    /// Alignment-class regression (witness frontier RUN 20260710T155049,
    /// `bridge.rs:571` "misaligned pointer dereference: address must be a
    /// multiple of 0x8"): `read_bridge_header_bits` must tolerate a
    /// **4-byte-aligned** object pointer. On `wasm32` a genuine C `PyObject` has
    /// struct alignment 4, so a statically-declared C object lands on a
    /// 4-aligned address and the trailer `u64` at `base + size_of::<PyObject>()`
    /// is only 4-aligned. Before the `read_unaligned` fix this test panicked
    /// under debug assertions (`cargo test` builds with `debug_assertions`);
    /// after it, the trailer bits round-trip.
    #[test]
    fn read_bridge_header_bits_tolerates_4byte_aligned_pointer() {
        use std::mem::size_of;

        // Over-aligned backing store; we offset into it to *force* a base that
        // is 4-byte aligned but NOT 8-byte aligned, mirroring a wasm32 C object.
        #[repr(align(8))]
        struct Backing([u8; 64]);
        let mut backing = Backing([0u8; 64]);

        let trailer_off = size_of::<PyObject>();
        assert!(trailer_off + size_of::<u64>() + 4 <= backing.0.len());

        // `+ 4` yields `addr % 8 == 4`: 4-aligned, deliberately not 8-aligned.
        let base = unsafe { backing.0.as_mut_ptr().add(4) };
        assert_eq!(
            base as usize % 8,
            4,
            "test precondition: base must be 4-aligned-not-8"
        );

        // Simulate the bridge layout: a `PyObject` header we never dereference,
        // followed by the Molt handle bits in the trailer slot.
        let expected: u64 = 0x0123_4567_89AB_CDEF;
        unsafe {
            std::ptr::write_unaligned(base.add(trailer_off).cast::<u64>(), expected);
        }

        // The pointer is neither null nor a static singleton, so this exercises
        // exactly the trailer read that panicked at the witness frontier.
        let got = unsafe { read_bridge_header_bits(base.cast::<PyObject>()) };
        assert_eq!(got, expected, "trailer bits must survive an unaligned read");
    }
}
