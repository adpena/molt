//! Native tp_methods publication, binding and callback custody through the
//! public C API and the existing real-runtime readiness fixture.

use super::{NativeType, dict_value_by_name, init, method_sentinel};
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{
    errors, mapping, memory, object, refcount, sequences, strings, typeobj,
};
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_int, c_void};
use std::ptr;

#[derive(Debug, PartialEq)]
struct Observed {
    receiver: usize,
    defining_class: usize,
    positional: Vec<usize>,
    keywords: Vec<(String, usize)>,
}

thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
    static FAILURE: Cell<*mut PyObject> = const { Cell::new(ptr::null_mut()) };
    static OBSERVED: RefCell<Option<Observed>> = const { RefCell::new(None) };
}

unsafe fn finish(receiver: *mut PyObject, observation: Observed) -> *mut PyObject {
    CALLS.with(|calls| calls.set(calls.get() + 1));
    OBSERVED.with(|slot| *slot.borrow_mut() = Some(observation));
    let failure = FAILURE.with(Cell::get);
    if !failure.is_null() {
        unsafe { errors::PyErr_SetObject((&raw mut PyExc_ValueError).cast(), failure) };
        return ptr::null_mut();
    }
    let result = if receiver.is_null() {
        &raw mut Py_None
    } else {
        receiver
    };
    unsafe { refcount::Py_INCREF(result) };
    result
}

unsafe extern "C" fn echo(receiver: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    assert!(args.is_null());
    unsafe {
        finish(
            receiver,
            Observed {
                receiver: receiver.addr(),
                defining_class: 0,
                positional: Vec::new(),
                keywords: Vec::new(),
            },
        )
    }
}

unsafe extern "C" fn defining_method(
    receiver: *mut PyObject,
    class: *mut PyTypeObject,
    args: *mut *mut PyObject,
    positional: usize,
    names: *mut PyObject,
) -> *mut PyObject {
    let keyword_count = if names.is_null() {
        0
    } else {
        unsafe { sequences::PyTuple_Size(names) }
    };
    let keywords = (0..keyword_count)
        .map(|index| unsafe {
            let key = sequences::PyTuple_GetItem(names, index);
            (
                CStr::from_ptr(strings::PyUnicode_AsUTF8(key))
                    .to_string_lossy()
                    .into_owned(),
                (*args.add(positional + index as usize)).addr(),
            )
        })
        .collect();
    let observation = Observed {
        receiver: receiver.addr(),
        defining_class: class.addr(),
        positional: (0..positional)
            .map(|index| unsafe { (*args.add(index)).addr() })
            .collect(),
        keywords,
    };
    unsafe { finish(receiver, observation) }
}

unsafe extern "C" fn keyword_method(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let count = unsafe { sequences::PyTuple_Size(args) };
    let mut keywords = Vec::new();
    if !kwargs.is_null() {
        let (mut position, mut key, mut value) = (0, ptr::null_mut(), ptr::null_mut());
        while unsafe {
            mapping::PyDict_Next(kwargs, &raw mut position, &raw mut key, &raw mut value)
        } != 0
        {
            keywords.push((
                unsafe { CStr::from_ptr(strings::PyUnicode_AsUTF8(key)) }
                    .to_string_lossy()
                    .into_owned(),
                value.addr(),
            ));
        }
    }
    let observation = Observed {
        receiver: receiver.addr(),
        defining_class: 0,
        positional: (0..count)
            .map(|index| unsafe { sequences::PyTuple_GetItem(args, index) }.addr())
            .collect(),
        keywords,
    };
    unsafe { finish(receiver, observation) }
}

fn declaration(name: &'static CStr, flags: c_int, target: *const ()) -> PyMethodDef {
    PyMethodDef {
        ml_name: name.as_ptr(),
        ml_meth: Some(unsafe { std::mem::transmute::<*const (), PyCFunction>(target) }),
        ml_flags: flags,
        ml_doc: c"probe($self, /)\n--\n\nReturn the bound receiver.".as_ptr(),
    }
}

