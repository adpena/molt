use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use super::{
    runtime_reset_for_init, runtime_teardown, runtime_teardown_for_process_exit, touch_tls_guard,
};

use crate::IoPoller;
use crate::ProcessTaskState;
use crate::concurrency::gil::{gil_held, hold_runtime_gil, release_runtime_gil};
use crate::object::utf8_cache::{Utf8CacheStore, Utf8CountCacheStore, build_utf8_count_cache};
use crate::{
    AsyncHangProbe, BuiltinClasses, CancelTokenEntry, GilGuard, HashSecret, InternedNames,
    MethodCache, MoltObject, MoltScheduler, PtrSlot, PyToken, SleepQueue, default_cancel_tokens,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{ThreadPool, ThreadTaskState, sleep_worker};

#[cfg(target_arch = "wasm32")]
unsafe extern "C" {
    fn __wasm_call_ctors();
}

#[cfg(target_arch = "wasm32")]
static WASM_CTORS_DONE: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "wasm32")]
fn ensure_wasm_ctors() {
    if WASM_CTORS_DONE.load(AtomicOrdering::Acquire) {
        return;
    }
    // Mark as in-progress BEFORE calling ctors to prevent recursive entry.
    WASM_CTORS_DONE.store(true, AtomicOrdering::Release);
    unsafe {
        __wasm_call_ctors();
    }
}

