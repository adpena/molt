use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering as AtomicOrdering};
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
use crate::concurrency::gil::{gil_held, hold_runtime_gil};
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

#[cfg(unix)]
static DEBUG_SIGTRAP_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
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
    pub(crate) open_default_mode: AtomicU64,
    pub(crate) molt_missing: AtomicU64,
    pub(crate) molt_not_implemented: AtomicU64,
    pub(crate) molt_ellipsis: AtomicU64,
    pub(crate) awaitable_await: AtomicU64,
    pub(crate) function_code_descriptor: AtomicU64,
    pub(crate) function_globals_descriptor: AtomicU64,
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
    pub(crate) container_cookie: Option<WeakContainerCookie>,
    /// CPython-compatible sticky hash: once computed while the referent is
    /// alive, the value remains available after referent death.
    pub(crate) cached_hash: Option<i64>,
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

pub(crate) struct ContextVarsThreadState {
    pub(crate) frames: Vec<HashMap<i64, u64>>,
    pub(crate) tokens: HashMap<i64, (i64, u64, bool)>,
    pub(crate) contexts: HashMap<i64, HashMap<i64, u64>>,
}

impl ContextVarsThreadState {
    pub(crate) fn new() -> Self {
        Self {
            frames: vec![HashMap::new()],
            tokens: HashMap::new(),
            contexts: HashMap::new(),
        }
    }
}

pub(crate) struct ContextVarsState {
    pub(crate) next_var_handle: i64,
    pub(crate) next_token_handle: i64,
    pub(crate) next_context_handle: i64,
    pub(crate) var_defaults: HashMap<i64, u64>,
    pub(crate) threads: HashMap<thread::ThreadId, ContextVarsThreadState>,
}

