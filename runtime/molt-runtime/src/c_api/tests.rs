// NOTE: c_api tests share a single process-global RuntimeState.
// The runtime is initialized once by the first test and reused.
// Each test acquires `RuntimeTestTransaction` to isolate process-global state
// and prevent concurrent GIL re-entry from corrupting the slab allocator.
//
// Run with: cargo test -p molt-runtime --lib -- c_api::tests --test-threads=1
// Individual tests pass; full suite may hit stack overflow from
// deep GIL re-entry accumulation across tests.

use super::*;
use crate::builtins::exceptions::molt_exception_class;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

struct CApiTestGuard {
    _transaction: crate::test_support::RuntimeTestTransaction,
}

impl CApiTestGuard {
    fn new() -> Self {
        let transaction = crate::test_support::RuntimeTestTransaction::new();
        let _ = molt_err_clear();
        Self {
            _transaction: transaction,
        }
    }
}

impl Drop for CApiTestGuard {
    fn drop(&mut self) {}
}

fn assert_pending_exception_class(_py: &PyToken<'_>, expected: &str) {
    assert!(exception_pending(_py));
    let exc_bits = molt_err_fetch();
    assert!(!obj_from_bits(exc_bits).is_none());
    let kind_bits = molt_exception_kind(exc_bits);
    let class_bits = molt_exception_class(kind_bits);
    let expected_bits = crate::builtins::exceptions::exception_type_bits_from_name(_py, expected);
    assert!(
        issubclass_bits(class_bits, expected_bits),
        "expected pending exception to be {expected}"
    );
    dec_ref_bits(_py, class_bits);
    dec_ref_bits(_py, kind_bits);
    dec_ref_bits(_py, exc_bits);
    assert!(!exception_pending(_py));
}

fn assert_none_with_exception_class(_py: &PyToken<'_>, bits: u64, expected: &str) {
    assert!(obj_from_bits(bits).is_none());
    assert_pending_exception_class(_py, expected);
}

/// Build a shaped memoryview over `owner_bits` through the live typed-strided
/// allocator (`alloc_memoryview_from_storage`). Replaces the removed
/// `alloc_memoryview_shaped` convenience wrapper; the behavior under test
/// (descriptor shape/strides/len, release semantics) is identical.
#[allow(clippy::too_many_arguments)]
fn alloc_shaped_memoryview_for_test(
    _py: &PyToken<'_>,
    owner_bits: u64,
    offset: isize,
    itemsize: usize,
    readonly: bool,
    format_bits: u64,
    shape: Vec<isize>,
    strides: Vec<isize>,
) -> *mut u8 {
    let Some(storage) = crate::object::memoryview::TypedStridedStorage::new(
        std::ptr::null_mut(),
        readonly,
        itemsize,
        offset,
        owner_bits,
        format_bits,
        shape,
        strides,
    ) else {
        return std::ptr::null_mut();
    };
    crate::object::builders::alloc_memoryview_from_storage(_py, storage)
}

struct CApiModuleCacheRestore {
    name_bits: u64,
    previous_bits: u64,
}

impl CApiModuleCacheRestore {
    fn new(name_bits: u64) -> Self {
        let previous_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
        let _ = molt_err_clear();
        let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
        let _ = molt_err_clear();
        Self {
            name_bits,
            previous_bits,
        }
    }

    fn name_bits(&self) -> u64 {
        self.name_bits
    }
}

impl Drop for CApiModuleCacheRestore {
    fn drop(&mut self) {
        crate::with_gil_entry_nopanic!(_py, {
            let _ = molt_err_clear();
            let _ = crate::builtins::modules::molt_module_cache_del(self.name_bits);
            let _ = molt_err_clear();
            if !obj_from_bits(self.previous_bits).is_none() {
                let restore_bits = crate::builtins::modules::molt_module_cache_set(
                    self.name_bits,
                    self.previous_bits,
                );
                if !obj_from_bits(restore_bits).is_none() {
                    dec_ref_bits(_py, restore_bits);
                }
                let _ = molt_err_clear();
                dec_ref_bits(_py, self.previous_bits);
            }
            dec_ref_bits(_py, self.name_bits);
        });
    }
}

extern "C" fn c_api_test_meth_varargs(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        let len = molt_sequence_length(args_bits);
        if len < 0 {
            return MoltObject::none().bits();
        }
        MoltObject::from_int(len).bits()
    })
}

extern "C" fn c_api_test_meth_varargs_keywords(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        let pos_len = molt_sequence_length(args_bits);
        if pos_len < 0 {
            return MoltObject::none().bits();
        }
        let kw_len = if kwargs_bits == 0 || obj_from_bits(kwargs_bits).is_none() {
            0
        } else if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
                    return raise_exception::<u64>(_py, "TypeError", "kwargs payload must be dict");
                }
                (dict_order(kwargs_ptr).len() / 2) as i64
            }
        } else {
            0
        };
        MoltObject::from_int(pos_len * 10 + kw_len).bits()
    })
}

extern "C" fn c_api_test_meth_noargs(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        if arg_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "noargs callback expected NULL argument pointer",
            );
        }
        MoltObject::from_int(101).bits()
    })
}

extern "C" fn c_api_test_meth_o(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "missing module self");
        }
        if arg_bits == 0 || obj_from_bits(arg_bits).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "METH_O callback missing arg");
        }
        inc_ref_bits(_py, arg_bits);
        arg_bits
    })
}

extern "C" fn c_api_test_dynamic_varargs(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic self must be an int for this probe",
            );
        };
        let len = molt_sequence_length(args_bits);
        if len < 0 {
            return MoltObject::none().bits();
        }
        MoltObject::from_int(self_value * 10 + len).bits()
    })
}

extern "C" fn c_api_test_dynamic_noargs(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic self must be an int for this probe",
            );
        };
        if arg_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "noargs callback expected NULL argument pointer",
            );
        }
        MoltObject::from_int(1000 + self_value).bits()
    })
}

extern "C" fn c_api_test_dynamic_o(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(self_value) = to_i64(obj_from_bits(self_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic self must be an int for this probe",
            );
        };
        let Some(arg_value) = to_i64(obj_from_bits(arg_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dynamic METH_O arg must be an int for this probe",
            );
        };
        MoltObject::from_int(self_value * 100 + arg_value).bits()
    })
}

extern "C" fn c_api_test_static_noargs(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if self_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "static callback expected NULL self_bits",
            );
        }
        if arg_bits != 0 {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "noargs callback expected NULL argument pointer",
            );
        }
        MoltObject::from_int(204).bits()
    })
}

extern "C" fn c_api_test_bound_identity(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "bound self missing");
        }
        if arg_bits == 0 || obj_from_bits(arg_bits).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "bound arg missing");
        }
        inc_ref_bits(_py, arg_bits);
        arg_bits
    })
}

