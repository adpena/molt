use super::native_test_fixture::NativeType;
use crate::concurrency::gil::with_gil;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, mapping, memory, numbers, object, refcount, typeobj};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

static OBJECT_FREES: AtomicUsize = AtomicUsize::new(0);
static TYPE_FREES: AtomicUsize = AtomicUsize::new(0);

type BufferConstructorProbe =
    unsafe extern "C" fn(*mut PyObject, std::os::raw::c_int) -> *mut PyObject;

unsafe extern "C" fn buffer_constructor_probe(
    object: *mut PyObject,
    kind: std::os::raw::c_int,
) -> *mut PyObject {
    with_gil(|_py| unsafe {
        let Some(bits) = molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(object)
        else {
            return ptr::null_mut();
        };
        let result = if kind == 0 {
            crate::object::ops_bytes::molt_bytes_from_obj(bits)
        } else if kind == 1 {
            crate::object::ops_bytes::molt_bytearray_from_obj(bits)
        } else {
            crate::object::ops_memoryview::molt_memoryview_new(bits)
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| crate::dec_ref_bits(&_py, bits));
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .owned_result_to_pyobj(super::owned_result_from_pending(result))
    })
}

type TypeFactoryProbe = unsafe extern "C" fn(
    *mut PyTypeObject,
    *mut PyObject,
    *mut PyObject,
    *mut PyModuleDef,
    *mut PyModuleDef,
    *mut PyModuleDef,
    *mut *mut PyObject,
    *mut usize,
    BufferConstructorProbe,
) -> std::os::raw::c_int;

unsafe extern "C" {
    fn molt_linked_type_factory_probe(
        metaclass: *mut PyTypeObject,
        module_a: *mut PyObject,
        module_b: *mut PyObject,
        def_a: *mut PyModuleDef,
        def_b: *mut PyModuleDef,
        missing: *mut PyModuleDef,
        types: *mut *mut PyObject,
        buffer_layout: *mut usize,
        constructor: BufferConstructorProbe,
    ) -> std::os::raw::c_int;
    fn molt_public_type_factory_probe(
        metaclass: *mut PyTypeObject,
        module_a: *mut PyObject,
        module_b: *mut PyObject,
        def_a: *mut PyModuleDef,
        def_b: *mut PyModuleDef,
        missing: *mut PyModuleDef,
        types: *mut *mut PyObject,
        buffer_layout: *mut usize,
        constructor: BufferConstructorProbe,
    ) -> std::os::raw::c_int;
}