#[cfg(not(target_arch = "wasm32"))]
static DEBUG_SIGTRAP_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(not(target_arch = "wasm32"))]
fn debug_sigtrap_backtrace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_DEBUG_SIGTRAP_BACKTRACE")
                .ok()
                .as_deref(),
            Some("1")
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
unsafe extern "C" fn debug_sigtrap_handler(sig: i32) {
    unsafe {
        let msg = b"molt debug: SIGTRAP backtrace\n";
        let _ = libc::write(2, msg.as_ptr() as *const _, msg.len());
        let mut addrs = [std::ptr::null_mut(); 128];
        let count = libc::backtrace(addrs.as_mut_ptr(), addrs.len() as i32);
        if count > 0 {
            libc::backtrace_symbols_fd(addrs.as_ptr(), count, 2);
        }
        libc::_exit(128 + sig);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn ensure_debug_sigtrap_handler() {
    if debug_sigtrap_backtrace_enabled()
        && !DEBUG_SIGTRAP_INSTALLED.swap(true, AtomicOrdering::Relaxed)
    {
        unsafe {
            libc::signal(libc::SIGTRAP, debug_sigtrap_handler as *const () as usize);
        }
    }
}

pub(crate) struct SpecialCache {
    pub(crate) open_default_mode: AtomicU64,
    pub(crate) molt_missing: AtomicU64,
    pub(crate) molt_not_implemented: AtomicU64,
    pub(crate) molt_ellipsis: AtomicU64,
    pub(crate) awaitable_await: AtomicU64,
    pub(crate) function_code_descriptor: AtomicU64,
    pub(crate) function_globals_descriptor: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct AsyncGenLocalsEntry {
    pub(crate) names: Vec<u64>,
    pub(crate) offsets: Vec<usize>,
}

#[derive(Clone)]
pub(crate) struct GenLocalsEntry {
    pub(crate) names: Vec<u64>,
    pub(crate) offsets: Vec<usize>,
}

#[derive(Clone)]
pub(crate) struct WeakRefEntry {
    pub(crate) target: PtrSlot,
    pub(crate) callback_bits: u64,
}

#[derive(Clone)]
pub(crate) struct WeakKeyDictEntry {
    pub(crate) key_ref_bits: u64,
    pub(crate) value_bits: u64,
}

#[derive(Clone)]
pub(crate) struct WeakValueDictEntry {
    pub(crate) key_bits: u64,
    pub(crate) value_ref_bits: u64,
}

#[derive(Clone)]
pub(crate) struct WeakSetEntry {
    pub(crate) item_ref_bits: u64,
}

#[derive(Clone)]
pub(crate) struct AtexitCallbackEntry {
    pub(crate) kind: AtexitCallbackKind,
    pub(crate) func_bits: u64,
    pub(crate) args_bits: u64,
    pub(crate) kwargs_bits: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtexitCallbackKind {
    Python,
    WeakrefFinalizerRunner,
}

pub(crate) struct WeakRefRegistry {
    pub(crate) by_ref: HashMap<PtrSlot, WeakRefEntry>,
    pub(crate) by_target: HashMap<PtrSlot, Vec<PtrSlot>>,
}

impl WeakRefRegistry {
    pub(crate) fn new() -> Self {
        Self {
            by_ref: HashMap::new(),
            by_target: HashMap::new(),
        }
    }
}

pub(crate) struct AsyncGenHooks {
    pub(crate) firstiter: u64,
    pub(crate) finalizer: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PythonVersionInfo {
    pub(crate) major: i64,
    pub(crate) minor: i64,
    pub(crate) micro: i64,
    pub(crate) releaselevel: String,
    pub(crate) serial: i64,
}

impl SpecialCache {
    fn new() -> Self {
        Self {
            open_default_mode: AtomicU64::new(0),
            molt_missing: AtomicU64::new(0),
            molt_not_implemented: AtomicU64::new(0),
            molt_ellipsis: AtomicU64::new(0),
            awaitable_await: AtomicU64::new(0),
            function_code_descriptor: AtomicU64::new(0),
            function_globals_descriptor: AtomicU64::new(0),
        }
    }
}

pub(crate) struct RuntimeState {
    pub(crate) builtin_classes: std::sync::atomic::AtomicPtr<BuiltinClasses>,
    pub(crate) interned: InternedNames,
    pub(crate) method_cache: MethodCache,
    pub(crate) special_cache: SpecialCache,
    pub(crate) last_exception: Mutex<Option<PtrSlot>>,
    pub(crate) last_exception_pending: AtomicBool,
    pub(crate) module_cache: Mutex<HashMap<String, u64>>,
    pub(crate) intrinsic_registry_module: AtomicPtr<u8>,
    pub(crate) exception_type_cache: Mutex<HashMap<String, u64>>,
    pub(crate) argv: Mutex<Vec<Vec<u8>>>,
    pub(crate) sys_version_info: Mutex<Option<PythonVersionInfo>>,
    pub(crate) sys_version: Mutex<Option<String>>,
    pub(crate) hash_secret: OnceLock<HashSecret>,
    pub(crate) profile_enabled: OnceLock<bool>,
    pub(crate) utf8_index_cache: Mutex<Utf8CacheStore>,
    pub(crate) utf8_count_cache: Vec<Mutex<Utf8CountCacheStore>>,
    pub(crate) string_count_cache_hit: AtomicU64,
    pub(crate) string_count_cache_miss: AtomicU64,
    pub(crate) scheduler_started: AtomicBool,
    pub(crate) scheduler: OnceLock<MoltScheduler>,
    pub(crate) sleep_queue_started: AtomicBool,
    pub(crate) sleep_queue: OnceLock<Arc<SleepQueue>>,
    pub(crate) io_poller_started: AtomicBool,
    pub(crate) io_poller: OnceLock<Arc<IoPoller>>,
    pub(crate) capabilities: OnceLock<HashSet<String>>,
    pub(crate) trusted: OnceLock<bool>,
    pub(crate) async_hang_probe: OnceLock<Option<AsyncHangProbe>>,
    pub(crate) cancel_tokens: Mutex<HashMap<u64, CancelTokenEntry>>,
    pub(crate) task_tokens: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) task_tokens_by_id: Mutex<HashMap<u64, HashSet<PtrSlot>>>,
    pub(crate) task_cancel_messages: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) asyncio_running_loops: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_event_loops: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_event_loop_policy: Mutex<u64>,
    pub(crate) asyncio_tasks: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_current_tasks: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_event_waiters: Mutex<HashMap<u64, Vec<u64>>>,
    pub(crate) task_exception_handler_stacks: Mutex<HashMap<PtrSlot, Vec<usize>>>,
    pub(crate) task_exception_stacks: Mutex<HashMap<PtrSlot, Vec<u64>>>,
    pub(crate) task_exception_depths: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_exception_baselines: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_last_exceptions: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) task_last_exception_pending: AtomicBool,
    pub(crate) dict_subclass_storage: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) await_waiters: Mutex<HashMap<PtrSlot, Vec<PtrSlot>>>,
    pub(crate) task_waiting_on: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) asyncgen_hooks: Mutex<AsyncGenHooks>,
    pub(crate) asyncgen_locals: Mutex<HashMap<u64, AsyncGenLocalsEntry>>,
    pub(crate) gen_locals: Mutex<HashMap<u64, GenLocalsEntry>>,
    pub(crate) weakrefs: Mutex<WeakRefRegistry>,
    pub(crate) weakref_finalizers: Mutex<Vec<u64>>,
    pub(crate) weakkeydicts: Mutex<HashMap<PtrSlot, Vec<WeakKeyDictEntry>>>,
    pub(crate) weakvaluedicts: Mutex<HashMap<PtrSlot, Vec<WeakValueDictEntry>>>,
    pub(crate) weaksets: Mutex<HashMap<PtrSlot, Vec<WeakSetEntry>>>,
    pub(crate) atexit_callbacks: Mutex<Vec<AtexitCallbackEntry>>,
    pub(crate) atexit_weakref_runner_registered: AtomicBool,
    pub(crate) abc_invalidation_counter: AtomicU64,
    pub(crate) asyncgen_registry: Mutex<HashSet<PtrSlot>>,
    pub(crate) fn_ptr_code: Mutex<HashMap<u64, u64>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_pool_started: AtomicBool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_pool: OnceLock<ThreadPool>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_tasks: Mutex<HashMap<PtrSlot, Arc<ThreadTaskState>>>,
    pub(crate) process_tasks: Mutex<HashMap<PtrSlot, Arc<ProcessTaskState>>>,
    pub(crate) code_slots: OnceLock<Vec<AtomicU64>>,
    pub(crate) start_time: OnceLock<Instant>,
    /// VFS state lazily initialized from environment variables on first access.
    pub(crate) vfs_state: OnceLock<Option<crate::vfs::VfsState>>,
}

