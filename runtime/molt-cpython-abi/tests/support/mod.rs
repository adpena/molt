use molt_cpython_abi::abi_types::PyObject;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

#[allow(dead_code)]
pub mod fake_complex;
#[allow(dead_code)]
pub mod fake_foreign;
#[allow(dead_code)]
pub mod fake_numbers;
#[allow(dead_code)]
pub mod fake_runtime;
pub mod fake_strings;
#[allow(dead_code)]
pub mod warnings;

// Builtin shells and the hook table are process-owned. A per-thread ledger
// cannot retire their roots while another fixture is executing against them.
static ABI_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Default)]
struct NativeGcState {
    tracked: bool,
    finalized: bool,
}

thread_local! {
    static NATIVE_GC_NODES: RefCell<HashMap<usize, NativeGcState>> =
        RefCell::new(HashMap::new());
}

unsafe extern "C" fn runtime_is_initialized() -> std::os::raw::c_int {
    1
}

unsafe extern "C" fn gil_ensure() -> std::os::raw::c_int {
    0
}

unsafe extern "C" fn gil_leave(_state: std::os::raw::c_int) {}

unsafe extern "C" fn gil_check() -> std::os::raw::c_int {
    1
}

unsafe extern "C" fn thread_state_drop_enter() -> u64 {
    1
}

unsafe extern "C" fn thread_state_drop_leave(_token: u64) {}

unsafe extern "C" fn native_gc_allocate(addr: usize) -> std::os::raw::c_int {
    if addr == 0 {
        return -1;
    }
    NATIVE_GC_NODES.with(|nodes| {
        nodes.borrow_mut().entry(addr).or_default();
    });
    0
}

unsafe extern "C" fn native_gc_track(addr: usize) -> std::os::raw::c_int {
    NATIVE_GC_NODES.with(|nodes| {
        let mut nodes = nodes.borrow_mut();
        let Some(node) = nodes.get_mut(&addr) else {
            return -1;
        };
        node.tracked = true;
        0
    })
}

unsafe extern "C" fn native_gc_untrack(addr: usize) {
    NATIVE_GC_NODES.with(|nodes| {
        if let Some(node) = nodes.borrow_mut().get_mut(&addr) {
            node.tracked = false;
        }
    });
}

unsafe extern "C" fn native_gc_deallocate(addr: usize) {
    NATIVE_GC_NODES.with(|nodes| {
        assert!(
            nodes.borrow_mut().remove(&addr).is_some(),
            "ABI integration test deallocated an unknown native GC identity"
        );
    });
}

unsafe extern "C" fn native_gc_is_tracked(addr: usize) -> std::os::raw::c_int {
    NATIVE_GC_NODES.with(|nodes| {
        std::os::raw::c_int::from(nodes.borrow().get(&addr).is_some_and(|node| node.tracked))
    })
}

unsafe extern "C" fn native_gc_is_finalized(addr: usize) -> std::os::raw::c_int {
    NATIVE_GC_NODES.with(|nodes| {
        std::os::raw::c_int::from(nodes.borrow().get(&addr).is_some_and(|node| node.finalized))
    })
}

unsafe extern "C" fn native_gc_claim_finalizer(addr: usize) -> std::os::raw::c_int {
    NATIVE_GC_NODES.with(|nodes| {
        let mut nodes = nodes.borrow_mut();
        let Some(node) = nodes.get_mut(&addr) else {
            return -1;
        };
        if node.finalized {
            0
        } else {
            node.finalized = true;
            1
        }
    })
}