/// The same C consumer observes all public builtin addresses with an exact
/// custom exception pending, including after runtime shutdown/reinitialization.
pub(super) unsafe fn assert_c_exception_symbols_preserve_pending() {
    use refcount::OwnedPyObject;

    unsafe extern "C" {
        fn molt_linked_type_identity_probe_exception_symbols(
            expected: *const *mut PyObject,
            count: usize,
            pending: *mut PyObject,
        ) -> std::os::raw::c_int;
        fn molt_public_type_identity_probe_exception_symbols(
            expected: *const *mut PyObject,
            count: usize,
            pending: *mut PyObject,
        ) -> std::os::raw::c_int;
    }

    molt_cpython_abi_test_support::link();
    let expected: Vec<_> = molt_cpython_abi::abi_types::exc_singleton_ptrs()
        .into_iter()
        .filter(|pointer| {
            molt_cpython_abi::abi_types::exc_singleton_name(*pointer)
                != Some("PyExc_ExceptionGroup")
        })
        .collect();
    unsafe {
        let class = OwnedPyObject::from_owned(errors::PyErr_NewException(
            c"probe.BufferSubclass".as_ptr(),
            (&raw mut PyExc_BufferError).cast(),
            ptr::null_mut(),
        ));
        assert!(!class.as_ptr().is_null());
        let value = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(class.as_ptr()));
        assert!(!value.as_ptr().is_null());
        for probe in [
            molt_linked_type_identity_probe_exception_symbols,
            molt_public_type_identity_probe_exception_symbols,
        ] {
            errors::PyErr_SetRaisedException(object::Py_NewRef(value.as_ptr()));
            assert_eq!(probe(expected.as_ptr(), expected.len(), value.as_ptr()), 0);
            let observed = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(observed.as_ptr(), value.as_ptr());
            assert_eq!(
                (*observed.as_ptr()).ob_type.cast::<PyObject>(),
                class.as_ptr()
            );
        }
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn both_c_headers_share_physical_type_factories_and_mro_module_observers() {
    use molt_cpython_abi::api::{modules, strings};
    use refcount::OwnedPyObject;

    #[repr(C)]
    struct FactoryPayload {
        header: PyObject,
        edge: *mut PyObject,
        value: std::os::raw::c_long,
    }

    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        assert_c_exception_symbols_preserve_pending();
        for (header, probe) in [
            ("linked", molt_linked_type_factory_probe as TypeFactoryProbe),
            ("public", molt_public_type_factory_probe as TypeFactoryProbe),
        ] {
            let mut slots = [PyModuleDef_Slot {
                slot: 0,
                value: ptr::null_mut(),
            }];
            let mut definitions: [PyModuleDef; 3] = std::mem::zeroed();
            let mut module_owners = Vec::new();
            for (index, name) in [c"factory_left", c"factory_right", c"factory_missing"]
                .into_iter()
                .enumerate()
            {
                let definition = &mut definitions[index];
                definition.m_base.ob_base.ob_refcnt = 1;
                definition.m_name = name.as_ptr();
                definition.m_size = std::mem::size_of::<u64>() as Py_ssize_t;
                definition.m_slots = slots.as_mut_ptr();
                assert!(!modules::PyModuleDef_Init(definition).is_null());
                if index == 2 {
                    continue;
                }
                // A real multi-phase module spec exercises the unregistered
                // module path, not a substitute PyState registry entry.
                let spec =
                    OwnedPyObject::from_owned(modules::PyModule_New(c"type_factory_spec".as_ptr()));
                let module_name =
                    OwnedPyObject::from_owned(strings::PyUnicode_FromString(name.as_ptr()));
                assert!(!spec.as_ptr().is_null() && !module_name.as_ptr().is_null());
                assert_eq!(
                    object::PyObject_SetAttrString(
                        spec.as_ptr(),
                        c"name".as_ptr(),
                        module_name.as_ptr()
                    ),
                    0
                );
                let module = OwnedPyObject::from_owned(modules::PyModule_FromDefAndSpec2(
                    definition,
                    spec.as_ptr(),
                    1013,
                ));
                assert!(!module.as_ptr().is_null());
                assert_eq!(modules::PyModule_ExecDef(module.as_ptr(), definition), 0);
                assert!(!modules::PyModule_GetState(module.as_ptr()).is_null());
                assert_eq!(
                    modules::PyModule_GetDef(module.as_ptr()),
                    definition as *mut PyModuleDef
                );
                assert!(modules::PyState_FindModule(definition).is_null());
                module_owners.push(module);
            }
            let module_a = module_owners[0].as_ptr();
            let module_b = module_owners[1].as_ptr();
            let module_refs = ((*module_a).ob_refcnt, (*module_b).ob_refcnt);
            let mut metaclass =
                NativeType::<PyTypeObject>::subtype(&raw mut PyType_Type, c"TypeFactoryMeta");
            assert_eq!(metaclass.ready(), 0);
            let meta_refs = metaclass.ob_base.ob_base.ob_refcnt;
            let mut types = [ptr::null_mut(); 13];
            let mut buffer_layout = [usize::MAX; 13];
            let result = probe(
                &raw mut *metaclass,
                module_a,
                module_b,
                &raw mut definitions[0],
                &raw mut definitions[1],
                &raw mut definitions[2],
                types.as_mut_ptr(),
                buffer_layout.as_mut_ptr(),
                buffer_constructor_probe,
            );
            // Capture a failing probe's exception while the definition storage
            // remains alive, then retire every returned native type normally.
            let pending = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            let pending_message = if pending.as_ptr().is_null() {
                String::from("no pending exception")
            } else {
                let rendered = OwnedPyObject::from_owned(typeobj::PyObject_Str(pending.as_ptr()));
                let text = strings::PyUnicode_AsUTF8(rendered.as_ptr());
                if text.is_null() {
                    String::from("exception rendering failed")
                } else {
                    std::ffi::CStr::from_ptr(text)
                        .to_string_lossy()
                        .into_owned()
                }
            };
            let owners: Vec<_> = types
                .into_iter()
                .map(|ty| OwnedPyObject::from_owned(ty))
                .collect();
            let physical_layouts = if result == 0 {
                let payload = owners[0].as_ptr().cast::<PyTypeObject>();
                let derived = owners[1].as_ptr().cast::<PyTypeObject>();
                let variable = owners[5].as_ptr().cast::<PyTypeObject>();
                Some((
                    (*payload).tp_basicsize,
                    (*derived).tp_basicsize,
                    (*variable).tp_basicsize,
                    (*variable).tp_itemsize,
                ))
            } else {
                None
            };
            for owner in owners.iter().rev() {
                if !owner.as_ptr().is_null() {
                    assert_eq!(typeobj::molt_type_clear(owner.as_ptr()), 0);
                }
            }
            drop(owners);
            assert_eq!(
                buffer_layout,
                [
                    std::mem::size_of::<Py_buffer>(),
                    std::mem::align_of::<Py_buffer>(),
                    std::mem::offset_of!(Py_buffer, buf),
                    std::mem::offset_of!(Py_buffer, obj),
                    std::mem::offset_of!(Py_buffer, len),
                    std::mem::offset_of!(Py_buffer, itemsize),
                    std::mem::offset_of!(Py_buffer, readonly),
                    std::mem::offset_of!(Py_buffer, ndim),
                    std::mem::offset_of!(Py_buffer, format),
                    std::mem::offset_of!(Py_buffer, shape),
                    std::mem::offset_of!(Py_buffer, strides),
                    std::mem::offset_of!(Py_buffer, suboffsets),
                    std::mem::offset_of!(Py_buffer, internal),
                ],
                "{header} Py_buffer must be the exact linked prefix"
            );
            assert_eq!(
                result, 0,
                "{header} C factory probe failed at contract code {result}: {pending_message}"
            );
            assert!(
                pending.as_ptr().is_null(),
                "{header} C probe left a pending exception"
            );
            assert_eq!(
                physical_layouts,
                Some((
                    std::mem::size_of::<FactoryPayload>() as Py_ssize_t,
                    std::mem::size_of::<FactoryPayload>() as Py_ssize_t,
                    std::mem::size_of::<PyVarObject>() as Py_ssize_t,
                    std::mem::size_of::<Py_ssize_t>() as Py_ssize_t
                ))
            );
            assert_eq!(
                ((*module_a).ob_refcnt, (*module_b).ob_refcnt),
                module_refs,
                "the type module edges must be released with their type owners"
            );
            assert_eq!(metaclass.ob_base.ob_base.ob_refcnt, meta_refs);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }
    });
}

