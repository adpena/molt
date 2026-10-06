//! Exercise real native consumers after normal type namespace mutation.
use super::super::native_test_fixture::NativeType;
use super::*;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::refcount::OwnedPyObject;
use molt_cpython_abi::api::{abstract_number, buffer, memory, numbers, typeobj};
use molt_cpython_abi::type_slots as slots;

thread_local! {
    static COLLISION_ROOT: Cell<usize> = const { Cell::new(0) };
    static COLLISION_FALLBACK: Cell<usize> = const { Cell::new(0) };
    static COLLISION_HASH: Cell<u64> = const { Cell::new(0) };
    static COLLISION_ARMED: Cell<bool> = const { Cell::new(false) };
    static COLLISION_CALLS: Cell<usize> = const { Cell::new(0) };
    static COLLISION_REBIND: Cell<u64> = const { Cell::new(0) };
    static COLLISION_WATCHES: Cell<usize> = const { Cell::new(0) };
}

extern "C" fn collision_hash(_: u64) -> u64 {
    let hash = COLLISION_HASH.with(Cell::get);
    with_gil(|py| inc_ref_bits(&py, hash));
    hash
}
extern "C" fn collision_equal(left: u64, right: u64) -> u64 {
    if COLLISION_ARMED.with(|armed| armed.replace(false)) {
        COLLISION_CALLS.with(|calls| calls.set(calls.get() + 1));
        unsafe {
            let root = COLLISION_ROOT.with(Cell::get) as *mut PyObject;
            let rebound = COLLISION_REBIND.with(Cell::get);
            if rebound != 0 {
                GLOBAL_BRIDGE
                    .bind_static_pyobj_to_runtime_handle(root, rebound, false)
                    .unwrap();
                if typeobj::PyType_Ready(root.cast()) < 0 {
                    return MoltObject::none().bits();
                }
            } else {
                let fallback = COLLISION_FALLBACK.with(Cell::get) as *mut PyObject;
                if object::PyObject_SetAttrString(root, c"__getattr__".as_ptr(), fallback) < 0 {
                    return MoltObject::none().bits();
                }
            }
        }
    }
    MoltObject::from_bool(
        crate::string_obj_to_owned(obj_from_bits(left))
            == crate::string_obj_to_owned(obj_from_bits(right)),
    )
    .bits()
}
extern "C" fn collision_fallback(_: u64, _: u64) -> u64 {
    MoltObject::from_int(37).bits()
}

// Both regressions enter through the same real string-subclass comparison.
unsafe fn collision_name(py: &crate::PyToken<'_>, spelling: u64) -> (u64, u64) {
    unsafe {
        let name = crate::attr_name_bits_from_bytes(py, b"CollisionName").unwrap();
        let name_class = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        crate::molt_class_set_base(name_class, crate::builtin_classes(py).str);
        method(py, name_class, b"__hash__", collision_hash as *const (), 1);
        method(py, name_class, b"__eq__", collision_equal as *const (), 2);
        crate::object::class_finish_definition(py, obj_from_bits(name_class).as_ptr().unwrap())
            .unwrap();
        let subclass_key =
            crate::call::bind::call_bind_borrowed(py, name_class, None, &[spelling], &[], &[]);
        (name_class, subclass_key)
    }
}

#[test]
fn native_slot_mutation_reads_aliases_after_reentrant_dictionary_equality() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let root = native_class(
                c"mutation.CollisionSlots",
                ptr::null_mut(),
                vec![PyType_Slot {
                    slot: slots::Py_tp_getattro,
                    pfunc: object::PyObject_GenericGetAttr as *const () as *mut std::ffi::c_void,
                }],
            );
            let original = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                root.as_ptr(),
                c"__getattribute__".as_ptr(),
            ));
            let spelling = crate::attr_name_bits_from_bytes(py, b"__getattribute__").unwrap();
            let key =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(spelling));
            assert_eq!(
                object::PyObject_GenericSetAttr(root.as_ptr(), key.as_ptr(), ptr::null_mut()),
                0,
                "{}",
                native_error_description()
            );
            let hash = crate::molt_hash_builtin(spelling);
            COLLISION_HASH.with(|slot| slot.set(hash));
            let (name_class, subclass_key) = collision_name(py, spelling);
            let subclass_name =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(subclass_key));
            assert_eq!(
                object::PyObject_GenericSetAttr(
                    root.as_ptr(),
                    subclass_name.as_ptr(),
                    original.as_ptr()
                ),
                0,
                "{}",
                native_error_description()
            );
            let fallback = python_method(py, collision_fallback as *const (), 2);
            COLLISION_ROOT.with(|slot| slot.set(root.as_ptr().addr()));
            COLLISION_FALLBACK.with(|slot| slot.set(fallback.as_ptr().addr()));
            COLLISION_CALLS.with(|slot| slot.set(0));
            COLLISION_ARMED.with(|slot| slot.set(true));
            set(root.as_ptr(), c"__getattribute__", original.as_ptr());
            assert_eq!(COLLISION_CALLS.with(Cell::get), 1);
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(root.as_ptr()));
            let result = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                instance.as_ptr(),
                c"missing_after_comparison".as_ptr(),
            ));
            assert_eq!(numbers::PyLong_AsLong(result.as_ptr()), 37);
            COLLISION_ROOT.with(|slot| slot.set(0));
            COLLISION_FALLBACK.with(|slot| slot.set(0));
            COLLISION_HASH.with(|slot| slot.set(0));
            for bits in [subclass_key, name_class, hash, spelling] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

