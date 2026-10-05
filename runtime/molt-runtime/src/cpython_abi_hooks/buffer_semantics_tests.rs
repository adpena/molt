//! Real runtime/C memoryview semantics; the ABI crate has no shadow object owner.
#![allow(unused_unsafe)]
use molt_cpython_abi::abi_types::*;
use std::ffi::{c_char, c_void};
use std::ptr;

#[test]
fn test_memoryview_from_memory_has_type_and_null_base() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let mut byte = b'x' as c_char;
        let view = unsafe {
            molt_cpython_abi::api::memory::PyMemoryView_FromMemory(&mut byte, 1, PyBUF_WRITE)
        };
        assert!(!view.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::memory::PyMemoryView_Check(view) },
            1
        );
        assert!(unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(view) }.is_null());
        let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
        assert!(!buffer.is_null());
        assert_eq!(unsafe { (*buffer).len }, 1);
        // CPython copies the managed buffer's descriptor into each view. Shape
        // and stride values must survive the originating view's release; their
        // addresses need not be aliases of fields in the exposed Py_buffer.
        assert!(unsafe { (*buffer).internal }.is_null());
        assert!(!unsafe { (*buffer).format }.is_null());
        assert!(!unsafe { (*buffer).shape }.is_null());
        assert!(!unsafe { (*buffer).strides }.is_null());
        unsafe {
            assert_eq!(*(*buffer).format as u8, b'B');
            assert_eq!(*(*buffer).shape, 1);
            assert_eq!(*(*buffer).strides, 1);
        }
        // CPython: FromObject(memoryview) returns a NEW distinct memoryview sharing
        // the source's buffer (mbuf_add_view) — never the same object aliased.
        let second_view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromObject(view) };
        assert!(!second_view.is_null());
        assert_ne!(
            second_view, view,
            "FromObject(mv) must mint a distinct view"
        );
        let second_buffer =
            unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(second_view) };
        assert_eq!(
            unsafe { (*second_buffer).buf },
            unsafe { (*buffer).buf },
            "the new view shares the source's memory"
        );
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
        unsafe {
            assert_eq!((*second_buffer).len, 1);
            assert_eq!(*(*second_buffer).shape, 1);
            assert_eq!(*(*second_buffer).strides, 1);
            assert_eq!(*(*second_buffer).format as u8, b'B');
            *(*second_buffer).buf.cast::<c_char>() = b'y' as c_char;
            assert_eq!(
                byte, b'y' as c_char,
                "the surviving view still aliases the exporter"
            );
            molt_cpython_abi::api::refcount::Py_DECREF(second_view);
        }

        let empty_view = unsafe {
            molt_cpython_abi::api::memory::PyMemoryView_FromMemory(ptr::null_mut(), 0, PyBUF_READ)
        };
        assert!(!empty_view.is_null());
        let empty_buffer =
            unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(empty_view) };
        assert!(!empty_buffer.is_null());
        assert_eq!(unsafe { (*empty_buffer).len }, 0);

        assert!(
            unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(empty_view) }.is_null()
        );
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(empty_view) };
    });
}

#[test]
fn test_memoryview_from_buffer_copies_descriptor_without_sharing_release() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let mut bytes = [1_u8, 2, 3, 4];
        let mut info: Py_buffer = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
                &mut info,
                ptr::null_mut(),
                bytes.as_mut_ptr().cast(),
                bytes.len() as isize,
                1,
                PyBUF_FORMAT | PyBUF_STRIDES,
            )
        };
        assert_eq!(rc, 0);
        // CPython-exact FillInfo: allocation-free, `internal` NULL, shape/strides
        // self-referential.
        assert!(info.internal.is_null());
        assert!(std::ptr::eq(info.shape.cast_const(), &raw const info.len));

        let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
        assert!(!view.is_null());
        // The caller still owns `info` and releases it exactly once; the
        // memoryview's copied descriptor must be unaffected.
        unsafe { molt_cpython_abi::api::buffer::PyBuffer_Release(&mut info) };
        assert!(info.internal.is_null());

        assert!(unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(view) }.is_null());
        let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
        assert!(!buffer.is_null());
        // Values and stable borrowed identity, independent of private projection layout.
        assert!(unsafe { (*buffer).internal }.is_null());
        assert_eq!(buffer, unsafe {
            molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view)
        });
        assert_eq!(unsafe { (*buffer).buf }, bytes.as_mut_ptr().cast());
        assert_eq!(unsafe { (*buffer).len }, bytes.len() as isize);
        assert_eq!(unsafe { (*buffer).itemsize }, 1);
        assert_eq!(unsafe { (*buffer).readonly }, 1);
        assert_eq!(unsafe { (*buffer).ndim }, 1);
        unsafe {
            assert_eq!(*(*buffer).format as u8, b'B');
            assert_eq!(*(*buffer).shape, bytes.len() as isize);
            assert_eq!(*(*buffer).strides, 1);
        }
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
    });
}