static FINALIZER_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static FINALIZER_PIN_PROBE_COUNT: AtomicUsize = AtomicUsize::new(0);
static FINALIZER_PIN_PROBE_STATUS: AtomicU32 = AtomicU32::new(0);
static FINALIZER_PIN_PROBE_C_BASELINE: AtomicUsize = AtomicUsize::new(0);
static FINALIZER_PIN_PROBE_RUNTIME_BASELINE: AtomicU32 = AtomicU32::new(0);
static GC_CLEAR_PROBE_CONTAINER_BITS: AtomicU64 = AtomicU64::new(0);
static GC_CLEAR_PROBE_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static GC_CLEAR_PROBE_OBSERVED_EMPTY: AtomicU32 = AtomicU32::new(0);
static SET_NAME_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static CALL_DESCRIPTOR_GET_COUNT: AtomicUsize = AtomicUsize::new(0);
static CALL_DESCRIPTOR_TARGET_BITS: AtomicU64 = AtomicU64::new(0);
static CUSTOM_METACLASS_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static MUTATED_METACLASS_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static INHERITED_CALL_REPLACEMENT_COUNT: AtomicUsize = AtomicUsize::new(0);

extern "C" fn c_api_test_identity(arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if arg_bits == 0 || obj_from_bits(arg_bits).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "identity arg missing");
        }
        inc_ref_bits(_py, arg_bits);
        arg_bits
    })
}

extern "C" fn c_api_test_call_descriptor_get_once(
    self_bits: u64,
    instance_bits: u64,
    owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none()
            || obj_from_bits(instance_bits).is_none()
            || obj_from_bits(owner_bits).is_none()
        {
            return raise_exception::<u64>(_py, "RuntimeError", "descriptor bind args missing");
        }
        let prior = CALL_DESCRIPTOR_GET_COUNT.fetch_add(1, Ordering::SeqCst);
        if prior != 0 {
            return raise_exception::<u64>(_py, "RuntimeError", "descriptor rebound");
        }
        let target_bits = CALL_DESCRIPTOR_TARGET_BITS.load(Ordering::SeqCst);
        if obj_from_bits(target_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "descriptor target missing");
        }
        inc_ref_bits(_py, target_bits);
        target_bits
    })
}

extern "C" fn c_api_test_custom_metaclass_call(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "metaclass self missing");
        }
        if arg_bits == 0 || obj_from_bits(arg_bits).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "metaclass arg missing");
        }
        CUSTOM_METACLASS_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        inc_ref_bits(_py, arg_bits);
        arg_bits
    })
}

extern "C" fn c_api_test_mutated_metaclass_call(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "metaclass self missing");
        }
        MUTATED_METACLASS_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        MoltObject::from_int(97).bits()
    })
}

extern "C" fn c_api_test_replaced_inherited_call(self_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "call self missing");
        }
        INHERITED_CALL_REPLACEMENT_COUNT.fetch_add(1, Ordering::SeqCst);
        inc_ref_bits(_py, arg_bits);
        arg_bits
    })
}

extern "C" fn c_api_test_finalizer_records(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__del__ self missing");
        }
        FINALIZER_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        none_bits()
    })
}

extern "C" fn c_api_test_finalizing_pin_probe(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        const HAS_VIEW: u32 = 1 << 0;
        const HAS_FINALIZING_PIN: u32 = 1 << 1;
        const MATCHED_C_BIAS: u32 = 1 << 2;
        const BALANCED_C_CUSTODY: u32 = 1 << 3;
        const BALANCED_RUNTIME_CUSTODY: u32 = 1 << 4;
        const SURVIVED_GC: u32 = 1 << 5;

        FINALIZER_PIN_PROBE_COUNT.fetch_add(1, Ordering::SeqCst);
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            return none_bits();
        };
        let mut status = 0u32;
        let view =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(self_bits) };
        if !view.is_null() {
            status |= HAS_VIEW;
        }
        if molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(self_bits) {
            status |= HAS_FINALIZING_PIN;
        }

        let c_baseline = if view.is_null() {
            0
        } else {
            unsafe { (*view).ob_refcnt.max(0) as usize }
        };
        FINALIZER_PIN_PROBE_C_BASELINE.store(c_baseline, Ordering::SeqCst);
        if c_baseline == 1 {
            status |= MATCHED_C_BIAS;
        }
        if !view.is_null() {
            unsafe {
                molt_cpython_abi::api::refcount::Py_INCREF(view);
                molt_cpython_abi::api::refcount::Py_DECREF(view);
            }
            if unsafe { (*view).ob_refcnt.max(0) as usize } == c_baseline
                && molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(self_bits)
            {
                status |= BALANCED_C_CUSTODY;
            }
        }

        let runtime_baseline =
            unsafe { (*crate::object::header_from_obj_ptr(self_ptr)).ref_count_snapshot() };
        FINALIZER_PIN_PROBE_RUNTIME_BASELINE.store(runtime_baseline, Ordering::SeqCst);
        inc_ref_bits(_py, self_bits);
        dec_ref_bits(_py, self_bits);
        let runtime_after =
            unsafe { (*crate::object::header_from_obj_ptr(self_ptr)).ref_count_snapshot() };
        if runtime_after == runtime_baseline
            && molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(self_bits)
        {
            status |= BALANCED_RUNTIME_CUSTODY;
        }

        let _ = crate::molt_gc_collect(MoltObject::from_int(2).bits());
        if !exception_pending(_py)
            && unsafe { (*crate::object::header_from_obj_ptr(self_ptr)).type_id }
                == crate::TYPE_ID_OBJECT
            && molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_finalizing_pin(self_bits)
            && unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(self_bits)
            } == view
        {
            status |= SURVIVED_GC;
        }
        FINALIZER_PIN_PROBE_STATUS.store(status, Ordering::SeqCst);
        none_bits()
    })
}

extern "C" fn c_api_test_gc_clear_observes_published_empty(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        GC_CLEAR_PROBE_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        let bits = GC_CLEAR_PROBE_CONTAINER_BITS.load(Ordering::SeqCst);
        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
            return none_bits();
        };
        let empty = unsafe {
            match crate::object::object_type_id(ptr) {
                crate::TYPE_ID_DICT => {
                    (*crate::builtins::containers::dict_order_ptr(ptr)).is_empty()
                        && (*crate::builtins::containers::dict_table_ptr(ptr)).is_empty()
                        && (*crate::builtins::containers::dict_hashes_ptr(ptr)).is_empty()
                }
                crate::TYPE_ID_SET | crate::TYPE_ID_FROZENSET => {
                    (*crate::builtins::containers::set_order_ptr(ptr)).is_empty()
                        && (*crate::builtins::containers::set_table_ptr(ptr)).is_empty()
                        && (*crate::builtins::containers::set_hashes_ptr(ptr)).is_empty()
                }
                _ => false,
            }
        };
        GC_CLEAR_PROBE_OBSERVED_EMPTY.store(u32::from(empty), Ordering::SeqCst);
        none_bits()
    })
}