thread_local! {
    static RETIRE_ROOT: Cell<usize> = const { Cell::new(0) };
    static RETIRE_INSTANCE: Cell<usize> = const { Cell::new(0) };
    static RETIRE_OBSERVATION: Cell<(isize, u32, usize)> = const { Cell::new((-9, 9, 9)) };
    static BUFFER_VIEW: Cell<u64> = const { Cell::new(0) };
    static BUFFER_RELEASES: Cell<(usize, bool)> = const { Cell::new((0, false)) };
    static NATIVE_RELEASES: Cell<usize> = const { Cell::new(0) };
}

extern "C" fn length_seventeen(_: u64) -> u64 {
    MoltObject::from_int(17).bits()
}
extern "C" fn length_twenty_three(_: u64) -> u64 {
    MoltObject::from_int(23).bits()
}
extern "C" fn binary_echo(_: u64, key: u64) -> u64 {
    with_gil(|py| inc_ref_bits(&py, key));
    key
}
unsafe extern "C" fn length_three(_: *mut PyObject) -> isize {
    3
}

unsafe fn native_class(
    name: &'static std::ffi::CStr,
    base: *mut PyObject,
    mut declarations: Vec<PyType_Slot>,
) -> OwnedPyObject {
    declarations.push(PyType_Slot {
        slot: slots::Py_tp_new,
        pfunc: typeobj::PyType_GenericNew as *const () as *mut std::ffi::c_void,
    });
    declarations.push(PyType_Slot {
        slot: 0,
        pfunc: ptr::null_mut(),
    });
    let mut specification = PyType_Spec {
        name: name.as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as i32,
        itemsize: 0,
        flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
        slots: declarations.as_mut_ptr(),
    };
    let result = unsafe {
        OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut specification,
            base,
        ))
    };
    assert!(!result.as_ptr().is_null());
    result
}

unsafe fn python_method(py: &crate::PyToken<'_>, callback: *const (), arity: u64) -> OwnedPyObject {
    let bits = function(py, callback, arity);
    let result =
        unsafe { OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)) };
    dec_ref_bits(py, bits);
    result
}

unsafe fn set(class: *mut PyObject, name: &'static std::ffi::CStr, value: *mut PyObject) {
    let key = unsafe { OwnedPyObject::from_owned(strings::PyUnicode_FromString(name.as_ptr())) };
    assert_eq!(
        unsafe { object::PyObject_SetAttr(class, key.as_ptr(), value) },
        0
    );
}

unsafe extern "C" fn retiring_value(object: *mut PyObject) {
    unsafe {
        let root = RETIRE_ROOT.with(Cell::get) as *mut PyHeapTypeObject;
        let instance = RETIRE_INSTANCE.with(Cell::get) as *mut PyObject;
        let version = (*root).ht_type.tp_version_tag;
        let getitem = (*root)._spec_cache.getitem.addr();
        let length = object::PyObject_Size(instance);
        RETIRE_OBSERVATION.with(|value| value.set((length, version, getitem)));
        memory::PyObject_Free(object.cast());
    }
}

