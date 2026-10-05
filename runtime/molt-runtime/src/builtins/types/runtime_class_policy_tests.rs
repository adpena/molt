use super::*;
use crate::object::class_storage::ClassSlotPolicy;
use crate::test_support::RuntimeTestTransaction;

fn assert_namespace_mutability(py: &PyToken<'_>, class: u64, immutable: bool) {
    assert_ne!(class, 0);
    assert!(!exception_pending(py));
    let ptr = obj_from_bits(class).as_ptr().expect("class pointer");
    let name_ptr = alloc_string(py, b"cached_class_policy_witness");
    assert!(!name_ptr.is_null());
    let _name = crate::PtrDropGuard::new(name_ptr);
    let name = MoltObject::from_ptr(name_ptr).bits();
    let value = MoltObject::from_int(73).bits();
    let result = crate::molt_set_attr_name(class, name, value);
    dec_ref_bits(py, result);
    let namespace = obj_from_bits(unsafe { class_dict_bits(ptr) })
        .as_ptr()
        .expect("class namespace");
    if immutable {
        assert!(
            exception_pending(py),
            "immutable class accepted a new attribute"
        );
        let exception = crate::builtins::exceptions::molt_exception_last_pending();
        let kind = crate::builtins::exceptions::molt_exception_kind(exception);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(kind)).as_deref(),
            Some("TypeError")
        );
        dec_ref_bits(py, kind);
        dec_ref_bits(py, exception);
        clear_exception(py);
        assert_eq!(unsafe { dict_get_in_place(py, namespace, name) }, None);
    } else {
        assert!(
            !exception_pending(py),
            "mutable class rejected an attribute"
        );
        assert_eq!(
            unsafe { dict_get_in_place(py, namespace, name) },
            Some(value)
        );
        let result = crate::molt_del_attr_name(class, name);
        dec_ref_bits(py, result);
        assert!(!exception_pending(py));
        assert_eq!(unsafe { dict_get_in_place(py, namespace, name) }, None);
    }
}

#[test]
fn cached_class_mutability_is_independent_of_identical_native_slots() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for (policy, immutable) in [
            (ClassSemanticPolicy::heap(false, true), false),
            (ClassSemanticPolicy::heap(true, true), true),
        ] {
            let slot = AtomicU64::new(0);
            let class = init_cached_runtime_class_configured(
                py,
                &slot,
                "ExplicitClassPolicy",
                policy,
                8,
                None,
                Some(ClassSlotPolicy::default()),
                |class, _namespace| {
                    assert_eq!(slot.load(AtomicOrdering::Acquire), 0);
                    assert!(!unsafe {
                        crate::object::class_is_immutable(
                            py,
                            obj_from_bits(class).as_ptr().unwrap(),
                        )
                    });
                    true
                },
            );
            assert_eq!(slot.load(AtomicOrdering::Acquire), class);
            assert_namespace_mutability(py, class, immutable);
            clear_atomic_slots(py, &[&slot]);
        }
    });
}