fn exercise_finalizing_pin_terminal_entry(c_last: bool) {
    let _guard = CApiTestGuard::new();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    crate::cpython_abi_hooks::register_cpython_hooks();
    crate::with_gil_entry_nopanic!(_py, {
        FINALIZER_PIN_PROBE_COUNT.store(0, Ordering::SeqCst);
        FINALIZER_PIN_PROBE_STATUS.store(0, Ordering::SeqCst);
        FINALIZER_PIN_PROBE_C_BASELINE.store(0, Ordering::SeqCst);
        FINALIZER_PIN_PROBE_RUNTIME_BASELINE.store(0, Ordering::SeqCst);

        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_finalizing_pin_probe",
                c_api_test_finalizing_pin_probe as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let (class_bits, attr_storage) = create_guarded_test_class(
            _py,
            if c_last {
                b"FinalizingPinCLast"
            } else {
                b"FinalizingPinRuntimeLast"
            },
            &[(b"__del__", func_bits)],
        );
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };

        if c_last {
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(inst_bits) };
            assert!(!view.is_null());
            assert_eq!(unsafe { (*view).ob_refcnt }, 1);
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
        } else {
            let view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(inst_bits)
            };
            assert!(!view.is_null());
            assert_eq!(unsafe { (*view).ob_refcnt }, 1);
            dec_ref_bits(_py, inst_bits);
        }

        assert_eq!(FINALIZER_PIN_PROBE_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(
            FINALIZER_PIN_PROBE_STATUS.load(Ordering::SeqCst),
            (1 << 6) - 1,
            "finalizer must observe one matched C bias, retain FinalizingPin across balanced C/runtime custody, and survive gc.collect"
        );
        assert_eq!(FINALIZER_PIN_PROBE_C_BASELINE.load(Ordering::SeqCst), 1);
        assert!(FINALIZER_PIN_PROBE_RUNTIME_BASELINE.load(Ordering::SeqCst) >= 2);

        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn finalizing_pin_runtime_owner_terminal_entry_is_stable() {
    exercise_finalizing_pin_terminal_entry(false);
}

#[test]
fn finalizing_pin_c_last_terminal_entry_is_stable() {
    exercise_finalizing_pin_terminal_entry(true);
}

fn exercise_gc_clear_publish_empty(is_set: bool) {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        GC_CLEAR_PROBE_CONTAINER_BITS.store(0, Ordering::SeqCst);
        GC_CLEAR_PROBE_CALL_COUNT.store(0, Ordering::SeqCst);
        GC_CLEAR_PROBE_OBSERVED_EMPTY.store(0, Ordering::SeqCst);

        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_gc_clear_observes_published_empty",
                c_api_test_gc_clear_observes_published_empty as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let (class_bits, attr_storage) = create_guarded_test_class(
            _py,
            if is_set {
                b"GcClearSetChild"
            } else {
                b"GcClearDictChild"
            },
            &[(b"__del__", func_bits)],
        );
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let child_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let container_ptr = if is_set {
            crate::object::builders::alloc_set_with_entries(_py, &[child_bits])
        } else {
            crate::object::builders::alloc_dict_with_pairs(
                _py,
                &[MoltObject::from_int(1).bits(), child_bits],
            )
        };
        assert!(!container_ptr.is_null());
        assert!(!exception_pending(_py));
        let container_bits = MoltObject::from_ptr(container_ptr).bits();
        GC_CLEAR_PROBE_CONTAINER_BITS.store(container_bits, Ordering::SeqCst);
        dec_ref_bits(_py, child_bits);

        unsafe { crate::object::gc::molt_clear(_py, container_ptr) };
        assert_eq!(GC_CLEAR_PROBE_CALL_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(
            GC_CLEAR_PROBE_OBSERVED_EMPTY.load(Ordering::SeqCst),
            1,
            "child finalizer must observe order/table/hash storage already empty"
        );

        GC_CLEAR_PROBE_CONTAINER_BITS.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, container_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn gc_clear_dict_publishes_empty_before_child_finalizer() {
    exercise_gc_clear_publish_empty(false);
}

#[test]
fn gc_clear_set_publishes_empty_before_child_finalizer() {
    exercise_gc_clear_publish_empty(true);
}

// --- Weakref-callback finalization-window probes (council #1 P0) ---------
//
// These globals let a weakref callback observe the refcount of its target AT
// CALLBACK TIME. The molt P0 fix guarantees the weakref callback runs inside
// the finalize+weakref-clear revival window, so the target is provably LIVE
// (rc >= 1) when the callback fires — never at rc=0. `WEAKREF_CB_TARGET_BITS`
// carries the target's bits to the callback (the weakref itself resolves to
// None during the callback, matching CPython), and `WEAKREF_CB_TARGET_RC_AT_FIRE`
// records the refcount the callback observed.
static WEAKREF_CB_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static WEAKREF_CB_TARGET_BITS: AtomicU64 = AtomicU64::new(0);
static WEAKREF_CB_TARGET_RC_AT_FIRE: AtomicU32 = AtomicU32::new(0);
// P6 regression: a callback that calls `gc.collect()` re-enters the cycle
// collector from INSIDE the outer target's revival window. The target must stay
// live (rc >= 1) across that
// re-entrant collection — this is the pure-`gc.collect()`-in-callback path.
static WEAKREF_CB_CALL_GC: AtomicU32 = AtomicU32::new(0);
// Records the target's `type_id` observed from INSIDE the callback AFTER the
// re-entrant `gc.collect()` — while the revival window still holds the target
// live. A freed-mid-collect target would show a poisoned (non-OBJECT) type_id.
static WEAKREF_CB_TYPE_ID_AFTER_GC: AtomicU32 = AtomicU32::new(0);
// P6 Scenario C: a callback that creates a NEW weakref against the dying target
// (`weakref.ref(target)`), re-inserting a registry entry keyed on the about-to-be
// -freed address. `weakref_clear_for_ptr`'s post-loop re-drain must remove it so
// no orphan survives to mis-fire on slot reuse. The callback stashes the new
// weakref's bits so the test can drop it afterward, and we record the by_target
// presence the test inspects.
static WEAKREF_CB_REREGISTER: AtomicU32 = AtomicU32::new(0);
static WEAKREF_CB_REREGISTER_WEAK_BITS: AtomicU64 = AtomicU64::new(0);
static WEAKREF_CB_REREGISTER_NEW_WEAK_OUT: AtomicU64 = AtomicU64::new(0);

fn weakref_target_rc(bits: u64) -> u32 {
    match obj_from_bits(bits).as_ptr() {
        Some(ptr) => unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() },
        None => 0,
    }
}

extern "C" fn c_api_test_weakref_callback_probe(weak_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // CPython contract: the weakref is already dead during its own callback.
        if weak_bits == 0 || obj_from_bits(weak_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "weakref callback arg missing");
        }
        WEAKREF_CB_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        let target_bits = WEAKREF_CB_TARGET_BITS.load(Ordering::SeqCst);
        // Record the target's refcount as observed from inside the callback.
        WEAKREF_CB_TARGET_RC_AT_FIRE.store(weakref_target_rc(target_bits), Ordering::SeqCst);
        if WEAKREF_CB_CALL_GC.load(Ordering::SeqCst) != 0 {
            // Re-enter the cycle collector from inside the outer revival window.
            // The outer target must stay live throughout (rc observed >= 1,
            // asserted by the caller). Record the
            // refcount AFTER the collection too, to prove the target was not freed
            // out from under the callback by the re-entrant collect.
            let _collected = crate::molt_gc_collect(MoltObject::from_int(0).bits());
            if exception_pending(_py) {
                clear_exception(_py);
            }
            WEAKREF_CB_TARGET_RC_AT_FIRE.store(weakref_target_rc(target_bits), Ordering::SeqCst);
            // Read the target header WHILE the window still holds it live. A
            // mid-collect free would poison this; capturing it here (not after the
            // window closes, where the non-resurrected target is legitimately freed)
            // is the sound UAF probe.
            let tid_after = match obj_from_bits(target_bits).as_ptr() {
                Some(p) => unsafe { (*crate::object::header_from_obj_ptr(p)).type_id },
                None => 0,
            };
            WEAKREF_CB_TYPE_ID_AFTER_GC.store(tid_after, Ordering::SeqCst);
        }
        if WEAKREF_CB_REREGISTER.load(Ordering::SeqCst) != 0 {
            // P6 Scenario C: create a fresh weakref against the DYING target. This
            // re-inserts a `by_target` entry keyed on the about-to-be-freed address.
            // `weakref_clear_for_ptr`'s post-loop re-drain must remove it. The new
            // weakref object is a heap instance supplied via REREGISTER_WEAK_BITS.
            let new_weak_bits = WEAKREF_CB_REREGISTER_WEAK_BITS.load(Ordering::SeqCst);
            if new_weak_bits != 0 && !obj_from_bits(new_weak_bits).is_none() {
                let registered =
                    crate::molt_weakref_register(new_weak_bits, target_bits, none_bits());
                // Return value is a fresh strong ref to the weakref; stash it so the
                // test owns and drops it. Do NOT drop here — the test asserts on it.
                WEAKREF_CB_REREGISTER_NEW_WEAK_OUT.store(registered, Ordering::SeqCst);
            }
        }
        none_bits()
    })
}