unsafe fn assert_type_error(result: *mut PyObject) {
    assert!(result.is_null());
    assert_eq!(
        unsafe { errors::PyErr_Occurred() },
        (&raw mut PyExc_TypeError).cast()
    );
    unsafe { errors::PyErr_Clear() };
}

#[test]
fn methods_publish_descriptors_and_bind_the_exact_inherited_receiver() {
    let _thread = init();
    CALLS.with(|calls| calls.set(0));
    let mut methods = [
        declaration(c"probe", METH_NOARGS, echo as _),
        method_sentinel(),
    ];
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.tp_name = c"MethodOwner".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    owner.tp_methods = methods.as_mut_ptr();
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let mut subtype = NativeType::<PyTypeObject>::new();
    subtype.tp_name = c"MethodSubtype".as_ptr();
    subtype.tp_base = &raw mut *owner;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *subtype) }, 0);
    assert!(
        subtype.tp_methods.is_null(),
        "declarations belong only to their owner"
    );
    let owner_baseline = owner.ob_base.ob_base.ob_refcnt;
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *subtype,
    };
    let mut wrong = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut PyBaseObject_Type,
    };
    unsafe {
        let descr = dict_value_by_name(owner.tp_dict, b"probe");
        assert_eq!((*descr).ob_type, &raw mut PyMethodDescr_Type);
        assert_eq!((*descr.cast::<PyDescrObject>()).d_type, &raw mut *owner);
        assert_eq!(typeobj::PyDescr_IsData(descr), 0);
        assert_eq!(memory::PyObject_GC_IsTracked(descr), 1);
        let class_read =
            object::PyObject_GetAttrString((&raw mut *subtype).cast(), c"probe".as_ptr());
        assert_eq!(
            class_read, descr,
            "class access leaves ordinary methods unbound"
        );
        refcount::Py_DECREF(class_read);
        let bound = object::PyObject_GetAttrString(&raw mut receiver, c"probe".as_ptr());
        assert!(!bound.is_null());
        assert_eq!(object::PyCFunction_GetSelf(bound), &raw mut receiver);
        let result = object::PyObject_CallNoArgs(bound);
        assert_eq!(result, &raw mut receiver);
        refcount::Py_DECREF(result);
        let result = object::PyObject_CallOneArg(descr, &raw mut receiver);
        assert_eq!(result, &raw mut receiver);
        refcount::Py_DECREF(result);
        let get = (*(*descr).ob_type).tp_descr_get.unwrap();
        assert_type_error(get(descr, &raw mut wrong, (&raw mut *owner).cast()));
        assert_type_error(object::PyObject_CallOneArg(descr, &raw mut wrong));
        assert_type_error(object::PyObject_CallNoArgs(descr));
        assert_eq!(
            CALLS.with(Cell::get),
            2,
            "receiver and arity rejection precede C code"
        );
        refcount::Py_DECREF(bound);
    }
    assert_eq!(receiver.ob_refcnt, 1);
    assert_eq!(owner.ob_base.ob_base.ob_refcnt, owner_baseline);
}

