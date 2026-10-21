//! Reference counting — Py_INCREF / Py_DECREF and variants.
//!
//! In a pure CPython world these are hot inlined macros. In our bridge they
//! are real functions because C extensions call them via PLT. We keep them as
//! `#[inline(always)]` to give the compiler maximum optimisation latitude when
//! the bridge itself calls them.
//!
//! Canonical managed ABI views have one lifecycle authority. The view owns one
//! runtime hold; the runtime owner contributes a C-visible bias while borrowed
//! access is valid. When runtime owners drain, that bias is detached and any
//! direct CPython C references keep the same view and runtime object alive.

use crate::abi_types::PyObject;
use std::ptr;

type NativeDestructor = unsafe extern "C" fn(*mut PyObject);

/// A conservative portable segment bound. CPython 3.12 uses 50 participating
/// destructor frames; newer CPython releases use a C-stack budget. Molt's ABI
/// has no physical stack-margin authority. This bounds participating native
/// chains; it does not substitute for target-specific physical stack headroom.
const NATIVE_DEALLOCATION_SEGMENT_LIMIT: usize = 50;

/// Queue custody belongs to the existing runtime-owned PyThreadState. The
/// intrusive links belong to the existing native GC allocation records.
#[derive(Default)]
pub(crate) struct NativeDeallocationState {
    depth: usize,
    head: usize,
    draining: bool,
}

impl NativeDeallocationState {
    pub(crate) fn assert_idle(&self) {
        assert_eq!(self.depth, 0, "thread teardown crossed native deallocation");
        assert_eq!(
            self.head, 0,
            "thread teardown abandoned native deallocation"
        );
        assert!(!self.draining, "thread teardown crossed native queue drain");
    }
}

unsafe fn legacy_deallocation_participant(
    object: *mut PyObject,
    dealloc: NativeDestructor,
) -> bool {
    unsafe {
        // These are actual native slots, not names or subclass flags. A custom
        // destructor which delegates to a builtin remains responsible for its
        // own pre-3.14 trashcan participation. In particular, queueing a base
        // call after its subtype released members would replay partial teardown.
        if [
            crate::api::memory::molt_list_dealloc as NativeDestructor,
            crate::api::sequences::molt_tuple_subtype_dealloc,
            crate::api::typeobj::subtype_dealloc,
        ]
        .iter()
        .any(|slot| ptr::fn_addr_eq(dealloc, *slot))
        {
            return true;
        }
        if crate::abi_types::_PyMethodWrapper_Type
            .tp_dealloc
            .is_some_and(|slot| ptr::fn_addr_eq(dealloc, slot))
        {
            return true;
        }
        // CPython's BaseException_dealloc participates, while its extended
        // layout destructors and MemoryError's freelist destructor do not.
        // Molt implements those layouts in one slot, so retain that semantic
        // distinction instead of treating its slot address as one participant.
        ptr::fn_addr_eq(
            dealloc,
            crate::api::errors::molt_native_exception_dealloc as NativeDestructor,
        ) && crate::abi_types::exception_layout_root_for_type((*object).ob_type)
            == Some(molt_lang_obj_model::ExceptionLayoutRoot::Base)
            && crate::api::typeobj::PyType_IsSubtype(
                (*object).ob_type,
                &raw mut crate::abi_types::PyExc_MemoryError,
            ) == 0
    }
}

struct NativeDeallocationFrame;

impl Drop for NativeDeallocationFrame {
    fn drop(&mut self) {
        let drain = crate::api::object::with_native_deallocation_state(|state| {
            state.depth = state.depth.checked_sub(1).expect("native depth underflow");
            if state.depth == 0 && state.head != 0 && !state.draining {
                state.draining = true;
                true
            } else {
                false
            }
        });
        if !drain {
            return;
        }
        loop {
            let address = crate::api::object::with_native_deallocation_state(|state| state.head);
            if address == 0 {
                break;
            }
            // No thread-state or GC-registry borrow survives a destructor.
            // Resume removes the link before restoring its previous tracking.
            let next = unsafe { (crate::hooks::hooks_or_stubs().native_gc_resume)(address) };
            crate::api::object::with_native_deallocation_state(|state| state.head = next);
            let object = ptr::with_exposed_provenance_mut::<PyObject>(address);
            unsafe {
                assert_eq!(
                    (*object).ob_refcnt,
                    0,
                    "queued native reference count changed"
                );
                // Bridge release happened exactly once at the original zero
                // transition. Replay only the actual current destructor.
                dispatch_native_deallocation(object);
            }
        }
        crate::api::object::with_native_deallocation_state(|state| {
            assert_eq!(state.depth, 0);
            state.draining = false;
        });
    }
}

