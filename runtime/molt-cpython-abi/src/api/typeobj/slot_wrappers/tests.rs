//! Independent callback oracles for the CPython 3.12.13 slotdef signatures.
//! Expected operand order, NULL deletion and index conversion are written here,
//! not computed from the adapter selected by the declaration being exercised.

use super::*;
use std::cell::RefCell;
use std::ffi::CStr;

thread_local! { static CALLS: RefCell<Vec<Vec<usize>>> = const { RefCell::new(Vec::new()) }; }

fn record(values: &[usize]) {
    CALLS.with(|calls| calls.borrow_mut().push(values.to_vec()));
}
fn take_call() -> Vec<usize> {
    CALLS.with(|calls| calls.borrow_mut().pop().expect("callback was invoked"))
}

unsafe extern "C" fn unary(a: *mut PyObject) -> *mut PyObject {
    record(&[a.addr()]);
    unsafe { new_reference(a) }
}
unsafe extern "C" fn binary(a: *mut PyObject, b: *mut PyObject) -> *mut PyObject {
    record(&[a.addr(), b.addr()]);
    unsafe { new_reference(a) }
}
unsafe extern "C" fn ternary(
    a: *mut PyObject,
    b: *mut PyObject,
    c: *mut PyObject,
) -> *mut PyObject {
    record(&[a.addr(), b.addr(), c.addr()]);
    unsafe { new_reference(a) }
}
unsafe extern "C" fn status(a: *mut PyObject, b: *mut PyObject, c: *mut PyObject) -> c_int {
    record(&[a.addr(), b.addr(), c.addr()]);
    0
}
unsafe extern "C" fn compare(a: *mut PyObject, b: *mut PyObject, op: c_int) -> *mut PyObject {
    record(&[a.addr(), b.addr(), op as usize]);
    unsafe { new_reference(&raw mut Py_NotImplementedSentinel) }
}
unsafe extern "C" fn size(a: *mut PyObject) -> isize {
    record(&[a.addr()]);
    7
}
unsafe extern "C" fn negative_hash(a: *mut PyObject) -> isize {
    record(&[a.addr()]);
    -23
}
unsafe extern "C" fn inquiry(a: *mut PyObject) -> c_int {
    record(&[a.addr()]);
    1
}
unsafe extern "C" fn contains(a: *mut PyObject, b: *mut PyObject) -> c_int {
    record(&[a.addr(), b.addr()]);
    1
}
unsafe extern "C" fn index(a: *mut PyObject, i: isize) -> *mut PyObject {
    record(&[a.addr(), i as usize]);
    unsafe { new_reference(a) }
}
unsafe extern "C" fn assign_index(a: *mut PyObject, i: isize, value: *mut PyObject) -> c_int {
    record(&[a.addr(), i as usize, value.addr()]);
    0
}
unsafe extern "C" fn finalize(a: *mut PyObject) {
    record(&[a.addr()]);
}
unsafe extern "C" fn stop(_: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

#[derive(Clone, Copy)]
enum Family {
    Direct,
    Number,
    Sequence,
    Mapping,
    Async,
}

fn declaration(name: &CStr, family: Family) -> &'static SlotWrapperDef {
    SLOT_WRAPPER_DEFS
        .iter()
        .find(|definition| {
            (unsafe { CStr::from_ptr(definition.base.name) }) == name
                && matches!(
                    (definition.slot, family),
                    (SlotWrapper::Direct(_), Family::Direct)
                        | (SlotWrapper::Number(_), Family::Number)
                        | (SlotWrapper::Sequence(_), Family::Sequence)
                        | (SlotWrapper::Mapping(_), Family::Mapping)
                        | (SlotWrapper::Async(_), Family::Async)
                )
        })
        .expect("CPython declaration exists")
}