#[test]
fn module_exception_identity_survives_mutation_and_does_not_follow_names() {
    use crate::builtins::exceptions::{
        exception_type_bits_from_name, is_builtin_exception_class_bits,
    };
    use crate::object::class_storage::{
        ClassOrigin, ClassSemanticPolicy, is_canonical_runtime_class,
    };
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for (name, module) in [
            ("CancelledError", "asyncio.exceptions"),
            ("UnsupportedOperation", "_io"),
        ] {
            let class = exception_type_bits_from_name(py, name);
            assert_namespace_mutability(py, class, false);
            let ptr = obj_from_bits(class).as_ptr().unwrap();
            assert_eq!(
                unsafe { ClassSemanticPolicy::of(py, ptr) }.origin(),
                ClassOrigin::Heap
            );
            assert!(is_canonical_runtime_class(py, class));
            assert!(is_builtin_exception_class_bits(py, class));
            let module_key = attr_name_bits_from_bytes(py, b"__module__").unwrap();
            let observed = crate::molt_get_attr_name(class, module_key);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(observed)).as_deref(),
                Some(module)
            );
            dec_ref_bits(py, observed);
            dec_ref_bits(py, module_key);
            let original = unsafe { crate::class_name_bits(ptr) };
            inc_ref_bits(py, original);
            let renamed = attr_name_bits_from_bytes(py, b"RenamedModuleException").unwrap();
            assert!(unsafe { crate::class_set_name_bits(py, ptr, renamed) });
            assert_eq!(exception_type_bits_from_name(py, name), class);
            assert!(is_builtin_exception_class_bits(py, class));
            assert!(unsafe { crate::class_set_name_bits(py, ptr, original) });
            dec_ref_bits(py, original);
            dec_ref_bits(py, renamed);
            let fake_name = attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
            let fake = crate::molt_class_new(fake_name);
            dec_ref_bits(py, fake_name);
            let result = crate::molt_class_set_base(fake, builtin_classes(py).base_exception);
            dec_ref_bits(py, result);
            assert!(!exception_pending(py));
            assert!(!is_builtin_exception_class_bits(py, fake));
            assert!(!is_canonical_runtime_class(py, fake));
            dec_ref_bits(py, fake);
        }
    });
}

#[test]
fn cached_types_enforce_native_and_python_namespace_policies() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let native = [
            mappingproxy_class(py),
            frame_locals_proxy_class(py),
            method_class(py),
            simplenamespace_class(py),
            capsule_class(py),
            cell_class(py),
        ];
        for class in native {
            assert_namespace_mutability(py, class, true);
        }
        let module_spec = molt_importlib_module_spec_type();
        let _module_spec = crate::PtrDropGuard::new(
            obj_from_bits(module_spec)
                .as_ptr()
                .expect("ModuleSpec type"),
        );
        let mutable = [
            dynamic_class_attribute_class(py),
            compiled_loader::compiled_loader_base_class(py),
            compiled_loader::compiled_loader_builtin_class(py),
            compiled_loader::compiled_loader_frozen_class(py),
            module_spec,
        ];
        for class in mutable {
            assert_namespace_mutability(py, class, false);
        }
    });
}

