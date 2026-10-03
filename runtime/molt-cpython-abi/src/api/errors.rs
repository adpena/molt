//! Error/exception API — PyErr_*, PyArg_ParseTuple.
//!
//! `PyArg_ParseTuple` is the hottest function in any C extension — called on
//! every function entry to unpack positional arguments. We implement the
//! most common format codes: `i`, `l`, `d`, `f`, `s`, `z`, `s#`, `O`, `p`,
//! `n`, `L`, `K`, `b`, `B`, `H`, `I`, `k`, `y`, `y#`, `C`.

use crate::abi_types::{
    MoltTypeTag, Py_buffer, Py_complex, Py_ssize_t, PyBUF_SIMPLE, PyBUF_WRITABLE,
    PyBaseExceptionObject, PyGetSetDef, PyMemberDef, PyObject, PyTypeObject,
};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use once_cell::sync::Lazy;
use std::ffi::{CStr, CString, c_void};
use std::os::raw::{c_char, c_int, c_long, c_ulong};
use std::ptr;

mod formatting;
pub(crate) use formatting::exception_str_slot;
pub use formatting::{molt_native_exception_repr, molt_native_exception_str};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use libc as platform_errno;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use molt_runtime_platform::libc_compat as platform_errno;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NewException(
    name: *const c_char,
    base: *mut PyObject,
    dict: *mut PyObject,
) -> *mut PyObject {
    if name.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyErr_NewException: name must be module.class".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let Some(dot) = bytes.iter().rposition(|byte| *byte == b'.') else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyErr_NewException: name must be module.class".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let mut owned_dict = ptr::null_mut();
    let dict = if dict.is_null() {
        owned_dict = unsafe { crate::api::mapping::PyDict_New() };
        owned_dict
    } else {
        dict
    };
    if dict.is_null() {
        return ptr::null_mut();
    }
    let module =
        unsafe { crate::api::strings::PyUnicode_FromStringAndSize(name, dot as Py_ssize_t) };
    if module.is_null()
        || unsafe {
            crate::api::mapping::PyDict_SetItemString(dict, c"__module__".as_ptr(), module)
        } < 0
    {
        unsafe {
            crate::api::refcount::Py_XDECREF(module);
            crate::api::refcount::Py_XDECREF(owned_dict);
        }
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_DECREF(module) };
    let base = if base.is_null() {
        (&raw mut crate::abi_types::PyExc_Exception).cast::<crate::abi_types::PyObject>()
    } else {
        base
    };
    let bases = if unsafe { crate::api::sequences::PyTuple_Check(base) } != 0 {
        unsafe { crate::api::object::Py_NewRef(base) }
    } else {
        let tuple = unsafe { crate::api::sequences::PyTuple_New(1) };
        if !tuple.is_null() {
            unsafe {
                crate::api::refcount::Py_INCREF(base);
                crate::api::sequences::PyTuple_SetItem(tuple, 0, base);
            }
        }
        tuple
    };
    let class_name = unsafe {
        crate::api::strings::PyUnicode_FromStringAndSize(
            name.add(dot + 1),
            (bytes.len() - dot - 1) as Py_ssize_t,
        )
    };
    let args = unsafe { crate::api::sequences::PyTuple_New(3) };
    if bases.is_null() || class_name.is_null() || args.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(bases);
            crate::api::refcount::Py_XDECREF(class_name);
            crate::api::refcount::Py_XDECREF(args);
            crate::api::refcount::Py_XDECREF(owned_dict);
        }
        return ptr::null_mut();
    }
    unsafe {
        crate::api::sequences::PyTuple_SetItem(args, 0, class_name);
        crate::api::sequences::PyTuple_SetItem(args, 1, bases);
        crate::api::refcount::Py_INCREF(dict);
        crate::api::sequences::PyTuple_SetItem(args, 2, dict);
    }
    let result = unsafe {
        crate::api::object::PyObject_Call(
            (&raw mut crate::abi_types::PyType_Type).cast(),
            args,
            ptr::null_mut(),
        )
    };
    unsafe {
        crate::api::refcount::Py_DECREF(args);
        crate::api::refcount::Py_XDECREF(owned_dict);
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NewExceptionWithDoc(
    name: *const c_char,
    doc: *const c_char,
    base: *mut PyObject,
    dict: *mut PyObject,
) -> *mut PyObject {
    let mut owned_dict = ptr::null_mut();
    let dict = if dict.is_null() {
        owned_dict = unsafe { crate::api::mapping::PyDict_New() };
        owned_dict
    } else {
        dict
    };
    if dict.is_null() {
        return ptr::null_mut();
    }
    if !doc.is_null() {
        let doc_obj = unsafe { crate::api::strings::PyUnicode_FromString(doc) };
        if doc_obj.is_null()
            || unsafe {
                crate::api::mapping::PyDict_SetItemString(dict, c"__doc__".as_ptr(), doc_obj)
            } < 0
        {
            unsafe {
                crate::api::refcount::Py_XDECREF(doc_obj);
                crate::api::refcount::Py_XDECREF(owned_dict);
            }
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_DECREF(doc_obj) };
    }
    let result = unsafe { PyErr_NewException(name, base, dict) };
    unsafe { crate::api::refcount::Py_XDECREF(owned_dict) };
    result
}

// ─── Thread-local error state ─────────────────────────────────────────────

pub use crate::api::object::OwnedCError;

thread_local! {
    static NORMALIZING_EXCEPTION: std::cell::Cell<ExceptionNormalizationState> = const {
        std::cell::Cell::new(ExceptionNormalizationState { depth: 0, preparing_text: 0 })
    };
}

const NORMALIZATION_RECURSION_LIMIT: u32 = 32;

#[derive(Clone, Copy)]
struct ExceptionNormalizationState {
    depth: u32,
    preparing_text: u32,
}

struct ExceptionNormalizationGuard {
    depth: u32,
    preparing_text: bool,
}

impl ExceptionNormalizationGuard {
    fn enter() -> Self {
        NORMALIZING_EXCEPTION.with(|state| {
            let mut next = state.get();
            next.depth += 1;
            if next.depth > NORMALIZATION_RECURSION_LIMIT + 2 {
                unsafe {
                    crate::api::memory::Py_FatalError(
                        c"Cannot recover from recursive exception normalization".as_ptr(),
                    )
                };
            }
            state.set(next);
            Self {
                depth: next.depth,
                preparing_text: false,
            }
        })
    }

    fn text_preparation_active(&self) -> bool {
        NORMALIZING_EXCEPTION.with(|state| state.get().preparing_text != 0)
    }

    fn prepare<T>(&mut self, kind: ExceptionPreparation, prepare: impl FnOnce() -> T) -> T {
        if matches!(kind, ExceptionPreparation::Diagnostic) {
            self.preparing_text = true;
            NORMALIZING_EXCEPTION.with(|state| {
                let mut next = state.get();
                next.preparing_text += 1;
                state.set(next);
            });
        }
        let result = prepare();
        self.finish_preparation();
        result
    }

    fn finish_preparation(&mut self) {
        if std::mem::take(&mut self.preparing_text) {
            NORMALIZING_EXCEPTION.with(|state| {
                let mut next = state.get();
                next.preparing_text -= 1;
                state.set(next);
            });
        }
    }
}

impl Drop for ExceptionNormalizationGuard {
    fn drop(&mut self) {
        self.finish_preparation();
        NORMALIZING_EXCEPTION.with(|state| {
            let mut next = state.get();
            next.depth -= 1;
            state.set(next);
        });
    }
}

fn replace_current_error(state: Option<OwnedCError>) {
    // Hold the selected state outside both indicators while retiring superseded
    // owners. Finalizers may use either channel; neither can replace this state.
    clear_all_pending_errors();
    let old = crate::api::object::replace_thread_state_error(state);
    debug_assert!(old.is_none());
}

/// Transfer the exact owned C error-indicator triple.
pub fn take_current_error() -> Option<OwnedCError> {
    crate::api::object::take_thread_state_error()
}

/// Restore a previously detached, already-normalized C error triple without
/// re-running construction or projecting it through text.
pub fn restore_current_error_exact(error: OwnedCError) {
    replace_current_error(Some(error));
}

/// Non-consuming peek at the currently-pending exception's type-handle bits.
/// `Some(0)` = an exception is set whose type was NULL/unresolvable; `None` = no
/// exception pending. Used by `PyErr_ExceptionMatches` to compare the live
/// exception's type against a candidate rather than answering "is any set".
fn current_exc_type_ptr() -> Option<*mut PyObject> {
    crate::api::object::thread_state_error_type()
}

/// Query both raised channels without projecting a C view, materializing a
/// traceback, or consuming an allocation-free runtime emergency error.
pub(crate) fn raised_error_pending() -> bool {
    current_exc_type_ptr().is_some()
        || unsafe { (crate::hooks::hooks_or_stubs().exception_pending)() } != 0
}

/// Install the only error shape available before runtime hooks are registered:
/// a concrete SystemError type with no fabricated payload. Production paths
/// normalize to an exception instance through the class-call authority; this
/// fail-closed state exists only when that authority itself is unavailable.
unsafe fn install_normalization_failure() {
    let exc_type =
        (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>();
    unsafe { crate::api::refcount::Py_INCREF(exc_type) };
    replace_current_error(Some(OwnedCError {
        exc_type,
        value: ptr::null_mut(),
        traceback: ptr::null_mut(),
    }));
}

fn owned_c_error_from_runtime_projection(
    result: crate::hooks::OwnedHandleResult,
    class_bits: u64,
    traceback_bits: u64,
) -> Option<OwnedCError> {
    let hooks = crate::hooks::hooks_or_stubs();
    let release_handles = |handles: &[u64]| {
        with_preserved_error(|| {
            for &bits in handles {
                if bits != 0 {
                    unsafe { (hooks.dec_ref)(bits) };
                }
            }
        });
    };
    let crate::hooks::DecodedHandleResult::Ok(exception_bits) = result.decode() else {
        release_handles(&[class_bits, traceback_bits]);
        return None;
    };
    if exception_bits == 0 || class_bits == 0 {
        release_handles(&[exception_bits, class_bits, traceback_bits]);
        return None;
    }
    // The three runtime owners stay pinned until independent C references have
    // been acquired. A native instance's wrapper does not own its class or
    // traceback projections, unlike a managed exception's physical fields.
    let exc_type = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class_bits) };
    if exc_type.is_null() {
        release_handles(&[exception_bits, class_bits, traceback_bits]);
        return None;
    }
    unsafe { crate::api::refcount::Py_INCREF(exc_type) };
    let traceback = if traceback_bits == 0 {
        ptr::null_mut()
    } else {
        unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(traceback_bits) }
    };
    if traceback_bits != 0 && traceback.is_null() {
        release_handles(&[exception_bits, class_bits, traceback_bits]);
        unsafe { release_preserving_error(&[exc_type]) };
        return None;
    }
    unsafe { crate::api::refcount::Py_XINCREF(traceback) };
    let value = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(exception_bits) };
    release_handles(&[class_bits, traceback_bits]);
    if value.is_null() {
        unsafe { release_preserving_error(&[exc_type, traceback]) };
        return None;
    }
    Some(OwnedCError {
        exc_type,
        value,
        traceback,
    })
}

/// Move an exact runtime-pending exception into the C indicator without text
/// conversion. The hook detaches the runtime pending edge; this function takes
/// independent C references to its exact class and traceback before consuming
/// their separate owned runtime projections.
fn take_runtime_pending_error() -> Option<OwnedCError> {
    let hooks = crate::hooks::hooks_or_stubs();
    let mut class_bits = 0u64;
    let mut traceback_bits = 0u64;
    let result =
        unsafe { (hooks.take_pending_exception)(&raw mut class_bits, &raw mut traceback_bits) };
    owned_c_error_from_runtime_projection(result, class_bits, traceback_bits)
}

/// Move the exact runtime pending instance into CURRENT_EXC when the C channel
/// is clear. Returns whether the C indicator contains an error; a failed
/// projection can leave runtime-only state. Use raised_error_pending for an
/// existence query that must not project or consume either channel.
pub(crate) fn transfer_runtime_pending_to_current() -> bool {
    if current_exc_type_ptr().is_some() {
        return true;
    }
    let Some(error) = take_runtime_pending_error() else {
        return false;
    };
    replace_current_error(Some(error));
    true
}

/// Detach the selected raised error. C has canonical precedence when a runtime
/// callback has also raised; retiring that superseded edge must leave both
/// indicators empty, even if its finalizers raise again.
fn take_raised_error() -> Option<OwnedCError> {
    let error = take_current_error()
        .or_else(take_runtime_pending_error)
        .or_else(take_current_error);
    if error.is_some() {
        clear_all_pending_errors();
    }
    // A failed runtime projection can leave a new failure pending. Only a
    // successfully selected error authorizes discarding the remaining channel.
    error
}

/// Explicitly suppress both error channels for APIs such as PyDict_GetItem
/// whose CPython contract masks lookup/hash failures.
pub(crate) fn clear_all_pending_errors() {
    let hooks = crate::hooks::hooks_or_stubs();
    loop {
        // Raw detachment is essential: restoring or promoting errors here would
        // preserve a cleanup-only failure instead of clearing the indicator.
        let current = take_current_error();
        let runtime_pending = unsafe { (hooks.exception_pending)() } != 0;
        if current.is_none() && !runtime_pending {
            break;
        }
        if runtime_pending {
            unsafe { (hooks.clear_pending_exception)() };
        }
        drop(current);
    }
}

