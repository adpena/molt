use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use super::{runtime_reset_for_init, runtime_teardown, touch_tls_guard};

#[cfg(not(target_arch = "wasm32"))]
use libc;

use crate::concurrency::gil::{gil_held, hold_runtime_gil, release_runtime_gil};
use crate::object::utf8_cache::{build_utf8_count_cache, Utf8CacheStore, Utf8CountCacheStore};
use crate::IoPoller;
use crate::ProcessTaskState;
use crate::{
    default_cancel_tokens, AsyncHangProbe, BuiltinClasses, CancelTokenEntry, GilGuard, HashSecret,
    InternedNames, MethodCache, MoltObject, MoltScheduler, PtrSlot, PyToken, SleepQueue,
    OBJECT_POOL_BUCKETS,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{sleep_worker, ThreadPool, ThreadTaskState};

#[cfg(target_arch = "wasm32")]
extern "C" {
    fn __wasm_call_ctors();
}

#[cfg(target_arch = "wasm32")]
static WASM_CTORS: OnceLock<()> = OnceLock::new();

#[cfg(target_arch = "wasm32")]
fn ensure_wasm_ctors() {
    WASM_CTORS.get_or_init(|| unsafe {
        __wasm_call_ctors();
    });
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
    let msg = b"molt debug: SIGTRAP backtrace\n";
    let _ = libc::write(2, msg.as_ptr() as *const _, msg.len());
    let mut addrs = [std::ptr::null_mut(); 128];
    let count = libc::backtrace(addrs.as_mut_ptr(), addrs.len() as i32);
    if count > 0 {
        libc::backtrace_symbols_fd(addrs.as_ptr(), count, 2);
    }
    libc::_exit(128 + sig);
}

#[cfg(not(target_arch = "wasm32"))]
fn ensure_debug_sigtrap_handler() {
    if debug_sigtrap_backtrace_enabled()
        && !DEBUG_SIGTRAP_INSTALLED.swap(true, AtomicOrdering::Relaxed)
    {
        unsafe {
            libc::signal(libc::SIGTRAP, debug_sigtrap_handler as usize);
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
    pub(crate) module_cache: Mutex<HashMap<String, u64>>,
    pub(crate) exception_type_cache: Mutex<HashMap<String, u64>>,
    pub(crate) argv: Mutex<Vec<String>>,
    pub(crate) sys_version_info: Mutex<Option<PythonVersionInfo>>,
    pub(crate) sys_version: Mutex<Option<String>>,
    pub(crate) object_pool: Mutex<Vec<Vec<PtrSlot>>>,
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
    pub(crate) task_exception_handler_stacks: Mutex<HashMap<PtrSlot, Vec<u8>>>,
    pub(crate) task_exception_stacks: Mutex<HashMap<PtrSlot, Vec<u64>>>,
    pub(crate) task_exception_depths: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_exception_baselines: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_last_exceptions: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) dict_subclass_storage: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) await_waiters: Mutex<HashMap<PtrSlot, Vec<PtrSlot>>>,
    pub(crate) task_waiting_on: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) asyncgen_hooks: Mutex<AsyncGenHooks>,
    pub(crate) asyncgen_locals: Mutex<HashMap<u64, AsyncGenLocalsEntry>>,
    pub(crate) gen_locals: Mutex<HashMap<u64, GenLocalsEntry>>,
    pub(crate) weakrefs: Mutex<WeakRefRegistry>,
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
    pub(crate) gil: Mutex<()>,
    pub(crate) start_time: OnceLock<Instant>,
}

impl RuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            builtin_classes: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            interned: InternedNames::new(),
            method_cache: MethodCache::new(),
            special_cache: SpecialCache::new(),
            last_exception: Mutex::new(None),
            module_cache: Mutex::new(HashMap::new()),
            exception_type_cache: Mutex::new(HashMap::new()),
            argv: Mutex::new(Vec::new()),
            sys_version_info: Mutex::new(None),
            sys_version: Mutex::new(None),
            object_pool: Mutex::new(vec![Vec::new(); OBJECT_POOL_BUCKETS]),
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
            task_exception_handler_stacks: Mutex::new(HashMap::new()),
            task_exception_stacks: Mutex::new(HashMap::new()),
            task_exception_depths: Mutex::new(HashMap::new()),
            task_exception_baselines: Mutex::new(HashMap::new()),
            task_last_exceptions: Mutex::new(HashMap::new()),
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
            gil: Mutex::new(()),
            start_time: OnceLock::new(),
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
}

pub(crate) fn runtime_state_lock() -> &'static Mutex<()> {
    RUNTIME_STATE_LOCK.get_or_init(|| Mutex::new(()))
}

#[allow(dead_code)]
fn runtime_state_ptr() -> Option<*mut RuntimeState> {
    let ptr = RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
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
        let ptr = runtime_state_ptr().expect("runtime state should be initialized");
        unsafe { &*ptr }
    }
}

#[no_mangle]
pub extern "C" fn molt_runtime_init() -> u64 {
    #[cfg(target_arch = "wasm32")]
    ensure_wasm_ctors();
    touch_tls_guard();
    #[cfg(not(target_arch = "wasm32"))]
    ensure_debug_sigtrap_handler();
    let _guard = runtime_state_lock().lock().unwrap();
    if !RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst).is_null() {
        return 1;
    }
    let state = Box::new(RuntimeState::new());
    let ptr = Box::into_raw(state);
    RUNTIME_STATE_PTR.store(ptr, AtomicOrdering::SeqCst);
    let state_ref = unsafe { &*ptr };
    let gil = GilGuard::new();
    {
        let py = gil.token();
        runtime_reset_for_init(&py, state_ref);
    }
    hold_runtime_gil(gil);
    1
}

#[no_mangle]
pub extern "C" fn molt_runtime_ensure_gil() {
    touch_tls_guard();
    if gil_held() {
        return;
    }
    hold_runtime_gil(GilGuard::new());
}

#[no_mangle]
pub extern "C" fn molt_runtime_shutdown() -> u64 {
    let _guard = runtime_state_lock().lock().unwrap();
    let ptr = RUNTIME_STATE_PTR.load(AtomicOrdering::SeqCst);
    if ptr.is_null() {
        return 0;
    }
    let state = unsafe { &*ptr };
    {
        let gil = GilGuard::new();
        let py = gil.token();
        runtime_teardown(&py, state);
        release_runtime_gil();
    }
    RUNTIME_STATE_PTR.store(std::ptr::null_mut(), AtomicOrdering::SeqCst);
    unsafe {
        drop(Box::from_raw(ptr));
    }
    1
}

static RUNTIME_STATE_PTR: AtomicPtr<RuntimeState> = AtomicPtr::new(std::ptr::null_mut());
static RUNTIME_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

thread_local! {
    static TLS_RUNTIME_STATE: Cell<*mut RuntimeState> = const { Cell::new(std::ptr::null_mut()) };
}

fn runtime_state_tls() -> Option<&'static RuntimeState> {
    TLS_RUNTIME_STATE.with(|slot| {
        let ptr = slot.get();
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &*ptr })
        }
    })
}

pub(crate) fn set_thread_runtime_state(ptr: *mut RuntimeState) {
    TLS_RUNTIME_STATE.with(|slot| slot.set(ptr));
}

pub(crate) fn clear_thread_runtime_state() {
    TLS_RUNTIME_STATE.with(|slot| slot.set(std::ptr::null_mut()));
}