#[test]
fn class_semantics_distinguish_static_heap_and_mutable_bank_types() {
    use crate::object::class_storage::{ClassOrigin, ClassSemanticPolicy};
    use molt_cpython_abi::abi_types::{
        Py_TPFLAGS_BASETYPE, Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_IMMUTABLETYPE, PyHeapTypeObject,
        PyTypeObject,
    };
    let _transaction = RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        let bank = builtin_classes(py);
        let args = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
        let partial = crate::builtins::functools::molt_functools_partial(
            bank.int,
            args,
            MoltObject::none().bits(),
        );
        dec_ref_bits(py, args);
        assert!(!exception_pending(py));
        let operator_classes = [
            crate::builtins::operator::molt_operator_itemgetter_type(),
            crate::builtins::operator::molt_operator_attrgetter_type(),
            crate::builtins::operator::molt_operator_methodcaller_type(),
        ];
        for (class, expected) in [
            (
                mappingproxy_class(py),
                ClassSemanticPolicy::static_type(false),
            ),
            (method_class(py), ClassSemanticPolicy::static_type(false)),
            (cell_class(py), ClassSemanticPolicy::static_type(false)),
            (capsule_class(py), ClassSemanticPolicy::static_type(false)),
            (
                frame_locals_proxy_class(py),
                ClassSemanticPolicy::static_type(false),
            ),
            (
                simplenamespace_class(py),
                ClassSemanticPolicy::static_type(true),
            ),
            (
                bank.base_exception_group,
                ClassSemanticPolicy::static_type(true),
            ),
            (bank.exception_group, ClassSemanticPolicy::heap(false, true)),
            (bank.io_base, ClassSemanticPolicy::heap(true, true)),
            (bank.file_io, ClassSemanticPolicy::heap(true, true)),
            (bank.bytes_io, ClassSemanticPolicy::heap(true, true)),
            (bank.bool, ClassSemanticPolicy::static_type(false)),
            (bank.builtin_method, ClassSemanticPolicy::static_type(false)),
            (
                type_of_bits(py, partial),
                ClassSemanticPolicy::heap(true, true),
            ),
            (operator_classes[0], ClassSemanticPolicy::heap(true, false)),
            (operator_classes[1], ClassSemanticPolicy::heap(true, false)),
            (operator_classes[2], ClassSemanticPolicy::heap(true, false)),
        ] {
            let ptr = obj_from_bits(class).as_ptr().unwrap();
            assert_eq!(unsafe { ClassSemanticPolicy::of(py, ptr) }, expected);
            assert_namespace_mutability(py, class, expected.immutable());
            unsafe {
                let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .handle_to_borrowed_pyobj(class)
                    .cast::<PyTypeObject>();
                assert!(!view.is_null());
                assert_eq!(molt_cpython_abi::api::typeobj::PyType_Ready(view), 0);
                assert_eq!(
                    (*view).tp_flags
                        & (Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_IMMUTABLETYPE | Py_TPFLAGS_BASETYPE),
                    expected.cpython_flags()
                );
                if expected.origin() == ClassOrigin::Heap {
                    let heap = view.cast::<PyHeapTypeObject>();
                    assert!(!(*heap).ht_name.is_null() && !(*heap).ht_qualname.is_null());
                }
            }
        }
        for owned in operator_classes {
            dec_ref_bits(py, owned);
        }
        dec_ref_bits(py, partial);
        // Static origin and immutability belong to the actual class, never its
        // inherited native shape. A SimpleNamespace subclass is a mutable heap.
        let name = attr_name_bits_from_bytes(py, b"NamespaceChild").unwrap();
        let child = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        let result = crate::molt_class_set_base(child, simplenamespace_class(py));
        dec_ref_bits(py, result);
        assert!(!exception_pending(py));
        let ptr = obj_from_bits(child).as_ptr().unwrap();
        assert_eq!(
            unsafe { ClassSemanticPolicy::of(py, ptr) }.origin(),
            ClassOrigin::Heap
        );
        assert_namespace_mutability(py, child, false);
        dec_ref_bits(py, child);
        // Admission consumes the same explicit BASETYPE fact as C flags.
        let name = attr_name_bits_from_bytes(py, b"RejectedMethodChild").unwrap();
        let child = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        let result = crate::molt_class_set_base(child, method_class(py));
        dec_ref_bits(py, result);
        assert!(exception_pending(py));
        clear_exception(py);
        dec_ref_bits(py, child);
    });
}

#[test]
fn annotations_admit_immutable_heap_types_but_reject_static_types() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let slot = AtomicU64::new(0);
        let heap = init_cached_runtime_class_configured(
            py,
            &slot,
            "ImmutableHeapAnnotations",
            ClassSemanticPolicy::heap(true, true),
            0,
            None,
            None,
            |_, _| true,
        );
        let field = attr_name_bits_from_bytes(py, b"__annotations__").unwrap();
        for class in [
            mappingproxy_class(py),
            method_class(py),
            builtin_classes(py).int,
        ] {
            let result = crate::molt_get_attr_name(class, field);
            dec_ref_bits(py, result);
            assert!(exception_pending(py), "static type exposed annotations");
            clear_exception(py);
        }
        let annotations = crate::molt_get_attr_name(heap, field);
        assert!(!exception_pending(py));
        assert_eq!(
            unsafe { object_type_id(obj_from_bits(annotations).as_ptr().unwrap()) },
            TYPE_ID_DICT
        );
        let again = crate::molt_get_attr_name(heap, field);
        assert_eq!(annotations, again, "heap annotation cache is stable");
        dec_ref_bits(py, again);
        let result = crate::molt_set_attr_name(heap, field, annotations);
        dec_ref_bits(py, result);
        assert!(
            exception_pending(py),
            "immutable heap accepted annotation assignment"
        );
        clear_exception(py);
        let result = crate::molt_del_attr_name(heap, field);
        dec_ref_bits(py, result);
        assert!(
            exception_pending(py),
            "immutable heap accepted annotation deletion"
        );
        clear_exception(py);
        dec_ref_bits(py, annotations);
        dec_ref_bits(py, field);
        clear_atomic_slots(py, &[&slot]);
    });
}