// One field inventory drives both exhaustiveness and comparison. Adding a hook
// must choose its comparison class; no production layout or registry is added.
macro_rules! assert_same_runtime_hooks {
    ($requested:expr, $installed:expr;
     scalar { $($scalar:ident),* $(,)? }
     required { $($required:ident),* $(,)? }
     optional { $($optional:ident),* $(,)? }) => {{
        let RuntimeHooks {
            $($scalar,)*
            $($required,)*
            $($optional,)*
        } = $requested;
        let installed = $installed;
        $(assert_eq!($scalar, installed.$scalar,
            "ABI integration test requested incompatible RuntimeHooks.{}", stringify!($scalar));)*
        $(assert!(std::ptr::fn_addr_eq($required, installed.$required),
            "ABI integration test requested incompatible RuntimeHooks.{}", stringify!($required));)*
        $(assert!(match ($optional, installed.$optional) {
            (None, None) => true,
            (Some(requested), Some(installed)) => std::ptr::fn_addr_eq(requested, installed),
            _ => false,
        }, "ABI integration test requested incompatible RuntimeHooks.{}", stringify!($optional));)*
    }};
}

fn assert_installed_runtime_hooks(requested: RuntimeHooks) {
    let installed = molt_cpython_abi::hooks::hooks()
        .expect("ABI integration test RuntimeHooks registration failed before attachment");
    assert_same_runtime_hooks!(requested, installed;
        scalar {
            abi_magic,
            abi_version,
            struct_size,
        }
        required {
            gil_ensure,
            gil_leave,
            gil_release,
            gil_restore,
            gil_check,
            runtime_is_initialized,
            thread_state_drop_enter,
            thread_state_drop_leave,
            attached_runtime_context,
            pending_call_error,
            alloc_bytes,
            alloc_bytearray,
            float_payload,
            int_from_i64,
            int_from_u64,
            int_from_bytes,
            int_to_bytes,
            int_num_bits,
            int_max_str_digits,
            alloc_list,
            alloc_list_presized,
            list_append,
            list_len,
            list_item,
            list_set,
            list_insert,
            list_sort,
            list_reverse,
            list_set_slice,
            alloc_dict,
            mappingproxy_new,
            dict_resolve,
            dict_mutate,
            dict_get,
            dict_pop,
            dict_len,
            dict_next,
            str_data,
            unicode_new,
            unicode_commit,
            unicode_encode,
            bytes_data,
            bytearray_data,
            bytearray_resize,
            buffer_supports,
            buffer_acquire,
            buffer_release,
            object_get_attr,
            object_set_attr,
            method_part,
            descriptor_protocol,
            descriptor_get,
            descriptor_set,
            object_format,
            object_str,
            object_repr,
            object_is_true,
            object_length,
            object_get_item,
            object_supports_subscript,
            object_set_item,
            object_get_iter,
            iter_check,
            iter_next,
            sys_get_object_borrowed,
            eval_get_builtins_borrowed,
            object_hash,
            type_dict_borrowed,
            type_metadata,
            type_lookup_borrowed,
            builtin_slot_owner,
            inc_ref,
            dec_ref,
            try_mark_abi_view,
            alloc_module,
            alloc_extension_module,
            module_get_dict_borrowed,
            import_add_module_borrowed,
            module_set_attr,
            module_capi_register,
            module_capi_get_state,
            module_capi_get_def,
            module_state_add,
            module_state_find,
            module_state_remove,
            module_exec_begin,
            import_module,
            initialize_extension,
            exception_pending,
            pending_exception_class,
            object_richcompare,
            object_richcompare_builtin,
            number_binary_op,
            number_unary_op,
            number_power,
            target_python_minor,
            dict_op,
            set_op,
            set_new,
            set_size,
            set_contains,
            set_add,
            set_discard,
            object_dir,
            object_call,
            object_vectorcall,
            object_is_callable,
            foreign_new,
            int_from_digits,
            int_from_f64_trunc,
            int_sign,
            complex_parts,
            complex_from_doubles,
            report_unraisable,
            exception_set_field,
            exception_get_field,
            exception_layout_kind,
            exception_snapshot,
            exception_commit_snapshot,
            type_is_subtype,
            take_pending_exception,
            clear_pending_exception,
            with_preserved_pending_exception,
            handled_exception_get,
            handled_exception_set,
            managed_gc_control,
            managed_gc_traverse,
            managed_gc_clear,
            native_gc_track,
            native_gc_untrack,
            native_gc_deallocate,
            native_gc_is_tracked,
            native_gc_is_finalized,
            native_gc_claim_finalizer,
            gc_collect,
            gc_enable,
            gc_disable,
            gc_is_enabled,
            check_signals,
            set_interrupt,
            interrupt_occurred,
            notify_pending_calls,
            sequence_check,
            sequence_item,
            object_length_hint,
            tuple_uses_length_hint,
            exception_group_admit,
            object_bytes,
            memoryview_new,
            memoryview_release,
            memoryview_from_buffer,
            memoryview_snapshot,
            private_c_heap_contains,
            slice_new,
            slice_item,
            object_contains,
            context_type_admit,
            context_new,
            context_copy_current,
            context_copy,
            context_enter,
            context_exit,
            context_var_new,
            context_var_get,
            context_var_set,
            context_var_reset,
        }
        optional {
            alloc_str,
            numeric_identity_new,
            alloc_tuple,
            tuple_set,
            tuple_len,
            tuple_item,
            method_new,
            classify_heap,
            ref_count,
            register_c_function,
            object_classinfo_match,
            runtime_class_borrowed,
            native_gc_allocate,
        }
    );
}