/// Release temporary owners with both exact raised-error channels detached.
/// Preservation never projects runtime state into C views or materializes a
/// traceback. Handled state remains live, and cleanup-only errors are drained
/// before either incoming channel is restored, including during Rust unwind.
pub fn with_preserved_error<T, F: FnOnce() -> T>(cleanup: F) -> T {
    struct Cleanup<F, T> {
        run: Option<F>,
        result: Option<std::thread::Result<T>>,
    }

    struct DrainPendingErrors;

    impl Drop for DrainPendingErrors {
        fn drop(&mut self) {
            clear_all_pending_errors();
        }
    }

    unsafe extern "C" fn invoke<F: FnOnce() -> T, T>(context: *mut std::ffi::c_void) {
        let cleanup = unsafe { &mut *context.cast::<Cleanup<F, T>>() };
        // Keep unwinding entirely on this side of the C callback boundary.
        cleanup.result = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || {
                let _drain = DrainPendingErrors;
                cleanup.run.take().expect("cleanup callback must run once")()
            },
        )));
    }

    let error = take_current_error();
    let mut cleanup = Cleanup::<F, T> {
        run: Some(cleanup),
        result: None,
    };
    let hooks = crate::hooks::hooks_or_stubs();
    unsafe {
        (hooks.with_preserved_pending_exception)(invoke::<F, T>, (&raw mut cleanup).cast());
    }
    // The hook has already restored the runtime channel. Raw C publication
    // must not invoke the public replacement path, which drains both channels.
    if let Some(error) = error {
        let replaced = crate::api::object::replace_thread_state_error(Some(error));
        debug_assert!(replaced.is_none());
    }
    match cleanup
        .result
        .expect("cleanup hook must call back synchronously")
    {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Release temporary C owners while preserving even an empty error indicator.
pub(crate) unsafe fn release_preserving_error(objects: &[*mut PyObject]) {
    with_preserved_error(|| unsafe {
        for &object in objects {
            crate::api::refcount::Py_XDECREF(object);
        }
    });
}

fn clear_new_runtime_pending_error(had_pending: bool) {
    if had_pending {
        return;
    }
    let hooks = crate::hooks::hooks_or_stubs();
    while unsafe { (hooks.exception_pending)() } != 0 {
        unsafe { (hooks.clear_pending_exception)() };
    }
}

/// The public APIs share construction and identity, but have distinct context
/// and traceback contracts (CPython 3.12 Python/errors.c).
#[derive(Clone, Copy)]
enum ExceptionIngress {
    SetObject,
    Restore,
    Normalize,
}

#[derive(Clone, Copy)]
enum ExceptionPreparation {
    Owned,
    Diagnostic,
}

/// Chain from the handled exception through physical/managed field APIs. The
/// held root owns the traversal; Floyd's algorithm also terminates an existing
/// cycle. No Python attribute lookup or second exception registry is involved.
unsafe fn chain_implicit_exception_context(value: *mut PyObject) -> bool {
    let context = unsafe { PyErr_GetHandledException() };
    if context.is_null() {
        return !raised_error_pending();
    }
    if context == value || context == &raw mut crate::abi_types::Py_None {
        unsafe { release_preserving_error(&[context]) };
        return true;
    }
    let mut current = unsafe { crate::api::object::Py_NewRef(context) };
    let mut slow = unsafe { crate::api::object::Py_NewRef(context) };
    let mut advance_slow = false;
    loop {
        let next = unsafe { PyException_GetContext(current) };
        if next.is_null() {
            break;
        }
        if next == value {
            unsafe {
                PyException_SetContext(current, ptr::null_mut());
                release_preserving_error(&[next]);
            }
            break;
        }
        let previous = std::mem::replace(&mut current, next);
        unsafe { release_preserving_error(&[previous]) };
        if current == slow {
            break;
        }
        if advance_slow {
            let next_slow = unsafe { PyException_GetContext(slow) };
            if next_slow.is_null() {
                break;
            }
            let previous = std::mem::replace(&mut slow, next_slow);
            unsafe { release_preserving_error(&[previous]) };
        }
        advance_slow = !advance_slow;
    }
    unsafe { release_preserving_error(&[current, slow]) };
    if raised_error_pending() {
        unsafe { release_preserving_error(&[context]) };
        return false;
    }
    // Steals context. SetObject replaces a prior explicit context, as CPython
    // does; Restore and Normalize never enter this operation.
    unsafe { PyException_SetContext(value, context) };
    !raised_error_pending()
}

/// Normalize through the existing class-call and exception-field authorities.
/// Native and managed instances retain identity under the same admission rule;
/// physical carrier tags never decide whether an exception must be rebuilt.
unsafe fn normalize_owned_error(
    error: OwnedCError,
    ingress: ExceptionIngress,
) -> Option<OwnedCError> {
    unsafe { normalize_prepared_error(move || Some(error), ingress, ExceptionPreparation::Owned) }
}

/// Preparation, replacement cleanup and class calls share one recursion scope.
/// Diagnostic text can itself fail while its runtime class is being projected;
/// admitting the scope only after that preparation leaves error reporting
/// unbounded even when every constructor obeys the normalization guard.
unsafe fn normalize_prepared_error(
    prepare: impl FnOnce() -> Option<OwnedCError>,
    ingress: ExceptionIngress,
    preparation: ExceptionPreparation,
) -> Option<OwnedCError> {
    let mut normalization = ExceptionNormalizationGuard::enter();
    if normalization.depth == NORMALIZATION_RECURSION_LIMIT {
        unsafe {
            if normalization.text_preparation_active() {
                // A failing text provider cannot prepare its own diagnostic.
                // Keep construction at the admitted class authority, with no
                // fabricated payload or second error-state representation.
                PyErr_SetNone((&raw mut crate::abi_types::PyExc_RecursionError).cast());
            } else {
                // Preserve the constructor recursion policy: report at the
                // limit, allow bounded RecursionError/MemoryError recovery,
                // and remain fatal if normalization itself cannot recover
                // (CPython 3.12 Python/errors.c, normalization error restart).
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RecursionError).cast(),
                    c"maximum recursion depth exceeded while normalizing an exception".as_ptr(),
                );
            }
        }
        with_preserved_error(|| drop(prepare));
        return None;
    }
    let superseded = if !matches!(ingress, ExceptionIngress::Normalize) {
        // A diagnostic may borrow its class or message from the old C error.
        // Detach that exact owner before clearing the indicators, then keep
        // its storage pinned until diagnostic preparation has finished.
        let previous = take_current_error();
        replace_current_error(None);
        previous
    } else {
        None
    };
    let prepared = normalization.prepare(preparation, prepare);
    if let Some(previous) = superseded {
        with_preserved_error(|| drop(previous));
    }
    let mut error = prepared?;
    if unsafe { normalize_error_in_place(&mut error, ingress) } {
        Some(error)
    } else {
        if !raised_error_pending() {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                    c"exception normalization failed without an exception".as_ptr(),
                );
            }
        }
        // Inputs and invalid constructor results can have reentrant finalizers.
        // Their disposal cannot replace the selected normalization failure.
        with_preserved_error(|| drop(error));
        None
    }
}

unsafe fn normalize_error_in_place(error: &mut OwnedCError, ingress: ExceptionIngress) -> bool {
    if !unsafe { exception_class_check(error.exc_type) } {
        if matches!(ingress, ExceptionIngress::Normalize) {
            return true;
        }
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                c"exception is not a BaseException subclass".as_ptr(),
            );
        }
        return false;
    }
    let instance_class = unsafe { exception_instance_class(error.value) };
    let matching = if instance_class.is_null() {
        false
    } else {
        match ingress {
            ExceptionIngress::Restore => instance_class.cast::<PyObject>() == error.exc_type,
            ExceptionIngress::SetObject | ExceptionIngress::Normalize => {
                let subclass = unsafe {
                    crate::api::object::PyObject_IsSubclass(instance_class.cast(), error.exc_type)
                };
                if subclass < 0 {
                    return false;
                }
                subclass != 0
            }
        }
    };
    if raised_error_pending() {
        return false;
    }
    if !matching {
        if !crate::hooks::native_gc_allocation_available() {
            // Before GC authority exists, construction cannot even allocate its
            // argument tuple. Keep only the admitted exception class in the
            // existing allocation-free error indicator. Arbitrary input is not
            // an exception instance and must never be published as one.
            let value = std::mem::replace(&mut error.value, ptr::null_mut());
            let traceback = std::mem::replace(&mut error.traceback, ptr::null_mut());
            unsafe { release_preserving_error(&[value, traceback]) };
            return true;
        }
        let args = if error.value.is_null() || error.value == &raw mut crate::abi_types::Py_None {
            unsafe { crate::api::sequences::native_call_args(&[]) }
        } else if unsafe { crate::api::sequences::PyTuple_Check(error.value) } != 0 {
            unsafe { crate::api::object::Py_NewRef(error.value) }
        } else {
            unsafe { crate::api::sequences::native_call_args(&[error.value]) }
        };
        if args.is_null() {
            return false;
        }
        let normalized =
            unsafe { crate::api::object::PyObject_Call(error.exc_type, args, ptr::null_mut()) };
        unsafe { release_preserving_error(&[args]) };
        if normalized.is_null() {
            return false;
        }
        let previous = std::mem::replace(&mut error.value, normalized);
        unsafe { release_preserving_error(&[previous]) };
    }
    let actual_class = unsafe { exception_instance_class(error.value) };
    if actual_class.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast(),
                c"calling an exception class did not return a BaseException instance".as_ptr(),
            );
        }
        return false;
    }
    if actual_class.cast::<PyObject>() != error.exc_type {
        let actual_type = unsafe { crate::api::object::Py_NewRef(actual_class.cast()) };
        let previous = std::mem::replace(&mut error.exc_type, actual_type);
        unsafe { release_preserving_error(&[previous]) };
    }
    match ingress {
        ExceptionIngress::SetObject => {
            if !unsafe { chain_implicit_exception_context(error.value) } {
                return false;
            }
            let traceback = unsafe { PyException_GetTraceback(error.value) };
            if raised_error_pending() {
                unsafe { release_preserving_error(&[traceback]) };
                return false;
            }
            let previous = std::mem::replace(&mut error.traceback, traceback);
            unsafe { release_preserving_error(&[previous]) };
        }
        ExceptionIngress::Restore => {
            // Restore explicitly clears the instance traceback when its owned
            // traceback argument is NULL/None. Normalize leaves it untouched.
            let traceback = if error.traceback.is_null() {
                &raw mut crate::abi_types::Py_None
            } else {
                error.traceback
            };
            if unsafe { PyException_SetTraceback(error.value, traceback) } != 0 {
                return false;
            }
            if error.traceback == &raw mut crate::abi_types::Py_None {
                let previous = std::mem::replace(&mut error.traceback, ptr::null_mut());
                unsafe { release_preserving_error(&[previous]) };
            }
        }
        ExceptionIngress::Normalize => {}
    }
    true
}

unsafe fn normalize_and_replace(error: OwnedCError, ingress: ExceptionIngress) {
    if let Some(error) = unsafe { normalize_owned_error(error, ingress) } {
        replace_current_error(Some(error));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetString(exc_type: *mut PyObject, message: *const c_char) {
    // Like SetObject, own the input before replacement/finalizers or diagnostic
    // allocation can retire a borrowed type's last current-error reference.
    unsafe { crate::api::refcount::Py_XINCREF(exc_type) };
    let mut error = OwnedCError {
        exc_type,
        value: ptr::null_mut(),
        traceback: ptr::null_mut(),
    };
    let normalized = unsafe {
        normalize_prepared_error(
            move || {
                let value = if crate::hooks::native_gc_allocation_available() {
                    let msg = if message.is_null() {
                        ""
                    } else {
                        let bytes = CStr::from_ptr(message).to_bytes();
                        let Ok(message_text) = std::str::from_utf8(bytes) else {
                            // Use the Unicode constructor's canonical decoder.
                            // Its exact error remains selected through cleanup.
                            let invalid = crate::api::strings::PyUnicode_FromString(message);
                            release_preserving_error(&[invalid]);
                            if !raised_error_pending() {
                                install_normalization_failure();
                            }
                            with_preserved_error(|| drop(error));
                            return None;
                        };
                        message_text
                    };
                    let value = allocate_exception_message(msg);
                    if value.is_null() && raised_error_pending() {
                        with_preserved_error(|| drop(error));
                        return None;
                    }
                    value
                } else {
                    // A partial text provider cannot admit construction without
                    // its GC owner. Normalize retains only the validated class.
                    ptr::null_mut()
                };
                error.value = value;
                Some(error)
            },
            ExceptionIngress::SetObject,
            ExceptionPreparation::Diagnostic,
        )
    };
    if let Some(error) = normalized {
        replace_current_error(Some(error));
    }
}

struct NativeExceptionKeywords {
    fields: [Option<(molt_lang_obj_model::ExceptionTypedField, *mut PyObject)>;
        molt_lang_obj_model::MAX_EXCEPTION_TYPED_FIELDS],
    len: usize,
}

impl Default for NativeExceptionKeywords {
    fn default() -> Self {
        Self {
            fields: [None; molt_lang_obj_model::MAX_EXCEPTION_TYPED_FIELDS],
            len: 0,
        }
    }
}

impl NativeExceptionKeywords {
    fn iter(
        &self,
    ) -> impl Iterator<Item = (molt_lang_obj_model::ExceptionTypedField, *mut PyObject)> + '_ {
        self.fields[..self.len].iter().flatten().copied()
    }
}

unsafe fn native_exception_keywords(
    layout: molt_lang_obj_model::ExceptionLayoutKind,
    kwds: *mut PyObject,
) -> Option<NativeExceptionKeywords> {
    let size = if kwds.is_null() {
        0
    } else {
        unsafe { crate::api::mapping::PyDict_Size(kwds) }
    };
    if size < 0 {
        return None;
    }
    let mut values = NativeExceptionKeywords::default();
    let mut recognized = 0isize;
    if !kwds.is_null() {
        for policy in layout.constructor_keyword_policies() {
            let name = CString::new(policy.python_name)
                .expect("exception constructor keyword contains no NUL");
            let value = unsafe { crate::api::mapping::PyDict_GetItemString(kwds, name.as_ptr()) };
            if !value.is_null() {
                values.fields[values.len] = Some((policy.field, value));
                values.len += 1;
                recognized += 1;
            }
        }
    }
    if recognized != size {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"invalid keyword argument for builtin exception".as_ptr(),
            )
        };
        return None;
    }
    Some(values)
}

unsafe fn clear_native_typed_fields(
    instance: *mut PyBaseExceptionObject,
    layout: molt_lang_obj_model::ExceptionLayoutKind,
) {
    for policy in layout.field_policies() {
        if let Some(slot) =
            unsafe { crate::abi_types::exception_typed_object_slot(instance, layout, policy.field) }
        {
            let value = unsafe { std::mem::replace(&mut *slot, ptr::null_mut()) };
            unsafe { crate::api::refcount::Py_XDECREF(value) };
        }
    }
    unsafe { crate::abi_types::initialize_exception_typed_scalars(instance, layout) };
}

/// Visit every owned edge in the exact native builtin-exception shape.  The
/// common prefix and typed tail share one schema-derived traversal authority so
/// a new field cannot be added to projection/allocation without also becoming
/// visible to cyclic GC.
pub unsafe extern "C" fn molt_native_exception_traverse(
    op: *mut PyObject,
    visit_raw: *mut c_void,
    arg: *mut c_void,
) -> c_int {
    if op.is_null() || visit_raw.is_null() {
        return 0;
    }
    if GLOBAL_BRIDGE.managed_handle_for_pyobj(op).is_some() {
        return unsafe { crate::api::memory::molt_managed_gc_traverse(op, visit_raw, arg) };
    }
    let Some(layout) = (unsafe { crate::abi_types::exception_layout_for_type((*op).ob_type) })
    else {
        return 0;
    };
    type VisitProc = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;
    let visit: VisitProc = unsafe { std::mem::transmute(visit_raw) };
    let instance = op.cast::<PyBaseExceptionObject>();
    let common = unsafe {
        [
            (*instance).dict,
            (*instance).args,
            (*instance).notes,
            (*instance).traceback,
            (*instance).cause,
            (*instance).context,
        ]
    };
    for reference in common {
        if !reference.is_null() {
            let result = unsafe { visit(reference, arg) };
            if result != 0 {
                return result;
            }
        }
    }
    for policy in layout.field_policies() {
        let Some(slot) = (unsafe {
            crate::abi_types::exception_typed_object_slot(instance, layout, policy.field)
        }) else {
            continue;
        };
        let reference = unsafe { *slot };
        if !reference.is_null() {
            let result = unsafe { visit(reference, arg) };
            if result != 0 {
                return result;
            }
        }
    }
    0
}

/// Break every owned edge in the exact native builtin-exception shape.  This
/// is both the type ``tp_clear`` slot and the deallocator's single clearing
/// authority; it is safe to call repeatedly.
pub unsafe extern "C" fn molt_native_exception_clear(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if GLOBAL_BRIDGE.managed_handle_for_pyobj(op).is_some() {
        return unsafe { crate::api::memory::molt_managed_gc_clear(op) };
    }
    let Some(layout) = (unsafe { crate::abi_types::exception_layout_for_type((*op).ob_type) })
    else {
        return 0;
    };
    let instance = op.cast::<PyBaseExceptionObject>();
    unsafe {
        clear_native_typed_fields(instance, layout);
        crate::api::refcount::Py_CLEAR(&raw mut (*instance).dict);
        crate::api::refcount::Py_CLEAR(&raw mut (*instance).args);
        crate::api::refcount::Py_CLEAR(&raw mut (*instance).notes);
        crate::api::refcount::Py_CLEAR(&raw mut (*instance).traceback);
        crate::api::refcount::Py_CLEAR(&raw mut (*instance).cause);
        crate::api::refcount::Py_CLEAR(&raw mut (*instance).context);
    }
    0
}

