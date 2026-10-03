use super::*;
use crate::abi_types::{PyTypeObject, PyVarObject};
use std::cell::RefCell;

#[derive(Clone, Copy, Default)]
enum Action {
    #[default]
    Observe,
    Replace,
    Clear,
    Raise,
}

#[derive(Default)]
struct CallbackState {
    action: Action,
    lists: [*mut PyListObject; 2],
    replacements: [*mut PyObject; 2],
    result: *mut PyObject,
    events: Vec<u8>,
}

thread_local! {
    static CALLBACK: RefCell<CallbackState> = RefCell::new(CallbackState::default());
}

#[repr(C)]
struct Probe {
    object: PyObject,
    id: u8,
}

fn probe(ty: *mut PyTypeObject, id: u8) -> *mut PyObject {
    Box::into_raw(Box::new(Probe {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: ty,
        },
        id,
    }))
    .cast()
}

unsafe extern "C" fn release_probe(object: *mut PyObject) {
    let probe = unsafe { Box::from_raw(object.cast::<Probe>()) };
    CALLBACK.with(|state| state.borrow_mut().events.push(probe.id));
    if CALLBACK.with(|state| matches!(state.borrow().action, Action::Raise)) {
        // The comparison's ValueError must survive either item's finalizer.
        unsafe {
            crate::api::errors::PyErr_SetNone(
                (&raw mut crate::abi_types::PyExc_RuntimeError).cast(),
            )
        };
    }
}

unsafe extern "C" fn compare_probe(
    left: *mut PyObject,
    right: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    let (action, lists, replacements, result) = CALLBACK.with(|state| {
        let mut state = state.borrow_mut();
        state.events.push(if op == RichCompareOp::Eq as c_int {
            10
        } else {
            20
        });
        (state.action, state.lists, state.replacements, state.result)
    });
    if op != RichCompareOp::Eq as c_int {
        assert_eq!(unsafe { (*left.cast::<Probe>()).id }, 3);
        assert_eq!(unsafe { (*right.cast::<Probe>()).id }, 4);
        assert_eq!(
            CALLBACK.with(|state| state.borrow().events.clone()),
            [10, 1, 2, 20]
        );
        unsafe { crate::api::refcount::Py_INCREF(result) };
        return result;
    }
    assert!(unsafe { (*left).ob_refcnt } >= 2);
    assert!(unsafe { (*right).ob_refcnt } >= 2);
    if !matches!(action, Action::Observe) {
        // Retire the containers' references while the adapter's two pins are
        // the only remaining owners. Publish both edits before any decref.
        for index in 0..2 {
            unsafe {
                if matches!(action, Action::Replace) {
                    *(*lists[index]).ob_item = replacements[index];
                } else {
                    (*lists[index]).ob_base.ob_size = 0;
                    *(*lists[index]).ob_item = ptr::null_mut();
                }
            }
        }
        unsafe {
            crate::api::refcount::Py_DECREF(left);
            crate::api::refcount::Py_DECREF(right);
        }
        assert_eq!(CALLBACK.with(|state| state.borrow().events.clone()), [10]);
    }
    if matches!(action, Action::Raise) {
        unsafe {
            crate::api::errors::PyErr_SetNone((&raw mut crate::abi_types::PyExc_ValueError).cast())
        };
        ptr::null_mut()
    } else {
        seq_richcmp_bool(false)
    }
}

struct NativeList {
    object: Box<PyListObject>,
    items: Box<[*mut PyObject]>,
}

impl NativeList {
    fn new(item: *mut PyObject) -> Self {
        let mut items = vec![item].into_boxed_slice();
        let object = Box::new(PyListObject {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut crate::abi_types::PyList_Type,
                },
                ob_size: 1,
            },
            ob_item: items.as_mut_ptr(),
            allocated: 1,
        });
        Self { object, items }
    }

    fn pointer(&mut self) -> *mut PyObject {
        (&mut *self.object as *mut PyListObject).cast()
    }
}

impl Drop for NativeList {
    fn drop(&mut self) {
        for &item in &self.items[..self.object.ob_base.ob_size as usize] {
            unsafe { crate::api::errors::release_preserving_error(&[item]) };
        }
    }
}

