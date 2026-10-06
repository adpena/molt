//! Immutable slice fields mirror the existing runtime owner and GC inventory.
use super::*;

impl ObjectBridge {
    /// Cold publication uses runtime fields. The C constructor supplies exact
    /// originating pointers before returning its new object to any caller.
    /// Both paths stage all three mirrors before replacing any published field.
    pub(crate) fn refresh_slice_view_with_origins(
        &self,
        bits: AbiHandle,
        origins: Option<[*mut PyObject; 3]>,
    ) -> bool {
        let is_slice = {
            let handle = self.handle_shard(bits).lock();
            matches!(
                handle.to_py.get(&bits).map(|entry| &entry.view),
                Some(ManagedView::Slice(_))
            )
        };
        if !is_slice {
            if origins.is_some() {
                unsafe { ensure_result_error(c"slice constructor did not publish a slice view") };
                return false;
            }
            return true;
        }
        let mut staged = [std::ptr::null_mut(); 3];
        let prepared = (|| {
            for (index, slot) in staged.iter_mut().enumerate() {
                let result = unsafe { (crate::hooks::hooks_or_stubs().slice_item)(bits, index) };
                let crate::hooks::DecodedHandleResult::Ok(value) = result.decode() else {
                    unsafe { ensure_result_error(c"runtime slice field unavailable") };
                    return false;
                };
                let pointer = if let Some(origins) = origins {
                    let pointer = origins[index];
                    if !self.pyobj_matches_handle(pointer, value) {
                        unsafe { ensure_result_error(c"slice origin differs from runtime field") };
                        return false;
                    }
                    if !unsafe { self.projection_incref(pointer) } {
                        return false;
                    }
                    pointer
                } else {
                    let Some(pointer) = self.list_projection_pointer(value) else {
                        return false;
                    };
                    pointer
                };
                *slot = pointer;
            }
            true
        })();
        if !prepared {
            crate::api::errors::with_preserved_error(|| {
                for pointer in staged {
                    unsafe { self.projection_decref(pointer) };
                }
            });
            return false;
        }
        let committed = {
            let mut handle = self.handle_shard(bits).lock();
            match handle.to_py.get_mut(&bits).map(|entry| &mut entry.view) {
                Some(ManagedView::Slice(object)) => unsafe {
                    let object = object.get();
                    std::mem::swap(&mut (*object).start, &mut staged[0]);
                    std::mem::swap(&mut (*object).stop, &mut staged[1]);
                    std::mem::swap(&mut (*object).step, &mut staged[2]);
                    true
                },
                _ => false,
            }
        };
        if !committed {
            unsafe { ensure_result_error(c"slice view disappeared during field publication") };
        }
        // No bridge locks cross a decref. On success these are the displaced
        // mirrors; on rollback these are exactly the successfully staged ones.
        crate::api::errors::with_preserved_error(|| {
            for pointer in staged {
                unsafe { self.projection_decref(pointer) };
            }
        });
        committed
    }
}