#[test]
fn test_memoryview_from_buffer_rejects_indirect_suboffsets() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let mut bytes = [1_u8, 2, 3, 4];
        let mut shape = [bytes.len() as isize];
        let mut strides = [1isize];
        let mut suboffsets = [0isize];
        let mut format = [b'B' as c_char, 0];
        let mut info: Py_buffer = unsafe { std::mem::zeroed() };
        info.buf = bytes.as_mut_ptr().cast();
        info.len = bytes.len() as isize;
        info.itemsize = 1;
        info.readonly = 1;
        info.ndim = 1;
        info.format = format.as_mut_ptr();
        info.shape = shape.as_mut_ptr();
        info.strides = strides.as_mut_ptr();
        info.suboffsets = suboffsets.as_mut_ptr();

        let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
        assert!(view.is_null());
    });
}

#[test]
fn test_memoryview_from_buffer_preserves_zero_dimensional_descriptor() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let mut bytes = [0_u8; 8];
        let mut format = [b'd' as c_char, 0];
        let mut info: Py_buffer = unsafe { std::mem::zeroed() };
        info.buf = bytes.as_mut_ptr().cast();
        info.len = bytes.len() as isize;
        info.itemsize = bytes.len() as isize;
        info.readonly = 1;
        info.ndim = 0;
        info.format = format.as_mut_ptr();

        let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
        assert!(!view.is_null());
        let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
        assert!(!buffer.is_null());
        assert_eq!(unsafe { (*buffer).ndim }, 0);
        assert_eq!(unsafe { (*buffer).len }, bytes.len() as isize);
        assert_eq!(unsafe { (*buffer).itemsize }, bytes.len() as isize);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
    });
}

#[test]
fn test_memoryview_from_buffer_ignores_foreign_private_internal_pointer() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let mut bytes = [1_u8, 2, 3, 4];
        let mut shape = [bytes.len() as isize];
        let mut strides = [1isize];
        let mut format = [b'B' as c_char, 0];
        let mut info: Py_buffer = unsafe { std::mem::zeroed() };
        info.buf = bytes.as_mut_ptr().cast();
        info.len = bytes.len() as isize;
        info.itemsize = 1;
        info.readonly = 1;
        info.ndim = 1;
        info.format = format.as_mut_ptr();
        info.shape = shape.as_mut_ptr();
        info.strides = strides.as_mut_ptr();
        info.internal = std::ptr::dangling_mut::<c_void>();

        let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
        assert!(!view.is_null());
        let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
        assert!(!buffer.is_null());
        assert_ne!(unsafe { (*buffer).internal }, info.internal);
        assert_eq!(unsafe { (*buffer).len }, bytes.len() as isize);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
    });
}

/// Constructs a memoryview over `data` in ITS OWN stack frame, so any
/// descriptor pointer that (incorrectly) targeted this frame's locals dangles
/// as soon as it returns.
#[inline(never)]
fn build_memoryview_from_memory(data: *mut c_char, len: isize) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromMemory(data, len, PyBUF_READ) }
}

/// Same for the FromBuffer path: the source `Py_buffer` is a FillInfo'd STACK
/// view that is released and dies with this frame — exactly the shape of the
/// reverted `7da58cff8f` field-trick UAF (a self-referential `shape =
/// &view.len` on a stack view that the memoryview then outlived).
#[inline(never)]
fn build_memoryview_from_stack_buffer(data: *mut c_void, len: isize) -> *mut PyObject {
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            &mut info,
            ptr::null_mut(),
            data,
            len,
            1,
            PyBUF_FORMAT | PyBUF_STRIDES,
        )
    };
    assert_eq!(rc, 0);
    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
    // The caller of FromBuffer keeps ownership of the original and releases it
    // exactly once — here, before the stack view goes out of scope.
    unsafe { molt_cpython_abi::api::buffer::PyBuffer_Release(&mut info) };
    view
}

/// Overwrites a stretch of stack so a dangling into-dead-frame pointer reads
/// garbage rather than accidentally-intact values on non-Miri runs.
#[inline(never)]
fn clobber_stack() -> u64 {
    let mut junk = [0u8; 4096];
    let mut acc = 0u64;
    for (i, byte) in junk.iter_mut().enumerate() {
        *byte = (i as u8) ^ 0xA5;
        acc = acc.wrapping_add(u64::from(*byte));
    }
    std::hint::black_box(acc)
}

