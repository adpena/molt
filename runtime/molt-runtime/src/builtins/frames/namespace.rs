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
/// object's, or a resumed activation's (its task, whose frame context holds the
/// namespace it was created with). Borrowed: the invoker keeps the callable
/// alive for the call, and the frame entry that takes it retains what it
/// keeps.
fn pending_namespace(callable: *mut u8) -> [u64; 4] {
    unsafe {
        if object_type_id(callable) == TYPE_ID_FUNCTION {
            [
                function_globals_bits(callable),
                function_builtins_bits(callable),
                function_code_bits(callable),
                0,
            ]
        } else {
            let [globals, builtins, code] =
                crate::object::aux_header::object_frame_context_bits(callable);
            [
                globals,
                builtins,
                code,
                crate::MoltObject::from_ptr(callable).bits(),
            ]
        }
    }
}

/// The invocation handoff this thread's next compiled frame entry may take:
/// the invoker's callable, borrowed (the thread record's
/// `THREAD_RECORD_PENDING_INVOCATION_OFFSET` word, which compiled direct calls
/// write themselves).
fn pending_invocation() -> Option<*mut u8> {
    obj_from_bits(crate::state::execution_tls().pending_invocation.get()).as_ptr()
}

/// Retains the pending context for a task constructor that precedes frame entry.
/// The eventual compiled entry still takes the original handoff exactly once.
pub(crate) fn acquire_pending_invocation_context(
    py: &PyToken<'_>,
    target: u64,
) -> Option<[u64; 3]> {
    let [globals, builtins, code, _] = pending_namespace(pending_invocation()?);
    if !code_targets_callable(code, target) {
        return None;
    }
    let context = [globals, builtins, code];
    for bits in context {
        inc_ref_bits(py, bits);
    }
    Some(context)
}

/// Publishes the invoker's callable for the compiled frame entry it calls, as
/// CPython's caller builds the callee's frame from the function object, and
/// restores the outer handoff when the call returns. Every compiled invoker
/// pairs a publication with a restore around one call: compiled code writes
/// the same thread-record word around a direct call.
pub(crate) struct FrameInvocationGuard {
    previous: u64,
}

impl FrameInvocationGuard {
    fn publish(callable_bits: u64) -> Self {
        let pending = &crate::state::execution_tls().pending_invocation;
        Self {
            previous: pending.replace(callable_bits),
        }
    }

    pub(crate) fn for_callable(_py: &PyToken<'_>, bits: u64) -> Self {
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
    pub(crate) fn for_function(_py: &PyToken<'_>, function: *mut u8) -> Self {
        if function.is_null() || unsafe { object_type_id(function) } != TYPE_ID_FUNCTION {
            return Self::publish(0);
        }
        Self::publish(crate::MoltObject::from_ptr(function).bits())
    }

    /// The resumed activation of `task`, whose frame context holds the
    /// namespace it was created with. Its poll pins the payload until the real
    /// compiled frame exits.
    pub(crate) fn for_activation(_py: &PyToken<'_>, task: *mut u8) -> Self {
        Self::publish(crate::MoltObject::from_ptr(task).bits())
    }
}

impl Drop for FrameInvocationGuard {
    fn drop(&mut self) {
        crate::state::execution_tls()
            .pending_invocation
            .set(self.previous);
    }
}

/// Takes the invoker's handoff for the compiled frame of `slot` that is
/// entering: its namespace, owned (globals, builtins, code, activation).
/// `Ok(None)`: no invoker handed this slot a namespace, and the entry takes its
/// code slot's defaults. `Err`: the handed namespace has no globals mapping;
/// the error is pending.
pub(crate) fn take_invocation_namespace(
    py: &PyToken<'_>,
    slot: u64,
) -> Result<Option<[u64; 4]>, ()> {
    let Some(callable) = pending_invocation() else {
        return Ok(None);
    };
    let namespace = pending_namespace(callable);
    let [globals, _, code, _] = namespace;
    if compiled_slot_for_code(code) != Some(slot) {
        return Ok(None);
    }
    crate::state::execution_tls().pending_invocation.set(0);
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
    for bits in namespace {
        inc_ref_bits(py, bits);
    }
    Ok(Some(namespace))
}