#[test]
fn spec_construction_and_repeated_abi_bootstrap_preserve_live_builtin_shells() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let descriptors = [
            &raw mut PyMethodDescr_Type,
            &raw mut PyClassMethodDescr_Type,
            &raw mut PyMemberDescr_Type,
            &raw mut PyGetSetDescr_Type,
            &raw mut PyWrapperDescr_Type,
            &raw mut _PyMethodWrapper_Type,
        ];
        let mut shells = descriptors.to_vec();
        shells.extend([
            &raw mut PyBaseObject_Type,
            &raw mut PyType_Type,
            &raw mut PyTuple_Type,
        ]);
        for &kind in &shells {
            assert_eq!(typeobj::PyType_Ready(kind), 0);
        }
        for &kind in &descriptors {
            assert_ne!((*kind).tp_flags & Py_TPFLAGS_HAVE_GC, 0);
            assert!((*kind).tp_traverse.is_some());
        }
        let snapshot = |kind: *mut PyTypeObject| {
            (
                (*kind).tp_flags,
                (*kind).tp_basicsize,
                (*kind).tp_itemsize,
                (*kind).tp_dict,
                (*kind).tp_mro,
                (*kind).tp_bases,
                (*kind).tp_members,
                (*kind).tp_getset,
            )
        };
        let before: Vec<_> = shells.iter().map(|&kind| snapshot(kind)).collect();
        let mut slots = [PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        }];
        let mut spec = PyType_Spec {
            name: c"lifecycle.BootstrapOwner".as_ptr(),
            basicsize: 0,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        for _ in 0..2 {
            molt_cpython_abi::bridge::molt_cpython_abi_init();
            let class =
                refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpec(&raw mut spec));
            assert!(!class.as_ptr().is_null());
            for (&kind, &expected) in shells.iter().zip(&before) {
                assert_eq!(
                    snapshot(kind),
                    expected,
                    "a new type must not reset a live builtin shell"
                );
            }
            assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        }
        assert!(!crate::exception_pending(&py));
    });
}

unsafe extern "C" fn observe_object_free(object: *mut c_void) {
    OBJECT_FREES.fetch_add(1, Ordering::SeqCst);
    unsafe { memory::PyObject_Free(object) };
}

unsafe extern "C" fn observe_type_free(object: *mut c_void) {
    TYPE_FREES.fetch_add(1, Ordering::SeqCst);
    unsafe { memory::PyObject_GC_Del(object) };
}

#[test]
fn generic_object_and_spec_type_retire_through_their_real_allocation_owners() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        OBJECT_FREES.store(0, Ordering::SeqCst);
        TYPE_FREES.store(0, Ordering::SeqCst);
        let mut declaration = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"GenericAllocationOwner",
        );
        declaration.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
        declaration.tp_free = Some(observe_object_free);
        assert_eq!(declaration.ready(), 0);
        assert!(declaration.tp_dealloc.is_some());
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            &raw mut *declaration,
            0,
        ));
        assert!(!instance.as_ptr().is_null());
        drop(instance);
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 1);

        let mut metaclass =
            NativeType::<PyTypeObject>::subtype(&raw mut PyType_Type, c"HeapAllocationOwner");
        metaclass.tp_free = Some(observe_type_free);
        assert_eq!(metaclass.ready(), 0);
        let meta_refs = metaclass.ob_base.ob_base.ob_refcnt;
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_free,
                pfunc: observe_object_free as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"native_lifecycle.SpecOwner".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
            slots: slots.as_mut_ptr(),
        };
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromMetaclass(
            &raw mut *metaclass,
            ptr::null_mut(),
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!class.as_ptr().is_null());
        let class_ptr = class.as_ptr().cast::<PyTypeObject>();
        assert_eq!((*class.as_ptr()).ob_type, &raw mut *metaclass);
        assert!(crate::object::gc::native_gc_is_enrolled(
            class.as_ptr().addr()
        ));
        assert!(crate::object::gc::native_gc_is_tracked(
            class.as_ptr().addr()
        ));
        let class_refs = (*class.as_ptr()).ob_refcnt;
        let instance =
            refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(class_ptr, 0));
        assert!(!instance.as_ptr().is_null());
        assert_eq!((*class.as_ptr()).ob_refcnt, class_refs + 1);
        drop(instance);
        assert_eq!((*class.as_ptr()).ob_refcnt, class_refs);
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 2);
        // Explicit clear is the public cycle-breaking boundary, not a fixture
        // destructor: it must leave the owned type valid until its last DECREF.
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        let address = class.as_ptr().addr();
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"pending across heap type destruction".as_ptr(),
        );
        let pending = errors::PyErr_Occurred();
        drop(class);
        assert_eq!(TYPE_FREES.load(Ordering::SeqCst), 1);
        assert!(!crate::object::gc::native_gc_is_enrolled(address));
        assert_eq!(metaclass.ob_base.ob_base.ob_refcnt, meta_refs);
        assert_eq!(errors::PyErr_Occurred(), pending);
        errors::PyErr_Clear();

        // A rejected slot must free partially initialized names and native GC
        // storage while preserving the actual construction exception.
        slots[0].slot = i32::MAX;
        spec.slots = slots.as_mut_ptr();
        let failed = typeobj::PyType_FromMetaclass(
            &raw mut *metaclass,
            ptr::null_mut(),
            &raw mut spec,
            ptr::null_mut(),
        );
        assert!(failed.is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        assert_eq!(
            TYPE_FREES.load(Ordering::SeqCst),
            1,
            "invalid slots fail before allocator callbacks"
        );
        assert_eq!(metaclass.ob_base.ob_base.ob_refcnt, meta_refs);
        errors::PyErr_Clear();
        assert!(!crate::exception_pending(&py));
    });
}

static ALLOC_ITEMS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn tracked_type_allocate(
    type_: *mut PyTypeObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    ALLOC_ITEMS.store(count as usize, Ordering::SeqCst);
    unsafe {
        let object = typeobj::PyType_GenericAlloc(type_, count);
        if !object.is_null() && memory::PyObject_GC_IsTracked(object) == 0 {
            memory::PyObject_GC_Track(object.cast());
        }
        object
    }
}

