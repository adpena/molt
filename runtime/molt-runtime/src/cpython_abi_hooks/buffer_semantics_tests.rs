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
/// reverted `2d287ebb17` field-trick UAF (a self-referential `shape =
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
            let mut descriptor = molt_cpython_abi::hooks::MoltBufferView {
                len: 123,
                ..Default::default()
            };
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
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
static EXPORT_RELEASES: AtomicUsize = AtomicUsize::new(0);
static EXPORT_FREES: AtomicUsize = AtomicUsize::new(0);
static SLICE_CALLBACK_PARENT: AtomicU64 = AtomicU64::new(0);
static SLICE_CALLBACK_FAILURE: AtomicUsize = AtomicUsize::new(0);
// Borrowed observations: the raised slot/test owns the exception throughout.
static EXPORT_RELEASE_OBSERVED_ERROR: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static SLICE_CALLBACK_ERROR_BITS: AtomicU64 = AtomicU64::new(0);

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
    unsafe {
        // A lawful native releaser observes and restores the exact raised owner.
        let error = errors::PyErr_GetRaisedException();
        EXPORT_RELEASE_OBSERVED_ERROR.store(error, Ordering::SeqCst);
        errors::PyErr_SetRaisedException(error);
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
            let result = crate::raise_exception(py, "LookupError", "native slice callback");
            SLICE_CALLBACK_ERROR_BITS.store(
                crate::exception_last_bits_noinc(py).unwrap_or(0),
                Ordering::SeqCst,
            );
            return result;
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
            EXPORT_RELEASE_OBSERVED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            SLICE_CALLBACK_ERROR_BITS.store(0, Ordering::SeqCst);
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
                5..=7 => {
                    SLICE_CALLBACK_PARENT.store(source, Ordering::SeqCst);
                    SLICE_CALLBACK_FAILURE.store(usize::from(path >= 6), Ordering::SeqCst);
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
                let error_owner = crate::builtins::exceptions::ExceptionValue::adopt(
                    &py,
                    crate::builtins::exceptions::molt_exception_last_pending(),
                );
                let error = error_owner.bits();
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
                assert_eq!(error, SLICE_CALLBACK_ERROR_BITS.load(Ordering::SeqCst));
                let raised =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!raised.as_ptr().is_null());
                assert_eq!(
                    raised.as_ptr(),
                    EXPORT_RELEASE_OBSERVED_ERROR.load(Ordering::SeqCst)
                );
                crate::clear_exception(&py);
                drop(error_owner);
                assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
                assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
                EXPORT_RELEASE_OBSERVED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
                SLICE_CALLBACK_ERROR_BITS.store(0, Ordering::SeqCst);
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
        // failure. A lawful native releaser observes and restores either outcome.
        for failure in [false, true] {
            EXPORT_RELEASES.store(0, Ordering::SeqCst);
            EXPORT_FREES.store(0, Ordering::SeqCst);
            EXPORT_RELEASE_OBSERVED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            SLICE_CALLBACK_ERROR_BITS.store(0, Ordering::SeqCst);
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
                let error_owner = crate::builtins::exceptions::ExceptionValue::adopt(
                    &py,
                    crate::builtins::exceptions::molt_exception_last_pending(),
                );
                let error = error_owner.bits();
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
                assert_eq!(error, SLICE_CALLBACK_ERROR_BITS.load(Ordering::SeqCst));
                let raised =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!raised.as_ptr().is_null());
                assert_eq!(
                    raised.as_ptr(),
                    EXPORT_RELEASE_OBSERVED_ERROR.load(Ordering::SeqCst)
                );
                crate::clear_exception(&py);
                drop(error_owner);
            } else {
                assert!(!crate::exception_pending(&py));
                assert!(
                    EXPORT_RELEASE_OBSERVED_ERROR
                        .load(Ordering::SeqCst)
                        .is_null()
                );
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
        EXPORT_RELEASE_OBSERVED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
        SLICE_CALLBACK_ERROR_BITS.store(0, Ordering::SeqCst);
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

#[test]
fn buffer_export_contract_memoryview_geometry_flags_and_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        // Literal layout expectations follow CPython memoryobject.c::init_flags,
        // not PyBuffer_IsContiguous: its empty rank-one rule is different.
        let layouts: &[(&str, usize, &[isize], &[isize], usize, bool, bool)] = &[
            ("scalar", 1, &[], &[], 1, true, true),
            ("empty-linear", 1, &[0], &[1], 0, true, true),
            ("empty-strided", 1, &[0], &[2], 0, false, false),
            ("singleton-strided", 1, &[1], &[2], 1, true, true),
            ("nonempty-strided", 1, &[2], &[2], 2, false, false),
            ("typed-singleton", 2, &[1], &[4], 2, true, true),
            ("empty-multidimensional", 1, &[0, 2], &[4, 1], 0, true, true),
            ("C-only", 1, &[2, 2], &[2, 1], 4, true, false),
            ("F-only", 1, &[2, 2], &[1, 2], 4, false, true),
        ];
        for &(label, itemsize, shape, strides, len, c_contiguous, f_contiguous) in layouts {
            let base = crate::alloc_bytearray(&py, b"01234567");
            assert!(!base.is_null());
            let base_bits = crate::MoltObject::from_ptr(base).bits();
            let format = crate::alloc_string(&py, if itemsize == 2 { b"H" } else { b"B" });
            assert!(!format.is_null());
            let format_bits = crate::MoltObject::from_ptr(format).bits();
            let storage = crate::object::memoryview::TypedStridedStorage::new(
                crate::object::buffer_exports::bytearray_data(base).0,
                false,
                itemsize,
                0,
                base_bits,
                format_bits,
                shape.to_vec(),
                strides.to_vec(),
            )
            .unwrap();
            let view = crate::object::builders::alloc_memoryview_from_storage(&py, storage);
            assert!(!view.is_null(), "{label}");
            let bits = crate::MoltObject::from_ptr(view).bits();
            let object = molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
            assert!(!object.is_null(), "{label}");
            assert_eq!(memory::PyMemoryView_Check(object), 1);
            // FULL_RO exposes geometry; the generic query accepts every empty
            // descriptor even when this memoryview cannot export SIMPLE/ND.
            let mut full: Py_buffer = std::mem::zeroed();
            assert_eq!(
                buffer::PyObject_GetBuffer(object, &mut full, PyBUF_FULL_RO),
                0
            );
            assert_eq!(
                buffer::PyBuffer_IsContiguous(&full, b'C' as c_char) != 0,
                len == 0 || c_contiguous,
                "{label}"
            );
            assert_eq!(
                buffer::PyBuffer_IsContiguous(&full, b'F' as c_char) != 0,
                len == 0 || f_contiguous,
                "{label}"
            );
            buffer::PyBuffer_Release(&mut full);
            // Each row independently specifies admission and the public fields
            // requested. In particular ND is not a STRIDES request.
            for (request, flags, accept, has_shape, has_strides, has_format) in [
                ("simple", PyBUF_SIMPLE, c_contiguous, false, false, false),
                (
                    "writable",
                    PyBUF_WRITABLE,
                    c_contiguous,
                    false,
                    false,
                    false,
                ),
                ("ND", PyBUF_ND, c_contiguous, true, false, false),
                (
                    "ND-format",
                    PyBUF_ND | PyBUF_FORMAT,
                    c_contiguous,
                    true,
                    false,
                    true,
                ),
                ("strides", PyBUF_STRIDES, true, true, true, false),
                ("C", PyBUF_C_CONTIGUOUS, c_contiguous, true, true, false),
                ("F", PyBUF_F_CONTIGUOUS, f_contiguous, true, true, false),
                (
                    "any",
                    PyBUF_ANY_CONTIGUOUS,
                    c_contiguous || f_contiguous,
                    true,
                    true,
                    false,
                ),
                ("format-only", PyBUF_FORMAT, false, false, false, true),
                ("full", PyBUF_FULL, true, true, true, true),
            ] {
                let mut exported: Py_buffer = std::mem::zeroed();
                let status = buffer::PyObject_GetBuffer(object, &mut exported, flags);
                if !accept {
                    assert_eq!(status, -1, "{label}/{request}");
                    assert_ne!(
                        errors::PyErr_ExceptionMatches((&raw mut PyExc_BufferError).cast()),
                        0,
                        "{label}/{request}"
                    );
                    assert!(
                        exported.obj.is_null(),
                        "failed export published an owner: {label}/{request}"
                    );
                    let expected_message = match request {
                        "F" => "memoryview: underlying buffer is not Fortran contiguous",
                        "any" => "memoryview: underlying buffer is not contiguous",
                        "format-only" if c_contiguous => {
                            "memoryview: cannot cast to unsigned bytes if the format flag is present"
                        }
                        _ => "memoryview: underlying buffer is not C-contiguous",
                    };
                    // The C API may publish a native exception without a
                    // managed pending value. Inspect through that same boundary.
                    let error =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!error.as_ptr().is_null(), "{label}/{request}");
                    let message =
                        refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr()));
                    assert!(!message.as_ptr().is_null(), "{label}/{request}");
                    let text = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(message.as_ptr());
                    assert!(!text.is_null(), "{label}/{request}");
                    assert_eq!(
                        std::ffi::CStr::from_ptr(text).to_bytes(),
                        expected_message.as_bytes(),
                        "{label}/{request}",
                    );
                    drop(message);
                    drop(error);
                    assert!(errors::PyErr_Occurred().is_null(), "{label}/{request}");
                    assert!(!crate::exception_pending(&py), "{label}/{request}");
                    continue;
                }
                assert_eq!(status, 0, "{label}/{request}");
                assert!(errors::PyErr_Occurred().is_null(), "{label}/{request}");
                assert_eq!(exported.len, len as isize, "{label}/{request}");
                assert_eq!(exported.itemsize, itemsize as isize, "{label}/{request}");
                assert_eq!(
                    exported.ndim,
                    if has_shape { shape.len() as i32 } else { 1 },
                    "{label}/{request}"
                );
                assert_eq!(
                    !exported.shape.is_null(),
                    has_shape && !shape.is_empty(),
                    "{label}/{request}"
                );
                assert_eq!(
                    !exported.strides.is_null(),
                    has_strides && !shape.is_empty(),
                    "{label}/{request}"
                );
                assert_eq!(!exported.format.is_null(), has_format, "{label}/{request}");
                assert!(exported.suboffsets.is_null());
                if has_shape && !shape.is_empty() {
                    assert_eq!(
                        std::slice::from_raw_parts(exported.shape, shape.len()),
                        shape
                    );
                }
                if has_strides && !shape.is_empty() {
                    assert_eq!(
                        std::slice::from_raw_parts(exported.strides, strides.len()),
                        strides
                    );
                }
                buffer::PyBuffer_Release(&mut exported);
            }
            // All successful and refused exports must have retired their pin.
            crate::object::ops_memoryview::molt_memoryview_release(bits);
            assert!(!crate::exception_pending(&py), "export leak: {label}");
            crate::molt_bytearray_resize(base_bits, crate::MoltObject::from_int(9).bits());
            assert!(
                !crate::exception_pending(&py),
                "backing export leak: {label}"
            );
            refcount::Py_DECREF(object);
            for value in [bits, format_bits, base_bits] {
                crate::dec_ref_bits(&py, value);
            }
        }
        // FORMAT without ND is legal for bytes/bytearray's FillInfo contract;
        // the new memoryview refusal must not become a blanket buffer rule.
        for mutable in [false, true] {
            let value = if mutable {
                crate::alloc_bytearray(&py, b"AB")
            } else {
                crate::alloc_bytes(&py, b"AB")
            };
            assert!(!value.is_null());
            let bits = crate::MoltObject::from_ptr(value).bits();
            let object = molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits);
            assert!(!object.is_null());
            let mut exported: Py_buffer = std::mem::zeroed();
            assert_eq!(
                buffer::PyObject_GetBuffer(object, &mut exported, PyBUF_FORMAT),
                0
            );
            assert_eq!(*exported.format as u8, b'B');
            assert_eq!(exported.ndim, 1);
            assert!(exported.shape.is_null() && exported.strides.is_null());
            buffer::PyBuffer_Release(&mut exported);
            if !mutable {
                assert_eq!(
                    buffer::PyObject_GetBuffer(object, &mut exported, PyBUF_WRITABLE),
                    -1
                );
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_BufferError).cast()),
                    0
                );
                assert!(exported.obj.is_null());
                errors::PyErr_Clear();
            }
            refcount::Py_DECREF(object);
            crate::dec_ref_bits(&py, bits);
        }
    });
}