impl RuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            builtin_classes: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            interned: InternedNames::new(),
            method_cache: MethodCache::new(),
            special_cache: SpecialCache::new(),
            last_exception: Mutex::new(None),
            last_exception_pending: AtomicBool::new(false),
            module_cache: Mutex::new(HashMap::new()),
            intrinsic_registry_module: AtomicPtr::new(std::ptr::null_mut()),
            exception_type_cache: Mutex::new(HashMap::new()),
            argv: Mutex::new(Vec::new()),
            sys_version_info: Mutex::new(None),
            sys_version: Mutex::new(None),
            hash_secret: OnceLock::new(),
            profile_enabled: OnceLock::new(),
            utf8_index_cache: Mutex::new(Utf8CacheStore::new()),
            utf8_count_cache: build_utf8_count_cache(),
            string_count_cache_hit: AtomicU64::new(0),
            string_count_cache_miss: AtomicU64::new(0),
            scheduler_started: AtomicBool::new(false),
            scheduler: OnceLock::new(),
            sleep_queue_started: AtomicBool::new(false),
            sleep_queue: OnceLock::new(),
            io_poller_started: AtomicBool::new(false),
            io_poller: OnceLock::new(),
            capabilities: OnceLock::new(),
            trusted: OnceLock::new(),
            async_hang_probe: OnceLock::new(),
            cancel_tokens: Mutex::new(default_cancel_tokens()),
            task_tokens: Mutex::new(HashMap::new()),
            task_tokens_by_id: Mutex::new(HashMap::new()),
            task_cancel_messages: Mutex::new(HashMap::new()),
            asyncio_running_loops: Mutex::new(HashMap::new()),
            asyncio_event_loops: Mutex::new(HashMap::new()),
            asyncio_event_loop_policy: Mutex::new(MoltObject::none().bits()),
            asyncio_tasks: Mutex::new(HashMap::new()),
            asyncio_current_tasks: Mutex::new(HashMap::new()),
            asyncio_event_waiters: Mutex::new(HashMap::new()),
            task_exception_handler_stacks: Mutex::new(HashMap::new()),
            task_exception_stacks: Mutex::new(HashMap::new()),
            task_exception_depths: Mutex::new(HashMap::new()),
            task_exception_baselines: Mutex::new(HashMap::new()),
            task_last_exceptions: Mutex::new(HashMap::new()),
            task_last_exception_pending: AtomicBool::new(false),
            dict_subclass_storage: Mutex::new(HashMap::new()),
            await_waiters: Mutex::new(HashMap::new()),
            task_waiting_on: Mutex::new(HashMap::new()),
            asyncgen_hooks: Mutex::new(AsyncGenHooks {
                firstiter: MoltObject::none().bits(),
                finalizer: MoltObject::none().bits(),
            }),
            asyncgen_locals: Mutex::new(HashMap::new()),
            gen_locals: Mutex::new(HashMap::new()),
            weakrefs: Mutex::new(WeakRefRegistry::new()),
            weakref_finalizers: Mutex::new(Vec::new()),
            weakkeydicts: Mutex::new(HashMap::new()),
            weakvaluedicts: Mutex::new(HashMap::new()),
            weaksets: Mutex::new(HashMap::new()),
            atexit_callbacks: Mutex::new(Vec::new()),
            atexit_weakref_runner_registered: AtomicBool::new(false),
            abc_invalidation_counter: AtomicU64::new(0),
            asyncgen_registry: Mutex::new(HashSet::new()),
            fn_ptr_code: Mutex::new(HashMap::new()),
            #[cfg(not(target_arch = "wasm32"))]
            thread_pool_started: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            thread_pool: OnceLock::new(),
            #[cfg(not(target_arch = "wasm32"))]
            thread_tasks: Mutex::new(HashMap::new()),
            process_tasks: Mutex::new(HashMap::new()),
            code_slots: OnceLock::new(),
            start_time: OnceLock::new(),
            vfs_state: OnceLock::new(),
        }
    }

    pub(crate) fn scheduler(&self) -> &MoltScheduler {
        self.scheduler_started.store(true, AtomicOrdering::SeqCst);
        self.scheduler.get_or_init(MoltScheduler::new)
    }

    pub(crate) fn sleep_queue(&self) -> &Arc<SleepQueue> {
        self.sleep_queue.get_or_init(|| {
            self.sleep_queue_started.store(true, AtomicOrdering::SeqCst);
            let queue = Arc::new(SleepQueue::new());
            #[cfg(not(target_arch = "wasm32"))]
            {
                let worker_queue = Arc::clone(&queue);
                let handle = thread::spawn(move || sleep_worker(worker_queue));
                queue.set_worker_handle(handle);
            }
            queue
        })
    }

    pub(crate) fn io_poller(&self) -> &Arc<IoPoller> {
        self.io_poller.get_or_init(|| {
            self.io_poller_started.store(true, AtomicOrdering::SeqCst);
            let poller = Arc::new(IoPoller::new());
            #[cfg(not(target_arch = "wasm32"))]
            poller.start_worker();
            poller
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn thread_pool(&self) -> &ThreadPool {
        self.thread_pool.get_or_init(|| {
            self.thread_pool_started.store(true, AtomicOrdering::SeqCst);
            ThreadPool::new()
        })
    }

    /// Returns a reference to the VFS state, lazily initialized from
    /// environment variables on first access.  Returns `None` when
    /// `MOLT_VFS_BUNDLE` is not set in the environment.
    pub(crate) fn get_vfs(&self) -> Option<&crate::vfs::VfsState> {
        self.vfs_state.get_or_init(crate::vfs::load_vfs).as_ref()
    }
}

pub(crate) fn runtime_state_lock() -> &'static Mutex<()> {
    RUNTIME_STATE_LOCK.get_or_init(|| Mutex::new(()))
}

#[allow(dead_code)]
fn runtime_state_ptr() -> Option<*mut RuntimeState> {
    let ptr = RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst);
    if ptr.is_null() { None } else { Some(ptr) }
}