#[test]
fn native_slot_mutation_publishes_all_aliases_and_descendants_before_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let base = native_class(
                c"mutation.NativeBase",
                ptr::null_mut(),
                vec![PyType_Slot {
                    slot: slots::Py_sq_length,
                    pfunc: length_three as *const () as *mut std::ffi::c_void,
                }],
            );
            let child = native_class(c"mutation.NativeChild", base.as_ptr(), vec![]);
            let shadow = native_class(c"mutation.NativeShadow", base.as_ptr(), vec![]);
            let first = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(base.as_ptr()));
            let second = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child.as_ptr()));
            let third = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(shadow.as_ptr()));
            assert!(
                !first.as_ptr().is_null()
                    && !second.as_ptr().is_null()
                    && !third.as_ptr().is_null()
            );
            for instance in [&first, &second, &third] {
                assert_eq!(object::PyObject_Size(instance.as_ptr()), 3);
            }
            let original = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                base.as_ptr(),
                c"__len__".as_ptr(),
            ));
            let seventeen = python_method(py, length_seventeen as *const (), 1);
            let twenty_three = python_method(py, length_twenty_three as *const (), 1);
            set(shadow.as_ptr(), c"__len__", twenty_three.as_ptr());
            set(base.as_ptr(), c"__len__", seventeen.as_ptr());
            assert_eq!(object::PyObject_Size(first.as_ptr()), 17);
            assert_eq!(object::PyObject_Size(second.as_ptr()), 17);
            assert_eq!(object::PyObject_Size(third.as_ptr()), 23);
            // Both distinct C ABIs for the one Python name must have been updated.
            for instance in [&first, &second] {
                let tp = (*instance.as_ptr()).ob_type;
                for id in [slots::Py_sq_length, slots::Py_mp_length] {
                    let callback: unsafe extern "C" fn(*mut PyObject) -> isize =
                        std::mem::transmute(typeobj::PyType_GetSlot(tp, id));
                    assert_eq!(callback(instance.as_ptr()), 17);
                }
            }
            set(shadow.as_ptr(), c"__len__", ptr::null_mut());
            assert_eq!(object::PyObject_Size(third.as_ptr()), 17);
            set(base.as_ptr(), c"__len__", original.as_ptr());
            assert_eq!(object::PyObject_Size(second.as_ptr()), 3);
            // Generic type storage is the real metatype's tp_dictoffset. It
            // aliases this native heap type's dictionary without publication.
            let native_type = base.as_ptr().cast::<PyTypeObject>();
            assert_eq!(
                object::_PyObject_GetDictPtr(base.as_ptr()),
                &raw mut (*native_type).tp_dict
            );
            // Raw GenericSetAttr mutates the dictionary without physical publication.
            let key = OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"__len__".as_ptr()));
            assert_eq!(
                object::PyObject_GenericSetAttr(base.as_ptr(), key.as_ptr(), seventeen.as_ptr()),
                0,
                "{}",
                native_error_description()
            );
            let raw = OwnedPyObject::from_owned(object::PyObject_GenericGetAttr(
                base.as_ptr(),
                key.as_ptr(),
            ));
            assert_eq!(
                raw.as_ptr(),
                seventeen.as_ptr(),
                "{}",
                native_error_description()
            );
            assert_eq!(object::PyObject_Size(second.as_ptr()), 3);
            set(base.as_ptr(), c"__len__", seventeen.as_ptr());
            assert_eq!(object::PyObject_Size(second.as_ptr()), 17);

            let echo = python_method(py, binary_echo as *const (), 2);
            set(base.as_ptr(), c"__getitem__", echo.as_ptr());
            set(base.as_ptr(), c"__add__", echo.as_ptr());
            set(base.as_ptr(), c"__call__", seventeen.as_ptr());
            let number = OwnedPyObject::from_owned(numbers::PyLong_FromLong(41));
            for result in [
                object::PyObject_GetItem(second.as_ptr(), number.as_ptr()),
                abstract_number::PyNumber_Add(second.as_ptr(), number.as_ptr()),
                object::PyObject_CallNoArgs(second.as_ptr()),
            ] {
                let result = OwnedPyObject::from_owned(result);
                assert!(!result.as_ptr().is_null());
                assert!(matches!(numbers::PyLong_AsLong(result.as_ptr()), 17 | 41));
            }

            // The actual displaced foreign value reenters the real C consumer.
            let mut retired_type: PyTypeObject = std::mem::zeroed();
            retired_type.ob_base.ob_base.ob_refcnt = 1;
            retired_type.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            retired_type.tp_name = c"RetiredMutationValue".as_ptr();
            retired_type.tp_flags = Py_TPFLAGS_READY;
            retired_type.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            retired_type.tp_dealloc = Some(retiring_value);
            let retired = memory::_PyObject_New(&raw mut retired_type);
            assert!(!retired.is_null());
            assert_eq!(
                object::PyObject_GenericSetAttr(base.as_ptr(), key.as_ptr(), retired),
                0,
                "{}",
                native_error_description()
            );
            refcount::Py_DECREF(retired);
            let root = base.as_ptr().cast::<PyHeapTypeObject>();
            assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(root.cast()), 1);
            (*root)._spec_cache.getitem = &raw mut Py_None;
            RETIRE_ROOT.with(|value| value.set(root.addr()));
            RETIRE_INSTANCE.with(|value| value.set(second.as_ptr().addr()));
            set(base.as_ptr(), c"__len__", twenty_three.as_ptr());
            assert_eq!(RETIRE_OBSERVATION.with(Cell::get), (23, 0, 0));
            RETIRE_ROOT.with(|value| value.set(0));
            RETIRE_INSTANCE.with(|value| value.set(0));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

thread_local! {
    // Observation/borrowed callback operands only; the test owns both native
    // allocations, protocol tables, and the semantic class for the whole call.
    static LATE_ALIAS_TYPES: Cell<[usize; 2]> = const { Cell::new([0; 2]) };
    static LATE_ALIAS_CLASS: Cell<u64> = const { Cell::new(0) };
    static LATE_ALIAS_CALLS: Cell<usize> = const { Cell::new(0) };
    static LATE_ALIAS_BEFORE: Cell<[isize; 2]> = const { Cell::new([-9; 2]) };
}

unsafe fn native_sequence_size(class: *mut PyTypeObject) -> isize {
    unsafe {
        let instance = OwnedPyObject::from_owned(typeobj::PyType_GenericNew(
            class,
            ptr::null_mut(),
            ptr::null_mut(),
        ));
        assert!(!instance.as_ptr().is_null());
        molt_cpython_abi::api::abstract_sequence::PySequence_Size(instance.as_ptr())
    }
}

unsafe fn assert_physical_mro(class: *mut PyTypeObject, expected: &[*mut PyTypeObject]) {
    unsafe {
        assert_eq!(
            sequences::PyTuple_Size((*class).tp_mro),
            expected.len() as isize
        );
        for (index, &entry) in expected.iter().enumerate() {
            assert_eq!(
                sequences::PyTuple_GetItem((*class).tp_mro, index as isize),
                entry.cast()
            );
        }
    }
}

unsafe extern "C" fn admit_alias_from_watcher(_: *mut PyObject) -> i32 {
    let [alias, child] = LATE_ALIAS_TYPES.with(|types| types.replace([0; 2]));
    if alias == 0 {
        return 0;
    }
    LATE_ALIAS_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe {
        let alias = alias as *mut PyTypeObject;
        let child = child as *mut PyTypeObject;
        GLOBAL_BRIDGE
            .bind_static_pyobj_to_runtime_handle(
                alias.cast(),
                LATE_ALIAS_CLASS.with(Cell::get),
                false,
            )
            .unwrap();
        if typeobj::PyType_Ready(alias) < 0 || typeobj::PyType_Ready(child) < 0 {
            return -1;
        }
        LATE_ALIAS_BEFORE.with(|before| {
            before.set([native_sequence_size(alias), native_sequence_size(child)]);
        });
        0
    }
}