extern "C" fn c_api_test_set_name_records(self_bits: u64, owner_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__set_name__ self missing");
        }
        if obj_from_bits(owner_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__set_name__ owner missing");
        }
        if obj_from_bits(name_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__set_name__ name missing");
        }
        SET_NAME_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        none_bits()
    })
}

extern "C" fn c_api_test_set_name_deletes_owner_attr(
    self_bits: u64,
    owner_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__set_name__ self missing");
        }
        if obj_from_bits(owner_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__set_name__ owner missing");
        }
        if obj_from_bits(name_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__set_name__ name missing");
        }
        SET_NAME_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        let _ = crate::molt_del_attr_name(owner_bits, name_bits);
        if exception_pending(_py) {
            return none_bits();
        }

        // Prove `molt_class_apply_set_name` retained the borrowed class-dict
        // key/value pair across arbitrary hook execution.  Without that owner,
        // deleting the class attr above can free both objects before this point.
        inc_ref_bits(_py, self_bits);
        dec_ref_bits(_py, self_bits);
        inc_ref_bits(_py, name_bits);
        dec_ref_bits(_py, name_bits);
        none_bits()
    })
}

extern "C" fn c_api_test_descriptor_get_deletes_owner_attr(
    self_bits: u64,
    _instance_bits: u64,
    owner_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__get__ self missing");
        }
        if obj_from_bits(owner_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__get__ owner missing");
        }
        let name_bits = unsafe { molt_string_from(b"marker".as_ptr(), 6) };
        assert!(!obj_from_bits(name_bits).is_none());
        let _ = crate::molt_del_attr_name(owner_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return none_bits();
        }

        // `descriptor_bind` must own the descriptor while `__get__` runs.  The
        // owner-class deletion above removes the class-dict owner, so this
        // retain/release pair catches stale borrowed descriptor bits.
        inc_ref_bits(_py, self_bits);
        dec_ref_bits(_py, self_bits);

        unsafe { molt_string_from(b"descriptor-value".as_ptr(), 16) }
    })
}

extern "C" fn c_api_test_init_stores_tag_and_borrows_self(self_bits: u64, tag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            return raise_exception::<u64>(_py, "RuntimeError", "__init__ self missing");
        }
        if tag_bits == 0 || obj_from_bits(tag_bits).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "__init__ tag missing");
        }
        let name_bits = unsafe { molt_string_from(b"tag".as_ptr(), 3) };
        assert!(!obj_from_bits(name_bits).is_none());
        let result =
            crate::object::ops_builtins::molt_object_setattr(self_bits, name_bits, tag_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        result
    })
}

fn create_test_heap_class(_py: &PyToken<'_>, name: &[u8], attrs: &[(&[u8], u64)]) -> u64 {
    let builtins = crate::builtins::classes::builtin_classes(_py);
    let name_bits = unsafe { molt_string_from(name.as_ptr(), name.len() as u64) };
    assert!(!obj_from_bits(name_bits).is_none());
    let namespace_bits = molt_dict_new(attrs.len() as u64);
    assert!(!obj_from_bits(namespace_bits).is_none());
    for &(attr_name, value_bits) in attrs {
        let attr_bits = unsafe { molt_string_from(attr_name.as_ptr(), attr_name.len() as u64) };
        assert!(!obj_from_bits(attr_bits).is_none());
        assert_eq!(
            molt_mapping_setitem(namespace_bits, attr_bits, value_bits),
            0
        );
        dec_ref_bits(_py, attr_bits);
    }
    let class_bits = crate::builtins::types::molt_type_new(
        builtins.type_obj,
        name_bits,
        none_bits(),
        namespace_bits,
        none_bits(),
    );
    assert!(!obj_from_bits(class_bits).is_none());
    dec_ref_bits(_py, namespace_bits);
    dec_ref_bits(_py, name_bits);
    class_bits
}

fn create_test_type(
    _py: &PyToken<'_>,
    metaclass_bits: u64,
    name: &[u8],
    base_bits: u64,
    attrs: &[(&[u8], u64)],
) -> u64 {
    let name_bits = unsafe { molt_string_from(name.as_ptr(), name.len() as u64) };
    assert!(!obj_from_bits(name_bits).is_none());
    let namespace_bits = molt_dict_new(attrs.len() as u64);
    assert!(!obj_from_bits(namespace_bits).is_none());
    for &(attr_name, value_bits) in attrs {
        let attr_bits = unsafe { molt_string_from(attr_name.as_ptr(), attr_name.len() as u64) };
        assert!(!obj_from_bits(attr_bits).is_none());
        assert_eq!(
            molt_mapping_setitem(namespace_bits, attr_bits, value_bits),
            0
        );
        dec_ref_bits(_py, attr_bits);
    }
    let class_bits = crate::builtins::types::molt_type_new(
        metaclass_bits,
        name_bits,
        base_bits,
        namespace_bits,
        none_bits(),
    );
    assert!(!obj_from_bits(class_bits).is_none());
    assert!(!exception_pending(_py));
    dec_ref_bits(_py, namespace_bits);
    dec_ref_bits(_py, name_bits);
    class_bits
}

