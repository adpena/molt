//! Managed C GC controls are projections of runtime membership/header state.
use molt_cpython_abi::hooks::ManagedGcAction;
use std::os::raw::c_int;

struct PinnedGcSnapshot<'a, 'py> {
    py: &'a crate::PyToken<'py>,
    owner: u64,
    edges: Vec<molt_cpython_abi::NativeGcEdge>,
}

impl PinnedGcSnapshot<'_, '_> {
    fn push(&mut self, edge: molt_cpython_abi::NativeGcEdge) -> bool {
        if self.edges.try_reserve(1).is_err() { return false; }
        unsafe {
            if edge.kind == molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8 {
                crate::inc_ref_bits(self.py, edge.value);
            } else {
                molt_cpython_abi::api::refcount::Py_INCREF(
                    core::ptr::with_exposed_provenance_mut::<molt_cpython_abi::abi_types::PyObject>(
                        usize::try_from(edge.value).unwrap_or_else(|_| std::process::abort()),
                    ),
                );
            }
        }
        self.edges.push(edge);
        true
    }
}

impl Drop for PinnedGcSnapshot<'_, '_> {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
            for edge in self.edges.drain(..) {
                if edge.kind == molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8 {
                    crate::dec_ref_bits(self.py, edge.value);
                } else {
                    molt_cpython_abi::api::refcount::Py_DECREF(
                        core::ptr::with_exposed_provenance_mut::<molt_cpython_abi::abi_types::PyObject>(
                            usize::try_from(edge.value).unwrap_or_else(|_| std::process::abort()),
                        ),
                    );
                }
            }
            crate::dec_ref_bits(self.py, self.owner);
        });
    }
}

pub(super) unsafe extern "C" fn hook_managed_gc_traverse(
    bits: u64,
    visit: molt_cpython_abi::api::memory::NativeGcVisitProc,
    context: *mut std::ffi::c_void,
) -> c_int {
    crate::concurrency::gil::with_gil(|py| unsafe {
        let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
            crate::raise_exception::<()>(&py, "SystemError", "GC traversal requires a managed object");
            return -1;
        };
        crate::inc_ref_bits(&py, bits);
        let mut snapshot = PinnedGcSnapshot { py: &py, owner: bits, edges: Vec::new() };
        let mut reserved = true;
        let status = crate::object::heap_lifecycle::visit_owned_gc_edges(&py, ptr, &mut |edge| {
            if reserved { reserved = snapshot.push(edge); }
        });
        if !reserved {
            crate::raise_exception::<()>(&py, "MemoryError", "cannot snapshot builtin GC edges");
            return -1;
        }
        if status != 0 { return status; }
        // Enumeration has dropped every runtime/bridge/module storage lock.
        // Pins protect this exact inventory if the visitor reenters or clears
        // the source; mirrored C holds were never added as independent edges.
        for &edge in &snapshot.edges {
            let status = visit(edge, context);
            if status != 0 { return status; }
        }
        0
    })
}

pub(super) unsafe extern "C" fn hook_managed_gc_clear(bits: u64) -> c_int {
    crate::concurrency::gil::with_gil(|py| unsafe {
        let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
            crate::raise_exception::<()>(&py, "SystemError", "GC clear requires a managed object");
            return -1;
        };
        crate::inc_ref_bits(&py, bits);
        let _pin = PinnedGcSnapshot { py: &py, owner: bits, edges: Vec::new() };
        crate::object::heap_lifecycle::try_clear_cycle_edges(&py, ptr)
    })
}

pub(super) unsafe extern "C" fn hook_managed_gc_control(
    bits: u64,
    action: ManagedGcAction,
) -> c_int {
    crate::concurrency::gil::with_gil(|py| {
        let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
            return match action {
                ManagedGcAction::Track => -1,
                ManagedGcAction::Untrack
                | ManagedGcAction::IsTracked
                | ManagedGcAction::IsFinalized => 0,
            };
        };
        unsafe {
            match action {
                ManagedGcAction::Track => {
                    if crate::object::gc::gc_track_existing(&py, ptr) {
                        0
                    } else {
                        -1
                    }
                }
                ManagedGcAction::Untrack => {
                    crate::object::gc::gc_untrack(
                        &py,
                        ptr,
                        crate::object::object_type_id(ptr),
                        crate::object::gc::GcUntrackReason::ExplicitControl,
                    );
                    0
                }
                ManagedGcAction::IsTracked => c_int::from(crate::object::gc::gc_is_tracked(ptr)),
                ManagedGcAction::IsFinalized => {
                    c_int::from(crate::object::gc::gc_is_finalized(ptr))
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