#[test]
fn managed_slot_mutation_publishes_alias_and_descendant_admitted_by_watcher() {
    exercise_watcher_alias_admission(false);
}

#[test]
fn watcher_alias_admission_preserves_a_preexisting_logical_method() {
    exercise_watcher_alias_admission(true);
}

fn exercise_watcher_alias_admission(preexisting_method: bool) {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let base = snapshot_class(py, b"LateSlotAlias");
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let twenty_three = function(py, length_twenty_three as *const (), 1);
            if preexisting_method {
                let seventeen = function(py, length_seventeen as *const (), 1);
                crate::molt_set_attr_name(base, key, seventeen);
                dec_ref_bits(py, seventeen);
            }
            let canonical =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(base));

            if preexisting_method {
                let instance =
                    OwnedPyObject::from_owned(object::PyObject_CallNoArgs(canonical.as_ptr()));
                assert!(!instance.as_ptr().is_null());
                assert_eq!(
                    molt_cpython_abi::api::abstract_sequence::PySequence_Size(instance.as_ptr()),
                    17
                );
            }

            // Allocate before the call, but admit neither type until the actual
            // watcher runs after the outer publication cohort was collected.
            let mut alias_sequence: PySequenceMethods = std::mem::zeroed();
            // The logical dictionary mutation is committed while physical
            // slot publication awaits this watcher. A native declaration and
            // its physical descendants still inherit native3 in both cases;
            // a preexisting canonical dispatcher must not enter their MRO.
            alias_sequence.sq_length = length_three as *const () as *mut std::ffi::c_void;
            let mut child_sequence: PySequenceMethods = std::mem::zeroed();
            let mut alias = NativeType::<PyTypeObject>::subtype(
                &raw mut PyBaseObject_Type,
                c"mutation.LateSlotAlias",
            );
            alias.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            alias.tp_as_sequence = (&raw mut alias_sequence).cast();
            let alias_pointer = &raw mut *alias;
            let mut child =
                NativeType::<PyTypeObject>::subtype(alias_pointer, c"mutation.LateSlotAliasChild");
            child.tp_as_sequence = (&raw mut child_sequence).cast();
            let child_pointer = &raw mut *child;
            assert_ne!(alias.tp_as_sequence, child.tp_as_sequence);
            LATE_ALIAS_TYPES.with(|types| types.set([alias_pointer.addr(), child_pointer.addr()]));
            LATE_ALIAS_CLASS.with(|class| class.set(base));
            LATE_ALIAS_CALLS.with(|calls| calls.set(0));
            LATE_ALIAS_BEFORE.with(|before| before.set([-9; 2]));
            let watcher = typeobj::PyType_AddWatcher(Some(admit_alias_from_watcher));
            assert!(watcher >= 0);
            assert_eq!(typeobj::PyType_Watch(watcher, canonical.as_ptr()), 0);
            assert_ne!(
                (*canonical.as_ptr().cast::<PyTypeObject>()).tp_version_tag,
                0
            );

            crate::molt_set_attr_name(base, key, twenty_three);
            assert!(!crate::exception_pending(py));
            assert_eq!(LATE_ALIAS_CALLS.with(Cell::get), 1);
            assert_eq!(LATE_ALIAS_BEFORE.with(Cell::get), [3, 3]);
            let object_type = &raw mut PyBaseObject_Type;
            assert_physical_mro(alias_pointer, &[alias_pointer, object_type]);
            assert_physical_mro(child_pointer, &[child_pointer, alias_pointer, object_type]);
            let canonical_type = canonical.as_ptr().cast::<PyTypeObject>();
            assert_physical_mro(canonical_type, &[canonical_type, object_type]);
            assert_eq!(child.tp_base, alias_pointer);
            assert_eq!(
                sequences::PyTuple_GetItem(child.tp_bases, 0),
                alias_pointer.cast()
            );
            assert_eq!(native_sequence_size(alias_pointer), 23);
            assert_eq!(native_sequence_size(child_pointer), 23);
            assert_eq!(typeobj::PyType_Unwatch(watcher, canonical.as_ptr()), 0);
            assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
            LATE_ALIAS_CLASS.with(|class| class.set(0));

            // NativeType retires readiness-owned roots and subclass membership;
            // the exact direct binding must end before its storage is freed.
            drop(child);
            assert!(
                GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(alias_pointer.cast(), base)
            );
            drop(alias);
            drop(canonical);
            for bits in [twenty_three, key, base] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

unsafe extern "C" fn arm_rebind_for_slot_lookup(root: *mut PyObject) -> i32 {
    assert_eq!(root.addr(), COLLISION_ROOT.with(Cell::get));
    COLLISION_WATCHES.with(|calls| calls.set(calls.get() + 1));
    // Dictionary commit has completed. The next equality callback is entered
    // by slot publication reading the original runtime class's live namespace.
    COLLISION_ARMED.with(|armed| armed.set(true));
    0
}