/// Anti-dangle gate: the C-visible `shape`/`strides`/`format` pointers of a
/// memoryview must remain valid AFTER the constructing stack frame has
/// returned (they must point into the object's own storage — never into a
/// stack `Py_buffer`). Under Miri (Stacked + Tree Borrows) a dangling read
/// here is flagged deterministically; natively, `clobber_stack` makes it
/// fail loudly on values too.
#[test]
fn test_memoryview_descriptor_outlives_constructing_frame() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| {
        let mut data = [7_u8; 32];

        let mv_mem = build_memoryview_from_memory(data.as_mut_ptr().cast(), data.len() as isize);
        assert!(!mv_mem.is_null());
        let mv_buf =
            build_memoryview_from_stack_buffer(data.as_mut_ptr().cast(), data.len() as isize);
        assert!(!mv_buf.is_null());
        std::hint::black_box(clobber_stack());

        for (label, mv) in [("FromMemory", mv_mem), ("FromBuffer", mv_buf)] {
            let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(mv) };
            assert!(!buffer.is_null(), "{label}: GET_BUFFER");
            unsafe {
                assert!(!(*buffer).shape.is_null(), "{label}: shape");
                assert!(!(*buffer).strides.is_null(), "{label}: strides");
                assert!(!(*buffer).format.is_null(), "{label}: format");
                assert_eq!(
                    *(*buffer).shape,
                    data.len() as isize,
                    "{label}: shape[0] read after the constructing frame returned",
                );
                assert_eq!(
                    *(*buffer).strides,
                    1,
                    "{label}: strides[0] read after the constructing frame returned",
                );
                assert_eq!(
                    *(*buffer).format as u8,
                    b'B',
                    "{label}: format read after the constructing frame returned",
                );
                assert_eq!((*buffer).len, data.len() as isize, "{label}: len");
            }
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(mv) };
        }
    });
}

use molt_cpython_abi::api::{buffer, errors, memory, object, refcount, typeobj};

#[test]
fn buffer_acquisition_preserves_released_memoryview_exception() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let mut data = *b"ABCD";
        let view = memory::PyMemoryView_FromMemory(data.as_mut_ptr().cast(), 4, PyBUF_READ);
        assert!(!view.is_null());
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(view)
            .unwrap();
        crate::object::ops_memoryview::molt_memoryview_release(bits);
        assert!(!crate::exception_pending(&py));
        let mut descriptor: Py_buffer = std::mem::zeroed();
        assert_eq!(
            buffer::PyObject_GetBuffer(view, &mut descriptor, PyBUF_SIMPLE),
            -1
        );
        assert_ne!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
            0
        );
        assert!(descriptor.obj.is_null());
        errors::PyErr_Clear();
        crate::dec_ref_bits(&py, bits);
        refcount::Py_DECREF(view);
    });
}

#[test]
fn managed_root_descriptor_reimport_cannot_escalate_write_permission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        for (root, readonly) in [
            (crate::alloc_bytes(&py, b"ABCD"), true),
            (crate::alloc_bytearray(&py, b"ABCD"), false),
        ] {
            assert!(!root.is_null());
            let bits = crate::MoltObject::from_ptr(root).bits();
            let mut descriptor = crate::object::memoryview::MoltBufferView::default();
            assert_eq!(crate::c_api::molt_buffer_acquire(bits, &mut descriptor), 0);
            let imported = crate::c_api::molt_memoryview_from_buffer(&descriptor);
            assert!(!crate::exception_pending(&py));
            let ptr = crate::obj_from_bits(imported).as_ptr().unwrap();
            assert_eq!(crate::memoryview_readonly(ptr), readonly);
            assert_eq!(
                crate::object::memoryview::memoryview_collect_bytes(ptr).unwrap(),
                b"ABCD"
            );
            crate::dec_ref_bits(&py, imported);
            if readonly {
                let mut forged = descriptor;
                forged.readonly = 0;
                let imported = crate::c_api::molt_memoryview_from_buffer(&forged);
                assert!(crate::obj_from_bits(imported).is_none());
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_BufferError).cast()),
                    0
                );
                errors::PyErr_Clear();
            }
            crate::c_api::molt_buffer_release(&mut descriptor);
            crate::dec_ref_bits(&py, bits);
        }
    });
}