/// Own the real CPython ABI runtime-execution boundary for one integration
/// test. Every test binary supplies its normal hook table; this transaction
/// adds only lifecycle/GC custody, installs the table once for that binary,
/// and publishes a thread-local `PyThreadState` through the production path.
#[must_use = "the ABI integration transaction must live for the whole C-API test"]
pub struct AbiTestThreadStateTransaction {
    _not_send: PhantomData<Rc<()>>,
    _exclusive: MutexGuard<'static, ()>,
}

impl AbiTestThreadStateTransaction {
    pub fn new(mut hooks: RuntimeHooks) -> Self {
        let exclusive = ABI_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        hooks.runtime_is_initialized = runtime_is_initialized;
        hooks.gil_ensure = gil_ensure;
        hooks.gil_leave = gil_leave;
        hooks.gil_check = gil_check;
        hooks.thread_state_drop_enter = thread_state_drop_enter;
        hooks.thread_state_drop_leave = thread_state_drop_leave;
        hooks.native_gc_allocate = Some(native_gc_allocate);
        hooks.native_gc_track = native_gc_track;
        hooks.native_gc_untrack = native_gc_untrack;
        hooks.native_gc_deallocate = native_gc_deallocate;
        hooks.native_gc_is_tracked = native_gc_is_tracked;
        hooks.native_gc_is_finalized = native_gc_is_finalized;
        hooks.native_gc_claim_finalizer = native_gc_claim_finalizer;

        // Registration is immutable. Every later request must name the same
        // normalized table before it can prepare or attach execution state.
        let _installed = unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks) };
        assert_installed_runtime_hooks(hooks);
        assert_ne!(
            unsafe { (molt_cpython_abi::hooks::hooks_or_stubs().runtime_is_initialized)() },
            0,
            "ABI integration test binary installed hooks without runtime lifecycle custody"
        );
        molt_cpython_abi::api::object::prepare_runtime_thread_state_lifetime();
        molt_cpython_abi::api::object::arm_runtime_thread_state_lifetime();
        assert!(
            molt_cpython_abi::api::object::attach_runtime_execution_thread(),
            "ABI integration test inherited an attached PyThreadState"
        );
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        unsafe { molt_cpython_abi::abi_types::prepare_builtin_static_type_runtime_state() };
        Self {
            _not_send: PhantomData,
            _exclusive: exclusive,
        }
    }
}

impl Drop for AbiTestThreadStateTransaction {
    fn drop(&mut self) {
        let primary_failure = std::thread::panicking();
        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
            // Retire roots while hooks and the execution attachment still exist.
            // This does not sweep fixture-owned nodes or forgive leaked edges.
            unsafe { molt_cpython_abi::abi_types::retire_builtin_static_type_runtime_state() };
            molt_cpython_abi::api::object::detach_runtime_execution_thread();
            molt_cpython_abi::api::object::clear_runtime_execution_thread_state();
            NATIVE_GC_NODES.with(|nodes| {
                let nodes = nodes.borrow();
                assert!(
                    nodes.is_empty(),
                    "ABI integration test leaked native GC identities: {nodes:?}"
                );
            });
        }));
        if let Err(failure) = cleanup {
            if !primary_failure {
                std::panic::resume_unwind(failure);
            }
            // Keep cleanup evidence even when the original test silences its
            // panic hook. The original failure continues unwinding unchanged.
            use std::io::Write;
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap_or("non-string cleanup panic");
            let _ = writeln!(std::io::stderr(), "ABI test cleanup also failed: {message}");
        }
    }
}