const PY_T_OBJECT: c_int = 6;
const PY_T_BOOL: c_int = 14;
const PY_T_PYSSIZET: c_int = 19;
const PY_READONLY: c_int = 1;

struct ExceptionDescriptorTable {
    _names: Vec<CString>,
    members: Box<[PyMemberDef]>,
    getsets: Box<[PyGetSetDef]>,
}

// Runtime descriptors and native C tables consume the same declarations.
// Only physical offsets and native callback adapters are selected here.
static EXCEPTION_DESCRIPTOR_TABLES: Lazy<Vec<ExceptionDescriptorTable>> = Lazy::new(|| {
    use molt_lang_obj_model::{ExceptionAttributeField as Field, ExceptionDescriptorKind};
    molt_lang_obj_model::ExceptionLayoutRoot::ALL.into_iter().map(|root| {
        let mut names = Vec::new();
        let mut members = Vec::new();
        let mut getsets = Vec::new();
        for declaration in root.attribute_declarations() {
            let name = CString::new(declaration.python_name)
                .expect("exception descriptor names contain no NUL");
            match declaration.field.descriptor_kind() {
                ExceptionDescriptorKind::Member => {
                    let (type_, offset, flags) = match declaration.field {
                        Field::SuppressContext => (
                            PY_T_BOOL,
                            core::mem::offset_of!(PyBaseExceptionObject, suppress_context) as Py_ssize_t,
                            0,
                        ),
                        Field::Typed(field) => {
                            let policy = root.kind().field_policy(field).expect("declared typed field policy");
                            let Some(offset) = crate::abi_types::exception_typed_field_offset(field) else { continue; };
                            let type_ = match policy.storage {
                                molt_lang_obj_model::ExceptionFieldStorage::RuntimeMessage
                                | molt_lang_obj_model::ExceptionFieldStorage::Object => PY_T_OBJECT,
                                molt_lang_obj_model::ExceptionFieldStorage::PySsize => PY_T_PYSSIZET,
                            };
                            (type_, offset, if policy.writable { 0 } else { PY_READONLY })
                        }
                        _ => unreachable!("member declaration has physical storage"),
                    };
                    members.push(PyMemberDef { name: name.as_ptr(), type_, offset, flags, doc: ptr::null() });
                }
                ExceptionDescriptorKind::GetSet => {
                    let (get, set): (crate::abi_types::getter, crate::abi_types::setter) = match declaration.field {
                        Field::Dictionary => (crate::api::object::PyObject_GenericGetDict, crate::api::object::PyObject_GenericSetDict),
                        Field::Args => (native_exception_args_get, native_exception_args_set),
                        Field::Traceback => (native_exception_traceback_get, native_exception_traceback_set),
                        Field::Context => (native_exception_context_get, native_exception_context_set),
                        Field::Cause => (native_exception_cause_get, native_exception_cause_set),
                        Field::Typed(molt_lang_obj_model::ExceptionTypedField::OSErrorCharactersWritten) => (native_oserror_written_get, native_oserror_written_set),
                        _ => unreachable!("getset declaration has native accessors"),
                    };
                    getsets.push(PyGetSetDef { name: name.as_ptr(), get: Some(get), set: Some(set), doc: ptr::null(), closure: ptr::null_mut() });
                }
            }
            names.push(name);
        }
        members.push(PyMemberDef { name: ptr::null(), type_: 0, offset: 0, flags: 0, doc: ptr::null() });
        getsets.push(PyGetSetDef { name: ptr::null(), get: None, set: None, doc: ptr::null(), closure: ptr::null_mut() });
        ExceptionDescriptorTable { _names: names, members: members.into_boxed_slice(), getsets: getsets.into_boxed_slice() }
    }).collect()
});
unsafe extern "C" fn native_exception_args_get(
    op: *mut PyObject,
    _closure: *mut c_void,
) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(op, crate::hooks::ExceptionField::Args) }
    {
        return match result {
            Ok(value) if value.is_null() => unsafe {
                crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None)
            },
            Ok(value) => value,
            Err(()) => ptr::null_mut(),
        };
    }
    let Some(base) = foreign_exception_layout(op) else {
        return ptr::null_mut();
    };
    let args = unsafe { (*base).args };
    if args.is_null() {
        unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
    } else {
        unsafe { crate::api::object::Py_NewRef(args) }
    }
}

unsafe extern "C" fn native_exception_args_set(
    op: *mut PyObject,
    value: *mut PyObject,
    _closure: *mut c_void,
) -> c_int {
    if value.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"args may not be deleted".as_ptr(),
            )
        };
        return -1;
    }
    let tuple = unsafe { crate::api::abstract_sequence::PySequence_Tuple(value) };
    if tuple.is_null() {
        return -1;
    }
    if let Some(status) =
        unsafe { managed_exception_set_field(op, crate::hooks::ExceptionField::Args, tuple) }
    {
        unsafe { crate::api::refcount::Py_DECREF(tuple) };
        return status;
    }
    let Some(base) = foreign_exception_layout(op) else {
        unsafe { crate::api::refcount::Py_DECREF(tuple) };
        return -1;
    };
    unsafe {
        let old = std::mem::replace(&mut (*base).args, tuple);
        crate::api::refcount::Py_XDECREF(old);
    }
    0
}

unsafe extern "C" fn native_exception_traceback_get(
    op: *mut PyObject,
    _closure: *mut c_void,
) -> *mut PyObject {
    let value = unsafe { PyException_GetTraceback(op) };
    if value.is_null() && unsafe { PyErr_Occurred() }.is_null() {
        unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
    } else {
        value
    }
}

unsafe extern "C" fn native_exception_traceback_set(
    op: *mut PyObject,
    value: *mut PyObject,
    _closure: *mut c_void,
) -> c_int {
    if value.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"__traceback__ may not be deleted".as_ptr(),
            )
        };
        return -1;
    }
    unsafe { PyException_SetTraceback(op, value) }
}

unsafe extern "C" fn native_exception_context_get(
    op: *mut PyObject,
    _closure: *mut c_void,
) -> *mut PyObject {
    let value = unsafe { PyException_GetContext(op) };
    if value.is_null() && unsafe { PyErr_Occurred() }.is_null() {
        unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
    } else {
        value
    }
}

unsafe extern "C" fn native_exception_context_set(
    op: *mut PyObject,
    value: *mut PyObject,
    _closure: *mut c_void,
) -> c_int {
    if value.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"__context__ may not be deleted".as_ptr(),
            )
        };
        return -1;
    }
    unsafe { PyException_SetContext(op, crate::api::object::Py_NewRef(value)) };
    if unsafe { PyErr_Occurred() }.is_null() {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn native_exception_cause_get(
    op: *mut PyObject,
    _closure: *mut c_void,
) -> *mut PyObject {
    let value = unsafe { PyException_GetCause(op) };
    if value.is_null() && unsafe { PyErr_Occurred() }.is_null() {
        unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
    } else {
        value
    }
}

unsafe extern "C" fn native_exception_cause_set(
    op: *mut PyObject,
    value: *mut PyObject,
    _closure: *mut c_void,
) -> c_int {
    if value.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"__cause__ may not be deleted".as_ptr(),
            )
        };
        return -1;
    }
    unsafe { PyException_SetCause(op, crate::api::object::Py_NewRef(value)) };
    if unsafe { PyErr_Occurred() }.is_null() {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn native_oserror_written_get(
    op: *mut PyObject,
    _closure: *mut c_void,
) -> *mut PyObject {
    let written = unsafe { (*op.cast::<crate::abi_types::PyOSErrorObject>()).written };
    if written == -1 {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_AttributeError).cast::<PyObject>(),
                c"characters_written".as_ptr(),
            )
        };
        ptr::null_mut()
    } else {
        unsafe { crate::api::numbers::PyLong_FromSsize_t(written) }
    }
}

unsafe extern "C" fn native_oserror_written_set(
    op: *mut PyObject,
    value: *mut PyObject,
    _closure: *mut c_void,
) -> c_int {
    let object = op.cast::<crate::abi_types::PyOSErrorObject>();
    if value.is_null() {
        if unsafe { (*object).written } == -1 {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_AttributeError).cast::<PyObject>(),
                    c"characters_written".as_ptr(),
                )
            };
            return -1;
        }
        unsafe { (*object).written = -1 };
        return 0;
    }
    let written = unsafe {
        crate::api::abstract_number::PyNumber_AsSsize_t(
            value,
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<PyObject>(),
        )
    };
    if written == -1 && !unsafe { PyErr_Occurred() }.is_null() {
        return -1;
    }
    unsafe { (*object).written = written };
    0
}

pub(crate) fn native_exception_members_for_builtin(
    builtin_name: &str,
    root: molt_lang_obj_model::ExceptionLayoutRoot,
) -> *mut PyMemberDef {
    if builtin_name != root.owner_name() {
        return ptr::null_mut();
    }
    let entries = &EXCEPTION_DESCRIPTOR_TABLES[root as usize].members;
    if entries.len() == 1 {
        ptr::null_mut()
    } else {
        entries.as_ptr().cast_mut()
    }
}

pub(crate) fn native_exception_getset_for_builtin(
    builtin_name: &str,
    root: molt_lang_obj_model::ExceptionLayoutRoot,
) -> *mut PyGetSetDef {
    if builtin_name != root.owner_name() {
        return ptr::null_mut();
    }
    let entries = &EXCEPTION_DESCRIPTOR_TABLES[root as usize].getsets;
    if entries.len() == 1 {
        ptr::null_mut()
    } else {
        entries.as_ptr().cast_mut()
    }
}
/// Construct the physical descriptor declared by a builtin exception owner.
/// Runtime native descriptors use this for genuinely C-owned receivers. The
/// member/getset table remains the physical accessor authority; ordinary type
/// lookup would redispatch into the shared semantic descriptor and recurse.
///
/// # Safety
/// `owner` must be a live, initialized builtin exception type shell.
pub unsafe fn native_exception_field_descriptor(
    owner: *mut PyTypeObject,
    name: &[u8],
) -> *mut PyObject {
    unsafe {
        if owner.is_null() {
            PyErr_BadInternalCall();
            return ptr::null_mut();
        }
        let mut member = (*owner).tp_members;
        if !member.is_null() {
            while !(*member).name.is_null() {
                if CStr::from_ptr((*member).name).to_bytes() == name {
                    return crate::api::typeobj::PyDescr_NewMember(owner, member);
                }
                member = member.add(1);
            }
        }
        let mut getset = (*owner).tp_getset;
        if !getset.is_null() {
            while !(*getset).name.is_null() {
                if CStr::from_ptr((*getset).name).to_bytes() == name {
                    return crate::api::typeobj::PyDescr_NewGetSet(owner, getset);
                }
                getset = getset.add(1);
            }
        }
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
            c"exception field is absent from its physical declaration".as_ptr(),
        );
        ptr::null_mut()
    }
}

unsafe fn set_native_typed_field(
    instance: *mut PyBaseExceptionObject,
    layout: molt_lang_obj_model::ExceptionLayoutKind,
    field: molt_lang_obj_model::ExceptionTypedField,
    value: *mut PyObject,
) -> bool {
    let Some(slot) =
        (unsafe { crate::abi_types::exception_typed_object_slot(instance, layout, field) })
    else {
        return false;
    };
    unsafe {
        crate::api::refcount::Py_XINCREF(value);
        let old = std::mem::replace(&mut *slot, value);
        crate::api::refcount::Py_XDECREF(old);
    }
    true
}

unsafe fn clear_native_typed_field(
    instance: *mut PyBaseExceptionObject,
    layout: molt_lang_obj_model::ExceptionLayoutKind,
    field: molt_lang_obj_model::ExceptionTypedField,
) {
    if let Some(slot) =
        unsafe { crate::abi_types::exception_typed_object_slot(instance, layout, field) }
    {
        let old = unsafe { std::mem::replace(&mut *slot, ptr::null_mut()) };
        unsafe { crate::api::refcount::Py_XDECREF(old) };
    }
}

unsafe fn replace_native_args(instance: *mut PyBaseExceptionObject, args: *mut PyObject) {
    unsafe {
        let old = std::mem::replace(&mut (*instance).args, args);
        crate::api::refcount::Py_XDECREF(old);
    }
}

unsafe fn owned_exception_args(args: *mut PyObject) -> *mut PyObject {
    if args.is_null() {
        unsafe { crate::api::sequences::PyTuple_New(0) }
    } else {
        unsafe { crate::api::object::Py_NewRef(args) }
    }
}

unsafe fn oserror_use_init(type_: *mut PyTypeObject) -> bool {
    if type_.is_null() {
        return false;
    }
    let custom_init = match unsafe { (*type_).tp_init } {
        Some(init) => !std::ptr::fn_addr_eq(
            init,
            molt_native_exception_init
                as unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int,
        ),
        None => true,
    };
    let inherited_new = unsafe { (*type_).tp_new }.is_some_and(|new_| {
        std::ptr::fn_addr_eq(
            new_,
            molt_native_exception_new
                as unsafe extern "C" fn(
                    *mut PyTypeObject,
                    *mut PyObject,
                    *mut PyObject,
                ) -> *mut PyObject,
        )
    });
    custom_init && inherited_new
}

unsafe fn oserror_subtype_for_errno(errno: c_int) -> *mut PyTypeObject {
    let singleton = |name| {
        crate::abi_types::exc_singleton_for_builtin_name(name)
            .expect("OSError errno subtype singleton")
            .cast::<PyTypeObject>()
    };
    if [
        platform_errno::EAGAIN,
        platform_errno::EALREADY,
        platform_errno::EINPROGRESS,
        platform_errno::EWOULDBLOCK,
    ]
    .contains(&errno)
    {
        singleton("BlockingIOError")
    } else if errno == platform_errno::EPIPE || {
        // CPython adds ESHUTDOWN to the BrokenPipeError map only when the
        // platform exposes that errno. WASI deliberately does not; keeping
        // the capability check at this classifier boundary avoids inventing
        // a numeric errno that the target cannot report.
        #[cfg(windows)]
        {
            errno == molt_runtime_platform::windows_abi::WSAESHUTDOWN
        }
        #[cfg(all(not(windows), not(target_os = "wasi")))]
        {
            errno == platform_errno::ESHUTDOWN
        }
        #[cfg(target_os = "wasi")]
        {
            false
        }
    } {
        singleton("BrokenPipeError")
    } else if errno == platform_errno::ECHILD {
        singleton("ChildProcessError")
    } else if errno == platform_errno::ECONNABORTED {
        singleton("ConnectionAbortedError")
    } else if errno == platform_errno::ECONNREFUSED {
        singleton("ConnectionRefusedError")
    } else if errno == platform_errno::ECONNRESET {
        singleton("ConnectionResetError")
    } else if errno == platform_errno::EEXIST {
        singleton("FileExistsError")
    } else if errno == platform_errno::ENOENT {
        singleton("FileNotFoundError")
    } else if errno == platform_errno::EISDIR {
        singleton("IsADirectoryError")
    } else if errno == platform_errno::ENOTDIR {
        singleton("NotADirectoryError")
    } else if errno == platform_errno::EINTR {
        singleton("InterruptedError")
    } else if errno == platform_errno::EACCES || errno == platform_errno::EPERM {
        singleton("PermissionError")
    } else if errno == platform_errno::ESRCH {
        singleton("ProcessLookupError")
    } else if errno == platform_errno::ETIMEDOUT {
        singleton("TimeoutError")
    } else {
        (&raw mut crate::abi_types::PyExc_OSError).cast::<PyTypeObject>()
    }
}