#[test]
fn managed_slot_mutation_preserves_root_rebound_during_slot_lookup() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let original_class = snapshot_class(py, b"LookupRebindOriginal");
            let replacement_class = snapshot_class(py, b"LookupRebindReplacement");
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            let seventeen = function(py, length_seventeen as *const (), 1);
            let twenty_three = function(py, length_twenty_three as *const (), 1);
            crate::molt_set_attr_name(replacement_class, key, seventeen);
            let original_view = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(original_class),
            );
            let method =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(seventeen));
            let hash = crate::molt_hash_builtin(key);
            COLLISION_HASH.with(|value| value.set(hash));
            let (name_class, subclass_key) = collision_name(py, key);
            let subclass_name =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(subclass_key));
            assert_eq!(
                object::PyObject_GenericSetAttr(
                    original_view.as_ptr(),
                    subclass_name.as_ptr(),
                    method.as_ptr(),
                ),
                0,
                "{}",
                native_error_description()
            );
            let mut sequence: PySequenceMethods = std::mem::zeroed();
            sequence.sq_length = length_three as *const () as *mut std::ffi::c_void;
            let mut alias = NativeType::<PyTypeObject>::subtype(
                &raw mut PyBaseObject_Type,
                c"mutation.LookupReboundAlias",
            );
            alias.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            alias.tp_as_sequence = (&raw mut sequence).cast();
            let alias_pointer = &raw mut *alias;
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias_pointer.cast(), original_class, false)
                .unwrap();
            assert_eq!(alias.ready(), 0);
            assert_eq!(native_sequence_size(alias_pointer), 3);
            COLLISION_ROOT.with(|value| value.set(alias_pointer.addr()));
            COLLISION_REBIND.with(|value| value.set(replacement_class));
            COLLISION_CALLS.with(|value| value.set(0));
            COLLISION_WATCHES.with(|value| value.set(0));
            COLLISION_ARMED.with(|value| value.set(false));
            let watcher = typeobj::PyType_AddWatcher(Some(arm_rebind_for_slot_lookup));
            assert!(watcher >= 0);
            assert_eq!(typeobj::PyType_Watch(watcher, alias_pointer.cast()), 0);
            assert_ne!(alias.tp_version_tag, 0);

            crate::molt_set_attr_name(original_class, key, twenty_three);
            assert!(!crate::exception_pending(py));
            assert_eq!(COLLISION_WATCHES.with(Cell::get), 1);
            assert_eq!(COLLISION_CALLS.with(Cell::get), 1);
            assert_eq!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(alias_pointer.cast())
                    .unwrap()
                    .bits(),
                replacement_class,
            );
            // Rebinding republishes the new semantic graph with the same
            // physical self, without adopting either canonical C allocation.
            assert_physical_mro(alias_pointer, &[alias_pointer, &raw mut PyBaseObject_Type]);
            let replacement_view = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(replacement_class),
            );
            assert_ne!(replacement_view.as_ptr(), alias_pointer.cast());
            for view in [&original_view, &replacement_view] {
                let tp = view.as_ptr().cast::<PyTypeObject>();
                assert_physical_mro(tp, &[tp, &raw mut PyBaseObject_Type]);
            }
            // The old transaction looked up a different class's function. It
            // must leave the rebound root's existing native dispatch intact.
            assert_eq!(native_sequence_size(alias_pointer), 3);
            assert_eq!(typeobj::PyType_Unwatch(watcher, alias_pointer.cast()), 0);
            assert_eq!(typeobj::PyType_ClearWatcher(watcher), 0);
            COLLISION_ROOT.with(|value| value.set(0));
            COLLISION_REBIND.with(|value| value.set(0));
            COLLISION_HASH.with(|value| value.set(0));
            COLLISION_ARMED.with(|value| value.set(false));

            // The admitted new identity participates in its own normal future
            // mutation; it is not merely abandoned by the old transaction.
            crate::molt_set_attr_name(replacement_class, key, seventeen);
            assert!(!crate::exception_pending(py));
            assert_eq!(native_sequence_size(alias_pointer), 17);
            assert!(
                GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(
                    alias_pointer.cast(),
                    replacement_class,
                )
            );
            drop(alias);
            drop(subclass_name);
            drop(method);
            drop(original_view);
            for bits in [
                subclass_key,
                name_class,
                hash,
                twenty_three,
                seventeen,
                key,
                replacement_class,
                original_class,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

extern "C" fn python_buffer(_: u64, _: u64) -> u64 {
    let bits = BUFFER_VIEW.with(Cell::get);
    with_gil(|py| inc_ref_bits(&py, bits));
    bits
}
extern "C" fn python_release(_: u64, view: u64) -> u64 {
    BUFFER_RELEASES.with(|state| {
        let (count, _) = state.get();
        state.set((count + 1, view == BUFFER_VIEW.with(Cell::get)));
    });
    MoltObject::none().bits()
}
extern "C" fn restricted_release(_: u64, view: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let pointer = obj_from_bits(view).as_ptr().unwrap();
            let format = crate::attr_name_bits_from_bytes(py, b"B").unwrap();
            let mut rejected = 0;
            for operation in 0..4 {
                let result = match operation {
                    0 => crate::molt_memoryview_new(view),
                    1 => crate::molt_memoryview_toreadonly(view),
                    2 => crate::molt_memoryview_cast(
                        view,
                        format,
                        MoltObject::none().bits(),
                        MoltObject::from_bool(false).bits(),
                    ),
                    _ => crate::object::memoryview::memoryview_slice(
                        py,
                        pointer,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                    ),
                };
                if crate::exception_pending(py) {
                    rejected += 1;
                    errors::PyErr_Clear();
                }
                dec_ref_bits(py, result);
            }
            let native =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(view));
            let mut export: Py_buffer = std::mem::zeroed();
            if buffer::PyObject_GetBuffer(native.as_ptr(), &raw mut export, PyBUF_FULL_RO) < 0 {
                rejected += 1;
                errors::PyErr_Clear();
            } else {
                buffer::PyBuffer_Release(&raw mut export);
            }
            dec_ref_bits(py, format);
            BUFFER_RELEASES.with(|state| {
                let (count, _) = state.get();
                state.set((count + 1, rejected == 5));
            });
            MoltObject::none().bits()
        }
    })
}
static BUFFER_BYTES: [u8; 3] = [1, 2, 3];
unsafe extern "C" fn native_buffer(
    receiver: *mut PyObject,
    view: *mut Py_buffer,
    flags: i32,
) -> i32 {
    unsafe {
        buffer::PyBuffer_FillInfo(
            view,
            receiver,
            BUFFER_BYTES.as_ptr().cast_mut().cast(),
            3,
            1,
            flags,
        )
    }
}
unsafe extern "C" fn native_release(_: *mut PyObject, _: *mut Py_buffer) {
    NATIVE_RELEASES.with(|count| count.set(count.get() + 1));
}