impl ContextVarsState {
    pub(crate) fn new() -> Self {
        Self {
            next_var_handle: 1,
            next_token_handle: 1,
            next_context_handle: 1,
            var_defaults: HashMap::new(),
            threads: HashMap::new(),
        }
    }
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
    pub(crate) gc: crate::object::gc::GcRuntimeState,
    pub(crate) gc_running: AtomicBool,
    pub(crate) gc_last_failure: AtomicU8,
    pub(crate) builtin_classes: std::sync::atomic::AtomicPtr<BuiltinClasses>,
    pub(crate) interned: InternedNames,
    pub(crate) runtime_static_names: RuntimeStaticNames,
    pub(crate) method_cache: MethodCache,
    pub(crate) special_cache: SpecialCache,
    pub(crate) canonical_objects: CanonicalObjectCache,
    pub(crate) module_cache: Mutex<HashMap<String, u64>>,
    /// Import-bedrock ModuleTable (design doc 69): dense per-ModuleId state
    /// machine + slots, one instance per isolate, sized from the installed
    /// module registry on first use. Owned by builtins::module_table.
    pub(crate) module_table: OnceLock<crate::builtins::module_table::ModuleTable>,
    pub(crate) importlib_default_meta_path_bootstrapped: AtomicBool,
    pub(crate) intrinsic_registry_module: AtomicPtr<u8>,
    pub(crate) exception_type_cache: Mutex<HashMap<String, u64>>,
    pub(crate) exceptions: ExceptionsRuntimeState,
    pub(crate) exception_str_cache: Mutex<HashMap<u64, (u64, bool)>>,
    pub(crate) codec_error_handlers: Mutex<HashMap<String, u64>>,
    pub(crate) argv: Mutex<Vec<Vec<u8>>>,
    pub(crate) sys_version_info: Mutex<Option<PythonVersionInfo>>,
    pub(crate) sys_version: Mutex<Option<String>>,
    pub(crate) hash_secret: OnceLock<HashSecret>,
    pub(crate) utf8_index_cache: Mutex<Utf8CacheStore>,
    pub(crate) utf8_count_cache: Vec<Mutex<Utf8CountCacheStore>>,
    pub(crate) scheduler_started: AtomicBool,
    pub(crate) scheduler: OnceLock<MoltScheduler>,
    pub(crate) sleep_queue_started: AtomicBool,
    pub(crate) sleep_queue: OnceLock<Arc<SleepQueue>>,
    pub(crate) io_poller_started: AtomicBool,
    pub(crate) io_poller: OnceLock<Arc<IoPoller>>,
    pub(crate) capabilities: OnceLock<HashSet<String>>,
    pub(crate) trusted: OnceLock<bool>,
    pub(crate) async_hang_probe: OnceLock<Option<AsyncHangProbe>>,
    pub(crate) event_loop_registry: EventLoopRegistry,
    pub(crate) pipe_transport_registry: PipeTransportRegistry,
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
    pub(crate) asyncio_event_waiter_index: Mutex<HashMap<u64, AsyncioEventWaiterIndex>>,
    pub(crate) task_exception_handler_stacks: Mutex<HashMap<PtrSlot, Vec<usize>>>,
    pub(crate) task_exception_stacks: Mutex<HashMap<PtrSlot, Vec<u64>>>,
    pub(crate) task_exception_depths: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_exception_baselines: Mutex<HashMap<PtrSlot, usize>>,
    pub(crate) task_last_exceptions: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) task_last_exception_pending: AtomicBool,
    pub(crate) task_results: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) attributes: AttributesRuntimeState,
    pub(crate) dict_subclass_storage: Mutex<HashMap<PtrSlot, u64>>,
    pub(crate) await_waiters: Mutex<HashMap<PtrSlot, Vec<PtrSlot>>>,
    pub(crate) await_waiter_index: Mutex<HashMap<PtrSlot, AwaitWaiterIndex>>,
    pub(crate) task_waiting_on: Mutex<HashMap<PtrSlot, PtrSlot>>,
    pub(crate) asyncgen_hooks: Mutex<AsyncGenHooks>,
    pub(crate) contextvars: Mutex<ContextVarsState>,
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
    pub(crate) asyncgen_locals: Mutex<HashMap<u64, AsyncGenLocalsEntry>>,
    pub(crate) gen_locals: Mutex<HashMap<u64, GenLocalsEntry>>,
    pub(crate) weakrefs: Mutex<WeakRefRegistry>,
    pub(crate) exit_registry: Mutex<ExitRegistry>,
    pub(crate) abc_invalidation_counter: AtomicU64,
    pub(crate) asyncgen_registry: Mutex<HashSet<PtrSlot>>,
    pub(crate) fn_ptr_code: Mutex<HashMap<u64, u64>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_pool_started: AtomicBool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_pool: OnceLock<ThreadPool>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) thread_tasks: Mutex<HashMap<PtrSlot, Arc<ThreadTaskState>>>,
    pub(crate) process_registry: ProcessRegistry,
    #[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
    pub(crate) socket_state: SocketRuntimeState,
    pub(crate) signal: SignalRuntimeState,
    pub(crate) process_tasks: Mutex<HashMap<PtrSlot, Arc<ProcessTaskState>>>,
    pub(crate) code_slots: OnceLock<Vec<AtomicU64>>,
    pub(crate) python_builtin_function_slots: OnceLock<Vec<AtomicU64>>,
    pub(crate) start_time: OnceLock<Instant>,
    /// VFS state lazily initialized from environment variables on first access.
    pub(crate) vfs_state: OnceLock<Option<crate::vfs::VfsState>>,
    /// Typed state owned by extracted runtime crates and scoped to this runtime.
    pub(crate) extension_states: Mutex<HashMap<Vec<u8>, RuntimeExtensionStateSlot>>,
}