/// Construct only the physical storage here so this unit oracle has no runtime
/// string/dictionary dependency. Public constructors are covered separately.
unsafe fn descriptor(
    owner: *mut PyTypeObject,
    definition: &'static SlotWrapperDef,
    callback: *mut c_void,
) -> *mut PyObject {
    let descr = unsafe { memory::_PyObject_GC_New(&raw mut PyWrapperDescr_Type) }
        .cast::<PyWrapperDescrObject>();
    assert!(!descr.is_null());
    unsafe {
        refcount::Py_INCREF(owner.cast());
        (*descr).d_common.d_type = owner;
        (*descr).d_base = (&raw const definition.base).cast_mut();
        (*descr).d_wrapped = callback;
        descriptors::publish(descr.cast())
    }
}

unsafe fn invoke(
    owner: *mut PyTypeObject,
    receiver: *mut PyObject,
    definition: &'static SlotWrapperDef,
    callback: *mut c_void,
    values: &[*mut PyObject],
) -> *mut PyObject {
    let descr = unsafe { descriptor(owner, definition, callback) };
    let bound = unsafe { PyWrapper_New(descr, receiver) };
    assert!(!bound.is_null());
    let args = unsafe { sequences::native_call_args(values) };
    assert!(!args.is_null());
    let result = unsafe { method_call(bound, args, ptr::null_mut()) };
    unsafe { errors::release_preserving_error(&[args, bound, descr]) };
    result
}