pub(crate) fn runtime_state_for_gil() -> Option<&'static RuntimeState> {
    if let Some(state) = runtime_state_tls() {
        return Some(state);
    }
    let ptr = RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

pub(crate) fn runtime_state(_py: &PyToken<'_>) -> &'static RuntimeState {
    let _ = _py;
    touch_tls_guard();
    if let Some(state) = runtime_state_tls() {
        return state;
    }
    if let Some(ptr) = runtime_state_ptr() {
        unsafe { &*ptr }
    } else {
        let _ = molt_runtime_init();
        // After `molt_runtime_shutdown`, `molt_runtime_init` refuses to
        // re-allocate (returns 0) so the pointer stays null.  Return a
        // leaked sentinel to avoid panicking during process-exit teardown.
        if let Some(ptr) = runtime_state_ptr() {
            unsafe { &*ptr }
        } else {
            post_shutdown_sentinel()
        }
    }
}

/// Returns a leaked, empty `RuntimeState` for use by straggler code that
/// calls `runtime_state()` after `molt_runtime_shutdown` has completed.
/// Allocated once and never freed (the OS reclaims it at process exit).
fn post_shutdown_sentinel() -> &'static RuntimeState {
    static SENTINEL: OnceLock<&'static RuntimeState> = OnceLock::new();
    SENTINEL.get_or_init(|| Box::leak(Box::new(RuntimeState::new())))
}

