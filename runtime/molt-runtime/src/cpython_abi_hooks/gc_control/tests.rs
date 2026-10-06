use crate::concurrency::gil::with_gil;
use crate::object::{dec_ref_bits, header_from_obj_ptr};
use molt_cpython_abi::abi_types::{PyObject, PyTypeObject, PyVarObject};
use molt_cpython_abi::api::{errors, memory};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_obj_model::MoltObject;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

unsafe extern "C" {
    fn molt_linked_type_identity_probe_gc_control(
        object: *mut PyObject,
        tracked: c_int,
        finalized: c_int,
    ) -> c_int;
    fn molt_public_type_identity_probe_gc_control(
        object: *mut PyObject,
        tracked: c_int,
        finalized: c_int,
    ) -> c_int;
    fn molt_linked_type_identity_probe_gc_allocation(ty: *mut PyTypeObject) -> c_int;
    fn molt_public_type_identity_probe_gc_allocation(ty: *mut PyTypeObject) -> c_int;
    fn molt_linked_type_identity_probe_gc_collect() -> isize;
    fn molt_public_type_identity_probe_gc_collect() -> isize;
}

type ControlProbe = unsafe extern "C" fn(*mut PyObject, c_int, c_int) -> c_int;
const CONTROL_PROBES: [ControlProbe; 2] = [
    molt_linked_type_identity_probe_gc_control,
    molt_public_type_identity_probe_gc_control,
];

struct TraverseWitness {
    source: *mut PyObject,
    expected: *mut PyObject,
    matches: usize,
    calls: usize,
    clear_source: bool,
    stop: c_int,
}

unsafe extern "C" fn visit_builtin_edge(object: *mut PyObject, raw: *mut c_void) -> c_int {
    let witness = unsafe { &mut *raw.cast::<TraverseWitness>() };
    witness.calls += 1;
    if object == witness.expected {
        witness.matches += 1;
    }
    if !witness.source.is_null() {
        // A public visitor may reenter the source list's storage transaction.
        assert!(unsafe { molt_cpython_abi::api::sequences::PyList_Size(witness.source) } >= 0);
        if witness.clear_source {
            witness.clear_source = false;
            assert_eq!(unsafe { memory::molt_managed_gc_clear(witness.source) }, 0);
        }
    }
    witness.stop
}

unsafe extern "C" fn native_subtype_traverse(
    _object: *mut PyObject,
    _visitor: *mut c_void,
    _context: *mut c_void,
) -> c_int {
    0
}