impl RuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            gc: crate::object::gc::GcRuntimeState::new(),
            gc_running: AtomicBool::new(false),
            gc_last_failure: AtomicU8::new(0),
            builtin_classes: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            interned: InternedNames::new(),
            runtime_static_names: RuntimeStaticNames::new(),
            method_cache: MethodCache::new(),
            special_cache: SpecialCache::new(),
            canonical_objects: CanonicalObjectCache::new(),
            module_cache: Mutex::new(HashMap::new()),
            module_table: OnceLock::new(),
            importlib_default_meta_path_bootstrapped: AtomicBool::new(false),
            intrinsic_registry_module: AtomicPtr::new(std::ptr::null_mut()),
            exception_type_cache: Mutex::new(HashMap::new()),
            exceptions: ExceptionsRuntimeState::new(),
            exception_str_cache: Mutex::new(HashMap::new()),
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
            sys_version: Mutex::new(None),
            hash_secret: OnceLock::new(),
            utf8_index_cache: Mutex::new(Utf8CacheStore::new()),
            utf8_count_cache: build_utf8_count_cache(),
            scheduler_started: AtomicBool::new(false),
            scheduler: OnceLock::new(),
            sleep_queue_started: AtomicBool::new(false),
            sleep_queue: OnceLock::new(),
            io_poller_started: AtomicBool::new(false),
            io_poller: OnceLock::new(),
            capabilities: OnceLock::new(),
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
            task_last_exception_pending: AtomicBool::new(false),
            task_results: Mutex::new(HashMap::new()),
            attributes: AttributesRuntimeState::new(),
            dict_subclass_storage: Mutex::new(HashMap::new()),
            await_waiters: Mutex::new(HashMap::new()),
            await_waiter_index: Mutex::new(HashMap::new()),
            task_waiting_on: Mutex::new(HashMap::new()),
            asyncgen_hooks: Mutex::new(AsyncGenHooks {
                firstiter: MoltObject::none().bits(),
                finalizer: MoltObject::none().bits(),
            }),
            contextvars: Mutex::new(ContextVarsState::new()),
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
            asyncgen_locals: Mutex::new(HashMap::new()),
            gen_locals: Mutex::new(HashMap::new()),
            weakrefs: Mutex::new(WeakRefRegistry::new()),
            exit_registry: Mutex::new(ExitRegistry::new()),
            abc_invalidation_counter: AtomicU64::new(0),
            asyncgen_registry: Mutex::new(HashSet::new()),
            fn_ptr_code: Mutex::new(HashMap::new()),
            #[cfg(not(target_arch = "wasm32"))]
            thread_pool_started: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            thread_pool: OnceLock::new(),
            #[cfg(not(target_arch = "wasm32"))]
            thread_tasks: Mutex::new(HashMap::new()),
            process_registry: ProcessRegistry::new(),
            #[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
            socket_state: SocketRuntimeState::new(),
            signal: SignalRuntimeState::new(),
            process_tasks: Mutex::new(HashMap::new()),
            code_slots: OnceLock::new(),
            python_builtin_function_slots: OnceLock::new(),
            start_time: OnceLock::new(),
            vfs_state: OnceLock::new(),
            extension_states: Mutex::new(HashMap::new()),
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

