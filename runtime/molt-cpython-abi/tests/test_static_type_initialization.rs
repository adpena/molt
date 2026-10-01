//! Process bootstrap is distinct from runtime-owned root retirement/rebuild.
//! No fake runtime/GIL is installed: callers race only the immutable publication
//! boundary, then repeat it while legitimate live type metadata is retained.

use molt_cpython_abi::abi_types::*;

#[test]
fn concurrent_and_repeated_bootstrap_preserves_live_builtin_shells() {
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..256 {
                    unsafe {
                        init_static_types();
                        let tuple = &raw const PyTuple_Type;
                        assert_ne!((*tuple).tp_flags & Py_TPFLAGS_HAVE_GC, 0);
                        assert_ne!((*tuple).tp_flags & Py_TPFLAGS_READY, 0);
                        assert_eq!((*tuple).ob_base.ob_base.ob_type, &raw mut PyType_Type);
                        assert!((*tuple).tp_alloc.is_some());
                        assert!((*tuple).tp_dealloc.is_some());
                        assert!(std::ptr::fn_addr_eq(
                            (*tuple).tp_free.expect("published tuple free"),
                            molt_cpython_abi::api::memory::PyObject_GC_Del
                                as unsafe extern "C" fn(*mut std::ffi::c_void),
                        ));
                    }
                }
            });
        }
    });

    unsafe {
        let tuple = &raw mut PyTuple_Type;
        // A live type's version authority can legitimately change after
        // bootstrap. Another extension's initialization must not erase it.
        (*tuple).tp_version_tag = 0x13579;
        (*tuple).tp_flags |= Py_TPFLAGS_VALID_VERSION_TAG;
        let flags = (*tuple).tp_flags;
        let refcount = (*tuple).ob_base.ob_base.ob_refcnt;
        for _ in 0..16 {
            init_static_types();
            assert_eq!((*tuple).tp_flags, flags);
            assert_eq!((*tuple).tp_version_tag, 0x13579);
            assert_eq!((*tuple).ob_base.ob_base.ob_refcnt, refcount);
        }
    }
}
