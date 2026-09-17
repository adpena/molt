//! One convention matrix across constructor, vectorcall and direct tp_call.
#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{
    cfunction::CFunctionConvention, errors, mapping, numbers, object, refcount, sequences, strings,
};
use molt_cpython_abi::hooks::BorrowedHandleResult;
use molt_lang_obj_model::MoltObject;
use std::cell::Cell;
use std::collections::HashMap;
use std::ptr;
use std::sync::{LazyLock, Mutex};

static LOCK: Mutex<()> = Mutex::new(());
type Dict = Vec<(u64, u64)>;
static DICTS: LazyLock<Mutex<HashMap<u64, Box<Dict>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

unsafe extern "C" fn alloc_dict() -> u64 {
    let dict = Box::new(Vec::new());
    let bits = MoltObject::from_ptr((&raw const *dict).cast_mut().cast()).bits();
    DICTS.lock().unwrap().insert(bits, dict);
    bits
}
unsafe extern "C" fn dict_set(bits: u64, key: u64, value: u64) -> i32 {
    let mut dicts = DICTS.lock().unwrap();
    let dict = dicts.get_mut(&bits).unwrap();
    if let Some(pair) = dict.iter_mut().find(|pair| pair.0 == key) {
        pair.1 = value;
    } else {
        dict.push((key, value));
    }
    0
}
unsafe extern "C" fn dict_get(bits: u64, key: u64) -> BorrowedHandleResult {
    match DICTS
        .lock()
        .unwrap()
        .get(&bits)
        .unwrap()
        .iter()
        .find(|pair| pair.0 == key)
    {
        Some(pair) => BorrowedHandleResult::ok(pair.1),
        None => BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn dict_del(bits: u64, key: u64) -> i32 {
    let mut dicts = DICTS.lock().unwrap();
    let dict = dicts.get_mut(&bits).unwrap();
    let Some(index) = dict.iter().position(|pair| pair.0 == key) else {
        return -1;
    };
    dict.remove(index);
    0
}
unsafe extern "C" fn dict_len(bits: u64) -> usize {
    DICTS.lock().unwrap().get(&bits).unwrap().len()
}
unsafe extern "C" fn dict_entry(bits: u64, index: usize, key: *mut u64, value: *mut u64) -> i32 {
    let dicts = DICTS.lock().unwrap();
    let Some(pair) = dicts.get(&bits).unwrap().get(index) else {
        return 0;
    };
    unsafe {
        *key = pair.0;
        *value = pair.1;
    }
    1
}
unsafe extern "C" fn classify(bits: u64) -> u8 {
    if DICTS.lock().unwrap().contains_key(&bits) {
        MoltTypeTag::Dict as u8
    } else if support::fake_strings::contains(bits) {
        MoltTypeTag::Str as u8
    } else {
        0xff
    }
}

fn setup() {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_strings::wire(&mut hooks);
    hooks.alloc_dict = alloc_dict;
    hooks.dict_set = dict_set;
    hooks.dict_get = dict_get;
    hooks.dict_del = dict_del;
    hooks.dict_len = dict_len;
    hooks.dict_entry = dict_entry;
    hooks.classify_heap = classify;
    support::prepare_abi_test_thread(hooks);
    unsafe { errors::PyErr_Clear() };
    *REC.lock().unwrap() = Record::default();
}

#[derive(Default, Debug)]
struct Record {
    calls: usize,
    receiver: usize,
    class: usize,
    values: Vec<i64>,
    names: Vec<String>,
}
static REC: Mutex<Record> = Mutex::new(Record {
    calls: 0,
    receiver: 0,
    class: 0,
    values: Vec::new(),
    names: Vec::new(),
});

unsafe fn record(
    self_: *mut PyObject,
    class: *mut PyTypeObject,
    args: &[*mut PyObject],
    names: *mut PyObject,
) -> *mut PyObject {
    let mut record = REC.lock().unwrap();
    record.calls += 1;
    record.receiver = self_ as usize;
    record.class = class as usize;
    record.values = args
        .iter()
        .map(|&arg| unsafe { numbers::PyLong_AsLong(arg) } as i64)
        .collect();
    record.names.clear();
    if !names.is_null() {
        for i in 0..unsafe { sequences::PyTuple_Size(names) } {
            let key = unsafe { sequences::PyTuple_GetItem(names, i) };
            let text = unsafe { strings::PyUnicode_AsUTF8(key) };
            if !text.is_null() {
                record.names.push(
                    unsafe { std::ffi::CStr::from_ptr(text) }
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    unsafe { object::Py_NewRef(&raw mut Py_None) }
}
unsafe extern "C" fn noargs(self_: *mut PyObject, arg: *mut PyObject) -> *mut PyObject {
    if !arg.is_null() {
        unsafe { errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    unsafe { record(self_, ptr::null_mut(), &[], ptr::null_mut()) }
}
unsafe extern "C" fn one(self_: *mut PyObject, arg: *mut PyObject) -> *mut PyObject {
    unsafe { record(self_, ptr::null_mut(), &[arg], ptr::null_mut()) }
}
unsafe extern "C" fn fast(
    self_: *mut PyObject,
    args: *mut *mut PyObject,
    nargs: Py_ssize_t,
) -> *mut PyObject {
    let args = if nargs == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, nargs as usize) }
    };
    unsafe { record(self_, ptr::null_mut(), args, ptr::null_mut()) }
}
unsafe extern "C" fn fast_kw(
    self_: *mut PyObject,
    args: *mut *mut PyObject,
    nargs: Py_ssize_t,
    names: *mut PyObject,
) -> *mut PyObject {
    let keywords = if names.is_null() {
        0
    } else {
        unsafe { sequences::PyTuple_Size(names) }
    };
    let total = nargs + keywords;
    let args = if total == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, total as usize) }
    };
    unsafe { record(self_, ptr::null_mut(), args, names) }
}
unsafe extern "C" fn method(
    self_: *mut PyObject,
    class: *mut PyTypeObject,
    args: *mut *mut PyObject,
    nargs: usize,
    names: *mut PyObject,
) -> *mut PyObject {
    let keywords = if names.is_null() {
        0
    } else {
        (unsafe { sequences::PyTuple_Size(names) }) as usize
    };
    let total = nargs + keywords;
    let args = if total == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(args, total) }
    };
    unsafe { record(self_, class, args, names) }
}
unsafe extern "C" fn varargs(self_: *mut PyObject, tuple: *mut PyObject) -> *mut PyObject {
    let args: Vec<_> = (0..unsafe { sequences::PyTuple_Size(tuple) })
        .map(|i| unsafe { sequences::PyTuple_GetItem(tuple, i) })
        .collect();
    unsafe { record(self_, ptr::null_mut(), &args, ptr::null_mut()) }
}
unsafe extern "C" fn varargs_kw(
    self_: *mut PyObject,
    positional: *mut PyObject,
    dict: *mut PyObject,
) -> *mut PyObject {
    let mut args: Vec<_> = (0..unsafe { sequences::PyTuple_Size(positional) })
        .map(|i| unsafe { sequences::PyTuple_GetItem(positional, i) })
        .collect();
    let mut names = Vec::new();
    if !dict.is_null() {
        let (mut pos, mut key, mut value) = (0, ptr::null_mut(), ptr::null_mut());
        while unsafe { mapping::PyDict_Next(dict, &raw mut pos, &raw mut key, &raw mut value) } != 0
        {
            names.push(key);
            args.push(value);
        }
    }
    let names = unsafe { tuple(&names) };
    let result = unsafe { record(self_, ptr::null_mut(), &args, names) };
    unsafe { refcount::Py_DECREF(names) };
    result
}
unsafe fn tuple(values: &[*mut PyObject]) -> *mut PyObject {
    let tuple = unsafe { sequences::PyTuple_New(values.len() as Py_ssize_t) };
    assert!(!tuple.is_null());
    for (i, &value) in values.iter().enumerate() {
        unsafe { refcount::Py_INCREF(value) };
        assert_eq!(
            unsafe { sequences::PyTuple_SetItem(tuple, i as Py_ssize_t, value) },
            0
        );
    }
    tuple
}
fn definition(flags: i32, function: *const ()) -> PyMethodDef {
    PyMethodDef {
        ml_name: c"probe".as_ptr(),
        ml_meth: Some(unsafe { std::mem::transmute::<*const (), PyCFunction>(function) }),
        ml_flags: flags,
        ml_doc: ptr::null(),
    }
}