#[test]
fn builtin_gc_slots_share_runtime_edges_and_keep_raw_carriers_unadmitted() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use molt_cpython_abi::abi_types::*;
        for ty in [
            &raw mut PyList_Type,
            &raw mut PyDict_Type,
            &raw mut PySet_Type,
            &raw mut PyFrozenSet_Type,
            &raw mut PyModule_Type,
            &raw mut PyTraceBack_Type,
        ] {
            assert_eq!(molt_cpython_abi::api::typeobj::PyType_Ready(ty), 0);
            assert!((*ty).tp_traverse.is_some());
            assert!(molt_cpython_abi::api::typeobj::PyType_GenericAlloc(ty, 0).is_null());
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
        }
        let frozenset_clear = PyFrozenSet_Type.tp_clear;
        let traceback_clear = PyTraceBack_Type.tp_clear;
        assert!(frozenset_clear.is_none());
        assert!(traceback_clear.is_none());
        let mut slots = [
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_base,
                pfunc: (&raw mut PyList_Type).cast(),
            },
            PyType_Slot {
                slot: molt_cpython_abi::type_slots::Py_tp_traverse,
                pfunc: native_subtype_traverse as *const () as *mut c_void,
            },
            PyType_Slot {
                slot: 0,
                pfunc: std::ptr::null_mut(),
            },
        ];
        let mut spec = PyType_Spec {
            name: c"gc.NativeListSubtype".as_ptr(),
            basicsize: std::mem::size_of::<PyListObject>() as c_int,
            itemsize: 0,
            flags: (Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC),
            slots: slots.as_mut_ptr(),
        };
        let subtype = molt_cpython_abi::api::typeobj::PyType_FromSpec(&raw mut spec);
        assert!(!subtype.is_null());
        assert!((*subtype.cast::<PyTypeObject>()).tp_clear.is_some());
        assert!(
            molt_cpython_abi::api::typeobj::PyType_GenericAlloc(subtype.cast(), 0).is_null(),
            "overriding traversal must not admit an inherited managed clear slot"
        );
        errors::PyErr_Clear();
        assert_eq!(molt_cpython_abi::api::typeobj::molt_type_clear(subtype), 0);
        molt_cpython_abi::api::refcount::Py_DECREF(subtype);

        let child = crate::alloc_list(&py, &[]);
        let child_bits = MoltObject::from_ptr(child).bits();
        let source = crate::alloc_list(&py, &[child_bits, child_bits]);
        let source_bits = MoltObject::from_ptr(source).bits();
        let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(source_bits);
        let child_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(child_bits);
        dec_ref_bits(&py, child_bits);
        let traverse = PyList_Type.tp_traverse.unwrap();
        let mut witness = TraverseWitness {
            source: view,
            expected: child_view,
            matches: 0,
            calls: 0,
            clear_source: false,
            stop: 73,
        };
        assert_eq!(
            traverse(
                view,
                visit_builtin_edge as *const () as *mut c_void,
                (&raw mut witness).cast()
            ),
            73
        );
        assert_eq!(
            witness.calls, 1,
            "visitor failure must stop the public callback sequence"
        );
        witness.calls = 0;
        witness.matches = 0;
        witness.stop = 0;
        witness.clear_source = true;
        assert_eq!(
            traverse(
                view,
                visit_builtin_edge as *const () as *mut c_void,
                (&raw mut witness).cast()
            ),
            0
        );
        assert_eq!(
            witness.matches, 2,
            "two runtime owners; clean C mirrors add none"
        );
        assert_eq!(molt_cpython_abi::api::sequences::PyList_Size(view), 0);
        assert!(
            GLOBAL_BRIDGE.managed_handle_for_pyobj(child_view).is_none(),
            "snapshot pins must drain after reentrant source clear"
        );
        assert_eq!(memory::molt_managed_gc_clear(view), 0);
        dec_ref_bits(&py, source_bits);
        assert!(!crate::exception_pending(&py));
    });
}

#[repr(C)]
struct ModuleGcState {
    edge: *mut PyObject,
    traverse_status: c_int,
    clear_status: c_int,
    drop_registry_root: bool,
}

static MODULE_CLEAR_CALLS: AtomicUsize = AtomicUsize::new(0);
static MODULE_FREE_CALLS: AtomicUsize = AtomicUsize::new(0);
static MODULE_STATE_OBSERVED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn module_state_traverse(
    object: *mut PyObject,
    visitor: *mut c_void,
    context: *mut c_void,
) -> c_int {
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(object) }
        .cast::<ModuleGcState>();
    assert!(!state.is_null());
    if unsafe { std::mem::replace(&mut (*state).drop_registry_root, false) } {
        let definition = unsafe { molt_cpython_abi::api::modules::PyModule_GetDef(object) };
        assert_eq!(
            unsafe { molt_cpython_abi::api::modules::PyState_RemoveModule(definition) },
            0
        );
        assert_eq!(
            MODULE_FREE_CALLS.load(Ordering::SeqCst),
            0,
            "traversal lease must outlive removal of the last original root"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::modules::PyModule_GetState(object) },
            state.cast()
        );
    }
    let edge = unsafe { (*state).edge };
    if !edge.is_null() {
        let visit: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int =
            unsafe { std::mem::transmute(visitor) };
        let status = unsafe { visit(edge, context) };
        if status != 0 {
            return status;
        }
    }
    let status = unsafe { (*state).traverse_status };
    if status != 0 {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"module traversal callback error".as_ptr(),
            )
        };
    }
    status
}