#[repr(C)]
struct TypedFormatExporter {
    header: PyObject,
    data: [u8; 10],
    format: *const c_char,
    first: usize,
    itemsize: isize,
    shape: isize,
    stride: isize,
}

unsafe extern "C" fn typed_format_exporter_get(
    object: *mut PyObject,
    view: *mut Py_buffer,
    flags: i32,
) -> i32 {
    unsafe {
        assert_eq!(flags, PyBUF_FULL_RO);
        let source = object.cast::<TypedFormatExporter>();
        let status = buffer::PyBuffer_FillInfo(
            view,
            object,
            (&raw mut (*source).data)
                .cast::<u8>()
                .add((*source).first)
                .cast(),
            (*source).shape * (*source).itemsize,
            1,
            flags,
        );
        if status == 0 {
            (*view).format = (*source).format.cast_mut();
            (*view).itemsize = (*source).itemsize;
            (*view).shape = &raw mut (*source).shape;
            (*view).strides = &raw mut (*source).stride;
        }
        status
    }
}

unsafe extern "C" fn typed_format_exporter_dealloc(object: *mut PyObject) {
    EXPORT_FREES.fetch_add(1, Ordering::SeqCst);
    unsafe { memory::PyObject_Free(object.cast()) };
}

// Only evaluated by a failed buffer assertion. Read the selected C error
// owner so a native-only pending exception is not mistaken for managed state.
unsafe fn pending_buffer_error() -> String {
    unsafe {
        let class = errors::PyErr_Occurred();
        let raised = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        if raised.as_ptr().is_null() {
            return format!("class={class:p}, no raised exception instance");
        }
        let message = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(raised.as_ptr()));
        let text = if message.as_ptr().is_null() {
            ptr::null()
        } else {
            molt_cpython_abi::api::strings::PyUnicode_AsUTF8(message.as_ptr())
        };
        let rendered = if text.is_null() {
            "<exception text unavailable>".into()
        } else {
            std::ffi::CStr::from_ptr(text).to_string_lossy()
        };
        format!(
            "class={class:p}, instance={:p}, message={rendered}",
            raised.as_ptr()
        )
    }
}