fn create_guarded_test_class(
    _py: &PyToken<'_>,
    name: &[u8],
    attrs: &[(&[u8], u64)],
) -> (u64, Vec<u64>) {
    let builtins = crate::builtins::classes::builtin_classes(_py);
    let name_bits = unsafe { molt_string_from(name.as_ptr(), name.len() as u64) };
    assert!(!obj_from_bits(name_bits).is_none());
    let mut attr_storage = Vec::with_capacity(attrs.len() * 2);
    for &(attr_name, value_bits) in attrs {
        let attr_bits = unsafe { molt_string_from(attr_name.as_ptr(), attr_name.len() as u64) };
        assert!(!obj_from_bits(attr_bits).is_none());
        attr_storage.push(attr_bits);
        attr_storage.push(value_bits);
    }
    let bases = [builtins.object];
    let class_bits = unsafe {
        crate::object::ops::molt_guarded_class_def(
            name_bits,
            crate::provenance::abi::expose_address(bases.as_ptr()),
            bases.len() as u64,
            crate::provenance::abi::expose_address(attr_storage.as_ptr()),
            attrs.len() as u64,
            std::mem::size_of::<u64>() as i64,
            1,
            0,
        )
    };
    assert!(!obj_from_bits(class_bits).is_none());
    dec_ref_bits(_py, name_bits);
    (class_bits, attr_storage)
}

fn heap_refcount(bits: u64) -> u32 {
    let ptr = obj_from_bits(bits).as_ptr().expect("expected heap object");
    unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
}

fn alloc_test_weakref(_py: &PyToken<'_>) -> u64 {
    let reference_type = crate::builtins::classes::builtin_classes(_py).reference_type;
    let reference_type_ptr = obj_from_bits(reference_type)
        .as_ptr()
        .expect("ReferenceType class ptr");
    let weak_bits = unsafe { crate::alloc_instance_for_class(_py, reference_type_ptr) };
    let weak_ptr = obj_from_bits(weak_bits)
        .as_ptr()
        .expect("ReferenceType instance ptr");
    assert_eq!(unsafe { object_type_id(weak_ptr) }, crate::TYPE_ID_WEAKREF);
    weak_bits
}

#[test]
fn guarded_class_def_retains_attr_values_after_source_drop() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let value_bits = unsafe { molt_string_from(b"owned-class-attr".as_ptr(), 16) };
        assert!(!obj_from_bits(value_bits).is_none());
        let source_refcount = heap_refcount(value_bits);
        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"AttrOwner", &[(b"marker", value_bits)]);
        assert_eq!(
            heap_refcount(value_bits),
            source_refcount + 1,
            "class definition must retain heap attribute values"
        );

        dec_ref_bits(_py, value_bits);

        let attr_bits = attr_storage[0];
        let got_bits = molt_object_getattr(class_bits, attr_bits);
        assert_eq!(got_bits, value_bits);
        dec_ref_bits(_py, got_bits);

        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

#[test]
fn guarded_class_def_keeps_structural_type_names_out_of_descriptor_shadowing() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let shadow_bits = unsafe { molt_string_from(b"ShadowName".as_ptr(), 10) };
        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"ActualName", &[(b"__name__", shadow_bits)]);

        let resolved = molt_object_getattr(class_bits, attr_storage[0]);
        assert_eq!(
            crate::string_obj_to_owned(obj_from_bits(resolved)).as_deref(),
            Some("ActualName")
        );

        dec_ref_bits(_py, resolved);
        dec_ref_bits(_py, shadow_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

#[test]
fn guarded_class_def_set_name_keeps_descriptor_attr_value_owner() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        SET_NAME_CALL_COUNT.store(0, Ordering::SeqCst);
        let set_name_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_set_name_records",
                c_api_test_set_name_records as *const (),
            ),
            3,
        );
        assert!(!set_name_ptr.is_null());
        let set_name_bits = MoltObject::from_ptr(set_name_ptr).bits();
        let descriptor_class_bits = create_test_heap_class(
            _py,
            b"DescriptorWithSetName",
            &[(b"__set_name__", set_name_bits)],
        );
        let _ = crate::molt_class_apply_set_name(descriptor_class_bits);
        let descriptor_class_ptr = obj_from_bits(descriptor_class_bits)
            .as_ptr()
            .expect("descriptor class ptr");
        let descriptor_bits = unsafe { crate::alloc_instance_for_class(_py, descriptor_class_ptr) };
        assert!(!obj_from_bits(descriptor_bits).is_none());
        let source_refcount = heap_refcount(descriptor_bits);

        let (owner_class_bits, attr_storage) =
            create_guarded_test_class(_py, b"OwnerWithDescriptor", &[(b"marker", descriptor_bits)]);
        crate::builtins::attr::clear_attr_tls_caches(_py);
        assert_eq!(SET_NAME_CALL_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(
            heap_refcount(descriptor_bits),
            source_refcount + 1,
            "class dict must retain descriptor after __set_name__ hook dispatch"
        );

        dec_ref_bits(_py, descriptor_bits);

        let attr_bits = attr_storage[0];
        let got_bits = molt_object_getattr(owner_class_bits, attr_bits);
        assert_eq!(got_bits, descriptor_bits);
        dec_ref_bits(_py, got_bits);

        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, owner_class_bits);
        dec_ref_bits(_py, descriptor_class_bits);
        dec_ref_bits(_py, set_name_bits);
    });
}

#[test]
fn class_apply_set_name_retains_entries_across_hook_mutation() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        SET_NAME_CALL_COUNT.store(0, Ordering::SeqCst);
        let set_name_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_set_name_deletes_owner_attr",
                c_api_test_set_name_deletes_owner_attr as *const (),
            ),
            3,
        );
        assert!(!set_name_ptr.is_null());
        let set_name_bits = MoltObject::from_ptr(set_name_ptr).bits();
        let descriptor_class_bits = create_test_heap_class(
            _py,
            b"DescriptorDeletingOwnerAttr",
            &[(b"__set_name__", set_name_bits)],
        );
        let _ = crate::molt_class_apply_set_name(descriptor_class_bits);
        let descriptor_class_ptr = obj_from_bits(descriptor_class_bits)
            .as_ptr()
            .expect("descriptor class ptr");
        let descriptor_bits = unsafe { crate::alloc_instance_for_class(_py, descriptor_class_ptr) };
        assert!(!obj_from_bits(descriptor_bits).is_none());

        let owner_class_bits = create_test_heap_class(
            _py,
            b"OwnerWithDeletingDescriptor",
            &[(b"marker", descriptor_bits)],
        );
        dec_ref_bits(_py, descriptor_bits);

        let res_bits = crate::molt_class_apply_set_name(owner_class_bits);
        assert!(obj_from_bits(res_bits).is_none());
        assert!(!exception_pending(_py));
        assert_eq!(SET_NAME_CALL_COUNT.load(Ordering::SeqCst), 1);

        dec_ref_bits(_py, owner_class_bits);
        dec_ref_bits(_py, descriptor_class_bits);
        dec_ref_bits(_py, set_name_bits);
    });
}