#[test]
fn classmethod_descriptor_binds_explicit_or_inferred_subtype_and_ignores_object() {
    let _thread = init();
    let mut methods = [
        declaration(c"probe", METH_CLASS | METH_NOARGS, echo as _),
        method_sentinel(),
    ];
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.tp_name = c"ClassMethodOwner".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    owner.tp_methods = methods.as_mut_ptr();
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let mut subtype = NativeType::<PyTypeObject>::new();
    subtype.tp_name = c"ClassMethodSubtype".as_ptr();
    subtype.tp_base = &raw mut *owner;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *subtype) }, 0);
    let subtype_baseline = subtype.ob_base.ob_base.ob_refcnt;
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *subtype,
    };
    let mut unrelated = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut PyBaseObject_Type,
    };
    unsafe {
        let descr = dict_value_by_name(owner.tp_dict, b"probe");
        assert_eq!((*descr).ob_type, &raw mut PyClassMethodDescr_Type);
        assert!(object::PyVectorcall_Function(descr).is_none());
        let get = (*(*descr).ob_type).tp_descr_get.unwrap();
        for bound in [
            object::PyObject_GetAttrString((&raw mut *subtype).cast(), c"probe".as_ptr()),
            object::PyObject_GetAttrString(&raw mut receiver, c"probe".as_ptr()),
            get(descr, &raw mut receiver, ptr::null_mut()),
            get(descr, &raw mut unrelated, (&raw mut *subtype).cast()),
        ] {
            assert!(!bound.is_null());
            assert_eq!(
                object::PyCFunction_GetSelf(bound),
                (&raw mut *subtype).cast()
            );
            let result = object::PyObject_CallNoArgs(bound);
            assert_eq!(result, (&raw mut *subtype).cast());
            refcount::Py_DECREF(result);
            refcount::Py_DECREF(bound);
        }
        let result = object::PyObject_CallOneArg(descr, (&raw mut *subtype).cast());
        assert_eq!(result, (&raw mut *subtype).cast());
        refcount::Py_DECREF(result);
        assert_type_error(get(descr, ptr::null_mut(), ptr::null_mut()));
        assert_type_error(get(descr, &raw mut receiver, &raw mut unrelated));
        assert_type_error(get(
            descr,
            ptr::null_mut(),
            (&raw mut PyBaseObject_Type).cast(),
        ));
        assert_type_error(object::PyObject_CallOneArg(descr, &raw mut receiver));
    }
    assert_eq!(subtype.ob_base.ob_base.ob_refcnt, subtype_baseline);
    assert_eq!((receiver.ob_refcnt, unrelated.ob_refcnt), (1, 1));
}

#[test]
fn defining_class_keywords_and_original_errors_survive_bound_and_unbound_calls() {
    let _thread = init();
    let flags = METH_METHOD | METH_FASTCALL | METH_KEYWORDS;
    let mut methods = [
        declaration(c"defined", flags, defining_method as _),
        declaration(c"class_defined", flags | METH_CLASS, defining_method as _),
        declaration(
            c"keywords",
            METH_VARARGS | METH_KEYWORDS,
            keyword_method as _,
        ),
        method_sentinel(),
    ];
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.tp_name = c"DefiningOwner".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    owner.tp_methods = methods.as_mut_ptr();
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let mut subtype = NativeType::<PyTypeObject>::new();
    subtype.tp_name = c"DefiningSubtype".as_ptr();
    subtype.tp_base = &raw mut *owner;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *subtype) }, 0);
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *subtype,
    };
    unsafe {
        let key = strings::PyUnicode_FromString(c"named".as_ptr());
        let names = sequences::PyTuple_FromArray((&raw const key).cast(), 1);
        let positional = sequences::PyList_New(0);
        let keyword = mapping::PyDict_New();
        let failure = object::PyObject_CallNoArgs((&raw mut PyExc_ValueError).cast());
        assert!(!failure.is_null());
        for (name, self_, defining_class) in [
            (c"defined", &raw mut receiver, &raw mut *owner),
            (
                c"class_defined",
                (&raw mut *subtype).cast(),
                &raw mut *owner,
            ),
            (c"keywords", &raw mut receiver, ptr::null_mut()),
        ] {
            let descr = dict_value_by_name(owner.tp_dict, name.to_bytes());
            let get = (*(*descr).ob_type).tp_descr_get.unwrap();
            let bound = get(descr, &raw mut receiver, (&raw mut *subtype).cast());
            assert!(!bound.is_null());
            if !defining_class.is_null() {
                assert_eq!((*bound).ob_type, &raw mut PyCMethod_Type);
                assert_eq!((*bound.cast::<PyCMethodObject>()).mm_class, &raw mut *owner);
            }
            for callable in [bound, descr] {
                let mut values = vec![positional, keyword];
                let mut count = 1;
                if callable == descr {
                    values.insert(0, self_);
                    count += 1;
                }
                for fail in [false, true] {
                    FAILURE.with(|slot| slot.set(if fail { failure } else { ptr::null_mut() }));
                    let result =
                        object::PyObject_Vectorcall(callable, values.as_mut_ptr(), count, names);
                    if fail {
                        assert!(result.is_null());
                        let raised = errors::PyErr_GetRaisedException();
                        let raised_owner = refcount::OwnedPyObject::from_owned(raised);
                        assert_eq!(
                            raised, failure,
                            "callback exception identity survives packing and cleanup"
                        );
                        drop(raised_owner);
                    } else {
                        assert_eq!(result, self_);
                        refcount::Py_DECREF(result);
                    }
                    OBSERVED.with(|slot| {
                        assert_eq!(
                            slot.borrow().as_ref(),
                            Some(&Observed {
                                receiver: self_.addr(),
                                defining_class: defining_class.addr(),
                                positional: vec![positional.addr()],
                                keywords: vec![("named".to_owned(), keyword.addr())],
                            })
                        )
                    });
                }
            }
            FAILURE.with(|slot| slot.set(ptr::null_mut()));
            refcount::Py_DECREF(bound);
        }
        let descr = dict_value_by_name(owner.tp_dict, b"defined");
        assert_type_error(((*(*descr).ob_type).tp_descr_get.unwrap())(
            descr,
            &raw mut receiver,
            ptr::null_mut(),
        ));
        for value in [failure, keyword, positional, names, key] {
            refcount::Py_DECREF(value);
        }
    }
    assert_eq!(receiver.ob_refcnt, 1);
}