#[test]
fn native_python_buffer_slots_preserve_view_identity_and_restrict_release_aliases() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = native_class(c"mutation.PythonBuffer", ptr::null_mut(), vec![]);
            let get = python_method(py, python_buffer as *const (), 2);
            let release = python_method(py, python_release as *const (), 2);
            set(class.as_ptr(), c"__buffer__", get.as_ptr());
            set(class.as_ptr(), c"__release_buffer__", release.as_ptr());
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(class.as_ptr()));
            let bytes = MoltObject::from_ptr(crate::alloc_bytes(py, b"abc")).bits();
            let view_bits = crate::molt_memoryview_new(bytes);
            dec_ref_bits(py, bytes);
            BUFFER_VIEW.with(|value| value.set(view_bits));
            BUFFER_RELEASES.with(|value| value.set((0, false)));
            let mut view: Py_buffer = std::mem::zeroed();
            assert_eq!(
                buffer::PyObject_GetBuffer(instance.as_ptr(), &raw mut view, PyBUF_FULL_RO),
                0
            );
            assert_eq!(view.len, 3);
            buffer::PyBuffer_Release(&raw mut view);
            assert_eq!(BUFFER_RELEASES.with(Cell::get), (1, true));
            BUFFER_VIEW.with(|value| value.set(0));
            dec_ref_bits(py, view_bits);

            let native = native_class(
                c"mutation.NativeBuffer",
                ptr::null_mut(),
                vec![
                    PyType_Slot {
                        slot: slots::Py_bf_getbuffer,
                        pfunc: native_buffer as *const () as *mut std::ffi::c_void,
                    },
                    PyType_Slot {
                        slot: slots::Py_bf_releasebuffer,
                        pfunc: native_release as *const () as *mut std::ffi::c_void,
                    },
                ],
            );
            let child = native_class(c"mutation.NativeBufferChild", native.as_ptr(), vec![]);
            let release = python_method(py, restricted_release as *const (), 2);
            set(child.as_ptr(), c"__release_buffer__", release.as_ptr());
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child.as_ptr()));
            BUFFER_RELEASES.with(|value| value.set((0, false)));
            NATIVE_RELEASES.with(|value| value.set(0));
            assert_eq!(
                buffer::PyObject_GetBuffer(instance.as_ptr(), &raw mut view, PyBUF_FULL_RO),
                0
            );
            buffer::PyBuffer_Release(&raw mut view);
            assert_eq!(BUFFER_RELEASES.with(Cell::get), (1, true));
            assert_eq!(NATIVE_RELEASES.with(Cell::get), 1);
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn managed_base_mutation_publishes_native_descendants_before_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let base = snapshot_class(py, b"ManagedSlotBase");
            let key = crate::attr_name_bits_from_bytes(py, b"__len__").unwrap();
            assert!(!GLOBAL_BRIDGE.type_has_projection(base));
            let view = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(base));
            let child = native_class(c"mutation.ManagedBaseChild", view.as_ptr(), vec![]);
            let grandchild =
                native_class(c"mutation.ManagedBaseGrandchild", child.as_ptr(), vec![]);
            let shadow = native_class(c"mutation.ManagedBaseShadow", view.as_ptr(), vec![]);
            let first = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(child.as_ptr()));
            let second =
                OwnedPyObject::from_owned(object::PyObject_CallNoArgs(grandchild.as_ptr()));
            let third = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(shadow.as_ptr()));
            assert!(
                !first.as_ptr().is_null()
                    && !second.as_ptr().is_null()
                    && !third.as_ptr().is_null()
            );
            let seventeen = function(py, length_seventeen as *const (), 1);
            let twenty_three = function(py, length_twenty_three as *const (), 1);
            let shadow_length =
                OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(twenty_three));
            set(shadow.as_ptr(), c"__len__", shadow_length.as_ptr());
            crate::molt_set_attr_name(base, key, seventeen);
            assert!(!crate::exception_pending(py));
            for instance in [&first, &second] {
                assert_eq!(
                    molt_cpython_abi::api::abstract_sequence::PySequence_Size(instance.as_ptr()),
                    17
                );
            }
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(third.as_ptr()),
                23
            );
            crate::molt_del_attr_name(base, key);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(second.as_ptr()),
                -1
            );
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            errors::PyErr_Clear();
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Size(third.as_ptr()),
                23
            );

            // A native value retired from the managed namespace observes both
            // descendant slot publication and invalidation through real C APIs.
            let mut retired_type: PyTypeObject = std::mem::zeroed();
            retired_type.ob_base.ob_base.ob_refcnt = 1;
            retired_type.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            retired_type.tp_name = c"RetiredManagedMutationValue".as_ptr();
            retired_type.tp_flags = Py_TPFLAGS_READY;
            retired_type.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
            retired_type.tp_dealloc = Some(retiring_value);
            let retired = memory::_PyObject_New(&raw mut retired_type);
            let retired_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(retired).unwrap();
            crate::molt_set_attr_name(base, key, retired_bits);
            dec_ref_bits(py, retired_bits);
            refcount::Py_DECREF(retired);
            let target = grandchild.as_ptr().cast::<PyHeapTypeObject>();
            assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(target.cast()), 1);
            (*target)._spec_cache.getitem = &raw mut Py_None;
            RETIRE_ROOT.with(|value| value.set(target.addr()));
            RETIRE_INSTANCE.with(|value| value.set(second.as_ptr().addr()));
            RETIRE_OBSERVATION.with(|value| value.set((-9, 9, 9)));
            crate::molt_set_attr_name(base, key, twenty_three);
            assert!(!crate::exception_pending(py));
            assert_eq!(RETIRE_OBSERVATION.with(Cell::get), (23, 0, 0));
            RETIRE_ROOT.with(|value| value.set(0));
            RETIRE_INSTANCE.with(|value| value.set(0));
            for bits in [seventeen, twenty_three, key, base] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