#[test]
fn native_buffer_formats_preserve_typed_assignment_comparison_and_retirement() {
    use molt_obj_model::sequence_compare::RichCompareOp;
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let mut class = super::native_test_fixture::NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"TypedFormatExporter",
        );
        class.tp_basicsize = std::mem::size_of::<TypedFormatExporter>() as isize;
        class.tp_dealloc = Some(typed_format_exporter_dealloc);
        let mut slots = PyBufferProcs {
            bf_getbuffer: typed_format_exporter_get as *mut c_void,
            bf_releasebuffer: exporter_release as *mut c_void,
        };
        class.tp_as_buffer = (&raw mut slots).cast();
        assert_eq!(class.ready(), 0);
        // Native h and its @ alias agree; negative strides and signed bytes
        // distinguish addressed span/format from logical nbytes/default B.
        // A full unsupported format must survive both lease and physical pins.
        for (format, itemsize, first, stride, expected, compatible, compare_values) in [
            (c"h", 2usize, 0usize, 4isize, [7i16, 8, 9], true, true),
            (c"@h", 2, 8, -4, [9, 8, 7], true, true),
            (c"b", 1, 0, 2, [-1, 2, -3], true, true),
            (
                c"T{h:member_with_long_name:}",
                2,
                0,
                4,
                [7, 8, 9],
                false,
                true,
            ),
            // Assignment must not erase an explicit non-native prefix.
            // General struct-format equality is an existing separate limit.
            (c">h", 2, 0, 4, [7, 8, 9], false, false),
        ] {
            EXPORT_RELEASES.store(0, Ordering::SeqCst);
            EXPORT_FREES.store(0, Ordering::SeqCst);
            EXPORT_RELEASE_OBSERVED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            let exporter = typeobj::PyType_GenericAlloc(&raw mut *class, 0);
            assert!(!exporter.is_null());
            let typed = exporter.cast::<TypedFormatExporter>();
            (*typed).format = format.as_ptr();
            (*typed).first = first;
            (*typed).itemsize = itemsize as isize;
            (*typed).shape = 3;
            (*typed).stride = stride;
            if itemsize == 2 {
                for (index, value) in [7i16, 99, 8, 99, 9].into_iter().enumerate() {
                    let bytes = value.to_ne_bytes();
                    (*typed).data[index * 2] = bytes[0];
                    (*typed).data[index * 2 + 1] = bytes[1];
                }
            } else {
                (*typed).data = [255, 99, 2, 99, 253, 0, 0, 0, 0, 0];
            }
            let source = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(exporter)
                .unwrap();
            let mut expected_bytes = Vec::new();
            for value in expected {
                if itemsize == 2 {
                    expected_bytes.extend_from_slice(&value.to_ne_bytes());
                } else {
                    expected_bytes.push(value as u8);
                }
            }
            let target_format = crate::MoltObject::from_ptr(crate::alloc_string(
                &py,
                if itemsize == 2 { b"h" } else { b"b" },
            ))
            .bits();
            let make_view = |bytes: &[u8]| {
                let owner = crate::MoltObject::from_ptr(crate::alloc_bytearray(&py, bytes)).bits();
                let byte_view = crate::molt_memoryview_new(owner);
                let view = crate::molt_memoryview_cast(
                    byte_view,
                    target_format,
                    crate::MoltObject::none().bits(),
                    crate::MoltObject::from_bool(false).bits(),
                );
                crate::dec_ref_bits(&py, byte_view);
                assert!(!crate::exception_pending(&py));
                (owner, view)
            };
            let (expected_owner, expected_view) = make_view(&expected_bytes);
            if compare_values {
                for (order, (left, right)) in [(expected_view, source), (source, expected_view)]
                    .into_iter()
                    .enumerate()
                {
                    let releases = EXPORT_RELEASES.load(Ordering::SeqCst);
                    let result = crate::object::ops_compare::molt_eq(left, right);
                    assert!(
                        !crate::exception_pending(&py),
                        "runtime equality: format={format:?}, order={order}: {}",
                        pending_buffer_error()
                    );
                    assert_eq!(result, crate::MoltObject::from_bool(compatible).bits());
                    assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases + 1);
                }
                // The C dispatcher and the declaring slot are distinct
                // consumers. Exercise the mixed native peer in both orders,
                // including NE; a managed-only comparison can bypass the slot.
                let view = refcount::OwnedPyObject::from_owned(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .borrowed_handle_to_new_pyobj(expected_view),
                );
                assert!(!view.as_ptr().is_null());
                let slot = (*(&raw mut PyMemoryView_Type)).tp_richcompare.unwrap();
                for operation in [RichCompareOp::Eq, RichCompareOp::Ne] {
                    let op = operation as i32;
                    let expected = if (operation == RichCompareOp::Eq) == compatible {
                        (&raw mut Py_True).cast::<PyObject>()
                    } else {
                        (&raw mut Py_False).cast::<PyObject>()
                    };
                    for (order, (left, right)) in
                        [(view.as_ptr(), exporter), (exporter, view.as_ptr())]
                            .into_iter()
                            .enumerate()
                    {
                        let releases = EXPORT_RELEASES.load(Ordering::SeqCst);
                        let result = refcount::OwnedPyObject::from_owned(
                            typeobj::PyObject_RichCompare(left, right, op),
                        );
                        assert!(
                            !result.as_ptr().is_null() && !crate::exception_pending(&py),
                            "C equality: format={format:?}, order={order}, op={op}: {}",
                            pending_buffer_error()
                        );
                        assert_eq!(result.as_ptr(), expected);
                        assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases + 1);
                    }
                    let releases = EXPORT_RELEASES.load(Ordering::SeqCst);
                    let result =
                        refcount::OwnedPyObject::from_owned(slot(view.as_ptr(), exporter, op));
                    assert!(
                        !result.as_ptr().is_null() && !crate::exception_pending(&py),
                        "declaring equality: format={format:?}, op={op}: {}",
                        pending_buffer_error()
                    );
                    assert_eq!(result.as_ptr(), expected);
                    assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases + 1);
                }
                // Ordering declines without acquiring a peer export. Public
                // dispatch must then produce its normal TypeError, not recurse.
                for operation in [
                    RichCompareOp::Lt,
                    RichCompareOp::Le,
                    RichCompareOp::Gt,
                    RichCompareOp::Ge,
                ] {
                    let op = operation as i32;
                    let releases = EXPORT_RELEASES.load(Ordering::SeqCst);
                    let declined =
                        refcount::OwnedPyObject::from_owned(slot(view.as_ptr(), exporter, op));
                    assert_eq!(declined.as_ptr(), &raw mut Py_NotImplementedSentinel);
                    assert!(!crate::exception_pending(&py));
                    for (left, right) in [(view.as_ptr(), exporter), (exporter, view.as_ptr())] {
                        let result = typeobj::PyObject_RichCompare(left, right, op);
                        assert!(result.is_null(), "format={format:?}, ordering op={op}");
                        assert_ne!(
                            errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                            0
                        );
                        let error =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert!(!error.as_ptr().is_null());
                        drop(error);
                        assert!(!crate::exception_pending(&py));
                    }
                    assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases);
                }
            }
            let zeros = vec![0u8; 5 * itemsize];
            let (destination_owner, destination) = make_view(&zeros);
            let key = crate::molt_slice_new(
                crate::MoltObject::none().bits(),
                crate::MoltObject::none().bits(),
                crate::MoltObject::from_int(2).bits(),
            );
            let releases = EXPORT_RELEASES.load(Ordering::SeqCst);
            crate::molt_store_index(destination, key, source);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases + 1);
            let mut assigned = zeros;
            if compatible {
                assert!(!crate::exception_pending(&py));
                for index in 0..3 {
                    assigned[index * 2 * itemsize..(index * 2 + 1) * itemsize]
                        .copy_from_slice(&expected_bytes[index * itemsize..(index + 1) * itemsize]);
                }
                assert!(
                    EXPORT_RELEASE_OBSERVED_ERROR
                        .load(Ordering::SeqCst)
                        .is_null()
                );
            } else {
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                    0
                );
                let raised =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(
                    raised.as_ptr(),
                    EXPORT_RELEASE_OBSERVED_ERROR.load(Ordering::SeqCst)
                );
                let message =
                    refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(raised.as_ptr()));
                let text = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(message.as_ptr());
                assert!(!text.is_null());
                assert_eq!(
                    std::ffi::CStr::from_ptr(text).to_bytes(),
                    b"memoryview assignment: lvalue and rvalue have different structures"
                );
                drop(message);
                drop(raised);
                assert!(!crate::exception_pending(&py));
                EXPORT_RELEASE_OBSERVED_ERROR.store(ptr::null_mut(), Ordering::SeqCst);
            }
            assert_eq!(
                crate::object::memoryview::bytes_like_slice_raw(
                    crate::obj_from_bits(destination_owner).as_ptr().unwrap()
                )
                .unwrap(),
                assigned
            );
            for bits in [
                key,
                destination,
                destination_owner,
                expected_view,
                expected_owner,
                target_format,
            ] {
                crate::dec_ref_bits(&py, bits);
            }

            let native_pin = crate::object::buffer_exports::ScopedBuffer::new(&py, source).unwrap();
            let view = crate::molt_memoryview_new(source);
            assert!(!crate::exception_pending(&py));
            let physical_pin = crate::object::buffer_exports::ScopedBuffer::new(&py, view).unwrap();
            crate::dec_ref_bits(&py, view);
            crate::dec_ref_bits(&py, source);
            refcount::Py_DECREF(exporter);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 0);
            assert_eq!(native_pin.format(), format.to_bytes());
            assert_eq!(physical_pin.format(), format.to_bytes());
            let releases = EXPORT_RELEASES.load(Ordering::SeqCst);
            drop(native_pin);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases + 1);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 0);
            drop(physical_pin);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), releases + 2);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }
    });
}