#[cfg(windows)]
unsafe fn normalize_oserror_windows_args(args: *mut PyObject) -> *mut PyObject {
    let nargs = unsafe { crate::api::sequences::PyTuple_Size(args) };
    if !(2..=5).contains(&nargs) || nargs < 4 {
        return args;
    }
    let winerror = unsafe { crate::api::sequences::PyTuple_GetItem(args, 3) };
    if unsafe { crate::api::numbers::PyLong_Check(winerror) } == 0 {
        return args;
    }
    let winerror = unsafe { crate::api::numbers::PyLong_AsLong(winerror) };
    if winerror == -1 && !unsafe { PyErr_Occurred() }.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(args) };
        return ptr::null_mut();
    }
    let errno = molt_runtime_platform::windows_abi::winerror_to_errno(winerror as i32);
    let errno_object = unsafe { crate::api::numbers::PyLong_FromLong(errno as c_long) };
    let normalized = unsafe { crate::api::sequences::PyTuple_New(nargs) };
    if errno_object.is_null() || normalized.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(errno_object);
            crate::api::refcount::Py_XDECREF(normalized);
            crate::api::refcount::Py_DECREF(args);
        }
        return ptr::null_mut();
    }
    unsafe { crate::api::sequences::PyTuple_SetItem(normalized, 0, errno_object) };
    for index in 1..nargs {
        let value = unsafe { crate::api::sequences::PyTuple_GetItem(args, index) };
        unsafe {
            crate::api::refcount::Py_INCREF(value);
            crate::api::sequences::PyTuple_SetItem(normalized, index, value);
        }
    }
    unsafe { crate::api::refcount::Py_DECREF(args) };
    normalized
}

#[cfg(windows)]
unsafe fn normalize_oserror_platform_args(args: *mut PyObject) -> *mut PyObject {
    unsafe { normalize_oserror_windows_args(args) }
}

#[cfg(not(windows))]
unsafe fn normalize_oserror_platform_args(args: *mut PyObject) -> *mut PyObject {
    args
}

unsafe fn initialize_oserror_fields(
    instance: *mut PyBaseExceptionObject,
    args: *mut PyObject,
) -> c_int {
    let layout = molt_lang_obj_model::ExceptionLayoutKind::OSError;
    unsafe { clear_native_typed_fields(instance, layout) };
    let nargs = unsafe { crate::api::sequences::PyTuple_Size(args) };
    if !(2..=5).contains(&nargs) {
        unsafe { replace_native_args(instance, crate::api::object::Py_NewRef(args)) };
        return 0;
    }
    let item = |index| unsafe { crate::api::sequences::PyTuple_GetItem(args, index) };
    unsafe {
        set_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::OSErrorErrno,
            item(0),
        );
        set_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::OSErrorStrError,
            item(1),
        );
    }
    let filename = if nargs >= 3 { item(2) } else { ptr::null_mut() };
    let filename_present =
        !filename.is_null() && !std::ptr::eq(filename, &raw mut crate::abi_types::Py_None);
    if filename_present {
        let exact_blocking = std::ptr::eq(
            unsafe { (*instance).ob_base.ob_type },
            &raw mut crate::abi_types::PyExc_BlockingIOError,
        );
        if exact_blocking && unsafe { crate::api::numbers::PyNumber_Check(filename) } != 0 {
            let written = unsafe {
                crate::api::abstract_number::PyNumber_AsSsize_t(
                    filename,
                    (&raw mut crate::abi_types::PyExc_ValueError).cast::<PyObject>(),
                )
            };
            if written == -1 && !unsafe { PyErr_Occurred() }.is_null() {
                return -1;
            }
            unsafe {
                (*instance.cast::<crate::abi_types::PyOSErrorObject>()).written = written;
            }
        } else {
            unsafe {
                set_native_typed_field(
                    instance,
                    layout,
                    molt_lang_obj_model::ExceptionTypedField::OSErrorFilename,
                    filename,
                )
            };
            if nargs >= 5 && !std::ptr::eq(item(4), &raw mut crate::abi_types::Py_None) {
                unsafe {
                    set_native_typed_field(
                        instance,
                        layout,
                        molt_lang_obj_model::ExceptionTypedField::OSErrorFilename2,
                        item(4),
                    )
                };
            }
            let short_args = unsafe { crate::api::sequences::PyTuple_GetSlice(args, 0, 2) };
            if short_args.is_null() {
                return -1;
            }
            unsafe { replace_native_args(instance, short_args) };
        }
    }
    #[cfg(windows)]
    if nargs >= 4 {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::OSErrorWinError,
                item(3),
            )
        };
    }
    if !filename_present {
        unsafe { replace_native_args(instance, crate::api::object::Py_NewRef(args)) };
    }
    0
}

unsafe fn initialize_unicode_fields(
    instance: *mut PyBaseExceptionObject,
    root: molt_lang_obj_model::ExceptionLayoutRoot,
    args: *mut PyObject,
) -> c_int {
    use molt_lang_obj_model::ExceptionLayoutRoot::{UnicodeDecodeError, UnicodeTranslateError};
    let layout = molt_lang_obj_model::ExceptionLayoutKind::Unicode;
    unsafe { clear_native_typed_fields(instance, layout) };
    let nargs = unsafe { crate::api::sequences::PyTuple_Size(args) };
    let expected = if root == UnicodeTranslateError { 4 } else { 5 };
    if nargs != expected {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                if root == UnicodeTranslateError {
                    c"UnicodeTranslateError requires exactly 4 arguments".as_ptr()
                } else {
                    c"Unicode error requires exactly 5 arguments".as_ptr()
                },
            )
        };
        return -1;
    }
    let item = |index| unsafe { crate::api::sequences::PyTuple_GetItem(args, index) };
    let (encoding, object, start_obj, end_obj, reason) = if root == UnicodeTranslateError {
        (ptr::null_mut(), item(0), item(1), item(2), item(3))
    } else {
        (item(0), item(1), item(2), item(3), item(4))
    };
    let unicode_ok = |value| unsafe { crate::api::strings::PyUnicode_Check(value) } != 0;
    if (root != UnicodeTranslateError && !unicode_ok(encoding))
        || (root != UnicodeDecodeError && !unicode_ok(object))
        || !unicode_ok(reason)
    {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"Unicode error arguments have invalid types".as_ptr(),
            )
        };
        return -1;
    }
    let start = unsafe { crate::api::numbers::PyLong_AsSsize_t(start_obj) };
    if start == -1 && !unsafe { PyErr_Occurred() }.is_null() {
        return -1;
    }
    let end = unsafe { crate::api::numbers::PyLong_AsSsize_t(end_obj) };
    if end == -1 && !unsafe { PyErr_Occurred() }.is_null() {
        return -1;
    }
    if root != UnicodeTranslateError {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::UnicodeEncoding,
                encoding,
            )
        };
    }
    if root == UnicodeDecodeError && unsafe { crate::api::strings::PyBytes_Check(object) } == 0 {
        let mut view: Py_buffer = unsafe { std::mem::zeroed() };
        if unsafe { crate::api::buffer::PyObject_GetBuffer(object, &raw mut view, PyBUF_SIMPLE) }
            != 0
        {
            return -1;
        }
        let bytes = unsafe {
            crate::api::strings::PyBytes_FromStringAndSize(view.buf.cast::<c_char>(), view.len)
        };
        unsafe { crate::api::buffer::PyBuffer_Release(&raw mut view) };
        if bytes.is_null() {
            return -1;
        }
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::UnicodeObject,
                bytes,
            );
            crate::api::refcount::Py_DECREF(bytes);
        }
    } else {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::UnicodeObject,
                object,
            )
        };
    }
    unsafe {
        set_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::UnicodeReason,
            reason,
        );
        let unicode = &mut *instance.cast::<crate::abi_types::PyUnicodeErrorObject>();
        unicode.start = start;
        unicode.end = end;
    }
    0
}

unsafe fn initialize_syntax_fields(
    instance: *mut PyBaseExceptionObject,
    args: *mut PyObject,
) -> c_int {
    let layout = molt_lang_obj_model::ExceptionLayoutKind::Syntax;
    let nargs = unsafe { crate::api::sequences::PyTuple_Size(args) };
    let item = |index| unsafe { crate::api::sequences::PyTuple_GetItem(args, index) };
    if nargs >= 1 {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::SyntaxMessage,
                item(0),
            )
        };
    }
    if nargs != 2 {
        return 0;
    }
    let info = unsafe { crate::api::abstract_sequence::PySequence_Tuple(item(1)) };
    if info.is_null() {
        return -1;
    }
    // CPython clears the optional end pair immediately after successful
    // sequence conversion, before arity validation or replacement of the
    // first four location fields.  Re-init failure therefore has observable
    // staged state that must not be made transactional.
    unsafe {
        clear_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::SyntaxEndLineNumber,
        );
        clear_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::SyntaxEndOffset,
        );
    }
    let info_len = unsafe { crate::api::sequences::PyTuple_Size(info) };
    if !(4..=6).contains(&info_len) {
        unsafe {
            crate::api::refcount::Py_DECREF(info);
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"SyntaxError location must contain 4 to 6 items".as_ptr(),
            );
        }
        return -1;
    }
    for (source, field) in [
        (0, molt_lang_obj_model::ExceptionTypedField::SyntaxFilename),
        (
            1,
            molt_lang_obj_model::ExceptionTypedField::SyntaxLineNumber,
        ),
        (2, molt_lang_obj_model::ExceptionTypedField::SyntaxOffset),
        (3, molt_lang_obj_model::ExceptionTypedField::SyntaxText),
    ] {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                field,
                crate::api::sequences::PyTuple_GetItem(info, source),
            );
        }
    }
    if info_len >= 5 {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::SyntaxEndLineNumber,
                crate::api::sequences::PyTuple_GetItem(info, 4),
            );
        }
    }
    if info_len == 6 {
        unsafe {
            set_native_typed_field(
                instance,
                layout,
                molt_lang_obj_model::ExceptionTypedField::SyntaxEndOffset,
                crate::api::sequences::PyTuple_GetItem(info, 5),
            );
        }
    }
    unsafe { crate::api::refcount::Py_DECREF(info) };
    if info_len == 5 {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"end_offset must be provided when end_lineno is provided".as_ptr(),
            )
        };
        return -1;
    }
    0
}

/// Initialize the exact native typed exception tail. This is the `tp_init`
/// authority for static PyExc types and their honest C subtypes; managed
/// runtime exceptions use the atomic snapshot transaction instead.
pub unsafe extern "C" fn molt_native_exception_init(
    op: *mut PyObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> c_int {
    if op.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return -1;
    }
    let subtype = unsafe { (*op).ob_type };
    let Some(root) = (unsafe { crate::abi_types::exception_layout_root_for_type(subtype) }) else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"native exception has no canonical physical layout".as_ptr(),
            )
        };
        return -1;
    };
    let layout = root.kind();
    if root == molt_lang_obj_model::ExceptionLayoutRoot::OSError
        && !unsafe { oserror_use_init(subtype) }
    {
        // The ordinary OSError path is parsed and initialized atomically in
        // tp_new so errno subtype selection precedes allocation.
        return 0;
    }
    let Some(keywords) = (unsafe { native_exception_keywords(layout, kwds) }) else {
        return -1;
    };
    let mut args = unsafe { owned_exception_args(args) };
    if args.is_null() {
        return -1;
    }
    if root == molt_lang_obj_model::ExceptionLayoutRoot::OSError {
        args = unsafe { normalize_oserror_platform_args(args) };
        if args.is_null() {
            return -1;
        }
    }
    let nargs = unsafe { crate::api::sequences::PyTuple_Size(args) };
    if nargs < 0 {
        unsafe { crate::api::refcount::Py_DECREF(args) };
        return -1;
    }
    let instance = op.cast::<PyBaseExceptionObject>();
    unsafe { replace_native_args(instance, args) };
    let item = |index: isize| unsafe { crate::api::sequences::PyTuple_GetItem(args, index) };
    match layout {
        molt_lang_obj_model::ExceptionLayoutKind::Base => {}
        molt_lang_obj_model::ExceptionLayoutKind::Group => {}
        molt_lang_obj_model::ExceptionLayoutKind::Syntax => {
            return unsafe { initialize_syntax_fields(instance, args) };
        }
        molt_lang_obj_model::ExceptionLayoutKind::Import => {
            unsafe { clear_native_typed_fields(instance, layout) };
            if nargs == 1 {
                unsafe {
                    set_native_typed_field(
                        instance,
                        layout,
                        molt_lang_obj_model::ExceptionTypedField::ImportMessage,
                        item(0),
                    )
                };
            }
            for (field, value) in keywords.iter() {
                unsafe { set_native_typed_field(instance, layout, field, value) };
            }
        }
        molt_lang_obj_model::ExceptionLayoutKind::Unicode => {
            return unsafe { initialize_unicode_fields(instance, root, args) };
        }
        molt_lang_obj_model::ExceptionLayoutKind::SystemExit => {
            let code = match nargs {
                0 => ptr::null_mut(),
                1 => item(0),
                _ => args,
            };
            if !code.is_null() {
                unsafe {
                    set_native_typed_field(
                        instance,
                        layout,
                        molt_lang_obj_model::ExceptionTypedField::SystemExitCode,
                        code,
                    )
                };
            }
        }
        molt_lang_obj_model::ExceptionLayoutKind::OSError => {
            return unsafe { initialize_oserror_fields(instance, args) };
        }
        molt_lang_obj_model::ExceptionLayoutKind::StopIteration => {
            unsafe { clear_native_typed_fields(instance, layout) };
            let value = if nargs == 0 {
                &raw mut crate::abi_types::Py_None
            } else {
                item(0)
            };
            unsafe {
                set_native_typed_field(
                    instance,
                    layout,
                    molt_lang_obj_model::ExceptionTypedField::StopIterationValue,
                    value,
                )
            };
        }
        molt_lang_obj_model::ExceptionLayoutKind::NameError => {
            unsafe { clear_native_typed_fields(instance, layout) };
            for (field, value) in keywords.iter() {
                unsafe { set_native_typed_field(instance, layout, field, value) };
            }
        }
        molt_lang_obj_model::ExceptionLayoutKind::AttributeError => {
            unsafe { clear_native_typed_fields(instance, layout) };
            for (field, value) in keywords.iter() {
                unsafe { set_native_typed_field(instance, layout, field, value) };
            }
        }
    }
    0
}

