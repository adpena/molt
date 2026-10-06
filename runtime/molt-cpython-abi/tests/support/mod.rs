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
    static ABI_TEST_THREAD_STATE: RefCell<Option<AbiTestThreadStateTransaction>> =
        const { RefCell::new(None) };
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
        hooks.native_gc_allocate = native_gc_allocate;
        hooks.native_gc_track = native_gc_track;
        hooks.native_gc_untrack = native_gc_untrack;
        hooks.native_gc_deallocate = native_gc_deallocate;
        hooks.native_gc_is_tracked = native_gc_is_tracked;
        hooks.native_gc_is_finalized = native_gc_is_finalized;
        hooks.native_gc_claim_finalizer = native_gc_claim_finalizer;

        let _installed = unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks) };
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
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        // Use the production root-retirement authority while hooks and the
        // execution attachment still exist. This does not sweep native nodes:
        // fixture-owned leaks must still fail the unchanged ledger assertion.
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

/// Enter the transaction once per test-harness thread. Rust's integration-test
/// harness gives each running test its own native thread, so this keeps the
/// transaction alive for the complete test and drops it before the ABI crate's
/// TLS sentinels.
#[allow(dead_code)]
pub fn prepare_abi_test_thread(hooks: RuntimeHooks) {
    // Strict initialization order is part of the proof: ledger first, ABI TLS
    // inside the constructor second, holder last. TLS teardown reverses that
    // order, so the transaction drains its state while every dependency lives.
    NATIVE_GC_NODES.with(|_| {});
    let transaction = AbiTestThreadStateTransaction::new(hooks);
    ABI_TEST_THREAD_STATE.with(|holder| {
        let mut slot = holder.borrow_mut();
        assert!(
            slot.is_none(),
            "ABI test initialized its TLS transaction twice"
        );
        *slot = Some(transaction);
    });
}

#[allow(dead_code)]
pub fn prepare_runtime_class_abi_test_thread(mut hooks: RuntimeHooks) {
    fake_runtime::wire_class_identity(&mut hooks);
    prepare_abi_test_thread(hooks);
    fake_runtime::prepare_class_bindings();
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