#[test]
fn descriptor_bind_retains_descriptor_across_get_mutation() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        crate::builtins::attr::clear_attr_tls_caches(_py);
        let get_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_descriptor_get_deletes_owner_attr",
                c_api_test_descriptor_get_deletes_owner_attr as *const (),
            ),
            3,
        );
        assert!(!get_ptr.is_null());
        let get_bits = MoltObject::from_ptr(get_ptr).bits();
        let descriptor_class_bits =
            create_test_heap_class(_py, b"DescriptorDeletingOnGet", &[(b"__get__", get_bits)]);
        let descriptor_class_ptr = obj_from_bits(descriptor_class_bits)
            .as_ptr()
            .expect("descriptor class ptr");
        let descriptor_bits = unsafe { crate::alloc_instance_for_class(_py, descriptor_class_ptr) };
        assert!(!obj_from_bits(descriptor_bits).is_none());

        let owner_class_bits =
            create_test_heap_class(_py, b"OwnerDescriptorGet", &[(b"marker", descriptor_bits)]);
        let owner_class_ptr = obj_from_bits(owner_class_bits)
            .as_ptr()
            .expect("owner class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, owner_class_ptr) };
        assert!(!obj_from_bits(inst_bits).is_none());
        dec_ref_bits(_py, descriptor_bits);

        let attr_bits = unsafe { molt_string_from(b"marker".as_ptr(), 6) };
        assert!(!obj_from_bits(attr_bits).is_none());
        let got_bits = molt_object_getattr(inst_bits, attr_bits);
        assert_eq!(
            string_obj_to_owned(obj_from_bits(got_bits)).as_deref(),
            Some("descriptor-value")
        );
        assert!(!exception_pending(_py));

        dec_ref_bits(_py, got_bits);
        dec_ref_bits(_py, attr_bits);
        dec_ref_bits(_py, inst_bits);
        dec_ref_bits(_py, owner_class_bits);
        dec_ref_bits(_py, descriptor_class_bits);
        dec_ref_bits(_py, get_bits);
        crate::builtins::attr::clear_attr_tls_caches(_py);
    });
}

#[test]
fn guarded_class_def_arms_and_runs_instance_finalizer() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        FINALIZER_CALL_COUNT.store(0, Ordering::SeqCst);
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_finalizer_records",
                c_api_test_finalizer_records as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"FinalizerA", &[(b"__del__", func_bits)]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let class_flags =
            unsafe { (*crate::object::header_from_obj_ptr(class_ptr)).load_metadata_flags() };
        assert_ne!(
            class_flags & crate::object::HEADER_FLAG_CLASS_HAS_FINALIZER,
            0,
            "sealed class must carry the finalizer fact before allocation"
        );

        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().expect("instance ptr");
        assert!(
            unsafe { crate::object::object_class_has_finalizer(inst_ptr) },
            "instance finalization must derive from the current class authority"
        );

        dec_ref_bits(_py, inst_bits);
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "last drop must run __del__ exactly once"
        );

        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