#[test]
fn cfunction_convention_matrix_uses_identical_vectorcall_and_tpcall_carriers() {
    let _lock = LOCK.lock().unwrap();
    setup();
    let cases = [
        (METH_NOARGS, noargs as *const (), false),
        (METH_O, one as *const (), false),
        (METH_VARARGS, varargs as *const (), false),
        (METH_VARARGS | METH_KEYWORDS, varargs_kw as *const (), true),
        (METH_FASTCALL, fast as *const (), false),
        (METH_FASTCALL | METH_KEYWORDS, fast_kw as *const (), true),
        (
            METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
            method as *const (),
            true,
        ),
    ];
    unsafe {
        let first = numbers::PyLong_FromLong(31);
        let second = numbers::PyLong_FromLong(47);
        let key = strings::PyUnicode_FromString(c"answer".as_ptr());
        assert!(!key.is_null());
        let names = tuple(&[key]);
        let empty_names = tuple(&[]);
        let empty_dict = mapping::PyDict_New();
        for (flags, target, keywords) in cases {
            let mut def = definition(flags, target);
            let class = if flags & METH_METHOD != 0 {
                &raw mut PyLong_Type
            } else {
                ptr::null_mut()
            };
            let function =
                object::PyCMethod_New(&raw mut def, &raw mut Py_None, ptr::null_mut(), class);
            assert!(!function.is_null());
            assert_eq!(object::PyCFunction_Check(function), 1);
            assert!(object::PyVectorcall_Function(function).is_some());
            assert_eq!(object::PyCFunction_GetFlags(function), flags);
            let pos: &[*mut PyObject] = if flags == METH_NOARGS { &[] } else { &[first] };
            let positional = tuple(pos);
            let kwargs = mapping::PyDict_New();
            if keywords {
                assert_eq!(mapping::PyDict_SetItem(kwargs, key, second), 0);
            }
            let mut flat = pos.to_vec();
            if keywords {
                flat.push(second);
            }
            for route in 0..4 {
                let result = match route {
                    0 => object::PyObject_Call(function, positional, kwargs),
                    1 => object::molt_cfunction_call(function, positional, kwargs),
                    2 => object::PyObject_Vectorcall(
                        function,
                        flat.as_mut_ptr(),
                        pos.len(),
                        if keywords { names } else { empty_names },
                    ),
                    _ => object::PyObject_VectorcallDict(
                        function,
                        flat.as_mut_ptr(),
                        pos.len(),
                        kwargs,
                    ),
                };
                assert!(!result.is_null(), "flags={flags:#x}, route={route}");
                assert!(errors::PyErr_Occurred().is_null());
                let observed = REC.lock().unwrap();
                let expected: Vec<i64> = if flags == METH_NOARGS {
                    vec![]
                } else if keywords {
                    vec![31, 47]
                } else {
                    vec![31]
                };
                assert_eq!(observed.values, expected);
                assert_eq!(
                    observed.names,
                    if keywords {
                        vec!["answer".to_owned()]
                    } else {
                        vec![]
                    }
                );
                assert_eq!(observed.receiver, (&raw mut Py_None) as usize);
                assert_eq!(observed.class, class as usize);
                drop(observed);
                refcount::Py_DECREF(result);
            }
            // Empty kwargs are absent even for non-keyword conventions.
            let result = object::PyObject_Call(function, positional, empty_dict);
            assert!(!result.is_null());
            refcount::Py_DECREF(result);
            refcount::Py_DECREF(kwargs);
            refcount::Py_DECREF(positional);
            refcount::Py_DECREF(function);
        }
        for value in [first, second, key, names, empty_names, empty_dict] {
            refcount::Py_DECREF(value);
        }
    }
}

