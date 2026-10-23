use crate::runtime_flags::{self, RuntimeFlag};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering as AtomicOrdering,
};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use super::{
    runtime_reset_for_init, runtime_teardown, runtime_teardown_for_process_exit, touch_tls_guard,
};

use crate::IoPoller;
use crate::ProcessTaskState;
use crate::async_rt::event_loop::{EventLoopRegistry, PipeTransportRegistry};
use crate::async_rt::scheduler::{AsyncioEventWaiterIndex, AwaitWaiterIndex};
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::async_rt::sockets::SocketRuntimeState;
use crate::builtins::attributes::AttributesRuntimeState;
use crate::builtins::concurrent::ConcurrentRuntimeState;
use crate::builtins::copy_mod::CopyMemoRuntimeState;
use crate::builtins::exceptions::ExceptionsRuntimeState;
use crate::builtins::functools::FunctoolsRuntimeState;
use crate::builtins::io::IoRuntimeState;
use crate::builtins::modules::ModulesRuntimeState;
use crate::builtins::operator::OperatorRuntimeState;
use crate::builtins::platform::PlatformRuntimeState;
use crate::builtins::signal_ext::{SignalRuntimeState, signal_runtime_state_publish};
use crate::builtins::sys_ext::SysRuntimeState;
use crate::builtins::types::TypesRuntimeState;
use crate::c_api::CApiModuleRuntimeState;
use crate::call::bind::CallBindRuntimeState;
use crate::concurrency::gil::gil_held;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::object::builders::CanonicalObjectCache;
use crate::object::utf8_cache::{Utf8CacheStore, Utf8CountCacheStore, build_utf8_count_cache};
use crate::object::weakref::WeakContainerCookie;
use crate::{
    AsyncHangProbe, BuiltinClasses, CancelTokenEntry, GilGuard, HashSecret, InternedNames,
    MethodCache, MoltObject, MoltScheduler, ProcessRegistry, PtrSlot, PyToken, RuntimeStaticNames,
    SleepQueue, default_cancel_tokens,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{ThreadPool, ThreadTaskState};

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

#[cfg(unix)]
static DEBUG_SIGTRAP_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn debug_sigtrap_backtrace_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_SIGTRAP_BACKTRACE)
}

#[cfg(unix)]
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

#[cfg(unix)]
fn ensure_debug_sigtrap_handler() {
    if debug_sigtrap_backtrace_enabled()
        && !DEBUG_SIGTRAP_INSTALLED.swap(true, AtomicOrdering::Relaxed)
    {
        unsafe {
            libc::signal(libc::SIGTRAP, debug_sigtrap_handler as *const () as usize);
        }
    }
}

#[cfg(not(unix))]
fn ensure_debug_sigtrap_handler() {}

pub(crate) struct SpecialCache {
    pub(crate) awaitable_await: AtomicU64,
    pub(crate) function_code_descriptor: AtomicU64,
    pub(crate) function_globals_descriptor: AtomicU64,
    pub(crate) weakref_callback_descriptor: AtomicU64,
}

pub(crate) type RuntimeExtensionStateInit = unsafe extern "C" fn() -> *mut u8;
pub(crate) type RuntimeExtensionStateClear = unsafe extern "C" fn(*mut u8);
pub(crate) type RuntimeExtensionStateDrop = unsafe extern "C" fn(*mut u8);

pub(crate) struct RuntimeExtensionStateSlot {
    ptr: *mut u8,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
}

// Extension states are only accessed through the runtime GIL plus this map's
// mutex. The raw pointer is an opaque Box owned by the registering crate.
unsafe impl Send for RuntimeExtensionStateSlot {}

/// One compiler-published binding stored in a stateful activation's typed
/// frame slot. Parameter slots are bound by the constructor; cell slots hold
/// the closure cell that the compiled prologue publishes for the binding.
#[derive(Clone, Copy)]
pub(crate) struct StatefulLocalSlot {
    pub(crate) name_bits: u64,
    pub(crate) offset: usize,
    pub(crate) parameter: bool,
    /// Prologue publication ordinal; None denotes a raw binding.
    pub(crate) cell: Option<usize>,
}

/// Immutable public-locals layout registered for one stateful poll target:
/// generators, coroutines, async generators and generator expressions alike.
/// One shared layout owns one reference to every name until its final lease
/// or registry edge retires. Never clone the vectors on a poll entry.
pub(crate) struct StatefulLocalsLayout {
    pub(crate) slots: Vec<StatefulLocalSlot>,
    pub(crate) closure_offset: Option<usize>,
    pub(crate) free_var_names: Vec<u64>,
}

impl StatefulLocalsLayout {
    pub(crate) fn name_bits(&self) -> impl Iterator<Item = u64> + '_ {
        self.slots
            .iter()
            .map(|slot| slot.name_bits)
            .chain(self.free_var_names.iter().copied())
    }
}

/// Release the registry or a scoped lease outside the registry mutex. Every
/// Arc owner follows this path, so names survive replacement/reentrant readers
/// and are released exactly once, by the last owner while holding a PyToken.
pub(crate) fn release_stateful_locals_layout(py: &PyToken<'_>, layout: Arc<StatefulLocalsLayout>) {
    if let Some(layout) = Arc::into_inner(layout) {
        for bits in layout.name_bits() {
            crate::dec_ref_bits(py, bits);
        }
    }
}

/// A constant-time shared layout acquisition whose final release has GIL custody.
/// The Arc is private to prevent a consumer from accidentally dropping its last
/// name-owning edge without the explicit release authority.
pub(crate) struct StatefulLocalsLease<'a, 'py> {
    py: &'a PyToken<'py>,
    layout: Option<Arc<StatefulLocalsLayout>>,
}

impl<'a, 'py> StatefulLocalsLease<'a, 'py> {
    pub(crate) fn acquire(py: &'a PyToken<'py>, layout: &Arc<StatefulLocalsLayout>) -> Self {
        Self {
            py,
            layout: Some(Arc::clone(layout)),
        }
    }
}

impl std::ops::Deref for StatefulLocalsLease<'_, '_> {
    type Target = StatefulLocalsLayout;
    fn deref(&self) -> &Self::Target {
        self.layout.as_ref().expect("live layout lease").as_ref()
    }
}

impl Drop for StatefulLocalsLease<'_, '_> {
    fn drop(&mut self) {
        if let Some(layout) = self.layout.take() {
            release_stateful_locals_layout(self.py, layout);
        }
    }
}

#[derive(Clone)]
pub(crate) struct WeakRefEntry {
    pub(crate) target: PtrSlot,
    pub(crate) callback_bits: u64,
    pub(crate) container_cookie: Option<WeakContainerCookie>,
    /// CPython-compatible sticky hash: once computed while the referent is
    /// alive, the value remains available after referent death. `-1` is the
    /// uncomputed sentinel; Python hash results normalize `-1` to `-2`.
    pub(crate) cached_hash: i64,
}

#[derive(Clone)]
pub(crate) struct AtexitCallbackEntry {
    pub(crate) registration_id: u64,
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WeakrefRunnerState {
    Available,
    Registered,
    Cleared,
}

#[derive(Clone, Copy, Debug)]
struct WeakFinalizerId {
    slot: usize,
    generation: u64,
}

#[derive(Debug)]
struct WeakFinalizerSlot {
    bits: u64,
    generation: u64,
    order_prev: Option<usize>,
    order_next: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct WeakFinalizerPrepared {
    index: HashMap<u64, WeakFinalizerId>,
    slots: Vec<Option<WeakFinalizerSlot>>,
    free_slots: Vec<usize>,
}

impl WeakFinalizerPrepared {
    pub(crate) fn try_with_capacity(capacity: usize) -> Result<Self, ()> {
        let mut index = HashMap::new();
        index.try_reserve(capacity).map_err(|_| ())?;
        let mut slots = Vec::new();
        slots.try_reserve_exact(capacity).map_err(|_| ())?;
        let mut free_slots = Vec::new();
        free_slots.try_reserve_exact(capacity).map_err(|_| ())?;
        Ok(Self {
            index,
            slots,
            free_slots,
        })
    }

    pub(crate) fn capacity(&self) -> usize {
        self.index
            .capacity()
            .min(self.slots.capacity())
            .min(self.free_slots.capacity())
    }
}

/// Stable LIFO order and an O(1) identity index share the same slot authority.
/// The generation prevents a recycled slot from ever validating a stale id.
pub(crate) struct WeakFinalizerRegistry {
    index: HashMap<u64, WeakFinalizerId>,
    slots: Vec<Option<WeakFinalizerSlot>>,
    free_slots: Vec<usize>,
    order_head: Option<usize>,
    order_tail: Option<usize>,
    next_generation: u64,
    #[cfg(test)]
    growth_count: usize,
}

impl WeakFinalizerRegistry {
    fn new() -> Self {
        Self {
            index: HashMap::new(),
            slots: Vec::new(),
            free_slots: Vec::new(),
            order_head: None,
            order_tail: None,
            next_generation: 1,
            #[cfg(test)]
            growth_count: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.index.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub(crate) fn contains(&self, bits: u64) -> bool {
        self.index.contains_key(&bits)
    }

    pub(crate) fn capacity(&self) -> usize {
        self.index
            .capacity()
            .min(self.slots.capacity())
            .min(self.free_slots.capacity())
    }

    pub(crate) fn can_insert(&self) -> bool {
        self.next_generation != u64::MAX
            && self.index.len() < self.index.capacity()
            && (!self.free_slots.is_empty() || self.slots.len() < self.slots.capacity())
    }

    pub(crate) fn generation_available(&self) -> bool {
        self.next_generation != u64::MAX
    }

    /// Replace all allocation-bearing stores with externally prepared buffers.
    /// Migration is allocation-free and the displaced empty stores are dropped
    /// by the caller after releasing the registry lock.
    pub(crate) fn install_prepared(
        &mut self,
        mut prepared: WeakFinalizerPrepared,
    ) -> Result<WeakFinalizerPrepared, WeakFinalizerPrepared> {
        let required = match self.index.len().checked_add(1) {
            Some(required) => required,
            None => return Err(prepared),
        };
        if prepared.capacity() < required {
            return Err(prepared);
        }
        for (bits, id) in self.index.drain() {
            prepared.index.insert(bits, id);
        }
        prepared.slots.append(&mut self.slots);
        prepared.free_slots.append(&mut self.free_slots);
        std::mem::swap(&mut prepared.index, &mut self.index);
        std::mem::swap(&mut prepared.slots, &mut self.slots);
        std::mem::swap(&mut prepared.free_slots, &mut self.free_slots);
        #[cfg(test)]
        {
            self.growth_count += 1;
        }
        Ok(prepared)
    }

    pub(crate) fn insert_prepared(&mut self, bits: u64) -> Result<bool, ()> {
        if self.contains(bits) {
            return Ok(false);
        }
        if !self.can_insert() {
            return Err(());
        }
        let generation = self.next_generation;
        let next_generation = generation.checked_add(1).ok_or(())?;
        if self
            .order_tail
            .is_some_and(|tail| self.slots.get(tail).is_none_or(|entry| entry.is_none()))
        {
            return Err(());
        }
        let slot = if let Some(slot) = self.free_slots.pop() {
            slot
        } else {
            let slot = self.slots.len();
            self.slots.push(None);
            slot
        };
        let id = WeakFinalizerId { slot, generation };
        self.slots[slot] = Some(WeakFinalizerSlot {
            bits,
            generation,
            order_prev: self.order_tail,
            order_next: None,
        });
        if let Some(tail) = self.order_tail {
            if let Some(tail_entry) = self.slots.get_mut(tail).and_then(Option::as_mut) {
                tail_entry.order_next = Some(slot);
            }
        } else {
            self.order_head = Some(slot);
        }
        self.order_tail = Some(slot);
        self.index.insert(bits, id);
        self.next_generation = next_generation;
        Ok(true)
    }

    pub(crate) fn remove(&mut self, bits: u64) -> Option<u64> {
        let id = *self.index.get(&bits)?;
        let entry = self.slots.get(id.slot)?.as_ref()?;
        if entry.bits != bits || entry.generation != id.generation {
            return None;
        }
        let (order_prev, order_next) = (entry.order_prev, entry.order_next);
        self.index.remove(&bits);
        if let Some(prev) = order_prev {
            if let Some(entry) = self.slots.get_mut(prev).and_then(Option::as_mut) {
                entry.order_next = order_next;
            }
        } else {
            self.order_head = order_next;
        }
        if let Some(next) = order_next {
            if let Some(entry) = self.slots.get_mut(next).and_then(Option::as_mut) {
                entry.order_prev = order_prev;
            }
        } else {
            self.order_tail = order_prev;
        }
        let entry = self.slots[id.slot].take()?;
        self.free_slots.push(id.slot);
        Some(entry.bits)
    }

    pub(crate) fn pop_lifo(&mut self) -> Option<u64> {
        let tail = self.order_tail?;
        let bits = self.slots.get(tail)?.as_ref()?.bits;
        self.remove(bits)
    }
}

/// One lock owns exit callback ordering, monotonic registration identities,
/// weakref-finalizer publication, and its runner. Keeping these facts together
/// makes publication atomic without an independent flag that can get ahead of
/// the callback it describes.
pub(crate) struct ExitRegistry {
    pub(crate) callbacks: Vec<AtexitCallbackEntry>,
    pub(crate) weakref_finalizers: WeakFinalizerRegistry,
    pub(crate) weakref_runner_state: WeakrefRunnerState,
    pub(crate) next_callback_id: u64,
}

impl ExitRegistry {
    pub(crate) fn new() -> Self {
        Self {
            callbacks: Vec::new(),
            weakref_finalizers: WeakFinalizerRegistry::new(),
            weakref_runner_state: WeakrefRunnerState::Available,
            next_callback_id: 1,
        }
    }

    pub(crate) fn allocate_callback_id(&mut self) -> Option<u64> {
        let id = self.next_callback_id;
        self.next_callback_id = self.next_callback_id.checked_add(1)?;
        Some(id)
    }
}

pub(crate) struct WeakBorrowEntry {
    pub(crate) generation: u64,
    pub(crate) borrowers: usize,
}

pub(crate) struct WeakRefRegistry {
    pub(crate) by_ref: HashMap<PtrSlot, WeakRefEntry>,
    pub(crate) by_target: HashMap<PtrSlot, Vec<PtrSlot>>,
    // Internal allocation-lifetime observation. These are not Python weakrefs:
    // public weakref admission, enumeration and callbacks use only by_ref/target.
    pub(crate) borrowed_targets: HashMap<PtrSlot, WeakBorrowEntry>,
    pub(crate) next_borrow_generation: u64,
}

impl WeakRefRegistry {
    pub(crate) fn new() -> Self {
        Self {
            by_ref: HashMap::new(),
            by_target: HashMap::new(),
            borrowed_targets: HashMap::new(),
            next_borrow_generation: 1,
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
            awaitable_await: AtomicU64::new(0),
            function_code_descriptor: AtomicU64::new(0),
            function_globals_descriptor: AtomicU64::new(0),
            weakref_callback_descriptor: AtomicU64::new(0),
        }
    }
}

/// Fixed operator policy captured before Ready. These typed values are
/// unrelated to the Python environment mapping and cannot be changed by it.
#[derive(Clone)]
pub(crate) struct RuntimePolicy {
    pub(crate) resources: Option<crate::resource::ResourceLimits>,
    pub(crate) stream_max_queued_bytes: usize,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) process_pipe_max_queued_bytes: usize,
    #[cfg(unix)]
    pub(crate) child_memory_rlimit_bytes: Option<u64>,
    pub(crate) thread_isolated: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async_workers: usize,
    pub(crate) exception_log_path: Option<std::path::PathBuf>,
    pub(crate) feedback_path: Option<std::path::PathBuf>,
}

impl RuntimePolicy {
    /// Freeze the startup CWD basis without resolving symlinks or opening output.
    fn capture_output_path(setting: &str, path: std::path::PathBuf) -> Option<std::path::PathBuf> {
        if path.is_absolute() {
            return Some(path);
        }
        match std::path::absolute(&path) {
            Ok(path) => Some(path),
            Err(error) => {
                eprintln!(
                    "molt runtime: disabling {setting}: cannot capture startup output path: {error}"
                );
                None
            }
        }
    }