unsafe extern "C" fn module_state_clear(object: *mut PyObject) -> c_int {
    MODULE_CLEAR_CALLS.fetch_add(1, Ordering::SeqCst);
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(object) }
        .cast::<ModuleGcState>();
    assert!(!state.is_null());
    MODULE_STATE_OBSERVED.store(state.addr(), Ordering::SeqCst);
    let status = unsafe { (*state).clear_status };
    if status != 0 {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"module clear callback error".as_ptr(),
            )
        };
        return status;
    }
    // An explicit nested clear can empty the runtime namespace and observes the
    // same md_state, but does not call m_clear recursively.
    assert_eq!(unsafe { memory::molt_managed_gc_clear(object) }, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_CLEAR(&raw mut (*state).edge) };
    0
}

unsafe extern "C" fn module_state_free(raw: *mut c_void) {
    MODULE_FREE_CALLS.fetch_add(1, Ordering::SeqCst);
    let object = raw.cast::<PyObject>();
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(object) }
        .cast::<ModuleGcState>();
    assert!(!state.is_null());
    assert_eq!(state.addr(), MODULE_STATE_OBSERVED.load(Ordering::SeqCst));
    assert!(!unsafe { molt_cpython_abi::api::modules::PyModule_GetDef(object) }.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_CLEAR(&raw mut (*state).edge) };
    unsafe {
        errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
            c"module free callback error".as_ptr(),
        )
    };
}

#[test]
fn module_gc_callbacks_share_state_ownership_through_clear_and_terminal_free() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use molt_cpython_abi::abi_types::*;
        MODULE_CLEAR_CALLS.store(0, Ordering::SeqCst);
        MODULE_FREE_CALLS.store(0, Ordering::SeqCst);
        let mut definition: PyModuleDef = std::mem::zeroed();
        definition.m_base.ob_base.ob_refcnt = 1;
        definition.m_name = c"module_gc_callback_owner".as_ptr();
        definition.m_size = std::mem::size_of::<ModuleGcState>() as Py_ssize_t;
        definition.m_traverse = module_state_traverse as *const () as *mut c_void;
        definition.m_clear = module_state_clear as *const () as *mut c_void;
        definition.m_free = module_state_free as *const () as *mut c_void;
        let module = molt_cpython_abi::api::modules::PyModule_Create2(&raw mut definition, 1013);
        assert!(!module.is_null());
        let bits = GLOBAL_BRIDGE.managed_handle_for_pyobj(module).unwrap();
        let state =
            molt_cpython_abi::api::modules::PyModule_GetState(module).cast::<ModuleGcState>();
        assert!(!state.is_null());
        // Only native module state owns this list; the runtime module namespace
        // cannot accidentally provide the referent tested below.
        (*state).edge = molt_cpython_abi::api::sequences::PyList_New(0);
        let edge = (*state).edge;
        let mut witness = TraverseWitness {
            source: std::ptr::null_mut(),
            expected: edge,
            matches: 0,
            calls: 0,
            clear_source: false,
            stop: 0,
        };
        assert_eq!(
            PyModule_Type.tp_traverse.unwrap()(
                module,
                visit_builtin_edge as *const () as *mut c_void,
                (&raw mut witness).cast()
            ),
            0
        );
        assert_eq!(witness.matches, 1);
        let mut graph_matches = 0;
        assert_eq!(
            crate::object::heap_lifecycle::visit_owned_gc_edges(
                &py,
                crate::obj_from_bits(bits).as_ptr().unwrap(),
                &mut |found| {
                    if found.kind == molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8
                        && Some(found.value) == GLOBAL_BRIDGE.managed_handle_for_pyobj(edge)
                    {
                        graph_matches += 1;
                    }
                }
            ),
            0
        );
        assert_eq!(
            graph_matches, 1,
            "the collector must see the same module-state owner"
        );
        assert_eq!(PyModule_Type.tp_clear.unwrap()(module), 0);
        assert_eq!(MODULE_CLEAR_CALLS.load(Ordering::SeqCst), 1);
        assert!((*state).edge.is_null());
        assert_eq!(
            molt_cpython_abi::api::modules::PyModule_GetState(module),
            state.cast()
        );
        errors::PyErr_SetString(
            (&raw mut PyExc_RuntimeError).cast(),
            c"outer module retirement error".as_ptr(),
        );
        let outer = errors::PyErr_Occurred();
        molt_cpython_abi::api::refcount::Py_DECREF(module);
        assert_eq!(MODULE_FREE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(errors::PyErr_Occurred(), outer);
        assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(module).is_none());
        errors::PyErr_Clear();
        assert!(!crate::exception_pending(&py));
    });
}

