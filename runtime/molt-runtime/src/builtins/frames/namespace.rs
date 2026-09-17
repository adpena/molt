//! Explicit namespace custody at compiled symbol and invocation boundaries.
//!
//! Code slots own lexical defaults; a function object or suspended task supplies
//! the activation's namespace. Source filenames and mutable module metadata never
//! participate. A keyed, single-consumption handoff does not create a second
//! Python frame, and cannot leak into a later direct recursive call.

use std::cell::RefCell;
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
pub(crate) fn globals_namespace_storage_bits(py: &PyToken<'_>, namespace_bits: u64) -> Option<u64> {
    let ptr = obj_from_bits(namespace_bits).as_ptr()?;
    unsafe { crate::object::ops::dict_like_bits_from_ptr(py, ptr) }
}

pub(crate) fn globals_namespace_storage_ptr(
    py: &PyToken<'_>,
    namespace_bits: u64,
) -> Option<*mut u8> {
    globals_namespace_storage_bits(py, namespace_bits).and_then(|bits| obj_from_bits(bits).as_ptr())
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
}

impl CompiledCodeSlot {
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

    pub(crate) fn take(&self, _py: &PyToken<'_>) -> CodeNamespace {
        crate::gil_assert();
        CodeNamespace {
            code_bits: self.code.swap(0, Ordering::Relaxed),
            globals_bits: self.globals.swap(0, Ordering::Relaxed),
        }
    }
}

struct PendingNamespace {
    slot: u64,
    /// Owned globals, builtins and exact callable code identity.
    namespace: Option<[u64; 3]>,
}

thread_local! {
    static PENDING_NAMESPACES: RefCell<Vec<PendingNamespace>> = const { RefCell::new(Vec::new()) };
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

/// Retains the pending context for a task constructor that precedes frame entry.
/// The eventual compiled entry still consumes the original handoff exactly once.
pub(crate) fn acquire_pending_invocation_context(
    py: &PyToken<'_>,
    target: u64,
) -> Option<[u64; 3]> {
    let context = PENDING_NAMESPACES.with(|pending| {
        let pending = pending.borrow();
        let entry = pending.last()?;
        let namespace = entry.namespace?;
        code_targets_callable(namespace[2], target).then_some(namespace)
    })?;
    for bits in context {
        inc_ref_bits(py, bits);
    }
    Some(context)
}

/// Owns one invocation handoff until entry consumes it or the call returns.
pub(crate) struct FrameInvocationGuard<'a, 'py> {
    py: &'a PyToken<'py>,
    depth: Option<usize>,
}

impl<'a, 'py> FrameInvocationGuard<'a, 'py> {
    /// Transfer custody to generated code that pairs entry and exit across a
    /// statically typed call (including targets with a void machine ABI).
    pub(crate) fn into_token(mut self) -> u64 {
        self.depth.take().map_or(1, |depth| depth as u64 + 2)
    }

    pub(crate) fn exit_token(py: &'a PyToken<'py>, token: u64) {
        if token == 0 {
            return;
        }
        drop(Self {
            py,
            depth: (token >= 2).then(|| (token - 2) as usize),
        });
    }

    pub(crate) fn for_callable(py: &'a PyToken<'py>, bits: u64) -> Option<Self> {
        let mut ptr = obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut());
        if !ptr.is_null() && unsafe { object_type_id(ptr) } == TYPE_ID_BOUND_METHOD {
            ptr = obj_from_bits(unsafe { bound_method_func_bits(ptr) })
                .as_ptr()
                .unwrap_or(std::ptr::null_mut());
        }
        Self::for_function(py, ptr)
    }

    pub(crate) fn for_function(py: &'a PyToken<'py>, function: *mut u8) -> Option<Self> {
        if function.is_null() || unsafe { object_type_id(function) } != TYPE_ID_FUNCTION {
            return Some(Self { py, depth: None });
        }
        unsafe {
            Self::for_suspended_namespace(
                py,
                function_code_bits(function),
                function_globals_bits(function),
                function_builtins_bits(function),
            )
        }
    }

    #[cfg(test)]
    pub(crate) fn for_namespace(
        py: &'a PyToken<'py>,
        code_bits: u64,
        globals_bits: u64,
    ) -> Option<Self> {
        let builtins_bits = super::frame_effective_builtins_bits(py, globals_bits);
        Self::for_suspended_namespace(py, code_bits, globals_bits, builtins_bits)
    }

    pub(crate) fn for_suspended_namespace(
        py: &'a PyToken<'py>,
        code_bits: u64,
        globals_bits: u64,
        builtins_bits: u64,
    ) -> Option<Self> {
        let Some(slot) = compiled_slot_for_code(code_bits) else {
            // Runtime builtins do not own Python execution frames.
            return Some(Self { py, depth: None });
        };
        if globals_namespace_storage_bits(py, globals_bits).is_none() {
            if crate::exception_pending(py) {
                return None;
            }
            raise_exception::<u64>(
                py,
                "SystemError",
                "compiled invocation has no globals namespace",
            );
            return None;
        }
        inc_ref_bits(py, globals_bits);
        inc_ref_bits(py, builtins_bits);
        inc_ref_bits(py, code_bits);
        let depth = PENDING_NAMESPACES.with(|pending| {
            let mut pending = pending.borrow_mut();
            let depth = pending.len();
            pending.push(PendingNamespace {
                slot,
                namespace: Some([globals_bits, builtins_bits, code_bits]),
            });
            depth
        });
        Some(Self {
            py,
            depth: Some(depth),
        })
    }
}

impl Drop for FrameInvocationGuard<'_, '_> {
    fn drop(&mut self) {
        let Some(depth) = self.depth else {
            return;
        };
        let namespace = PENDING_NAMESPACES.with(|pending| {
            let mut pending = pending.borrow_mut();
            assert_eq!(
                pending.len(),
                depth + 1,
                "compiled invocation custody is not LIFO"
            );
            pending.pop().expect("owned invocation").namespace
        });
        if let Some(namespace) = namespace {
            for bits in namespace {
                dec_ref_bits(self.py, bits);
            }
        }
    }
}

/// Transfers the owned namespace only to the explicitly targeted compiled slot.
pub(crate) fn take_invocation_namespace(slot: u64) -> Option<[u64; 3]> {
    PENDING_NAMESPACES.with(|pending| {
        let mut pending = pending.borrow_mut();
        let entry = pending.last_mut()?;
        if entry.slot == slot {
            entry.namespace.take()
        } else {
            None
        }
    })
}

pub(crate) fn take_pending_namespaces_for_teardown() -> Vec<u64> {
    PENDING_NAMESPACES
        .try_with(|pending| {
            std::mem::take(&mut *pending.borrow_mut())
                .into_iter()
                .filter_map(|entry| entry.namespace)
                .flatten()
                .collect()
        })
        .unwrap_or_default()
}