fn probe_type() -> Box<PyTypeObject> {
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"comparison_probe".as_ptr();
    ty.tp_richcompare = Some(compare_probe);
    ty.tp_dealloc = Some(release_probe);
    ty
}

fn compare_mutating_lists(action: Action, operation: RichCompareOp) {
    let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
    crate::bridge::molt_cpython_abi_init();
    let mut ty = probe_type();
    let ty = &mut *ty as *mut PyTypeObject;
    let mut left = NativeList::new(probe(ty, 1));
    let mut right = NativeList::new(probe(ty, 2));
    let mut result_object = PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    };
    let result_pointer = &raw mut result_object;
    let replacements = if matches!(action, Action::Replace) {
        [probe(ty, 3), probe(ty, 4)]
    } else {
        [ptr::null_mut(); 2]
    };
    CALLBACK.with(|state| {
        *state.borrow_mut() = CallbackState {
            action,
            lists: [left.pointer().cast(), right.pointer().cast()],
            replacements,
            result: result_pointer,
            events: Vec::new(),
        }
    });
    let result =
        unsafe { molt_list_richcompare(left.pointer(), right.pointer(), operation as c_int) };
    match action {
        Action::Replace => {
            assert_eq!(result, result_pointer);
            assert_eq!(result_object.ob_refcnt, 2);
            unsafe { crate::api::refcount::Py_DECREF(result) };
            assert_eq!(result_object.ob_refcnt, 1);
        }
        Action::Clear => {
            assert_eq!(result, (&raw mut crate::abi_types::Py_True).cast());
            assert_eq!(
                CALLBACK.with(|state| state.borrow().events.clone()),
                [10, 1, 2]
            );
        }
        Action::Raise => {
            assert!(result.is_null());
            assert_eq!(
                unsafe { crate::api::errors::PyErr_Occurred() },
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
            );
            assert_eq!(
                CALLBACK.with(|state| state.borrow().events.clone()),
                [10, 1, 2]
            );
            unsafe { crate::api::errors::PyErr_Clear() };
        }
        Action::Observe => unreachable!(),
    }
    CALLBACK.with(|state| state.borrow_mut().action = Action::Observe);
}

#[test]
fn native_list_order_reloads_after_equality_and_keeps_arbitrary_result() {
    compare_mutating_lists(Action::Replace, RichCompareOp::Lt);
}

#[test]
fn native_list_equality_rechecks_sizes_after_dropping_items() {
    compare_mutating_lists(Action::Clear, RichCompareOp::Eq);
}

#[test]
fn native_list_comparison_preserves_callback_error_through_item_finalizers() {
    compare_mutating_lists(Action::Raise, RichCompareOp::Lt);
}

#[test]
fn native_tuple_unequal_lengths_still_compare_the_common_prefix() {
    let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
    crate::bridge::molt_cpython_abi_init();
    CALLBACK.with(|state| *state.borrow_mut() = CallbackState::default());
    let mut ty = probe_type();
    let ty = &mut *ty as *mut PyTypeObject;
    let left_item = probe(ty, 1);
    let right_item = probe(ty, 2);
    let left = unsafe { native_call_args(&[left_item]) };
    let right = unsafe { native_call_args(&[right_item, &raw mut crate::abi_types::Py_None]) };
    assert!(!left.is_null() && !right.is_null());
    unsafe { crate::api::errors::release_preserving_error(&[left_item, right_item]) };
    let result = unsafe { molt_tuple_richcompare(left, right, RichCompareOp::Eq as c_int) };
    assert_eq!(result, (&raw mut crate::abi_types::Py_False).cast());
    assert_eq!(CALLBACK.with(|state| state.borrow().events.clone()), [10]);
    unsafe { crate::api::errors::release_preserving_error(&[result, left, right]) };
}