#[test]
fn buffer_rejection_names_the_semantic_type_for_managed_and_native_objects() {
    use molt_cpython_abi::api::{buffer, errors, refcount, strings, typeobj};
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let managed_bits = crate::molt_object_new();
        assert!(!crate::exception_pending(&py));
        let managed = refcount::OwnedPyObject::from_owned(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(managed_bits),
        );
        assert!(!managed.as_ptr().is_null());
        crate::dec_ref_bits(&py, managed_bits);
        let string_bits =
            crate::MoltObject::from_ptr(crate::alloc_string(&py, b"not a buffer")).bits();
        let string = refcount::OwnedPyObject::from_owned(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(string_bits),
        );
        assert!(!string.as_ptr().is_null());
        crate::dec_ref_bits(&py, string_bits);

        let mut slots = [PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        }];
        let mut spec = PyType_Spec {
            name: c"native.NoBuffer".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: Py_TPFLAGS_DEFAULT as u32,
            slots: slots.as_mut_ptr(),
        };
        let native_type = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&mut spec));
        assert!(!native_type.as_ptr().is_null());
        let native = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            native_type.as_ptr().cast(),
            0,
        ));
        assert!(!native.as_ptr().is_null());
        for (object, expected) in [
            (
                managed.as_ptr(),
                "a bytes-like object is required, not 'object'",
            ),
            (
                string.as_ptr(),
                "a bytes-like object is required, not 'str'",
            ),
            (
                native.as_ptr(),
                "a bytes-like object is required, not 'native.NoBuffer'",
            ),
        ] {
            for flags in [PyBUF_SIMPLE, PyBUF_FULL_RO] {
                let count = (*object).ob_refcnt;
                let mut view: Py_buffer = std::mem::zeroed();
                assert_eq!(buffer::PyObject_GetBuffer(object, &mut view, flags), -1);
                assert!(
                    view.obj.is_null(),
                    "rejection cannot publish an export owner"
                );
                assert_eq!((*object).ob_refcnt, count);
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                    0
                );
                let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!error.as_ptr().is_null());
                let message =
                    refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr()));
                assert!(!message.as_ptr().is_null());
                let text = strings::PyUnicode_AsUTF8(message.as_ptr());
                assert!(!text.is_null());
                assert_eq!(
                    std::ffi::CStr::from_ptr(text).to_bytes(),
                    expected.as_bytes()
                );
                drop(message);
                drop(error);
                assert!(errors::PyErr_Occurred().is_null());
                assert!(!crate::exception_pending(&py));
            }
        }
    });
}