unsafe extern "C" fn erroneous_type_allocate(
    type_: *mut PyTypeObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    unsafe {
        let result = typeobj::PyType_GenericAlloc(type_, count);
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"allocator result with pending error".as_ptr(),
        );
        result
    }
}

#[test]
fn spec_member_storage_bases_and_allocator_failures_share_one_transaction() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        TYPE_FREES.store(0, Ordering::SeqCst);
        let mut metaclass =
            NativeType::<PyTypeObject>::subtype(&raw mut PyType_Type, c"TrackedTypeAllocator");
        metaclass.tp_basicsize = (std::mem::size_of::<PyHeapTypeObject>() + 32) as Py_ssize_t;
        metaclass.tp_alloc = Some(tracked_type_allocate);
        metaclass.tp_free = Some(observe_type_free);
        assert_eq!(metaclass.ready(), 0);
        let mut base_slots = [PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        }];
        let mut base_spec = PyType_Spec {
            name: c"lifecycle.BasesOwner".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
            slots: base_slots.as_mut_ptr(),
        };
        let base = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromMetaclass(
            &raw mut *metaclass,
            ptr::null_mut(),
            &raw mut base_spec,
            ptr::null_mut(),
        ));
        assert!(!base.as_ptr().is_null());
        // The single-type argument and the Py_tp_bases declaration must select
        // the same actual metaclass and physical base as tuple arguments.
        let bases = refcount::OwnedPyObject::from_owned(
            molt_cpython_abi::api::sequences::PyTuple_FromArray(&base.as_ptr(), 1),
        );
        assert!(!bases.as_ptr().is_null());
        let mut members = vec![
            PyMemberDef {
                name: c"value".as_ptr(),
                type_: 19,
                offset: 0,
                flags: Py_RELATIVE_OFFSET,
                doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_bases,
                pfunc: bases.as_ptr().cast(),
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_members,
                pfunc: members.as_mut_ptr().cast(),
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"lifecycle.RelativeMember".as_ptr(),
            basicsize: -(std::mem::size_of::<Py_ssize_t>() as i32),
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        for input in [ptr::null_mut(), base.as_ptr()] {
            let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
                &raw mut spec,
                input,
            ));
            assert!(!class.as_ptr().is_null());
            let type_ = class.as_ptr().cast::<PyTypeObject>();
            assert_eq!((*class.as_ptr()).ob_type, &raw mut *metaclass);
            assert_eq!((*type_).tp_base, base.as_ptr().cast());
            assert_eq!(ALLOC_ITEMS.load(Ordering::SeqCst), 1);
            assert_eq!(
                (*type_).tp_members.cast::<u8>(),
                class.as_ptr().cast::<u8>().offset(metaclass.tp_basicsize)
            );
            let member = &*(*type_).tp_members;
            assert_eq!(member.flags & Py_RELATIVE_OFFSET, 0);
            assert!(member.offset >= std::mem::size_of::<PyObject>() as isize);
            assert!((*(*type_).tp_members.add(1)).name.is_null());
            let instance =
                refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(type_, 0));
            assert!(!instance.as_ptr().is_null());
            let value = refcount::OwnedPyObject::from_owned(numbers::PyLong_FromLong(37));
            assert_eq!(
                object::PyObject_SetAttrString(
                    instance.as_ptr(),
                    c"value".as_ptr(),
                    value.as_ptr()
                ),
                0
            );
            let actual = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                instance.as_ptr(),
                c"value".as_ptr(),
            ));
            assert_eq!(numbers::PyLong_AsLong(actual.as_ptr()), 37);
            drop(instance);
            assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        }
        // Invalidate the caller table before descriptor dispatch on its copy.
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!class.as_ptr().is_null());
        members[0].offset = 0;
        members[0].name = ptr::null();
        drop(members);
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            class.as_ptr().cast(),
            0,
        ));
        let value = refcount::OwnedPyObject::from_owned(numbers::PyLong_FromLong(91));
        assert_eq!(
            object::PyObject_SetAttrString(instance.as_ptr(), c"value".as_ptr(), value.as_ptr()),
            0
        );
        let actual = refcount::OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            instance.as_ptr(),
            c"value".as_ptr(),
        ));
        assert_eq!(numbers::PyLong_AsLong(actual.as_ptr()), 91);
        drop(instance);
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        drop(class);

        metaclass.tp_alloc = Some(erroneous_type_allocate);
        let frees = TYPE_FREES.load(Ordering::SeqCst);
        let failed = typeobj::PyType_FromMetaclass(
            &raw mut *metaclass,
            ptr::null_mut(),
            &raw mut base_spec,
            ptr::null_mut(),
        );
        assert!(failed.is_null());
        assert_ne!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast()),
            0
        );
        assert_eq!(TYPE_FREES.load(Ordering::SeqCst), frees + 1);
        errors::PyErr_Clear();
        assert_eq!(typeobj::molt_type_clear(base.as_ptr()), 0);
        assert!(!crate::exception_pending(&py));
    });
}

static FINALIZER_CALLS: AtomicUsize = AtomicUsize::new(0);
static REPLACEMENT_TYPE: AtomicPtr<PyTypeObject> = AtomicPtr::new(ptr::null_mut());
static RESURRECTED: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn change_type_and_resurrect(object: *mut PyObject) {
    unsafe {
        let new_type = REPLACEMENT_TYPE.load(Ordering::SeqCst);
        if (*object).ob_type != new_type {
            refcount::Py_INCREF(new_type.cast());
            let previous = std::mem::replace(&mut (*object).ob_type, new_type);
            refcount::Py_DECREF(previous.cast());
        }
        if FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            refcount::Py_INCREF(object);
            RESURRECTED.store(object, Ordering::SeqCst);
        }
    }
}