#[test]
fn memoryview_snapshot_rejects_null_outputs_and_clears_terminal_geometry() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        for null_output in 0..5 {
            let mut descriptor = molt_cpython_abi::hooks::MoltBufferView::default();
            descriptor.len = 123;
            let mut base = std::ptr::dangling_mut::<PyObject>();
            let mut format = std::ptr::dangling::<u8>();
            let mut format_len = 123;
            assert_eq!(
                super::hook_memoryview_snapshot(
                    crate::MoltObject::none().bits(),
                    if null_output == 0 {
                        ptr::null_mut()
                    } else {
                        &mut descriptor
                    },
                    if null_output == 1 {
                        ptr::null_mut()
                    } else {
                        &mut base
                    },
                    if null_output == 2 {
                        ptr::null_mut()
                    } else {
                        &mut format
                    },
                    if null_output == 3 {
                        ptr::null_mut()
                    } else {
                        &mut format_len
                    }
                ),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast()),
                0
            );
            if null_output != 0 {
                assert_eq!(descriptor.len, 0);
            }
            if null_output != 1 {
                assert!(base.is_null());
            }
            if null_output != 2 {
                assert!(format.is_null());
            }
            if null_output != 3 {
                assert_eq!(format_len, 0);
            }
            errors::PyErr_Clear();
        }
        let mut data = *b"ABCD";
        let view = memory::PyMemoryView_FromMemory(data.as_mut_ptr().cast(), 4, PyBUF_READ);
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(view)
            .unwrap();
        let mut descriptor = molt_cpython_abi::hooks::MoltBufferView::default();
        let mut base = ptr::null_mut();
        let mut format = ptr::null();
        let mut format_len = 0;
        assert_eq!(
            super::hook_memoryview_snapshot(
                bits,
                &mut descriptor,
                &mut base,
                &mut format,
                &mut format_len
            ),
            0
        );
        assert_eq!(descriptor.len, 4);
        assert_eq!(std::slice::from_raw_parts(format, format_len), b"B");
        crate::object::ops_memoryview::molt_memoryview_release(bits);
        assert_eq!(
            super::hook_memoryview_snapshot(
                bits,
                &mut descriptor,
                &mut base,
                &mut format,
                &mut format_len
            ),
            1
        );
        assert_eq!(descriptor.len, 0);
        assert!(base.is_null());
        assert!(format.is_null());
        assert_eq!(format_len, 0);
        assert!(!crate::exception_pending(&py));
        crate::dec_ref_bits(&py, bits);
        refcount::Py_DECREF(view);
    });
}
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
static EXPORT_RELEASES: AtomicUsize = AtomicUsize::new(0);
static EXPORT_FREES: AtomicUsize = AtomicUsize::new(0);
static SLICE_CALLBACK_PARENT: AtomicU64 = AtomicU64::new(0);
static SLICE_CALLBACK_FAILURE: AtomicUsize = AtomicUsize::new(0);
static EXPORT_RELEASE_ERROR: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct Exporter {
    header: PyObject,
    edge: *mut PyObject,
    data: [u8; 3],
    len: isize,
}

unsafe extern "C" fn exporter_get(object: *mut PyObject, view: *mut Py_buffer, flags: i32) -> i32 {
    unsafe {
        buffer::PyBuffer_FillInfo(
            view,
            object,
            (&raw mut (*object.cast::<Exporter>()).data).cast(),
            (*object.cast::<Exporter>()).len,
            0,
            flags,
        )
    }
}
unsafe extern "C" fn exporter_release(_object: *mut PyObject, _view: *mut Py_buffer) {
    EXPORT_RELEASES.fetch_add(1, Ordering::SeqCst);
    if EXPORT_RELEASE_ERROR.load(Ordering::SeqCst) != 0 {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast(),
                c"exporter release error".as_ptr(),
            )
        };
    }
}

extern "C" fn release_native_slice_parent(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let parent = SLICE_CALLBACK_PARENT.load(Ordering::SeqCst);
        if parent != 0 {
            crate::molt_memoryview_release(parent);
            assert!(!crate::exception_pending(py));
        }
        // The caller dropped its exporter reference before conversion. Only
        // the derived pin or assignment source export keeps the lease alive.
        assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 0);
        assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 0);
        if SLICE_CALLBACK_FAILURE.load(Ordering::SeqCst) != 0 {
            return crate::raise_exception(py, "LookupError", "native slice callback");
        }
        crate::MoltObject::from_int(0).bits()
    })
}