unsafe fn allocate_native_exception(
    subtype: *mut PyTypeObject,
    args: *mut PyObject,
) -> *mut PyBaseExceptionObject {
    let Some(exception_layout) = (unsafe { crate::abi_types::exception_layout_for_type(subtype) })
    else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"native exception type has no canonical physical layout".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let required_size = crate::abi_types::exception_layout_basicsize(exception_layout);
    if unsafe { (*subtype).tp_basicsize } < required_size {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"native exception tp_basicsize is smaller than its physical layout".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    if let Some(reason) = unsafe { crate::api::memory::native_gc_type_admission_error(subtype) } {
        unsafe { crate::api::memory::raise_native_gc_admission_error(reason) };
        return ptr::null_mut();
    }
    let owned_args = unsafe { owned_exception_args(args) };
    if owned_args.is_null() {
        return ptr::null_mut();
    }
    let allocate =
        unsafe { (*subtype).tp_alloc }.unwrap_or(crate::api::typeobj::PyType_GenericAlloc);
    let generic_allocator = std::ptr::fn_addr_eq(
        allocate,
        crate::api::typeobj::PyType_GenericAlloc
            as unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject,
    );
    let allocation = unsafe { allocate(subtype, 0) };
    if allocation.is_null() {
        unsafe {
            check_native_result(allocation, "native exception tp_alloc");
            release_preserving_error(&[owned_args]);
        }
        return ptr::null_mut();
    }
    if !generic_allocator
        && unsafe { (crate::hooks::hooks_or_stubs().native_gc_allocate)(allocation.addr()) } < 0
    {
        unsafe { check_native_status(-1, "native_gc_allocate") };
        let free = unsafe { (*subtype).tp_free }.unwrap_or(crate::api::memory::PyObject_GC_Del);
        let heap_type = unsafe { (*subtype).tp_flags & crate::abi_types::Py_TPFLAGS_HEAPTYPE != 0 };
        with_preserved_error(|| unsafe {
            crate::api::refcount::Py_DECREF(owned_args);
            if !std::ptr::fn_addr_eq(
                free,
                crate::api::memory::PyObject_GC_Del as unsafe extern "C" fn(*mut c_void),
            ) {
                crate::api::memory::native_gc_node_deallocate(allocation.addr());
            }
            free(allocation.cast::<c_void>());
            if heap_type {
                crate::api::refcount::Py_DECREF(subtype.cast::<PyObject>());
            }
        });
        return ptr::null_mut();
    }
    let instance = allocation.cast::<PyBaseExceptionObject>();
    unsafe {
        (*instance).dict = ptr::null_mut();
        (*instance).args = owned_args;
        (*instance).notes = ptr::null_mut();
        (*instance).traceback = ptr::null_mut();
        (*instance).context = ptr::null_mut();
        (*instance).cause = ptr::null_mut();
        (*instance).suppress_context = 0;
        for policy in exception_layout.field_policies() {
            if let Some(slot) = crate::abi_types::exception_typed_object_slot(
                instance,
                exception_layout,
                policy.field,
            ) {
                *slot = ptr::null_mut();
            }
        }
        crate::abi_types::initialize_exception_typed_scalars(instance, exception_layout);
    }
    if unsafe { crate::api::memory::PyObject_GC_IsTracked(allocation) } == 0
        && unsafe { (crate::hooks::hooks_or_stubs().native_gc_track)(allocation.addr()) } < 0
    {
        unsafe {
            check_native_status(-1, "native_gc_track");
            release_preserving_error(&[allocation]);
        }
        return ptr::null_mut();
    }
    instance
}

/// [`native_exception_identity`] bits.
pub const NATIVE_EXCEPTION_INSTANCE: u8 = 1;
pub const NATIVE_EXCEPTION_SUBCLASS: u8 = 2;

/// Real-type identity of one native C object for the runtime exception-group
/// authority: `PyExceptionInstance_Check` over the canonical native exception
/// layout, then `Exception` ancestry of the exact type. Instance `__class__` is never consulted.
///
/// # Safety
/// `object` must be null or a live native object.
pub unsafe fn native_exception_identity(object: *mut PyObject) -> u8 {
    if !unsafe { native_exception_instance(object) } {
        return 0;
    }
    let exception_type = unsafe { (*object).ob_type };
    let mut identity = NATIVE_EXCEPTION_INSTANCE;
    if unsafe {
        crate::api::typeobj::PyType_IsSubtype(
            exception_type,
            &raw mut crate::abi_types::PyExc_Exception,
        )
    } != 0
    {
        identity |= NATIVE_EXCEPTION_SUBCLASS;
    }
    identity
}

/// Native `BaseExceptionGroup.__new__`. The runtime group authority admits the
/// arguments for the exact native type (`RuntimeHooks::exception_group_admit`);
/// this allocator retains the physical C layout and its typed-field owners.
unsafe fn new_native_exception_group(
    subtype: *mut PyTypeObject,
    args: *mut PyObject,
) -> *mut PyObject {
    let exception_group =
        (&raw mut crate::abi_types::PYEXC_EXCEPTION_GROUP_INTERNAL).cast::<PyTypeObject>();
    let request = if std::ptr::eq(subtype, &raw mut crate::abi_types::PyExc_BaseExceptionGroup) {
        crate::hooks::ExceptionGroupRequest::BaseExceptionGroup
    } else if std::ptr::eq(subtype, exception_group) {
        crate::hooks::ExceptionGroupRequest::ExceptionGroup
    } else if unsafe {
        crate::api::typeobj::PyType_IsSubtype(subtype, &raw mut crate::abi_types::PyExc_Exception)
    } != 0
    {
        crate::hooks::ExceptionGroupRequest::ExceptionSubclass
    } else {
        crate::hooks::ExceptionGroupRequest::BaseExceptionSubclass
    };
    let Some(args_bits) = (unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(args) }) else {
        if !raised_error_pending() {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"BaseExceptionGroup arguments have no runtime value".as_ptr(),
                )
            };
        }
        return ptr::null_mut();
    };
    let hooks = crate::hooks::hooks_or_stubs();
    let mut message_bits = 0u64;
    let mut exceptions_bits = 0u64;
    let status = unsafe {
        (hooks.exception_group_admit)(
            request as u32,
            (*subtype).tp_name,
            args_bits,
            &raw mut message_bits,
            &raw mut exceptions_bits,
        )
    };
    if status < 0 {
        with_preserved_error(|| unsafe { (hooks.dec_ref)(args_bits) });
        if !raised_error_pending() {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"BaseExceptionGroup admission failed without an exception".as_ptr(),
                )
            };
        }
        return ptr::null_mut();
    }
    unsafe { (hooks.dec_ref)(args_bits) };
    // Each conversion consumes its runtime owner on every path.
    let message = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(message_bits) };
    let exceptions = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(exceptions_bits) };
    if message.is_null() || exceptions.is_null() {
        unsafe { release_preserving_error(&[message, exceptions]) };
        if !raised_error_pending() {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"BaseExceptionGroup fields have no C projection".as_ptr(),
                )
            };
        }
        return ptr::null_mut();
    }
    let selected = if status == 1 {
        exception_group
    } else {
        subtype
    };
    let instance = unsafe { allocate_native_exception(selected, args) };
    if instance.is_null() {
        unsafe { release_preserving_error(&[message, exceptions]) };
        return ptr::null_mut();
    }
    let layout = molt_lang_obj_model::ExceptionLayoutKind::Group;
    unsafe {
        set_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::GroupMessage,
            message,
        );
        set_native_typed_field(
            instance,
            layout,
            molt_lang_obj_model::ExceptionTypedField::GroupExceptions,
            exceptions,
        );
        crate::api::refcount::Py_DECREF(message);
        crate::api::refcount::Py_DECREF(exceptions);
    }
    instance.cast::<PyObject>()
}

/// Allocate a concrete native `PyBaseExceptionObject` for canonical `PyExc_*`
/// types before (or without) runtime-handle binding. This is also inherited by
/// honest C subtypes through `PyType_Ready`, so `type_call` has one physical
/// exception construction authority instead of recursively reporting a missing
/// `tp_new` slot through `PyErr_SetString`.
pub unsafe extern "C" fn molt_native_exception_new(
    subtype: *mut PyTypeObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    if subtype.is_null() {
        unsafe {
            replace_current_error(None);
            install_normalization_failure();
        }
        return ptr::null_mut();
    }
    let mut args = if args.is_null() {
        let empty = unsafe { crate::api::sequences::PyTuple_New(0) };
        if empty.is_null() {
            return ptr::null_mut();
        }
        empty
    } else {
        unsafe { crate::api::object::Py_NewRef(args) }
    };
    let Some(root) = (unsafe { crate::abi_types::exception_layout_root_for_type(subtype) }) else {
        unsafe { crate::api::refcount::Py_DECREF(args) };
        return ptr::null_mut();
    };
    let result = if root == molt_lang_obj_model::ExceptionLayoutRoot::BaseExceptionGroup {
        unsafe { new_native_exception_group(subtype, args) }
    } else if root == molt_lang_obj_model::ExceptionLayoutRoot::OSError
        && !unsafe { oserror_use_init(subtype) }
    {
        args = unsafe { normalize_oserror_platform_args(args) };
        if args.is_null() {
            return ptr::null_mut();
        }
        if unsafe {
            native_exception_keywords(molt_lang_obj_model::ExceptionLayoutKind::OSError, kwds)
        }
        .is_none()
        {
            ptr::null_mut()
        } else {
            let mut selected = subtype;
            let nargs = unsafe { crate::api::sequences::PyTuple_Size(args) };
            if std::ptr::eq(subtype, &raw mut crate::abi_types::PyExc_OSError)
                && (2..=5).contains(&nargs)
            {
                let errno_obj = unsafe { crate::api::sequences::PyTuple_GetItem(args, 0) };
                if unsafe { crate::api::numbers::PyLong_Check(errno_obj) } != 0 {
                    let errno = unsafe { crate::api::numbers::PyLong_AsLong(errno_obj) };
                    if errno == -1 && !unsafe { PyErr_Occurred() }.is_null() {
                        unsafe { crate::api::refcount::Py_DECREF(args) };
                        return ptr::null_mut();
                    }
                    selected = unsafe { oserror_subtype_for_errno(errno as c_int) };
                }
            }
            let instance = unsafe { allocate_native_exception(selected, args) };
            if instance.is_null() || unsafe { initialize_oserror_fields(instance, args) } < 0 {
                unsafe { crate::api::refcount::Py_XDECREF(instance.cast::<PyObject>()) };
                ptr::null_mut()
            } else {
                instance.cast::<PyObject>()
            }
        }
    } else if root == molt_lang_obj_model::ExceptionLayoutRoot::OSError {
        // A subclass with a custom tp_init receives an empty base allocation;
        // its init may explicitly delegate back to OSError initialization.
        unsafe { allocate_native_exception(subtype, ptr::null_mut()) }.cast::<PyObject>()
    } else {
        unsafe { allocate_native_exception(subtype, args) }.cast::<PyObject>()
    };
    unsafe { crate::api::refcount::Py_DECREF(args) };
    result
}

/// Release the exact field/type edges owned by [`molt_native_exception_new`].
pub unsafe extern "C" fn molt_native_exception_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let subtype = unsafe { (*op).ob_type };
    if subtype.is_null()
        || unsafe { crate::abi_types::exception_layout_for_type(subtype) }.is_none()
    {
        return;
    }
    with_preserved_error(|| unsafe {
        let Some(deallocation) = crate::api::typeobj::NativeDeallocation::storage(op) else {
            return;
        };
        molt_native_exception_clear(op);
        deallocation.finish();
    });
}

/// Validate an owned native callback result before publishing it across either
/// ABI boundary. A success with an exception is a contract violation; its
/// result is released and the original exception becomes SystemError cause and context.
pub(crate) unsafe fn check_native_result(result: *mut PyObject, operation: &str) -> *mut PyObject {
    let pending = raised_error_pending();
    if result.is_null() {
        if !pending {
            crate::capi_trace::record_silent_failure(operation, None);
            unsafe {
                replace_current_with_system_error(&format!(
                    "{operation} returned NULL without an exception"
                ))
            };
        }
        return result;
    }
    if pending {
        unsafe {
            release_preserving_error(&[result]);
            replace_current_with_system_error(&format!(
                "{operation} returned a result with an exception set"
            ));
        }
        return ptr::null_mut();
    }
    result
}

/// Status callbacks use the same two-channel contract as object results.
/// Returns 0 on success and -1 on error. Inquiry callers must retain their
/// original successful value after validation instead of returning this status.
pub(crate) unsafe fn check_native_status(status: c_int, operation: &str) -> c_int {
    let pending = raised_error_pending();
    if status < 0 {
        if !pending {
            crate::capi_trace::record_silent_failure(operation, None);
            unsafe {
                replace_current_with_system_error(&format!(
                    "{operation} failed without an exception"
                ))
            };
        }
        return -1;
    }
    if pending {
        unsafe {
            replace_current_with_system_error(&format!(
                "{operation} succeeded with an exception set"
            ))
        };
        return -1;
    }
    0
}

/// Replace an already-pending C error with a SystemError while retaining the
/// exact original exception instance as cause and context. Native-call result
/// validators use this for the CPython "success result with error set" state.
pub(crate) unsafe fn replace_current_with_system_error(message: &str) {
    // Only a malformed successful callback needs a new diagnostic. A runtime
    // error must first supply its exact instance for cause/context; if that
    // projection fails, retain the failure it leaves instead of fabricating a
    // C SystemError that erases the runtime channel.
    if current_exc_type_ptr().is_none()
        && raised_error_pending()
        && !transfer_runtime_pending_to_current()
        && raised_error_pending()
    {
        return;
    }
    let original = take_current_error();
    let c_message = std::ffi::CString::new(message).unwrap_or_else(|_| {
        std::ffi::CString::new("native call contract violation")
            .expect("static message contains no nul")
    });
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            c_message.as_ptr(),
        )
    };
    let Some(replacement) = take_current_error() else {
        with_preserved_error(|| drop(original));
        if !raised_error_pending() {
            unsafe { install_normalization_failure() };
        }
        return;
    };
    if let Some(original) = original {
        if !replacement.value.is_null()
            && !original.value.is_null()
            && !std::ptr::eq(replacement.value, original.value)
        {
            unsafe {
                crate::api::refcount::Py_INCREF(original.value);
                PyException_SetContext(replacement.value, original.value);
                crate::api::refcount::Py_INCREF(original.value);
                PyException_SetCause(replacement.value, original.value);
            }
            // The generic field authority should accept both normalized
            // instances. If it cannot, retain the SystemError rather than let
            // the attachment failure replace the boundary diagnosis.
            if current_exc_type_ptr().is_some() {
                replace_current_error(None);
            }
        }
        with_preserved_error(|| drop(original));
    }
    replace_current_error(Some(replacement));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetNone(exc_type: *mut PyObject) {
    unsafe { PyErr_SetObject(exc_type, ptr::null_mut()) };
}