fn module_gc_definition() -> molt_cpython_abi::abi_types::PyModuleDef {
    let mut definition: molt_cpython_abi::abi_types::PyModuleDef = unsafe { std::mem::zeroed() };
    definition.m_base.ob_base.ob_refcnt = 1;
    definition.m_name = c"module_gc_callback_failure".as_ptr();
    definition.m_size = std::mem::size_of::<ModuleGcState>() as isize;
    definition.m_traverse = module_state_traverse as *const () as *mut c_void;
    definition.m_clear = module_state_clear as *const () as *mut c_void;
    definition.m_free = module_state_free as *const () as *mut c_void;
    definition
}

#[test]
fn module_gc_callback_failures_keep_namespace_and_release_snapshot_pins() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use molt_cpython_abi::abi_types::*;
        MODULE_CLEAR_CALLS.store(0, Ordering::SeqCst);
        MODULE_FREE_CALLS.store(0, Ordering::SeqCst);
        let mut definition = module_gc_definition();
        let module = molt_cpython_abi::api::modules::PyModule_Create2(&raw mut definition, 1013);
        assert!(!module.is_null());
        let bits = GLOBAL_BRIDGE.managed_handle_for_pyobj(module).unwrap();
        let ptr = crate::obj_from_bits(bits).as_ptr().unwrap();
        let state =
            molt_cpython_abi::api::modules::PyModule_GetState(module).cast::<ModuleGcState>();
        MODULE_STATE_OBSERVED.store(state.addr(), Ordering::SeqCst);
        (*state).edge = molt_cpython_abi::api::sequences::PyList_New(0);
        let edge = (*state).edge;
        let dictionary = molt_cpython_abi::api::modules::PyModule_GetDict(module);
        assert!(!dictionary.is_null());
        (*state).traverse_status = -67;
        let mut witness = TraverseWitness {
            source: std::ptr::null_mut(),
            expected: edge,
            matches: 0,
            calls: 0,
            clear_source: false,
            stop: 0,
        };
        assert_eq!(
            PyModule_Type.tp_traverse.unwrap()(
                module,
                visit_builtin_edge as *const () as *mut c_void,
                (&raw mut witness).cast()
            ),
            -67
        );
        assert_eq!(
            witness.calls, 0,
            "failed inventories must not expose a partial public snapshot"
        );
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        let args = crate::alloc_tuple(&py, &[bits]);
        assert!(matches!(
            crate::object::gc::get_referents(&py, args),
            Err(crate::object::gc::GcIntrospectionError::Callback(_))
        ));
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        dec_ref_bits(&py, MoltObject::from_ptr(args).bits());
        let outcome = crate::object::gc::collect_cycles(&py);
        assert!(matches!(
            outcome.status,
            crate::object::gc::GcCollectStatus::CallbackError(_)
        ));
        assert!(!(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_GC_PINNED));
        assert!(!(*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_GC_COLLECTING));
        errors::PyErr_Clear();
        let gc = &crate::runtime_state(&py).gc;
        gc.set_thresholds([1, 10, 10]);
        gc.set_enabled(true);
        let pressure = [crate::alloc_list(&py, &[]), crate::alloc_list(&py, &[])];
        errors::PyErr_SetString(
            (&raw mut PyExc_RuntimeError).cast(),
            c"outer automatic GC error".as_ptr(),
        );
        let outer = errors::PyErr_Occurred();
        let automatic = crate::object::gc::collect_pending(&py);
        assert!(matches!(
            automatic.status,
            crate::object::gc::GcCollectStatus::CallbackError(_)
        ));
        assert_eq!(errors::PyErr_Occurred(), outer);
        errors::PyErr_Clear();
        gc.set_thresholds([0, 10, 10]);
        for root in pressure {
            dec_ref_bits(&py, MoltObject::from_ptr(root).bits());
        }
        (*state).traverse_status = 0;
        (*state).clear_status = -71;
        assert_eq!(PyModule_Type.tp_clear.unwrap()(module), -71);
        assert_eq!((*state).edge, edge);
        assert_eq!(MODULE_CLEAR_CALLS.load(Ordering::SeqCst), 1);
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert_eq!(
            molt_cpython_abi::api::modules::PyModule_GetDict(module),
            dictionary
        );
        assert!(
            !molt_cpython_abi::api::mapping::PyDict_GetItemString(dictionary, c"__name__".as_ptr())
                .is_null()
        );
        (*state).clear_status = 0;
        assert_eq!(PyModule_Type.tp_clear.unwrap()(module), 0);
        assert_eq!(MODULE_CLEAR_CALLS.load(Ordering::SeqCst), 2);
        molt_cpython_abi::api::refcount::Py_DECREF(module);
        assert_eq!(MODULE_FREE_CALLS.load(Ordering::SeqCst), 1);
        assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(module).is_none());
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn module_traversal_can_remove_the_last_registry_root_in_every_graph_consumer() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        use molt_cpython_abi::abi_types::*;
        for consumer in 0..4 {
            MODULE_FREE_CALLS.store(0, Ordering::SeqCst);
            let mut definition = module_gc_definition();
            let module =
                molt_cpython_abi::api::modules::PyModule_Create2(&raw mut definition, 1013);
            assert!(!module.is_null());
            let bits = GLOBAL_BRIDGE.managed_handle_for_pyobj(module).unwrap();
            let ptr = crate::obj_from_bits(bits).as_ptr().unwrap();
            let state =
                molt_cpython_abi::api::modules::PyModule_GetState(module).cast::<ModuleGcState>();
            MODULE_STATE_OBSERVED.store(state.addr(), Ordering::SeqCst);
            (*state).edge = molt_cpython_abi::api::sequences::PyList_New(0);
            let edge = (*state).edge;
            let edge_bits = GLOBAL_BRIDGE.managed_handle_for_pyobj(edge).unwrap();
            (*state).drop_registry_root = true;
            assert_eq!(
                molt_cpython_abi::api::modules::PyState_AddModule(module, &raw mut definition),
                0
            );
            molt_cpython_abi::api::refcount::Py_DECREF(module);
            match consumer {
                0 => assert_eq!(
                    crate::object::heap_lifecycle::visit_owned_gc_edges(&py, ptr, &mut |_| {}),
                    0
                ),
                1 => {
                    let mut witness = TraverseWitness {
                        source: std::ptr::null_mut(),
                        expected: edge,
                        matches: 0,
                        calls: 0,
                        clear_source: false,
                        stop: 0,
                    };
                    assert_eq!(
                        PyModule_Type.tp_traverse.unwrap()(
                            module,
                            visit_builtin_edge as *const () as *mut c_void,
                            (&raw mut witness).cast()
                        ),
                        0
                    );
                    assert_eq!(witness.matches, 1);
                }
                2 => assert_eq!(
                    crate::object::gc::collect_cycles(&py).status,
                    crate::object::gc::GcCollectStatus::Completed
                ),
                _ => {
                    let args = crate::alloc_tuple(&py, &[edge_bits]);
                    let referrers = crate::object::gc::get_referrers(&py, args).unwrap();
                    let found = crate::object::seq_access::with_borrowed(referrers, |values| {
                        values.contains(&bits)
                    });
                    assert!(found);
                    dec_ref_bits(&py, MoltObject::from_ptr(referrers).bits());
                    dec_ref_bits(&py, MoltObject::from_ptr(args).bits());
                }
            }
            assert_eq!(MODULE_FREE_CALLS.load(Ordering::SeqCst), 1);
            assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(module).is_none());
            assert!(
                molt_cpython_abi::api::modules::PyState_FindModule(&raw mut definition).is_null()
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
        }
    });
}