#[test]
fn raw_callback_memoryviews_expire_without_extending_exporter_custody() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    fn exercise(inject_failure: bool, writable: bool, pending_error: bool) {
        crate::concurrency::gil::with_gil(|py| unsafe {
            use molt_runtime_core::OwnedRuntimeValue;
            EXPORT_RELEASES.store(0, Ordering::SeqCst);
            EXPORT_FREES.store(0, Ordering::SeqCst);
            // Declaration storage must outlive its class, including assertion unwind.
            let mut slots = PyBufferProcs {
                bf_getbuffer: exporter_get as *mut c_void,
                bf_releasebuffer: exporter_release as *mut c_void,
            };
            let mut class = super::native_test_fixture::NativeType::<PyTypeObject>::subtype(
                &raw mut PyBaseObject_Type,
                c"RawCallbackExporter",
            );
            class.tp_basicsize = std::mem::size_of::<Exporter>() as isize;
            class.tp_flags |= Py_TPFLAGS_HAVE_GC;
            class.tp_traverse = Some(exporter_traverse);
            class.tp_clear = Some(exporter_clear);
            class.tp_dealloc = Some(exporter_dealloc);
            class.tp_as_buffer = (&raw mut slots).cast();
            assert_eq!(class.ready(), 0);
            let exporter_owner = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut *class,
                0,
            ));
            let exporter = exporter_owner.as_ptr();
            assert!(!exporter.is_null());
            (*exporter.cast::<Exporter>()).data = *b"ABC";
            (*exporter.cast::<Exporter>()).len = 3;
            let source = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(exporter)
                .unwrap();
            let source_owner = OwnedRuntimeValue::from_owned_bits(py.core_token(), source);
            let input = (!writable)
                .then(|| crate::object::buffer_exports::ScopedBuffer::simple(&py, source).unwrap());
            let mut writable_input = writable.then(|| {
                crate::object::buffer_exports::ScopedWritableBuffer::new(&py, source).unwrap()
            });
            let scope = if let Some(input) = writable_input.as_mut() {
                input.writable_subview(0, 3).unwrap()
            } else {
                input.as_ref().unwrap().readonly_subview(0, 3).unwrap()
            };
            let root = scope.bits();
            let root_ptr = crate::obj_from_bits(root).as_ptr().unwrap();
            assert_eq!(crate::memoryview_base_bits(root_ptr), 0);
            assert_eq!(crate::memoryview_readonly(root_ptr), !writable);
            let clone_owner = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::molt_memoryview_new(root),
            );
            let clone = clone_owner.bits();
            let readonly_owner = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::molt_memoryview_toreadonly(root),
            );
            let format_owner = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::MoltObject::from_ptr(crate::alloc_string(&py, b"B")).bits(),
            );
            let format = format_owner.bits();
            let none = crate::MoltObject::none().bits();
            let cast = crate::molt_memoryview_cast(
                root,
                format,
                none,
                crate::MoltObject::from_bool(false).bits(),
            );
            let cast_owner = OwnedRuntimeValue::from_owned_bits(py.core_token(), cast);
            // This existing real __index__ callback releases the parent before the
            // slice allocator publishes its already-pinned descendant.
            SLICE_CALLBACK_PARENT.store(root, Ordering::SeqCst);
            SLICE_CALLBACK_FAILURE.store(0, Ordering::SeqCst);
            let index_owner =
                OwnedRuntimeValue::from_owned_bits(py.core_token(), native_slice_index(&py));
            let index = index_owner.bits();
            let sliced =
                crate::object::memoryview::memoryview_slice(&py, root_ptr, index, none, none);
            SLICE_CALLBACK_PARENT.store(0, Ordering::SeqCst);
            let sliced_owner = OwnedRuntimeValue::from_owned_bits(py.core_token(), sliced);
            assert!(!crate::exception_pending(&py));
            assert!(crate::memoryview_released(root_ptr));
            assert_eq!((*crate::memoryview_ptr(root_ptr)).exports.count(), 0);
            let native_child_owner = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(clone),
            );
            let native_child = native_child_owner.as_ptr();
            let index = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::numbers::PyLong_FromLong(1),
            );
            let value = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::numbers::PyLong_FromLong(b'Z'.into()),
            );
            let assigned = object::PyObject_SetItem(native_child, index.as_ptr(), value.as_ptr());
            if writable {
                assert_eq!(assigned, 0);
                assert!(!crate::exception_pending(&py));
                assert_eq!((*exporter.cast::<Exporter>()).data, *b"AZC");
            } else {
                assert_eq!(assigned, -1);
                assert_ne!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                    0
                );
                errors::PyErr_Clear();
                assert_eq!((*exporter.cast::<Exporter>()).data, *b"ABC");
            }
            drop(value);
            drop(index);
            let method = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                native_child,
                c"__buffer__".as_ptr(),
            ));
            assert!(
                !method.as_ptr().is_null(),
                "active child __buffer__ lookup: {}",
                pending_buffer_error()
            );
            if inject_failure {
                panic!("scoped-view fixture owner unwind");
            }
            let flags = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::numbers::PyLong_FromLong(0),
            );
            let native_alias_owner = refcount::OwnedPyObject::from_owned(
                object::PyObject_CallOneArg(method.as_ptr(), flags.as_ptr()),
            );
            let native_alias = native_alias_owner.as_ptr();
            assert!(
                !native_alias.is_null(),
                "active child __buffer__ call: {}",
                pending_buffer_error()
            );
            let alias = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(native_alias)
                .unwrap();
            let alias_owner = OwnedRuntimeValue::from_owned_bits(py.core_token(), alias);
            let alias_clone_owner = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::molt_memoryview_new(alias),
            );
            let alias_obj = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                native_alias,
                c"obj".as_ptr(),
            ));
            assert_eq!(alias_obj.as_ptr(), native_child);
            drop(alias_obj);
            let mut descriptor = crate::object::memoryview::MoltBufferView::default();
            let acquired = crate::c_api::molt_buffer_acquire(clone, &mut descriptor);
            let c_api_alias = if acquired == 0 {
                crate::c_api::molt_memoryview_from_buffer(&descriptor)
            } else {
                crate::MoltObject::none().bits()
            };
            let c_api_alias_owner =
                OwnedRuntimeValue::from_owned_bits(py.core_token(), c_api_alias);
            // Retire the explicit C descriptor before assertions can unwind.
            let released = crate::c_api::molt_buffer_release(&mut descriptor);
            assert_eq!(acquired, 0);
            assert!(!crate::exception_pending(&py));
            assert_eq!(released, 0);
            drop(native_alias_owner);
            drop(flags);
            drop(method);
            drop(native_child_owner);
            let children = [
                clone_owner,
                readonly_owner,
                cast_owner,
                sliced_owner,
                alias_owner,
                alias_clone_owner,
                c_api_alias_owner,
            ];
            (*exporter.cast::<Exporter>()).data[1] = b'Z';
            for child in &children {
                let child = child.bits();
                let pointer = crate::obj_from_bits(child).as_ptr().unwrap();
                assert_eq!(crate::memoryview_base_bits(pointer), 0);
                assert_eq!(
                    crate::object::memoryview::memoryview_collect_bytes(pointer).unwrap(),
                    b"AZC"
                );
            }
            if !writable {
                let expected_hash_bytes = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::MoltObject::from_ptr(crate::alloc_bytes(&py, b"AZC")).bits(),
                );
                assert_eq!(
                    crate::hash_bits_signed(&py, clone),
                    crate::hash_bits_signed(&py, expected_hash_bytes.bits())
                );
                drop(expected_hash_bytes);
            }
            let iterator_owner =
                OwnedRuntimeValue::from_owned_bits(py.core_token(), crate::molt_iter(clone));
            let iterator = iterator_owner.bits();
            let c_child_owner = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(clone),
            );
            let c_child = c_child_owner.as_ptr();
            let native_export =
                memory::MemoryViewLease::acquire(c_child, PyBUF_SIMPLE, buffer::PyObject_GetBuffer)
                    .expect("active child exports through the real C API");
            // Scope expiry never dereferences retained raw C addresses. Their use
            // beyond the raw callback violates its lifetime precondition; release
            // remains valid and must retire only this view's counted pin.
            let expected_error = if pending_error {
                errors::PyErr_SetString(
                    (&raw mut PyExc_RuntimeError).cast(),
                    c"raw readinto callback failed".as_ptr(),
                );
                let error = errors::PyErr_GetRaisedException();
                assert!(!error.is_null());
                errors::PyErr_SetRaisedException(error);
                error
            } else {
                ptr::null_mut()
            };
            drop(scope);
            drop(input);
            drop(writable_input);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
            let observed_error =
                refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(observed_error.as_ptr(), expected_error);
            drop(observed_error);
            drop(source_owner);
            drop(exporter_owner);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
            for child in &children {
                let child = child.bits();
                let pointer = crate::obj_from_bits(child).as_ptr().unwrap();
                assert!(crate::memoryview_released(pointer));
                let refused_bytes = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::molt_memoryview_tobytes(child),
                );
                assert!(crate::exception_pending(&py));
                crate::clear_exception(&py);
                drop(refused_bytes);
                let refused_view = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::molt_memoryview_new(child),
                );
                assert!(crate::exception_pending(&py));
                crate::clear_exception(&py);
                drop(refused_view);
            }
            let refused_next = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::molt_iter_next(iterator),
            );
            assert!(crate::exception_pending(&py));
            crate::clear_exception(&py);
            drop(refused_next);
            let zero = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::numbers::PyLong_FromLong(0),
            );
            let refused_item = refcount::OwnedPyObject::from_owned(object::PyObject_GetItem(
                c_child,
                zero.as_ptr(),
            ));
            assert!(refused_item.as_ptr().is_null());
            errors::PyErr_Clear();
            drop(zero);
            let mut refused: Py_buffer = std::mem::zeroed();
            let refused_status = buffer::PyObject_GetBuffer(c_child, &mut refused, PyBUF_SIMPLE);
            buffer::PyBuffer_Release(&mut refused);
            assert_eq!(refused_status, -1);
            errors::PyErr_Clear();
            drop(native_export);
            drop(c_child_owner);
            assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
            // Release descriptor aliases before their exported source. Cloned
            // aliases share that export until the final descendant is retired.
            for child in children.into_iter().rev() {
                crate::molt_memoryview_release(child.bits());
                assert!(!crate::exception_pending(&py));
                drop(child);
            }
            drop(iterator_owner);
            drop(format_owner);
            drop(index_owner);
            assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
        });
    }
    for writable in [false, true] {
        let failure =
            crate::test_support::catch_expected_unwind(|| exercise(true, writable, false));
        let error = failure.expect_err("the selected assertion point must unwind");
        assert_eq!(
            error.downcast_ref::<&str>().copied(),
            Some("scoped-view fixture owner unwind")
        );
        // The real native declaration/leases/children retire before the class,
        // including assertion unwind and an ordinary callback exception.
        assert_eq!(EXPORT_RELEASES.load(Ordering::SeqCst), 1);
        assert_eq!(EXPORT_FREES.load(Ordering::SeqCst), 1);
        exercise(false, writable, false);
        exercise(false, writable, true);
    }
}