/// The sole native tp_dealloc dispatch for both public zero-reference entries.
/// Ordinary callbacks observe the caller's current error; queueing does not
/// create a saved-error boundary or acquire another reference to a dying object.
unsafe fn dispatch_native_deallocation(object: *mut PyObject) {
    unsafe {
        let type_ = (*object).ob_type;
        if type_.is_null() {
            return;
        }
        let Some(dealloc) = (*type_).tp_dealloc else {
            return;
        };
        let gc = (*type_).tp_flags & crate::abi_types::Py_TPFLAGS_HAVE_GC != 0;
        let modern = (crate::hooks::hooks_or_stubs().target_python_at_least)(3, 14);
        let participant = gc && (modern || legacy_deallocation_participant(object, dealloc));
        // Old local trashcans charge participating destructors. The modern
        // common entry also sees intervening non-GC native frames, but those
        // frames always execute synchronously and are never queued.
        if !participant && !modern {
            dealloc(object);
            return;
        }
        let deferred = crate::api::object::with_native_deallocation_state(|state| {
            if participant && state.depth >= NATIVE_DEALLOCATION_SEGMENT_LIMIT {
                Some(state.head)
            } else {
                state.depth = state.depth.checked_add(1).expect("native depth overflow");
                None
            }
        });
        if let Some(next) = deferred {
            // 3.12/13 enter their local mechanism already untracked. In 3.14
            // common dispatch must restore the original bit before the slot.
            if (crate::hooks::hooks_or_stubs().native_gc_defer)(object.addr(), next, modern) != 0 {
                crate::api::memory::Py_FatalError(
                    c"native deallocation queue lost allocation custody".as_ptr(),
                );
            }
            crate::api::object::with_native_deallocation_state(|state| state.head = object.addr());
            return;
        }
        let _frame = NativeDeallocationFrame;
        dealloc(object);
    }
}

/// Increment the reference count.
///
/// # Safety
/// `op` must be a non-null bridge-managed PyObject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_INCREF(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    // Acquire the execution token before identity lookup. Publication,
    // retirement, and the managed header mutation are one transaction even
    // for a future free-threaded caller.
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if let Some(bits) = unsafe { crate::bridge::GLOBAL_BRIDGE.managed_incref_pyobj(op) } {
        if unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 1) } == 0 {
            eprintln!("molt fatal: Py_INCREF attempted after managed object terminal death");
            std::process::abort();
        }
        return;
    }
    unsafe {
        let rc = (*op).ob_refcnt;
        if crate::abi_types::is_immortal_refcnt(rc) {
            return;
        }
        (*op).ob_refcnt = rc.wrapping_add(1);
    }
}

/// Decrement the reference count and its matching runtime ownership edge.
///
/// # Safety
/// `op` must be a non-null bridge-managed PyObject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_DECREF(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    unsafe {
        match crate::bridge::GLOBAL_BRIDGE.managed_decref_pyobj(op) {
            crate::bridge::ManagedDecref::Immortal
            | crate::bridge::ManagedDecref::Alive
            | crate::bridge::ManagedDecref::RetiredInline => return,
            crate::bridge::ManagedDecref::ReleaseRuntimeHold(bits) => {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
                return;
            }
            crate::bridge::ManagedDecref::NotManaged => {}
        }
        let rc = (*op).ob_refcnt;
        if crate::abi_types::is_immortal_refcnt(rc) {
            return; // immortal singleton — permanent no-op, never freed
        }
        if rc <= 0 {
            return;
        }
        let new_rc = rc - 1;
        (*op).ob_refcnt = new_rc;
        if new_rc == 0
            && crate::bridge::GLOBAL_BRIDGE
                .release_pyobj(op)
                .requires_type_dealloc()
        {
            dispatch_native_deallocation(op);
        }
    }
}

/// CPython private `_Py_Dealloc` (Objects/object.c): the object finalizer that a
/// C extension's `Py_DECREF` macro tail-calls the moment a refcount reaches
/// zero. numpy links it directly. This mirrors the zero-refcount branch of
/// [`Py_DECREF`]: transfer a molt-owned canonical view into the runtime's
/// terminal/finalization path, which retires bridge identity only after the
/// resurrection window, or invoke a foreign C type's own `tp_dealloc`.
///
/// # Safety
/// `op` must be a valid PyObject whose refcount has already reached zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_Dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let managed_bits = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op);
    unsafe {
        if let Some(bits) = managed_bits {
            if crate::bridge::GLOBAL_BRIDGE.c_ref_zero(bits)
                == crate::bridge::CRefZero::ReleaseRuntimeHold
            {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            }
        } else if crate::bridge::GLOBAL_BRIDGE
            .release_pyobj(op)
            .requires_type_dealloc()
        {
            dispatch_native_deallocation(op);
        }
    }
}