#[test]
fn module_state_cycle_reclaims_aliased_runtime_and_c_owners_once() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        MODULE_CLEAR_CALLS.store(0, Ordering::SeqCst);
        MODULE_FREE_CALLS.store(0, Ordering::SeqCst);
        let mut definition = module_gc_definition();
        let module = molt_cpython_abi::api::modules::PyModule_Create2(&raw mut definition, 1013);
        assert!(!module.is_null());
        let state =
            molt_cpython_abi::api::modules::PyModule_GetState(module).cast::<ModuleGcState>();
        MODULE_STATE_OBSERVED.store(state.addr(), Ordering::SeqCst);
        let list = molt_cpython_abi::api::sequences::PyList_New(2);
        (*state).edge = list;
        for index in 0..2 {
            molt_cpython_abi::api::refcount::Py_INCREF(module);
            assert_eq!(
                molt_cpython_abi::api::sequences::PyList_SetItem(list, index, module),
                0
            );
        }
        molt_cpython_abi::api::refcount::Py_DECREF(module);
        assert_eq!(
            crate::object::gc::collect_cycles(&py).status,
            crate::object::gc::GcCollectStatus::Completed
        );
        assert_eq!(MODULE_CLEAR_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(MODULE_FREE_CALLS.load(Ordering::SeqCst), 1);
        assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(module).is_none());
        assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(list).is_none());
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!crate::exception_pending(&py));
    });
}