// ---------------------------------------------------------------------------
// GIL vtable shims — bridge core crate's function-pointer GIL to the real
// mutex-based GIL in this crate.
// ---------------------------------------------------------------------------

extern "C" fn __core_gil_acquire() -> u64 {
    let guard = GilGuard::new();
    // Leak the guard — it will be released by __core_gil_release
    Box::into_raw(Box::new(guard)) as u64
}

extern "C" fn __core_gil_release(token: u64) {
    if token != 0 {
        unsafe {
            drop(Box::from_raw(token as *mut GilGuard));
        }
    }
}

extern "C" fn __core_gil_is_held() -> bool {
    gil_held()
}

static CORE_GIL_VT: molt_runtime_core::GilVtable = molt_runtime_core::GilVtable {
    acquire: __core_gil_acquire,
    release: __core_gil_release,
    is_held: __core_gil_is_held,
};

#[inline]
fn trace_runtime_init_enabled() -> bool {
    matches!(
        std::env::var("MOLT_TRACE_RUNTIME_INIT").ok().as_deref(),
        Some("1")
    )
}

#[inline]
fn trace_runtime_init(stage: &str) {
    if trace_runtime_init_enabled() {
        eprintln!("[molt runtime_init] {stage}");
    }
}

/// Clean executable process exit.
///
/// Runs Python-level process-exit finalization once, then calls `_exit` so C
/// global destructors and Rust/TLS destructors cannot race runtime allocator
/// state. Explicit embedding teardown remains `molt_runtime_shutdown()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_exit(code_bits: u64) -> u64 {
    let code = match code_bits {
        0 => 0,
        1 => 1,
        other if other <= i32::MAX as u64 => other as i32,
        _ => 1,
    };
    if !PROCESS_EXIT_FINALIZED.swap(true, AtomicOrdering::SeqCst) {
        let gil = GilGuard::new();
        {
            let _guard = runtime_state_lock().lock().unwrap();
            let ptr = RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst);
            if !ptr.is_null() {
                let state = unsafe { &*ptr };
                let py = gil.token();
                runtime_teardown_for_process_exit(&py, state);
            }
        }
        drop(gil);
    }
    {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }
    unsafe { libc::_exit(code) }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init() -> u64 {
    #[cfg(target_arch = "wasm32")]
    ensure_wasm_ctors();
    trace_runtime_init("enter");
    touch_tls_guard();
    #[cfg(not(target_arch = "wasm32"))]
    ensure_debug_sigtrap_handler();
    if !RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst).is_null() {
        trace_runtime_init("already_initialized");
        return 1;
    }
    if RUNTIME_SHUTDOWN_COMPLETE.load(AtomicOrdering::SeqCst) {
        trace_runtime_init("shutdown_complete");
        return 0;
    }
    let gil = GilGuard::new();
    let _guard = runtime_state_lock().lock().unwrap();
    if !RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst).is_null() {
        trace_runtime_init("already_initialized");
        return 1;
    }
    // After `molt_runtime_shutdown` has run, the process is exiting.
    // During exit, Rust static/TLS destructors or C `atexit` handlers may
    // indirectly call `runtime_state()` which auto-calls `molt_runtime_init`.
    // Re-allocating a RuntimeState at this point is futile (the new state is
    // immediately torn down again) and dangerous: the second teardown's
    // `drop(Box::from_raw)` frees memory while mimalloc's global allocator
    // may already be partially destroyed, causing a use-after-free segfault
    // (exit code 245 on macOS / SIGSEGV on Linux).
    if RUNTIME_SHUTDOWN_COMPLETE.load(AtomicOrdering::SeqCst) {
        trace_runtime_init("shutdown_complete");
        return 0;
    }
    let state = Box::new(RuntimeState::new());
    let ptr = Box::into_raw(state);
    RUNTIME_STATE_PTR.store(ptr, AtomicOrdering::SeqCst);
    let state_ref = unsafe { &*ptr };
    trace_runtime_init("state_allocated");
    {
        let py = gil.token();
        runtime_reset_for_init(&py, state_ref);
    }
    trace_runtime_init("runtime_reset_for_init");
    // Register synthetic _intrinsics module so stdlib .py files can import it
    {
        let py = crate::concurrency::GilGuard::new();
        let tok = py.token();
        crate::intrinsics::registry::register_intrinsics_module(&tok);
    }
    trace_runtime_init("intrinsics_registered");
    hold_runtime_gil(gil);

    // Initialize the serial crate vtable so all bridge functions dispatch
    // through a single struct instead of 58 individual extern "C" symbols.
    #[cfg(feature = "stdlib_serial")]
    molt_runtime_serial::bridge::init_vtable();
    trace_runtime_init("serial_vtable");

    #[cfg(feature = "stdlib_itertools")]
    molt_runtime_itertools::bridge::init_vtable();
    trace_runtime_init("itertools_vtable");

    // Initialize the core GIL vtable so extracted crates can acquire the GIL
    // via molt-runtime-core without depending on molt-runtime.
    molt_runtime_core::set_gil_vtable(&CORE_GIL_VT);
    trace_runtime_init("core_gil_vtable");

    // Initialize resource limits, audit sink, and IO mode from environment
    // variables set by the capability manifest.
    crate::object::ops_sys::molt_runtime_init_resources();
    trace_runtime_init("resources");
    crate::object::ops_sys::molt_runtime_init_audit();
    trace_runtime_init("audit");
    crate::object::ops_sys::molt_runtime_init_io_mode();
    trace_runtime_init("io_mode");

    // SECURITY: Eagerly load capabilities and trusted flag from environment
    // BEFORE any user code runs.  Lazy loading (OnceLock::get_or_init) would
    // allow a program to write MOLT_TRUSTED=1 to the process environment
    // before the first capability check, escalating privileges.
    {
        let py = crate::concurrency::GilGuard::new();
        let tok = py.token();
        let _ = crate::is_trusted(&tok);
        let _ = crate::has_capability(&tok, "_init");
    }
    trace_runtime_init("capabilities");

    trace_runtime_init("ok");
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_ensure_gil() {
    touch_tls_guard();
    if gil_held() {
        return;
    }
    hold_runtime_gil(GilGuard::new());
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_shutdown() -> u64 {
    let _guard = runtime_state_lock().lock().unwrap();
    let ptr = RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst);
    if ptr.is_null() {
        return 0;
    }
    let state = unsafe { &*ptr };
    let gil = GilGuard::new();
    let py = gil.token();
    runtime_teardown(&py, state);
    release_runtime_gil();
    // Clear the TLS cache BEFORE nulling the global pointer and freeing the
    // state.  Without this, `TLS_RUNTIME_STATE` holds a dangling pointer to
    // the about-to-be-freed `RuntimeState`.  During process exit, Rust's TLS
    // destructors (`ThreadLocalGuard::drop`) may still run and indirectly call
    // `runtime_state()` — which would dereference the dangling pointer,
    // causing a use-after-free crash (exit code 245 on macOS).
    clear_thread_runtime_state();
    RUNTIME_STATE_PTR.store(std::ptr::null_mut(), AtomicOrdering::SeqCst);
    // Mark shutdown as complete BEFORE freeing the state.  This prevents
    // `molt_runtime_init` from re-allocating a RuntimeState during process
    // exit (triggered by atexit handlers / TLS destructors calling
    // `runtime_state()` which has auto-init logic).
    RUNTIME_SHUTDOWN_COMPLETE.store(true, AtomicOrdering::SeqCst);
    unsafe {
        drop(Box::from_raw(ptr));
    }
    drop(gil);
    1
}