unsafe extern "C" fn count_finalization(_object: *mut PyObject) {
    FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn extension_base_dealloc(object: *mut PyObject) {
    unsafe {
        (*(*object).ob_type).tp_free.unwrap()(object.cast());
    }
}

#[test]
fn heap_subtype_owns_members_finalization_and_foreign_base_type_release() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        OBJECT_FREES.store(0, Ordering::SeqCst);
        FINALIZER_CALLS.store(0, Ordering::SeqCst);
        let mut foreign_base = NativeType::<PyTypeObject>::subtype(
            &raw mut PyBaseObject_Type,
            c"ForeignBaseDestructor",
        );
        foreign_base.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
        foreign_base.tp_dealloc = Some(extension_base_dealloc);
        foreign_base.tp_free = Some(observe_object_free);
        assert_eq!(foreign_base.ready(), 0);
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_finalize,
                pfunc: count_finalization as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"lifecycle.ForeignDerived".as_ptr(),
            basicsize: 0,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            (&raw mut *foreign_base).cast(),
        ));
        assert!(!class.as_ptr().is_null());
        let count = (*class.as_ptr()).ob_refcnt;
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            class.as_ptr().cast(),
            0,
        ));
        assert!(!instance.as_ptr().is_null());
        drop(instance);
        assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 1);
        assert_eq!((*class.as_ptr()).ob_refcnt, count);
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        drop(class);

        let mut members = [
            PyMemberDef {
                name: c"child".as_ptr(),
                type_: 16,
                offset: std::mem::size_of::<PyObject>() as isize,
                flags: 0,
                doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        slots[0] = PyType_Slot {
            slot: molt_cpython_abi::type_slots::Py_tp_members,
            pfunc: members.as_mut_ptr().cast(),
        };
        spec.slots = slots.as_mut_ptr();
        spec.name = c"lifecycle.OwnedMember".as_ptr();
        spec.basicsize =
            (std::mem::size_of::<PyObject>() + std::mem::size_of::<*mut PyObject>()) as i32;
        spec.flags |= PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_HAVE_GC);
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!class.as_ptr().is_null());
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            class.as_ptr().cast(),
            0,
        ));
        let child = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            &raw mut *foreign_base,
            0,
        ));
        assert!(!instance.as_ptr().is_null() && !child.as_ptr().is_null());
        assert_ne!(memory::PyObject_GC_IsTracked(instance.as_ptr()), 0);
        assert_eq!(
            object::PyObject_SetAttrString(instance.as_ptr(), c"child".as_ptr(), child.as_ptr()),
            0
        );
        drop(child);
        drop(instance);
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 2);
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            class.as_ptr().cast(),
            0,
        ));
        assert!(!instance.as_ptr().is_null());
        let address = instance.as_ptr().addr();
        assert_eq!(
            object::PyObject_SetAttrString(instance.as_ptr(), c"child".as_ptr(), instance.as_ptr()),
            0
        );
        drop(instance);
        assert_eq!(
            crate::object::gc::collect_cycles(&py).status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert!(!crate::object::gc::native_gc_is_enrolled(address));
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        assert!(!crate::exception_pending(&py));
    });
}

static OLD_OWNER_FREES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn old_owner_free(object: *mut c_void) {
    OLD_OWNER_FREES.fetch_add(1, Ordering::SeqCst);
    unsafe { memory::PyObject_Free(object) };
}