// Council #1 P0: a weakref callback must run while its target is LIVE (rc >= 1),
// inside the finalize+weakref-clear revival window — never at rc=0. Before the
// fix, `dec_ref_ptr` dropped the finalizer's revival ref before clearing
// weakrefs, so the callback ran with the target at refcount 0 and any code that
// re-touched the target's storage was a use-after-free. This drives the exact
// dec->0 path on a plain (no-`__del__`) instance that carries a weakref with a
// callback and asserts the callback observed the target at rc >= 1.
#[test]
fn weakref_callback_runs_with_live_target_not_rc0() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        WEAKREF_CB_CALL_COUNT.store(0, Ordering::SeqCst);
        WEAKREF_CB_TARGET_BITS.store(0, Ordering::SeqCst);
        WEAKREF_CB_TARGET_RC_AT_FIRE.store(0, Ordering::SeqCst);

        // A plain class: instances have NO __del__, so the revival window here
        // is opened SOLELY by the HAS_WEAKREF lifetime-boundary bit.
        let (class_bits, attr_storage) = create_guarded_test_class(_py, b"WeakTargetA", &[]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().expect("instance ptr");
        assert!(
            !unsafe { crate::object::object_class_has_finalizer(inst_ptr) },
            "plain instance must not derive finalizer sensitivity"
        );

        let weak_bits = alloc_test_weakref(_py);

        // The callback records the target refcount it observes at fire time.
        let cb_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_weakref_callback_probe",
                c_api_test_weakref_callback_probe as *const (),
            ),
            1,
        );
        assert!(!cb_ptr.is_null());
        let cb_bits = MoltObject::from_ptr(cb_ptr).bits();

        WEAKREF_CB_TARGET_BITS.store(inst_bits, Ordering::SeqCst);
        let registered = crate::molt_weakref_register(weak_bits, inst_bits, cb_bits);
        assert!(is_truthy(_py, obj_from_bits(registered)));

        assert_ne!(
            unsafe { (*crate::object::header_from_obj_ptr(inst_ptr)).load_synchronized_flags() }
                & crate::object::HEADER_FLAG_HAS_WEAKREF,
            0,
            "registering a weakref must stamp the HAS_WEAKREF lifetime-boundary bit"
        );

        // Drop the sole strong reference to the target: rc -> 0 -> revival window
        // -> weakref_clear_for_ptr fires the callback. The callback must observe a
        // LIVE target.
        dec_ref_bits(_py, inst_bits);

        assert_eq!(
            WEAKREF_CB_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "weakref callback must fire exactly once on the target's death"
        );
        assert!(
            WEAKREF_CB_TARGET_RC_AT_FIRE.load(Ordering::SeqCst) >= 1,
            "weakref callback must run with the target LIVE (rc >= 1), not at rc=0 \
             (observed rc={})",
            WEAKREF_CB_TARGET_RC_AT_FIRE.load(Ordering::SeqCst)
        );

        // The target was not resurrected, so it is now truly destroyed.
        dec_ref_bits(_py, weak_bits);
        dec_ref_bits(_py, cb_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

#[test]
fn explicit_gc_collect_preserves_strongly_reachable_weakref_target() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"WeakTargetStillReachable", &[]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let target_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let target_ptr = obj_from_bits(target_bits).as_ptr().expect("target ptr");
        let weak_bits = alloc_test_weakref(_py);

        let registered = crate::molt_weakref_register(weak_bits, target_bits, none_bits());
        assert!(is_truthy(_py, obj_from_bits(registered)));

        let collected = crate::molt_gc_collect(MoltObject::from_int(2).bits());
        assert!(
            !obj_from_bits(collected).is_none(),
            "gc.collect must return its integer collection count"
        );

        let resolved = crate::molt_weakref_get(weak_bits);
        assert_eq!(
            obj_from_bits(resolved).as_ptr(),
            Some(target_ptr),
            "explicit collection must not clear a weakref whose target still has a strong owner"
        );
        dec_ref_bits(_py, resolved);

        dec_ref_bits(_py, target_bits);
        assert!(obj_from_bits(crate::molt_weakref_get(weak_bits)).is_none());
        dec_ref_bits(_py, weak_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

#[test]
fn repeated_weakref_callbacks_transfer_registration_custody_without_leak() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        WEAKREF_CB_CALL_COUNT.store(0, Ordering::SeqCst);
        WEAKREF_CB_CALL_GC.store(0, Ordering::SeqCst);
        WEAKREF_CB_REREGISTER.store(0, Ordering::SeqCst);

        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"WeakCallbackCustody", &[]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let cb_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_weakref_callback_probe",
                c_api_test_weakref_callback_probe as *const (),
            ),
            1,
        );
        assert!(!cb_ptr.is_null());
        let cb_bits = MoltObject::from_ptr(cb_ptr).bits();
        let callback_baseline = weakref_target_rc(cb_bits);
        assert_eq!(callback_baseline, 1);

        for _ in 0..16 {
            let target_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
            let weak_bits = alloc_test_weakref(_py);
            WEAKREF_CB_TARGET_BITS.store(target_bits, Ordering::SeqCst);
            let registered = crate::molt_weakref_register(weak_bits, target_bits, cb_bits);
            assert!(is_truthy(_py, obj_from_bits(registered)));
            assert_eq!(
                weakref_target_rc(cb_bits),
                callback_baseline + 1,
                "registration owns exactly one callback edge"
            );

            dec_ref_bits(_py, target_bits);
            assert_eq!(
                weakref_target_rc(cb_bits),
                callback_baseline,
                "callback invocation must consume the transferred registration edge"
            );
            dec_ref_bits(_py, weak_bits);
        }
        assert_eq!(WEAKREF_CB_CALL_COUNT.load(Ordering::SeqCst), 16);

        dec_ref_bits(_py, cb_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

// P6 (council #1): a weakref callback that calls `gc.collect()` re-enters the
// cycle collector from INSIDE the dying target's revival window. The window must
// keep the target live
// (rc >= 1) across that re-entrant collection — a freed-out-from-under-the-callback
// target would be a use-after-free. This is the pure-`gc.collect()`-in-callback
// path the resurrection P0 fix (0e3b062fd) must hold against.
#[test]
fn weakref_callback_calling_gc_collect_keeps_target_live() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        WEAKREF_CB_CALL_COUNT.store(0, Ordering::SeqCst);
        WEAKREF_CB_TARGET_BITS.store(0, Ordering::SeqCst);
        WEAKREF_CB_TARGET_RC_AT_FIRE.store(0, Ordering::SeqCst);
        WEAKREF_CB_CALL_GC.store(1, Ordering::SeqCst);
        WEAKREF_CB_TYPE_ID_AFTER_GC.store(0, Ordering::SeqCst);

        let (class_bits, attr_storage) = create_guarded_test_class(_py, b"WeakTargetGc", &[]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let weak_bits = alloc_test_weakref(_py);

        let cb_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_weakref_callback_probe",
                c_api_test_weakref_callback_probe as *const (),
            ),
            1,
        );
        assert!(!cb_ptr.is_null());
        let cb_bits = MoltObject::from_ptr(cb_ptr).bits();

        WEAKREF_CB_TARGET_BITS.store(inst_bits, Ordering::SeqCst);
        let registered = crate::molt_weakref_register(weak_bits, inst_bits, cb_bits);
        assert!(is_truthy(_py, obj_from_bits(registered)));

        // Drop the sole strong ref: the callback fires inside the window and calls
        // gc.collect(), which re-enters the weakref subsystem. The target must be
        // live both during and after that re-entrant collection.
        dec_ref_bits(_py, inst_bits);

        assert_eq!(
            WEAKREF_CB_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "callback must fire exactly once"
        );
        assert!(
            WEAKREF_CB_TARGET_RC_AT_FIRE.load(Ordering::SeqCst) >= 1,
            "target must stay live (rc >= 1) across a re-entrant gc.collect() in the \
             callback (observed rc={})",
            WEAKREF_CB_TARGET_RC_AT_FIRE.load(Ordering::SeqCst)
        );
        // The header observed FROM INSIDE the callback, after gc.collect() but while
        // the window still holds the target live, must be intact (TYPE_ID_OBJECT) —
        // proving the re-entrant collect did not free the target out from under the
        // running callback. (After the window closes, the non-resurrected target is
        // legitimately freed, so we must NOT read its header post-window.)
        assert_eq!(
            WEAKREF_CB_TYPE_ID_AFTER_GC.load(Ordering::SeqCst),
            crate::TYPE_ID_OBJECT,
            "target header must remain valid during callback's gc.collect() (observed \
             type_id={})",
            WEAKREF_CB_TYPE_ID_AFTER_GC.load(Ordering::SeqCst)
        );

        WEAKREF_CB_CALL_GC.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, weak_bits);
        dec_ref_bits(_py, cb_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

// P6 Scenario C (council #1): a weakref callback that registers a NEW weakref
// against the DYING target re-inserts a registry entry keyed on the about-to-be
// -freed address. Without the post-loop re-drain in `weakref_clear_for_ptr`, that
// orphan would survive the free and, on slot reuse, mis-fire as a wrong-target
// callback. The re-drain nulls the orphan's target so it resolves to None and
// never fires; this test proves the orphan does not survive the target's death.
#[test]
fn weakref_callback_reregistering_on_dying_target_leaves_no_orphan() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        WEAKREF_CB_CALL_COUNT.store(0, Ordering::SeqCst);
        WEAKREF_CB_TARGET_BITS.store(0, Ordering::SeqCst);
        WEAKREF_CB_TARGET_RC_AT_FIRE.store(0, Ordering::SeqCst);
        WEAKREF_CB_CALL_GC.store(0, Ordering::SeqCst);
        WEAKREF_CB_REREGISTER.store(1, Ordering::SeqCst);
        WEAKREF_CB_REREGISTER_NEW_WEAK_OUT.store(0, Ordering::SeqCst);

        let (class_bits, attr_storage) = create_guarded_test_class(_py, b"WeakTargetReReg", &[]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let weak_bits = alloc_test_weakref(_py);
        // The fresh weakref object the callback will register against the dying
        // target. Allocated up front and kept alive across the death so its
        // registry entry would persist as an orphan absent the re-drain.
        let new_weak_bits = alloc_test_weakref(_py);
        WEAKREF_CB_REREGISTER_WEAK_BITS.store(new_weak_bits, Ordering::SeqCst);

        let cb_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_weakref_callback_probe",
                c_api_test_weakref_callback_probe as *const (),
            ),
            1,
        );
        assert!(!cb_ptr.is_null());
        let cb_bits = MoltObject::from_ptr(cb_ptr).bits();

        WEAKREF_CB_TARGET_BITS.store(inst_bits, Ordering::SeqCst);
        let registered = crate::molt_weakref_register(weak_bits, inst_bits, cb_bits);
        assert!(is_truthy(_py, obj_from_bits(registered)));

        // Drop the sole strong ref: the callback fires and registers `new_weak`
        // against the dying target. Registration does NOT incref the target, so the
        // target is still freed at the window close; the post-loop re-drain must
        // remove the orphan entry it created.
        dec_ref_bits(_py, inst_bits);

        assert_eq!(
            WEAKREF_CB_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "original callback must fire exactly once"
        );
        // Registration after the committed-death transition is rejected. The
        // callback error is reported unraisable and the boundary restores a
        // clean raised state.
        let new_weak_registered = WEAKREF_CB_REREGISTER_NEW_WEAK_OUT.load(Ordering::SeqCst);
        assert!(
            obj_from_bits(new_weak_registered).is_none(),
            "callback must not publish a weakref against a committed-dead target"
        );
        assert!(!crate::exception_pending(_py));

        // THE CONTRACT: the orphan must not survive. Resolving the new weakref must
        // return None (its target was nulled by the re-drain), and it must NOT
        // resolve to the dead/freed target nor a reused slot.
        let resolved = crate::molt_weakref_get(new_weak_bits);
        assert!(
            obj_from_bits(resolved).is_none(),
            "weakref registered against a dying target must resolve to None after the \
             target's death (orphan re-drain), got non-None bits=0x{:x}",
            resolved
        );
        if !obj_from_bits(resolved).is_none() {
            dec_ref_bits(_py, resolved);
        }

        // Cleanup the never-registered weakref object.
        WEAKREF_CB_REREGISTER.store(0, Ordering::SeqCst);
        WEAKREF_CB_REREGISTER_WEAK_BITS.store(0, Ordering::SeqCst);
        dec_ref_bits(_py, new_weak_bits);
        dec_ref_bits(_py, weak_bits);
        dec_ref_bits(_py, cb_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
    });
}

