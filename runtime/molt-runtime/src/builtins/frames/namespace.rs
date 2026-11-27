//! Explicit namespace custody at compiled symbol and invocation boundaries.
//!
//! Code slots own lexical defaults; a function object or suspended task supplies
//! the activation's namespace. Source filenames and mutable module metadata never
//! participate. A keyed, single-consumption handoff does not create a second
//! Python frame, and cannot leak into a later direct recursive call.

use crate::BuiltinExc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::object::layout::{
    bound_method_func_bits, code_frame_slot_id, function_builtins_bits, function_code_bits,
    function_globals_bits,
};
use crate::{
    PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_CODE, TYPE_ID_FUNCTION, dec_ref_bits, inc_ref_bits,
    obj_from_bits, object_type_id, raise_exception,
};

/// Resolve the exact dictionary that owns raw globals storage while retaining
/// the Python-visible namespace object as the frame identity. Dynamic dict
/// subclasses deliberately use the existing object-shaped side storage lane.
pub(crate) fn globals_namespace_storage_bits(
    py: &PyToken<'_>,
    namespace_bits: u64,
) -> Result<Option<u64>, ()> {
    crate::object::ops::dict_backing_bits(py, namespace_bits)
}

pub(crate) fn globals_namespace_storage_ptr(
    py: &PyToken<'_>,
    namespace_bits: u64,
) -> Result<Option<*mut u8>, ()> {
    Ok(globals_namespace_storage_bits(py, namespace_bits)?
        .and_then(|bits| obj_from_bits(bits).as_ptr()))
}

/// A Python activation's owned globals, builtins and code. The handoff
/// owns each edge once and transfers it to a frame or task, or drops it if
/// entry fails before publication.
pub(crate) struct OwnedFrameNamespace<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: [u64; 3],
}

impl<'a, 'py> OwnedFrameNamespace<'a, 'py> {
    /// Adopt one already-owned reference to each nonzero word.
    pub(crate) fn adopt(py: &'a PyToken<'py>, bits: [u64; 3]) -> Self {
        Self { py, bits }
    }

    pub(crate) fn retain(py: &'a PyToken<'py>, bits: [u64; 3]) -> Self {
        for word in bits {
            inc_ref_bits(py, word);
        }
        Self::adopt(py, bits)
    }

    pub(crate) fn bits(&self) -> [u64; 3] {
        self.bits
    }

    pub(crate) fn into_bits(mut self) -> [u64; 3] {
        std::mem::take(&mut self.bits)
    }
}