thread_local! {
    static OPTIONAL_UNRAISABLE: Cell<usize> = const { Cell::new(0) };
}

extern "C" fn optional_item(_: u64, index: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if obj_from_bits(index).as_int() == Some(0) {
            MoltObject::from_int(31).bits()
        } else {
            crate::raise_exception::<_>(py, "IndexError", "end of optional sequence")
        }
    })
}
extern "C" fn optional_descriptor(receiver: u64, _: u64, _: u64) -> u64 {
    with_gil(|py| inc_ref_bits(&py, receiver));
    receiver
}
extern "C" fn optional_lookup_failure(_: u64, _: u64, _: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::raise_exception::<_>(py, "RuntimeError", "optional lookup sentinel")
    })
}
extern "C" fn optional_capture(_: u64) -> u64 {
    OPTIONAL_UNRAISABLE.with(|count| count.set(count.get() + 1));
    MoltObject::none().bits()
}
unsafe fn raw_slot_value(
    class: *mut PyObject,
    name: &'static std::ffi::CStr,
    value: *mut PyObject,
) {
    unsafe {
        let key = OwnedPyObject::from_owned(strings::PyUnicode_FromString(name.as_ptr()));
        assert_eq!(
            object::PyObject_GenericSetAttr(class, key.as_ptr(), value),
            0,
            "{}",
            native_error_description()
        );
        // Invalidate borrowed lookup caches without changing the installed C slot.
        typeobj::PyType_Modified(class.cast());
    }
}
unsafe fn optional_error(kind: *mut PyObject, expected: &str) {
    unsafe {
        assert_ne!(errors::PyErr_ExceptionMatches(kind), 0);
        let error = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        let text = OwnedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr()));
        assert_eq!(
            std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(text.as_ptr())).to_string_lossy(),
            expected
        );
    }
}