#[test]
fn declaration_families_call_the_exact_signature_and_operand_order() {
    let _thread = crate::api::object::AbiTestThreadStateTransaction::new();
    let mut owner: PyTypeObject = unsafe { std::mem::zeroed() };
    owner.ob_base.ob_base.ob_refcnt = 1;
    owner.tp_name = c"SlotProbe".as_ptr();
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut owner,
    };
    let owner = &raw mut owner;
    let receiver = &raw mut receiver;
    let other = &raw mut Py_None;
    let value = &raw mut Py_NotImplementedSentinel;
    let call =
        move |name: &CStr, family, callback, args: &[*mut PyObject], expected: Vec<usize>| {
            let result =
                unsafe { invoke(owner, receiver, declaration(name, family), callback, args) };
            assert!(!result.is_null(), "{}", name.to_string_lossy());
            assert_eq!(take_call(), expected, "{}", name.to_string_lossy());
            unsafe { refcount::Py_DECREF(result) };
        };
    for name in [c"__repr__", c"__str__", c"__iter__", c"__next__"] {
        call(
            name,
            Family::Direct,
            unary as *const () as _,
            &[],
            vec![receiver.addr()],
        );
    }
    for name in [
        c"__neg__",
        c"__pos__",
        c"__abs__",
        c"__invert__",
        c"__int__",
        c"__float__",
        c"__index__",
    ] {
        call(
            name,
            Family::Number,
            unary as *const () as _,
            &[],
            vec![receiver.addr()],
        );
    }
    for name in [c"__await__", c"__aiter__", c"__anext__"] {
        call(
            name,
            Family::Async,
            unary as *const () as _,
            &[],
            vec![receiver.addr()],
        );
    }
    for name in [
        c"__add__",
        c"__sub__",
        c"__mul__",
        c"__mod__",
        c"__divmod__",
        c"__lshift__",
        c"__rshift__",
        c"__and__",
        c"__xor__",
        c"__or__",
        c"__floordiv__",
        c"__truediv__",
        c"__matmul__",
        c"__iadd__",
        c"__isub__",
        c"__imul__",
        c"__imod__",
        c"__ilshift__",
        c"__irshift__",
        c"__iand__",
        c"__ixor__",
        c"__ior__",
        c"__ifloordiv__",
        c"__itruediv__",
        c"__imatmul__",
    ] {
        call(
            name,
            Family::Number,
            binary as *const () as _,
            &[other],
            vec![receiver.addr(), other.addr()],
        );
    }
    for name in [
        c"__radd__",
        c"__rsub__",
        c"__rmul__",
        c"__rmod__",
        c"__rdivmod__",
        c"__rlshift__",
        c"__rrshift__",
        c"__rand__",
        c"__rxor__",
        c"__ror__",
        c"__rfloordiv__",
        c"__rtruediv__",
        c"__rmatmul__",
    ] {
        call(
            name,
            Family::Number,
            binary as *const () as _,
            &[other],
            vec![other.addr(), receiver.addr()],
        );
    }
    for name in [c"__pow__", c"__ipow__"] {
        call(
            name,
            Family::Number,
            ternary as *const () as _,
            &[other],
            vec![receiver.addr(), other.addr(), other.addr()],
        );
        call(
            name,
            Family::Number,
            ternary as *const () as _,
            &[other, value],
            vec![receiver.addr(), other.addr(), value.addr()],
        );
    }
    call(
        c"__rpow__",
        Family::Number,
        ternary as *const () as _,
        &[other, value],
        vec![other.addr(), receiver.addr(), value.addr()],
    );
    for (op, name) in [
        c"__lt__", c"__le__", c"__eq__", c"__ne__", c"__gt__", c"__ge__",
    ]
    .into_iter()
    .enumerate()
    {
        call(
            name,
            Family::Direct,
            compare as *const () as _,
            &[other],
            vec![receiver.addr(), other.addr(), op],
        );
    }
    call(
        c"__getattribute__",
        Family::Direct,
        binary as *const () as _,
        &[other],
        vec![receiver.addr(), other.addr()],
    );
    call(
        c"__get__",
        Family::Direct,
        ternary as *const () as _,
        &[other, value],
        vec![receiver.addr(), 0, value.addr()],
    );
    for (name, args) in [
        (c"__set__", vec![other, value]),
        (c"__delete__", vec![other]),
        (c"__setattr__", vec![other, value]),
        (c"__delattr__", vec![other]),
    ] {
        let final_value = if args.len() == 2 { value.addr() } else { 0 };
        call(
            name,
            Family::Direct,
            status as *const () as _,
            &args,
            vec![receiver.addr(), other.addr(), final_value],
        );
    }
    call(
        c"__del__",
        Family::Direct,
        finalize as *const () as _,
        &[],
        vec![receiver.addr()],
    );
    call(
        c"__hash__",
        Family::Direct,
        negative_hash as *const () as _,
        &[],
        vec![receiver.addr()],
    );
    call(
        c"__bool__",
        Family::Number,
        inquiry as *const () as _,
        &[],
        vec![receiver.addr()],
    );
    for family in [Family::Mapping, Family::Sequence] {
        call(
            c"__len__",
            family,
            size as *const () as _,
            &[],
            vec![receiver.addr()],
        );
    }
    call(
        c"__getitem__",
        Family::Mapping,
        binary as *const () as _,
        &[other],
        vec![receiver.addr(), other.addr()],
    );
    call(
        c"__setitem__",
        Family::Mapping,
        status as *const () as _,
        &[other, value],
        vec![receiver.addr(), other.addr(), value.addr()],
    );
    call(
        c"__delitem__",
        Family::Mapping,
        status as *const () as _,
        &[other],
        vec![receiver.addr(), other.addr(), 0],
    );
    for name in [c"__add__", c"__iadd__"] {
        call(
            name,
            Family::Sequence,
            binary as *const () as _,
            &[other],
            vec![receiver.addr(), other.addr()],
        );
    }
    call(
        c"__contains__",
        Family::Sequence,
        contains as *const () as _,
        &[other],
        vec![receiver.addr(), other.addr()],
    );

    let negative = unsafe { numbers::PyLong_FromLong(-2) };
    let mut sequence: PySequenceMethods = unsafe { std::mem::zeroed() };
    sequence.sq_length = size as *const () as _;
    unsafe { (*owner).tp_as_sequence = (&raw mut sequence).cast() };
    call(
        c"__getitem__",
        Family::Sequence,
        index as *const () as _,
        &[negative],
        vec![receiver.addr(), 5],
    );
    let _length_call = take_call();
    for name in [c"__mul__", c"__rmul__", c"__imul__"] {
        call(
            name,
            Family::Sequence,
            index as *const () as _,
            &[negative],
            vec![receiver.addr(), (-2isize) as usize],
        );
    }
    call(
        c"__setitem__",
        Family::Sequence,
        assign_index as *const () as _,
        &[negative, value],
        vec![receiver.addr(), 5, value.addr()],
    );
    let _length_call = take_call();
    call(
        c"__delitem__",
        Family::Sequence,
        assign_index as *const () as _,
        &[negative],
        vec![receiver.addr(), 5, 0],
    );
    let _length_call = take_call();
    unsafe { refcount::Py_DECREF(negative) };
    assert_eq!(unsafe { (*receiver).ob_refcnt }, 1);
    assert_eq!(unsafe { (*owner).ob_base.ob_base.ob_refcnt }, 1);
}