static MODULE_SHUTDOWN_FREE_CALLS: AtomicUsize = AtomicUsize::new(0);
static MODULE_SHUTDOWN_EXTERNAL_OWNERS: [AtomicUsize; 2] =
    [AtomicUsize::new(0), AtomicUsize::new(0)];

unsafe extern "C" fn module_shutdown_free(raw: *mut c_void) {
    let object = raw.cast::<PyObject>();
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(object) };
    assert!(!state.is_null());
    if MODULE_SHUTDOWN_FREE_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
        // Both original C owners disappear before nested shutdown. The peer
        // and this active callback now depend on the outer cohort pins.
        for owner in &MODULE_SHUTDOWN_EXTERNAL_OWNERS {
            let address = owner.swap(0, Ordering::SeqCst);
            assert_ne!(address, 0);
            unsafe {
                molt_cpython_abi::api::refcount::Py_DECREF(std::ptr::with_exposed_provenance_mut::<
                    PyObject,
                >(address))
            };
        }
        with_gil(|py| {
            crate::c_api::c_api_module_clear_state(&py, crate::runtime_state(&py));
        });
        assert_eq!(
            unsafe { molt_cpython_abi::api::modules::PyModule_GetState(object) },
            state,
            "recursive shutdown must leave active callback state leased"
        );
    }
}

