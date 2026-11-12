use super::*;
use crate::object::*;
use crate::{MoltObject, dec_ref_bits, obj_from_bits};
use std::cell::Cell;

thread_local! {
    static SCOPED_FINALIZERS: Cell<usize> = const { Cell::new(0) };
}

extern "C" fn drop_scoped_during_finalizer(__context: u64, _self_bits: u64) -> u64 {
    assert_eq!(
        __context,
        crate::state::molt_attached_context(),
        "Python callback context identity"
    );
    let __py = unsafe { crate::PyToken::from_attached_context(__context) };

    {
        let py = __py;
        let active = crate::state::execution_tls().terminal_release.get();
        assert!(!active.is_null());
        // This finalizer runs on the last allowed heap frame. A nested scoped
        // terminal must complete even though another heap terminal would wait.
        assert_eq!(unsafe { (*active).depth.get() }, RELEASE_DEPTH_LIMIT);
        let survivor = crate::alloc_list(py, &[]);
        let survivor_bits = MoltObject::from_ptr(survivor).bits();
        let mut storage = [0u64; 16];
        let scoped = unsafe {
            alloc_scoped_boxed_object(py, 8, |bytes| {
                assert!(bytes <= std::mem::size_of_val(&storage));
                storage.as_mut_ptr().cast()
            })
        };
        let ptr = obj_from_bits(scoped).as_ptr().unwrap();
        unsafe {
            crate::object::accessors::molt_object_field_init_attached_body(
                py,
                scoped,
                0,
                survivor_bits,
            )
        };
        assert_eq!(
            unsafe { (*header_from_obj_ptr(survivor)).ref_count_snapshot() },
            2
        );
        dec_ref_bits(py, scoped);
        // The header lives in our frame. These observations happen BEFORE
        // unwinding/draining, and distinguish immediate retirement from merely
        // not crashing when a queued dangling pointer is read later.
        assert!(unsafe { (*header_from_obj_ptr(ptr)).has_flag(HEADER_FLAG_DEALLOCATING) });
        assert!(!unsafe { gc::gc_is_tracked(ptr) });
        assert_eq!(
            unsafe { (*header_from_obj_ptr(survivor)).ref_count_snapshot() },
            1
        );
        storage.fill(u64::MAX);
        dec_ref_bits(py, survivor_bits);
        SCOPED_FINALIZERS.with(|count| count.set(count.get() + 1));
        MoltObject::none().bits()
    }
}

#[test]
fn finalizer_retires_scoped_storage_before_returning_at_the_heap_depth_bound() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        SCOPED_FINALIZERS.with(|count| count.set(0));
        // Ordinary string keys exercise the actual class definition boundary;
        // this fixture adds no interned-name lookup site.
        let name = MoltObject::from_ptr(crate::alloc_string(py, b"ScopedReleaseFinalizer")).bits();
        let del = MoltObject::from_ptr(crate::alloc_string(py, b"__del__")).bits();
        let class = crate::molt_class_new_attached_body(py, name);
        let callback = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "drop_scoped_during_finalizer",
                drop_scoped_during_finalizer as *const (),
            ),
            1,
        );
        let callback = MoltObject::from_ptr(callback).bits();
        crate::molt_set_attr_name_attached_body(py, class, del, callback);
        let mut chain = crate::molt_object_new_bound_attached_body(py, class);
        // Cross several segments, then reach the finalizer exactly at the
        // bound. None of the loop temporaries retains a chain owner.
        for _ in 0..(RELEASE_DEPTH_LIMIT * 3 - 1) {
            let next = MoltObject::from_ptr(crate::alloc_list(py, &[chain])).bits();
            dec_ref_bits(py, chain);
            chain = next;
        }
        dec_ref_bits(py, chain);
        assert_eq!(SCOPED_FINALIZERS.with(Cell::get), 1);
        assert!(
            crate::state::execution_tls()
                .terminal_release
                .get()
                .is_null()
        );
        for bits in [callback, class, del, name] {
            dec_ref_bits(py, bits);
        }
        assert!(!crate::exception_pending(py));
    });
}