#[test]
fn owned_list_builder_drop_runs_remaining_element_finalizer() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        FINALIZER_CALL_COUNT.store(0, Ordering::SeqCst);
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_finalizer_records",
                c_api_test_finalizer_records as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"FinalizerListItem", &[(b"__del__", func_bits)]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");

        let first_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        let second_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        assert!(!obj_from_bits(first_bits).is_none());
        assert!(!obj_from_bits(second_bits).is_none());

        let builder_bits =
            crate::object::builders::molt_list_builder_new(MoltObject::from_int(2).bits());
        assert!(!obj_from_bits(builder_bits).is_none());
        unsafe {
            crate::object::builders::molt_list_builder_append(builder_bits, first_bits);
            crate::object::builders::molt_list_builder_append(builder_bits, second_bits);
        }
        let list_bits =
            unsafe { crate::object::builders::molt_list_builder_finish_owned(builder_bits) };
        assert!(!obj_from_bits(list_bits).is_none());

        let popped_bits = crate::object::ops_list::molt_list_pop(list_bits, none_bits());
        assert_eq!(popped_bits, second_bits);
        dec_ref_bits(_py, popped_bits);
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "discarding the popped owned element must run its finalizer"
        );

        dec_ref_bits(_py, list_bits);
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            2,
            "dropping the list must run the remaining element finalizer"
        );

        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn list_append_retains_finalizer_element_until_clear() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        FINALIZER_CALL_COUNT.store(0, Ordering::SeqCst);
        let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_finalizer_records",
                c_api_test_finalizer_records as *const (),
            ),
            1,
        );
        assert!(!func_ptr.is_null());
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let (class_bits, attr_storage) =
            create_guarded_test_class(_py, b"FinalizerAppendItem", &[(b"__del__", func_bits)]);
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");

        let list_bits =
            crate::object::builders::molt_list_builder_new(MoltObject::from_int(0).bits());
        let list_bits =
            unsafe { crate::object::builders::molt_list_builder_finish_owned(list_bits) };
        assert!(!obj_from_bits(list_bits).is_none());

        let item_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        assert!(!obj_from_bits(item_bits).is_none());
        assert!(obj_from_bits(crate::molt_list_append(list_bits, item_bits)).is_none());
        dec_ref_bits(_py, item_bits);
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            0,
            "list_append must retain an appended heap element beyond the caller temporary"
        );

        assert!(obj_from_bits(crate::object::ops_list::molt_list_clear(list_bits)).is_none());
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "list_clear must release the retained element exactly once"
        );

        dec_ref_bits(_py, list_bits);
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "dropping the empty list must not re-run the element finalizer"
        );

        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, func_bits);
    });
}

#[test]
fn call_bind_constructed_finalizer_element_survives_append_temp_drop_until_clear() {
    let _guard = CApiTestGuard::new();
    crate::with_gil_entry_nopanic!(_py, {
        FINALIZER_CALL_COUNT.store(0, Ordering::SeqCst);
        let del_func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "c_api_test_finalizer_records",
                c_api_test_finalizer_records as *const (),
            ),
            1,
        );
        assert!(!del_func_ptr.is_null());
        let init_func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::provenance::abi::expose_function_address(
                c_api_test_init_stores_tag_and_borrows_self as *const (),
            ),
            2,
        );
        assert!(!init_func_ptr.is_null());
        let del_func_bits = MoltObject::from_ptr(del_func_ptr).bits();
        let init_func_bits = MoltObject::from_ptr(init_func_ptr).bits();
        let (class_bits, attr_storage) = create_guarded_test_class(
            _py,
            b"FinalizerCallBindItem",
            &[(b"__del__", del_func_bits), (b"__init__", init_func_bits)],
        );

        let builder_bits = crate::call::bind::molt_callargs_new(1, 0);
        assert!(!obj_from_bits(builder_bits).is_none());
        let _ =
            unsafe { crate::molt_callargs_push_pos(builder_bits, MoltObject::from_int(1).bits()) };
        let item_bits = crate::molt_call_bind(class_bits, builder_bits);
        assert!(!exception_pending(_py));
        assert!(!obj_from_bits(item_bits).is_none());

        let list_bits =
            crate::object::builders::molt_list_builder_new(MoltObject::from_int(0).bits());
        let list_bits =
            unsafe { crate::object::builders::molt_list_builder_finish_owned(list_bits) };
        assert!(!obj_from_bits(list_bits).is_none());
        assert!(obj_from_bits(crate::molt_list_append(list_bits, item_bits)).is_none());
        dec_ref_bits(_py, item_bits);
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            0,
            "constructed object must remain alive after append retains it and caller drops the call result"
        );

        assert!(obj_from_bits(crate::object::ops_list::molt_list_clear(list_bits)).is_none());
        assert_eq!(
            FINALIZER_CALL_COUNT.load(Ordering::SeqCst),
            1,
            "list_clear must release the constructed element exactly once"
        );

        dec_ref_bits(_py, list_bits);
        for attr_bits in attr_storage.into_iter().step_by(2) {
            dec_ref_bits(_py, attr_bits);
        }
        dec_ref_bits(_py, class_bits);
        dec_ref_bits(_py, init_func_bits);
        dec_ref_bits(_py, del_func_bits);
    });
}

include!("tests/api_runtime.rs");
include!("tests/api_collections.rs");