unsafe extern "C" fn visit(edge: *mut PyObject, context: *mut c_void) -> c_int {
    unsafe { (*context.cast::<Vec<usize>>()).push(edge.addr()) };
    0
}

#[test]
fn method_descriptor_metadata_and_gc_edges_share_the_common_header() {
    let _thread = init();
    let mut definition = declaration(c"probe", METH_NOARGS, echo as _);
    let mut class_definition = declaration(c"probe", METH_CLASS | METH_NOARGS, echo as _);
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.tp_name = c"MetadataOwner".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let baseline = owner.ob_base.ob_base.ob_refcnt;
    unsafe {
        for descr in [
            typeobj::PyDescr_NewMethod(&raw mut *owner, &raw mut definition),
            typeobj::PyDescr_NewClassMethod(&raw mut *owner, &raw mut class_definition),
        ] {
            assert!(!descr.is_null());
            assert_eq!(memory::PyObject_GC_IsTracked(descr), 1);
            let objclass = object::PyObject_GetAttrString(descr, c"__objclass__".as_ptr());
            assert_eq!(objclass, (&raw mut *owner).cast());
            refcount::Py_DECREF(objclass);
            for (name, expected) in [
                (c"__name__", c"probe"),
                (c"__qualname__", c"MetadataOwner.probe"),
                (c"__doc__", c"Return the bound receiver."),
                (c"__text_signature__", c"($self, /)"),
            ] {
                let value = object::PyObject_GetAttrString(descr, name.as_ptr());
                assert!(!value.is_null());
                let text = strings::PyUnicode_AsUTF8(value);
                assert!(!text.is_null());
                assert_eq!(CStr::from_ptr(text), expected);
                refcount::Py_DECREF(value);
            }
            let representation = typeobj::PyObject_Repr(descr);
            assert!(!representation.is_null());
            assert_eq!(
                CStr::from_ptr(strings::PyUnicode_AsUTF8(representation)),
                c"<method 'probe' of 'MetadataOwner' objects>"
            );
            refcount::Py_DECREF(representation);
            let header = descr.cast::<PyDescrObject>();
            let mut edges: Vec<usize> = Vec::new();
            ((*(*descr).ob_type).tp_traverse.unwrap())(
                descr,
                visit as *const () as _,
                (&raw mut edges).cast(),
            );
            assert_eq!(
                edges,
                [
                    (&raw mut *owner).addr(),
                    (*header).d_name.addr(),
                    (*header).d_qualname.addr()
                ]
            );
            refcount::Py_DECREF(descr);
            assert!(!crate::object::gc::native_gc_is_enrolled(descr.addr()));
        }
    }
    assert_eq!(owner.ob_base.ob_base.ob_refcnt, baseline);
}