/// A test-owned static type shell whose published roots cannot outlive its
/// physical storage. PyObject_Free also revokes its production type receipt.
#[allow(dead_code)]
pub struct StaticType(*mut molt_cpython_abi::abi_types::PyTypeObject);

#[allow(dead_code)]
impl StaticType {
    pub fn new() -> Self {
        unsafe {
            let pointer = molt_cpython_abi::api::memory::PyObject_Malloc(std::mem::size_of::<
                molt_cpython_abi::abi_types::PyTypeObject,
            >())
            .cast::<molt_cpython_abi::abi_types::PyTypeObject>();
            assert!(!pointer.is_null());
            pointer.write(std::mem::zeroed());
            (*pointer).ob_base.ob_base.ob_refcnt = 1;
            Self(pointer)
        }
    }
    pub fn as_ptr(&self) -> *mut molt_cpython_abi::abi_types::PyTypeObject {
        self.0
    }
}
impl std::ops::Deref for StaticType {
    type Target = molt_cpython_abi::abi_types::PyTypeObject;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.0 }
    }
}
impl std::ops::DerefMut for StaticType {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.0 }
    }
}
impl Drop for StaticType {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
            for field in [
                &raw mut (*self.0).tp_dict,
                &raw mut (*self.0).tp_mro,
                &raw mut (*self.0).tp_bases,
                &raw mut (*self.0).tp_cache,
            ] {
                molt_cpython_abi::api::refcount::Py_CLEAR(field);
            }
            molt_cpython_abi::api::memory::PyObject_Free(self.0.cast());
        });
    }
}

#[allow(dead_code)]
pub fn stub_runtime_hooks() -> RuntimeHooks {
    molt_cpython_abi::hooks::STUB_HOOKS
}

/// Own the transaction in the test's lexical scope, before its fixture values.
/// A cleanup assertion then belongs to that test instead of aborting the entire
/// integration binary from a thread-local destructor.
#[allow(dead_code)]
pub fn enter_abi_test(hooks: RuntimeHooks) -> AbiTestThreadStateTransaction {
    NATIVE_GC_NODES.with(|_| {});
    AbiTestThreadStateTransaction::new(hooks)
}

#[allow(dead_code)]
pub fn enter_runtime_class_abi_test(mut hooks: RuntimeHooks) -> AbiTestThreadStateTransaction {
    fake_runtime::wire_class_identity(&mut hooks);
    let transaction = enter_abi_test(hooks);
    fake_runtime::prepare_class_bindings();
    transaction
}

/// Consume the exact pending exception and render its normalized instance.
/// Tests use the public ownership API rather than reviving the deleted
/// text-only error side channel.
#[allow(dead_code)]
pub fn take_current_error_text() -> Option<String> {
    let error = molt_cpython_abi::api::errors::take_current_error()?;
    if error.value.is_null() {
        return None;
    }
    unsafe {
        let rendered = molt_cpython_abi::api::typeobj::PyObject_Str(error.value);
        if rendered.is_null() {
            molt_cpython_abi::api::errors::PyErr_Clear();
            return None;
        }
        let mut len = 0;
        let data = molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(rendered, &raw mut len);
        let text = (!data.is_null() && len >= 0).then(|| {
            String::from_utf8_lossy(std::slice::from_raw_parts(data.cast::<u8>(), len as usize))
                .into_owned()
        });
        molt_cpython_abi::api::refcount::Py_DECREF(rendered.cast::<PyObject>());
        molt_cpython_abi::api::errors::PyErr_Clear();
        text
    }
}