    fn capture(resources: Option<crate::resource::ResourceLimits>) -> Self {
        let stream_max_queued_bytes = crate::async_rt::channels::capture_stream_max_queued_bytes();
        #[cfg(not(target_arch = "wasm32"))]
        let process_pipe_max_queued_bytes = std::env::var("MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(stream_max_queued_bytes);
        #[cfg(unix)]
        let child_memory_rlimit_bytes = crate::async_rt::process::child_memory_rlimit_bytes(
            resources.as_ref(),
            std::env::var("MOLT_CHILD_RLIMIT_GB").ok().as_deref(),
        );
        let thread_isolated = runtime_flags::enabled(RuntimeFlag::MOLT_THREAD_ISOLATED);
        #[cfg(not(target_arch = "wasm32"))]
        let async_workers = std::env::var("MOLT_ASYNC_THREADS")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(0)
            .min(num_cpus::get().max(1));
        let exception_log_path = std::env::var("MOLT_EXCEPTION_LOG_PATH")
            .ok()
            .and_then(|path| Self::capture_output_path("MOLT_EXCEPTION_LOG_PATH", path.into()));
        let feedback_path = runtime_flags::enabled(RuntimeFlag::MOLT_RUNTIME_FEEDBACK)
            .then(|| {
                Self::capture_output_path(
                    "MOLT_RUNTIME_FEEDBACK_FILE",
                    std::env::var("MOLT_RUNTIME_FEEDBACK_FILE")
                        .ok()
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "molt_runtime_feedback.json".to_owned())
                        .into(),
                )
            })
            .flatten();
        Self {
            resources,
            stream_max_queued_bytes,
            #[cfg(not(target_arch = "wasm32"))]
            process_pipe_max_queued_bytes,
            #[cfg(unix)]
            child_memory_rlimit_bytes,
            thread_isolated,
            #[cfg(not(target_arch = "wasm32"))]
            async_workers,
            exception_log_path,
            feedback_path,
        }
    }
}

pub(crate) struct RuntimeState {
    policy: OnceLock<RuntimePolicy>,
    /// External native threads borrowing this exact heap, plus its closed bit.
    #[cfg(not(target_arch = "wasm32"))]
    thread_borrow_admission: AtomicUsize,
    /// Interpreter policy shared by its threads; execution depth stays thread-local.
    pub(crate) recursion_limit: AtomicUsize,
    pub(crate) gc: crate::object::gc::GcRuntimeState,
    pub(crate) gc_running: AtomicBool,
    pub(crate) gc_last_failure: AtomicU8,
    pub(crate) builtin_classes: std::sync::atomic::AtomicPtr<BuiltinClasses>,
    pub(crate) cpython: crate::cpython_abi_hooks::CpythonRuntimeState,
    pub(crate) interned: InternedNames,
    pub(crate) runtime_static_names: RuntimeStaticNames,
    pub(crate) method_cache: MethodCache,
    pub(crate) special_cache: SpecialCache,
    pub(crate) canonical_objects: CanonicalObjectCache,
    pub(crate) module_cache: Mutex<HashMap<String, u64>>,
    pub(crate) interpreter_sys: crate::builtins::module_table::InterpreterSysNamespace,
    /// Import-bedrock ModuleTable (design doc 69): dense per-ModuleId state
    /// machine + slots, one instance per isolate, sized from the installed
    /// module registry on first use. Owned by builtins::module_table.
    pub(crate) module_table: OnceLock<crate::builtins::module_table::ModuleTable>,
    pub(crate) importlib_default_meta_path_bootstrapped: AtomicBool,
    pub(crate) intrinsic_registry_module: AtomicPtr<u8>,
    pub(crate) exception_type_cache: Mutex<HashMap<String, u64>>,
    pub(crate) exceptions: ExceptionsRuntimeState,
    pub(crate) codec_error_handlers: Mutex<HashMap<String, u64>>,
    pub(crate) argv: Mutex<Vec<Vec<u8>>>,
    pub(crate) sys_version_info: Mutex<Option<PythonVersionInfo>>,
    pub(crate) hash_secret: OnceLock<HashSecret>,
    pub(crate) utf8_index_cache: Mutex<Utf8CacheStore>,
    pub(crate) utf8_count_cache: Vec<Mutex<Utf8CountCacheStore>>,
    pub(crate) scheduler: OnceLock<MoltScheduler>,
    pub(crate) sleep_queue: OnceLock<Arc<SleepQueue>>,
    pub(crate) io_poller: OnceLock<Arc<IoPoller>>,
    pub(crate) capabilities: OnceLock<HashSet<String>>,
    pub(crate) execution_target: OnceLock<String>,
    pub(crate) trusted: OnceLock<bool>,
    pub(crate) async_hang_probe: OnceLock<Option<AsyncHangProbe>>,
    pub(crate) event_loop_registry: EventLoopRegistry,
    pub(crate) pipe_transport_registry: PipeTransportRegistry,
    pub(crate) cancel_tokens: Mutex<HashMap<u64, CancelTokenEntry>>,
    pub(crate) task_tokens:
        Mutex<HashMap<PtrSlot, crate::async_rt::cancellation::TaskExecutionAttachment>>,
    pub(crate) task_tokens_by_id: Mutex<HashMap<u64, HashSet<PtrSlot>>>,
    pub(crate) task_cancel_messages: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) asyncio_running_loops: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_event_loops: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_event_loop_policy: Mutex<u64>,
    pub(crate) asyncio_tasks: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_current_tasks: Mutex<HashMap<u64, u64>>,
    pub(crate) asyncio_event_waiters: Mutex<HashMap<u64, Vec<u64>>>,
    pub(crate) asyncio_event_waiter_index: Mutex<HashMap<u64, AsyncioEventWaiterIndex>>,
    pub(crate) task_exception_handler_stacks: Mutex<HashMap<PtrSlot, Vec<usize>>>,
    pub(crate) task_exception_stacks: Mutex<HashMap<PtrSlot, Vec<u64>>>,
    pub(crate) task_exception_depths: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_exception_baselines: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_last_exceptions: Mutex<HashMap<PtrSlot, Option<PtrSlot>>>,
    pub(crate) task_results: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) attributes: AttributesRuntimeState,
    pub(crate) await_waiters: Mutex<HashMap<PtrSlot, Vec<PtrSlot>>>,
    pub(crate) await_waiter_index: Mutex<HashMap<PtrSlot, AwaitWaiterIndex>>,
    pub(crate) task_waiting_on: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) asyncgen_hooks: Mutex<HashMap<thread::ThreadId, AsyncGenHooks>>,
    pub(crate) concurrent: ConcurrentRuntimeState,
    pub(crate) copy_memo: Mutex<CopyMemoRuntimeState>,
    pub(crate) functools: FunctoolsRuntimeState,
    pub(crate) io: IoRuntimeState,
    pub(crate) modules: ModulesRuntimeState,
    pub(crate) operator: OperatorRuntimeState,
    pub(crate) platform: PlatformRuntimeState,
    pub(crate) types: TypesRuntimeState,
    pub(crate) sys_ext: SysRuntimeState,
    pub(crate) c_api_module: Mutex<CApiModuleRuntimeState>,
    pub(crate) call_bind: Mutex<CallBindRuntimeState>,
    pub(crate) stateful_locals: Mutex<HashMap<u64, Arc<StatefulLocalsLayout>>>,
    pub(crate) weakrefs: Mutex<WeakRefRegistry>,
    pub(crate) exit_registry: Mutex<ExitRegistry>,
    pub(crate) abc_invalidation_counter: AtomicU64,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_pool: OnceLock<ThreadPool>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_registry: OnceLock<Mutex<crate::concurrency::isolates::ThreadRegistry>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_tasks: Mutex<HashMap<PtrSlot, Arc<ThreadTaskState>>>,
    pub(crate) process_registry: ProcessRegistry,
    #[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
    pub(crate) socket_state: SocketRuntimeState,
    pub(crate) signal: SignalRuntimeState,
    pub(crate) process_tasks: Mutex<HashMap<PtrSlot, Arc<ProcessTaskState>>>,
    pub(crate) code_slots: OnceLock<Vec<crate::builtins::frames::CompiledCodeSlot>>,
    pub(crate) start_time: OnceLock<Instant>,
    /// Routing is frozen at startup; bundle payload loading remains lazy.
    pub(crate) vfs_state: OnceLock<Option<Arc<crate::vfs::VfsState>>>,
    /// Typed state owned by extracted runtime crates and scoped to this runtime.
    pub(crate) extension_states: Mutex<HashMap<Vec<u8>, RuntimeExtensionStateSlot>>,
}

impl RuntimeState {
    pub(crate) fn policy(&self) -> &RuntimePolicy {
        self.policy
            .get()
            .expect("runtime policy must be frozen before use")
    }
    /// Stable while this runtime owns heap/C state. Failed teardown retains the
    /// allocation, so a live owner identity cannot be reused by another isolate.
    #[inline]
    pub(crate) fn identity(&self) -> usize {
        std::ptr::from_ref(self).expose_provenance()
    }

    pub(crate) fn new() -> Self {
        Self::with_process_registry(ProcessRegistry::new())
    }