static RUNTIME_STATE_PTR: AtomicPtr<RuntimeState> = AtomicPtr::new(std::ptr::null_mut());
static RUNTIME_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static PROCESS_EXIT_FINALIZED: AtomicBool = AtomicBool::new(false);
/// Set to `true` after `molt_runtime_shutdown` completes.  Prevents
/// `molt_runtime_init` from re-allocating state during process exit.
static RUNTIME_SHUTDOWN_COMPLETE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TLS_RUNTIME_STATE: Cell<*mut RuntimeState> = const { Cell::new(std::ptr::null_mut()) };
}

fn runtime_state_tls() -> Option<&'static RuntimeState> {
    // Use `try_with` instead of `with` to avoid panicking (and aborting)
    // when this TLS variable has already been destroyed during process exit.
    // During Rust's TLS destructor phase, `ThreadLocalGuard::drop` calls
    // `runtime_state_for_gil()` which calls this function.  If
    // `TLS_RUNTIME_STATE` is destroyed before `TLS_GUARD`, `.with()` would
    // panic inside a Drop impl, causing an abort (exit code 134/139).
    TLS_RUNTIME_STATE
        .try_with(|slot| {
            let ptr = slot.get();
            if ptr.is_null() {
                None
            } else {
                Some(unsafe { &*ptr })
            }
        })
        .ok()
        .flatten()
}