static ANNOTATION_EVALUATIONS: AtomicU64 = AtomicU64::new(0);

extern "C" fn class_annotation_evaluator(format: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if obj_from_bits(format).as_int() != Some(1) {
            return raise_exception::<_>(py, "ValueError", "expected annotation value format");
        }
        ANNOTATION_EVALUATIONS.fetch_add(1, AtomicOrdering::Relaxed);
        let name = attr_name_bits_from_bytes(py, b"answer").unwrap();
        let dictionary = alloc_dict_with_pairs(py, &[name, MoltObject::from_int(42).bits()]);
        dec_ref_bits(py, name);
        MoltObject::from_ptr(dictionary).bits()
    })
}

#[test]
fn immutable_heap_evaluator_and_cache_follow_target_annotation_semantics() {
    use molt_cpython_abi::api::typeobj::TypeAttributeField;
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let state = runtime_state(py);
        let saved = state.sys_version_info.lock().unwrap().clone();
        let mut info = crate::object::ops_sys::runtime_target_python_info(state);
        for minor in [12, 13, 14] {
            info.minor = minor;
            *state.sys_version_info.lock().unwrap() = Some(info.clone());
            ANNOTATION_EVALUATIONS.store(0, AtomicOrdering::Relaxed);
            let slot = AtomicU64::new(0);
            let class = init_cached_runtime_class_configured(
                py,
                &slot,
                "ImmutableHeapEvaluator",
                ClassSemanticPolicy::heap(true, true),
                0,
                None,
                None,
                |_, namespace| {
                    let name = attr_name_bits_from_bytes(py, b"__annotate__").unwrap();
                    let function = crate::builtins::functions::alloc_runtime_function_obj(
                        py,
                        crate::builtins::functions::runtime_fn_addr(
                            "class_annotation_evaluator",
                            class_annotation_evaluator as *const (),
                        ),
                        1,
                    );
                    assert!(!function.is_null());
                    let function = MoltObject::from_ptr(function).bits();
                    unsafe { dict_set_in_place(py, namespace, name, function) };
                    dec_ref_bits(py, function);
                    dec_ref_bits(py, name);
                    !exception_pending(py)
                },
            );
            let ptr = obj_from_bits(class).as_ptr().unwrap();
            let annotations = unsafe {
                crate::builtins::attributes::type_metadata::read_type_annotations(
                    py,
                    ptr,
                    TypeAttributeField::Annotations,
                )
            }
            .expect("immutable heap annotation read");
            let again = unsafe {
                crate::builtins::attributes::type_metadata::read_type_annotations(
                    py,
                    ptr,
                    TypeAttributeField::Annotations,
                )
            }
            .expect("immutable heap cached annotation read");
            assert_eq!(again, annotations);
            assert_eq!(
                ANNOTATION_EVALUATIONS.load(AtomicOrdering::Relaxed),
                u64::from(minor == 14)
            );
            assert!(!exception_pending(py));
            dec_ref_bits(py, again);
            dec_ref_bits(py, annotations);
            clear_atomic_slots(py, &[&slot]);
        }
        *state.sys_version_info.lock().unwrap() = saved;
    });
}