/// CPython `PyErr_Occurred` (Python/errors.c): returns the pending exception's
/// actual TYPE (borrowed) or NULL. Consumers do identity/subtype tests on the
/// result (`PyErr_Occurred() == PyExc_StopIteration`,
/// `GivenExceptionMatches(PyErr_Occurred(), X)`), so the pre-fix `&Py_None`
/// sentinel mis-decided every such probe. Normalization stores the exact class
/// of the canonical exception instance (including an accepted subtype or an
/// OSError-selected subtype); there is no non-type fallback pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Occurred() -> *mut PyObject {
    if let Some(exc_type) = current_exc_type_ptr() {
        return exc_type;
    }
    match unsafe { (crate::hooks::hooks_or_stubs().pending_exception_class)() } {
        crate::hooks::PendingExceptionClass::None => ptr::null_mut(),
        crate::hooks::PendingExceptionClass::EmergencyMemoryError => {
            (&raw mut crate::abi_types::PyExc_MemoryError).cast()
        }
        crate::hooks::PendingExceptionClass::Class(bits) => {
            // Builtins use their canonical static bindings. A heap class may
            // need its Type view, but observing that class must never consume
            // the raised instance or materialize its lazy traceback.
            with_preserved_error(|| unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) })
        }
        crate::hooks::PendingExceptionClass::NativeClass(class) => class.cast(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Clear() {
    // Raw ABI cleanup is valid before initialization and after shutdown, but
    // those phases have no live runtime whose managed pointers may be
    // decref'd. Shutdown proves the retained-state count is zero before it
    // frees RuntimeState; preserve that proof by never opening thread-state
    // TLS on the runtime-absent path.
    if !crate::api::object::runtime_is_initialized() {
        return;
    }
    replace_current_error(None);
}

/// Counted Python-text messages never cross a C-string/UTF-8 codec boundary.
pub(crate) unsafe fn set_python_error_bytes(exc: *mut PyObject, message: &[u8]) {
    let value = unsafe { crate::api::strings::unicode_from_python_bytes(message) };
    if value.is_null() {
        return;
    }
    unsafe {
        PyErr_SetObject(exc, value);
        release_preserving_error(&[value]);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Print() {
    unsafe { PyErr_PrintEx(1) };
}

unsafe fn print_owned_error(state: OwnedCError) {
    let rendered = if state.value.is_null() {
        ptr::null_mut()
    } else {
        unsafe { crate::api::typeobj::PyObject_Str(state.value) }
    };
    if rendered.is_null() {
        // Display failures must not replace the exception being printed with a
        // new pending exception. PyErr_Print/PyErr_PrintEx consume the error
        // indicator even when bootstrap mode cannot allocate its text.
        replace_current_error(None);
        let type_name =
            crate::abi_types::exc_singleton_name(state.exc_type).unwrap_or("<exception>");
        eprintln!("[molt-cpython-abi] PyErr_Print: {type_name}");
        return;
    }
    let encoded = unsafe {
        crate::api::strings::PyUnicode_AsEncodedString(
            rendered,
            c"utf-8".as_ptr(),
            c"backslashreplace".as_ptr(),
        )
    };
    if !encoded.is_null() {
        let mut data = ptr::null_mut();
        let mut length = 0;
        if unsafe { crate::api::strings::PyBytes_AsStringAndSize(encoded, &mut data, &mut length) }
            == 0
        {
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) };
            eprintln!(
                "[molt-cpython-abi] PyErr_Print: {}",
                String::from_utf8_lossy(bytes)
            );
        }
    } else {
        let type_name =
            crate::abi_types::exc_singleton_name(state.exc_type).unwrap_or("<exception>");
        eprintln!("[molt-cpython-abi] PyErr_Print: {type_name}");
    }
    unsafe { release_preserving_error(&[encoded, rendered]) };
    replace_current_error(None);
}

unsafe fn publish_sys_last_error(state: &OwnedCError) {
    let hooks = crate::hooks::hooks_or_stubs();
    let had_runtime_pending = unsafe { (hooks.exception_pending)() } != 0;
    let sys_bits = unsafe { (hooks.import_module)(b"sys".as_ptr(), 3) };
    if sys_bits == 0 {
        clear_new_runtime_pending_error(had_runtime_pending);
        replace_current_error(None);
        return;
    }
    for (name, value) in [
        (b"last_type".as_slice(), state.exc_type),
        (b"last_value".as_slice(), state.value),
        (b"last_traceback".as_slice(), state.traceback),
        (b"last_exc".as_slice(), state.value),
    ] {
        let (value_bits, owned) = if value.is_null() {
            (MoltObject::none().bits(), false)
        } else {
            match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(value) } {
                Some(bits) => (bits, true),
                None => break,
            }
        };
        let status = unsafe {
            let status = (hooks.module_set_attr)(sys_bits, name.as_ptr(), name.len(), value_bits);
            if owned {
                (hooks.dec_ref)(value_bits);
            }
            status
        };
        if status != 0 {
            break;
        }
    }
    unsafe { (hooks.dec_ref)(sys_bits) };
    clear_new_runtime_pending_error(had_runtime_pending);
    replace_current_error(None);
}

/// Print and clear the exact pending exception; when requested, publish the
/// CPython 3.12 `sys.last_exc` and legacy `sys.last_*` views first (the ABI
/// target is fixed at 3.12).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_PrintEx(set_sys_last_vars: c_int) {
    let Some(state) = take_raised_error() else {
        return;
    };
    with_preserved_error(move || {
        if set_sys_last_vars != 0 {
            unsafe { publish_sys_last_error(&state) };
        }
        unsafe { print_owned_error(state) };
    });
}

unsafe extern "C" {
    /// C-runtime `errno` accessors from the shim (`pyarg_variadic.c`) — the C
    /// runtime is the only portable authority for `errno` (on Windows,
    /// `std::io::Error::last_os_error()` reads `GetLastError()`, which is a
    /// DIFFERENT channel from the C `errno` an extension just set).
    fn molt_capi_errno() -> c_int;
    fn molt_capi_strerror(errnum: c_int) -> *const c_char;
}

/// Build CPython's structured OSError constructor tuple. The runtime's class
/// call remains the one construction authority; this layer only preserves the
/// `(errno, strerror[, filename[, 0, filename2]])` argument shape.
unsafe fn set_from_errno_with_filename_objects(
    exc_type: *mut PyObject,
    filename: *mut PyObject,
    filename2: *mut PyObject,
) -> *mut PyObject {
    let errnum = unsafe { molt_capi_errno() };
    let detail = unsafe { molt_capi_strerror(errnum) };
    let detail = if detail.is_null() {
        c"operating system error".as_ptr()
    } else {
        detail
    };
    let argc = if filename2.is_null() {
        if filename.is_null() { 2 } else { 3 }
    } else {
        5
    };
    let args = unsafe { crate::api::sequences::PyTuple_New(argc) };
    if args.is_null() {
        return ptr::null_mut();
    }
    let errno_obj = unsafe { crate::api::numbers::PyLong_FromLong(errnum as c_long) };
    let strerror_obj = unsafe { crate::api::strings::PyUnicode_FromString(detail) };
    if errno_obj.is_null() || strerror_obj.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(errno_obj);
            crate::api::refcount::Py_XDECREF(strerror_obj);
            crate::api::refcount::Py_DECREF(args);
        }
        return ptr::null_mut();
    }
    if unsafe { crate::api::sequences::PyTuple_SetItem(args, 0, errno_obj) } != 0
        || unsafe { crate::api::sequences::PyTuple_SetItem(args, 1, strerror_obj) } != 0
    {
        unsafe { crate::api::refcount::Py_DECREF(args) };
        return ptr::null_mut();
    }
    if !filename.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(filename) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 2, filename) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
    }
    if !filename2.is_null() {
        let winerror = unsafe { crate::api::numbers::PyLong_FromLong(0) };
        if winerror.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 3, winerror) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_INCREF(filename2) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 4, filename2) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
    }
    let exc_type = if exc_type.is_null() {
        (&raw mut crate::abi_types::PyExc_OSError).cast::<crate::abi_types::PyObject>()
    } else {
        exc_type
    };
    unsafe {
        PyErr_SetObject(exc_type, args);
        crate::api::refcount::Py_DECREF(args);
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrno(exc_type: *mut PyObject) -> *mut PyObject {
    unsafe { set_from_errno_with_filename_objects(exc_type, ptr::null_mut(), ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrnoWithFilenameObject(
    exc_type: *mut PyObject,
    filename: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_from_errno_with_filename_objects(exc_type, filename, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrnoWithFilenameObjects(
    exc_type: *mut PyObject,
    filename: *mut PyObject,
    filename2: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_from_errno_with_filename_objects(exc_type, filename, filename2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrnoWithFilename(
    exc_type: *mut PyObject,
    filename: *const c_char,
) -> *mut PyObject {
    if filename.is_null() {
        return unsafe { PyErr_SetFromErrno(exc_type) };
    }
    let filename_obj = unsafe { crate::api::strings::PyUnicode_FromString(filename) };
    if filename_obj.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyErr_SetFromErrnoWithFilenameObject(exc_type, filename_obj) };
    unsafe { crate::api::refcount::Py_DECREF(filename_obj) };
    result
}

// ─── Additional error API ─────────────────────────────────────────────────

/// `PyErr_SetObject(type, value)` — set the current exception (Python/errors.c).
///
/// CPython 3.12 constructs or accepts the canonical exception instance
/// immediately. `value == NULL` is a zero-argument construction; a non-instance
/// value is the exact single positional argument. A matching instance is kept
/// by identity, and no payload is converted to text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetObject(exc_type: *mut PyObject, value: *mut PyObject) {
    unsafe {
        crate::api::refcount::Py_XINCREF(exc_type);
        crate::api::refcount::Py_XINCREF(value);
    }
    unsafe {
        normalize_and_replace(
            OwnedCError {
                exc_type,
                value,
                traceback: ptr::null_mut(),
            },
            ExceptionIngress::SetObject,
        )
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NoMemory() -> *mut PyObject {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_MemoryError).cast::<crate::abi_types::PyObject>(),
            c"out of memory".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_BadArgument() -> c_int {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            c"bad argument type for built-in operation".as_ptr(),
        );
    }
    0
}

/// CPython `PyErr_BadInternalCall` (Python/errors.c): sets **SystemError**
/// ("bad argument to internal function") — the pre-fix RuntimeError broke any
/// caller's `ExceptionMatches(PyExc_SystemError)` probe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_BadInternalCall() {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            c"bad argument to internal function".as_ptr(),
        );
    }
}

/// CPython private `_PyErr_BadInternalCall(filename, lineno)` (Python/errors.c):
/// the located form of [`PyErr_BadInternalCall`] — sets **SystemError**
/// `"<file>:<line>: bad argument to internal function"`. When a C extension is
/// built with `assert`-style internal-call checks, its `PyErr_BadInternalCall()`
/// macro expands to this private form carrying `__FILE__`/`__LINE__`; numpy
/// links it. A NULL filename or an interior-NUL message degrades to the
/// no-location wrapper rather than dropping the error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyErr_BadInternalCall(filename: *const c_char, lineno: c_int) {
    if filename.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return;
    }
    let file = unsafe { CStr::from_ptr(filename) }.to_string_lossy();
    let message = format!("{file}:{lineno}: bad argument to internal function");
    match std::ffi::CString::new(message) {
        Ok(cmessage) => unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                cmessage.as_ptr(),
            );
        },
        Err(_) => unsafe { PyErr_BadInternalCall() },
    }
}

/// Transfer the exact owned `(type, value, traceback)` error indicator and
/// clear it without normalization or payload conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Fetch(
    p_type: *mut *mut PyObject,
    p_value: *mut *mut PyObject,
    p_tb: *mut *mut PyObject,
) {
    let state = take_raised_error().map(std::mem::ManuallyDrop::new);
    let (type_ptr, value_ptr, traceback_ptr) = state
        .as_ref()
        .map(|state| (state.exc_type, state.value, state.traceback))
        .unwrap_or((ptr::null_mut(), ptr::null_mut(), ptr::null_mut()));
    with_preserved_error(|| {
        if !p_type.is_null() {
            unsafe { *p_type = type_ptr };
        } else if !type_ptr.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(type_ptr) };
        }
        if !p_value.is_null() {
            unsafe { *p_value = value_ptr };
        } else if !value_ptr.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(value_ptr) };
        }
        if !p_tb.is_null() {
            unsafe { *p_tb = traceback_ptr };
        } else if !traceback_ptr.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(traceback_ptr) };
        }
    });
}

/// Transfer the normalized raised instance and clear the propagating indicator.
/// The instance already owns its traceback; retiring the triple's additional
/// references must neither reconstruct the exception nor publish cleanup errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GetRaisedException() -> *mut PyObject {
    let Some(mut error) = take_raised_error() else {
        return ptr::null_mut();
    };
    let value = std::mem::replace(&mut error.value, ptr::null_mut());
    with_preserved_error(|| drop(error));
    value
}

/// Steal an already constructed exception instance, or clear on NULL. Class
/// and traceback projections use the same managed/foreign authorities as the
/// other C APIs. Unlike Restore, this API never calls an exception constructor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetRaisedException(exc: *mut PyObject) {
    let previous = take_current_error();
    clear_all_pending_errors();
    if !exc.is_null() {
        let mut next = OwnedCError {
            exc_type: unsafe { crate::api::typeobj::PyObject_Type(exc) },
            value: exc,
            traceback: ptr::null_mut(),
        };
        if !next.exc_type.is_null() {
            next.traceback = unsafe { PyException_GetTraceback(exc) };
        }
        if !next.exc_type.is_null() && !raised_error_pending() {
            replace_current_error(Some(next));
        } else {
            with_preserved_error(|| drop(next));
        }
    }
    with_preserved_error(|| drop(previous));
}

/// Return the runtime's active handled exception (`sys.exception()`) as a new
/// reference, distinct from the propagating error indicator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GetHandledException() -> *mut PyObject {
    let result = unsafe { (crate::hooks::hooks_or_stubs().handled_exception_get)() };
    match result.decode() {
        crate::hooks::DecodedHandleResult::Missing => ptr::null_mut(),
        crate::hooks::DecodedHandleResult::Error => {
            let _ = transfer_runtime_pending_to_current();
            ptr::null_mut()
        }
        crate::hooks::DecodedHandleResult::Ok(bits) => unsafe {
            GLOBAL_BRIDGE.owned_handle_to_pyobj(bits)
        },
    }
}

/// Replace the active handled exception. The public CPython API borrows `exc`;
/// the hook consumes the independent owned runtime handle minted here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetHandledException(exc: *mut PyObject) {
    let owned_bits = if exc.is_null() {
        Some(0)
    } else {
        unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(exc) }
    };
    let status = match owned_bits {
        Some(bits) => unsafe { (crate::hooks::hooks_or_stubs().handled_exception_set)(bits) },
        None => -1,
    };
    if status != 0 && !raised_error_pending() {
        crate::capi_trace::record_silent_failure(
            "PyErr_SetHandledException",
            Some("runtime handled-exception authority unavailable"),
        );
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"PyErr_SetHandledException: runtime handled-exception authority unavailable"
                    .as_ptr(),
            )
        };
    }
}

/// Read the runtime's active handled exception (`sys.exc_info()`). Every
/// non-NULL output is a new reference, matching CPython 3.12.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GetExcInfo(
    p_type: *mut *mut PyObject,
    p_value: *mut *mut PyObject,
    p_tb: *mut *mut PyObject,
) {
    unsafe {
        if !p_type.is_null() {
            *p_type = ptr::null_mut();
        }
        if !p_value.is_null() {
            *p_value = ptr::null_mut();
        }
        if !p_tb.is_null() {
            *p_tb = ptr::null_mut();
        }
    }
    let value = unsafe { PyErr_GetHandledException() };
    if value.is_null() {
        return;
    }
    let exc_type = unsafe {
        let tp = (*value).ob_type;
        if tp.is_null() {
            ptr::null_mut()
        } else {
            crate::api::object::Py_NewRef(tp.cast::<PyObject>())
        }
    };
    let traceback = unsafe { PyException_GetTraceback(value) };
    unsafe {
        if p_type.is_null() {
            crate::api::refcount::Py_XDECREF(exc_type);
        } else {
            *p_type = exc_type;
        }
        if p_value.is_null() {
            crate::api::refcount::Py_DECREF(value);
        } else {
            *p_value = value;
        }
        if p_tb.is_null() {
            crate::api::refcount::Py_XDECREF(traceback);
        } else {
            *p_tb = traceback;
        }
    }
}

