//! Attribute slot inheritance is completed by readiness, before dispatch.
use super::*;
use molt_cpython_abi::api::{errors, object, refcount};

#[repr(C)]
struct Receiver {
    object: PyObject,
    reads: usize,
    writes: usize,
}

unsafe extern "C" fn modern_get(object: *mut PyObject, _name: *mut PyObject) -> *mut PyObject {
    unsafe {
        (*object.cast::<Receiver>()).reads += 1;
        refcount::Py_INCREF(object);
    }
    object
}

unsafe extern "C" fn modern_set(
    object: *mut PyObject,
    _name: *mut PyObject,
    _value: *mut PyObject,
) -> c_int {
    unsafe {
        (*object.cast::<Receiver>()).writes += 1;
    }
    0
}

unsafe extern "C" fn legacy_get(object: *mut PyObject, _name: *const c_char) -> *mut PyObject {
    unsafe {
        (*object.cast::<Receiver>()).reads += 10;
        refcount::Py_INCREF(object);
    }
    object
}

unsafe extern "C" fn legacy_set(
    object: *mut PyObject,
    _name: *const c_char,
    _value: *mut PyObject,
) -> c_int {
    unsafe {
        (*object.cast::<Receiver>()).writes += 10;
    }
    0
}

unsafe fn exercise_attribute_consumers(kind: *mut PyTypeObject, expected: usize) {
    let mut receiver = Receiver {
        object: PyObject {
            ob_refcnt: 1,
            ob_type: kind,
        },
        reads: 0,
        writes: 0,
    };
    let receiver_ptr = &raw mut receiver.object;
    let result = unsafe { object::PyObject_GetAttrString(receiver_ptr, c"field".as_ptr()) };
    assert_eq!(result, receiver_ptr);
    unsafe {
        refcount::Py_DECREF(result);
    }
    assert_eq!(
        unsafe { object::PyObject_SetAttrString(receiver_ptr, c"field".as_ptr(), receiver_ptr) },
        0
    );
    assert_eq!(receiver.reads, expected);
    assert_eq!(receiver.writes, expected);
    assert_eq!(receiver.object.ob_refcnt, 1);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn leaf_readiness_completes_attribute_slots_through_three_generations() {
    let _transaction = init();
    let mut root = NativeType::<PyTypeObject>::new();
    root.ob_base.ob_base.ob_refcnt = 1;
    root.tp_name = c"AttributeRoot".as_ptr();
    root.tp_basicsize = std::mem::size_of::<Receiver>() as Py_ssize_t;
    root.tp_getattro = Some(modern_get);
    root.tp_setattro = Some(modern_set);
    let mut middle = NativeType::<PyTypeObject>::new();
    middle.ob_base.ob_base.ob_refcnt = 1;
    middle.tp_name = c"AttributeMiddle".as_ptr();
    middle.tp_base = &raw mut *root;
    let mut leaf = NativeType::<PyTypeObject>::new();
    leaf.ob_base.ob_base.ob_refcnt = 1;
    leaf.tp_name = c"AttributeLeaf".as_ptr();
    leaf.tp_base = &raw mut *middle;
    assert_eq!(unsafe { ready(&raw mut *leaf) }, 0);
    for kind in [&raw mut *root, &raw mut *middle, &raw mut *leaf] {
        assert_ne!(unsafe { (*kind).tp_flags } & Py_TPFLAGS_READY, 0);
        assert!(!unsafe { (*kind).tp_dict }.is_null());
        assert!(unsafe { (*kind).tp_getattro }.is_some());
        assert!(unsafe { (*kind).tp_setattro }.is_some());
    }
    unsafe {
        exercise_attribute_consumers(&raw mut *leaf, 1);
    }
}

fn check_explicit_attribute_pair(legacy_child: bool) {
    let _transaction = init();
    let mut base = NativeType::<PyTypeObject>::new();
    base.ob_base.ob_base.ob_refcnt = 1;
    base.tp_name = c"AttributePairBase".as_ptr();
    base.tp_basicsize = std::mem::size_of::<Receiver>() as Py_ssize_t;
    let mut child = NativeType::<PyTypeObject>::new();
    child.ob_base.ob_base.ob_refcnt = 1;
    child.tp_name = c"AttributePairChild".as_ptr();
    child.tp_base = &raw mut *base;
    if legacy_child {
        base.tp_getattro = Some(modern_get);
        base.tp_setattro = Some(modern_set);
        child.tp_getattr = Some(legacy_get);
        child.tp_setattr = Some(legacy_set);
    } else {
        base.tp_getattr = Some(legacy_get);
        base.tp_setattr = Some(legacy_set);
        child.tp_getattro = Some(modern_get);
        child.tp_setattro = Some(modern_set);
    }
    assert_eq!(unsafe { ready(&raw mut *child) }, 0);
    assert_eq!(child.tp_getattr.is_some(), legacy_child);
    assert_eq!(child.tp_getattro.is_some(), !legacy_child);
    assert_eq!(child.tp_setattr.is_some(), legacy_child);
    assert_eq!(child.tp_setattro.is_some(), !legacy_child);
    unsafe {
        exercise_attribute_consumers(&raw mut *child, if legacy_child { 10 } else { 1 });
    }
}

#[test]
fn legacy_and_modern_attribute_overrides_each_preserve_their_entire_pair() {
    check_explicit_attribute_pair(true);
    check_explicit_attribute_pair(false);
}

#[test]
fn readiness_of_class_preserves_its_legacy_only_metaclass_getter() {
    let _transaction = init();
    let mut meta = NativeType::<PyTypeObject>::new();
    meta.ob_base.ob_base.ob_refcnt = 1;
    meta.tp_name = c"LegacyAttributeMeta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = &raw mut PyType_Type;
    meta.tp_getattr = Some(legacy_get);
    let mut class = NativeType::<PyTypeObject>::new();
    class.ob_base.ob_base.ob_refcnt = 1;
    class.ob_base.ob_base.ob_type = &raw mut *meta;
    class.tp_name = c"ClassWithLegacyAttributeMeta".as_ptr();
    assert_eq!(unsafe { ready(&raw mut *meta) }, 0);
    assert!(meta.tp_getattro.is_none());
    assert_eq!(unsafe { ready(&raw mut *class) }, 0);
    assert!(meta.tp_getattr.is_some());
    assert!(
        meta.tp_getattro.is_none(),
        "metatype completion must honor the same getter pair"
    );
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}