#[test]
fn keyword_call_and_init_preserve_tuple_and_null_c_meanings() {
    let _thread = crate::api::object::AbiTestThreadStateTransaction::new();
    let mut owner: PyTypeObject = unsafe { std::mem::zeroed() };
    owner.ob_base.ob_base.ob_refcnt = 1;
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut owner,
    };
    let receiver = &raw mut receiver;
    let args = unsafe { sequences::native_call_args(&[&raw mut Py_None]) };
    // NULL kwargs is a protocol value, not an empty dictionary. Non-NULL
    // dictionary identity and operand order are covered with real runtime
    // dictionaries in the native-call projection suite.
    for (name, callback) in [
        (c"__call__", ternary as *const () as *mut c_void),
        (c"__init__", status as *const () as *mut c_void),
    ] {
        let descr =
            unsafe { descriptor(&raw mut owner, declaration(name, Family::Direct), callback) };
        let result = unsafe { raw_call(descr.cast(), receiver, args, ptr::null_mut()) };
        assert!(!result.is_null());
        assert_eq!(take_call(), [receiver.addr(), args.addr(), 0]);
        unsafe { errors::release_preserving_error(&[result, descr]) };
    }
    let next = unsafe {
        descriptor(
            &raw mut owner,
            declaration(c"__next__", Family::Direct),
            stop as *const () as _,
        )
    };
    let empty = unsafe { sequences::native_call_args(&[]) };
    assert!(unsafe { raw_call(next.cast(), receiver, empty, ptr::null_mut()) }.is_null());
    assert_eq!(
        unsafe { errors::PyErr_ExceptionMatches((&raw mut PyExc_StopIteration).cast()) },
        1
    );
    unsafe {
        errors::PyErr_Clear();
        errors::release_preserving_error(&[args, next, empty])
    };
}

unsafe extern "C" fn visit(edge: *mut PyObject, out: *mut c_void) -> c_int {
    unsafe { (*out.cast::<Vec<usize>>()).push(edge.addr()) };
    0
}

#[test]
fn wrapper_and_bound_wrapper_trace_clear_and_release_every_owned_edge() {
    let _thread = crate::api::object::AbiTestThreadStateTransaction::new();
    let mut owner: PyTypeObject = unsafe { std::mem::zeroed() };
    owner.ob_base.ob_base.ob_refcnt = 1;
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut owner,
    };
    let mut name = PyObject {
        ob_refcnt: 2,
        ob_type: ptr::null_mut(),
    };
    let mut qualname = PyObject {
        ob_refcnt: 2,
        ob_type: ptr::null_mut(),
    };
    let descr = unsafe {
        descriptor(
            &raw mut owner,
            declaration(c"__repr__", Family::Direct),
            unary as *const () as _,
        )
    };
    unsafe {
        (*descr.cast::<PyDescrObject>()).d_name = &raw mut name;
        (*descr.cast::<PyDescrObject>()).d_qualname = &raw mut qualname;
    }
    let bound = unsafe { PyWrapper_New(descr, &raw mut receiver) };
    assert_eq!(unsafe { memory::PyObject_GC_IsTracked(descr) }, 1);
    assert_eq!(unsafe { memory::PyObject_GC_IsTracked(bound) }, 1);
    let mut edges = Vec::<usize>::new();
    unsafe { descriptors::traverse(descr, visit as *const () as _, (&raw mut edges).cast()) };
    assert_eq!(
        edges,
        [
            (&raw mut owner).addr(),
            (&raw mut name).addr(),
            (&raw mut qualname).addr()
        ]
    );
    edges.clear();
    unsafe { method_traverse(bound, visit as *const () as _, (&raw mut edges).cast()) };
    assert_eq!(edges, [descr.addr(), (&raw mut receiver).addr()]);
    unsafe {
        method_clear(bound);
        method_clear(bound);
        refcount::Py_DECREF(bound);
        refcount::Py_DECREF(descr);
    }
    assert_eq!(
        (
            owner.ob_base.ob_base.ob_refcnt,
            receiver.ob_refcnt,
            name.ob_refcnt,
            qualname.ob_refcnt
        ),
        (1, 1, 1, 1)
    );
}