/// Replace the runtime's active handled exception. CPython 3.12 ignores the
/// legacy type/traceback values, derives both from `value`, and still steals all
/// three incoming references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetExcInfo(
    exc_type: *mut PyObject,
    value: *mut PyObject,
    traceback: *mut PyObject,
) {
    unsafe { PyErr_SetHandledException(value) };
    unsafe {
        crate::api::refcount::Py_XDECREF(exc_type);
        crate::api::refcount::Py_XDECREF(value);
        crate::api::refcount::Py_XDECREF(traceback);
    }
}

/// Take ownership of an ingress triple, normalize it to the exact exception
/// instance, attach/validate its traceback, and install that canonical state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Restore(
    exc_type: *mut PyObject,
    value: *mut PyObject,
    tb: *mut PyObject,
) {
    if exc_type.is_null() {
        drop(OwnedCError {
            exc_type,
            value,
            traceback: tb,
        });
        replace_current_error(None);
    } else {
        unsafe {
            normalize_and_replace(
                OwnedCError {
                    exc_type,
                    value,
                    traceback: tb,
                },
                ExceptionIngress::Restore,
            )
        };
    }
}

/// Normalize an owned caller triple in place without any text projection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NormalizeException(
    exc: *mut *mut PyObject,
    val: *mut *mut PyObject,
    tb: *mut *mut PyObject,
) {
    if exc.is_null() || val.is_null() {
        return;
    }
    let exc_type = unsafe { *exc };
    if exc_type.is_null() {
        return;
    }
    let value = unsafe { std::mem::replace(&mut *val, ptr::null_mut()) };
    unsafe { *exc = ptr::null_mut() };
    let traceback = if tb.is_null() {
        ptr::null_mut()
    } else {
        unsafe { std::mem::replace(&mut *tb, ptr::null_mut()) }
    };
    let original_traceback = unsafe { crate::api::object::Py_XNewRef(traceback) };
    let normalized = unsafe {
        normalize_owned_error(
            OwnedCError {
                exc_type,
                value,
                traceback,
            },
            ExceptionIngress::Normalize,
        )
    };
    let normalized = if let Some(normalized) = normalized {
        unsafe { release_preserving_error(&[original_traceback]) };
        normalized
    } else {
        // Constructors report through the same normalized ingress. Transfer
        // their replacement error to the caller's triple, as CPython does,
        // rather than losing the caller's owners and returning three NULLs.
        let replacement = take_raised_error().or_else(|| {
            unsafe { install_normalization_failure() };
            take_current_error()
        });
        let Some(mut replacement) = replacement else {
            unsafe { release_preserving_error(&[original_traceback]) };
            return;
        };
        if replacement.traceback.is_null() {
            replacement.traceback = original_traceback;
        } else {
            unsafe { release_preserving_error(&[original_traceback]) };
        }
        replacement
    };
    let normalized = std::mem::ManuallyDrop::new(normalized);
    unsafe {
        *exc = normalized.exc_type;
        *val = normalized.value;
        if tb.is_null() {
            release_preserving_error(&[normalized.traceback]);
        } else {
            *tb = normalized.traceback;
        }
    }
}

/// Prepare a diagnostic string inside the normalization scope. A partial
/// bootstrap table needs both allocation and class identity to publish text;
/// select that capability before calling it. Failures of an installed producer
/// remain errors, including failures during the owned handle's projection.
fn allocate_exception_message(text: &str) -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    if std::ptr::fn_addr_eq(h.alloc_str, crate::hooks::STUB_HOOKS.alloc_str)
        || std::ptr::fn_addr_eq(
            h.runtime_class_borrowed,
            crate::hooks::STUB_HOOKS.runtime_class_borrowed,
        )
    {
        return ptr::null_mut();
    }
    let bits = unsafe { (h.alloc_str)(text.as_ptr(), text.len()) };
    if raised_error_pending() {
        with_preserved_error(|| {
            if bits != 0 {
                unsafe { (h.dec_ref)(bits) };
            }
        });
        return ptr::null_mut();
    }
    if bits == 0 {
        // Allocation failed with no selected callback error. Reporting this
        // must not allocate another diagnostic through the failing producer.
        unsafe { PyErr_SetNone((&raw mut crate::abi_types::PyExc_MemoryError).cast()) };
        return ptr::null_mut();
    }
    let value = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    if value.is_null() {
        return value;
    }
    // Unicode storage publication alone does not resolve the runtime class.
    // Complete that producer contract while text preparation owns the guard:
    // deferring it until normalization's tuple check misclassifies a broken
    // text provider as recursively failing exception construction.
    let is_text = unsafe {
        crate::bridge::is_semantic_instance_of(value, &raw mut crate::abi_types::PyUnicode_Type)
    };
    if !is_text || raised_error_pending() {
        unsafe { release_preserving_error(&[value]) };
        if !raised_error_pending() {
            // A malformed producer result has no usable diagnostic payload.
            // Report that concrete contract violation through normal native
            // construction; recursion exhaustion retains its fatal policy.
            unsafe { PyErr_SetNone((&raw mut crate::abi_types::PyExc_SystemError).cast()) };
        }
        return ptr::null_mut();
    }
    value
}

/// `PyErr_ExceptionMatches(exc)` — does the pending exception match `exc`?
/// CPython defines this as `PyErr_GivenExceptionMatches(PyErr_Occurred(), exc)`
/// (Python/errors.c); with `PyErr_Occurred` now returning the REAL pending
/// type, the delegation is exact — subclass walks (pending IndexError vs
/// `exc = LookupError`) and tuple candidates both match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_ExceptionMatches(exc: *mut PyObject) -> c_int {
    let given = unsafe { PyErr_Occurred() };
    unsafe { PyErr_GivenExceptionMatches(given, exc) }
}

/// Physical exception-class admission. `PyType_Check` accepts real metaclass
/// subtypes; neither it nor `PyType_IsSubtype` invokes Python identity hooks.
unsafe fn exception_class_pointer(value: *mut PyObject) -> bool {
    unsafe {
        crate::api::typeobj::PyType_Check(value) != 0
            && crate::api::typeobj::PyType_IsSubtype(
                value.cast::<PyTypeObject>(),
                &raw mut crate::abi_types::PyExc_BaseException,
            ) != 0
    }
}

/// Real exception-class admission shared by runtime handlers and C matching.
/// Bootstrap singleton shells already carry their canonical base edges; no
/// type readiness, Python attribute lookup, or second hierarchy is needed.
///
/// # Safety
/// `value` must be NULL or a live C object.
pub unsafe fn exception_class_check(value: *mut PyObject) -> bool {
    with_preserved_error(|| unsafe { exception_class_pointer(value) && !raised_error_pending() })
}

/// Borrow the actual class only after physical managed/native exception
/// admission. A native subclass is identified by its canonical layout and
/// actual ob_type, never by exact exported-singleton identity.
unsafe fn exception_instance_class(value: *mut PyObject) -> *mut PyTypeObject {
    if value.is_null() {
        return ptr::null_mut();
    }
    let class = if let Some(handle) = GLOBAL_BRIDGE.molt_handle_for_pyobj(value)
        && unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(handle.bits()) }
            == MoltTypeTag::Exception as u8
    {
        match unsafe { (crate::hooks::hooks_or_stubs().runtime_class_borrowed)(handle.bits()) }
            .decode()
        {
            crate::hooks::DecodedHandleResult::Ok(class_bits) => unsafe {
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class_bits)
            },
            crate::hooks::DecodedHandleResult::Missing
            | crate::hooks::DecodedHandleResult::Error => return ptr::null_mut(),
        }
    } else if unsafe { native_exception_instance(value) } {
        unsafe { (*value).ob_type.cast::<PyObject>() }
    } else {
        return ptr::null_mut();
    };
    if unsafe { exception_class_pointer(class) } {
        class.cast()
    } else {
        ptr::null_mut()
    }
}

unsafe fn given_exception_matches(given: *mut PyObject, exc: *mut PyObject) -> bool {
    if given.is_null() || exc.is_null() {
        return false;
    }
    // The tuple API is the single physical/runtime authority: its accessors
    // cover ABI-layout and managed tuples without holding a bridge lock across
    // element conversion. C matching, unlike Python handler validation, accepts
    // recursive tuples, as CPython's PyErr_GivenExceptionMatches does.
    if unsafe { crate::api::sequences::PyTuple_Check(exc) } != 0 {
        let len = unsafe { crate::api::sequences::PyTuple_GET_SIZE(exc) };
        for index in 0..len {
            let item = unsafe { crate::api::sequences::PyTuple_GET_ITEM(exc, index) };
            if item.is_null() || raised_error_pending() {
                return false;
            }
            let matches = unsafe { given_exception_matches(given, item) };
            if raised_error_pending() {
                return false;
            }
            if matches {
                return true;
            }
        }
        return false;
    }
    let instance_class = unsafe { exception_instance_class(given) };
    let given = if instance_class.is_null() {
        given
    } else {
        instance_class.cast()
    };
    if unsafe { exception_class_pointer(given) && exception_class_pointer(exc) } {
        return unsafe { crate::api::typeobj::PyType_IsSubtype(given.cast(), exc.cast()) != 0 };
    }
    // CPython retains pointer identity for non-exception inputs. Arbitrary
    // non-exception types do not acquire exception subclass matching.
    std::ptr::eq(given, exc)
}

/// CPython exception matching over real classes and instance types, including
/// native subclasses, metaclass subclasses, and recursive candidate tuples.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GivenExceptionMatches(
    given: *mut PyObject,
    exc: *mut PyObject,
) -> c_int {
    // Canonical bootstrap shells need no runtime projection or error-state
    // transaction. This also permits emergency matching before runtime class
    // roots exist, using the same physical subtype authority as heap types.
    if crate::abi_types::exc_singleton_name(given).is_some()
        && crate::abi_types::exc_singleton_name(exc).is_some()
    {
        return unsafe { crate::api::typeobj::PyType_IsSubtype(given.cast(), exc.cast()) };
    }
    with_preserved_error(|| unsafe {
        (given_exception_matches(given, exc) && !raised_error_pending()) as c_int
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetTraceback(exc: *mut PyObject, tb: *mut PyObject) -> c_int {
    if exc.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyException_SetTraceback: NULL exception".as_ptr(),
            )
        };
        return -1;
    }
    if let Some(exception) = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc) {
        let traceback_bits = if tb.is_null() {
            None
        } else {
            unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(tb) }
        };
        if !tb.is_null() && traceback_bits.is_none() {
            unsafe { set_exception_field_type_error(c"__traceback__ must be a traceback or None") };
            return -1;
        }
        let hooks = crate::hooks::hooks_or_stubs();
        let status = unsafe {
            (hooks.exception_set_field)(
                exception.bits(),
                crate::hooks::ExceptionField::Traceback as u32,
                traceback_bits.unwrap_or(0),
                c_int::from(!tb.is_null()),
            )
        };
        let published = status != 0 || GLOBAL_BRIDGE.refresh_exception_view(exception.bits());
        if let Some(bits) = traceback_bits {
            unsafe { (hooks.dec_ref)(bits) };
        }
        if status == 0 && published {
            return 0;
        }
        if !raised_error_pending() {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"__traceback__ must be a traceback or None".as_ptr(),
                )
            };
        }
        return -1;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyException_SetTraceback: expected an exception instance".as_ptr(),
            )
        };
        return -1;
    };
    let tb = if std::ptr::eq(tb, &raw mut crate::abi_types::Py_None) {
        ptr::null_mut()
    } else {
        tb
    };
    if !tb.is_null() {
        let class = unsafe { crate::bridge::semantic_type(tb) };
        if class.is_null() {
            return -1;
        }
        if class != &raw mut crate::abi_types::PyTraceBack_Type {
            unsafe { set_exception_field_type_error(c"__traceback__ must be a traceback or None") };
            return -1;
        }
    }
    unsafe {
        if !tb.is_null() {
            crate::api::refcount::Py_INCREF(tb);
        }
        let old = (*base).traceback;
        (*base).traceback = tb;
        if !old.is_null() {
            crate::api::refcount::Py_DECREF(old);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetTraceback(exc: *mut PyObject) -> *mut PyObject {
    if exc.is_null() {
        return ptr::null_mut();
    }
    if let Some(exception) = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc) {
        let result = unsafe {
            (crate::hooks::hooks_or_stubs().exception_get_field)(
                exception.bits(),
                crate::hooks::ExceptionField::Traceback as u32,
            )
        };
        return match result.decode() {
            crate::hooks::DecodedHandleResult::Missing => ptr::null_mut(),
            crate::hooks::DecodedHandleResult::Ok(bits) => unsafe {
                GLOBAL_BRIDGE.owned_handle_to_pyobj(bits)
            },
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetTraceback: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyException_GetTraceback: expected an exception instance".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let traceback = unsafe { (*base).traceback };
    unsafe { crate::api::refcount::Py_XINCREF(traceback) };
    traceback
}

fn foreign_exception_layout(exc: *mut PyObject) -> Option<*mut PyBaseExceptionObject> {
    if exc.is_null() || GLOBAL_BRIDGE.molt_handle_for_pyobj(exc).is_some() {
        return None;
    }
    let exception_type = unsafe { (*exc).ob_type };
    if exception_type.is_null()
        || unsafe { (*exception_type).tp_flags } & crate::abi_types::Py_TPFLAGS_BASE_EXC_SUBCLASS
            == 0
    {
        return None;
    }
    let layout = unsafe { crate::abi_types::exception_layout_for_type(exception_type) }?;
    if unsafe { (*exception_type).tp_basicsize }
        < crate::abi_types::exception_layout_basicsize(layout)
    {
        return None;
    }
    Some(exc.cast::<PyBaseExceptionObject>())
}

/// Admit genuine C exception storage without Python attribute lookup or
/// materialization. Managed views use the runtime's real class/layout check.
///
/// # Safety
/// `value` must be NULL or a live, owned C object.
pub unsafe fn native_exception_instance(value: *mut PyObject) -> bool {
    foreign_exception_layout(value).is_some()
}

/// Return `StopIteration.value` as a new reference without projecting a native
/// bootstrap exception through generic attribute lookup. Runtime-backed
/// exceptions retain their own attribute authority; native exceptions created
/// by [`molt_native_exception_new`] carry the constructor argument in `args`.
/// CPython's StopIteration constructor accepts zero or one positional value,
/// so those two physical forms are exact and allocation-free here.
pub(crate) unsafe fn stop_iteration_value(exc: *mut PyObject) -> *mut PyObject {
    if exc.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(exc).is_some() {
        return unsafe { crate::api::object::PyObject_GetAttrString(exc, c"value".as_ptr()) };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        return unsafe { crate::api::object::PyObject_GetAttrString(exc, c"value".as_ptr()) };
    };
    let args = unsafe { (*base).args };
    if args.is_null() {
        return unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) };
    }
    match unsafe { crate::api::sequences::PyTuple_Size(args) } {
        0 => unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) },
        1 => {
            let value = unsafe { crate::api::sequences::PyTuple_GetItem(args, 0) };
            unsafe { crate::api::object::Py_XNewRef(value) }
        }
        _ => unsafe { crate::api::object::PyObject_GetAttrString(exc, c"value".as_ptr()) },
    }
}