#[test]
fn native_bytearray_comparison_releases_canonical_exports() {
    let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
    crate::bridge::molt_cpython_abi_init();
    let left = unsafe { foreign_bytearray_fixture(c"same".as_ptr(), 4) };
    let right = unsafe { foreign_bytearray_fixture(c"same".as_ptr(), 4) };
    assert!(!left.is_null() && !right.is_null());
    let mut view: crate::abi_types::Py_buffer = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe {
            crate::api::buffer::PyObject_GetBuffer(
                left,
                &raw mut view,
                crate::abi_types::PyBUF_SIMPLE,
            )
        },
        0
    );
    assert_eq!(
        unsafe { (*left.cast::<crate::abi_types::PyByteArrayObject>()).ob_exports },
        1
    );
    unsafe { crate::api::buffer::PyBuffer_Release(&raw mut view) };
    assert_eq!(
        unsafe { (*left.cast::<crate::abi_types::PyByteArrayObject>()).ob_exports },
        0
    );
    let result = unsafe {
        crate::api::typeobj::PyObject_RichCompare(left, right, RichCompareOp::Eq as c_int)
    };
    assert_eq!(result, (&raw mut crate::abi_types::Py_True).cast());
    assert_eq!(
        unsafe { (*right.cast::<crate::abi_types::PyByteArrayObject>()).ob_exports },
        0
    );
    unsafe { crate::api::errors::release_preserving_error(&[result, left, right]) };
}

#[repr(C)]
struct MutatingExporter {
    object: PyObject,
    left: *mut crate::abi_types::PyByteArrayObject,
    byte: u8,
    reject: bool,
}

unsafe extern "C" fn export_after_mutation(
    object: *mut PyObject,
    view: *mut crate::abi_types::Py_buffer,
    flags: c_int,
) -> c_int {
    let exporter = object.cast::<MutatingExporter>();
    // The receiver must already own its public export lease before a peer's
    // arbitrary callback can try to resize or otherwise inspect it.
    assert_eq!(unsafe { (*(*exporter).left).ob_exports }, 1);
    unsafe { *(*(*exporter).left).ob_start = b'z' as std::ffi::c_char };
    if unsafe { (*exporter).reject } {
        unsafe {
            crate::api::errors::PyErr_SetNone((&raw mut crate::abi_types::PyExc_ValueError).cast())
        };
        return -1;
    }
    unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            view,
            object,
            (&raw mut (*exporter).byte).cast(),
            1,
            1,
            flags,
        )
    }
}

#[test]
fn native_bytearray_pins_before_peer_export_and_declines_failed_buffer() {
    let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
    crate::bridge::molt_cpython_abi_init();
    let left = unsafe { foreign_bytearray_fixture(c"a".as_ptr(), 1) };
    let mut buffer = crate::abi_types::PyBufferProcs {
        bf_getbuffer: export_after_mutation as *mut std::ffi::c_void,
        bf_releasebuffer: ptr::null_mut(),
    };
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.tp_name = c"mutating_exporter".as_ptr();
    ty.tp_as_buffer = (&raw mut buffer).cast();
    let mut exporter = MutatingExporter {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        },
        left: left.cast(),
        byte: b'z',
        reject: false,
    };
    let right = (&raw mut exporter).cast();
    let result = unsafe {
        crate::api::typeobj::molt_bytearray_richcompare(left, right, RichCompareOp::Eq as c_int)
    };
    assert_eq!(result, (&raw mut crate::abi_types::Py_True).cast());
    assert_eq!(
        unsafe { (*left.cast::<crate::abi_types::PyByteArrayObject>()).ob_exports },
        0
    );
    assert_eq!(exporter.object.ob_refcnt, 1);
    exporter.reject = true;
    let declined = unsafe {
        crate::api::typeobj::molt_bytearray_richcompare(left, right, RichCompareOp::Eq as c_int)
    };
    assert_eq!(
        declined,
        &raw mut crate::abi_types::Py_NotImplementedSentinel
    );
    assert!(unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
    assert_eq!(exporter.object.ob_refcnt, 1);
    unsafe { crate::api::errors::release_preserving_error(&[result, declined, left]) };
}

#[repr(C)]
struct BufferSlotProbe {
    object: PyObject,
    id: u8,
    byte: u8,
    reject: bool,
}

unsafe extern "C" fn probe_getbuffer(
    object: *mut PyObject,
    view: *mut crate::abi_types::Py_buffer,
    flags: c_int,
) -> c_int {
    let probe = object.cast::<BufferSlotProbe>();
    CALLBACK.with(|state| state.borrow_mut().events.push(unsafe { (*probe).id }));
    if unsafe { (*probe).reject } {
        unsafe {
            crate::api::errors::PyErr_SetNone((&raw mut crate::abi_types::PyExc_ValueError).cast())
        };
        return -1;
    }
    unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            view,
            object,
            (&raw mut (*probe).byte).cast(),
            1,
            1,
            flags,
        )
    }
}