unsafe extern "C" fn getbuffer(object: *mut PyObject, view: *mut Py_buffer, flags: c_int) -> c_int {
    record(&[object.addr(), flags as usize]);
    unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            view,
            object,
            ptr::NonNull::<u8>::dangling().as_ptr().cast(),
            8,
            1,
            flags,
        )
    }
}
unsafe extern "C" fn unexpected_buffer(_: *mut PyObject, _: *mut Py_buffer, _: c_int) -> c_int {
    record(&[usize::MAX]);
    -1
}
unsafe extern "C" fn releasebuffer(object: *mut PyObject, view: *mut Py_buffer) {
    record(&[object.addr(), unsafe { (*view).len } as usize]);
}

#[test]
fn native_buffer_lease_uses_declaring_callback_and_last_share_releases_once() {
    let _thread = crate::api::object::AbiTestThreadStateTransaction::new();
    let mut owner: PyTypeObject = unsafe { std::mem::zeroed() };
    owner.ob_base.ob_base.ob_refcnt = 1;
    let mut buffer = PyBufferProcs {
        bf_getbuffer: unexpected_buffer as *const () as _,
        bf_releasebuffer: releasebuffer as *const () as _,
    };
    owner.tp_as_buffer = (&raw mut buffer).cast();
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut owner,
    };
    let lease =
        unsafe { memory::MemoryViewLease::acquire(&raw mut receiver, PyBUF_FULL_RO, getbuffer) }
            .unwrap();
    assert_eq!(
        take_call(),
        [(&raw mut receiver).addr(), PyBUF_FULL_RO as usize]
    );
    let alias = lease.clone();
    assert_eq!(receiver.ob_refcnt, 3);
    assert_eq!(
        alias.gc_edge().unwrap().value,
        (&raw mut receiver).addr() as u64
    );
    drop(lease);
    assert_eq!(receiver.ob_refcnt, 2);
    assert!(CALLS.with(|calls| calls.borrow().is_empty()));
    assert_eq!(unsafe { *(*alias.descriptor()).shape }, 8);
    assert_eq!(unsafe { *(*alias.descriptor()).strides }, 1);
    drop(alias);
    assert_eq!(take_call(), [(&raw mut receiver).addr(), 8]);
    assert_eq!(receiver.ob_refcnt, 1);
}

#[test]
fn physical_wrapper_layout_matches_the_pinned_cpython_header() {
    let word = std::mem::size_of::<*mut c_void>();
    assert_eq!(std::mem::size_of::<PyDescrObject>(), 5 * word);
    assert_eq!(std::mem::offset_of!(PyWrapperDescrObject, d_base), 5 * word);
    assert_eq!(
        std::mem::offset_of!(PyWrapperDescrObject, d_wrapped),
        6 * word
    );
    assert_eq!(std::mem::size_of::<PyWrapperDescrObject>(), 7 * word);
    assert_eq!(std::mem::size_of::<PyMethodWrapperObject>(), 4 * word);
    assert_eq!(std::mem::size_of::<PyWrapperBase>(), 7 * word);
}