pub(crate) fn runtime_extension_states_clear_and_drop(state: &RuntimeState) {
    crate::gil_assert();
    let slots: Vec<RuntimeExtensionStateSlot> = {
        let mut guard = state.extension_states.lock().unwrap();
        guard.drain().map(|(_, slot)| slot).collect()
    };
    clear_and_drop_extension_slots(slots);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeLifecyclePhase {
    Uninitialized,
    Initializing { owner: thread::ThreadId },
    Ready { ptr: usize },
    Finalizing { owner: thread::ThreadId, ptr: usize },
    Shutdown,
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

pub(crate) fn runtime_is_initialized() -> bool {
    matches!(
        *runtime_lifecycle().phase.lock().unwrap(),
        RuntimeLifecyclePhase::Ready { .. } | RuntimeLifecyclePhase::Finalizing { .. }
    )
}

#[inline(always)]
fn runtime_ready_ptr() -> Option<*mut RuntimeState> {
    let ptr = RUNTIME_READY_PTR.load(AtomicOrdering::Acquire);
    if ptr.is_null() { None } else { Some(ptr) }
}

pub(crate) fn runtime_state_for_gil() -> Option<&'static RuntimeState> {
    if let Some(ptr) = runtime_ready_ptr() {
        return runtime_state_tls().or_else(|| Some(unsafe { &*ptr }));
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

pub(crate) fn runtime_state(_py: &PyToken<'_>) -> &'static RuntimeState {
    let _ = _py;
    touch_tls_guard();
    if let Some(state) = runtime_state_for_gil() {
        return state;
    }
    let _ = molt_runtime_init();
    if let Some(state) = runtime_state_for_gil() {
        state
    } else {
        panic!("runtime state requested after permanent shutdown")
    }
}

// ---------------------------------------------------------------------------
// GIL vtable shims — bridge core crate's function-pointer GIL to the real
// mutex-based GIL in this crate.
// ---------------------------------------------------------------------------

extern "C" fn __core_gil_acquire() -> u64 {
    GilGuard::new().into_encoded_lane()
}

extern "C" fn __core_gil_release(token: u64) {
    // SAFETY: the core guard is !Send/!Sync and calls release exactly once on
    // the same thread with the unmatched token returned by acquire.
    drop(unsafe { GilGuard::from_encoded_lane(token) });
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
        let lifecycle = runtime_lifecycle();
        let owner = thread::current().id();
        let ptr = {
            let mut phase = lifecycle.phase.lock().unwrap();
            match *phase {
                RuntimeLifecyclePhase::Ready { ptr } => {
                    RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
                    *phase = RuntimeLifecyclePhase::Finalizing { owner, ptr };
                    lifecycle.changed.notify_all();
                    Some(ptr as *mut RuntimeState)
                }
                _ => None,
            }
        };
        if let Some(ptr) = ptr {
            #[cfg(not(target_arch = "wasm32"))]
            molt_cpython_abi::api::object::attach_runtime_execution_thread();
            let state = unsafe { &*ptr };
            let py = gil.token();
            crate::object::ops::profile_dump_with_gil(&py);
            // RC drop-insertion substrate (design 20). Two distinct gates for
            // two distinct properties:
            //
            // 1. Pre-teardown RUNAWAY guard. Runs here, while the full working
            //    set is resident — a coarse peak-live/OOM canary at
            //    EXPECTED_LIVE_OBJECTS (a reachable high-water-mark, not a leak;
            //    teardown below reclaims every reachable acyclic graph).
            crate::object::ops::assert_no_leak_at_exit(&py);
            // Run the cyclic collector before module teardown so unreachable
            // cycles are finalized and reclaimed in CPython's shutdown position.
            unsafe {
                let outcome = crate::object::gc::collect_cycles(&py);
                match outcome.status {
                    crate::object::gc::GcCollectStatus::Completed
                    | crate::object::gc::GcCollectStatus::ReentrantNoop => {}
                    failure => {
                        eprintln!("molt gc: process-exit collection failed closed: {failure:?}")
                    }
                }
            }
            runtime_teardown_for_process_exit(&py, state);
            // 2. Post-teardown TRUE-LEAK gauge (ownership_lattice_phase0.md
            //    §2.4). Teardown above has reclaimed every reachable acyclic
            //    graph and the collector has reclaimed unreachable cycles, so the
            //    only survivors now are the immortal floor + genuine leaks. GIL
            //    still held; reads crate-static counters only, never touches
            //    `state`.
            crate::object::ops::assert_no_true_leak_post_teardown(&py);
            let mut phase = lifecycle.phase.lock().unwrap();
            assert_eq!(
                *phase,
                RuntimeLifecyclePhase::Finalizing {
                    owner,
                    ptr: ptr as usize,
                }
            );
            *phase = RuntimeLifecyclePhase::Shutdown;
            lifecycle.changed.notify_all();
            #[cfg(not(target_arch = "wasm32"))]
            molt_cpython_abi::api::object::detach_runtime_execution_thread();
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

fn initialize_runtime_state(gil: &GilGuard, state: &RuntimeState) {
    trace_runtime_init("state_allocated");
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

    crate::object::ops_sys::molt_runtime_init_resources();
    trace_runtime_init("resources");
    crate::object::ops_sys::molt_runtime_init_audit();
    trace_runtime_init("audit");
    crate::object::ops_sys::molt_runtime_init_io_mode();
    trace_runtime_init("io_mode");

    // Freeze security-sensitive environment state before user code can run.
    {
        let nested = GilGuard::new();
        let py = nested.token();
        let _ = crate::is_trusted(&py);
        let _ = crate::has_capability(&py, "_init");
    }
    trace_runtime_init("capabilities");

    // Publish the one CPython ABI hook table as part of the runtime
    // initialization transaction. Lifecycle queries such as
    // `Py_IsInitialized` must observe this lifecycle authority immediately
    // after Ready publication; lazily installing the table on an unrelated ABI
    // call leaves the detached stub as a second, false initialization authority.
    crate::cpython_abi_hooks::register_cpython_hooks();
    trace_runtime_init("cpython_abi_hooks");

    super::metrics::snapshot_live_floor();
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init() -> u64 {
    #[cfg(target_arch = "wasm32")]
    ensure_wasm_ctors();
    // The GIL authority is process-lifetime state and must precede every
    // extracted-crate init callback; RuntimeState publication remains later.
    // SAFETY: `CORE_GIL_VT` is process-static and is the sole encoded GIL
    // acquire/release authority for every extracted runtime crate.
    unsafe { molt_runtime_core::set_gil_vtable(&CORE_GIL_VT) };
    trace_runtime_init("enter");
    super::metrics::init_profile_enabled_from_env();
    touch_tls_guard();
    #[cfg(not(target_arch = "wasm32"))]
    ensure_debug_sigtrap_handler();
    if runtime_ready_ptr().is_some() {
        trace_runtime_init("already_initialized");
        return 1;
    }
    let owner = thread::current().id();
    loop {
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
                *phase = RuntimeLifecyclePhase::Initializing { owner };
            }
        }
        drop(phase);

        // Initialization is a fail-closed publication transaction. No pointer
        // becomes globally reachable until every initialization step succeeds.
        // An invariant panic is intentionally not converted into a plausible
        // retryable result: this extern-C boundary aborts rather than exposing
        // unknown partial side effects to a second initialization attempt.
        let mut state = Box::new(RuntimeState::new());
        let state_ptr = (&mut *state) as *mut RuntimeState;
        crate::object::gc::gc_bind_registry(&state);
        set_thread_runtime_state(state_ptr);
        initialize_runtime_state(&gil, &state);

        #[cfg(test)]
        if let Some((entered, release)) = RUNTIME_INIT_TEST_GATE.lock().unwrap().clone() {
            entered.wait();
            release.wait();
        }

        let ptr = Box::into_raw(state);
        signal_runtime_state_publish(unsafe { &*ptr });
        let mut phase = lifecycle.phase.lock().unwrap();
        assert_eq!(*phase, RuntimeLifecyclePhase::Initializing { owner });
        *phase = RuntimeLifecyclePhase::Ready { ptr: ptr as usize };
        RUNTIME_READY_PTR.store(ptr, AtomicOrdering::Release);
        clear_thread_runtime_state();
        lifecycle.changed.notify_all();
        trace_runtime_init("ok");
        return 1;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_ensure_gil() {
    touch_tls_guard();
    if !gil_held() {
        hold_runtime_gil(GilGuard::new());
    }
    #[cfg(not(target_arch = "wasm32"))]
    molt_cpython_abi::api::object::attach_runtime_execution_thread();
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_shutdown() -> u64 {
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
    };
    debug_assert_eq!(runtime_ready_ptr(), Some(ptr));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let attachments = molt_cpython_abi::api::object::runtime_execution_attachment_count();
        if attachments != 0 {
            eprintln!(
                "molt runtime shutdown refused: {attachments} runtime execution attachment(s) remain live"
            );
            return 0;
        }
    }
    RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
    *phase = RuntimeLifecyclePhase::Finalizing {
        owner,
        ptr: ptr as usize,
    };
    lifecycle.changed.notify_all();
    drop(phase);

    #[cfg(not(target_arch = "wasm32"))]
    molt_cpython_abi::api::object::attach_runtime_execution_thread();

    let state = unsafe { &*ptr };
    let py = gil.token();
    runtime_teardown(&py, state);
    #[cfg(not(target_arch = "wasm32"))]
    molt_cpython_abi::api::object::detach_runtime_execution_thread();
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
    *phase = RuntimeLifecyclePhase::Shutdown;
    lifecycle.changed.notify_all();
    1
}

/// Read-optimized projection of the canonical lifecycle phase. It is non-null
/// exactly while the lifecycle is `Ready`.
static RUNTIME_READY_PTR: AtomicPtr<RuntimeState> = AtomicPtr::new(std::ptr::null_mut());
static RUNTIME_LIFECYCLE: OnceLock<RuntimeLifecycle> = OnceLock::new();
static PROCESS_EXIT_FINALIZED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static RUNTIME_INIT_TEST_GATE: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>> =
    Mutex::new(None);
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
/// 1. `test_mutex_guard` ensures no concurrent runtime
///    access.
/// 2. The previous runtime has been fully shut down via
///    `molt_runtime_shutdown()`.
/// 3. The caller will immediately re-initialize via `molt_runtime_init()`.
#[cfg(test)]
pub(crate) fn molt_runtime_reset_for_testing() {
    RUNTIME_READY_PTR.store(std::ptr::null_mut(), AtomicOrdering::Release);
    let lifecycle = runtime_lifecycle();
    let mut phase = lifecycle.phase.lock().unwrap();
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
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn finalizing_unpublishes_before_atexit_reentry_and_racing_entrants() {
        let _guard = crate::test_mutex_guard();
        if runtime_ready_ptr().is_some() {
            assert_eq!(molt_runtime_shutdown(), 1);
        }
        molt_runtime_reset_for_testing();
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
        *RUNTIME_FINALIZE_ATEXIT_TEST_HOOK.lock().unwrap() = Some(RuntimeFinalizeAtexitTestHook {
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
    }

    #[test]
    #[ignore = "mutates the process-global runtime lifecycle; run in isolation"]
    fn shutdown_refuses_while_a_foreign_runtime_attachment_is_live() {
        let _guard = crate::test_mutex_guard();
        if runtime_ready_ptr().is_some() {
            assert_eq!(molt_runtime_shutdown(), 1);
        }
        molt_runtime_reset_for_testing();
        assert_eq!(molt_runtime_init(), 1);

        let (attached_tx, attached_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let gil = GilGuard::new();
            molt_cpython_abi::api::object::attach_runtime_execution_thread();
            drop(gil);
            attached_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let _gil = GilGuard::new();
            molt_cpython_abi::api::object::detach_runtime_execution_thread();
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
        worker.join().unwrap();
        assert_eq!(
            molt_cpython_abi::api::object::runtime_execution_attachment_count(),
            0
        );
        assert_eq!(molt_runtime_shutdown(), 1);
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    fn encoded_core_gil_entry_is_allocation_free_after_warmup() {
        let _guard = crate::test_mutex_guard();
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
        let _guard = crate::test_mutex_guard();
        if runtime_ready_ptr().is_some() {
            assert_eq!(molt_runtime_shutdown(), 1);
        }
        molt_runtime_reset_for_testing();

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
    }
}