#[test]
fn cfunction_constructor_admits_only_complete_conventions_and_class_pairings() {
    let _lock = LOCK.lock().unwrap();
    setup();
    unsafe {
        for flags in [
            0,
            METH_KEYWORDS,
            METH_METHOD,
            METH_METHOD | METH_FASTCALL,
            METH_NOARGS | METH_O,
            METH_VARARGS | METH_FASTCALL,
            METH_NOARGS | METH_KEYWORDS,
            METH_O | METH_CLASS | METH_STATIC,
            METH_O | 0x4000,
        ] {
            assert!(
                CFunctionConvention::from_flags(flags).is_none(),
                "flags={flags:#x}"
            );
            let mut def = definition(flags, noargs as *const ());
            assert!(object::PyCFunction_New(&raw mut def, ptr::null_mut()).is_null());
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
        }
        let mut method_def = definition(
            METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
            method as *const (),
        );
        assert!(object::PyCFunction_New(&raw mut method_def, ptr::null_mut()).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        let mut plain = definition(METH_O, one as *const ());
        assert!(
            object::PyCMethod_New(
                &raw mut plain,
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut PyLong_Type
            )
            .is_null()
        );
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        plain.ml_meth = None;
        assert!(object::PyCFunction_New(&raw mut plain, ptr::null_mut()).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        for modifier in [METH_CLASS, METH_STATIC, METH_COEXIST] {
            assert_eq!(
                CFunctionConvention::from_flags(METH_O | modifier),
                Some(CFunctionConvention::OneObject)
            );
        }
    }
}

unsafe extern "C" fn null_without_error(
    _self: *mut PyObject,
    _arg: *mut PyObject,
) -> *mut PyObject {
    ptr::null_mut()
}
unsafe extern "C" fn result_with_error(_self: *mut PyObject, _arg: *mut PyObject) -> *mut PyObject {
    unsafe {
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"callee failed".as_ptr(),
        );
        object::Py_NewRef(&raw mut Py_None)
    }
}

#[test]
fn cfunction_result_validation_and_bad_carriers_do_not_invoke_target() {
    let _lock = LOCK.lock().unwrap();
    setup();
    unsafe {
        for target in [
            null_without_error as *const (),
            result_with_error as *const (),
        ] {
            let mut def = definition(METH_NOARGS, target);
            let function = object::PyCFunction_New(&raw mut def, ptr::null_mut());
            for route in 0..2 {
                let result = if route == 0 {
                    object::PyObject_CallNoArgs(function)
                } else {
                    object::molt_cfunction_call(function, ptr::null_mut(), ptr::null_mut())
                };
                assert!(result.is_null());
                assert_eq!(
                    errors::PyErr_Occurred(),
                    (&raw mut PyExc_SystemError).cast()
                );
                errors::PyErr_Clear();
            }
            refcount::Py_DECREF(function);
        }
        let mut def = definition(METH_NOARGS, noargs as *const ());
        let function = object::PyCFunction_New(&raw mut def, ptr::null_mut());
        let integer = numbers::PyLong_FromLong(2);
        for (args, kwargs) in [(integer, ptr::null_mut()), (ptr::null_mut(), integer)] {
            assert!(object::molt_cfunction_call(function, args, kwargs).is_null());
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
        }
        let args = tuple(&[integer]);
        assert!(object::PyObject_Call(function, args, ptr::null_mut()).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        let key = strings::PyUnicode_FromString(c"forbidden".as_ptr());
        let names = tuple(&[key]);
        let mut values = [integer];
        assert!(object::PyObject_Vectorcall(function, values.as_mut_ptr(), 0, names).is_null());
        assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_TypeError).cast());
        errors::PyErr_Clear();
        assert!(
            object::PyObject_Vectorcall(function, ptr::null_mut(), 1, ptr::null_mut()).is_null()
        );
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        let mut kwdef = definition(METH_FASTCALL | METH_KEYWORDS, fast_kw as *const ());
        let kwfunction = object::PyCFunction_New(&raw mut kwdef, ptr::null_mut());
        let invalid_names = tuple(&[integer]);
        assert!(
            object::PyObject_Vectorcall(kwfunction, values.as_mut_ptr(), 0, invalid_names)
                .is_null()
        );
        assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_TypeError).cast());
        errors::PyErr_Clear();
        assert_eq!(REC.lock().unwrap().calls, 0);
        for value in [
            invalid_names,
            kwfunction,
            names,
            key,
            args,
            integer,
            function,
        ] {
            refcount::Py_DECREF(value);
        }
    }
}