#[test]
#[ignore = "requires initial process-static builtin readiness; run in the isolated runtime gate"]
fn builtin_buffer_names_share_cold_runtime_lookup_and_native_readiness() {
    // A runtime restart cannot restore the process's first rootless READY
    // shells. After C exposure and retirement, bootstrap must rebuild those
    // shells and publishes their namespaces before a test body can enter.
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        crate::concurrency::gil::with_gil(|py| unsafe {
            use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            use molt_runtime_core::OwnedRuntimeValue;
            let classes = crate::builtin_classes(&py);
            let bytes = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::MoltObject::from_ptr(crate::alloc_bytes(&py, b"ABC")).bits(),
            );
            let mutable = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::MoltObject::from_ptr(crate::alloc_bytearray(&py, b"ABC")).bits(),
            );
            let view = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::molt_memoryview_new(bytes.bits()),
            );
            let get_name = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::attr_name_bits_from_bytes(&py, b"__buffer__").unwrap(),
            );
            let release_name = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::attr_name_bits_from_bytes(&py, b"__release_buffer__").unwrap(),
            );
            let flags = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::numbers::PyLong_FromLong(0),
            );
            for (class_bits, value, native, has_release) in [
                (classes.bytes, bytes.bits(), &raw mut PyBytes_Type, false),
                (
                    classes.bytearray,
                    mutable.bits(),
                    &raw mut PyByteArray_Type,
                    true,
                ),
                (
                    classes.memoryview,
                    view.bits(),
                    &raw mut PyMemoryView_Type,
                    true,
                ),
            ] {
                let class = crate::obj_from_bits(class_bits).as_ptr().unwrap();
                assert!(!crate::object::class_storage::class_declares(
                    class,
                    crate::object::class_storage::ClassDeclaration::NativeNamespacePublished
                ));
                // Request one real declaration before C readiness. This must
                // neither enumerate the namespace nor invent missing slots.
                let descriptor =
                    crate::builtins::attr::class_namespace_lookup_raw(&py, class, get_name.bits())
                        .unwrap()
                        .expect("cold builtin __buffer__ declaration");
                let descriptor = OwnedRuntimeValue::retain(py.core_token(), descriptor);
                assert!(!crate::object::class_storage::class_declares(
                    class,
                    crate::object::class_storage::ClassDeclaration::NativeNamespacePublished
                ));
                let release = crate::builtins::attr::class_namespace_lookup_raw(
                    &py,
                    class,
                    release_name.bits(),
                )
                .unwrap();
                assert_eq!(release.is_some(), has_release);
                let c_descriptor = refcount::OwnedPyObject::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(descriptor.bits()),
                );
                let owner = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                    c_descriptor.as_ptr(),
                    c"__objclass__".as_ptr(),
                ));
                assert_eq!(owner.as_ptr(), native.cast());
                let source = refcount::OwnedPyObject::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value),
                );
                let method = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                    source.as_ptr(),
                    c"__buffer__".as_ptr(),
                ));
                assert!(
                    !method.as_ptr().is_null(),
                    "cold builtin lookup: {}",
                    pending_buffer_error()
                );
                let alias = refcount::OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                    method.as_ptr(),
                    flags.as_ptr(),
                ));
                assert!(
                    !alias.as_ptr().is_null(),
                    "cold builtin buffer: {}",
                    pending_buffer_error()
                );
                let alias_owner = refcount::OwnedPyObject::from_owned(
                    object::PyObject_GetAttrString(alias.as_ptr(), c"obj".as_ptr()),
                );
                assert_eq!(alias_owner.as_ptr(), source.as_ptr());
                let descriptor_view = memory::PyMemoryView_GET_BUFFER(alias.as_ptr());
                assert_eq!((*descriptor_view).len, 3);
                assert_eq!(
                    std::slice::from_raw_parts((*descriptor_view).buf.cast::<u8>(), 3),
                    b"ABC"
                );
                if has_release {
                    let release =
                        refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                            source.as_ptr(),
                            c"__release_buffer__".as_ptr(),
                        ));
                    let result = refcount::OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                        release.as_ptr(),
                        alias.as_ptr(),
                    ));
                    assert_eq!(result.as_ptr(), &raw mut Py_None);
                }
                drop(alias_owner);
                drop(alias);
                assert_eq!(typeobj::PyType_Ready(native), 0);
                assert_eq!(
                    molt_cpython_abi::api::mapping::_PyDict_GetItemStringWithError(
                        (*native).tp_dict,
                        c"__buffer__".as_ptr()
                    ),
                    c_descriptor.as_ptr()
                );
                assert_eq!(
                    !molt_cpython_abi::api::mapping::_PyDict_GetItemStringWithError(
                        (*native).tp_dict,
                        c"__release_buffer__".as_ptr()
                    )
                    .is_null(),
                    has_release
                );
            }
            let object_class = crate::obj_from_bits(classes.object).as_ptr().unwrap();
            assert!(
                crate::builtins::attr::class_namespace_lookup_raw(
                    &py,
                    object_class,
                    get_name.bits()
                )
                .unwrap()
                .is_none()
            );
            // A real native subtype inherits the exact declaring descriptor;
            // an actual namespace replacement wins over physical slot synthesis.
            let mut subtype = super::native_test_fixture::NativeType::<PyTypeObject>::subtype(
                &raw mut PyBytes_Type,
                c"BufferNamespaceSubtype",
            );
            subtype.tp_basicsize = PyBytes_Type.tp_basicsize;
            subtype.tp_itemsize = PyBytes_Type.tp_itemsize;
            assert_eq!(subtype.ready(), 0);
            let inherited = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                (&raw mut *subtype).cast(),
                c"__buffer__".as_ptr(),
            ));
            let original = molt_cpython_abi::api::mapping::_PyDict_GetItemStringWithError(
                PyBytes_Type.tp_dict,
                c"__buffer__".as_ptr(),
            );
            assert_eq!(inherited.as_ptr(), original);
            let replacement = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::numbers::PyLong_FromLong(42),
            );
            assert_eq!(
                molt_cpython_abi::api::mapping::PyDict_SetItemString(
                    subtype.tp_dict,
                    c"__buffer__".as_ptr(),
                    replacement.as_ptr()
                ),
                0
            );
            typeobj::PyType_Modified(&raw mut *subtype);
            let replaced = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                (&raw mut *subtype).cast(),
                c"__buffer__".as_ptr(),
            ));
            assert_eq!(replaced.as_ptr(), replacement.as_ptr());
            drop(replaced);
            drop(replacement);
            drop(inherited);
            drop(subtype);
            // Once the class namespace is published, deletion remains absent.
            assert_eq!(
                molt_cpython_abi::api::mapping::PyDict_DelItemString(
                    PyBytes_Type.tp_dict,
                    c"__buffer__".as_ptr()
                ),
                0
            );
            typeobj::PyType_Modified(&raw mut PyBytes_Type);
            assert_eq!(typeobj::PyType_Ready(&raw mut PyBytes_Type), 0);
            let class = crate::obj_from_bits(classes.bytes).as_ptr().unwrap();
            assert!(
                crate::builtins::attr::class_namespace_lookup_raw(&py, class, get_name.bits())
                    .unwrap()
                    .is_none()
            );
            assert!(!crate::exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
        });
    });
}