#[test]
fn module_shutdown_pins_the_original_cohort_through_recursive_free() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        MODULE_SHUTDOWN_FREE_CALLS.store(0, Ordering::SeqCst);
        let mut definitions = [module_gc_definition(), module_gc_definition()];
        let mut modules = [std::ptr::null_mut::<PyObject>(); 2];
        for (index, (definition, module)) in
            definitions.iter_mut().zip(modules.iter_mut()).enumerate()
        {
            definition.m_free = module_shutdown_free as *const () as *mut c_void;
            *module = molt_cpython_abi::api::modules::PyModule_Create2(definition, 1013);
            assert!(!(*module).is_null());
            MODULE_SHUTDOWN_EXTERNAL_OWNERS[index].store((*module).addr(), Ordering::SeqCst);
        }
        assert!(crate::c_api::c_api_module_clear_state(
            &py,
            crate::runtime_state(&py)
        ));
        assert_eq!(MODULE_SHUTDOWN_FREE_CALLS.load(Ordering::SeqCst), 2);
        for module in modules {
            assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(module).is_none());
        }
        assert!(
            MODULE_SHUTDOWN_EXTERNAL_OWNERS
                .iter()
                .all(|owner| owner.load(Ordering::SeqCst) == 0)
        );
        assert_eq!(MODULE_SHUTDOWN_FREE_CALLS.load(Ordering::SeqCst), 2);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn c_gc_controls_use_runtime_membership_without_recounting_or_clearing_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let objects = [
            crate::alloc_list(&py, &[]),
            crate::alloc_dict_with_pairs(&py, &[]),
            crate::alloc_tuple(&py, &[MoltObject::from_int(7).bits()]),
        ];
        for ptr in objects {
            let bits = MoltObject::from_ptr(ptr).bits();
            let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits);
            assert!(!view.is_null());
            let initial = crate::object::gc::gc_is_tracked(ptr);
            errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast(),
                c"pending across GC control".as_ptr(),
            );
            let pending = errors::PyErr_Occurred();
            assert!(!pending.is_null());
            let counts = crate::runtime_state(&py).gc.counts();
            let refs = (*header_from_obj_ptr(ptr)).ref_count_snapshot();
            for probe in CONTROL_PROBES {
                assert_eq!(probe(view, c_int::from(initial), 0), 0);
                assert_eq!(crate::object::gc::gc_is_tracked(ptr), initial);
                assert_eq!(crate::runtime_state(&py).gc.counts(), counts);
                assert_eq!((*header_from_obj_ptr(ptr)).ref_count_snapshot(), refs);
                assert_eq!(errors::PyErr_Occurred(), pending);
                assert!(!crate::object::gc::native_gc_is_enrolled(view.addr()));
            }
            errors::PyErr_Clear();
            dec_ref_bits(&py, bits);
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[test]
fn promoted_scalar_lists_keep_one_gc_claim_through_both_c_control_facades() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let state = &crate::runtime_state(&py).gc;
        state.set_enabled(false);
        for integers in [true, false] {
            for promote_before_view in [true, false] {
                let allocation_count = state.counts()[0];
                let ptr = if integers {
                    crate::object::builders::alloc_list_int_from_raw_slice(&py, &[])
                } else {
                    crate::object::builders::alloc_list_bool_from_raw_slice(&py, &[])
                }
                .expect("scalar list allocation");
                let bits = MoltObject::from_ptr(ptr).bits();
                assert!(!crate::object::gc::gc_is_tracked(ptr));
                assert_eq!(state.counts()[0], allocation_count);
                if promote_before_view {
                    crate::molt_list_append(bits, MoltObject::none().bits());
                    assert!(!crate::exception_pending(&py));
                    assert!(crate::object::gc::gc_is_tracked(ptr));
                    assert_eq!(state.counts()[0], allocation_count + 1);
                }
                let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits);
                assert!(!view.is_null());
                assert_eq!(crate::object::object_type_id(ptr), crate::TYPE_ID_LIST);
                assert!(crate::object::gc::gc_is_tracked(ptr));
                assert_eq!(state.counts()[0], allocation_count + 1);
                for probe in CONTROL_PROBES {
                    assert_eq!(probe(view, 1, 0), 0);
                    assert!(crate::object::gc::gc_is_tracked(ptr));
                    assert_eq!(state.counts()[0], allocation_count + 1);
                    assert!(!crate::object::gc::native_gc_is_enrolled(view.addr()));
                }
                dec_ref_bits(&py, bits);
                assert!(!crate::object::gc::gc_is_tracked(ptr));
                assert_eq!(state.counts()[0], allocation_count);
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

static FINALIZER_CALLS: AtomicUsize = AtomicUsize::new(0);
static RESURRECTED: AtomicU64 = AtomicU64::new(0);

extern "C" fn finalized(receiver: u64) -> u64 {
    FINALIZER_CALLS.fetch_add(1, Ordering::SeqCst);
    with_gil(|py| crate::inc_ref_bits(&py, receiver));
    RESURRECTED.store(receiver, Ordering::SeqCst);
    MoltObject::none().bits()
}

#[test]
fn c_gc_finalized_query_survives_untracking_and_retracking_real_finalization() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        FINALIZER_CALLS.store(0, Ordering::SeqCst);
        assert_eq!(RESURRECTED.load(Ordering::SeqCst), 0);
        let name = crate::attr_name_bits_from_bytes(&py, b"ManagedGcFinalization").unwrap();
        let class = crate::molt_class_new(name);
        crate::molt_class_set_base(class, crate::builtin_classes(&py).object);
        let class_ptr = MoltObject::from_bits(class).as_ptr().unwrap();
        let del = crate::attr_name_bits_from_bytes(&py, b"__del__").unwrap();
        let callback =
            MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
                &py,
                crate::builtins::functions::runtime_fn_addr(
                    "managed_gc_finalized",
                    finalized as *const (),
                ),
                1,
            ))
            .bits();
        crate::molt_set_attr_name(class, del, callback);
        crate::object::class_finish_definition(&py, class_ptr).unwrap();
        let bits = crate::alloc_instance_for_class(&py, class_ptr);
        let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits);
        assert!(!view.is_null());
        assert_eq!(memory::PyObject_GC_IsFinalized(view), 0);
        dec_ref_bits(&py, bits);
        assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(RESURRECTED.load(Ordering::SeqCst), bits);
        assert_eq!(
            crate::object::ops_sys::molt_gc_is_finalized(bits),
            MoltObject::from_bool(true).bits()
        );
        for probe in CONTROL_PROBES {
            assert_eq!(probe(view, 1, 1), 0);
        }
        for value in [
            RESURRECTED.swap(0, Ordering::SeqCst),
            callback,
            del,
            class,
            name,
        ] {
            dec_ref_bits(&py, value);
        }
        assert_eq!(FINALIZER_CALLS.load(Ordering::SeqCst), 1);
        assert!(!crate::exception_pending(&py));
    });
}