impl Drop for OwnedFrameNamespace<'_, '_> {
    fn drop(&mut self) {
        for word in self.bits {
            if word != 0 {
                dec_ref_bits(self.py, word);
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct CodeNamespace {
    pub(crate) code_bits: u64,
    pub(crate) globals_bits: u64,
}

impl CodeNamespace {
    fn retain(&self, py: &PyToken<'_>) {
        inc_ref_bits(py, self.code_bits);
        inc_ref_bits(py, self.globals_bits);
    }

    pub(crate) fn release(self, py: &PyToken<'_>) {
        dec_ref_bits(py, self.code_bits);
        dec_ref_bits(py, self.globals_bits);
    }

    /// Move this acquired slot's references into the activation.
    pub(crate) fn with_builtins<'a, 'py>(
        self,
        py: &'a PyToken<'py>,
        builtins: u64,
    ) -> OwnedFrameNamespace<'a, 'py> {
        inc_ref_bits(py, builtins);
        OwnedFrameNamespace::adopt(py, [self.globals_bits, builtins, self.code_bits])
    }
}

#[derive(Default)]
pub(crate) struct CompiledCodeSlot {
    code: AtomicU64,
    globals: AtomicU64,
    /// The frame plan of every synchronous activation of this compiled body
    /// ([`super::FramePlan`]), derived once when its code is published.
    frame_plan: AtomicU64,
}

impl CompiledCodeSlot {
    pub(crate) fn set_frame_plan(&self, plan: super::FramePlan) {
        crate::gil_assert();
        self.frame_plan.store(plan.bits(), Ordering::Relaxed);
    }

    pub(crate) fn frame_plan(&self) -> super::FramePlan {
        super::FramePlan::from_bits(self.frame_plan.load(Ordering::Relaxed))
    }

    pub(crate) fn replace(&self, py: &PyToken<'_>, value: CodeNamespace) {
        crate::gil_assert();
        value.retain(py);
        // The PyToken serializes the pair; publication contains no callback.
        // Atomic storage preserves shared RuntimeState without adding a lock
        // on every compiled entry. Release only after both fields are visible.
        let old = CodeNamespace {
            code_bits: self.code.swap(value.code_bits, Ordering::Relaxed),
            globals_bits: self.globals.swap(value.globals_bits, Ordering::Relaxed),
        };
        old.release(py);
    }

    /// Returned pair owns both references, acquired under the active PyToken.
    pub(crate) fn acquire(&self, py: &PyToken<'_>) -> CodeNamespace {
        crate::gil_assert();
        let value = CodeNamespace {
            code_bits: self.code.load(Ordering::Relaxed),
            globals_bits: self.globals.load(Ordering::Relaxed),
        };
        value.retain(py);
        value
    }

    /// Empty this slot for the next test: release its pair, forget its plan.
    #[cfg(test)]
    pub(crate) fn clear(&self, py: &PyToken<'_>) {
        self.take(py).release(py);
        self.frame_plan.store(0, Ordering::Relaxed);
    }

    pub(crate) fn take(&self, _py: &PyToken<'_>) -> CodeNamespace {
        crate::gil_assert();
        CodeNamespace {
            code_bits: self.code.swap(0, Ordering::Relaxed),
            globals_bits: self.globals.swap(0, Ordering::Relaxed),
        }
    }
}

pub(crate) fn compiled_slot_for_code(code_bits: u64) -> Option<u64> {
    obj_from_bits(code_bits).as_ptr().and_then(|ptr| unsafe {
        (object_type_id(ptr) == TYPE_ID_CODE)
            .then(|| code_frame_slot_id(ptr))
            .flatten()
    })
}

/// Match the actual code's physical target, never a last-published pointer map.
fn code_targets_callable(code_bits: u64, target: u64) -> bool {
    target != 0
        && obj_from_bits(code_bits).as_ptr().is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_CODE
                && crate::object::layout::code_callable_fn_ptr(ptr) == target
        })
}

/// One admission rule for every compiled Python frame: the code must own a
/// compiled slot and its activation must carry a real globals namespace.
/// `Ok(None)` identifies runtime builtins, which own no Python frame.
pub(crate) fn admit_compiled_namespace(
    py: &PyToken<'_>,
    code_bits: u64,
    globals_bits: u64,
) -> Result<Option<u64>, ()> {
    let Some(slot) = compiled_slot_for_code(code_bits) else {
        return Ok(None);
    };
    if globals_namespace_storage_bits(py, globals_bits)?.is_none() {
        if !crate::exception_pending(py) {
            raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "compiled invocation has no globals namespace",
            );
        }
        return Err(());
    }
    Ok(Some(slot))
}

/// The namespace an invoker hands to the compiled frame it calls: a function
/// object's (globals, builtins, code). Borrowed: the invoker keeps the
/// function alive for the call, and the frame entry that takes it retains what
/// it keeps. A resumed activation hands nothing: its task's persistent record
/// carries its frame.
fn pending_namespace(function: *mut u8) -> Option<[u64; 3]> {
    unsafe {
        (object_type_id(function) == TYPE_ID_FUNCTION).then(|| {
            [
                function_globals_bits(function),
                function_builtins_bits(function),
                function_code_bits(function),
            ]
        })
    }
}

/// The invocation handoff this thread's next compiled frame entry may take:
/// the invoker's callable, borrowed (the thread record's
/// `THREAD_RECORD_PENDING_INVOCATION_OFFSET` word, which compiled direct calls
/// write themselves).
pub(crate) fn pending_invocation(py: &PyToken<'_>) -> Option<*mut u8> {
    obj_from_bits(py.execution_tls().pending_invocation.get()).as_ptr()
}

/// Retains the pending context for a task constructor that precedes frame entry.
/// The eventual compiled entry still takes the original handoff exactly once.
pub(crate) fn acquire_pending_invocation_context<'a, 'py>(
    py: &'a PyToken<'py>,
    target: u64,
) -> Option<OwnedFrameNamespace<'a, 'py>> {
    acquire_function_context(py, pending_invocation(py)?, target)
}

/// Retains `function`'s (globals, builtins, code) when its code's compiled
/// target is `target`: the frame context of a task whose poll is that target.
/// A compiled task constructor names its function directly; a trampoline names
/// it through the pending invocation.
pub(crate) fn acquire_function_context<'a, 'py>(
    py: &'a PyToken<'py>,
    function: *mut u8,
    target: u64,
) -> Option<OwnedFrameNamespace<'a, 'py>> {
    let namespace = pending_namespace(function)?;
    code_targets_callable(namespace[2], target).then(|| OwnedFrameNamespace::retain(py, namespace))
}

/// Publishes the invoker's callable for the compiled frame entry it calls, as
/// CPython's caller builds the callee's frame from the function object, and
/// restores the outer handoff when the call returns. Every compiled invoker
/// pairs a publication with a restore around one call: compiled code writes
/// the same thread-record word around a direct call.
pub(crate) struct FrameInvocationGuard<'scope, 'py> {
    py: &'scope PyToken<'py>,
    scope: crate::concurrency::execution::CompiledScopeGuard<'scope, 'py>,
    previous: u64,
}

impl<'scope, 'py> FrameInvocationGuard<'scope, 'py> {
    fn publish(py: &'scope PyToken<'py>, callable_bits: u64) -> Self {
        let scope = crate::concurrency::execution::CompiledScopeGuard::enter(py);
        let pending = &py.execution_tls().pending_invocation;
        Self {
            py,
            scope,
            previous: pending.replace(callable_bits),
        }
    }