#[test]
fn cmethod_deallocation_releases_defining_class_and_receiver_exactly_once() {
    let _lock = LOCK.lock().unwrap();
    setup();
    unsafe {
        // Stack-owned foreign objects keep a sentinel owner, so reaching zero
        // would be observable instead of allowing a leaked class to pass.
        let mut class: PyTypeObject = std::mem::zeroed();
        class.ob_base.ob_base.ob_refcnt = 1;
        class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        let mut receiver = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyBaseObject_Type,
        };
        let mut module = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyBaseObject_Type,
        };
        let mut def = definition(
            METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
            method as *const (),
        );
        let function = object::PyCMethod_New(
            &raw mut def,
            &raw mut receiver,
            &raw mut module,
            &raw mut class,
        );
        assert!(!function.is_null());
        assert_eq!(class.ob_base.ob_base.ob_refcnt, 2);
        assert_eq!(receiver.ob_refcnt, 2);
        assert_eq!(module.ob_refcnt, 2);
        refcount::Py_DECREF(function);
        assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
        assert_eq!(receiver.ob_refcnt, 1);
        assert_eq!(module.ob_refcnt, 1);
    }
}

thread_local! {
    static REENTRY_DICT: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
    static REENTRY_TARGET: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
    static REENTRY_RETAINED: Cell<bool> = const { Cell::new(false) };
    static VALUE_REFS_BEFORE: Cell<Py_ssize_t> = const { Cell::new(0) };
    static CALLEE_ERROR_VALUE: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
    static DESTRUCTOR_CALLS: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn reentrant_keywords(
    self_: *mut PyObject,
    args: *mut *mut PyObject,
    nargs: Py_ssize_t,
    names: *mut PyObject,
) -> *mut PyObject {
    let key = unsafe { sequences::PyTuple_GetItem(names, 0) };
    let value = unsafe { *args.add(nargs as usize) };
    REENTRY_RETAINED.set(unsafe { (*value).ob_refcnt } > VALUE_REFS_BEFORE.get());
    let dictionary = REENTRY_DICT.get();
    if unsafe { mapping::PyDict_DelItem(dictionary, key) } != 0 {
        return ptr::null_mut();
    }
    let nested = unsafe { object::PyObject_CallNoArgs(REENTRY_TARGET.get()) };
    if nested.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        refcount::Py_DECREF(nested);
        fast_kw(self_, args, nargs, names)
    }
}