/// `Py_INCREF` that accepts null (null is silently ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_XINCREF(op: *mut PyObject) {
    if !op.is_null() {
        unsafe { Py_INCREF(op) };
    }
}

/// `Py_DECREF` that accepts null (null is silently ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_XDECREF(op: *mut PyObject) {
    if !op.is_null() {
        unsafe { Py_DECREF(op) };
    }
}

/// Clear a `*mut PyObject` pointer: Py_XDECREF + set to NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_CLEAR(op: *mut *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let tmp = *op;
        if !tmp.is_null() {
            *op = ptr::null_mut();
            Py_DECREF(tmp);
        }
    }
}

/// Compile-time policy for a reference or resource retirement action.
///
/// # Safety
/// Implementations must invoke the supplied FnOnce exactly once, synchronously,
/// without suppressing its execution or repeating it. The resource owner alone
/// defines reference custody; a policy only supplies its error-state boundary.
/// Any unwind from the action must remain on the invoking Rust stack.
pub unsafe trait ReferenceRetirement {
    fn retire(release: impl FnOnce());
}

/// Ordinary C retirement: native destructors observe the operation error.
pub enum CReferenceRelease {}

unsafe impl ReferenceRetirement for CReferenceRelease {
    #[inline]
    fn retire(release: impl FnOnce()) {
        release();
    }
}

/// Explicitly protected cleanup, including unpublished projection rollback.
pub enum ProtectedReferenceRelease {}

unsafe impl ReferenceRetirement for ProtectedReferenceRelease {
    #[inline]
    fn retire(release: impl FnOnce()) {
        crate::api::errors::with_preserved_error(release);
    }
}

/// One pointer of reference custody with a compile-time retirement policy.
/// NULL is an empty owner. The raw pointer deliberately retains its !Send/!Sync
/// behavior; no policy grants cross-thread access to a Python object.
#[repr(transparent)]
pub struct PyObjectOwner<P: ReferenceRetirement> {
    object: *mut PyObject,
    policy: std::marker::PhantomData<P>,
}

/// An ordinary semantic operand, callback result, or callable reference.
pub type OwnedPyObject = PyObjectOwner<CReferenceRelease>;
/// A reference retired inside an explicit error-suppression boundary.
pub type ProtectedPyObject = PyObjectOwner<ProtectedReferenceRelease>;

impl<P: ReferenceRetirement> PyObjectOwner<P> {
    /// Adopt an already owned reference without an additional INCREF/DECREF pair.
    ///
    /// # Safety
    /// A non-null pointer must carry one owned reference transferred to this guard.
    #[inline]
    pub unsafe fn from_owned(object: *mut PyObject) -> Self {
        Self {
            object,
            policy: std::marker::PhantomData,
        }
    }

    /// # Safety
    /// A non-null pointer must remain live until this guard retains it.
    #[inline]
    pub unsafe fn from_borrowed(object: *mut PyObject) -> Self {
        unsafe { Py_XINCREF(object) };
        unsafe { Self::from_owned(object) }
    }

    /// Retain a non-owning observation only while its allocation is live.
    /// Internal teardown pins may have a positive C count after the runtime
    /// has committed death; those pins are not permission to resurrect it.
    /// Refusal leaves the reference count and both exception channels intact.
    ///
    /// # Safety
    /// A non-null pointer must remain allocated through this call. Callers
    /// observing registry addresses must validate their allocation generation.
    #[inline]
    pub unsafe fn try_from_borrowed(object: *mut PyObject) -> Option<Self> {
        if object.is_null() {
            return None;
        }
        let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
        if unsafe { (*object).ob_refcnt } == 0 {
            return None;
        }
        if let Some(bits) = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(object)
            && unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 1) } == 0
        {
            return None;
        }
        unsafe { Py_INCREF(object) };
        Some(unsafe { Self::from_owned(object) })
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut PyObject {
        self.object
    }

    /// Transfer the owned reference without releasing it.
    #[inline]
    pub fn into_ptr(self) -> *mut PyObject {
        let object = self.object;
        std::mem::forget(self);
        object
    }
}

impl<P: ReferenceRetirement> Drop for PyObjectOwner<P> {
    #[inline]
    fn drop(&mut self) {
        if !self.object.is_null() {
            P::retire(|| unsafe { Py_DECREF(self.object) });
        }
    }
}

const _: () = {
    assert!(std::mem::size_of::<OwnedPyObject>() == std::mem::size_of::<*mut PyObject>());
    assert!(std::mem::align_of::<OwnedPyObject>() == std::mem::align_of::<*mut PyObject>());
    assert!(std::mem::size_of::<ProtectedPyObject>() == std::mem::size_of::<*mut PyObject>());
    assert!(std::mem::align_of::<ProtectedPyObject>() == std::mem::align_of::<*mut PyObject>());
};