pub(crate) fn set_thread_runtime_state(ptr: *mut RuntimeState) {
    let _ = TLS_RUNTIME_STATE.try_with(|slot| slot.set(ptr));
}

pub(crate) fn clear_thread_runtime_state() {
    let _ = TLS_RUNTIME_STATE.try_with(|slot| slot.set(std::ptr::null_mut()));
}

/// Resets all one-shot flags that prevent runtime re-initialization.
///
/// # Safety contract
///
/// This function is **test-only** (`#[cfg(test)]`).  It must NEVER be
/// compiled into production binaries.  The flags it clears exist to prevent
/// dangerous double-init / use-after-free during process exit.  Resetting
/// them is only safe in a controlled test harness where:
///
/// 1. A serialization mutex (`TEST_MUTEX`) ensures no concurrent runtime
///    access.
/// 2. The previous runtime has been fully shut down via
///    `molt_runtime_shutdown()`.
/// 3. The caller will immediately re-initialize via `molt_runtime_init()`.
#[cfg(test)]
pub(crate) fn molt_runtime_reset_for_testing() {
    // Clear the permanent shutdown flag so `molt_runtime_init` will accept
    // a new `RuntimeState` allocation.
    RUNTIME_SHUTDOWN_COMPLETE.store(false, AtomicOrdering::SeqCst);

    // Clear the global state pointer (should already be null after shutdown,
    // but be defensive).
    RUNTIME_STATE_PTR.store(std::ptr::null_mut(), AtomicOrdering::SeqCst);

    // Clear the TLS cache so no stale pointer is returned by
    // `runtime_state_tls()`.
    clear_thread_runtime_state();

    // Clear the intrinsic registry's one-shot flags so the next init can
    // re-register intrinsics into a fresh builtins module.  Without this,
    // BUILTINS_MODULE_PTR holds a dangling pointer to the destroyed module
    // and MANIFEST_SET prevents re-setting the manifest.
    crate::intrinsics::registry::reset_for_testing();
}