#[test]
fn cfunction_keyword_snapshot_retains_values_across_dict_mutation_and_reentry() {
    let _lock = LOCK.lock().unwrap();
    setup();
    unsafe {
        let mut inner_def = definition(METH_NOARGS, noargs as *const ());
        let inner = object::PyCFunction_New(&raw mut inner_def, ptr::null_mut());
        let mut outer_def = definition(
            METH_FASTCALL | METH_KEYWORDS,
            reentrant_keywords as *const (),
        );
        let outer = object::PyCFunction_New(&raw mut outer_def, ptr::null_mut());
        let value = numbers::PyLong_FromLong(7349);
        let key = strings::PyUnicode_FromString(c"owned".as_ptr());
        let kwargs = mapping::PyDict_New();
        assert_eq!(mapping::PyDict_SetItem(kwargs, key, value), 0);
        REENTRY_DICT.set(kwargs);
        REENTRY_TARGET.set(inner);
        REENTRY_RETAINED.set(false);
        VALUE_REFS_BEFORE.set((*value).ob_refcnt);
        let result = object::molt_cfunction_call(outer, ptr::null_mut(), kwargs);
        assert!(!result.is_null());
        assert!(errors::PyErr_Occurred().is_null());
        assert!(
            REENTRY_RETAINED.get(),
            "flattened keyword value needs an independent owner"
        );
        assert_eq!(mapping::PyDict_Size(kwargs), 0);
        assert_eq!(REC.lock().unwrap().values, [7349]);
        assert_eq!(REC.lock().unwrap().calls, 2);
        REENTRY_DICT.set(ptr::null_mut());
        REENTRY_TARGET.set(ptr::null_mut());
        for object in [result, kwargs, key, value, outer, inner] {
            refcount::Py_DECREF(object);
        }
    }
}