unsafe extern "C" fn probe_releasebuffer(
    object: *mut PyObject,
    _view: *mut crate::abi_types::Py_buffer,
) {
    let id = unsafe { (*object.cast::<BufferSlotProbe>()).id };
    CALLBACK.with(|state| state.borrow_mut().events.push(10 + id));
}

#[test]
fn bytearray_declaring_comparison_uses_both_slots_in_protocol_order() {
    let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
    crate::bridge::molt_cpython_abi_init();
    let mut buffer = crate::abi_types::PyBufferProcs {
        bf_getbuffer: probe_getbuffer as *mut std::ffi::c_void,
        bf_releasebuffer: probe_releasebuffer as *mut std::ffi::c_void,
    };
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.tp_name = c"bytearray_buffer_subclass".as_ptr();
    ty.tp_base = &raw mut crate::abi_types::PyByteArray_Type;
    ty.tp_as_buffer = (&raw mut buffer).cast();
    let mut left = BufferSlotProbe {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        },
        id: 1,
        byte: b'x',
        reject: false,
    };
    let mut right = BufferSlotProbe {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        },
        id: 2,
        byte: b'x',
        reject: false,
    };
    for (left_reject, right_reject, events) in [
        (false, false, &[1, 2, 11, 12][..]),
        (true, false, &[1][..]),
        (false, true, &[1, 2, 11][..]),
    ] {
        CALLBACK.with(|state| state.borrow_mut().events.clear());
        left.reject = left_reject;
        right.reject = right_reject;
        let result = unsafe {
            crate::api::typeobj::molt_bytearray_richcompare(
                (&raw mut left).cast(),
                (&raw mut right).cast(),
                RichCompareOp::Eq as c_int,
            )
        };
        assert_eq!(CALLBACK.with(|state| state.borrow().events.clone()), events);
        let expected = if left_reject || right_reject {
            &raw mut crate::abi_types::Py_NotImplementedSentinel
        } else {
            (&raw mut crate::abi_types::Py_True).cast()
        };
        assert_eq!(result, expected);
        assert!(unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
        assert_eq!(left.object.ob_refcnt, 1);
        assert_eq!(right.object.ob_refcnt, 1);
        unsafe { crate::api::refcount::Py_DECREF(result) };
    }
    CALLBACK.with(|state| state.borrow_mut().events.clear());
    let declined = unsafe {
        crate::api::typeobj::molt_bytearray_richcompare(
            (&raw mut left).cast(),
            &raw mut crate::abi_types::Py_None,
            RichCompareOp::Eq as c_int,
        )
    };
    assert_eq!(
        declined,
        &raw mut crate::abi_types::Py_NotImplementedSentinel
    );
    assert!(CALLBACK.with(|state| state.borrow().events.is_empty()));
    unsafe { crate::api::refcount::Py_DECREF(declined) };
}

/// An explicit foreign-extension allocation; public constructors return managed
/// objects and therefore never authorize a PyByteArrayObject payload cast.
unsafe fn foreign_bytearray_fixture(data: *const std::ffi::c_char, len: isize) -> *mut PyObject {
    let object = unsafe {
        crate::api::memory::PyObject_Calloc(
            1,
            std::mem::size_of::<crate::abi_types::PyByteArrayObject>(),
        )
    }
    .cast::<crate::abi_types::PyByteArrayObject>();
    assert!(!object.is_null());
    let bytes =
        unsafe { crate::api::memory::PyMem_Calloc(1, len as usize + 1) }.cast::<std::ffi::c_char>();
    assert!(!bytes.is_null());
    unsafe {
        crate::api::memory::PyObject_Init(
            object.cast(),
            &raw mut crate::abi_types::PyByteArray_Type,
        );
        if len != 0 {
            std::ptr::copy_nonoverlapping(data, bytes, len as usize);
        }
        (*object).ob_base.ob_size = len;
        (*object).ob_alloc = len + 1;
        (*object).ob_bytes = bytes;
        (*object).ob_start = bytes;
    }
    object.cast()
}