unsafe extern "C" fn empty_native_traverse(
    _object: *mut PyObject,
    _visit: *mut c_void,
    _context: *mut c_void,
) -> c_int {
    0
}

#[test]
fn both_c_headers_use_native_gc_allocation_and_the_runtime_collector() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    molt_cpython_abi_test_support::link();
    with_gil(|py| unsafe {
        let mut native_type: PyTypeObject = std::mem::zeroed();
        native_type.ob_base.ob_base.ob_refcnt = molt_cpython_abi::abi_types::IMMORTAL_REFCNT;
        native_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        native_type.tp_name = c"GcAllocationProbe".as_ptr();
        native_type.tp_basicsize = std::mem::size_of::<PyVarObject>() as isize;
        native_type.tp_itemsize = std::mem::size_of::<*mut PyObject>() as isize;
        native_type.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_HAVE_GC;
        native_type.tp_traverse = Some(empty_native_traverse);
        for allocate in [
            molt_linked_type_identity_probe_gc_allocation
                as unsafe extern "C" fn(*mut PyTypeObject) -> c_int,
            molt_public_type_identity_probe_gc_allocation,
        ] {
            let counts = crate::runtime_state(&py).gc.counts();
            assert_eq!(allocate(&raw mut native_type), 0);
            assert_eq!(crate::runtime_state(&py).gc.counts(), counts);
        }
        for collect in [
            molt_linked_type_identity_probe_gc_collect as unsafe extern "C" fn() -> isize,
            molt_public_type_identity_probe_gc_collect,
        ] {
            let ptr = crate::alloc_list(&py, &[]);
            let bits = MoltObject::from_ptr(ptr).bits();
            crate::molt_list_append(bits, bits);
            dec_ref_bits(&py, bits);
            let enabled = memory::PyGC_IsEnabled();
            assert!(collect() >= 1);
            assert!(!crate::object::gc::gc_is_tracked(ptr));
            assert_eq!(memory::PyGC_IsEnabled(), enabled);
        }
        assert!(!crate::exception_pending(&py));
    });
}