unsafe fn managed_exception_set_field(
    exc: *mut PyObject,
    field: crate::hooks::ExceptionField,
    value: *mut PyObject,
) -> Option<c_int> {
    let exception = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc)?;
    let c_none = value.is_null()
        || (!matches!(field, crate::hooks::ExceptionField::Args)
            && std::ptr::eq(value, &raw mut crate::abi_types::Py_None));
    let value_bits = if c_none {
        None
    } else {
        unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(value) }
    };
    if !c_none && value_bits.is_none() {
        return Some(-1);
    }
    let hooks = crate::hooks::hooks_or_stubs();
    let status = unsafe {
        (hooks.exception_set_field)(
            exception.bits(),
            field as u32,
            value_bits.unwrap_or(0),
            c_int::from(!c_none),
        )
    };
    let published = status != 0 || GLOBAL_BRIDGE.refresh_exception_view(exception.bits());
    if let Some(bits) = value_bits {
        unsafe { (hooks.dec_ref)(bits) };
    }
    if status != 0 {
        let _ = transfer_runtime_pending_to_current();
    }
    Some(if published { status } else { -1 })
}

unsafe fn managed_exception_get_field(
    exc: *mut PyObject,
    field: crate::hooks::ExceptionField,
) -> Option<Result<*mut PyObject, ()>> {
    let exception = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc)?;
    let result = unsafe {
        (crate::hooks::hooks_or_stubs().exception_get_field)(exception.bits(), field as u32)
    };
    Some(match result.decode() {
        crate::hooks::DecodedHandleResult::Missing => Ok(ptr::null_mut()),
        crate::hooks::DecodedHandleResult::Ok(bits) => {
            Ok(unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) })
        }
        crate::hooks::DecodedHandleResult::Error => {
            let _ = transfer_runtime_pending_to_current();
            Err(())
        }
    })
}

unsafe fn set_exception_field_type_error(message: &'static CStr) {
    if !raised_error_pending() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                message.as_ptr(),
            )
        };
    }
}

unsafe fn exception_instance_pointer(value: *mut PyObject) -> bool {
    with_preserved_error(|| unsafe { !exception_instance_class(value).is_null() })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetContext(exc: *mut PyObject, context: *mut PyObject) {
    if exc.is_null() {
        unsafe { crate::api::refcount::Py_XDECREF(context) };
        return;
    }
    if let Some(status) =
        unsafe { managed_exception_set_field(exc, crate::hooks::ExceptionField::Context, context) }
    {
        unsafe { crate::api::refcount::Py_XDECREF(context) };
        if status != 0 {
            unsafe {
                set_exception_field_type_error(c"exception context must be an exception or None")
            };
        }
        return;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            crate::api::refcount::Py_XDECREF(context);
            set_exception_field_type_error(
                c"PyException_SetContext: expected an exception instance",
            );
        }
        return;
    };
    let context = if std::ptr::eq(context, &raw mut crate::abi_types::Py_None) {
        unsafe { crate::api::refcount::Py_DECREF(context) };
        ptr::null_mut()
    } else {
        context
    };
    if !context.is_null() && !unsafe { exception_instance_pointer(context) } {
        unsafe {
            crate::api::refcount::Py_DECREF(context);
            set_exception_field_type_error(c"exception context must be an exception or None");
        }
        return;
    }
    unsafe {
        let old = (*base).context;
        if old == context {
            crate::api::refcount::Py_XDECREF(context);
        } else {
            (*base).context = context;
            crate::api::refcount::Py_XDECREF(old);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetContext(exc: *mut PyObject) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(exc, crate::hooks::ExceptionField::Context) }
    {
        return match result {
            Ok(value) => value,
            Err(()) => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetContext: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(
                c"PyException_GetContext: expected an exception instance",
            )
        };
        return ptr::null_mut();
    };
    let context = unsafe { (*base).context };
    unsafe { crate::api::refcount::Py_XINCREF(context) };
    context
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetCause(exc: *mut PyObject, cause: *mut PyObject) {
    if exc.is_null() {
        unsafe { crate::api::refcount::Py_XDECREF(cause) };
        return;
    }
    if let Some(status) =
        unsafe { managed_exception_set_field(exc, crate::hooks::ExceptionField::Cause, cause) }
    {
        unsafe { crate::api::refcount::Py_XDECREF(cause) };
        if status != 0 {
            unsafe {
                set_exception_field_type_error(c"exception cause must be an exception or None")
            };
        }
        return;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            crate::api::refcount::Py_XDECREF(cause);
            set_exception_field_type_error(c"PyException_SetCause: expected an exception instance");
        }
        return;
    };
    let cause = if std::ptr::eq(cause, &raw mut crate::abi_types::Py_None) {
        unsafe { crate::api::refcount::Py_DECREF(cause) };
        ptr::null_mut()
    } else {
        cause
    };
    if !cause.is_null() && !unsafe { exception_instance_pointer(cause) } {
        unsafe {
            crate::api::refcount::Py_DECREF(cause);
            set_exception_field_type_error(c"exception cause must be an exception or None");
        }
        return;
    }
    unsafe {
        let old = (*base).cause;
        if old == cause {
            crate::api::refcount::Py_XDECREF(cause);
        } else {
            (*base).cause = cause;
            crate::api::refcount::Py_XDECREF(old);
        }
        (*base).suppress_context = 1;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetCause(exc: *mut PyObject) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(exc, crate::hooks::ExceptionField::Cause) }
    {
        return match result {
            Ok(value) => value,
            Err(()) => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetCause: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(c"PyException_GetCause: expected an exception instance")
        };
        return ptr::null_mut();
    };
    let cause = unsafe { (*base).cause };
    unsafe { crate::api::refcount::Py_XINCREF(cause) };
    cause
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetArgs(exc: *mut PyObject, args: *mut PyObject) {
    if let Some(status) =
        unsafe { managed_exception_set_field(exc, crate::hooks::ExceptionField::Args, args) }
    {
        if status != 0 {
            unsafe { set_exception_field_type_error(c"exception args must be a tuple") };
        }
        return;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(c"PyException_SetArgs: expected an exception instance")
        };
        return;
    };
    if args.is_null() || unsafe { crate::api::sequences::PyTuple_Check(args) } == 0 {
        unsafe { set_exception_field_type_error(c"exception args must be a tuple") };
        return;
    }
    unsafe {
        crate::api::refcount::Py_INCREF(args);
        let old = (*base).args;
        (*base).args = args;
        crate::api::refcount::Py_XDECREF(old);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetArgs(exc: *mut PyObject) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(exc, crate::hooks::ExceptionField::Args) }
    {
        return match result {
            Ok(value) => value,
            Err(()) => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetArgs: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(c"PyException_GetArgs: expected an exception instance")
        };
        return ptr::null_mut();
    };
    let args = unsafe { (*base).args };
    unsafe { crate::api::refcount::Py_XINCREF(args) };
    args
}

/// Route warnings through the runtime `warnings.warn` callable so filters,
/// warning-as-error policy, category validation, and stacklevel share the same
/// authority as compiled Python.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WarnEx(
    category: *mut PyObject,
    message: *const c_char,
    stack_level: Py_ssize_t,
) -> c_int {
    let warnings = unsafe { crate::api::imports::PyImport_ImportModule(c"warnings".as_ptr()) };
    if warnings.is_null() {
        return -1;
    }
    let warn = unsafe { crate::api::object::PyObject_GetAttrString(warnings, c"warn".as_ptr()) };
    unsafe { crate::api::refcount::Py_DECREF(warnings) };
    if warn.is_null() {
        return -1;
    }
    let message = unsafe {
        crate::api::strings::PyUnicode_FromString(if message.is_null() {
            c"".as_ptr()
        } else {
            message
        })
    };
    let category = if category.is_null() {
        unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
    } else {
        unsafe { crate::api::object::Py_NewRef(category) }
    };
    let stack_level = unsafe { crate::api::numbers::PyLong_FromLongLong(stack_level as i64) };
    let args = unsafe { crate::api::sequences::PyTuple_New(3) };
    if message.is_null() || category.is_null() || stack_level.is_null() || args.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(message);
            crate::api::refcount::Py_XDECREF(category);
            crate::api::refcount::Py_XDECREF(stack_level);
            crate::api::refcount::Py_XDECREF(args);
            crate::api::refcount::Py_DECREF(warn);
        }
        return -1;
    }
    unsafe {
        let _ = crate::api::sequences::PyTuple_SetItem(args, 0, message);
        let _ = crate::api::sequences::PyTuple_SetItem(args, 1, category);
        let _ = crate::api::sequences::PyTuple_SetItem(args, 2, stack_level);
    }
    let result = unsafe { crate::api::object::PyObject_CallObject(warn, args) };
    unsafe {
        crate::api::refcount::Py_DECREF(args);
        crate::api::refcount::Py_DECREF(warn);
    }
    if result.is_null() {
        -1
    } else {
        unsafe { crate::api::refcount::Py_DECREF(result) };
        0
    }
}

/// `PyErr_WriteUnraisable(obj)` consumes the C-API error indicator and forwards
/// its typed payload plus owned runtime context to the runtime's canonical
/// unraisable transaction. This surface has CPython's null `err_msg` contract;
/// version-specific formatted messages are supplied by runtime call sites that
/// model `PyErr_FormatUnraisable` instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WriteUnraisable(obj: *mut PyObject) {
    unsafe { write_unraisable_impl(obj, None) };
}

unsafe fn write_unraisable_impl(obj: *mut PyObject, err_msg: Option<&[u8]>) {
    let Some(state) = take_raised_error() else {
        return;
    };
    // The unraisable boundary consumes errors from conversion, reporting and
    // owner retirement as well as the selected raised error. The same selection
    // authority as PyErr_Fetch gives C precedence over the runtime channel.
    // Callback boundaries detach their outer state before invoking this API.
    with_preserved_error(move || {
        let owned_bits = |ptr: *mut PyObject| {
            if ptr.is_null() {
                (MoltObject::none().bits(), false)
            } else {
                match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(ptr) } {
                    Some(bits) => (bits, true),
                    None => (MoltObject::none().bits(), false),
                }
            }
        };
        let (type_bits, owns_type) = owned_bits(state.exc_type);
        let (value_bits, owns_value) = owned_bits(state.value);
        let (traceback_bits, owns_traceback) = owned_bits(state.traceback);
        let (context_bits, owns_context) = if obj.is_null() {
            (MoltObject::none().bits(), false)
        } else {
            match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(obj) } {
                Some(bits) => (bits, true),
                None => (MoltObject::none().bits(), false),
            }
        };
        let hooks = crate::hooks::hooks_or_stubs();
        unsafe {
            let (err_ptr, err_len, has_err) = err_msg
                .map(|text| (text.as_ptr(), text.len(), 1))
                .unwrap_or((std::ptr::null(), 0, 0));
            (hooks.report_unraisable)(
                context_bits,
                type_bits,
                value_bits,
                traceback_bits,
                std::ptr::null(),
                0,
                err_ptr,
                err_len,
                has_err,
            )
        };
        for (bits, owned) in [
            (context_bits, owns_context),
            (type_bits, owns_type),
            (value_bits, owns_value),
            (traceback_bits, owns_traceback),
        ] {
            if owned {
                unsafe { (hooks.dec_ref)(bits) };
            }
        }
        drop(state);
    });
}

/// Variadic C shim entry after CPython-style `%R`/`%T` formatting.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_err_format_unraisable(message: *const u8, len: usize) {
    let formatted = if message.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(message, len) }
    };
    unsafe { write_unraisable_impl(std::ptr::null_mut(), Some(formatted)) };
}

/// SIGINT is 2 on every supported target (POSIX, the Windows CRT, WASI).
const SIGINT_NUMBER: c_int = 2;

/// CPython `PyErr_CheckSignals`: run pending Python signal handlers when called
/// on the registered main thread. The runtime signal authority owns recording
/// and dispatch. A handler's exception becomes the C error indicator and the
/// call returns -1. Other threads always get 0, as in CPython.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_CheckSignals() -> c_int {
    if unsafe { (crate::hooks::hooks_or_stubs().check_signals)() } == 0 {
        return 0;
    }
    if !raised_error_pending() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"signal handler failed without setting an exception".as_ptr(),
            )
        };
    }
    -1
}

/// CPython `PyErr_SetInterruptEx`: simulate the arrival of `signum` without an
/// OS signal. A signal not handled by Python (SIG_DFL or SIG_IGN) is ignored.
/// Returns -1 only for an out-of-range number. Async-signal-safe: callable from
/// a C signal handler or any thread without an attached thread state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetInterruptEx(signum: c_int) -> c_int {
    unsafe { (crate::hooks::hooks_or_stubs().set_interrupt)(signum) }
}

/// CPython `PyErr_SetInterrupt`: `PyErr_SetInterruptEx(SIGINT)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetInterrupt() {
    let _ = unsafe { PyErr_SetInterruptEx(SIGINT_NUMBER) };
}

/// CPython `PyOS_InterruptOccurred`: on the registered main thread, report and
/// consume a recorded SIGINT delivery without running its handler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_InterruptOccurred() -> c_int {
    unsafe { (crate::hooks::hooks_or_stubs().interrupt_occurred)() }
}

// ─── PyArg_ParseTuple ─────────────────────────────────────────────────────
//
// Implements the subset of format codes that cover ~95% of real extensions:
//   i  → c_int*       (int)
//   l  → c_long*      (long)
//   L  → i64*         (long long)
//   K  → u64*         (unsigned long long)
//   d  → f64*         (double)
//   f  → f32*         (float)
//   s  → *const c_char* (str, null-terminated, borrowed)
//   s# → (*const c_char*, Py_ssize_t*) (str + length)
//   z  → *const c_char* (str or None → null)
//   O  → *mut PyObject* (any object, borrowed ref)
//   p  → c_int*        (bool/predicate)
//   n  → Py_ssize_t*   (ssize_t)
//   |  → marks optional args start
//   :  → function name for error messages
//   ;  → error message override
//
// Variadic C calling convention: we use `...` via a shim. The actual
// argument list is unpacked by inspecting the format string and reading
// pointer arguments from the va_list.

// PyArg_ParseTuple / PyArg_ParseTupleAndKeywords / PyArg_UnpackTuple are
// implemented in shims/pyarg_variadic.c (C file compiled via build.rs) because
// Rust stable does not support exporting variadic extern "C" functions.
//
// errors/arguments.rs owns the shared format grammar, borrowed physical
// argument binding and conversion. C only extracts the va_list addresses.

unsafe extern "C" {
    /// Variadic formatter authority implemented by `shims/pyarg_variadic.c`.
    pub fn PyUnicode_FromFormat(format: *const c_char, ...) -> *mut PyObject;

    /// Error formatter backed by the same fallible Unicode formatter.
    pub fn PyErr_Format(exc_type: *mut PyObject, format: *const c_char, ...) -> *mut PyObject;
}

/// Formatter-only typed adapter for `%T`/`%N`.
///
/// Both static extension types and Molt-managed type views keep their
/// canonical `module.qualname` spelling in `tp_name`. The C variadic shim
/// cannot safely redeclare the full `PyTypeObject` layout just to reach that
/// field, so it crosses this fixed-arity boundary instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_type_fully_qualified_name(
    tp: *mut PyTypeObject,
) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    unsafe {
        crate::api::strings::PyUnicode_FromStringAndSize(
            bytes.as_ptr().cast(),
            bytes.len() as Py_ssize_t,
        )
    }
}

mod arguments;
pub use arguments::{
    PyArg_ValidateKeywordArguments, molt_pyarg_format_out_count, molt_pyarg_parse_tuple_inner,
    molt_pyarg_parse_tuple_keywords_inner,
};