unsafe extern "C" fn terminal_type_change(object: *mut PyObject) {
    unsafe {
        let target = REPLACEMENT_TYPE.load(Ordering::SeqCst);
        refcount::Py_INCREF(target.cast());
        let old = std::mem::replace(&mut (*object).ob_type, target);
        refcount::Py_DECREF(old.cast());
        FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn terminal_type_change_retires_the_old_class_and_uses_the_new_free() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        OBJECT_FREES.store(0, Ordering::SeqCst);
        OLD_OWNER_FREES.store(0, Ordering::SeqCst);
        FINALIZER_CALLS.store(0, Ordering::SeqCst);
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_finalize,
                pfunc: terminal_type_change as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_free,
                pfunc: old_owner_free as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"lifecycle.TerminalOld".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        let old = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            ptr::null_mut(),
        ));
        let old_address = old.as_ptr().addr();
        slots[0].slot = molt_cpython_abi::type_slots::Py_tp_free;
        slots[0].pfunc = observe_object_free as *const () as *mut c_void;
        slots[1].slot = 0;
        spec.slots = slots.as_mut_ptr();
        spec.name = c"lifecycle.TerminalNew".as_ptr();
        let new = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!old.as_ptr().is_null() && !new.as_ptr().is_null());
        REPLACEMENT_TYPE.store(new.as_ptr().cast(), Ordering::SeqCst);
        let count = (*new.as_ptr()).ob_refcnt;
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            old.as_ptr().cast(),
            0,
        ));
        assert!(!instance.as_ptr().is_null());
        assert_eq!(typeobj::molt_type_clear(old.as_ptr()), 0);
        drop(old);
        drop(instance);
        assert!(!crate::object::gc::native_gc_is_enrolled(old_address));
        assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(OLD_OWNER_FREES.load(Ordering::SeqCst), 0);
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 1);
        assert_eq!((*new.as_ptr()).ob_refcnt, count);
        REPLACEMENT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        assert_eq!(typeobj::molt_type_clear(new.as_ptr()), 0);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn spec_offsets_are_checked_and_negative_dict_owners_are_collected() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let mut members = [
            PyMemberDef {
                name: c"__dictoffset__".as_ptr(),
                type_: 19,
                offset: (2 * std::mem::size_of::<PyObject>()) as isize,
                flags: 1,
                doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_members,
                pfunc: members.as_mut_ptr().cast(),
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"lifecycle.InvalidOffset".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        assert!(typeobj::PyType_FromSpecWithBases(&raw mut spec, ptr::null_mut()).is_null());
        assert_ne!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
            0
        );
        errors::PyErr_Clear();
        members[0].offset = -(std::mem::size_of::<*mut PyObject>() as isize);
        slots[0].pfunc = members.as_mut_ptr().cast();
        spec.slots = slots.as_mut_ptr();
        spec.name = c"lifecycle.NegativeTupleDict".as_ptr();
        spec.basicsize =
            (PyTuple_Type.tp_basicsize as usize + std::mem::size_of::<*mut PyObject>()) as i32;
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            (&raw mut PyTuple_Type).cast(),
        ));
        assert!(!class.as_ptr().is_null());
        assert!(
            mapping::PyDict_GetItemString(
                (*class.as_ptr().cast::<PyTypeObject>()).tp_dict,
                c"__dictoffset__".as_ptr()
            )
            .is_null()
        );
        for size in [0, 3] {
            let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                class.as_ptr().cast(),
                size,
            ));
            assert!(!instance.as_ptr().is_null());
            let address = instance.as_ptr().addr();
            assert_eq!(
                object::PyObject_SetAttrString(
                    instance.as_ptr(),
                    c"self".as_ptr(),
                    instance.as_ptr()
                ),
                0
            );
            assert!(!object::_PyObject_GetDictPtr(instance.as_ptr()).is_null());
            drop(instance);
            assert_eq!(
                crate::object::gc::collect_cycles(&py).status,
                crate::object::gc::GcCollectStatus::Completed
            );
            assert!(!crate::object::gc::native_gc_is_enrolled(address));
        }
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);

        spec.basicsize = 0;
        slots[0].slot = 0;
        spec.slots = slots.as_mut_ptr();
        for base in [
            &raw mut PyList_Type,
            &raw mut PyDict_Type,
            &raw mut PySet_Type,
            &raw mut PyModule_Type,
        ] {
            let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
                &raw mut spec,
                base.cast(),
            ));
            assert!(!class.as_ptr().is_null());
            assert!(typeobj::PyType_GenericAlloc(class.as_ptr().cast(), 0).is_null());
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast()),
                0
            );
            errors::PyErr_Clear();
            assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn non_gc_resurrection_and_type_change_reread_the_terminal_storage_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        OBJECT_FREES.store(0, Ordering::SeqCst);
        FINALIZER_CALLS.store(0, Ordering::SeqCst);
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_finalize,
                pfunc: change_type_and_resurrect as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_free,
                pfunc: observe_object_free as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"lifecycle.BeforeFinalization".as_ptr(),
            basicsize: std::mem::size_of::<PyObject>() as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        let old_class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            ptr::null_mut(),
        ));
        spec.name = c"lifecycle.AfterFinalization".as_ptr();
        let new_class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!old_class.as_ptr().is_null() && !new_class.as_ptr().is_null());
        REPLACEMENT_TYPE.store(new_class.as_ptr().cast(), Ordering::SeqCst);
        let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            old_class.as_ptr().cast(),
            0,
        ));
        assert!(!instance.as_ptr().is_null());
        assert_eq!(typeobj::molt_type_clear(old_class.as_ptr()), 0);
        drop(old_class);
        let new_refs = (*new_class.as_ptr()).ob_refcnt;
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"preserve through type change".as_ptr(),
        );
        let pending = errors::PyErr_Occurred();
        drop(instance);
        assert_eq!(errors::PyErr_Occurred(), pending);
        let resurrected = refcount::OwnedPyObject::from_owned(
            RESURRECTED.swap(ptr::null_mut(), Ordering::SeqCst),
        );
        assert!(!resurrected.as_ptr().is_null());
        assert_eq!((*resurrected.as_ptr()).ob_type, new_class.as_ptr().cast());
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 0);
        drop(resurrected);
        assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 1);
        assert_eq!((*new_class.as_ptr()).ob_refcnt, new_refs);
        assert_eq!(errors::PyErr_Occurred(), pending);
        errors::PyErr_Clear();
        assert_eq!(typeobj::molt_type_clear(new_class.as_ptr()), 0);
        REPLACEMENT_TYPE.store(ptr::null_mut(), Ordering::SeqCst);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn native_numeric_tuple_and_bytearray_subclasses_use_their_own_allocator() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        OBJECT_FREES.store(0, Ordering::SeqCst);
        FINALIZER_CALLS.store(0, Ordering::SeqCst);
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_finalize,
                pfunc: count_finalization as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_free,
                pfunc: observe_object_free as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"lifecycle.NativeBuiltinDerived".as_ptr(),
            basicsize: 0,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        for base in [
            &raw mut PyLong_Type,
            &raw mut PyFloat_Type,
            &raw mut PyComplex_Type,
            &raw mut PyTuple_Type,
            &raw mut PyByteArray_Type,
        ] {
            let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
                &raw mut spec,
                base.cast(),
            ));
            assert!(!class.as_ptr().is_null());
            let count = (*class.as_ptr()).ob_refcnt;
            let instance = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                class.as_ptr().cast(),
                0,
            ));
            assert!(!instance.as_ptr().is_null());
            drop(instance);
            assert_eq!((*class.as_ptr()).ob_refcnt, count);
            assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        }
        assert_eq!(OBJECT_FREES.load(Ordering::SeqCst), 5);
        assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 5);
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn heap_metaclass_cycle_exposes_its_actual_class_owner_to_the_collector() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        let mut slots = [PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        }];
        let mut spec = PyType_Spec {
            name: c"lifecycle.HeapMetaclass".as_ptr(),
            basicsize: 0,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
            slots: slots.as_mut_ptr(),
        };
        let metaclass = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            (&raw mut PyType_Type).cast(),
        ));
        assert!(!metaclass.as_ptr().is_null());
        spec.name = c"lifecycle.MetaclassChild".as_ptr();
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromMetaclass(
            metaclass.as_ptr().cast(),
            ptr::null_mut(),
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!class.as_ptr().is_null());
        assert_eq!(
            mapping::PyDict_SetItemString(
                (*metaclass.as_ptr().cast::<PyTypeObject>()).tp_dict,
                c"child".as_ptr(),
                class.as_ptr()
            ),
            0
        );
        let meta_addr = metaclass.as_ptr().addr();
        let class_addr = class.as_ptr().addr();
        drop(class);
        drop(metaclass);
        assert_eq!(
            crate::object::gc::collect_cycles(&py).status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert!(!crate::object::gc::native_gc_is_enrolled(meta_addr));
        assert!(!crate::object::gc::native_gc_is_enrolled(class_addr));
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn native_hash_readies_inherited_slots_and_limits_type_names_by_bytes() {
    unsafe extern "C" fn hash(_object: *mut PyObject) -> isize {
        867
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    with_gil(|py| unsafe {
        let name = std::ffi::CString::new(format!("{}é", "a".repeat(199))).unwrap();
        let mut base = NativeType::subtype(&raw mut PyBaseObject_Type, c"HashReadyBase");
        base.tp_hash = Some(hash);
        assert_eq!(base.ready(), 0);
        let mut child = NativeType::subtype(&raw mut *base, c"HashUnreadyChild");
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut *child,
        };
        assert_eq!(child.tp_flags & Py_TPFLAGS_READY, 0);
        assert!(child.tp_hash.is_none());
        assert_eq!(typeobj::PyObject_Hash(&raw mut object), 867);
        assert_ne!(child.tp_flags & Py_TPFLAGS_READY, 0);
        assert!(errors::PyErr_Occurred().is_null());

        // Objects/unicodeobject.c truncates %.200s before replacement decoding,
        // including when the byte limit falls inside a multibyte UTF-8 sequence.
        child.tp_name = name.as_ptr();
        assert_eq!(typeobj::PyObject_HashNotImplemented(&raw mut object), -1);
        let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        let text = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr()));
        let bytes = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(text.as_ptr());
        assert!(!bytes.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(bytes).to_string_lossy(),
            format!("unhashable type: '{}�'", "a".repeat(199))
        );
        // Restore the static declaration while the dynamic name still lives.
        child.tp_name = c"HashUnreadyChild".as_ptr();
        assert!(!crate::exception_pending(&py));

        // Runtime ingress shares the same classifier as the two C headers;
        // provenance cannot publish a foreign wrapper around non-PyObject data.
        let mut private_storage = [0xa5_u64; 8];
        let private_object = private_storage.as_mut_ptr().cast::<PyObject>();
        assert_eq!(crate::c_api::molt_c_heap_register(private_object.addr()), 0);
        let projected =
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(private_object);
        // Revoke the stack pointer before assertions can unwind the frame.
        assert_eq!(
            crate::c_api::molt_c_heap_unregister(private_object.addr()),
            0
        );
        assert!(projected.is_none());
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
            1
        );
        errors::PyErr_Clear();
        assert_eq!(private_storage, [0xa5_u64; 8]);
    });
}