#[test]
fn static_method_publication_uses_the_runtime_wrapper_without_binding_self() {
    let _thread = init();
    let mut methods = [
        declaration(c"probe", METH_STATIC | METH_NOARGS, echo as _),
        method_sentinel(),
    ];
    let mut owner = NativeType::<PyTypeObject>::new();
    owner.tp_name = c"StaticMethodOwner".as_ptr();
    owner.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
    owner.tp_methods = methods.as_mut_ptr();
    assert_eq!(unsafe { typeobj::PyType_Ready(&raw mut *owner) }, 0);
    let mut receiver = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *owner,
    };
    unsafe {
        let wrapper = dict_value_by_name(owner.tp_dict, b"probe");
        let wrapper_type = typeobj::PyObject_Type(wrapper);
        assert_eq!(wrapper_type, (&raw mut PyStaticMethod_Type).cast());
        refcount::Py_DECREF(wrapper_type);
        let original = object::PyObject_GetAttrString(wrapper, c"__func__".as_ptr());
        assert!(!original.is_null());
        type Constructor = unsafe extern "C" fn(*mut PyObject) -> *mut PyObject;
        for (construct, class) in [
            (
                typeobj::PyStaticMethod_New as Constructor,
                &raw mut PyStaticMethod_Type,
            ),
            (
                typeobj::PyClassMethod_New as Constructor,
                &raw mut PyClassMethod_Type,
            ),
        ] {
            let constructed = construct(original);
            assert!(!constructed.is_null());
            assert_ne!(
                constructed, original,
                "wrapper constructors create descriptor storage"
            );
            let actual = typeobj::PyObject_Type(constructed);
            assert_eq!(actual, class.cast());
            refcount::Py_DECREF(actual);
            let target = object::PyObject_GetAttrString(constructed, c"__func__".as_ptr());
            assert_eq!(target, original);
            refcount::Py_DECREF(target);
            refcount::Py_DECREF(constructed);
        }
        for accessed in [(&raw mut *owner).cast(), &raw mut receiver] {
            let callable = object::PyObject_GetAttrString(accessed, c"probe".as_ptr());
            assert_eq!(callable, original);
            assert!(object::PyCFunction_GetSelf(callable).is_null());
            let result = object::PyObject_CallNoArgs(callable);
            assert_eq!(result, &raw mut Py_None);
            refcount::Py_DECREF(result);
            refcount::Py_DECREF(callable);
        }
        OBSERVED.with(|slot| assert_eq!(slot.borrow().as_ref().unwrap().receiver, 0));
        refcount::Py_DECREF(original);
    }
    assert_eq!(receiver.ob_refcnt, 1);
}