    /// A lease pins the primary allocation through isolate teardown. Inherit
    /// operator policy before attaching the fresh heap or running bootstrap;
    /// spawning after a guest environment mutation cannot grant new authority.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn new_isolate(lifetime: &RuntimeLifetimeLease) -> Self {
        let primary: &RuntimeState =
            unsafe { &*std::ptr::with_exposed_provenance::<RuntimeState>(lifetime.runtime_owner) };
        let state = Self::with_process_registry(primary.process_registry.new_isolate());
        assert!(state.policy.set(primary.policy().clone()).is_ok());
        assert!(
            state
                .capabilities
                .set(primary.capabilities.get().expect("startup grants").clone())
                .is_ok()
        );
        assert!(
            state
                .trusted
                .set(*primary.trusted.get().expect("startup trust"))
                .is_ok()
        );
        assert!(
            state
                .execution_target
                .set(
                    primary
                        .execution_target
                        .get()
                        .expect("startup target")
                        .clone()
                )
                .is_ok()
        );
        // Share the same mount and backend authority, including its lazy load
        // state and quotas. A fresh heap is not a fresh host filesystem grant.
        assert!(
            state
                .vfs_state
                .set(primary.vfs_state.get().expect("startup VFS").clone())
                .is_ok()
        );
        crate::builtins::platform::inherit_startup_state(&state, primary);
        state
    }

    fn with_process_registry(process_registry: ProcessRegistry) -> Self {
        Self {
            policy: OnceLock::new(),
            #[cfg(not(target_arch = "wasm32"))]
            thread_borrow_admission: AtomicUsize::new(0),
            recursion_limit: AtomicUsize::new(super::recursion::DEFAULT_RECURSION_LIMIT),
            gc: crate::object::gc::GcRuntimeState::new(),
            gc_running: AtomicBool::new(false),
            gc_last_failure: AtomicU8::new(0),
            builtin_classes: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            cpython: crate::cpython_abi_hooks::CpythonRuntimeState::new(),
            interned: InternedNames::new(),
            runtime_static_names: RuntimeStaticNames::new(),
            method_cache: MethodCache::new(),
            special_cache: SpecialCache::new(),
            canonical_objects: CanonicalObjectCache::new(),
            module_cache: Mutex::new(HashMap::new()),
            interpreter_sys: crate::builtins::module_table::InterpreterSysNamespace::new(),
            module_table: OnceLock::new(),
            importlib_default_meta_path_bootstrapped: AtomicBool::new(false),
            intrinsic_registry_module: AtomicPtr::new(std::ptr::null_mut()),
            exception_type_cache: Mutex::new(HashMap::new()),
            exceptions: ExceptionsRuntimeState::new(),
            codec_error_handlers: Mutex::new({
                let mut handlers = HashMap::new();
                for name in [
                    "strict",
                    "ignore",
                    "replace",
                    "xmlcharrefreplace",
                    "backslashreplace",
                    "namereplace",
                    "surrogateescape",
                    "surrogatepass",
                ] {
                    handlers.insert(name.to_owned(), MoltObject::from_bool(true).bits());
                }
                handlers
            }),
            argv: Mutex::new(Vec::new()),
            sys_version_info: Mutex::new(None),
            hash_secret: OnceLock::new(),
            utf8_index_cache: Mutex::new(Utf8CacheStore::new()),
            utf8_count_cache: build_utf8_count_cache(),
            scheduler: OnceLock::new(),
            sleep_queue: OnceLock::new(),
            io_poller: OnceLock::new(),
            capabilities: OnceLock::new(),
            execution_target: OnceLock::new(),
            trusted: OnceLock::new(),
            async_hang_probe: OnceLock::new(),
            event_loop_registry: EventLoopRegistry::new(),
            pipe_transport_registry: PipeTransportRegistry::new(),
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
            asyncio_event_waiter_index: Mutex::new(HashMap::new()),
            task_exception_handler_stacks: Mutex::new(HashMap::new()),
            task_exception_stacks: Mutex::new(HashMap::new()),
            task_exception_depths: Mutex::new(HashMap::new()),
            task_exception_baselines: Mutex::new(HashMap::new()),
            task_last_exceptions: Mutex::new(HashMap::new()),
            task_results: Mutex::new(HashMap::new()),
            attributes: AttributesRuntimeState::new(),
            await_waiters: Mutex::new(HashMap::new()),
            await_waiter_index: Mutex::new(HashMap::new()),
            task_waiting_on: Mutex::new(HashMap::new()),
            asyncgen_hooks: Mutex::new(HashMap::new()),
            concurrent: ConcurrentRuntimeState::new(),
            copy_memo: Mutex::new(CopyMemoRuntimeState::new()),
            functools: FunctoolsRuntimeState::new(),
            io: IoRuntimeState::new(),
            modules: ModulesRuntimeState::new(),
            operator: OperatorRuntimeState::new(),
            platform: PlatformRuntimeState::new(),
            types: TypesRuntimeState::new(),
            sys_ext: SysRuntimeState::new(),
            c_api_module: Mutex::new(CApiModuleRuntimeState::new()),
            call_bind: Mutex::new(CallBindRuntimeState::new()),
            stateful_locals: Mutex::new(HashMap::new()),
            weakrefs: Mutex::new(WeakRefRegistry::new()),
            exit_registry: Mutex::new(ExitRegistry::new()),
            abc_invalidation_counter: AtomicU64::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            thread_pool: OnceLock::new(),
            #[cfg(not(target_arch = "wasm32"))]
            thread_registry: OnceLock::new(),
            #[cfg(not(target_arch = "wasm32"))]
            thread_tasks: Mutex::new(HashMap::new()),
            process_registry,
            #[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
            socket_state: SocketRuntimeState::new(),
            signal: SignalRuntimeState::new(),
            process_tasks: Mutex::new(HashMap::new()),
            code_slots: OnceLock::new(),
            start_time: OnceLock::new(),
            vfs_state: OnceLock::new(),
            extension_states: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn scheduler(&self) -> &MoltScheduler {
        let scheduler = self.scheduler.get_or_init(MoltScheduler::new);
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(error) =
            scheduler.start_workers(RuntimeWorkerOrigin::new(self), self.policy().async_workers)
        {
            scheduler.stop();
            panic!("runtime scheduler startup failed: {error}");
        }
        scheduler
    }

    pub(crate) fn sleep_queue(&self) -> &Arc<SleepQueue> {
        let queue = self.sleep_queue.get_or_init(|| Arc::new(SleepQueue::new()));
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(error) = queue.start_worker(RuntimeWorkerOrigin::new(self)) {
            queue.stop();
            panic!("runtime timer startup failed: {error}");
        }
        queue
    }

    pub(crate) fn io_poller(&self) -> &Arc<IoPoller> {
        let poller = self.io_poller.get_or_init(|| Arc::new(IoPoller::new()));
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(error) = poller.start_worker(RuntimeWorkerOrigin::new(self)) {
            poller.stop();
            panic!("runtime I/O startup failed: {error}");
        }
        poller
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn thread_pool(&self) -> &ThreadPool {
        let pool = self.thread_pool.get_or_init(ThreadPool::new);
        if let Err(error) = pool.start_workers(RuntimeWorkerOrigin::new(self)) {
            pool.stop();
            panic!("runtime thread pool startup failed: {error}");
        }
        pool
    }

    /// Return startup routing. An absent mount remains absent even if the
    /// application later changes the process environment.
    pub(crate) fn get_vfs(&self) -> Option<&crate::vfs::VfsState> {
        self.vfs_state
            .get()
            .expect("VFS routing must be frozen before use")
            .as_deref()
    }
}

pub(crate) fn runtime_extension_state_get_or_init(
    state: &RuntimeState,
    key: &[u8],
    init: RuntimeExtensionStateInit,
    clear: RuntimeExtensionStateClear,
    drop: RuntimeExtensionStateDrop,
) -> *mut u8 {
    let mut guard = state.extension_states.lock().unwrap();
    if let Some(slot) = guard.get(key) {
        return slot.ptr;
    }
    let ptr = unsafe { init() };
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    guard.insert(key.to_vec(), RuntimeExtensionStateSlot { ptr, clear, drop });
    ptr
}

pub(crate) fn runtime_extension_states_clear_and_drop(state: &RuntimeState) -> bool {
    crate::gil_assert();
    let slots: Vec<RuntimeExtensionStateSlot> = {
        let mut guard = state.extension_states.lock().unwrap();
        guard.drain().map(|(_, slot)| slot).collect()
    };
    let changed = !slots.is_empty();
    clear_and_drop_extension_slots(slots);
    changed
}

pub(crate) fn runtime_extension_state_clear_and_drop_key(state: &RuntimeState, key: &[u8]) -> bool {
    crate::gil_assert();
    let Some(slot) = state.extension_states.lock().unwrap().remove(key) else {
        return false;
    };
    clear_and_drop_extension_slots(vec![slot]);
    true
}

fn clear_and_drop_extension_slots(slots: Vec<RuntimeExtensionStateSlot>) {
    for slot in slots {
        if slot.ptr.is_null() {
            continue;
        }
        unsafe {
            (slot.clear)(slot.ptr);
            (slot.drop)(slot.ptr);
        }
    }
}

/// The thread that last moved the lifecycle phase, kept for test diagnostics:
/// a lifecycle test that finds the runtime mid-transition names its owner.
#[cfg(test)]
static LIFECYCLE_TRANSITION_THREAD: Mutex<Option<String>> = Mutex::new(None);

#[cfg(test)]
fn note_lifecycle_transition() {
    let name = thread::current().name().unwrap_or("<unnamed>").to_string();
    *LIFECYCLE_TRANSITION_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(name);
}

/// The lifecycle phase and the thread that last moved it.
#[cfg(test)]
pub(crate) fn runtime_lifecycle_test_diagnostic() -> String {
    let phase = *runtime_lifecycle()
        .phase
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mover = LIFECYCLE_TRANSITION_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_default();
    format!("phase {phase:?}, last moved by thread '{mover}'")
}

// `Failed` is terminal after a lifecycle panic caught at a C entrypoint. Abort
// builds stop at the panic, so that phase exists only in unwind builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeLifecyclePhase {
    Uninitialized,
    Initializing {
        owner: thread::ThreadId,
    },
    Ready {
        ptr: usize,
    },
    Finalizing {
        owner: thread::ThreadId,
        ptr: usize,
    },
    Shutdown,
    #[cfg(panic = "unwind")]
    Failed,
}

struct RuntimeLifecycle {
    phase: Mutex<RuntimeLifecyclePhase>,
    changed: Condvar,
}

impl RuntimeLifecycle {
    fn new() -> Self {
        Self {
            phase: Mutex::new(RuntimeLifecyclePhase::Uninitialized),
            changed: Condvar::new(),
        }
    }
}

fn runtime_lifecycle() -> &'static RuntimeLifecycle {
    RUNTIME_LIFECYCLE.get_or_init(RuntimeLifecycle::new)
}

/// Run one lifecycle C entrypoint body under the shared FFI panic dispatch
/// (`molt_runtime_core::with_gil_entry_body!`). `panic = "abort"` builds stop
/// at an invariant panic. `panic = "unwind"` builds (tests, CI, dev) record a
/// terminal lifecycle failure after the body's execution and GIL guards have
/// unwound, then return the zero failure value through C.
macro_rules! runtime_lifecycle_entry {
    ($body:expr) => {
        ::molt_runtime_core::with_gil_entry_body!(raise: |message| {
            runtime_lifecycle_failed(message);
        }, $body)
    };
}

#[cfg(panic = "unwind")]
/// A lifecycle invariant failure is terminal: a partially initialized or
/// retired runtime cannot restart or admit execution. Its allocation stays
/// alive because published native roots may still borrow it; only process
/// exit reclaims it.
fn runtime_lifecycle_failed(message: &str) {
    use std::io::Write;

    RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
    RUNTIME_EXECUTION_ADMISSION
        .fetch_or(RUNTIME_EXECUTION_ADMISSION_CLOSED, AtomicOrdering::AcqRel);
    let lifecycle = runtime_lifecycle();
    // The panic may have unwound through a held phase guard.
    let mut phase = lifecycle
        .phase
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Initialization has not published a process pointer yet. Finalization
    // may already have unpublished it. Preserve the exact current owner before
    // replacing either authority, without reentering the failed runtime.
    let runtime_owner = runtime_state_tls()
        .map(RuntimeState::identity)
        .unwrap_or_else(|| match *phase {
            RuntimeLifecyclePhase::Ready { ptr }
            | RuntimeLifecyclePhase::Finalizing { ptr, .. } => ptr,
            _ => 0,
        });
    if !molt_cpython_abi::api::object::retain_current_thread_state_after_runtime_failure(
        runtime_owner,
    ) {
        // A mismatched or still-borrowed record cannot be safely abandoned for
        // this owner, and this outer C boundary must not unwind a second time.
        let _ = writeln!(
            std::io::stderr(),
            "molt runtime lifecycle failed: {message}; could not retain its C thread-state owner"
        );
        std::process::abort();
    }
    #[cfg(test)]
    note_lifecycle_transition();
    *phase = RuntimeLifecyclePhase::Failed;
    lifecycle.phase.clear_poison();
    lifecycle.changed.notify_all();
    drop(phase);
    clear_thread_runtime_state();
    // eprintln! panics when stderr is closed; nothing may unwind from here.
    let _ = writeln!(
        std::io::stderr(),
        "molt runtime lifecycle failed: {message}"
    );
}

pub(crate) fn runtime_is_initialized() -> bool {
    matches!(
        *runtime_lifecycle().phase.lock().unwrap(),
        RuntimeLifecyclePhase::Ready { .. } | RuntimeLifecyclePhase::Finalizing { .. }
    )
}

pub(crate) fn runtime_is_ready() -> bool {
    runtime_ready_ptr().is_some()
}

const RUNTIME_EXECUTION_ADMISSION_CLOSED: usize = 1usize << (usize::BITS - 1);
const RUNTIME_EXECUTION_COUNT_MASK: usize = !RUNTIME_EXECUTION_ADMISSION_CLOSED;
// A single CAS word classifies independently owned entrants and callbacks whose
// published service JoinHandle must be drained by this RuntimeState. The top
// bit closes both roles; each bounded count has independent overflow checks.
const RUNTIME_SERVICE_COUNT_SHIFT: u32 = usize::BITS / 2;
const RUNTIME_SERVICE_COUNT_UNIT: usize = 1usize << RUNTIME_SERVICE_COUNT_SHIFT;
const RUNTIME_ORDINARY_COUNT_MASK: usize = RUNTIME_SERVICE_COUNT_UNIT - 1;
const RUNTIME_SERVICE_COUNT_MASK: usize =
    RUNTIME_EXECUTION_COUNT_MASK & !RUNTIME_ORDINARY_COUNT_MASK;

/// Reserve one unit without changing thread-local attachment/depth. The same
/// CLOSED/count linearization owns process execution and origin heap borrows.
fn try_acquire_admission_count(admission: &AtomicUsize) -> bool {
    try_acquire_admission_units(admission, 1, RUNTIME_ORDINARY_COUNT_MASK)
}