    pub(crate) fn for_callable(_py: &'scope PyToken<'py>, bits: u64) -> Self {
        let mut ptr = obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut());
        if !ptr.is_null() && unsafe { object_type_id(ptr) } == TYPE_ID_BOUND_METHOD {
            ptr = obj_from_bits(unsafe { bound_method_func_bits(ptr) })
                .as_ptr()
                .unwrap_or(std::ptr::null_mut());
        }
        Self::for_function(_py, ptr)
    }

    /// A runtime function or a non-function callable owns no Python frame and
    /// hands nothing over; the call still restores the outer handoff.
    pub(crate) fn for_function(_py: &'scope PyToken<'py>, function: *mut u8) -> Self {
        if function.is_null() || unsafe { object_type_id(function) } != TYPE_ID_FUNCTION {
            return Self::publish(_py, 0);
        }
        Self::publish(_py, crate::MoltObject::from_ptr(function).bits())
    }
}

impl Drop for FrameInvocationGuard<'_, '_> {
    fn drop(&mut self) {
        self.scope.validate();
        self.py
            .execution_tls()
            .pending_invocation
            .set(self.previous);
    }
}

/// Takes the invoker's handoff for the compiled frame of `slot` that is
/// entering: its namespace, owned (globals, builtins, code).
/// `Ok(None)`: no invoker handed this slot a namespace, and the entry takes its
/// code slot's defaults. `Err`: the handed namespace has no globals mapping;
/// the error is pending.
pub(crate) fn take_invocation_namespace<'a, 'py>(
    py: &'a PyToken<'py>,
    slot: u64,
) -> Result<Option<OwnedFrameNamespace<'a, 'py>>, ()> {
    let Some(namespace) = pending_invocation(py).and_then(pending_namespace) else {
        return Ok(None);
    };
    let [globals, _, code] = namespace;
    if compiled_slot_for_code(code) != Some(slot) {
        return Ok(None);
    }
    py.execution_tls().pending_invocation.set(0);
    if globals_namespace_storage_bits(py, globals)?.is_none() {
        if !crate::exception_pending(py) {
            raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "compiled invocation has no globals namespace",
            );
        }
        return Err(());
    }
    Ok(Some(OwnedFrameNamespace::retain(py, namespace)))
}