#[test]
fn cfunction_publication_retains_module_without_observing_open_container() {
    let _thread = init();
    let mut method = declaration(c"opaque_module", METH_NOARGS, echo as _);
    unsafe {
        // CPython accepts an arbitrary object here. An open construction list
        // may be retained, but cannot yet be semantically observed as a list.
        let module = sequences::PyList_New(1);
        assert!(!module.is_null());
        let baseline = (*module).ob_refcnt;
        let callable = object::PyCFunction_NewEx(&raw mut method, ptr::null_mut(), module);
        assert!(!callable.is_null());
        assert!(errors::PyErr_Occurred().is_null());
        assert_eq!((*callable.cast::<PyCFunctionObject>()).m_module, module);
        assert_eq!((*module).ob_refcnt, baseline + 1);
        refcount::Py_INCREF(&raw mut Py_None);
        assert_eq!(sequences::PyList_SetItem(module, 0, &raw mut Py_None), 0);
        assert_eq!(sequences::PyList_GetItem(module, 0), &raw mut Py_None);
        refcount::Py_DECREF(callable);
        assert_eq!((*module).ob_refcnt, baseline);
        refcount::Py_DECREF(module);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn native_alias_of_managed_type_completes_its_own_declarations() {
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _thread = init();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = crate::attr_name_bits_from_bytes(py, b"NativeAliasProjection").unwrap();
            let class = crate::molt_class_new(name);
            crate::dec_ref_bits(py, name);
            crate::molt_class_set_base(class, crate::builtin_classes(py).object);
            crate::object::class_finish_definition(
                py,
                crate::MoltObject::from_bits(class).as_ptr().unwrap(),
            )
            .unwrap();
            let managed = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class);
            assert!(!managed.is_null());
            assert_eq!(typeobj::PyType_Ready(managed.cast()), 0);
            let dictionary = (*managed.cast::<PyTypeObject>()).tp_dict;
            let references = (*dictionary).ob_refcnt;
            let mut methods = [
                declaration(c"alias_probe", METH_NOARGS, echo as _),
                method_sentinel(),
            ];
            let mut alias = NativeType::<PyTypeObject>::new();
            alias.tp_name = c"NativeAliasProjection".as_ptr();
            alias.tp_methods = methods.as_mut_ptr();
            let alias_pointer = &raw mut *alias;
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias_pointer.cast(), class, false)
                .unwrap();

            assert_eq!(typeobj::PyType_Ready(alias_pointer), 0);
            assert_eq!(alias.tp_dict, dictionary);
            assert_eq!((*dictionary).ob_refcnt, references + 1);
            let descriptor = dict_value_by_name(dictionary, b"alias_probe");
            assert_eq!((*descriptor).ob_type, &raw mut PyMethodDescr_Type);
            assert_eq!(
                (*descriptor.cast::<PyMethodDescrObject>()).d_common.d_type,
                alias_pointer
            );
            assert!(errors::PyErr_Occurred().is_null());

            // A separately admitted native alias is another physical cache and
            // watcher consumer of the same logical type mutation. Origin does
            // not remove it from publication custody.
            static WATCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            unsafe extern "C" fn watched(_: *mut PyObject) -> c_int {
                WATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                0
            }
            let watcher = typeobj::PyType_AddWatcher(Some(watched));
            assert!(watcher >= 0);
            assert_eq!(typeobj::PyType_Watch(watcher, managed), 0);
            assert_eq!(typeobj::PyType_Watch(watcher, alias_pointer.cast()), 0);
            assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(managed.cast()), 1);
            assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(alias_pointer), 1);
            WATCHES.store(0, std::sync::atomic::Ordering::Relaxed);
            let owned_name =
                crate::class_name_bits(crate::MoltObject::from_bits(class).as_ptr().unwrap());
            let abstract_methods =
                crate::MoltObject::from_ptr(crate::alloc_tuple(py, &[owned_name])).bits();
            crate::builtins::attributes::type_metadata::write(
                py,
                class,
                typeobj::TypeAttributeField::AbstractMethods,
                Some(abstract_methods),
            );
            assert!(!crate::exception_pending(py));
            assert_eq!(WATCHES.load(std::sync::atomic::Ordering::Relaxed), 2);
            for pointer in [managed.cast::<PyTypeObject>(), alias_pointer] {
                assert_eq!((*pointer).tp_version_tag, 0);
                assert_ne!((*pointer).tp_flags & Py_TPFLAGS_IS_ABSTRACT, 0);
            }
            assert_eq!(typeobj::PyType_Unwatch(watcher, managed), 0);
            assert_eq!(typeobj::PyType_Unwatch(watcher, alias_pointer.cast()), 0);
            assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
            crate::builtins::attributes::type_metadata::write(
                py,
                class,
                typeobj::TypeAttributeField::AbstractMethods,
                None,
            );
            crate::dec_ref_bits(py, abstract_methods);

            // The declaration lives in the shared namespace; retire it before its
            // stack-owned method table and native shell leave scope.
            assert_eq!(
                mapping::PyDict_DelItemString(dictionary, c"alias_probe".as_ptr()),
                0
            );
            assert!(
                GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(alias_pointer.cast(), class)
            );
            drop(alias);
            assert_eq!((*dictionary).ob_refcnt, references);
            refcount::Py_DECREF(managed);
            crate::dec_ref_bits(py, class);
        }
    });
}