// This allocator does not pass through GenericAlloc or a Molt byte receipt.
// It honors the selected metaclass request, then changes that mutable class
// field to prove that the factory keeps its captured member offset/extent.
unsafe extern "C" fn raw_type_allocate_then_mutate_layout(
    type_: *mut PyTypeObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    unsafe {
        let bytes =
            (*type_).tp_basicsize as usize + (*type_).tp_itemsize as usize * (count as usize + 1);
        let raw = libc::calloc(1, bytes).cast::<PyVarObject>();
        if raw.is_null() {
            return errors::PyErr_NoMemory();
        }
        let object = memory::PyObject_InitVar(raw, type_, count).cast();
        (*type_).tp_basicsize += 128;
        object
    }
}

static RAW_TYPE_RELEASE_STATE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn type_allocate_with_insufficient_extent(
    type_: *mut PyTypeObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    unsafe {
        let requested = (*type_).tp_basicsize;
        (*type_).tp_basicsize = std::mem::size_of::<PyHeapTypeObject>() as Py_ssize_t;
        let object = typeobj::PyType_GenericAlloc(type_, count);
        (*type_).tp_basicsize = requested;
        object
    }
}

unsafe extern "C" fn type_allocate_with_insufficient_extent_and_error(
    type_: *mut PyTypeObject,
    count: Py_ssize_t,
) -> *mut PyObject {
    let object = unsafe { type_allocate_with_insufficient_extent(type_, count) };
    if !object.is_null() {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_LookupError).cast(),
                c"allocator origin".as_ptr(),
            )
        };
    }
    object
}

unsafe extern "C" fn raw_type_free_observes_revoked_custody(storage: *mut c_void) {
    let address = storage.addr();
    let physical = unsafe { molt_cpython_abi::bridge::molt_foreign_object_is_gc_capable(address) };
    let collector = crate::object::gc::native_gc_is_enrolled(address);
    RAW_TYPE_RELEASE_STATE.store(
        1 + usize::from(physical) * 2 + usize::from(collector) * 4,
        Ordering::SeqCst,
    );
    unsafe { libc::free(storage) };
}