fn native_slice_index(py: &crate::PyToken<'_>) -> u64 {
    let name = crate::attr_name_bits_from_bytes(py, b"NativeSliceIndex").unwrap();
    let class = crate::molt_class_new(name);
    crate::molt_class_set_base(class, crate::builtin_classes(py).object);
    let method = crate::attr_name_bits_from_bytes(py, b"__index__").unwrap();
    let function = crate::alloc_runtime_function_obj(
        py,
        crate::runtime_fn_addr(
            "release_native_slice_parent",
            release_native_slice_parent as *const (),
        ),
        1,
    );
    assert!(!function.is_null());
    let function = crate::MoltObject::from_ptr(function).bits();
    crate::molt_set_attr_name(class, method, function);
    let class_ptr = crate::obj_from_bits(class).as_ptr().unwrap();
    unsafe { crate::object::class_finish_definition(py, class_ptr).unwrap() };
    let size = unsafe { crate::object::layout::class_cached_layout_size(class_ptr).unwrap() };
    let instance = crate::object::builders::alloc_class_instance(py, size, class);
    unsafe {
        crate::object::gc::gc_publish_initialized(
            py,
            crate::obj_from_bits(instance).as_ptr().unwrap(),
        )
    };
    for bits in [name, method, function, class] {
        crate::dec_ref_bits(py, bits);
    }
    instance
}
unsafe extern "C" fn exporter_traverse(
    object: *mut PyObject,
    callback: *mut c_void,
    context: *mut c_void,
) -> i32 {
    let edge = unsafe { (*object.cast::<Exporter>()).edge };
    if edge.is_null() {
        return 0;
    }
    let visit: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(callback) };
    unsafe { visit(edge, context) }
}
unsafe extern "C" fn exporter_clear(object: *mut PyObject) -> i32 {
    let edge =
        unsafe { std::mem::replace(&mut (*object.cast::<Exporter>()).edge, ptr::null_mut()) };
    unsafe { refcount::Py_XDECREF(edge) };
    0
}
unsafe extern "C" fn exporter_dealloc(object: *mut PyObject) {
    unsafe {
        memory::PyObject_GC_UnTrack(object.cast());
        exporter_clear(object);
        EXPORT_FREES.fetch_add(1, Ordering::SeqCst);
        memory::PyObject_GC_Del(object.cast());
    }
}