unsafe extern "C" fn destructive_argument_drop(object: *mut PyObject) {
    DESTRUCTOR_CALLS.set(DESTRUCTOR_CALLS.get() + 1);
    unsafe {
        errors::PyErr_SetString((&raw mut PyExc_KeyError).cast(), c"cleanup error".as_ptr());
        drop(Box::from_raw(object));
    }
}
unsafe extern "C" fn fails_after_releasing_external_owner(
    _self: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        let argument = sequences::PyTuple_GetItem(args, 0);
        // The temporary positional tuple must be the final owner, retired only
        // after this callee has published its exact error indicator.
        refcount::Py_DECREF(argument);
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"original error".as_ptr(),
        );
    }
    let pending = errors::take_current_error().unwrap();
    CALLEE_ERROR_VALUE.set(pending.value);
    errors::restore_current_error_exact(pending);
    ptr::null_mut()
}

#[test]
fn cfunction_temporary_cleanup_preserves_exact_callee_exception_during_reentry() {
    let _lock = LOCK.lock().unwrap();
    setup();
    unsafe {
        let mut typ: PyTypeObject = std::mem::zeroed();
        typ.tp_dealloc = Some(destructive_argument_drop);
        let argument = Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut typ,
        }));
        DESTRUCTOR_CALLS.set(0);
        let result = CFunctionConvention::VarArgs.invoke(
            fails_after_releasing_external_owner as *const (),
            ptr::null_mut(),
            ptr::null_mut(),
            &[argument],
            1,
            ptr::null_mut(),
            || "cleanup_probe".to_owned(),
        );
        assert!(result.is_null());
        assert_eq!(DESTRUCTOR_CALLS.get(), 1);
        let error =
            errors::take_current_error().expect("callee error must survive argument destruction");
        assert_eq!(error.exc_type, (&raw mut PyExc_ValueError).cast());
        assert_eq!(error.value, CALLEE_ERROR_VALUE.get());
        drop(error);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn cfunction_static_modifier_masks_receiver_without_losing_owned_storage() {
    let _lock = LOCK.lock().unwrap();
    setup();
    unsafe {
        let mut receiver = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyBaseObject_Type,
        };
        let mut def = definition(METH_NOARGS | METH_STATIC, noargs as *const ());
        let function = object::PyCFunction_New(&raw mut def, &raw mut receiver);
        assert!(!function.is_null());
        assert_eq!(receiver.ob_refcnt, 2);
        assert!(object::PyCFunction_GetSelf(function).is_null());
        let result = object::PyObject_CallNoArgs(function);
        assert!(!result.is_null());
        assert_eq!(REC.lock().unwrap().receiver, 0);
        refcount::Py_DECREF(result);
        refcount::Py_DECREF(function);
        assert_eq!(receiver.ob_refcnt, 1);
    }
}