#[test]
fn rejected_custom_type_allocation_revokes_custody_before_raw_free() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|_py| unsafe {
        type Allocate = unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject;
        for (allocate, expected) in [
            (
                type_allocate_with_insufficient_extent as Allocate,
                &raw mut PyExc_TypeError,
            ),
            (
                type_allocate_with_insufficient_extent_and_error as Allocate,
                &raw mut PyExc_SystemError,
            ),
        ] {
            RAW_TYPE_RELEASE_STATE.store(0, Ordering::SeqCst);
            let mut metaclass =
                NativeType::<PyTypeObject>::subtype(&raw mut PyType_Type, c"RejectedTypeAllocator");
            metaclass.tp_basicsize = (std::mem::size_of::<PyHeapTypeObject>() + 128) as Py_ssize_t;
            metaclass.tp_alloc = Some(allocate);
            metaclass.tp_free = Some(raw_type_free_observes_revoked_custody);
            assert_eq!(metaclass.ready(), 0);
            let meta_refs = metaclass.ob_base.ob_base.ob_refcnt;
            let mut spec = PyType_Spec {
                name: c"storage.RejectedType".as_ptr(),
                basicsize: std::mem::size_of::<PyObject>() as i32,
                itemsize: 0,
                flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
                slots: ptr::null_mut(),
            };
            let class = typeobj::PyType_FromMetaclass(
                &raw mut *metaclass,
                ptr::null_mut(),
                &raw mut spec,
                ptr::null_mut(),
            );
            assert!(class.is_null());
            assert_eq!(
                RAW_TYPE_RELEASE_STATE.load(Ordering::SeqCst),
                1,
                "raw free must see both allocation permissions revoked"
            );
            assert_eq!(errors::PyErr_ExceptionMatches(expected.cast()), 1);
            if expected == (&raw mut PyExc_SystemError) {
                let raised =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                let cause = refcount::OwnedPyObject::from_owned(errors::PyException_GetCause(
                    raised.as_ptr(),
                ));
                assert!(!cause.as_ptr().is_null());
                assert_eq!(
                    errors::PyErr_GivenExceptionMatches(
                        cause.as_ptr(),
                        (&raw mut PyExc_LookupError).cast()
                    ),
                    1
                );
            }
            errors::PyErr_Clear();
            assert_eq!(metaclass.ob_base.ob_base.ob_refcnt, meta_refs);
        }
    });
}

#[test]
fn raw_custom_type_allocator_preserves_captured_member_extent_and_heap_ownership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        TYPE_FREES.store(0, Ordering::SeqCst);
        let mut metaclass =
            NativeType::<PyTypeObject>::subtype(&raw mut PyType_Type, c"RawTypeAllocator");
        let member_offset = std::mem::size_of::<PyHeapTypeObject>() + 32;
        metaclass.tp_basicsize = member_offset as Py_ssize_t;
        metaclass.tp_alloc = Some(raw_type_allocate_then_mutate_layout);
        metaclass.tp_free = Some(observe_type_free);
        assert_eq!(metaclass.ready(), 0);
        let meta_refs = metaclass.ob_base.ob_base.ob_refcnt;
        let mut members = [
            PyMemberDef {
                name: c"value".as_ptr(),
                type_: 19,
                offset: std::mem::size_of::<PyObject>() as Py_ssize_t,
                flags: 0,
                doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_members,
                pfunc: members.as_mut_ptr().cast(),
            },
            PyType_Slot {
                slot: 0,
                pfunc: ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            // Nonidentifier spelling avoids the immortal identifier cache so
            // this fixture can observe real name-owner decrements at release.
            name: c"storage.Raw-AllocatedType".as_ptr(),
            basicsize: (std::mem::size_of::<PyObject>() + std::mem::size_of::<Py_ssize_t>()) as i32,
            itemsize: 0,
            flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
            slots: slots.as_mut_ptr(),
        };
        let class = refcount::OwnedPyObject::from_owned(typeobj::PyType_FromMetaclass(
            &raw mut *metaclass,
            ptr::null_mut(),
            &raw mut spec,
            ptr::null_mut(),
        ));
        assert!(!class.as_ptr().is_null());
        let type_ = class.as_ptr().cast::<PyTypeObject>();
        let heap = class.as_ptr().cast::<PyHeapTypeObject>();
        assert_eq!((*class.as_ptr()).ob_type, &raw mut *metaclass);
        assert_eq!(metaclass.tp_basicsize, member_offset as Py_ssize_t + 128);
        assert_eq!(
            (*type_).tp_members.cast::<u8>(),
            class.as_ptr().cast::<u8>().add(member_offset)
        );
        assert!((*(*type_).tp_members.add(1)).name.is_null());
        assert_ne!(memory::PyObject_GC_IsTracked(class.as_ptr()), 0);
        let name_owner = refcount::OwnedPyObject::from_borrowed((*heap).ht_name);
        let name_refs = (*name_owner.as_ptr()).ob_refcnt;
        // Physical name and GC ownership survive a mutable semantic flag.
        (*type_).tp_flags &= !Py_TPFLAGS_HEAPTYPE;
        assert_eq!(typeobj::molt_type_is_gc(class.as_ptr()), 0);
        assert!(molt_cpython_abi::bridge::molt_foreign_object_is_gc_capable(
            class.as_ptr().addr()
        ));
        let name = refcount::OwnedPyObject::from_owned(typeobj::PyType_GetName(type_));
        assert_eq!(name.as_ptr(), name_owner.as_ptr());
        drop(name);
        assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
        drop(class);
        assert_eq!(TYPE_FREES.load(Ordering::SeqCst), 1);
        assert_eq!((*name_owner.as_ptr()).ob_refcnt, name_refs - 2);
        assert_eq!(metaclass.ob_base.ob_base.ob_refcnt, meta_refs);
        assert!(!crate::exception_pending(&py));
    });
}