#[test]
fn native_memoryview_lease_traces_cycles_and_shared_buffer_consumers() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        EXPORT_RELEASES.store(0, Ordering::SeqCst);
        EXPORT_FREES.store(0, Ordering::SeqCst);
        let mut class = super::native_test_fixture::NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"BufferCycleExporter",
        );
        class.tp_basicsize = std::mem::size_of::<Exporter>() as isize;
        class.tp_flags |= Py_TPFLAGS_HAVE_GC;
        class.tp_traverse = Some(exporter_traverse);
        class.tp_clear = Some(exporter_clear);
        class.tp_dealloc = Some(exporter_dealloc);
        let mut slots = PyBufferProcs {
            bf_getbuffer: exporter_get as *mut c_void,
            bf_releasebuffer: exporter_release as *mut c_void,
        };
        class.tp_as_buffer = (&raw mut slots).cast();
        assert_eq!(class.ready(), 0);
        let exporter = typeobj::PyType_GenericAlloc(&raw mut *class, 0);
        assert!(!exporter.is_null());
        (*exporter.cast::<Exporter>()).data = *b"ABC";
        (*exporter.cast::<Exporter>()).len = 3;
        let source = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(exporter)
            .unwrap();
        // The shared runtime buffer API acquires foreign exporters too.
        let mut descriptor = crate::object::memoryview::MoltBufferView::default();
        assert_eq!(
            crate::c_api::molt_buffer_acquire(source, &mut descriptor),
            0
        );
        assert_eq!(
            std::slice::from_raw_parts(descriptor.data, descriptor.len as usize),
            b"ABC"
        );
        let imported = crate::c_api::molt_memoryview_from_buffer(&descriptor);
        assert!(!crate::exception_pending(&py));
        crate::c_api::molt_buffer_release(&mut descriptor);
        crate::dec_ref_bits(&py, source);
        assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 0);
        let imported_ptr = crate::obj_from_bits(imported).as_ptr().unwrap();
        assert_eq!(
            crate::object::memoryview::memoryview_collect_bytes(imported_ptr).unwrap(),
            b"ABC"
        );
        crate::dec_ref_bits(&py, imported);
        assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
        // Native edge -> runtime view -> native lease owner is one mixed cycle.
        let view = memory::PyMemoryView_FromObject(exporter);
        assert!(!view.is_null());
        refcount::Py_INCREF(view);
        (*exporter.cast::<Exporter>()).edge = view;
        let address = exporter.addr();
        refcount::Py_DECREF(view);
        refcount::Py_DECREF(exporter);
        assert_eq!(
            crate::object::gc::collect_cycles(&py).status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert!(!crate::object::gc::native_gc_is_enrolled(address));
        assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 2);
        assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn native_memoryview_slice_family_retains_lease_until_last_release() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let mut class = super::native_test_fixture::NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"BufferSliceExporter",
        );
        class.tp_basicsize = std::mem::size_of::<Exporter>() as isize;
        class.tp_flags |= Py_TPFLAGS_HAVE_GC;
        class.tp_traverse = Some(exporter_traverse);
        class.tp_clear = Some(exporter_clear);
        class.tp_dealloc = Some(exporter_dealloc);
        let mut slots = PyBufferProcs {
            bf_getbuffer: exporter_get as *mut c_void,
            bf_releasebuffer: exporter_release as *mut c_void,
        };
        class.tp_as_buffer = (&raw mut slots).cast();
        assert_eq!(class.ready(), 0);
        // Ordinary, empty, stepped, C-API, nested and callback-release slices.
        // Callback failures cover native-lease and real counted-owner cleanup.
        for (path, expected) in [
            b"AB".as_slice(),
            b"",
            b"CBA",
            b"CBA",
            b"CB",
            b"AB",
            b"",
            b"",
        ]
        .into_iter()
        .enumerate()
        {
            EXPORT_RELEASES.store(0, Ordering::SeqCst);
            EXPORT_FREES.store(0, Ordering::SeqCst);
            EXPORT_RELEASE_ERROR.store(0, Ordering::SeqCst);
            let exporter = typeobj::PyType_GenericAlloc(&raw mut *class, 0);
            assert!(!exporter.is_null());
            (*exporter.cast::<Exporter>()).data = *b"ABC";
            (*exporter.cast::<Exporter>()).len = 3;
            let (source, native_view) = if path == 7 {
                let foreign = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_value_for_pyobj(exporter)
                    .unwrap();
                let mut descriptor = crate::object::memoryview::MoltBufferView::default();
                assert_eq!(
                    crate::c_api::molt_buffer_acquire(foreign, &mut descriptor),
                    0
                );
                let format = crate::alloc_string(&py, b"B");
                assert!(!format.is_null());
                let format_bits = crate::MoltObject::from_ptr(format).bits();
                // Keep the acquired descriptor's actual counted owner. The C
                // importer intentionally normalizes this owner to native_lease
                // and therefore cannot exercise the counted-owner Drop arm.
                let storage = crate::object::memoryview::TypedStridedStorage::new(
                    descriptor.data,
                    false,
                    1,
                    0,
                    0,
                    format_bits,
                    descriptor.shape[..1].to_vec(),
                    descriptor.strides[..1].to_vec(),
                )
                .unwrap()
                .with_owner(descriptor.owner);
                let view = crate::object::builders::alloc_memoryview_from_storage(&py, storage);
                assert!(!view.is_null());
                crate::c_api::molt_buffer_release(&mut descriptor);
                crate::dec_ref_bits(&py, format_bits);
                crate::dec_ref_bits(&py, foreign);
                (crate::MoltObject::from_ptr(view).bits(), ptr::null_mut())
            } else {
                let native_view = memory::PyMemoryView_FromObject(exporter);
                assert!(!native_view.is_null());
                let source = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_value_for_pyobj(native_view)
                    .unwrap();
                (source, native_view)
            };
            let source_ptr = crate::obj_from_bits(source).as_ptr().unwrap();
            assert_eq!(crate::memoryview_base_bits(source_ptr), 0);
            assert_eq!(crate::memoryview_owner_bits(source_ptr) != 0, path == 7);
            let none = crate::MoltObject::none().bits();
            let zero = crate::MoltObject::from_int(0).bits();
            let two = crate::MoltObject::from_int(2).bits();
            refcount::Py_DECREF(exporter);
            let derived = match path {
                0 => crate::object::ops_builtins::molt_slice(source, zero, two),
                1 => crate::object::ops_builtins::molt_slice(source, two, zero),
                2 | 4 => {
                    let key =
                        crate::molt_slice_new(none, none, crate::MoltObject::from_int(-1).bits());
                    let reversed = crate::object::ops::molt_index(source, key);
                    crate::dec_ref_bits(&py, key);
                    if path == 4 {
                        let result = crate::object::ops_builtins::molt_slice(reversed, zero, two);
                        crate::dec_ref_bits(&py, reversed);
                        result
                    } else {
                        reversed
                    }
                }
                3 => {
                    let step = molt_cpython_abi::api::numbers::PyLong_FromLong(-1);
                    let key = molt_cpython_abi::api::slice::PySlice_New(
                        ptr::null_mut(),
                        ptr::null_mut(),
                        step,
                    );
                    refcount::Py_DECREF(step);
                    let result = object::PyObject_GetItem(native_view, key);
                    refcount::Py_DECREF(key);
                    assert!(!result.is_null());
                    let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .molt_value_for_pyobj(result)
                        .unwrap();
                    refcount::Py_DECREF(result);
                    bits
                }
                5 | 6 | 7 => {
                    SLICE_CALLBACK_PARENT.store(source, Ordering::SeqCst);
                    SLICE_CALLBACK_FAILURE.store(usize::from(path >= 6), Ordering::SeqCst);
                    EXPORT_RELEASE_ERROR.store(usize::from(path >= 6), Ordering::SeqCst);
                    let index = native_slice_index(&py);
                    let result = crate::object::ops_builtins::molt_slice(source, index, two);
                    crate::dec_ref_bits(&py, index);
                    SLICE_CALLBACK_PARENT.store(0, Ordering::SeqCst);
                    result
                }
                _ => unreachable!(),
            };
            if path >= 6 {
                assert!(crate::exception_pending(&py));
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    &py,
                    error,
                    "LookupError"
                ));
                assert_eq!(
                    crate::builtins::exceptions::format_exception_message(
                        &py,
                        crate::obj_from_bits(error).as_ptr().unwrap(),
                    ),
                    "native slice callback"
                );
                crate::clear_exception(&py);
                crate::dec_ref_bits(&py, error);
                assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
                assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
                EXPORT_RELEASE_ERROR.store(0, Ordering::SeqCst);
                crate::dec_ref_bits(&py, source);
                refcount::Py_XDECREF(native_view);
                assert!(errors::PyErr_Occurred().is_null());
                assert!(!crate::exception_pending(&py));
                continue;
            }
            assert!(!crate::exception_pending(&py));
            assert!(crate::obj_from_bits(derived).as_ptr().is_some());
            crate::object::ops_memoryview::molt_memoryview_release(source);
            crate::dec_ref_bits(&py, source);
            refcount::Py_XDECREF(native_view);
            // Check ownership before dereferencing storage: the old path would
            // release and free the native exporter at the parent's release.
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 0, "path {path}");
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 0, "path {path}");
            let derived_ptr = crate::obj_from_bits(derived).as_ptr().unwrap();
            assert_eq!(
                crate::object::memoryview::memoryview_collect_bytes(derived_ptr).unwrap(),
                expected,
            );
            crate::object::ops_memoryview::molt_memoryview_release(derived);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1, "path {path}");
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1, "path {path}");
            crate::dec_ref_bits(&py, derived);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }

        // Assignment releases its acquired source on both success and callback
        // failure. A hostile native releaser must replace neither outcome.
        for failure in [false, true] {
            EXPORT_RELEASES.store(0, Ordering::SeqCst);
            EXPORT_FREES.store(0, Ordering::SeqCst);
            EXPORT_RELEASE_ERROR.store(1, Ordering::SeqCst);
            SLICE_CALLBACK_PARENT.store(0, Ordering::SeqCst);
            SLICE_CALLBACK_FAILURE.store(usize::from(failure), Ordering::SeqCst);
            let exporter = typeobj::PyType_GenericAlloc(&raw mut *class, 0);
            assert!(!exporter.is_null());
            (*exporter.cast::<Exporter>()).data = *b"ABC";
            (*exporter.cast::<Exporter>()).len = 2;
            let source = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(exporter)
                .unwrap();
            refcount::Py_DECREF(exporter);
            let owner = crate::alloc_bytearray(&py, b"..!");
            assert!(!owner.is_null());
            let owner_bits = crate::MoltObject::from_ptr(owner).bits();
            let destination = crate::molt_memoryview_new(owner_bits);
            let index = native_slice_index(&py);
            let key = crate::molt_slice_new(
                index,
                crate::MoltObject::from_int(2).bits(),
                crate::MoltObject::none().bits(),
            );
            crate::molt_store_index(destination, key, source);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
            if failure {
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    &py,
                    error,
                    "LookupError"
                ));
                assert_eq!(
                    crate::builtins::exceptions::format_exception_message(
                        &py,
                        crate::obj_from_bits(error).as_ptr().unwrap()
                    ),
                    "native slice callback"
                );
                crate::clear_exception(&py);
                crate::dec_ref_bits(&py, error);
            } else {
                assert!(!crate::exception_pending(&py));
            }
            assert_eq!(
                crate::object::memoryview::bytes_like_slice_raw(owner).unwrap(),
                if failure { b"..!" } else { b"AB!" }
            );
            for bits in [key, index, destination, owner_bits, source] {
                crate::dec_ref_bits(&py, bits);
            }
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }
        SLICE_CALLBACK_FAILURE.store(0, Ordering::SeqCst);
        EXPORT_RELEASE_ERROR.store(0, Ordering::SeqCst);
    });
}

