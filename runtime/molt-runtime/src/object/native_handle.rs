use std::any::TypeId;
use std::sync::Arc;

use crate::{PyToken, TYPE_ID_NATIVE_HANDLE};

use super::{alloc_object, bits_from_ptr, object_type_id, ptr_from_bits};

struct NativeHandlePayload {
    type_id: TypeId,
    data: *const (),
    drop_fn: unsafe fn(*const ()),
}

unsafe fn drop_arc_payload<T: Send + Sync + 'static>(data: *const ()) {
    unsafe {
        drop(Arc::<T>::from_raw(data.cast::<T>()));
    }
}

pub(crate) fn native_handle_new<T: Send + Sync + 'static>(
    _py: &PyToken<'_>,
    handle: Arc<T>,
) -> u64 {
    let total =
        std::mem::size_of::<crate::MoltHeader>() + std::mem::size_of::<*mut NativeHandlePayload>();
    let ptr = alloc_object(_py, total, TYPE_ID_NATIVE_HANDLE);
    if ptr.is_null() {
        return 0;
    }
    let payload = Box::new(NativeHandlePayload {
        type_id: TypeId::of::<T>(),
        data: Arc::into_raw(handle).cast::<()>(),
        drop_fn: drop_arc_payload::<T>,
    });
    unsafe {
        *(ptr as *mut *mut NativeHandlePayload) = Box::into_raw(payload);
    }
    bits_from_ptr(ptr)
}

pub(crate) fn native_handle_arc<T: Send + Sync + 'static>(bits: u64) -> Option<Arc<T>> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        if object_type_id(ptr) != TYPE_ID_NATIVE_HANDLE {
            return None;
        }
        let payload_ptr = *(ptr as *mut *mut NativeHandlePayload);
        if payload_ptr.is_null() {
            return None;
        }
        let payload = &*payload_ptr;
        if payload.type_id != TypeId::of::<T>() {
            return None;
        }
        let arc = Arc::from_raw(payload.data.cast::<T>());
        let cloned = Arc::clone(&arc);
        let _ = Arc::into_raw(arc);
        Some(cloned)
    }
}

pub(crate) fn native_handle_drop(ptr: *mut u8) {
    unsafe {
        let payload_ptr = *(ptr as *mut *mut NativeHandlePayload);
        if payload_ptr.is_null() {
            return;
        }
        let payload = Box::from_raw(payload_ptr);
        (payload.drop_fn)(payload.data);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{dec_ref_bits, inc_ref_bits};

    use super::{native_handle_arc, native_handle_new};

    struct DropCounter {
        drops: Arc<AtomicUsize>,
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn native_handle_participates_in_object_refcounting() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let drops = Arc::new(AtomicUsize::new(0));
            let bits = native_handle_new(
                _py,
                Arc::new(DropCounter {
                    drops: Arc::clone(&drops),
                }),
            );
            assert_ne!(bits, 0);

            inc_ref_bits(_py, bits);
            let cloned = native_handle_arc::<DropCounter>(bits).expect("native handle clone");
            drop(cloned);
            dec_ref_bits(_py, bits);
            assert_eq!(drops.load(Ordering::Relaxed), 0);

            dec_ref_bits(_py, bits);
            assert_eq!(drops.load(Ordering::Relaxed), 1);
        });
    }
}