#[test]
fn heap_c_view_names_are_owned_and_follow_runtime_metadata_mutation() {
    use molt_cpython_abi::abi_types::{
        Py_TPFLAGS_BASETYPE, Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_IMMUTABLETYPE, PyHeapTypeObject,
        PyTypeObject,
    };
    use molt_cpython_abi::api::{strings, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
    let _transaction = RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        // The ownership assertions need a mortal value. Interned attribute
        // names intentionally keep the immortal C refcount encoding unchanged.
        let name = MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(
            py,
            b"HeapProjectionName",
        ))
        .bits();
        let class = crate::molt_class_new(name);
        let ptr = obj_from_bits(class).as_ptr().unwrap();
        let result = crate::molt_class_set_base(class, builtin_classes(py).object);
        dec_ref_bits(py, result);
        unsafe { crate::object::class_finish_definition(py, ptr) }.unwrap();
        let qualname = unsafe { crate::class_qualname_bits(ptr) };
        let name_view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name) };
        let qualname_view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(qualname) };
        let before_name = unsafe { (*name_view).ob_refcnt };
        let before_qualname = unsafe { (*qualname_view).ob_refcnt };
        let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class) }.cast::<PyTypeObject>();
        assert!(!view.is_null());
        unsafe {
            assert_eq!(
                (*view).tp_flags
                    & (Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_IMMUTABLETYPE | Py_TPFLAGS_BASETYPE),
                Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_BASETYPE
            );
            let heap = view.cast::<PyHeapTypeObject>();
            assert_eq!((*heap).ht_name, name_view);
            assert_eq!((*heap).ht_qualname, qualname_view);
            let aliases = isize::from(name_view == qualname_view);
            assert_eq!((*name_view).ob_refcnt, before_name + 1 + aliases);
            assert_eq!((*qualname_view).ob_refcnt, before_qualname + 1 + aliases);
            assert!(!GLOBAL_BRIDGE.has_direct_c_refs(name));
            assert_eq!(typeobj::PyType_Ready(view), 0);
            // This public C consumer reads the heap tail's module field.
            assert!(typeobj::PyType_GetModule(view).is_null());
            assert_eq!(
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ),
                1
            );
            molt_cpython_abi::api::errors::PyErr_Clear();
            assert!(!exception_pending(py));
            for (attribute, value, is_qualname) in [
                (b"__name__".as_slice(), b"RenamedHeap".as_slice(), false),
                (
                    b"__qualname__".as_slice(),
                    b"Outer.RenamedHeap".as_slice(),
                    true,
                ),
            ] {
                let key = attr_name_bits_from_bytes(py, attribute).unwrap();
                let value_bits =
                    MoltObject::from_ptr(crate::object::builders::alloc_string_nointern(py, value))
                        .bits();
                let result = crate::molt_set_attr_name(class, key, value_bits);
                dec_ref_bits(py, result);
                dec_ref_bits(py, key);
                assert!(!exception_pending(py));
                let projected = if is_qualname {
                    (*heap).ht_qualname
                } else {
                    (*heap).ht_name
                };
                assert_eq!(
                    GLOBAL_BRIDGE
                        .observed_handle_for_pyobj(projected)
                        .unwrap()
                        .bits(),
                    value_bits
                );
                dec_ref_bits(py, value_bits);
                assert_eq!(
                    std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(projected)).to_bytes(),
                    value
                );
                assert_eq!(
                    std::ffi::CStr::from_ptr((*view).tp_name).to_bytes(),
                    b"RenamedHeap"
                );
            }
            assert_eq!((*name_view).ob_refcnt, before_name);
            assert_eq!((*qualname_view).ob_refcnt, before_qualname);
        }
        let heap = view.cast::<PyHeapTypeObject>();
        let owned_names = unsafe { [(*heap).ht_name, (*heap).ht_qualname] };
        let before_retirement = owned_names.map(|name| unsafe { (*name).ob_refcnt });
        // The cohort primitive consumes its stable runtime holds after all
        // physical projection edges drain. The caller retains only its own ref.
        assert!(GLOBAL_BRIDGE.retire_runtime_type_views(&[class]));
        for (name, before) in owned_names.into_iter().zip(before_retirement) {
            assert_eq!(unsafe { (*name).ob_refcnt }, before - 1);
        }
        dec_ref_bits(py, name);
        dec_ref_bits(py, class);
        assert!(!exception_pending(py));
    });
}