#[test]
fn c_memoryview_reverse_slice_survives_source_release() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|_py| unsafe {
        let mut data = *b"ABCD";
        let view = memory::PyMemoryView_FromMemory(data.as_mut_ptr().cast(), 4, PyBUF_WRITE);
        assert!(!view.is_null());
        let step = molt_cpython_abi::api::numbers::PyLong_FromLong(-1);
        let slice =
            molt_cpython_abi::api::slice::PySlice_New(ptr::null_mut(), ptr::null_mut(), step);
        refcount::Py_DECREF(step);
        let derived = object::PyObject_GetItem(view, slice);
        refcount::Py_DECREF(slice);
        assert!(!derived.is_null());
        let method = object::PyObject_GetAttrString(view, c"release".as_ptr());
        assert!(!method.is_null());
        let released = object::PyObject_CallNoArgs(method);
        refcount::Py_DECREF(method);
        assert!(!released.is_null());
        refcount::Py_DECREF(released);
        refcount::Py_DECREF(view);
        let mut exported: Py_buffer = std::mem::zeroed();
        assert_eq!(
            buffer::PyObject_GetBuffer(derived, &mut exported, PyBUF_FULL_RO),
            0
        );
        assert_eq!(*exported.strides, -1);
        assert_eq!(*exported.buf.cast::<u8>(), b'D');
        buffer::PyBuffer_Release(&mut exported);
        let bytes = object::PyObject_Bytes(derived);
        assert!(!bytes.is_null());
        assert_eq!(
            std::slice::from_raw_parts(
                molt_cpython_abi::api::strings::PyBytes_AsString(bytes).cast::<u8>(),
                4
            ),
            b"DCBA"
        );
        refcount::Py_DECREF(bytes);
        let derived_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(derived)
            .unwrap();
        let mut descriptor = crate::object::memoryview::MoltBufferView::default();
        assert_eq!(
            crate::c_api::molt_buffer_acquire(derived_bits, &mut descriptor),
            0
        );
        let imported = crate::c_api::molt_memoryview_from_buffer(&descriptor);
        assert!(!crate::exception_pending(&_py));
        crate::c_api::molt_buffer_release(&mut descriptor);
        crate::dec_ref_bits(&_py, derived_bits);
        refcount::Py_DECREF(derived);
        let imported_ptr = crate::obj_from_bits(imported).as_ptr().unwrap();
        assert_eq!(
            crate::object::memoryview::memoryview_collect_bytes(imported_ptr).unwrap(),
            b"DCBA"
        );
        crate::dec_ref_bits(&_py, imported);
    });
}

#[test]
fn memoryview_descriptor_reimport_preserves_readonly_and_source_bounds() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let mut data = *b"ABCD";
        let view = memory::PyMemoryView_FromMemory(data.as_mut_ptr().cast(), 4, PyBUF_READ);
        assert!(!view.is_null());
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(view)
            .unwrap();
        let mut descriptor = crate::object::memoryview::MoltBufferView::default();
        assert_eq!(crate::c_api::molt_buffer_acquire(bits, &mut descriptor), 0);
        for forged in 0..3 {
            let mut candidate = descriptor;
            match forged {
                0 => candidate.readonly = 0,
                1 => {
                    candidate.data = candidate.data.wrapping_add(1);
                    candidate.backing_capacity = u64::MAX;
                }
                _ => candidate.offset = 1,
            }
            let result = crate::c_api::molt_memoryview_from_buffer(&candidate);
            assert!(
                crate::exception_pending(&py),
                "accepted forged descriptor {forged}"
            );
            assert!(crate::obj_from_bits(result).is_none());
            errors::PyErr_Clear();
        }
        crate::c_api::molt_buffer_release(&mut descriptor);
        crate::dec_ref_bits(&py, bits);
        refcount::Py_DECREF(view);
    });
}