fn try_acquire_admission_units(admission: &AtomicUsize, unit: usize, mask: usize) -> bool {
    loop {
        let observed = admission.load(AtomicOrdering::Acquire);
        if observed & RUNTIME_EXECUTION_ADMISSION_CLOSED != 0 {
            return false;
        }
        assert_ne!(
            observed & mask,
            mask,
            "runtime lifetime admission count overflow"
        );
        if admission
            .compare_exchange_weak(
                observed,
                observed + unit,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_ok()
        {
            return true;
        }
    }
}

fn release_admission_count(admission: &AtomicUsize) {
    release_admission_units(admission, 1, RUNTIME_ORDINARY_COUNT_MASK);
}

fn release_admission_units(admission: &AtomicUsize, unit: usize, mask: usize) {
    // Valid custody owns one unit in this field. An invalid release is a
    // fail-stop invariant violation, as with the original single-field counter.
    let previous = admission.fetch_sub(unit, AtomicOrdering::AcqRel);
    assert_ne!(
        previous & mask,
        0,
        "runtime lifetime admission count underflow"
    );
}

/// A process lifetime reservation transferable into a not-yet-started native
/// worker. It is acquired before spawn; cancellation drops the same count.
/// It establishes neither GIL custody nor thread-local execution attachment.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RuntimeLifetimeLease {
    runtime_owner: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeLifetimeLease {
    pub(crate) fn try_acquire() -> Option<Self> {
        let expected = runtime_ready_ptr()?;
        if !try_acquire_admission_count(&RUNTIME_EXECUTION_ADMISSION) {
            return None;
        }
        let lease = Self {
            runtime_owner: expected.expose_provenance(),
        };
        if runtime_ready_ptr() == Some(expected) {
            Some(lease)
        } else {
            drop(lease);
            None
        }
    }

    pub(crate) fn identity(&self) -> usize {
        self.runtime_owner
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for RuntimeLifetimeLease {
    fn drop(&mut self) {
        release_admission_count(&RUNTIME_EXECUTION_ADMISSION);
    }
}

/// A borrow of one exact RuntimeState. Callers already own either process
/// admission or a retained native join owner before reading this allocation.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RuntimeOriginBorrow {
    runtime_owner: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeOriginBorrow {
    pub(crate) fn try_acquire(state: &RuntimeState) -> Option<Self> {
        if !try_acquire_admission_count(&state.thread_borrow_admission) {
            return None;
        }
        Some(Self {
            runtime_owner: state.identity(),
        })
    }

    pub(crate) fn state_ptr(&self) -> *mut RuntimeState {
        std::ptr::with_exposed_provenance_mut(self.runtime_owner)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for RuntimeOriginBorrow {
    fn drop(&mut self) {
        let state = unsafe { &*self.state_ptr() };
        release_admission_count(&state.thread_borrow_admission);
    }
}

/// Shared detached workers additionally reserve the process before scheduling.
/// Joined services reuse the same origin borrow only while dispatching work;
/// their existing native join owner retains storage through terminal cleanup.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RuntimeThreadBorrow {
    origin: RuntimeOriginBorrow,
    _process: RuntimeLifetimeLease,
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeThreadBorrow {
    pub(crate) fn try_acquire(py: &PyToken<'_>) -> Option<Self> {
        let process = RuntimeLifetimeLease::try_acquire()?;
        let origin = RuntimeOriginBorrow::try_acquire(runtime_state(py))?;
        Some(Self {
            origin,
            _process: process,
        })
    }

    pub(crate) fn state_ptr(&self) -> *mut RuntimeState {
        self.origin.state_ptr()
    }
}

/// Exact origin transported only by a RuntimeState-owned service JoinHandle.
/// This value is not a lifetime reservation: its owner must stop/join before
/// freeing RuntimeState, and retain that allocation after any failed join.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
pub(crate) struct RuntimeWorkerOrigin {
    runtime_owner: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeWorkerOrigin {
    pub(crate) fn new(state: &RuntimeState) -> Self {
        Self {
            runtime_owner: state.identity(),
        }
    }

    pub(crate) fn identity(self) -> usize {
        self.runtime_owner
    }

    pub(crate) fn state(self) -> &'static RuntimeState {
        unsafe { &*std::ptr::with_exposed_provenance::<RuntimeState>(self.runtime_owner) }
    }

    pub(crate) fn install(self) {
        set_thread_runtime_state(std::ptr::with_exposed_provenance_mut(self.runtime_owner));
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeState {
    #[cfg(test)]
    pub(crate) fn thread_borrow_count(&self) -> usize {
        self.thread_borrow_admission.load(AtomicOrdering::Acquire) & RUNTIME_EXECUTION_COUNT_MASK
    }

    pub(crate) fn thread_borrow_admission_is_open(&self) -> bool {
        self.thread_borrow_admission.load(AtomicOrdering::Acquire)
            & RUNTIME_EXECUTION_ADMISSION_CLOSED
            == 0
    }

    pub(crate) fn close_thread_borrow_admission(&self) {
        self.thread_borrow_admission
            .fetch_or(RUNTIME_EXECUTION_ADMISSION_CLOSED, AtomicOrdering::AcqRel);
    }

    pub(crate) fn assert_thread_borrows_drained(&self) {
        let borrowers = self.thread_borrow_admission.load(AtomicOrdering::Acquire)
            & RUNTIME_EXECUTION_COUNT_MASK;
        assert_eq!(
            borrowers, 0,
            "runtime teardown refused: shared native thread borrowers remain live"
        );
    }
}

// The existing depth word carries one inherited role for its entire stack.
// Depth zero is always untagged; global per-role bounds are smaller than this
// word's count range, so successful admission cannot overflow its TLS count.
pub(crate) const RUNTIME_EXECUTION_SERVICE_ROLE: usize = 1 << (usize::BITS - 1);

/// Acquire lifecycle custody before an execution boundary can proceed toward
/// GIL/thread-state attachment.
///
/// The count is independent of both the GIL and CPython attachment state. That
/// makes the admission -> attachment interval safe for a future free-threaded
/// runtime: shutdown closes the high bit atomically and cannot free RuntimeState
/// while any caller that observed Ready still owns a count unit.
pub(crate) fn try_acquire_runtime_execution_lease(requires_runtime: bool) -> Option<bool> {
    let service =
        crate::state::execution_tls().lease_depth.get() & RUNTIME_EXECUTION_SERVICE_ROLE != 0;
    try_acquire_runtime_execution_lease_kind(requires_runtime, service)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn try_acquire_runtime_service_execution_lease() -> Option<bool> {
    assert!(
        !current_thread_holds_runtime_execution_lease(),
        "service dispatch requires its own outer execution boundary"
    );
    try_acquire_runtime_execution_lease_kind(true, true)
}

fn try_acquire_runtime_execution_lease_kind(requires_runtime: bool, service: bool) -> Option<bool> {
    #[cfg(test)]
    RUNTIME_EXECUTION_LEASE_ACQUIRE_ATTEMPTS.fetch_add(1, AtomicOrdering::Relaxed);
    if runtime_ready_ptr().is_none() {
        let owner = thread::current().id();
        return match *runtime_lifecycle().phase.lock().unwrap() {
            RuntimeLifecyclePhase::Initializing { owner: active }
            | RuntimeLifecyclePhase::Finalizing { owner: active, .. }
                if active == owner =>
            {
                Some(false)
            }
            RuntimeLifecyclePhase::Uninitialized | RuntimeLifecyclePhase::Shutdown
                if !requires_runtime =>
            {
                Some(false)
            }
            _ => None,
        };
    }

    let (unit, mask) = if service {
        (RUNTIME_SERVICE_COUNT_UNIT, RUNTIME_SERVICE_COUNT_MASK)
    } else {
        (1, RUNTIME_ORDINARY_COUNT_MASK)
    };
    if !try_acquire_admission_units(&RUNTIME_EXECUTION_ADMISSION, unit, mask) {
        return None;
    }
    {
        let depth = &crate::state::execution_tls().lease_depth;
        let current = depth.get() & !RUNTIME_EXECUTION_SERVICE_ROLE;
        assert_ne!(
            current,
            RUNTIME_EXECUTION_SERVICE_ROLE - 1,
            "current-thread runtime execution lease depth overflow"
        );
        depth.set(
            (current + 1)
                | if service {
                    RUNTIME_EXECUTION_SERVICE_ROLE
                } else {
                    0
                },
        );
    }
    #[cfg(test)]
    if let Some((entered, release)) = RUNTIME_EXECUTION_LEASE_POST_CAS_TEST_GATE
        .lock()
        .unwrap()
        .clone()
    {
        entered.wait();
        release.wait();
    }
    if runtime_ready_ptr().is_some() {
        return Some(true);
    }
    release_runtime_execution_lease();
    None
}

pub(crate) fn release_runtime_execution_lease() {
    let service = {
        let depth = &crate::state::execution_tls().lease_depth;
        let current = depth.get();
        let count = current & !RUNTIME_EXECUTION_SERVICE_ROLE;
        assert_ne!(count, 0, "current-thread runtime execution lease underflow");
        depth.set(if count == 1 { 0 } else { current - 1 });
        current & RUNTIME_EXECUTION_SERVICE_ROLE != 0
    };
    if service {
        release_admission_units(
            &RUNTIME_EXECUTION_ADMISSION,
            RUNTIME_SERVICE_COUNT_UNIT,
            RUNTIME_SERVICE_COUNT_MASK,
        );
    } else {
        release_admission_count(&RUNTIME_EXECUTION_ADMISSION);
    }
}

pub(crate) fn current_thread_holds_runtime_execution_lease() -> bool {
    crate::state::execution_tls().lease_depth.get() != 0
}

fn close_runtime_execution_admission() -> usize {
    RUNTIME_EXECUTION_ADMISSION.fetch_or(RUNTIME_EXECUTION_ADMISSION_CLOSED, AtomicOrdering::AcqRel)
        & RUNTIME_EXECUTION_COUNT_MASK
}

fn reopen_runtime_execution_admission() {
    let previous =
        RUNTIME_EXECUTION_ADMISSION.fetch_and(RUNTIME_EXECUTION_COUNT_MASK, AtomicOrdering::AcqRel);
    assert_ne!(
        previous & RUNTIME_EXECUTION_ADMISSION_CLOSED,
        0,
        "runtime execution admission reopened while already open"
    );
}

/// Close admission before unpublishing Ready or touching roots. Process exit
/// may inherit this thread's existing execution lease; pending native workers
/// and every other thread remain independent lifetime owners in both modes.
fn close_process_retirement_admission(allow_current_execution: bool, operation: &str) -> bool {
    let counts = close_runtime_execution_admission();
    if crate::state::execution_tls().lease_depth.get() & RUNTIME_EXECUTION_SERVICE_ROLE != 0 {
        eprintln!(
            "molt runtime {operation} refused: joined service callback cannot retire its own runtime"
        );
        reopen_runtime_execution_admission();
        return false;
    }
    let active = counts & RUNTIME_ORDINARY_COUNT_MASK;
    let joined = (counts & RUNTIME_SERVICE_COUNT_MASK) >> RUNTIME_SERVICE_COUNT_SHIFT;
    let current = if allow_current_execution {
        crate::state::execution_tls().lease_depth.get()
    } else {
        0
    };
    assert!(
        active >= current,
        "current execution count exceeds process custody"
    );
    if active != current {
        eprintln!(
            "molt runtime {operation} refused: {} active execution lease(s) remain live",
            active - current
        );
        reopen_runtime_execution_admission();
        return false;
    }
    // A service callback may have suspended its GIL while retaining C state.
    // Its published owner drains it before the foreign-C-owner assertion in
    // runtime teardown. Failure there is terminal and retains original storage;
    // no collector, atexit, or root retirement has run.
    if joined != 0 {
        return true;
    }
    {
        let attachments = molt_cpython_abi::api::object::runtime_execution_attachment_count();
        let own_attachment = usize::from(
            allow_current_execution
                && molt_cpython_abi::api::object::runtime_execution_thread_is_attached(),
        );
        assert!(
            attachments >= own_attachment,
            "current attachment exceeds process custody"
        );
        if attachments != own_attachment {
            eprintln!(
                "molt runtime {operation} refused: {} runtime execution attachment(s) remain live",
                attachments - own_attachment
            );
            reopen_runtime_execution_admission();
            return false;
        }
        let retained = molt_cpython_abi::api::object::runtime_retained_thread_state_count();
        let current =
            usize::from(molt_cpython_abi::api::object::current_thread_has_retained_runtime_state());
        assert!(
            retained >= current,
            "current retained PyThreadState is absent from the global count"
        );
        let foreign = retained - current;
        if foreign != 0 {
            eprintln!(
                "molt runtime {operation} refused: {foreign} foreign retained PyThreadState owner(s) remain live"
            );
            reopen_runtime_execution_admission();
            return false;
        }
    }
    true
}

#[cfg(test)]
pub(crate) fn active_runtime_execution_lease_count() -> usize {
    let counts = RUNTIME_EXECUTION_ADMISSION.load(AtomicOrdering::Acquire);
    (counts & RUNTIME_ORDINARY_COUNT_MASK)
        + ((counts & RUNTIME_SERVICE_COUNT_MASK) >> RUNTIME_SERVICE_COUNT_SHIFT)
}

#[cfg(test)]
static RUNTIME_EXECUTION_LEASE_ACQUIRE_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
static RUNTIME_EXECUTION_LEASE_POST_CAS_TEST_GATE: Mutex<
    Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>,
> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn runtime_execution_lease_acquire_attempts() -> usize {
    RUNTIME_EXECUTION_LEASE_ACQUIRE_ATTEMPTS.load(AtomicOrdering::Relaxed)
}

/// Wait for a lifecycle transition owner before taking the GIL. Ready,
/// Uninitialized, and terminal Shutdown are all stable entry observations;
/// individual operations decide whether they require a live RuntimeState.
pub(crate) fn wait_for_runtime_execution_admission() {
    if runtime_ready_ptr().is_some() {
        return;
    }
    let owner = thread::current().id();
    loop {
        #[cfg(test)]
        wait_for_runtime_test_restart(owner);
        if runtime_ready_ptr().is_some() {
            return;
        }
        let lifecycle = runtime_lifecycle();
        let mut phase = lifecycle.phase.lock().unwrap();
        let transitioning = matches!(
            *phase,
            RuntimeLifecyclePhase::Initializing { owner: active }
                | RuntimeLifecyclePhase::Finalizing { owner: active, .. }
                if active != owner
        );
        if !transitioning {
            return;
        }
        assert!(
            !gil_held(),
            "runtime lifecycle admission wait attempted while holding the GIL"
        );
        phase = lifecycle.changed.wait(phase).unwrap();
        drop(phase);
    }
}

pub(crate) fn runtime_execution_is_admitted_for_current_thread(requires_runtime: bool) -> bool {
    let owner = thread::current().id();
    match *runtime_lifecycle().phase.lock().unwrap() {
        RuntimeLifecyclePhase::Ready { .. } => runtime_ready_ptr().is_some(),
        RuntimeLifecyclePhase::Initializing { owner: active }
        | RuntimeLifecyclePhase::Finalizing { owner: active, .. } => active == owner,
        RuntimeLifecyclePhase::Uninitialized | RuntimeLifecyclePhase::Shutdown => !requires_runtime,
        #[cfg(panic = "unwind")]
        RuntimeLifecyclePhase::Failed => false,
    }
}

#[cfg(test)]
struct RuntimeTestRestart {
    owner: Mutex<Option<thread::ThreadId>>,
    changed: Condvar,
}

#[cfg(test)]
static RUNTIME_TEST_RESTART: OnceLock<RuntimeTestRestart> = OnceLock::new();

#[cfg(test)]
fn runtime_test_restart() -> &'static RuntimeTestRestart {
    RUNTIME_TEST_RESTART.get_or_init(|| RuntimeTestRestart {
        owner: Mutex::new(None),
        changed: Condvar::new(),
    })
}

#[cfg(test)]
pub(crate) fn begin_runtime_test_restart(owner: thread::ThreadId) {
    let restart = runtime_test_restart();
    let mut active = restart.owner.lock().unwrap();
    assert!(
        active.is_none(),
        "runtime test restart owner already published"
    );
    *active = Some(owner);
    restart.changed.notify_all();
}

#[cfg(test)]
pub(crate) fn end_runtime_test_restart(owner: thread::ThreadId) {
    let restart = runtime_test_restart();
    let mut active = restart.owner.lock().unwrap();
    assert_eq!(*active, Some(owner), "runtime test restart owner changed");
    *active = None;
    restart.changed.notify_all();
}

#[cfg(test)]
fn wait_for_runtime_test_restart(owner: thread::ThreadId) {
    let restart = runtime_test_restart();
    let mut active = restart.owner.lock().unwrap();
    while active.is_some_and(|active_owner| active_owner != owner) {
        assert!(
            !gil_held(),
            "runtime lifecycle restart wait attempted while holding the GIL"
        );
        active = restart.changed.wait(active).unwrap();
    }
}

#[inline(always)]
fn runtime_ready_ptr() -> Option<*mut RuntimeState> {
    let ptr = RUNTIME_READY_PTR.load(AtomicOrdering::Acquire);
    if ptr.is_null() { None } else { Some(ptr) }
}

/// Process-static C shells cannot be rebound to an isolated RuntimeState.
/// Resolve ownership from the lifecycle authority, including cold bootstrap
/// and the private finalization owner, rather than a second global owner flag.
pub(crate) fn owns_process_cpython_state(state: &RuntimeState) -> bool {
    let phase = runtime_lifecycle().phase.lock().unwrap();
    match *phase {
        RuntimeLifecyclePhase::Ready { ptr } | RuntimeLifecyclePhase::Finalizing { ptr, .. } => {
            std::ptr::eq(state, ptr as *const RuntimeState)
        }
        RuntimeLifecyclePhase::Initializing { owner } => owner == thread::current().id(),
        RuntimeLifecyclePhase::Uninitialized | RuntimeLifecyclePhase::Shutdown => false,
        #[cfg(panic = "unwind")]
        RuntimeLifecyclePhase::Failed => false,
    }
}

pub(crate) fn runtime_state_for_gil() -> Option<&'static RuntimeState> {
    runtime_state_for_gil_in(crate::state::execution_tls())
}

/// `runtime_state_for_gil` for a caller that has already read this thread's
/// execution facts, such as a runtime entry's GIL guard.
pub(crate) fn runtime_state_for_gil_in(
    tls: &crate::state::ExecutionTls,
) -> Option<&'static RuntimeState> {
    #[cfg(not(target_arch = "wasm32"))]
    if tls.lease_depth.get() & RUNTIME_EXECUTION_SERVICE_ROLE != 0 {
        // An admitted callback may finish after shutdown closes new work. Its
        // published join owner and origin borrow retain this exact allocation.
        return runtime_state_tls_in(tls);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(origin) = tls.retained_worker_origin.get() {
        // Only a worker whose native join owner retains this exact allocation
        // can establish this destruction-only capability during Finalizing.
        return Some(origin.state());
    }
    if let Some(ptr) = runtime_ready_ptr() {
        return runtime_state_tls_in(tls).or_else(|| Some(unsafe { &*ptr }));
    }

    // The initializing owner may recursively enter runtime code before the
    // state is globally ready. Its private TLS pointer is the only permitted
    // view of that unpublished state.
    let lifecycle = runtime_lifecycle();
    let phase = lifecycle.phase.lock().unwrap();
    match *phase {
        RuntimeLifecyclePhase::Ready { ptr } => Some(unsafe { &*(ptr as *mut RuntimeState) }),
        RuntimeLifecyclePhase::Initializing { owner } if owner == thread::current().id() => {
            runtime_state_tls()
        }
        RuntimeLifecyclePhase::Finalizing { owner, ptr } if owner == thread::current().id() => {
            Some(unsafe { &*(ptr as *mut RuntimeState) })
        }
        _ => None,
    }
}

pub(crate) fn runtime_state(py: &PyToken<'_>) -> &'static RuntimeState {
    // While the runtime is ready, the full resolution below returns this
    // thread's own state when it has one (an isolate, which a thread may
    // switch to under a live token) and otherwise the published ready state.
    // A token reuses its earlier answer only while that is still the result;
    // lifecycle edges, where no ready state is published, resolve in full.
    let tls = py.execution_tls();
    if let Some(state) = py.resolved_runtime_state()
        && let Some(ready) = runtime_ready_ptr()
        && std::ptr::eq(
            runtime_state_tls_in(tls).map_or(ready.cast_const(), std::ptr::from_ref),
            state,
        )
    {
        return state;
    }
    crate::state::lifecycle::touch_tls_guard_in(tls);
    let state = match runtime_state_for_gil_in(tls) {
        Some(state) => state,
        None => {
            let _ = molt_runtime_init();
            runtime_state_for_gil()
                .unwrap_or_else(|| panic!("runtime state requested after permanent shutdown"))
        }
    };
    py.remember_runtime_state(state);
    state
}

// ---------------------------------------------------------------------------
// GIL vtable shims — bridge core crate's function-pointer GIL to the real
// mutex-based GIL in this crate.
// ---------------------------------------------------------------------------

extern "C" fn __core_gil_acquire() -> u64 {
    crate::concurrency::RuntimeExecutionGuard::enter().into_encoded_lane()
}

extern "C" fn __core_gil_release(token: u64) {
    // SAFETY: the core guard is !Send/!Sync and calls release exactly once on
    // the same thread with the unmatched token returned by acquire.
    drop(unsafe { crate::concurrency::RuntimeExecutionGuard::from_encoded_lane(token) });
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
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_RUNTIME_INIT)
}

thread_local! {
    /// `(t0, t_prev)` captured at the first `enter` stage so each subsequent
    /// `trace_runtime_init` call can report cumulative elapsed since init began
    /// and the per-phase delta. Reset on every `enter` so a re-entrant init
    /// attempt (the `already_initialized` fast path) times independently rather
    /// than appearing to take the whole prior init's wall time.
    static RUNTIME_INIT_CLOCK: Cell<Option<(Instant, Instant)>> = const { Cell::new(None) };
}

#[inline]
fn trace_runtime_init(stage: &str) {
    if trace_runtime_init_enabled() {
        let now = Instant::now();
        let (t0, t_prev) = RUNTIME_INIT_CLOCK.with(|c| match c.get() {
            Some(v) if stage != "enter" => v,
            _ => (now, now),
        });
        RUNTIME_INIT_CLOCK.with(|c| c.set(Some((t0, now))));
        let total_us = now.duration_since(t0).as_micros();
        let delta_us = now.duration_since(t_prev).as_micros();
        eprintln!("[molt runtime_init] +{total_us:>6}us (d{delta_us:>5}us) {stage}");
    }
}

/// Clean executable process exit.
///
/// Runs Python-level process-exit finalization once, drains native managed TLS
/// while runtime/GIL custody is still live, then calls `_exit` to bypass normal
/// user-level C teardown. Explicit embedding teardown remains
/// `molt_runtime_shutdown()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_exit(code_bits: u64) -> u64 {
    runtime_lifecycle_entry!(runtime_exit(code_bits));
    // runtime_exit normally ends the process. Refused lifetime admission or
    // a caught finalization panic must terminate nonzero without further teardown.
    unsafe { libc::_exit(1) }
}

fn runtime_exit(code_bits: u64) -> u64 {
    // Process-exit attachment is governed by the same prepare -> runtime TLS
    // anchors -> arm sequence as ordinary execution and embedding teardown.
    touch_tls_guard();
    let code = u32::try_from(code_bits).map_or(1, |raw| raw as i32);
    if !PROCESS_EXIT_FINALIZED.swap(true, AtomicOrdering::SeqCst) {
        let gil = GilGuard::new();
        let lifecycle = runtime_lifecycle();
        let owner = thread::current().id();
        let ptr = {
            let mut phase = lifecycle.phase.lock().unwrap();
            match *phase {
                RuntimeLifecyclePhase::Ready { ptr } => {
                    if !close_process_retirement_admission(true, "process exit") {
                        // The outer C entry terminates nonzero without mutating
                        // roots still borrowed by another owner.
                        return 0;
                    }
                    RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
                    #[cfg(test)]
                    note_lifecycle_transition();
                    *phase = RuntimeLifecyclePhase::Finalizing { owner, ptr };
                    lifecycle.changed.notify_all();
                    Some(ptr as *mut RuntimeState)
                }
                _ => None,
            }
        };
        if let Some(ptr) = ptr {
            let state = unsafe { &*ptr };
            let py = gil.token();
            runtime_teardown_for_process_exit(&py, state);
            // Post-teardown TRUE-LEAK gauge (ownership_lattice_phase0.md
            //    §2.4). Teardown above has reclaimed every reachable acyclic
            //    graph and the collector has reclaimed unreachable cycles, so the
            //    only survivors now are the immortal floor + genuine leaks. GIL
            //    still held; reads crate-static counters only, never touches
            //    `state`.
            // The shared teardown transaction has already quiesced runtime/C
            // TLS and retired exact static bindings before class destruction.
            // Do not reenter the runtime in this callback-free process-exit tail.
            crate::object::ops::assert_no_true_leak_post_teardown(&py);
            let mut phase = lifecycle.phase.lock().unwrap();
            assert_eq!(
                *phase,
                RuntimeLifecyclePhase::Finalizing {
                    owner,
                    ptr: ptr as usize,
                }
            );
            #[cfg(test)]
            note_lifecycle_transition();
            *phase = RuntimeLifecyclePhase::Shutdown;
            lifecycle.changed.notify_all();
        }
        drop(gil);
    }
    {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }
    let code = crate::builtins::signal_ext::signal_exit_status_after_finalization(code);
    unsafe { libc::_exit(code) }
}

fn initialize_runtime_state(
    gil: &GilGuard,
    state: &RuntimeState,
    #[cfg(test)] fixture_vfs: Option<Option<Arc<crate::vfs::VfsState>>>,
) {
    trace_runtime_init("state_allocated");
    crate::builtins::platform::initialize_startup_encoding(state);
    crate::builtins::platform::initialize_startup_environment(state);
    trace_runtime_init("startup_encoding");
    // Bootstrap constructs builtin classes and hashes their dictionary keys.
    // Hash-secret selection admits RandomEntropy, so the target and grants must
    // already be frozen before bootstrap enters any Python object operation.
    crate::async_rt::channels::freeze_capabilities(&gil.token());
    trace_runtime_init("capabilities");
    runtime_reset_for_init(&gil.token(), state);
    trace_runtime_init("runtime_reset_for_init");

    // Register synthetic _intrinsics module so stdlib .py files can import it.
    {
        let nested = GilGuard::new();
        crate::intrinsics::registry::register_intrinsics_module(&nested.token());
    }
    trace_runtime_init("intrinsics_registered");

    #[cfg(feature = "stdlib_serial")]
    molt_runtime_serial::bridge::init_vtable();
    trace_runtime_init("serial_vtable");

    #[cfg(feature = "stdlib_itertools")]
    molt_runtime_itertools::bridge::init_vtable();
    trace_runtime_init("itertools_vtable");

    let resources = crate::object::ops_sys::initialize_runtime_resources();
    assert!(
        state.policy.set(RuntimePolicy::capture(resources)).is_ok(),
        "runtime policy initialized twice"
    );
    // I/O fixtures supply real backend owners at this same pre-Ready boundary.
    // Production always captures routes/quotas; payload I/O remains lazy.
    #[cfg(test)]
    let vfs = fixture_vfs.unwrap_or_else(|| crate::vfs::load_vfs().map(Arc::new));
    #[cfg(not(test))]
    let vfs = crate::vfs::load_vfs().map(Arc::new);
    assert!(
        state.vfs_state.set(vfs).is_ok(),
        "VFS routing initialized twice"
    );
    crate::diagnostics::initialize();
    #[cfg(all(not(target_arch = "wasm32"), feature = "molt_tk_native"))]
    molt_runtime_tk::initialize_library_policy();
    trace_runtime_init("resources");
    crate::object::ops_sys::molt_runtime_init_audit();
    trace_runtime_init("audit");
    crate::object::ops_sys::molt_runtime_init_io_mode();
    trace_runtime_init("io_mode");

    crate::builtins::exceptions::memory_error::initialize(&gil.token());

    // Publish the one CPython ABI hook table as part of the runtime
    // initialization transaction. Lifecycle queries such as
    // `Py_IsInitialized` must observe this lifecycle authority immediately
    // after Ready publication; lazily installing the table on an unrelated ABI
    // call leaves the detached stub as a second, false initialization authority.
    let cpython_bootstrap_state =
        molt_cpython_abi::api::object::RuntimeInitializationThreadStateGuard::enter(
            state.identity(),
        );
    assert!(
        crate::cpython_abi_hooks::register_cpython_hooks(),
        "primary runtime C-API bootstrap was rejected"
    );
    assert!(
        crate::builtins::exceptions::memory_error::prepare_c_views(&gil.token()),
        "MemoryError C transport bootstrap failed"
    );
    drop(cpython_bootstrap_state);
    trace_runtime_init("cpython_abi_hooks");

    super::metrics::snapshot_live_floor();
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init() -> u64 {
    runtime_lifecycle_entry!(runtime_init())
}

fn runtime_init() -> u64 {
    runtime_init_impl(
        #[cfg(test)]
        None,
    )
}

/// Real lifecycle initialization with a pre-Ready VFS fixture. Kept out of all
/// shipped images; fixtures cannot mutate the routing slot after publication.
#[cfg(test)]
pub(crate) fn runtime_init_with_vfs_for_testing(vfs: Option<crate::vfs::VfsState>) -> u64 {
    runtime_lifecycle_entry!(runtime_init_impl(Some(vfs.map(Arc::new))))
}

fn runtime_init_impl(#[cfg(test)] fixture_vfs: Option<Option<Arc<crate::vfs::VfsState>>>) -> u64 {
    #[cfg(target_arch = "wasm32")]
    ensure_wasm_ctors();
    // The GIL authority is process-lifetime state and must precede every
    // extracted-crate init callback; RuntimeState publication remains later.
    // SAFETY: `CORE_GIL_VT` is process-static and is the sole encoded GIL
    // acquire/release authority for every extracted runtime crate.
    unsafe { molt_runtime_core::set_gil_vtable(&CORE_GIL_VT) };
    // Every embedding thread still establishes the runtime's TLS destructor
    // boundary. Once Ready is published, however, initialization diagnostics,
    // environment policy, and signal-handler setup are immutable process
    // state: re-reading their environment variables allocated four times per
    // otherwise no-op call and made the canonical test transaction pay that
    // cost at every boundary.
    touch_tls_guard();
    if runtime_ready_ptr().is_some() {
        return 1;
    }
    // The first flag query reads every runtime flag (`runtime_flags`), so
    // user code cannot change them through `os.environ`.
    trace_runtime_init("enter");
    super::metrics::init_profile_enabled_from_env();
    #[cfg(not(target_arch = "wasm32"))]
    ensure_debug_sigtrap_handler();
    if runtime_ready_ptr().is_some() {
        trace_runtime_init("already_initialized");
        return 1;
    }
    let owner = thread::current().id();
    loop {
        #[cfg(test)]
        wait_for_runtime_test_restart(owner);
        let gil = GilGuard::new();
        let lifecycle = runtime_lifecycle();
        let mut phase = lifecycle.phase.lock().unwrap();
        match *phase {
            RuntimeLifecyclePhase::Ready { .. } => {
                trace_runtime_init("already_initialized");
                return 1;
            }
            RuntimeLifecyclePhase::Shutdown => {
                trace_runtime_init("shutdown_complete");
                return 0;
            }
            #[cfg(panic = "unwind")]
            RuntimeLifecyclePhase::Failed => {
                trace_runtime_init("lifecycle_failed");
                return 0;
            }
            RuntimeLifecyclePhase::Finalizing { owner: active, .. } if active == owner => {
                trace_runtime_init("recursive_finalizing");
                return 0;
            }
            RuntimeLifecyclePhase::Finalizing { .. } => {
                drop(phase);
                drop(gil);
                let mut phase = lifecycle.phase.lock().unwrap();
                while matches!(*phase, RuntimeLifecyclePhase::Finalizing { .. }) {
                    phase = lifecycle.changed.wait(phase).unwrap();
                }
                continue;
            }
            RuntimeLifecyclePhase::Initializing { owner: active } if active == owner => {
                // Recursive initialization by the owner observes its private
                // TLS state but does not publish it to other threads.
                return u64::from(runtime_state_tls().is_some());
            }
            RuntimeLifecyclePhase::Initializing { .. } => {
                drop(phase);
                drop(gil);
                let mut phase = lifecycle.phase.lock().unwrap();
                while matches!(*phase, RuntimeLifecyclePhase::Initializing { .. }) {
                    phase = lifecycle.changed.wait(phase).unwrap();
                }
                continue;
            }
            RuntimeLifecyclePhase::Uninitialized => {
                // Main-thread custody is part of the same winning lifecycle
                // transaction as RuntimeState initialization. A losing caller
                // must never publish a competing pending-call identity.
                assert!(
                    molt_cpython_abi::api::pending_calls::register_main_thread(owner),
                    "runtime initialization attempted to transfer process-main custody"
                );
                #[cfg(test)]
                note_lifecycle_transition();
                *phase = RuntimeLifecyclePhase::Initializing { owner };
            }
        }
        drop(phase);

        // Retain custody even if initialization unwinds after installing private
        // TLS/native roots. The terminal Failed boundary forbids reuse and
        // deliberately leaves this allocation alive instead of dangling them.
        let state_ptr = Box::into_raw(Box::new(RuntimeState::new()));
        let state = unsafe { &*state_ptr };
        crate::object::gc::gc_bind_registry(state);
        set_thread_runtime_state(state_ptr);
        #[cfg(test)]
        RUNTIME_INIT_TEST_PANIC.with(|pending| {
            if pending.replace(false) {
                std::panic::resume_unwind(Box::new("injected unpublished runtime init panic"));
            }
        });
        initialize_runtime_state(
            &gil,
            state,
            #[cfg(test)]
            fixture_vfs,
        );

        #[cfg(test)]
        if let Some((entered, release)) = RUNTIME_INIT_TEST_GATE.lock().unwrap().clone() {
            entered.wait();
            release.wait();
        }

        let ptr = state_ptr;
        assert!(
            signal_runtime_state_publish(unsafe { &*ptr }),
            "runtime initialization could not publish its signal authority"
        );
        let mut phase = lifecycle.phase.lock().unwrap();
        assert_eq!(*phase, RuntimeLifecyclePhase::Initializing { owner });
        #[cfg(test)]
        note_lifecycle_transition();
        *phase = RuntimeLifecyclePhase::Ready { ptr: ptr as usize };
        RUNTIME_READY_PTR.store(ptr, AtomicOrdering::Release);
        let previous = RUNTIME_EXECUTION_ADMISSION.swap(0, AtomicOrdering::AcqRel);
        assert_eq!(
            previous, RUNTIME_EXECUTION_ADMISSION_CLOSED,
            "runtime publication found live or already-open execution admission"
        );
        clear_thread_runtime_state();
        lifecycle.changed.notify_all();
        trace_runtime_init("ok");
        return 1;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_ensure_gil() {
    runtime_lifecycle_entry!(crate::concurrency::ensure_persistent_runtime_execution())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_shutdown() -> u64 {
    runtime_lifecycle_entry!(runtime_shutdown())
}

fn runtime_shutdown() -> u64 {
    // Establish the canonical TLS destructor boundary before shutdown touches
    // any other runtime TLS on this embedding thread.
    touch_tls_guard();
    let gil = GilGuard::new();
    let lifecycle = runtime_lifecycle();
    let owner = thread::current().id();
    let mut phase = lifecycle.phase.lock().unwrap();
    let ptr = match *phase {
        RuntimeLifecyclePhase::Ready { ptr } => ptr as *mut RuntimeState,
        RuntimeLifecyclePhase::Finalizing { owner: active, .. } if active == owner => return 0,
        RuntimeLifecyclePhase::Finalizing { .. } => {
            drop(phase);
            drop(gil);
            let mut phase = lifecycle.phase.lock().unwrap();
            while matches!(*phase, RuntimeLifecyclePhase::Finalizing { .. }) {
                phase = lifecycle.changed.wait(phase).unwrap();
            }
            return 0;
        }
        RuntimeLifecyclePhase::Uninitialized
        | RuntimeLifecyclePhase::Initializing { .. }
        | RuntimeLifecyclePhase::Shutdown => return 0,
        #[cfg(panic = "unwind")]
        RuntimeLifecyclePhase::Failed => return 0,
    };
    debug_assert_eq!(runtime_ready_ptr(), Some(ptr));
    if !close_process_retirement_admission(false, "shutdown") {
        return 0;
    }
    RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
    #[cfg(test)]
    note_lifecycle_transition();
    *phase = RuntimeLifecyclePhase::Finalizing {
        owner,
        ptr: ptr as usize,
    };
    lifecycle.changed.notify_all();
    drop(phase);

    // The shared teardown transaction owns any new C attachment from the
    // first callback through success or unwind. Prepare TLS lifetime here;
    // preserve an existing caller-owned attachment for that caller to revoke.
    touch_tls_guard();

    let state = unsafe { &*ptr };
    let py = gil.token();
    runtime_teardown(&py, state);
    {
        // Callback-capable teardown belongs before static-class retirement,
        // never here after their builtin graph and singleton pools are gone.
        assert!(!molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            0,
            "retained PyThreadState survived runtime teardown"
        );
    }
    // Clear the teardown owner's private cache before freeing the state. The
    // public ready projection was already unpublished at the Finalizing edge.
    clear_thread_runtime_state();
    unsafe {
        drop(Box::from_raw(ptr));
    }
    let mut phase = lifecycle.phase.lock().unwrap();
    assert_eq!(
        *phase,
        RuntimeLifecyclePhase::Finalizing {
            owner,
            ptr: ptr as usize,
        }
    );
    #[cfg(test)]
    note_lifecycle_transition();
    *phase = RuntimeLifecyclePhase::Shutdown;
    lifecycle.changed.notify_all();
    1
}

/// Read-optimized projection of the canonical lifecycle phase. It is non-null
/// exactly while the lifecycle is `Ready`.
static RUNTIME_READY_PTR: AtomicPtr<RuntimeState> = AtomicPtr::new(std::ptr::null_mut());
static RUNTIME_EXECUTION_ADMISSION: AtomicUsize =
    AtomicUsize::new(RUNTIME_EXECUTION_ADMISSION_CLOSED);
static RUNTIME_LIFECYCLE: OnceLock<RuntimeLifecycle> = OnceLock::new();
static PROCESS_EXIT_FINALIZED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static RUNTIME_INIT_TEST_GATE: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>> =
    Mutex::new(None);
#[cfg(test)]
thread_local! {
    static RUNTIME_INIT_TEST_PANIC: Cell<bool> = const { Cell::new(false) };
}
#[cfg(test)]
struct RuntimeFinalizeAtexitTestHook {
    results: std::sync::mpsc::Sender<(u64, u64)>,
    entered: std::sync::mpsc::Sender<()>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(test)]
static RUNTIME_FINALIZE_ATEXIT_TEST_HOOK: Mutex<Option<RuntimeFinalizeAtexitTestHook>> =
    Mutex::new(None);

#[cfg(test)]
pub(crate) fn run_finalizing_atexit_test_hook() {
    let hook = RUNTIME_FINALIZE_ATEXIT_TEST_HOOK.lock().unwrap().take();
    if let Some(hook) = hook {
        let recursive_init = molt_runtime_init();
        let recursive_shutdown = molt_runtime_shutdown();
        hook.results
            .send((recursive_init, recursive_shutdown))
            .unwrap();
        hook.entered.send(()).unwrap();
        hook.release.wait();
    }
}

fn runtime_state_tls() -> Option<&'static RuntimeState> {
    runtime_state_tls_in(crate::state::execution_tls())
}

/// The runtime state this thread attached to. The execution facts have no
/// destructor, so this stays readable while thread-locals are destroyed.
#[inline(always)]
fn runtime_state_tls_in(tls: &crate::state::ExecutionTls) -> Option<&'static RuntimeState> {
    let ptr = tls.runtime_state.get();
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

pub(crate) fn set_thread_runtime_state(ptr: *mut RuntimeState) {
    crate::state::execution_tls().runtime_state.set(ptr);
}

pub(crate) fn clear_thread_runtime_state() {
    crate::state::execution_tls()
        .runtime_state
        .set(std::ptr::null_mut());
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
/// 1. `RuntimeTestTransaction` ensures no concurrent runtime access.
/// 2. The previous runtime has been fully shut down via
///    `molt_runtime_shutdown()`.
/// 3. The caller will immediately re-initialize via `molt_runtime_init()`.
#[cfg(test)]
pub(crate) fn molt_runtime_reset_for_testing() {
    assert_eq!(
        RUNTIME_EXECUTION_ADMISSION.load(AtomicOrdering::Acquire),
        RUNTIME_EXECUTION_ADMISSION_CLOSED,
        "runtime test reset requires closed admission with no active execution leases"
    );
    let lifecycle = runtime_lifecycle();
    let mut phase = lifecycle.phase.lock().unwrap();
    #[cfg(panic = "unwind")]
    assert_ne!(
        *phase,
        RuntimeLifecyclePhase::Failed,
        "a failed runtime cannot be reset: partial native roots remain live"
    );
    assert!(
        matches!(
            *phase,
            RuntimeLifecyclePhase::Uninitialized | RuntimeLifecyclePhase::Shutdown
        ),
        "runtime test reset requires completed teardown"
    );
    assert!(
        runtime_ready_ptr().is_none(),
        "runtime test reset found a published owner"
    );
    #[cfg(test)]
    note_lifecycle_transition();
    *phase = RuntimeLifecyclePhase::Uninitialized;
    lifecycle.changed.notify_all();
    drop(phase);

    // Clear the TLS cache so no stale pointer is returned by
    // `runtime_state_tls()`.
    clear_thread_runtime_state();

    // Clear the intrinsic registry's one-shot flags so the next init can
    // re-register intrinsics into a fresh builtins module.  Without this,
    // BUILTINS_MODULE_PTR holds a dangling pointer to the destroyed module
    // and the manifest publication state prevents re-setting the manifest.
    crate::intrinsics::registry::reset_for_testing();
}

#[cfg(test)]
mod tests {

    struct PolicyEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl PolicyEnvironment {
        fn replace(entries: &[(&'static str, Option<&str>)]) -> Self {
            let saved = entries
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect();
            for (key, value) in entries {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
            Self(saved)
        }
    }
    impl Drop for PolicyEnvironment {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn runtime_policy_freezes_before_first_consumers_and_keeps_absent_vfs_absent() {
        use super::*;
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            let _environment = PolicyEnvironment::replace(&[
                ("MOLT_STREAM_MAX_QUEUED_BYTES", Some("65536")),
                ("MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES", Some("131072")),
                (RuntimeFlag::MOLT_THREAD_ISOLATED.name(), Some("1")),
                ("MOLT_ASYNC_THREADS", Some("0")),
                ("MOLT_CHILD_RLIMIT_GB", Some("0.5")),
                ("MOLT_EXCEPTION_LOG_PATH", None),
                (RuntimeFlag::MOLT_RUNTIME_FEEDBACK.name(), Some("0")),
                ("MOLT_RUNTIME_FEEDBACK_FILE", None),
                ("MOLT_VFS_BUNDLE", None),
                // An explicit numeric seed bypasses bootstrap's operation
                // admission. Leave it absent to exercise the real first use.
                ("PYTHONHASHSEED", None),
                ("MOLT_CAPABILITY_TIER", Some("none")),
                ("MOLT_CAPABILITIES", Some("fs.read")),
                ("MOLT_EXECUTION_TARGET", Some("wasi")),
            ]);
            assert_eq!(molt_runtime_init(), 1);
            unsafe {
                std::env::set_var("MOLT_STREAM_MAX_QUEUED_BYTES", "1073741824");
                std::env::set_var("MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES", "1073741824");
                std::env::set_var(RuntimeFlag::MOLT_THREAD_ISOLATED.name(), "false");
                std::env::set_var("MOLT_ASYNC_THREADS", "999");
                std::env::set_var("MOLT_CHILD_RLIMIT_GB", "0");
                std::env::set_var("MOLT_EXCEPTION_LOG_PATH", "unauthorized-exception-output");
                std::env::set_var(RuntimeFlag::MOLT_RUNTIME_FEEDBACK.name(), "1");
                std::env::set_var("MOLT_RUNTIME_FEEDBACK_FILE", "unauthorized-feedback-output");
                std::env::set_var("MOLT_VFS_BUNDLE", "late-untrusted-bundle");
                std::env::set_var("MOLT_CAPABILITY_TIER", "full");
                std::env::set_var("MOLT_CAPABILITIES", "random");
                std::env::set_var("MOLT_EXECUTION_TARGET", "invalid-late-target");
            }
            assert_eq!(
                molt_runtime_init(),
                1,
                "Ready initialization keeps its policy"
            );
            let gil = GilGuard::new();
            let py = gil.token();
            let state = runtime_state(&py);
            assert!(!crate::is_trusted(&py));
            assert!(crate::operation_allowed(
                &py,
                crate::OperationId::IoOpenRead,
                crate::audit::AuditArgs::None,
            ));
            assert!(!crate::operation_allowed(
                &py,
                crate::OperationId::RandomEntropy,
                crate::audit::AuditArgs::None,
            ));
            // The no-random startup grant selects the zero secret. Reuse the
            // independent CPython siphash13 seed=0 'abc' vector through the
            // runtime consumer, after late environment changes could reseed it.
            assert_eq!(
                crate::object::ops_hash::hash_string_bytes(&py, b"abc"),
                if molt_cpython_abi::abi_types::Py_hash_t::BITS == 64 {
                    -4594863902769663758
                } else {
                    69611762
                },
            );
            assert_eq!(state.policy().stream_max_queued_bytes, 65536);
            #[cfg(not(target_arch = "wasm32"))]
            {
                assert_eq!(state.policy().process_pipe_max_queued_bytes, 131072);
                assert_eq!(state.policy().async_workers, 0);
            }
            assert!(state.policy().thread_isolated);
            assert!(state.policy().exception_log_path.is_none());
            assert!(state.policy().feedback_path.is_none());
            assert!(
                state.get_vfs().is_none(),
                "first VFS access must not read the mutated environment"
            );
            #[cfg(unix)]
            assert_ne!(state.policy().child_memory_rlimit_bytes, None);
            drop(gil);
            assert_eq!(molt_runtime_shutdown(), 1);
        });
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn runtime_vfs_freezes_route_and_quota_without_loading_payload() {
        use super::*;
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            let root = std::env::temp_dir().join(format!(
                "molt-policy-vfs-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let original = root.join("original");
            let replacement = root.join("replacement");
            let original_text = original.to_str().unwrap();
            let _environment = PolicyEnvironment::replace(&[
                ("MOLT_VFS_BUNDLE", Some(original_text)),
                ("MOLT_VFS_BUNDLE_MAX_BYTES", Some("4")),
                ("MOLT_VFS_BUNDLE_MAX_ENTRY_BYTES", Some("4")),
                ("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS", Some("0")),
            ]);
            // Neither source exists yet. Startup may capture routes, not load it.
            assert_eq!(molt_runtime_init(), 1);
            std::fs::create_dir_all(&original).unwrap();
            std::fs::create_dir_all(&replacement).unwrap();
            std::fs::write(original.join("data"), b"oversize").unwrap();
            std::fs::write(replacement.join("data"), b"safe").unwrap();
            unsafe {
                std::env::set_var("MOLT_VFS_BUNDLE", &replacement);
                std::env::set_var("MOLT_VFS_BUNDLE_MAX_BYTES", "9999999");
                std::env::set_var("MOLT_VFS_BUNDLE_MAX_ENTRY_BYTES", "9999999");
                std::env::set_var("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS", "1");
            }
            let gil = GilGuard::new();
            let state = runtime_state(&gil.token());
            let vfs = state.get_vfs().expect("startup bundle owns its route");
            let (_, backend, relative) = vfs.resolve("/bundle/data").expect("bundle route");
            assert!(matches!(
                backend.open_read(&relative),
                Err(crate::vfs::VfsError::QuotaExceeded)
            ));
            drop(gil);
            assert_eq!(molt_runtime_shutdown(), 1);
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle and CWD; run in isolation"]
    fn runtime_relative_vfs_route_keeps_startup_cwd_until_first_use() {
        use super::*;
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            struct RestoreCwd(std::path::PathBuf);
            impl Drop for RestoreCwd {
                fn drop(&mut self) {
                    std::env::set_current_dir(&self.0).unwrap();
                }
            }
            let cwd = RestoreCwd(std::env::current_dir().unwrap());
            let root = std::env::temp_dir().join(format!(
                "molt-relative-vfs-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let original = root.join("original");
            let replacement = root.join("replacement");
            std::fs::create_dir_all(original.join("bundle")).unwrap();
            std::fs::create_dir_all(replacement.join("bundle")).unwrap();
            std::fs::write(original.join("bundle/data"), b"operator-source").unwrap();
            std::fs::write(replacement.join("bundle/data"), b"redirected-source").unwrap();
            let _environment = PolicyEnvironment::replace(&[
                ("MOLT_VFS_BUNDLE", Some("bundle")),
                ("MOLT_VFS_BUNDLE_MAX_BYTES", Some("1024")),
                ("MOLT_VFS_BUNDLE_MAX_ENTRY_BYTES", Some("1024")),
                ("MOLT_VFS_BUNDLE_UNSAFE_FOLLOW_HOST_LINKS", Some("0")),
            ]);
            std::env::set_current_dir(&original).unwrap();
            assert_eq!(molt_runtime_init(), 1);
            std::env::set_current_dir(&replacement).unwrap();
            let gil = GilGuard::new();
            let vfs = runtime_state(&gil.token()).get_vfs().unwrap();
            let (_, backend, relative) = vfs.resolve("/bundle/data").unwrap();
            assert_eq!(backend.open_read(&relative).unwrap(), b"operator-source");
            drop(gil);
            assert_eq!(molt_runtime_shutdown(), 1);
            drop(cwd);
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    #[ignore = "mutates the process-global runtime lifecycle and CWD; run in isolation"]
    fn runtime_output_sinks_keep_startup_cwd_until_first_write() {
        use super::*;
        unsafe extern "C" {
            fn molt_exception_report_uncaught(exc_bits: u64) -> u64;
        }
        // Cover the default feedback filename as well as an explicit relative
        // destination whose parent must remain lazy until the first emission.
        for feedback in [None, Some("feedback/data.json")] {
            crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
                struct RestoreCwd(std::path::PathBuf);
                impl Drop for RestoreCwd {
                    fn drop(&mut self) {
                        std::env::set_current_dir(&self.0).unwrap();
                    }
                }
                let cwd = RestoreCwd(std::env::current_dir().unwrap());
                let root = std::env::temp_dir().join(format!(
                    "molt-output-policy-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                let original = root.join("original");
                let replacement = root.join("replacement");
                std::fs::create_dir_all(&original).unwrap();
                std::fs::create_dir_all(&replacement).unwrap();
                std::env::set_current_dir(&original).unwrap();
                let _environment = PolicyEnvironment::replace(&[
                    ("MOLT_EXCEPTION_LOG_PATH", Some("exception.log")),
                    (RuntimeFlag::MOLT_RUNTIME_FEEDBACK.name(), Some("1")),
                    ("MOLT_RUNTIME_FEEDBACK_FILE", feedback),
                ]);
                assert_eq!(molt_runtime_init(), 1);
                assert_eq!(
                    std::fs::read_dir(&original).unwrap().count(),
                    0,
                    "capturing output policy must not create files or directories"
                );
                std::env::set_current_dir(&replacement).unwrap();
                let gil = GilGuard::new();
                let py = gil.token();
                crate::object::ops_convert::maybe_emit_runtime_feedback_file(
                    &py,
                    &serde_json::json!({"startup_sink_canary": 73}),
                );
                let exception = crate::alloc_exception(&py, "RuntimeError", "startup sink canary");
                assert!(!exception.is_null());
                let bits = MoltObject::from_ptr(exception).bits();
                assert_eq!(unsafe { molt_exception_report_uncaught(bits) }, 1);
                crate::dec_ref_bits(&py, bits);
                let feedback_path = original.join(feedback.unwrap_or("molt_runtime_feedback.json"));
                let payload: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(feedback_path).unwrap()).unwrap();
                assert_eq!(payload["startup_sink_canary"], 73);
                assert!(
                    std::fs::read_to_string(original.join("exception.log"))
                        .unwrap()
                        .contains("startup sink canary")
                );
                assert_eq!(
                    std::fs::read_dir(&replacement).unwrap().count(),
                    0,
                    "later chdir must not retarget either output sink"
                );
                drop(gil);
                assert_eq!(molt_runtime_shutdown(), 1);
                drop(cwd);
                std::fs::remove_dir_all(root).unwrap();
            });
        }
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "mutates process-global CWD; run in isolation"]
    fn relative_output_sink_capture_failure_disables_without_fallback() {
        use super::RuntimePolicy;
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            struct RestoreCwd(std::path::PathBuf);
            impl Drop for RestoreCwd {
                fn drop(&mut self) {
                    std::env::set_current_dir(&self.0).unwrap();
                }
            }
            let _cwd = RestoreCwd(std::env::current_dir().unwrap());
            let root = std::env::temp_dir().join(format!(
                "molt-unlinked-output-cwd-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            std::env::set_current_dir(&root).unwrap();
            std::fs::remove_dir(&root).unwrap();
            assert!(
                std::env::current_dir().is_err(),
                "oracle requires a lost CWD basis"
            );
            assert!(
                RuntimePolicy::capture_output_path("test optional sink", "relative.json".into())
                    .is_none()
            );
            let absolute = root.join("absolute.json");
            assert_eq!(
                RuntimePolicy::capture_output_path("test absolute sink", absolute.clone()),
                Some(absolute)
            );
            assert!(!root.exists(), "capture must not create output directories");
        });
    }

    #[test]
    fn admission_roles_do_not_carry_borrow_or_reopen_each_other() {
        use super::*;
        let word = AtomicUsize::new(3 * RUNTIME_SERVICE_COUNT_UNIT + 2);
        assert!(try_acquire_admission_count(&word));
        assert_eq!(
            word.load(AtomicOrdering::Acquire),
            3 * RUNTIME_SERVICE_COUNT_UNIT + 3
        );
        assert!(try_acquire_admission_units(
            &word,
            RUNTIME_SERVICE_COUNT_UNIT,
            RUNTIME_SERVICE_COUNT_MASK
        ));
        assert_eq!(
            word.load(AtomicOrdering::Acquire),
            4 * RUNTIME_SERVICE_COUNT_UNIT + 3
        );
        word.fetch_or(RUNTIME_EXECUTION_ADMISSION_CLOSED, AtomicOrdering::AcqRel);
        assert!(!try_acquire_admission_count(&word));
        assert!(!try_acquire_admission_units(
            &word,
            RUNTIME_SERVICE_COUNT_UNIT,
            RUNTIME_SERVICE_COUNT_MASK
        ));
        release_admission_units(
            &word,
            RUNTIME_SERVICE_COUNT_UNIT,
            RUNTIME_SERVICE_COUNT_MASK,
        );
        release_admission_count(&word);
        assert_eq!(
            word.load(AtomicOrdering::Acquire),
            RUNTIME_EXECUTION_ADMISSION_CLOSED + 3 * RUNTIME_SERVICE_COUNT_UNIT + 2
        );
        for (unit, mask, initial) in [
            (
                1,
                RUNTIME_ORDINARY_COUNT_MASK,
                RUNTIME_ORDINARY_COUNT_MASK + RUNTIME_SERVICE_COUNT_UNIT,
            ),
            (
                RUNTIME_SERVICE_COUNT_UNIT,
                RUNTIME_SERVICE_COUNT_MASK,
                RUNTIME_SERVICE_COUNT_MASK + 1,
            ),
        ] {
            let full = AtomicUsize::new(initial);
            assert!(
                std::panic::catch_unwind(|| try_acquire_admission_units(&full, unit, mask))
                    .is_err()
            );
            assert_eq!(
                full.load(AtomicOrdering::Acquire),
                initial,
                "overflow changed another role"
            );
            let empty_role = initial & !mask;
            let empty = AtomicUsize::new(empty_role);
            assert!(
                std::panic::catch_unwind(|| release_admission_units(&empty, unit, mask)).is_err()
            );
            // Invalid custody is fail-stop; do not reuse the corrupted word.
            // Valid cross-role decrement independence was asserted above.
        }
    }

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Every lifecycle C entrypoint converts an invariant panic into a terminal
    /// failure instead of unwinding through C (which aborts the process and
    /// hides every later result). Each mode runs in a child process because the
    /// failed lifecycle is process-terminal by design.
    #[cfg(panic = "unwind")]
    #[test]
    fn lifecycle_ffi_panics_fail_closed_without_unwinding() {
        const MODE: &str = "MOLT_TEST_LIFECYCLE_FFI_PANIC";
        static RETAINED_CAPSULE_DROPS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C" fn retained_capsule_drop(
            _capsule: *mut molt_cpython_abi::abi_types::PyObject,
        ) {
            RETAINED_CAPSULE_DROPS.fetch_add(1, Ordering::SeqCst);
        }
        if let Ok(mode) = std::env::var(MODE) {
            std::panic::set_hook(Box::new(|_| {}));
            match mode.as_str() {
                "init" => {
                    RUNTIME_INIT_TEST_PANIC.with(|pending| pending.set(true));
                    assert_eq!(molt_runtime_init(), 0);
                }
                "shutdown" => {
                    assert_eq!(molt_runtime_init(), 1);
                    crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                    assert_eq!(molt_runtime_shutdown(), 0);
                }
                "exit" => {
                    assert_eq!(molt_runtime_init(), 1);
                    crate::concurrency::execution::inject_shutdown_drain_drop_panic();
                    molt_runtime_exit(0);
                    unreachable!("process exit returned");
                }
                "shutdown-thread-state" | "exit-thread-state" => {
                    use molt_cpython_abi::api::{capsule, mapping, object, refcount, sys};
                    let process_exit = mode == "exit-thread-state";
                    // The native thread must actually exit after Failed. A
                    // record left in TLS would try to reacquire dead runtime
                    // custody there and abort instead of joining successfully.
                    let (owner, address, id, retained) = std::thread::spawn(move || {
                        assert_eq!(molt_runtime_init(), 1);
                        let (owner, address, id, retained) = {
                            let gil = GilGuard::new();
                            let py = gil.token();
                            let state = runtime_state(&py);
                            let owner = state.identity();
                            assert!(!object::current_thread_has_retained_runtime_state());
                            assert!(object::retain_current_thread_state_after_runtime_failure(0));
                            assert!(!object::current_thread_has_retained_runtime_state());
                            let retained = Arc::downgrade(state.sleep_queue());
                            object::attach_runtime_execution_thread(owner);
                            let tstate = unsafe { object::_PyThreadState_UncheckedGet() };
                            assert!(!tstate.is_null());
                            let id = unsafe { object::PyThreadState_GetID(tstate) };
                            let address = tstate.expose_provenance();
                            unsafe {
                                let dict = sys::PyThreadState_GetDict();
                                assert!(!dict.is_null());
                                let new_payload = || {
                                    capsule::PyCapsule_New(
                                        std::ptr::NonNull::<u8>::dangling().as_ptr().cast(),
                                        std::ptr::null(),
                                        Some(retained_capsule_drop),
                                    )
                                };
                                // Prove the callback observation before leaving
                                // the real payload owned only by the C dict.
                                let probe = new_payload();
                                assert!(!probe.is_null());
                                refcount::Py_DECREF(probe);
                                assert_eq!(RETAINED_CAPSULE_DROPS.load(Ordering::SeqCst), 1);
                                RETAINED_CAPSULE_DROPS.store(0, Ordering::SeqCst);
                                let payload = new_payload();
                                assert!(!payload.is_null());
                                assert_eq!(
                                    mapping::PyDict_SetItemString(
                                        dict,
                                        c"failed_owner_payload".as_ptr(),
                                        payload,
                                    ),
                                    0
                                );
                                refcount::Py_DECREF(payload);
                            }
                            let owners = object::runtime_retained_thread_state_count_for(owner);
                            assert!(!object::retain_current_thread_state_after_runtime_failure(
                                owner.wrapping_add(1)
                            ));
                            assert_eq!(
                                unsafe { object::_PyThreadState_UncheckedGet() },
                                tstate,
                                "a different failed owner must not steal the record"
                            );
                            assert_eq!(
                                object::runtime_retained_thread_state_count_for(owner),
                                owners
                            );
                            assert_eq!(
                                object::retained_thread_state_identity_for_test(address),
                                Some((owner, id))
                            );
                            object::detach_runtime_execution_thread();
                            (owner, address, id, retained)
                        };
                        assert_eq!(RETAINED_CAPSULE_DROPS.load(Ordering::SeqCst), 0);
                        object::inject_thread_state_finalizer_panic_for_test();
                        if process_exit {
                            use std::io::Write;
                            println!("lifecycle thread-state failure armed: exit-thread-state");
                            std::io::stdout().flush().unwrap();
                            molt_runtime_exit(0);
                            unreachable!("process exit returned");
                        }
                        assert_eq!(molt_runtime_shutdown(), 0);
                        assert_eq!(
                            *runtime_lifecycle().phase.lock().unwrap(),
                            RuntimeLifecyclePhase::Failed
                        );
                        assert!(!object::current_thread_has_retained_runtime_state());
                        assert_eq!(object::runtime_retained_thread_state_count_for(owner), 1);
                        // Repetition must neither forget twice nor fabricate a
                        // replacement state for the terminal runtime.
                        assert!(object::retain_current_thread_state_after_runtime_failure(
                            owner
                        ));
                        assert!(!object::current_thread_has_retained_runtime_state());
                        assert_eq!(object::runtime_retained_thread_state_count_for(owner), 1);
                        assert_eq!(RETAINED_CAPSULE_DROPS.load(Ordering::SeqCst), 0);
                        (owner, address, id, retained)
                    })
                    .join()
                    .expect("failed runtime owner survives actual native TLS destruction");
                    assert!(
                        retained.upgrade().is_some(),
                        "failed RuntimeState stays alive"
                    );
                    assert_eq!(object::runtime_retained_thread_state_count(), 1);
                    assert_eq!(object::runtime_retained_thread_state_count_for(owner), 1);
                    assert_eq!(
                        object::retained_thread_state_identity_for_test(address),
                        Some((owner, id)),
                        "thread exit preserves the exact registered owner and state identity"
                    );
                    assert_eq!(RETAINED_CAPSULE_DROPS.load(Ordering::SeqCst), 0);
                    println!("lifecycle thread-state retention verified: shutdown-thread-state");
                }
                _ => panic!("unknown lifecycle panic mode"),
            }
            assert!(!runtime_is_ready());
            assert!(!runtime_is_initialized());
            assert!(!runtime_execution_is_admitted_for_current_thread(false));
            assert_eq!(molt_runtime_init(), 0, "a failed lifecycle never restarts");
            assert_eq!(molt_runtime_shutdown(), 0);
            assert_eq!(
                *runtime_lifecycle().phase.lock().unwrap(),
                RuntimeLifecyclePhase::Failed
            );
            return;
        }
        for (mode, cause, exit_code, stdout_marker) in [
            ("init", "injected unpublished runtime init panic", 0, None),
            (
                "shutdown",
                "injected shutdown drain C extension cleanup panic",
                0,
                None,
            ),
            (
                "exit",
                "injected shutdown drain C extension cleanup panic",
                1,
                None,
            ),
            (
                "shutdown-thread-state",
                "injected thread-state finalizer panic",
                0,
                Some("lifecycle thread-state retention verified: shutdown-thread-state\n"),
            ),
            (
                "exit-thread-state",
                "injected thread-state finalizer panic",
                1,
                Some("lifecycle thread-state failure armed: exit-thread-state\n"),
            ),
        ] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "state::runtime_state::tests::lifecycle_ffi_panics_fail_closed_without_unwinding",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(MODE, mode);
            let output = crate::test_support::captured_runtime_children::capture(
                &mut command,
                "lifecycle-ffi",
                mode,
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(exit_code), "{mode}: {stderr}");
            assert!(
                stderr.contains(&format!("molt runtime lifecycle failed: {cause}")),
                "{mode}: the diagnostic names the original panic: {stderr}"
            );
            if let Some(marker) = stdout_marker {
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains(marker),
                    "{mode}: missing thread-state ownership observation"
                );
            }
        }
    }

    static EXT_INIT_COUNT: AtomicUsize = AtomicUsize::new(0);
    static EXT_CLEAR_COUNT: AtomicUsize = AtomicUsize::new(0);
    static EXT_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn test_extension_init() -> *mut u8 {
        EXT_INIT_COUNT.fetch_add(1, Ordering::SeqCst);
        Box::into_raw(Box::new(0x5a5a_u64)) as *mut u8
    }

    unsafe extern "C" fn test_extension_clear(ptr: *mut u8) {
        assert!(!ptr.is_null());
        unsafe {
            assert_eq!(*(ptr as *const u64), 0x5a5a);
        }
        EXT_CLEAR_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn test_extension_drop(ptr: *mut u8) {
        assert!(!ptr.is_null());
        EXT_DROP_COUNT.fetch_add(1, Ordering::SeqCst);
        unsafe {
            drop(Box::from_raw(ptr as *mut u64));
        }
    }

    #[test]
    fn atexit_registration_id_exhaustion_is_fallible() {
        let mut registry = ExitRegistry::new();
        registry.next_callback_id = u64::MAX;
        assert_eq!(registry.allocate_callback_id(), None);
        assert_eq!(registry.next_callback_id, u64::MAX);
    }

    #[test]
    fn weak_finalizer_registry_churn_is_indexed_lifo_and_geometric() {
        let mut registry = WeakFinalizerRegistry::new();
        let mut target = 8;
        for bits in 1..=10_000_u64 {
            if !registry.can_insert() {
                let prepared = WeakFinalizerPrepared::try_with_capacity(target)
                    .expect("prepared finalizer storage");
                drop(
                    registry
                        .install_prepared(prepared)
                        .expect("install finalizer storage"),
                );
                target = target.checked_mul(2).expect("test capacity");
            }
            assert_eq!(registry.insert_prepared(bits), Ok(true));
        }
        assert_eq!(registry.len(), 10_000);
        assert!(registry.growth_count <= 12);
        assert!(registry.contains(5_000));
        assert_eq!(registry.remove(5_000), Some(5_000));
        assert!(!registry.contains(5_000));

        let mut previous = u64::MAX;
        let mut drained = 0;
        while let Some(bits) = registry.pop_lifo() {
            assert!(bits < previous);
            assert_ne!(bits, 5_000);
            previous = bits;
            drained += 1;
        }
        assert_eq!(drained, 9_999);
        assert!(registry.is_empty());
    }

    #[test]
    fn weak_finalizer_generation_exhaustion_is_transactional() {
        let mut registry = WeakFinalizerRegistry::new();
        let prepared =
            WeakFinalizerPrepared::try_with_capacity(8).expect("prepared finalizer storage");
        drop(
            registry
                .install_prepared(prepared)
                .expect("install finalizer storage"),
        );
        registry.next_generation = u64::MAX;
        assert!(!registry.generation_available());
        assert_eq!(registry.insert_prepared(1), Err(()));
        assert!(registry.is_empty());
        assert!(registry.order_head.is_none());
        assert!(registry.order_tail.is_none());
    }

    #[test]
    fn extension_state_is_scoped_to_runtime_and_drained_once() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        EXT_INIT_COUNT.store(0, Ordering::SeqCst);
        EXT_CLEAR_COUNT.store(0, Ordering::SeqCst);
        EXT_DROP_COUNT.store(0, Ordering::SeqCst);

        let state = RuntimeState::new();
        let first = runtime_extension_state_get_or_init(
            &state,
            b"test-extension",
            test_extension_init,
            test_extension_clear,
            test_extension_drop,
        );
        let second = runtime_extension_state_get_or_init(
            &state,
            b"test-extension",
            test_extension_init,
            test_extension_clear,
            test_extension_drop,
        );

        assert_eq!(first, second);
        assert_eq!(EXT_INIT_COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(state.extension_states.lock().unwrap().len(), 1);

        crate::with_gil_entry_nopanic!(_py, {
            let _ = _py;
            runtime_extension_states_clear_and_drop(&state);
            assert!(state.extension_states.lock().unwrap().is_empty());
            assert_eq!(EXT_CLEAR_COUNT.load(Ordering::SeqCst), 1);
            assert_eq!(EXT_DROP_COUNT.load(Ordering::SeqCst), 1);

            runtime_extension_states_clear_and_drop(&state);
            assert_eq!(EXT_CLEAR_COUNT.load(Ordering::SeqCst), 1);
            assert_eq!(EXT_DROP_COUNT.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn closing_winner_refuses_transferable_admission_before_ready_unpublication() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let entry = crate::concurrency::RuntimeExecutionGuard::enter_with_worker_cleanup();
        let ready = runtime_ready_ptr().unwrap();
        let baseline = active_runtime_execution_lease_count();
        let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            attempt_rx.recv().unwrap();
            let reservation = RuntimeLifetimeLease::try_acquire();
            done_tx.send(reservation.is_none()).unwrap();
        });
        // Exercise the actual ProcessExit closure, which admits only this
        // exact current execution depth, before Ready is unpublished.
        assert!(close_process_retirement_admission(
            true,
            "test process exit"
        ));
        attempt_tx.send(()).unwrap();
        let refused = done_rx.recv().unwrap();
        reopen_runtime_execution_admission();
        worker.join().unwrap();
        assert!(refused);
        assert_eq!(runtime_ready_ptr(), Some(ready));
        assert_eq!(active_runtime_execution_lease_count(), baseline);
        let reservation = RuntimeLifetimeLease::try_acquire().expect("reopened admission");
        assert_eq!(reservation.identity(), ready.expose_provenance());
        drop(reservation);
        assert_eq!(active_runtime_execution_lease_count(), baseline);
        drop(entry);
    }

    #[test]
    fn lease_ready_recheck_rollback_balances_local_and_global_custody() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let ready = runtime_ready_ptr().expect("test runtime Ready");
        let baseline = active_runtime_execution_lease_count();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        *RUNTIME_EXECUTION_LEASE_POST_CAS_TEST_GATE.lock().unwrap() =
            Some((Arc::clone(&entered), Arc::clone(&release)));
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let acquired = try_acquire_runtime_execution_lease(true);
            result_tx
                .send((acquired, current_thread_holds_runtime_execution_lease()))
                .unwrap();
        });

        entered.wait();
        RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
        release.wait();
        assert_eq!(result_rx.recv().unwrap(), (None, false));
        worker.join().unwrap();
        RUNTIME_READY_PTR.store(ready, AtomicOrdering::Release);
        *RUNTIME_EXECUTION_LEASE_POST_CAS_TEST_GATE.lock().unwrap() = None;
        assert_eq!(
            active_runtime_execution_lease_count(),
            baseline,
            "failed Ready recheck must roll back the exact admitted count"
        );
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn finalizing_unpublishes_before_atexit_reentry_and_racing_entrants() {
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            assert_eq!(
                unsafe { molt_cpython_abi::api::object::Py_IsInitialized() },
                0
            );
            assert_eq!(molt_runtime_init(), 1);
            assert_eq!(
                unsafe { molt_cpython_abi::api::object::Py_IsInitialized() },
                1
            );

            let (init_start_tx, init_start_rx) = std::sync::mpsc::channel();
            let (init_prepared_tx, init_prepared_rx) = std::sync::mpsc::channel();
            let (init_started_tx, init_started_rx) = std::sync::mpsc::channel();
            let (init_done_tx, init_done_rx) = std::sync::mpsc::channel();
            let init_entrant = std::thread::spawn(move || {
                let gil = GilGuard::new();
                let _ = runtime_state(&gil.token());
                drop(gil);
                init_prepared_tx.send(()).unwrap();
                init_start_rx.recv().unwrap();
                init_started_tx.send(()).unwrap();
                init_done_tx.send(molt_runtime_init()).unwrap();
            });
            init_prepared_rx.recv().unwrap();

            let (shutdown_start_tx, shutdown_start_rx) = std::sync::mpsc::channel();
            let (shutdown_started_tx, shutdown_started_rx) = std::sync::mpsc::channel();
            let (shutdown_done_tx, shutdown_done_rx) = std::sync::mpsc::channel();
            let shutdown_entrant = std::thread::spawn(move || {
                shutdown_start_rx.recv().unwrap();
                shutdown_started_tx.send(()).unwrap();
                shutdown_done_tx.send(molt_runtime_shutdown()).unwrap();
            });

            let (recursive_tx, recursive_rx) = std::sync::mpsc::channel();
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let release = Arc::new(std::sync::Barrier::new(2));
            *RUNTIME_FINALIZE_ATEXIT_TEST_HOOK.lock().unwrap() =
                Some(RuntimeFinalizeAtexitTestHook {
                    results: recursive_tx,
                    entered: entered_tx,
                    release: Arc::clone(&release),
                });

            let owner = std::thread::spawn(|| molt_runtime_shutdown());
            entered_rx.recv().unwrap();
            assert_eq!(recursive_rx.recv().unwrap(), (0, 0));
            assert!(runtime_ready_ptr().is_none());
            assert!(matches!(
                *runtime_lifecycle().phase.lock().unwrap(),
                RuntimeLifecyclePhase::Finalizing { .. }
            ));
            assert_eq!(
                unsafe { molt_cpython_abi::api::object::Py_IsInitialized() },
                1
            );

            init_start_tx.send(()).unwrap();
            shutdown_start_tx.send(()).unwrap();
            init_started_rx.recv().unwrap();
            shutdown_started_rx.recv().unwrap();
            assert!(
                init_done_rx
                    .recv_timeout(std::time::Duration::from_millis(50))
                    .is_err()
            );
            assert!(
                shutdown_done_rx
                    .recv_timeout(std::time::Duration::from_millis(50))
                    .is_err()
            );

            release.wait();
            assert_eq!(owner.join().unwrap(), 1);
            assert_eq!(init_done_rx.recv().unwrap(), 0);
            assert_eq!(shutdown_done_rx.recv().unwrap(), 0);
            init_entrant.join().unwrap();
            shutdown_entrant.join().unwrap();
            assert!(matches!(
                *runtime_lifecycle().phase.lock().unwrap(),
                RuntimeLifecyclePhase::Shutdown
            ));
            assert_eq!(
                unsafe { molt_cpython_abi::api::object::Py_IsInitialized() },
                0
            );
        });
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn shutdown_refuses_attached_and_detached_foreign_thread_state_owners() {
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            assert_eq!(molt_runtime_init(), 1);
            let retained_owner_baseline =
                molt_cpython_abi::api::object::runtime_retained_thread_state_count();
            assert_eq!(
                retained_owner_baseline, 0,
                "runtime initialization prepares TLS lifetime but creates no state before admission"
            );

            let (leased_tx, leased_rx) = std::sync::mpsc::channel();
            let (unlease_tx, unlease_rx) = std::sync::mpsc::channel();
            let lease_holder = std::thread::spawn(move || {
                assert_eq!(try_acquire_runtime_execution_lease(true), Some(true));
                leased_tx.send(()).unwrap();
                unlease_rx.recv().unwrap();
                release_runtime_execution_lease();
            });
            leased_rx.recv().unwrap();
            assert_eq!(active_runtime_execution_lease_count(), 1);
            assert_eq!(
                molt_runtime_shutdown(),
                0,
                "admission-to-attachment lifecycle lease must independently refuse shutdown"
            );
            assert!(runtime_ready_ptr().is_some());
            unlease_tx.send(()).unwrap();
            lease_holder.join().unwrap();

            let (attached_tx, attached_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let (detached_tx, detached_rx) = std::sync::mpsc::channel();
            let (cleanup_tx, cleanup_rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                touch_tls_guard();
                let gil = GilGuard::new();
                molt_cpython_abi::api::object::attach_runtime_execution_thread(
                    runtime_state_for_gil()
                        .expect("live runtime execution owner")
                        .identity(),
                );
                drop(gil);
                attached_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let gil = GilGuard::new();
                molt_cpython_abi::api::object::detach_runtime_execution_thread();
                drop(gil);
                detached_tx.send(()).unwrap();
                cleanup_rx.recv().unwrap();
                let _gil = GilGuard::new();
                molt_cpython_abi::api::object::clear_runtime_execution_thread_state(|| false);
            });
            attached_rx.recv().unwrap();
            assert_eq!(
                molt_cpython_abi::api::object::runtime_execution_attachment_count(),
                1
            );
            assert_eq!(
                molt_runtime_shutdown(),
                0,
                "live attachment must refuse shutdown"
            );
            assert!(
                runtime_ready_ptr().is_some(),
                "refused shutdown must leave the runtime published and usable"
            );

            release_tx.send(()).unwrap();
            detached_rx.recv().unwrap();
            assert_eq!(
                molt_cpython_abi::api::object::runtime_execution_attachment_count(),
                0
            );
            assert_eq!(
                molt_runtime_shutdown(),
                0,
                "a detached foreign PyThreadState may still own managed edges"
            );
            assert!(
                runtime_ready_ptr().is_some(),
                "refusing retained-state shutdown must reopen execution admission"
            );

            cleanup_tx.send(()).unwrap();
            worker.join().unwrap();
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                retained_owner_baseline
            );

            let exit_barrier = Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = Arc::clone(&exit_barrier);
            let (retained_tx, retained_rx) = std::sync::mpsc::channel();
            let tls_worker = std::thread::spawn(move || {
                let guard = crate::concurrency::RuntimeExecutionGuard::enter();
                unsafe {
                    molt_cpython_abi::api::errors::PyErr_SetString(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError)
                            .cast::<molt_cpython_abi::abi_types::PyObject>(),
                        c"concurrent TLS destruction".as_ptr(),
                    );
                }
                drop(guard);
                retained_tx.send(()).unwrap();
                worker_barrier.wait();
                // ThreadStateRecord::drop must acquire the non-attaching
                // lifecycle/GIL destruction lane while shutdown races here.
            });
            retained_rx.recv().unwrap();
            exit_barrier.wait();
            let first_shutdown = molt_runtime_shutdown();
            tls_worker.join().unwrap();
            if first_shutdown == 0 {
                assert_eq!(
                    molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                    retained_owner_baseline,
                    "concurrent TLS destruction must retire only its foreign owner"
                );
                assert!(runtime_ready_ptr().is_some());
                assert_eq!(molt_runtime_shutdown(), 1);
            } else {
                assert_eq!(first_shutdown, 1);
            }
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                0,
                "teardown owner state must be destroyed before RuntimeState is freed"
            );
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                0,
                "post-shutdown raw exception cleanup must not reopen stale TLS"
            );
        });
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    fn encoded_core_gil_entry_is_allocation_free_after_warmup() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert_eq!(molt_runtime_init(), 1);

        for _ in 0..64 {
            drop(molt_runtime_core::CoreGilGuard::new());
        }
        crate::attestation_probe::reset();
        crate::attestation_probe::set_tracking(true);
        for _ in 0..10_000 {
            drop(molt_runtime_core::CoreGilGuard::new());
        }
        crate::attestation_probe::set_tracking(false);

        let observed = crate::attestation_probe::snapshot();
        assert_eq!(observed.allocations, 0, "{observed:?}");
        assert_eq!(observed.allocated_bytes, 0, "{observed:?}");
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn concurrent_init_waits_for_ready_publication() {
        crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            let entered = Arc::new(std::sync::Barrier::new(2));
            let release = Arc::new(std::sync::Barrier::new(2));
            *RUNTIME_INIT_TEST_GATE.lock().unwrap() =
                Some((Arc::clone(&entered), Arc::clone(&release)));

            let owner = std::thread::spawn(|| molt_runtime_init());
            entered.wait();
            assert!(runtime_ready_ptr().is_none());
            assert!(matches!(
                *runtime_lifecycle().phase.lock().unwrap(),
                RuntimeLifecyclePhase::Initializing { .. }
            ));

            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let waiter = std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                done_tx.send(molt_runtime_init()).unwrap();
            });
            started_rx.recv().unwrap();
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_millis(50))
                    .is_err(),
                "a racing initializer must not observe unpublished state as ready"
            );

            release.wait();
            assert_eq!(owner.join().unwrap(), 1);
            assert_eq!(done_rx.recv().unwrap(), 1);
            waiter.join().unwrap();
            *RUNTIME_INIT_TEST_GATE.lock().unwrap() = None;

            let ready = runtime_ready_ptr().expect("runtime published after complete init");
            assert!(matches!(
                *runtime_lifecycle().phase.lock().unwrap(),
                RuntimeLifecyclePhase::Ready { ptr } if ptr == ready as usize
            ));
            assert_eq!(
                crate::object::gc::gc_registry_owner_identity(),
                ready.expose_provenance(),
                "racing initializers must converge on the one registry-owning runtime"
            );
            assert_eq!(molt_runtime_shutdown(), 1);
            assert_eq!(crate::object::gc::gc_registry_owner_identity(), 0);
        });
    }
}