#[test]
fn native_optional_slot_policies_preserve_missing_none_and_lookup_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = native_class(c"mutation.OptionalSlots", ptr::null_mut(), vec![]);
            let tp = class.as_ptr().cast::<PyTypeObject>();
            let instance = OwnedPyObject::from_owned(object::PyObject_CallNoArgs(class.as_ptr()));
            assert!(!instance.as_ptr().is_null());
            let unary = python_method(py, length_seventeen as *const (), 1);
            set(class.as_ptr(), c"__iter__", &raw mut Py_None);
            assert!(object::PyObject_GetIter(instance.as_ptr()).is_null());
            optional_error(
                (&raw mut PyExc_TypeError).cast(),
                "'mutation.OptionalSlots' object is not iterable",
            );
            set(class.as_ptr(), c"__iter__", unary.as_ptr());
            raw_slot_value(class.as_ptr(), c"__iter__", ptr::null_mut());
            assert!(object::PyObject_GetIter(instance.as_ptr()).is_null());
            optional_error(
                (&raw mut PyExc_TypeError).cast(),
                "'mutation.OptionalSlots' object is not iterable",
            );
            let item = python_method(py, optional_item as *const (), 2);
            set(class.as_ptr(), c"__getitem__", item.as_ptr());
            let iterator = OwnedPyObject::from_owned(object::PyObject_GetIter(instance.as_ptr()));
            assert!(!iterator.as_ptr().is_null());
            let first = OwnedPyObject::from_owned(object::PyIter_Next(iterator.as_ptr()));
            assert_eq!(numbers::PyLong_AsLong(first.as_ptr()), 31);
            assert!(object::PyIter_Next(iterator.as_ptr()).is_null());
            assert!(errors::PyErr_Occurred().is_null());

            let binary = python_method(py, binary_echo as *const (), 2);
            set(class.as_ptr(), c"__contains__", binary.as_ptr());
            raw_slot_value(class.as_ptr(), c"__contains__", ptr::null_mut());
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Contains(
                    instance.as_ptr(),
                    first.as_ptr()
                ),
                1
            );
            raw_slot_value(class.as_ptr(), c"__contains__", &raw mut Py_None);
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Contains(
                    instance.as_ptr(),
                    first.as_ptr()
                ),
                -1
            );
            optional_error(
                (&raw mut PyExc_TypeError).cast(),
                "'mutation.OptionalSlots' object is not a container",
            );
            set(class.as_ptr(), c"__hash__", unary.as_ptr());
            raw_slot_value(class.as_ptr(), c"__hash__", &raw mut Py_None);
            assert_eq!(typeobj::PyObject_Hash(instance.as_ptr()), -1);
            assert_ne!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_TypeError).cast()),
                0
            );
            errors::PyErr_Clear();

            let get = python_method(py, optional_descriptor as *const (), 3);
            set(class.as_ptr(), c"__get__", get.as_ptr());
            let descriptor = (*tp).tp_descr_get.unwrap();
            raw_slot_value(class.as_ptr(), c"__get__", ptr::null_mut());
            let unchanged = OwnedPyObject::from_owned(descriptor(
                instance.as_ptr(),
                ptr::null_mut(),
                class.as_ptr(),
            ));
            assert_eq!(unchanged.as_ptr(), instance.as_ptr());
            assert!((*tp).tp_descr_get.is_none());

            // Descriptor lookup failures have slot-specific policies: repr
            // clears them, async replaces them, and contains preserves them.
            let descriptor_class = attribute_class(py, crate::builtin_classes(py).object, false);
            method(
                py,
                descriptor_class,
                b"__get__",
                optional_lookup_failure as *const (),
                3,
            );
            let descriptor_bits =
                crate::call::bind::call_bind_borrowed(py, descriptor_class, None, &[], &[], &[]);
            let broken = OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(descriptor_bits),
            );
            set(class.as_ptr(), c"__repr__", broken.as_ptr());
            let representation =
                OwnedPyObject::from_owned(typeobj::PyObject_Repr(instance.as_ptr()));
            assert!(!representation.as_ptr().is_null());
            assert!(
                std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(representation.as_ptr()))
                    .to_bytes()
                    .starts_with(b"<mutation.OptionalSlots object at ")
            );
            set(class.as_ptr(), c"__contains__", broken.as_ptr());
            assert_eq!(
                molt_cpython_abi::api::abstract_sequence::PySequence_Contains(
                    instance.as_ptr(),
                    first.as_ptr()
                ),
                -1
            );
            optional_error(
                (&raw mut PyExc_RuntimeError).cast(),
                "optional lookup sentinel",
            );
            for (name, slot, expected) in [
                (
                    c"__await__",
                    slots::Py_am_await,
                    "object mutation.OptionalSlots does not have __await__ method",
                ),
                (
                    c"__aiter__",
                    slots::Py_am_aiter,
                    "object mutation.OptionalSlots does not have __aiter__ method",
                ),
                (
                    c"__anext__",
                    slots::Py_am_anext,
                    "object mutation.OptionalSlots does not have __anext__ method",
                ),
            ] {
                set(class.as_ptr(), name, broken.as_ptr());
                let callback: unsafe extern "C" fn(*mut PyObject) -> *mut PyObject =
                    std::mem::transmute(typeobj::PyType_GetSlot(tp, slot));
                assert!(callback(instance.as_ptr()).is_null());
                optional_error((&raw mut PyExc_AttributeError).cast(), expected);
                raw_slot_value(class.as_ptr(), name, ptr::null_mut());
                assert!(callback(instance.as_ptr()).is_null());
                optional_error((&raw mut PyExc_AttributeError).cast(), expected);
            }
            set(class.as_ptr(), c"__iter__", broken.as_ptr());
            let fallback = OwnedPyObject::from_owned(object::PyObject_GetIter(instance.as_ptr()));
            assert!(!fallback.as_ptr().is_null());
            assert!(errors::PyErr_Occurred().is_null());

            let sys_name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
            let sys = crate::builtins::modules::molt_module_new(sys_name);
            crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
            let hook_key = crate::attr_name_bits_from_bytes(py, b"unraisablehook").unwrap();
            let hook = function(py, optional_capture as *const (), 1);
            crate::builtins::modules::molt_module_set_attr(sys, hook_key, hook);
            OPTIONAL_UNRAISABLE.with(|count| count.set(0));
            set(class.as_ptr(), c"__del__", unary.as_ptr());
            let finalize = (*tp).tp_finalize.unwrap();
            raw_slot_value(class.as_ptr(), c"__del__", ptr::null_mut());
            errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"saved error".as_ptr());
            finalize(instance.as_ptr());
            optional_error((&raw mut PyExc_ValueError).cast(), "saved error");
            assert_eq!(OPTIONAL_UNRAISABLE.with(Cell::get), 0);
            set(class.as_ptr(), c"__del__", broken.as_ptr());
            finalize(instance.as_ptr());
            assert_eq!(OPTIONAL_UNRAISABLE.with(Cell::get), 0);
            raw_slot_value(class.as_ptr(), c"__del__", ptr::null_mut());
            for bits in [
                sys_name,
                sys,
                hook_key,
                hook,
                descriptor_bits,
                descriptor_class,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[path = "physical_readiness.rs"]
mod physical_readiness;

#[path = "builtin_slot_identity.rs"]
mod builtin_slot_identity;

#[path = "physical_alias_hierarchy.rs"]
mod physical_alias_hierarchy;
