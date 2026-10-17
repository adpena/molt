// Call-site inline-cache authority for call binding, method loads (CPython's
// `LOAD_ATTR` and `LOAD_SUPER_ATTR` in method form) and the calls that
// consume them, C-ABI IC entry points, and cache lifecycle.

use super::arguments::{ArgumentCustody, CallArguments};
use super::constructors::is_default_type_call;
use super::frame_binding::{call_owned_function, function_requires_full_binding};
use super::*;
use crate::object::layout::function_mutation_version;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{attr_name_bits_from_bytes, molt_super_new};
fn trace_call_bind_ic_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_CALL_BIND_IC)
}

#[inline]
fn trace_call_bind_ic_bypass(kind: &str, reason: &str) {
    if trace_call_bind_ic_enabled() {
        eprintln!("[molt call_bind_ic] bypass {kind} reason={reason}");
    }
}

#[inline]
fn trace_call_bind_ic_epoch_stage(recorded: u64, stage: &str) {
    if trace_call_bind_ic_enabled() {
        let current = crate::object::global_type_version();
        if current != recorded {
            eprintln!(
                "[molt call_bind_ic] type epoch changed stage={stage} recorded={recorded} current={current}"
            );
        }
    }
}

fn disable_call_bind_ic_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_DISABLE_CALL_BIND_IC)
}

#[inline(always)]
pub(super) fn type_epoch_matches(recorded: u64) -> bool {
    crate::object::global_type_version() == recorded
}

/// Run an MRO-dependent cache classification against one stable publication
/// epoch. Callers sample before touching type state and must reject the cache
/// candidate if this check fails after resolution. This prevents a concurrent
/// mutation from publishing a stale borrowed target under the new epoch.
#[inline(always)]
pub(super) fn type_resolution_epoch_is_stable(recorded: u64) -> bool {
    type_epoch_matches(recorded)
}

#[derive(Clone, Copy)]
pub(super) struct CallBindIcEntry {
    pub(super) fn_ptr: u64,
    /// Shared executable/signature/defaults epoch, independent of class mutation.
    pub(super) function_version: u64,
    pub(super) target_bits: u64,
    pub(super) class_bits: u64,
    pub(super) class_version: u64,
    /// Global MRO dependency epoch for entries resolved through type lookup.
    /// It is checked before dereferencing any borrowed cached target.
    pub(super) type_version: u64,
    /// For `CALL_BIND_IC_KIND_TYPE_CALL`: cached total allocation size
    /// (header + payload) computed once at IC-population time.  Avoids
    /// re-running `class_layout_size` (MRO walks, dict probes, name
    /// interning) on every instance allocation.
    pub(super) cached_alloc_size: usize,
    pub(super) arity: u8,
    pub(super) kind: u8,
}

pub(super) const CALL_BIND_IC_KIND_DIRECT_FUNC: u8 = 1;
pub(super) const CALL_BIND_IC_KIND_LIST_APPEND: u8 = 2;
pub(super) const CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC: u8 = 3;
pub(super) const CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC: u8 = 4;
pub(super) const CALL_BIND_IC_KIND_TYPE_CALL: u8 = 5;

// Thread-local direct-mapped inline cache for call_bind dispatch.
// Each slot stores (site_id, entry). On lookup, we check if the stored site_id
// matches — if so, it's a hit with zero synchronization overhead.
// This replaces a Mutex<HashMap> that required a lock on every call.
const IC_TLS_SIZE: usize = 256; // Must be power of 2

#[inline]
pub(super) fn ic_tls_lookup(site_id: u64) -> Option<CallBindIcEntry> {
    REF_OWNING_IC_TLS.with(|cache| {
        let cache = cache.borrow();
        let idx = (site_id as usize) & (IC_TLS_SIZE - 1);
        let (stored_id, entry) = cache.call[idx];
        if stored_id == site_id && entry.kind != 0 {
            Some(entry)
        } else {
            None
        }
    })
}

#[inline]
fn call_bind_ic_owns_target(entry: CallBindIcEntry) -> bool {
    matches!(
        entry.kind,
        CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC | CALL_BIND_IC_KIND_TYPE_CALL
    ) && entry.target_bits != 0
}

#[inline]
pub(super) fn ic_tls_insert(_py: &PyToken<'_>, site_id: u64, entry: CallBindIcEntry) {
    if call_bind_ic_owns_target(entry) {
        inc_ref_bits(_py, entry.target_bits);
    }
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let idx = (site_id as usize) & (IC_TLS_SIZE - 1);
        let previous = cache.call[idx].1;
        cache.call[idx] = (site_id, entry);
        previous
    });
    if call_bind_ic_owns_target(previous) {
        dec_ref_bits(_py, previous.target_bits);
    }
}

/// Detach this thread's owned call-cache targets and report whether any existed.
pub(crate) fn clear_call_bind_ic_cache(_py: &PyToken<'_>) -> bool {
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        std::mem::replace(
            &mut cache.borrow_mut().call,
            [(0, EMPTY_CALL_IC_ENTRY); IC_TLS_SIZE],
        )
    });
    release_call_ic_entries(_py, &previous)
}

/// Per-site inline cache for method loads (`LOAD_ATTR` in method form).
///
/// Keyed on the load-site id, validated against class and function mutation epochs.
/// On a hit the resolved class function and the pre-interned attribute name are
/// reused, eliminating the per-call name interning + MRO walk + descriptor-cache
/// probe.  `can_shadow` records whether an instance of this class could possibly
/// carry an own attribute of this name (a managed field slot) — when false, the
/// per-call instance-shadow check is skipped entirely.
#[derive(Clone, Copy)]
struct MethodIcEntry {
    pub(super) class_bits: u64,
    pub(super) class_version: u64,
    type_version: u64,
    func_bits: u64,
    function_version: u64,
    attr_bits: u64,
    can_shadow: bool,
    valid: bool,
}

impl MethodIcEntry {
    fn pin<'a, 'py>(self, py: &'a PyToken<'py>) -> PinnedMethodIcEntry<'a, 'py> {
        if self.valid {
            inc_ref_bits(py, self.attr_bits);
            inc_ref_bits(py, self.func_bits);
        }
        PinnedMethodIcEntry { py, entry: self }
    }

    /// Shadow lookup can run Python equality and change every lookup authority.
    /// Compare fresh receiver/type facts before dereferencing the recorded class.
    unsafe fn matches_receiver(self, recv_ptr: *mut u8) -> bool {
        unsafe {
            self.valid
                && type_epoch_matches(self.type_version)
                && object_class_bits(recv_ptr) == self.class_bits
                && cached_function_version_matches(self.func_bits, self.function_version)
                && obj_from_bits(self.class_bits)
                    .as_ptr()
                    .is_some_and(|class_ptr| {
                        class_layout_version_bits(class_ptr) == self.class_version
                    })
        }
    }
}

/// Own the selected method and name across shadow lookup, cache replacement,
/// and callee reentry. TLS residency alone cannot provide that lifetime.
struct PinnedMethodIcEntry<'a, 'py> {
    py: &'a PyToken<'py>,
    entry: MethodIcEntry,
}

impl Drop for PinnedMethodIcEntry<'_, '_> {
    fn drop(&mut self) {
        if self.entry.valid {
            dec_ref_bits(self.py, self.entry.attr_bits);
            dec_ref_bits(self.py, self.entry.func_bits);
        }
    }
}

const METHOD_IC_TLS_SIZE: usize = 256; // Must be power of 2.

#[inline]
fn method_ic_lookup<'a, 'py>(
    py: &'a PyToken<'py>,
    site_id: u64,
) -> Option<PinnedMethodIcEntry<'a, 'py>> {
    REF_OWNING_IC_TLS.with(|cache| {
        let cache = cache.borrow();
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        let (stored_id, entry) = cache.method[idx];
        if stored_id == site_id && entry.valid {
            Some(entry.pin(py))
        } else {
            None
        }
    })
}

/// Install a method-IC entry. The caller transfers its owned `attr_bits` ref;
/// the cache additionally retains `func_bits`, so concurrent class mutation
/// cannot free a target after the epoch check and before dispatch.
#[inline]
fn method_ic_insert(_py: &PyToken<'_>, site_id: u64, entry: MethodIcEntry) {
    inc_ref_bits(_py, entry.func_bits);
    let prev = REF_OWNING_IC_TLS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        let (_, prev) = cache.method[idx];
        cache.method[idx] = (site_id, entry);
        prev
    });
    if prev.valid {
        if prev.attr_bits != 0 {
            dec_ref_bits(_py, prev.attr_bits);
        }
        if prev.func_bits != 0 {
            dec_ref_bits(_py, prev.func_bits);
        }
    }
}

/// Detach this thread's owned method-cache edges and report whether any existed.
pub(crate) fn clear_method_ic_cache(_py: &PyToken<'_>) -> bool {
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        std::mem::replace(
            &mut cache.borrow_mut().method,
            [(0, EMPTY_METHOD_IC_ENTRY); METHOD_IC_TLS_SIZE],
        )
    });
    release_method_ic_entries(_py, &previous)
}

/// Per-site super dispatch cache. The start class is a live call operand,
/// not a call-site constant; all lookup identities and the callable are retained.
#[derive(Clone, Copy)]
struct SuperIcEntry {
    start_class_bits: u64,
    self_class_bits: u64,
    self_class_version: u64,
    type_version: u64,
    func_bits: u64,
    function_version: u64,
    attr_bits: u64,
    valid: bool,
}

impl SuperIcEntry {
    fn owned_edges(self) -> [u64; 4] {
        [
            self.start_class_bits,
            self.self_class_bits,
            self.func_bits,
            self.attr_bits,
        ]
    }

    fn retain(self, py: &PyToken<'_>) {
        if self.valid {
            for bits in self.owned_edges() {
                inc_ref_bits(py, bits);
            }
        }
    }

    fn release(self, py: &PyToken<'_>) {
        if self.valid {
            for bits in self.owned_edges() {
                dec_ref_bits(py, bits);
            }
        }
    }

    fn pin<'a, 'py>(self, py: &'a PyToken<'py>) -> PinnedSuperIcEntry<'a, 'py> {
        self.retain(py);
        PinnedSuperIcEntry { py, entry: self }
    }
}

/// Pin a resolved entry while it is installed: releasing the entry it replaces
/// can run finalizers that reenter the cache.
struct PinnedSuperIcEntry<'a, 'py> {
    py: &'a PyToken<'py>,
    entry: SuperIcEntry,
}

impl Drop for PinnedSuperIcEntry<'_, '_> {
    fn drop(&mut self) {
        self.entry.release(self.py);
    }
}

const EMPTY_CALL_IC_ENTRY: CallBindIcEntry = CallBindIcEntry {
    fn_ptr: 0,
    function_version: 0,
    target_bits: 0,
    class_bits: 0,
    class_version: 0,
    type_version: 0,
    cached_alloc_size: 0,
    arity: 0,
    kind: 0,
};
const EMPTY_METHOD_IC_ENTRY: MethodIcEntry = MethodIcEntry {
    class_bits: 0,
    class_version: 0,
    type_version: 0,
    func_bits: 0,
    function_version: 0,
    attr_bits: 0,
    can_shadow: true,
    valid: false,
};
const EMPTY_SUPER_IC_ENTRY: SuperIcEntry = SuperIcEntry {
    start_class_bits: 0,
    self_class_bits: 0,
    self_class_version: 0,
    type_version: 0,
    func_bits: 0,
    function_version: 0,
    attr_bits: 0,
    valid: false,
};

struct RefOwningIcTls {
    call: [(u64, CallBindIcEntry); IC_TLS_SIZE],
    method: [(u64, MethodIcEntry); METHOD_IC_TLS_SIZE],
    super_method: [(u64, SuperIcEntry); METHOD_IC_TLS_SIZE],
}

impl RefOwningIcTls {
    const fn new() -> Self {
        Self {
            call: [(0, EMPTY_CALL_IC_ENTRY); IC_TLS_SIZE],
            method: [(0, EMPTY_METHOD_IC_ENTRY); METHOD_IC_TLS_SIZE],
            super_method: [(0, EMPTY_SUPER_IC_ENTRY); METHOD_IC_TLS_SIZE],
        }
    }
}

impl Drop for RefOwningIcTls {
    fn drop(&mut self) {
        // Never touch another LocalKey here: Rust does not specify cross-key
        // destructor order. Worker wrappers and runtime teardown explicitly
        // drain owned refs under a live PyToken; at process death integer
        // carriers may be abandoned with the dying address space.
    }
}

thread_local! {
    static REF_OWNING_IC_TLS: std::cell::RefCell<RefOwningIcTls> =
        const { std::cell::RefCell::new(RefOwningIcTls::new()) };
}

/// Owned cache edges detached without running any decref or callback. The caller
/// publishes all derived function state before explicitly retiring this receipt.
#[must_use = "detached cache ownership must be released under a live PyToken"]
pub(crate) struct DetachedCallableIcCaches {
    entries: RefOwningIcTls,
}

impl DetachedCallableIcCaches {
    pub(crate) fn release(self, py: &PyToken<'_>) -> bool {
        let mut detached = release_call_ic_entries(py, &self.entries.call);
        detached |= release_method_ic_entries(py, &self.entries.method);
        detached |= release_super_ic_entries(py, &self.entries.super_method);
        detached
    }
}

/// One callback-free publication removes every callable cache on this thread.
/// Other threads reject old plans through the shared function mutation epoch.
pub(crate) fn detach_callable_ic_caches() -> DetachedCallableIcCaches {
    crate::gil_assert();
    let entries = REF_OWNING_IC_TLS
        .with(|cache| std::mem::replace(&mut *cache.borrow_mut(), RefOwningIcTls::new()));
    DetachedCallableIcCaches { entries }
}

fn release_call_ic_entries(py: &PyToken<'_>, entries: &[(u64, CallBindIcEntry)]) -> bool {
    let mut detached = false;
    for &(_, entry) in entries {
        if call_bind_ic_owns_target(entry) {
            detached = true;
            dec_ref_bits(py, entry.target_bits);
        }
    }
    detached
}

fn release_method_ic_entries(py: &PyToken<'_>, entries: &[(u64, MethodIcEntry)]) -> bool {
    let mut detached = false;
    for &(_, entry) in entries {
        if entry.valid {
            for bits in [entry.attr_bits, entry.func_bits] {
                if bits != 0 {
                    detached = true;
                    dec_ref_bits(py, bits);
                }
            }
        }
    }
    detached
}

fn release_super_ic_entries(py: &PyToken<'_>, entries: &[(u64, SuperIcEntry)]) -> bool {
    let mut detached = false;
    for &(_, entry) in entries {
        detached |= entry.valid;
        entry.release(py);
    }
    detached
}

/// Only dereference a live call operand or a cache-owned function edge.
unsafe fn cached_function_version_matches(func_bits: u64, recorded: u64) -> bool {
    unsafe {
        obj_from_bits(func_bits).as_ptr().is_some_and(|ptr| {
            object_type_id(ptr) == TYPE_ID_FUNCTION && function_mutation_version(ptr) == recorded
        })
    }
}

#[inline]
/// The entry at `site_id`, copied without retaining its edges. The cache owns
/// a reference to every edge. A hit check runs no Python code and does not
/// change the cache, so the edges stay live until the caller retains the
/// function it selects.
fn super_ic_entry(site_id: u64) -> Option<SuperIcEntry> {
    REF_OWNING_IC_TLS.with(|cache| {
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        let (stored_id, entry) = cache.borrow().super_method[idx];
        (stored_id == site_id && entry.valid).then_some(entry)
    })
}

#[inline]
fn super_ic_insert(_py: &PyToken<'_>, site_id: u64, selected: &PinnedSuperIcEntry<'_, '_>) {
    let entry = selected.entry;
    entry.retain(_py);
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        std::mem::replace(&mut cache.super_method[idx], (site_id, entry)).1
    });
    // Publish before release: weakref/finalizer callbacks can reenter this cache.
    previous.release(_py);
}

/// Detach this thread's owned super-cache edges and report whether any existed.
pub(crate) fn clear_super_ic_cache(_py: &PyToken<'_>) -> bool {
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        std::mem::replace(
            &mut cache.borrow_mut().super_method,
            [(0, EMPTY_SUPER_IC_ENTRY); METHOD_IC_TLS_SIZE],
        )
    });
    release_super_ic_entries(_py, &previous)
}

fn ic_site_from_bits(site_bits: u64) -> Option<u64> {
    let site = obj_from_bits(site_bits);
    if let Some(i) = site.as_int() {
        return u64::try_from(i).ok();
    }
    if site.is_bool() {
        return Some(if site.as_bool().unwrap_or(false) {
            1
        } else {
            0
        });
    }
    if site.is_ptr() || site.is_none() || site.is_pending() {
        return None;
    }
    Some(site_bits)
}

pub(super) unsafe fn call_bind_ic_entry_for_call(
    _py: &PyToken<'_>,
    call_bits: u64,
) -> Option<CallBindIcEntry> {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let call_ptr = call_obj.as_ptr()?;
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {
                if function_requires_full_binding(_py, call_ptr) {
                    if trace_call_bind_ic_enabled() {
                        let name_bits = function_name_bits(_py, call_ptr);
                        let name = if name_bits == 0 {
                            "<unnamed>".to_string()
                        } else {
                            string_obj_to_owned(obj_from_bits(name_bits))
                                .unwrap_or_else(|| "<unnamed>".to_string())
                        };
                        eprintln!(
                            "[molt call_bind_ic] bypass direct func name={} reason=full_binding_required",
                            name
                        );
                    }
                    return None;
                }
                let arity = function_arity(call_ptr);
                if arity <= 4 {
                    if trace_call_bind_ic_enabled() {
                        let name_bits = function_name_bits(_py, call_ptr);
                        let name = if name_bits == 0 {
                            "<unnamed>".to_string()
                        } else {
                            string_obj_to_owned(obj_from_bits(name_bits))
                                .unwrap_or_else(|| "<unnamed>".to_string())
                        };
                        eprintln!(
                            "[molt call_bind_ic] install direct func name={} arity={}",
                            name, arity
                        );
                    }
                    Some(CallBindIcEntry {
                        fn_ptr: function_fn_ptr(call_ptr),
                        function_version: function_mutation_version(call_ptr),
                        target_bits: call_bits,
                        class_bits: 0,
                        class_version: 0,
                        type_version: 0,
                        cached_alloc_size: 0,
                        arity: arity as u8,
                        kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
                    })
                } else {
                    if trace_call_bind_ic_enabled() {
                        let name_bits = function_name_bits(_py, call_ptr);
                        let name = if name_bits == 0 {
                            "<unnamed>".to_string()
                        } else {
                            string_obj_to_owned(obj_from_bits(name_bits))
                                .unwrap_or_else(|| "<unnamed>".to_string())
                        };
                        eprintln!(
                            "[molt call_bind_ic] bypass direct func name={} reason=arity_gt_4 arity={}",
                            name, arity
                        );
                    }
                    None
                }
            }
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(call_ptr);
                let func_ptr = obj_from_bits(func_bits).as_ptr()?;
                if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                    return None;
                }
                let fn_ptr = function_fn_ptr(func_ptr);
                if fn_ptr == fn_key!(molt_list_append) {
                    Some(CallBindIcEntry {
                        fn_ptr,
                        function_version: function_mutation_version(func_ptr),
                        target_bits: func_bits,
                        class_bits: 0,
                        class_version: 0,
                        type_version: 0,
                        cached_alloc_size: 0,
                        arity: 1,
                        kind: CALL_BIND_IC_KIND_LIST_APPEND,
                    })
                } else if !function_requires_full_binding(_py, func_ptr) {
                    let arity = function_arity(func_ptr);
                    if (1..=5).contains(&arity) {
                        Some(CallBindIcEntry {
                            fn_ptr,
                            function_version: function_mutation_version(func_ptr),
                            target_bits: func_bits,
                            class_bits: 0,
                            class_version: 0,
                            type_version: 0,
                            cached_alloc_size: 0,
                            arity: (arity - 1) as u8,
                            kind: CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            TYPE_ID_TYPE => {
                let class_bits = MoltObject::from_ptr(call_ptr).bits();
                // Builtin types have dedicated fast paths in call_type_with_arguments;
                // the IC is for user-defined classes only.
                if is_builtin_class_bits(_py, class_bits) {
                    trace_call_bind_ic_bypass("type_call", "builtin_class");
                    return None;
                }
                let type_version = crate::object::global_type_version();
                let layout_size = crate::object::layout::class_cached_layout_size(call_ptr)?;
                trace_call_bind_ic_epoch_stage(type_version, "layout_memo_read");
                // Cache installation must use the same metaclass-call policy as
                // the slow path. A custom metaclass __call__ owns construction
                // semantics and must never be bypassed by TYPE_CALL after the
                // first miss. Inspect the raw MRO descriptor without binding it:
                // the exact-string cache probe declines callback-bearing namespaces.
                let metaclass_bits = object_class_bits(call_ptr);
                let metaclass_ptr = obj_from_bits(metaclass_bits).as_ptr()?;
                if object_type_id(metaclass_ptr) != TYPE_ID_TYPE {
                    trace_call_bind_ic_bypass("type_call", "metaclass_not_type");
                    return None;
                }
                let metaclass_call_bits = crate::builtins::attr::class_namespace_lookup_mro_pure(
                    _py,
                    metaclass_ptr,
                    b"__call__",
                    None,
                )??;
                trace_call_bind_ic_epoch_stage(type_version, "metaclass_call_lookup");
                if !is_default_type_call(_py, metaclass_call_bits) {
                    trace_call_bind_ic_bypass("type_call", "custom_metaclass_call");
                    return None;
                }
                // Only cacheable when __new__ is the default object.__new__.
                let new_bits = crate::builtins::attr::class_namespace_lookup_mro_pure(
                    _py, call_ptr, b"__new__", None,
                )?;
                trace_call_bind_ic_epoch_stage(type_version, "new_lookup");
                if !resolved_new_is_default_object_new(new_bits) {
                    trace_call_bind_ic_bypass("type_call", "custom_new");
                    return None;
                }
                // Resolve __init__ and ensure it is a simple direct-callable function.
                let init_bits = crate::builtins::attr::class_namespace_lookup_mro_pure(
                    _py,
                    call_ptr,
                    b"__init__",
                    None,
                )??;
                trace_call_bind_ic_epoch_stage(type_version, "init_lookup");
                let init_ptr = obj_from_bits(init_bits).as_ptr()?;
                if object_type_id(init_ptr) != TYPE_ID_FUNCTION {
                    trace_call_bind_ic_bypass("type_call", "init_not_function");
                    return None;
                }
                if function_requires_full_binding(_py, init_ptr) {
                    trace_call_bind_ic_bypass("type_call", "init_requires_full_binding");
                    return None;
                }
                let init_arity = function_arity(init_ptr);
                // __init__ arity includes `self`, so cacheable range is 1..=5
                // (0 args up to 4 user args).
                if !(1..=5).contains(&init_arity) {
                    trace_call_bind_ic_bypass("type_call", "init_arity_out_of_range");
                    return None;
                }
                // Cache the allocation size so the IC fast path skips
                // the entire class_layout_size computation (MRO walks,
                // dict probes, name interning) on every instantiation.
                let total_alloc =
                    layout_size.checked_add(std::mem::size_of::<crate::object::MoltHeader>())?;
                if !type_resolution_epoch_is_stable(type_version) {
                    trace_call_bind_ic_bypass("type_call", "type_epoch_changed_during_resolution");
                    return None;
                }
                Some(CallBindIcEntry {
                    fn_ptr: function_fn_ptr(init_ptr),
                    function_version: function_mutation_version(init_ptr),
                    target_bits: init_bits,
                    class_bits,
                    class_version: class_layout_version_bits(call_ptr),
                    type_version,
                    cached_alloc_size: total_alloc,
                    arity: (init_arity - 1) as u8,
                    kind: CALL_BIND_IC_KIND_TYPE_CALL,
                })
            }
            _ => {
                let type_version = crate::object::global_type_version();
                // Eligibility is a raw type/MRO fact. Binding __call__ here
                // would invoke arbitrary descriptor code a second time after
                // the real slow call, potentially raising a spurious exception
                // or caching a dynamic descriptor result. Only the canonical
                // plain-function descriptor shape is safe for this fast path.
                let class_bits = object_class_bits(call_ptr);
                let class_ptr = obj_from_bits(class_bits).as_ptr()?;
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    return None;
                }
                let func_bits = crate::builtins::attr::class_namespace_lookup_mro_pure(
                    _py,
                    class_ptr,
                    b"__call__",
                    None,
                )??;
                let func_ptr = obj_from_bits(func_bits).as_ptr()?;
                if object_type_id(func_ptr) != TYPE_ID_FUNCTION
                    || function_requires_full_binding(_py, func_ptr)
                {
                    return None;
                }
                let arity = function_arity(func_ptr);
                if !(1..=5).contains(&arity) {
                    return None;
                }
                if !type_resolution_epoch_is_stable(type_version) {
                    return None;
                }
                Some(CallBindIcEntry {
                    fn_ptr: function_fn_ptr(func_ptr),
                    function_version: function_mutation_version(func_ptr),
                    target_bits: func_bits,
                    class_bits,
                    class_version: class_layout_version_bits(class_ptr),
                    type_version,
                    cached_alloc_size: 0,
                    arity: (arity - 1) as u8,
                    kind: CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
                })
            }
        }
    }
}

/// Cached exact-arity dispatch. An inlined frame's adopting entry takes the
/// call's positional values over as its parameters. Every other fast path
/// borrows them, and the argument vector releases them after the callee
/// returns, in the callee frame's own slot order for these shapes.
pub(super) unsafe fn try_call_bind_ic_fast(
    _py: &PyToken<'_>,
    entry: CallBindIcEntry,
    call_bits: u64,
    args: &mut CallArguments<'_, '_>,
) -> Option<u64> {
    unsafe {
        if args.keyword_count() != 0 {
            return None;
        }
        if !crate::builtins::functions::native_callable::admit_native_callable(
            _py,
            call_bits,
            args.positional(),
        ) {
            return Some(MoltObject::none().bits());
        }
        let positional = args.positional();

        let call_obj = obj_from_bits(call_bits);
        let call_ptr = call_obj.as_ptr()?;

        // Class epochs do not change when a function's own metadata changes.
        // Revalidate the shared executable/signature/defaults epoch and binder
        // flag before any cached already-bound ABI, including cross-thread hits.
        let metadata_target = match entry.kind {
            CALL_BIND_IC_KIND_DIRECT_FUNC => Some(call_ptr),
            CALL_BIND_IC_KIND_LIST_APPEND | CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC
                if object_type_id(call_ptr) == TYPE_ID_BOUND_METHOD =>
            {
                obj_from_bits(bound_method_func_bits(call_ptr)).as_ptr()
            }
            CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC | CALL_BIND_IC_KIND_TYPE_CALL => {
                obj_from_bits(entry.target_bits).as_ptr()
            }
            _ => None,
        };
        if metadata_target.is_some_and(|ptr| {
            object_type_id(ptr) == TYPE_ID_FUNCTION
                && (function_mutation_version(ptr) != entry.function_version
                    || function_requires_binder_flag(ptr))
        }) {
            return None;
        }

        if entry.kind == CALL_BIND_IC_KIND_LIST_APPEND {
            if object_type_id(call_ptr) != TYPE_ID_BOUND_METHOD || positional.len() != 1 {
                return None;
            }
            let func_bits = bound_method_func_bits(call_ptr);
            let func_ptr = obj_from_bits(func_bits).as_ptr()?;
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            if function_fn_ptr(func_ptr) != entry.fn_ptr {
                return None;
            }
            let self_bits = bound_method_self_bits(call_ptr);
            let arg0 = positional[0];
            return Some(molt_list_append(self_bits, arg0));
        }

        if entry.kind == CALL_BIND_IC_KIND_DIRECT_FUNC {
            if object_type_id(call_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            if function_fn_ptr(call_ptr) != entry.fn_ptr {
                return None;
            }
            if positional.len() != entry.arity as usize {
                return None;
            }
            if args.custody() == ArgumentCustody::Frame && function_bits_adopt_arguments(call_bits)
            {
                return Some(call_function_obj_moved(
                    _py,
                    call_bits,
                    args.surrender_positional(),
                ));
            }
            return Some(call_function_obj_bound_vec(_py, call_bits, positional));
        }

        if entry.kind == CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC {
            if object_type_id(call_ptr) != TYPE_ID_BOUND_METHOD {
                return None;
            }
            let func_bits = bound_method_func_bits(call_ptr);
            let func_ptr = obj_from_bits(func_bits).as_ptr()?;
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            if function_fn_ptr(func_ptr) != entry.fn_ptr {
                return None;
            }
            if positional.len() != entry.arity as usize {
                return None;
            }
            let self_bits = bound_method_self_bits(call_ptr);
            let mut argv = [0u64; 5];
            argv[0] = self_bits;
            if args.custody() == ArgumentCustody::Frame && function_bits_adopt_arguments(func_bits)
            {
                // CALL expands the bound method: the frame takes a new
                // reference to `__self__`, which the bound method keeps, and
                // the call's moved arguments.
                inc_ref_bits(_py, self_bits);
                let moved = args.surrender_positional();
                argv[1..=moved.len()].copy_from_slice(moved);
                return Some(call_function_obj_moved(
                    _py,
                    func_bits,
                    &argv[..moved.len() + 1],
                ));
            }
            for (idx, arg) in positional.iter().copied().enumerate() {
                argv[idx + 1] = arg;
            }
            return Some(call_function_obj_bound_vec(
                _py,
                func_bits,
                &argv[..positional.len() + 1],
            ));
        }

        if entry.kind == CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC {
            if !type_epoch_matches(entry.type_version) {
                return None;
            }
            let class_bits = object_class_bits(call_ptr);
            if class_bits != entry.class_bits {
                return None;
            }
            let class_ptr = obj_from_bits(class_bits).as_ptr()?;
            if class_layout_version_bits(class_ptr) != entry.class_version {
                return None;
            }
            let func_ptr = obj_from_bits(entry.target_bits).as_ptr()?;
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            if function_fn_ptr(func_ptr) != entry.fn_ptr {
                return None;
            }
            if positional.len() != entry.arity as usize {
                return None;
            }
            let mut argv = [0u64; 5];
            argv[0] = call_bits;
            for (idx, arg) in positional.iter().copied().enumerate() {
                argv[idx + 1] = arg;
            }
            return Some(call_function_obj_bound_vec(
                _py,
                entry.target_bits,
                &argv[..positional.len() + 1],
            ));
        }

        // IC fast path for user-class instantiation: TYPE_ID_TYPE with default
        // __new__ and a known simple __init__.  Skips the entire
        // call_type_with_arguments resolution (intern __new__/__init__, MRO
        // lookup, abstractmethod check, init-arg policy) and goes straight to
        // alloc + direct __init__ call.
        if entry.kind == CALL_BIND_IC_KIND_TYPE_CALL {
            if !type_epoch_matches(entry.type_version) {
                return None;
            }
            if object_type_id(call_ptr) != TYPE_ID_TYPE {
                return None;
            }
            let class_bits = MoltObject::from_ptr(call_ptr).bits();
            if class_bits != entry.class_bits {
                return None;
            }
            if class_layout_version_bits(call_ptr) != entry.class_version {
                return None;
            }
            if positional.len() != entry.arity as usize {
                return None;
            }
            // Verify the cached __init__ function pointer is still valid.
            let init_ptr = obj_from_bits(entry.target_bits).as_ptr()?;
            if object_type_id(init_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            if function_fn_ptr(init_ptr) != entry.fn_ptr {
                return None;
            }
            // Allocate instance using the IC-cached allocation size.
            // This skips the entire class_layout_size recomputation
            // (MRO walks, dict probes, issubclass checks) on every
            // instantiation — the layout was computed once when the IC
            // entry was populated.
            let inst_bits = if entry.cached_alloc_size > 0 {
                let total = entry.cached_alloc_size;
                crate::call::class_init::alloc_published_instance_for_class_with_total_size(
                    _py, call_ptr, total,
                )
            } else {
                let bits = alloc_instance_for_default_object_new(_py, call_ptr);
                if exception_pending(_py) {
                    return Some(MoltObject::none().bits());
                }
                bits
            };
            // Fast-path __init__ call: bypass call_function_obj_vec to skip
            // profiling, exception baseline, trampoline probe, arity check,
            // and double enforce_no_pending.  We already validated fn_ptr,
            // arity, and no-full-binding in call_bind_ic_entry_for_call.
            //
            // `inst_bits` is passed as a borrowed parameter. The constructor's
            // single owning reference is the result returned to the caller; an
            // extra retain here leaks finalizer-bearing instances at rc=1 after
            // the caller's TIR drop runs.
            //
            // `entry.fn_ptr` is the value stored in the function object's
            // identity slot, not necessarily an executable address. Synthetic
            // runtime functions carry a generated runtime-callable key there,
            // while compiler-emitted native functions publish their executable
            // address through the function call-target slot. The IC fast path
            // uses the same required-target authority as the slow fixed-arity
            // path and fails closed if construction did not initialize it.
            //
            // On wasm the fixed-arity call lowers through the function-table
            // trampoline (`molt_call_indirect*` / `fixed_arity_trampoline_target_ptr`),
            // not a raw-address `transmute`, so the native decode authority does
            // not apply there; the wasm arms below keep the stored `fn_ptr`
            // exactly as the surrounding wasm call paths already do.
            #[cfg(not(target_arch = "wasm32"))]
            let call_target = {
                let Some(call_target) = crate::call::function::function_required_call_target_ptr(
                    init_ptr,
                    entry.fn_ptr,
                ) else {
                    dec_ref_bits(_py, inst_bits);
                    return Some(raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "type-call inline cache function target is not initialized",
                    ));
                };
                call_target
            };
            #[cfg(target_arch = "wasm32")]
            let call_target = {
                let Some(call_target) = crate::provenance::abi::function_ptr(entry.fn_ptr) else {
                    dec_ref_bits(_py, inst_bits);
                    return Some(raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "type-call inline cache target exceeds the active address space",
                    ));
                };
                call_target
            };
            let closure_bits = function_execution_closure_bits(init_ptr);
            let Some(_recursion) = RecursionGuard::enter(_py) else {
                dec_ref_bits(_py, inst_bits);
                return Some(MoltObject::none().bits());
            };
            // Direct IC calls borrow `self` exactly like the generic function
            // call path. The freshly allocated instance's original ref remains
            // the constructor result; no extra callee-owned self lane exists.
            let Some(_invocation) = FrameInvocationGuard::for_function(_py, init_ptr) else {
                dec_ref_bits(_py, inst_bits);
                return Some(MoltObject::none().bits());
            };
            // The direct entry calls below bypass the runtime's borrowed lane,
            // so an adopting `__init__` receives its own reference to the
            // instance and to each argument here. Construction keeps the
            // instance as its result and the arguments as the call's.
            if positional.len() <= 3
                && crate::object::layout::function_entry_custody(init_ptr)
                    == crate::object::layout::EntryCustody::Adopting
            {
                inc_ref_bits(_py, inst_bits);
                for &bits in positional {
                    inc_ref_bits(_py, bits);
                }
            }
            let init_result = if closure_bits != 0 {
                match positional.len() {
                    0 => {
                        let f: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(call_target);
                        f(closure_bits, inst_bits) as u64
                    }
                    1 => {
                        let f: extern "C" fn(u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(closure_bits, inst_bits, positional[0]) as u64
                    }
                    2 => {
                        let f: extern "C" fn(u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(closure_bits, inst_bits, positional[0], positional[1]) as u64
                    }
                    3 => {
                        let f: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(
                            closure_bits,
                            inst_bits,
                            positional[0],
                            positional[1],
                            positional[2],
                        ) as u64
                    }
                    _ => {
                        let mut argv = [0u64; 5];
                        argv[0] = inst_bits;
                        for (idx, arg) in positional.iter().copied().enumerate() {
                            argv[idx + 1] = arg;
                        }
                        call_function_obj_bound_vec(
                            _py,
                            entry.target_bits,
                            &argv[..positional.len() + 1],
                        )
                    }
                }
            } else {
                match positional.len() {
                    0 => {
                        let f: extern "C" fn(u64) -> i64 = std::mem::transmute(call_target);
                        f(inst_bits) as u64
                    }
                    1 => {
                        let f: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(call_target);
                        f(inst_bits, positional[0]) as u64
                    }
                    2 => {
                        let f: extern "C" fn(u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(inst_bits, positional[0], positional[1]) as u64
                    }
                    3 => {
                        let f: extern "C" fn(u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(inst_bits, positional[0], positional[1], positional[2]) as u64
                    }
                    _ => {
                        let mut argv = [0u64; 5];
                        argv[0] = inst_bits;
                        for (idx, arg) in positional.iter().copied().enumerate() {
                            argv[idx + 1] = arg;
                        }
                        call_function_obj_bound_vec(
                            _py,
                            entry.target_bits,
                            &argv[..positional.len() + 1],
                        )
                    }
                }
            };
            drop(_invocation);
            drop(_recursion);
            // Same post-`__init__` resolution as every other constructor path:
            // consume and validate the owned result, then drop the instance and
            // yield `none` on either a pending exception or a non-None return.
            // The full-binding lane routes through the identical authority.
            return Some(crate::call::class_init::resolve_construct_after_init(
                _py,
                inst_bits,
                init_result,
            ));
        }

        None
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must provide a call-site id in `site_bits` and a valid callargs builder in
/// `builder_bits`.
pub extern "C" fn molt_call_bind_ic(site_bits: u64, call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_bind_ic_dispatch(_py, site_bits, call_bits, builder_bits) }
    })
}

/// `call_bind` and `call_indirect` of an ordinary source call (CPython's CALL,
/// a builder with the stack call form) whose instruction adopted its callable
/// as well as the builder. A temporary bound method ends before its function
/// runs; any other callable ends after the call. An expanded call
/// (CALL_FUNCTION_EX) keeps its callable and never reaches this entry.
///
/// # Safety
/// Caller must provide a call-site id in `site_bits` and a live CallArgs
/// builder in `builder_bits`; this call consumes both references.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_bind_ic_owned(
    site_bits: u64,
    call_bits: u64,
    builder_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let builder_ptr = ptr_from_bits(builder_bits);
            let builder_guard = PtrDropGuard::new(builder_ptr);
            // A pending error means argument preparation failed: the builder
            // ends as that call's value stack, then its callable.
            if exception_pending(_py) {
                drop(builder_guard);
                dec_ref_bits(_py, call_bits);
                return MoltObject::none().bits();
            }
            let args = match CallArguments::from_builder(_py, builder_ptr) {
                Ok(args) => args,
                Err(err) => {
                    drop(builder_guard);
                    dec_ref_bits(_py, call_bits);
                    return err;
                }
            };
            drop(builder_guard);
            if args.form != CallForm::Stack {
                drop(args);
                dec_ref_bits(_py, call_bits);
                return raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "an expanded call keeps its callable; it cannot adopt it",
                );
            }
            call_with_adopted_callable(_py, call_bits, args, |callable, arguments| {
                call_bind_ic_with_arguments(_py, site_bits, callable, arguments)
            })
        }
    })
}

/// One method load's selection, CPython's `LOAD_ATTR` result pair: the plain
/// function and its receiver when the attribute is a function on the
/// receiver's type, else the attribute and the null sentinel. Both words are
/// owned; the call that consumes the pair adopts them.
struct LoadedMethod {
    callable: u64,
    self_or_null: u64,
}

impl LoadedMethod {
    /// CPython's NULL: never a Python value, and a release of it is a no-op.
    const NULL: u64 = molt_codegen_abi::loaded_method_null_bits() as u64;

    /// The selected function bound to its receiver, each with its own reference.
    fn function(py: &PyToken<'_>, func_bits: u64, receiver_bits: u64) -> Self {
        inc_ref_bits(py, func_bits);
        inc_ref_bits(py, receiver_bits);
        Self {
            callable: func_bits,
            self_or_null: receiver_bits,
        }
    }

    /// Any other attribute, already owned.
    fn attribute(attribute_bits: u64) -> Self {
        Self {
            callable: attribute_bits,
            self_or_null: Self::NULL,
        }
    }

    /// An exception is pending. Neither word holds a reference.
    fn failed() -> Self {
        Self::attribute(MoltObject::none().bits())
    }
}

/// The class-side outcome of a method load on a heap receiver.
enum MethodSelection {
    /// A plain function found on the receiver's type (owned).
    Function(u64),
    /// The instance shadows the class function: its own attribute (owned).
    Shadow(u64),
    /// The cache cannot answer; the generic attribute lookup does.
    Generic,
}

/// Select a plain class function for `recv_ptr.name` through the per-site
/// method cache, without materializing a bound method. Err means an exception
/// is pending.
///
/// # Safety
/// `recv_ptr` must be live; the GIL must be held.
unsafe fn select_class_method(
    py: &PyToken<'_>,
    site_bits: u64,
    recv_ptr: *mut u8,
    name: &[u8],
) -> Result<MethodSelection, ()> {
    unsafe {
        // A hit validates the receiver class, its layout and the function
        // version, and runs the instance-shadow check only when the class
        // permits shadowing: no name interning and no MRO walk.
        if let Some(site_id) = ic_site_from_bits(site_bits)
            && let Some(selected) = method_ic_lookup(py, site_id)
        {
            let entry = selected.entry;
            if entry.matches_receiver(recv_ptr)
                && crate::object::ops_format::string_obj_matches_bytes(
                    obj_from_bits(entry.attr_bits),
                    name,
                )
                && let Some(class_ptr) = obj_from_bits(entry.class_bits).as_ptr()
            {
                if entry.can_shadow
                    && let Some(value) = crate::builtins::attr::object_instance_attribute(
                        py,
                        recv_ptr,
                        class_ptr,
                        entry.attr_bits,
                    )?
                {
                    return Ok(MethodSelection::Shadow(value));
                }
                // The shadow check may run Python equality, which can mutate
                // the namespace or detach the cache; this load keeps the
                // selection it made, as CPython's LOAD_ATTR does.
                inc_ref_bits(py, entry.func_bits);
                return Ok(MethodSelection::Function(entry.func_bits));
            }
        }

        // A miss resolves the method class-side and installs the cache.
        let Some(attr_bits) = attr_name_bits_from_bytes(py, name) else {
            return Ok(MethodSelection::Generic);
        };
        let type_version = crate::object::global_type_version();
        let selection =
            crate::builtins::attr::object_method_ic_resolve(py, recv_ptr, attr_bits, |info| {
                // Pin the selection inside the descriptor snapshot: cache
                // eviction may run code before resolution returns.
                let function_version = obj_from_bits(info.func_bits)
                    .as_ptr()
                    .filter(|ptr| object_type_id(*ptr) == TYPE_ID_FUNCTION)
                    .map_or(0, |ptr| function_mutation_version(ptr));
                MethodIcEntry {
                    class_bits: info.class_bits,
                    class_version: info.class_version,
                    type_version,
                    func_bits: info.func_bits,
                    function_version,
                    attr_bits,
                    can_shadow: info.can_shadow,
                    valid: true,
                }
                .pin(py)
            });
        let selected = match selection {
            Ok(Some(selected)) => selected,
            Ok(None) => {
                dec_ref_bits(py, attr_bits);
                return Ok(MethodSelection::Generic);
            }
            Err(()) => {
                dec_ref_bits(py, attr_bits);
                return Err(());
            }
        };
        let entry = selected.entry;
        if entry.can_shadow {
            let class_ptr = obj_from_bits(entry.class_bits)
                .as_ptr()
                .expect("selected class");
            match crate::builtins::attr::object_instance_attribute(
                py, recv_ptr, class_ptr, attr_bits,
            ) {
                Ok(Some(value)) => {
                    dec_ref_bits(py, attr_bits);
                    return Ok(MethodSelection::Shadow(value));
                }
                Ok(None) => {}
                Err(()) => {
                    dec_ref_bits(py, attr_bits);
                    return Err(());
                }
            }
        }
        // Cache eligibility may change during the instance lookup; the
        // semantic selection does not. Decline the insertion, keep the load.
        if entry.matches_receiver(recv_ptr)
            && let Some(site_id) = ic_site_from_bits(site_bits)
        {
            // The cache takes over this load's name reference.
            method_ic_insert(py, site_id, entry);
        } else {
            dec_ref_bits(py, attr_bits);
        }
        inc_ref_bits(py, entry.func_bits);
        Ok(MethodSelection::Function(entry.func_bits))
    }
}

/// CPython's `LOAD_ATTR` in method form (`_PyObject_GetMethod`). A plain
/// function on the receiver's type that the instance does not shadow loads as
/// `[function, receiver]`, with no bound method. A native method descriptor
/// also loads as `[descriptor, receiver]` through the
/// shared attribute transaction. Data descriptors, custom attribute hooks and
/// instance attributes return their actual value as `[attribute, NULL]`.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe the attribute name; the GIL is held.
unsafe fn load_method(
    py: &PyToken<'_>,
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
) -> LoadedMethod {
    unsafe {
        let Some(name) = crate::provenance::abi::slice(name_ptr, name_len_bits) else {
            let _ = raise_exception::<u64>(
                py,
                "RuntimeError",
                "method name range is invalid for the active target",
            );
            return LoadedMethod::failed();
        };
        if let Some(recv_ptr) = obj_from_bits(recv_bits).as_ptr() {
            match select_class_method(py, site_bits, recv_ptr, name) {
                Ok(MethodSelection::Function(func_bits)) => {
                    let loaded = LoadedMethod::function(py, func_bits, recv_bits);
                    dec_ref_bits(py, func_bits);
                    return loaded;
                }
                Ok(MethodSelection::Shadow(value)) => return LoadedMethod::attribute(value),
                Ok(MethodSelection::Generic) => {}
                Err(()) => return LoadedMethod::failed(),
            }
        }
        let receiver = std::cell::Cell::new(None);
        let selected = crate::builtins::attributes::get_attr_object_ic_with_mode(
            py,
            recv_bits,
            name,
            site_bits,
            crate::builtins::attr::AttributeLookupMode::Method(&receiver),
        );
        match selected {
            Ok(callable) => match receiver.get() {
                Some(receiver) => {
                    let loaded = LoadedMethod::function(py, callable, receiver);
                    dec_ref_bits(py, callable);
                    loaded
                }
                None => LoadedMethod::attribute(callable),
            },
            Err(()) => LoadedMethod::failed(),
        }
    }
}

/// CPython's `LOAD_DEREF __class__` and `LOAD_SUPER_ATTR` in method form for
/// zero-argument `super()`: the MRO of `type(first_arg)` after the class in
/// `class_cell_bits`. A plain function loads as `[function, first_arg]`
/// without a super object; anything else takes the attribute of
/// `super(start_class, first_arg)` and loads as `[attribute, NULL]`. An empty
/// cell raises LOAD_DEREF's `NameError`.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe the attribute name; the GIL is held.
unsafe fn load_super_method(
    py: &PyToken<'_>,
    site_bits: u64,
    class_cell_bits: u64,
    first_arg_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
) -> LoadedMethod {
    unsafe {
        let Some(name) = crate::provenance::abi::slice(name_ptr, name_len_bits) else {
            let _ = raise_exception::<u64>(
                py,
                "RuntimeError",
                "method name range is invalid for the active target",
            );
            return LoadedMethod::failed();
        };
        let Some(class_cell) = crate::object::cells::cell_ptr_from_bits(class_cell_bits) else {
            let _ = raise_exception::<u64>(
                py,
                "SystemError",
                "zero-argument super() operand is not the __class__ cell",
            );
            return LoadedMethod::failed();
        };
        // A hit validates the start class, `type(first_arg)` and its layout,
        // the type epoch and the function version: no super object, no name
        // interning, no MRO walk and no reference traffic on the cache edges.
        if let Some(first_arg_ptr) = obj_from_bits(first_arg_bits).as_ptr()
            && let Some(site_id) = ic_site_from_bits(site_bits)
            && let Some(entry) = super_ic_entry(site_id)
            && crate::object::cells::cell_value_bits(class_cell) == entry.start_class_bits
            && type_epoch_matches(entry.type_version)
            && cached_function_version_matches(entry.func_bits, entry.function_version)
            && match object_class_bits(first_arg_ptr) {
                0 => type_of_bits(py, first_arg_bits),
                class_bits => class_bits,
            } == entry.self_class_bits
            && crate::object::ops_format::string_obj_matches_bytes(
                obj_from_bits(entry.attr_bits),
                name,
            )
            && let Some(self_class_ptr) = obj_from_bits(entry.self_class_bits).as_ptr()
            && class_layout_version_bits(self_class_ptr) == entry.self_class_version
        {
            return LoadedMethod::function(py, entry.func_bits, first_arg_bits);
        }

        if crate::object::cells::cell_value_bits(class_cell) == missing_bits(py) {
            let _ = raise_exception::<u64>(
                py,
                "NameError",
                "cannot access free variable '__class__' where it is not associated \
                 with a value in enclosing scope",
            );
            return LoadedMethod::failed();
        }
        // Resolution can run Python code that rebinds the cell; the load
        // keeps the class it read, as CPython's operand stack does.
        let start_class = crate::object::cells::pin_cell_value(py, class_cell);
        let start_class_bits = start_class.bits();

        if let Some(attr_bits) = attr_name_bits_from_bytes(py, name) {
            let type_version = crate::object::global_type_version();
            let resolved = match crate::builtins::attr::super_resolve_method_unbound(
                py,
                start_class_bits,
                first_arg_bits,
                attr_bits,
            ) {
                Ok(resolved) => resolved,
                Err(()) => {
                    dec_ref_bits(py, attr_bits);
                    return LoadedMethod::failed();
                }
            };
            if let Some(info) = resolved {
                if type_resolution_epoch_is_stable(type_version) {
                    let selected = SuperIcEntry {
                        start_class_bits,
                        self_class_bits: info.self_class_bits,
                        self_class_version: info.self_class_version,
                        type_version,
                        func_bits: info.func_bits,
                        function_version: function_mutation_version(
                            obj_from_bits(info.func_bits)
                                .as_ptr()
                                .expect("resolved super function"),
                        ),
                        attr_bits,
                        valid: true,
                    }
                    .pin(py);
                    dec_ref_bits(py, info.func_bits);
                    dec_ref_bits(py, attr_bits);
                    if let Some(site_id) = ic_site_from_bits(site_bits) {
                        super_ic_insert(py, site_id, &selected);
                    }
                    return LoadedMethod::function(py, selected.entry.func_bits, first_arg_bits);
                }
                dec_ref_bits(py, info.func_bits);
            }
            dec_ref_bits(py, attr_bits);
        }

        // Any other attribute: a super object and its attribute, as CPython's
        // unspecialized LOAD_SUPER_ATTR does.
        let super_bits = molt_super_new(start_class_bits, first_arg_bits);
        if super_bits == 0 || exception_pending(py) {
            if super_bits != 0 {
                dec_ref_bits(py, super_bits);
            }
            return LoadedMethod::failed();
        }
        let super_ptr = obj_from_bits(super_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        let attribute = crate::molt_get_attr_generic(super_ptr, name_ptr, name_len_bits);
        dec_ref_bits(py, super_bits);
        if exception_pending(py) {
            dec_ref_bits(py, attribute);
            return LoadedMethod::failed();
        }
        LoadedMethod::attribute(attribute)
    }
}

/// Write a method load's `self_or_null` word and return its callable. The
/// out word is written last, after every Python callback the load ran, so a
/// target may pass a call scratch slot those callbacks reuse. Every path with
/// a valid address writes it; a failed load writes the null sentinel. An
/// address the target cannot reach raises without a write, so native and
/// LLVM code store the sentinel in their slot before the call, and WASM
/// passes a static scratch word.
fn publish_loaded_method(
    py: &PyToken<'_>,
    self_out_bits: u64,
    load: impl FnOnce() -> LoadedMethod,
) -> u64 {
    let Some(self_out) =
        crate::provenance::abi::mut_ptr::<u64>(self_out_bits).filter(|out| !out.is_null())
    else {
        return raise_exception::<u64>(
            py,
            "RuntimeError",
            "method load output address is invalid for the active target",
        );
    };
    let loaded = load();
    // SAFETY: the caller supplies writable storage for one word.
    unsafe {
        *self_out = loaded.self_or_null;
    }
    loaded.callable
}

/// C-ABI entry for CPython's `LOAD_ATTR` in method form at one call site.
/// Returns the callable and writes `self_or_null` to `self_out_bits`: a new
/// reference to the receiver, or the null sentinel
/// (`molt_codegen_abi::loaded_method_null_bits`). Both words are owned, and
/// the `molt_call_loaded_method*` entry that follows adopts them.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 name; `self_out_bits`
/// encodes writable storage for one `u64`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_load_method(
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    self_out_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        publish_loaded_method(_py, self_out_bits, || unsafe {
            load_method(_py, site_bits, recv_bits, name_ptr, name_len_bits)
        })
    })
}

/// C-ABI entry for CPython's `LOAD_DEREF __class__` and `LOAD_SUPER_ATTR` in
/// method form at one zero-argument `super()` site: `class_cell_bits` is the
/// `__class__` cell and `first_arg_bits` the method's first argument, both
/// borrowed. Returns the callable and writes `self_or_null` as
/// `molt_load_method` does.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 name; `self_out_bits`
/// encodes writable storage for one `u64`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_load_super_method(
    site_bits: u64,
    class_cell_bits: u64,
    first_arg_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    self_out_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        publish_loaded_method(_py, self_out_bits, || unsafe {
            load_super_method(
                _py,
                site_bits,
                class_cell_bits,
                first_arg_bits,
                name_ptr,
                name_len_bits,
            )
        })
    })
}

/// CPython's `CALL` after a method load. The instruction adopts every input:
/// `callable_bits`, `self_bits` and `args` belong to this call. With a
/// receiver, `callable_bits` is the plain function the load selected, entered
/// with `self` prepended and no bound method; with the null sentinel it is an
/// ordinary CALL of `callable_bits(*args)`. None of the inputs ever returns to
/// the caller.
///
/// # Safety
/// The GIL is held; every input is a reference this call owns.
unsafe fn call_loaded_method(
    py: &PyToken<'_>,
    callable_bits: u64,
    self_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe {
        if self_bits == LoadedMethod::NULL {
            return super::call_owned_arguments(py, callable_bits, args);
        }
        if exception_pending(py) {
            // Argument preparation failed: the inputs end as the ended
            // instruction's value stack, the callable last.
            release_stack_arguments(py, Some(self_bits), args);
            dec_ref_bits(py, callable_bits);
            return MoltObject::none().bits();
        }
        let result = call_owned_function(py, callable_bits, Some(self_bits), args);
        dec_ref_bits(py, callable_bits);
        result
    }
}

/// C-ABI entry for a loaded method call with no positional arguments.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_loaded_method0(callable_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_loaded_method(_py, callable_bits, self_bits, &[]) }
    })
}

/// C-ABI entry for a loaded method call with one positional argument.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_loaded_method1(callable_bits: u64, self_bits: u64, a0: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_loaded_method(_py, callable_bits, self_bits, &[a0]) }
    })
}

/// C-ABI entry for a loaded method call with two positional arguments.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_loaded_method2(
    callable_bits: u64,
    self_bits: u64,
    a0: u64,
    a1: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_loaded_method(_py, callable_bits, self_bits, &[a0, a1]) }
    })
}

/// C-ABI entry for a loaded method call with three positional arguments.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_loaded_method3(
    callable_bits: u64,
    self_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_loaded_method(_py, callable_bits, self_bits, &[a0, a1, a2]) }
    })
}

/// C-ABI entry for a loaded method call with four positional arguments.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_loaded_method4(
    callable_bits: u64,
    self_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_loaded_method(_py, callable_bits, self_bits, &[a0, a1, a2, a3]) }
    })
}

/// C-ABI entry for a loaded method call of any arity: the adopted positional
/// arguments are spilled at `args_ptr_bits[..nargs]`. The entry copies them
/// before any Python code can reuse the spill region.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_loaded_method_spilled(
    callable_bits: u64,
    self_bits: u64,
    args_ptr_bits: u64,
    nargs: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(args) = (unsafe { EntryArguments::copy(args_ptr_bits, nargs) }) else {
            // The arguments are unreachable; the loaded pair still ends here.
            dec_ref_bits(_py, self_bits);
            dec_ref_bits(_py, callable_bits);
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "call argument range is invalid for the active target",
            );
        };
        unsafe { call_loaded_method(_py, callable_bits, self_bits, args.as_slice()) }
    })
}

unsafe fn call_bind_ic_dispatch(
    _py: &PyToken<'_>,
    site_bits: u64,
    call_bits: u64,
    builder_bits: u64,
) -> u64 {
    unsafe {
        let builder_ptr = ptr_from_bits(builder_bits);
        let builder_guard = PtrDropGuard::new(builder_ptr);
        // A pending error means argument preparation failed: the builder is
        // still that call's value stack and releases as one.
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let args = match CallArguments::from_builder(_py, builder_ptr) {
            Ok(args) => args,
            Err(err) => return err,
        };
        // T1 is complete; the builder owns no argument edge any longer.
        drop(builder_guard);
        call_bind_ic_with_arguments(_py, site_bits, call_bits, args)
    }
}

/// Inline-cache dispatch for a call that owns its argument vector. A cache
/// hit borrows the vector for the callee; a miss hands it to the full binder.
unsafe fn call_bind_ic_with_arguments(
    _py: &PyToken<'_>,
    site_bits: u64,
    call_bits: u64,
    mut args: CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        let Some(site_id) = ic_site_from_bits(site_bits) else {
            return call_bind_with_arguments(_py, call_bits, args);
        };
        if disable_call_bind_ic_enabled() {
            if trace_call_bind_ic_enabled() {
                eprintln!(
                    "[molt call_bind_ic] bypass site={} reason=disabled_via_env",
                    site_id
                );
            }
            return call_bind_with_arguments(_py, call_bits, args);
        }
        // The call instruction decides custody from its callee before any
        // cached shortcut borrows the arguments.
        args.admit_custody(callee_custody(_py, call_bits, args.form));

        // Thread-local IC lookup — zero synchronization overhead on hits.
        if let Some(entry) = ic_tls_lookup(site_id)
            && let Some(res) = try_call_bind_ic_fast(_py, entry, call_bits, &mut args)
        {
            // An inlined frame's borrow made the vector its parameters.
            args.enter_inlined_frame();
            if trace_call_bind_ic_enabled() {
                let kind = match entry.kind {
                    CALL_BIND_IC_KIND_DIRECT_FUNC => "direct_func",
                    CALL_BIND_IC_KIND_LIST_APPEND => "list_append",
                    CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC => "bound_direct_func",
                    CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC => "heap_call_simple_bound_func",
                    CALL_BIND_IC_KIND_TYPE_CALL => "type_call",
                    _ => "unknown",
                };
                eprintln!(
                    "[molt call_bind_ic] hit site={} kind={} arity={} fn_ptr=0x{:x}",
                    site_id, kind, entry.arity, entry.fn_ptr,
                );
            }
            profile_hit_unchecked(&CALL_BIND_IC_HIT_COUNT);
            return res;
        }

        profile_hit_unchecked(&CALL_BIND_IC_MISS_COUNT);
        if trace_call_bind_ic_enabled() {
            let call_type = type_name(_py, obj_from_bits(call_bits));
            eprintln!(
                "[molt call_bind_ic] miss site={} callee_type={} pos_len={} kw_len={}",
                site_id,
                call_type,
                args.positional().len(),
                args.keyword_count()
            );
        }
        let res = call_bind_with_arguments(_py, call_bits, args);
        // Failed calls never populate the cache. Optional preparation itself
        // uses only callback-free metadata and declines undecidable probes.
        if !exception_pending(_py)
            && let Some(entry) = call_bind_ic_entry_for_call(_py, call_bits)
        {
            ic_tls_insert(_py, site_id, entry);
        }
        res
    }
}

fn bool_flag_from_bits(bits: u64) -> bool {
    let obj = obj_from_bits(bits);
    if let Some(v) = obj.as_int() {
        return v != 0;
    }
    if obj.is_bool() {
        return obj.as_bool().unwrap_or(false);
    }
    false
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must provide a call-site id in `site_bits` and a valid callargs builder in
/// `builder_bits`. When `require_bridge_cap_bits` is truthy, runtime enforces
/// `python.bridge` capability in non-trusted mode.
pub extern "C" fn molt_invoke_ffi_ic(
    site_bits: u64,
    call_bits: u64,
    builder_bits: u64,
    require_bridge_cap_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if bool_flag_from_bits(require_bridge_cap_bits) && !is_trusted(_py) {
            let bridge_allowed = has_capability(_py, "python.bridge");
            audit_capability_decision(
                "ffi.bridge",
                "python.bridge",
                AuditArgs::None,
                bridge_allowed,
            );
            if !bridge_allowed {
                profile_hit_unchecked(&INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT);
                return raise_exception::<_>(
                    _py,
                    "PermissionError",
                    "missing python.bridge capability",
                );
            }
        }
        unsafe { call_bind_ic_dispatch(_py, site_bits, call_bits, builder_bits) }
    })
}

#[cfg(test)]
mod super_cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static SHADOW_ARMED: AtomicBool = AtomicBool::new(false);
    static SHADOW_CLASS: AtomicU64 = AtomicU64::new(0);
    static SHADOW_NAME: AtomicU64 = AtomicU64::new(0);
    static SHADOW_HASH: AtomicU64 = AtomicU64::new(0);
    static SHADOW_CALLS: AtomicU64 = AtomicU64::new(0);

    extern "C" fn colliding_shadow_hash(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let bits = SHADOW_HASH.load(Ordering::SeqCst);
            inc_ref_bits(py, bits);
            bits
        })
    }

    extern "C" fn colliding_shadow_eq(_self: u64, _other: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            if SHADOW_ARMED.swap(false, Ordering::SeqCst) {
                SHADOW_CALLS.fetch_add(1, Ordering::SeqCst);
                let result = crate::molt_del_attr_name(
                    SHADOW_CLASS.load(Ordering::SeqCst),
                    SHADOW_NAME.load(Ordering::SeqCst),
                );
                dec_ref_bits(py, result);
                detach_callable_ic_caches().release(py);
            }
            MoltObject::from_bool(false).bits()
        })
    }

    extern "C" fn return_second(_self: u64, value: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, value);
            value
        })
    }

    /// `receiver.name(*args)` as compiled code runs it: a method load at
    /// `site_bits`, then the call, which adopts the loaded pair and its own
    /// reference to each argument.
    unsafe fn method_call(
        py: &PyToken<'_>,
        site_bits: u64,
        receiver: u64,
        name: &[u8],
        args: &[u64],
    ) -> u64 {
        unsafe {
            let loaded = load_method(py, site_bits, receiver, name.as_ptr(), name.len() as u64);
            for &arg in args {
                inc_ref_bits(py, arg);
            }
            call_loaded_method(py, loaded.callable, loaded.self_or_null, args)
        }
    }

    /// Zero-argument `super().name(*args)` in a method of `start_class`
    /// whose first argument is `receiver`, as compiled code runs it: the load
    /// reads a `__class__` cell that holds `start_class`.
    unsafe fn super_method_call(
        py: &PyToken<'_>,
        site_bits: u64,
        start_class: u64,
        receiver: u64,
        name: &[u8],
        args: &[u64],
    ) -> u64 {
        unsafe {
            let class_cell = crate::object::cells::alloc_cell(py, start_class);
            assert!(!class_cell.is_null());
            let class_cell_bits = MoltObject::from_ptr(class_cell).bits();
            let loaded = load_super_method(
                py,
                site_bits,
                class_cell_bits,
                receiver,
                name.as_ptr(),
                name.len() as u64,
            );
            dec_ref_bits(py, class_cell_bits);
            for &arg in args {
                inc_ref_bits(py, arg);
            }
            call_loaded_method(py, loaded.callable, loaded.self_or_null, args)
        }
    }

    unsafe fn signature_function(py: &PyToken<'_>, shape: &str) -> u64 {
        unsafe {
            let pointer = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::builtins::functions::runtime_fn_addr(
                    "cache_replacement_return_second",
                    return_second as *const (),
                ),
                2,
            );
            assert!(!pointer.is_null());
            let self_name = attr_name_bits_from_bytes(py, b"self").unwrap();
            let arg_name = attr_name_bits_from_bytes(py, b"replacement_arg").unwrap();
            let positional_names = if shape == "positional" {
                vec![self_name, arg_name]
            } else {
                vec![self_name]
            };
            let keyword_names = if shape == "kwonly" {
                vec![arg_name]
            } else {
                vec![]
            };
            let positional = crate::alloc_tuple(py, &positional_names);
            let keyword = crate::alloc_tuple(py, &keyword_names);
            assert!(!positional.is_null() && !keyword.is_null());
            let positional_bits = MoltObject::from_ptr(positional).bits();
            let keyword_bits = MoltObject::from_ptr(keyword).bits();
            for (name, value) in [
                (b"__molt_arg_names__".as_slice(), positional_bits),
                (b"__molt_posonly__", MoltObject::from_int(0).bits()),
                (b"__molt_kwonly_names__", keyword_bits),
                (
                    b"__molt_vararg__",
                    if shape == "vararg" {
                        arg_name
                    } else {
                        MoltObject::none().bits()
                    },
                ),
                (b"__molt_varkw__", MoltObject::none().bits()),
            ] {
                assert!(crate::call::class_init::function_set_attr_name(
                    py, pointer, name, value
                ));
            }
            for bits in [self_name, arg_name, positional_bits, keyword_bits] {
                dec_ref_bits(py, bits);
            }
            MoltObject::from_ptr(pointer).bits()
        }
    }

    #[test]
    fn super_code_replacement_binds_raw_arguments_on_cold_and_warm_sites() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                for warm in [false, true] {
                    for shape in ["positional", "kwonly", "vararg"] {
                        detach_callable_ic_caches().release(py);
                        let base =
                            make_class(py, b"RawSuperBase", builtin_classes(py).object, None);
                        let leaf = make_class(py, b"RawSuperLeaf", base, None);
                        let receiver = crate::call::class_init::alloc_instance_for_class(
                            py,
                            obj_from_bits(leaf).as_ptr().unwrap(),
                        );
                        let target = signature_function(py, "positional");
                        let source = signature_function(py, shape);
                        let method_name = attr_name_bits_from_bytes(py, b"value").unwrap();
                        crate::molt_set_attr_name(base, method_name, target);
                        let default = MoltObject::from_int(41).bits();
                        let defaults =
                            MoltObject::from_ptr(crate::alloc_tuple(py, &[default])).bits();
                        let arg_name = attr_name_bits_from_bytes(py, b"replacement_arg").unwrap();
                        let kwdefaults = MoltObject::from_ptr(crate::alloc_dict_with_pairs(
                            py,
                            &[arg_name, MoltObject::from_int(73).bits()],
                        ))
                        .bits();
                        for (name, value) in [
                            (b"__defaults__".as_slice(), defaults),
                            (b"__kwdefaults__", kwdefaults),
                        ] {
                            let name_bits = attr_name_bits_from_bytes(py, name).unwrap();
                            crate::molt_set_attr_name(target, name_bits, value);
                            dec_ref_bits(py, name_bits);
                        }
                        let site = MoltObject::from_int(821).bits();
                        let call = |args: &[u64]| {
                            super_method_call(py, site, leaf, receiver, b"value", args)
                        };
                        let arg = MoltObject::from_int(9).bits();
                        if warm {
                            let result = call(&[arg]);
                            assert_eq!(obj_from_bits(result).as_int(), Some(9));
                            dec_ref_bits(py, result);
                            assert!(super_ic_entry(821).is_some());
                        }
                        let code = crate::object::layout::ensure_function_code_bits(
                            py,
                            obj_from_bits(source).as_ptr().unwrap(),
                        );
                        let code_name = attr_name_bits_from_bytes(py, b"__code__").unwrap();
                        crate::molt_set_attr_name(target, code_name, code);
                        assert!(!exception_pending(py));
                        for _ in 0..2 {
                            for args in [&[][..], &[arg][..]] {
                                let result = call(args);
                                if shape == "kwonly" && !args.is_empty() {
                                    let error =
                                        crate::builtins::exceptions::molt_exception_last_pending();
                                    assert!(
                                        crate::builtins::exceptions::exception_matches_builtin_name(
                                            py,
                                            error,
                                            "TypeError"
                                        )
                                    );
                                    crate::molt_exception_clear();
                                    dec_ref_bits(py, error);
                                } else if shape == "vararg" {
                                    assert!(!exception_pending(py));
                                    let tuple = obj_from_bits(result).as_ptr().unwrap();
                                    assert_eq!(object_type_id(tuple), TYPE_ID_TUPLE);
                                    let values =
                                        crate::object::seq_access::pin_tuple(py, tuple).unwrap();
                                    assert_eq!(values.len(), args.len());
                                    assert!(values.iter().copied().eq(args.iter().copied()));
                                } else {
                                    assert!(!exception_pending(py));
                                    let expected = if shape == "kwonly" {
                                        73
                                    } else if args.is_empty() {
                                        41
                                    } else {
                                        9
                                    };
                                    assert_eq!(obj_from_bits(result).as_int(), Some(expected));
                                }
                                dec_ref_bits(py, result);
                                assert!(super_ic_entry(821).is_some());
                            }
                        }
                        detach_callable_ic_caches().release(py);
                        for bits in [
                            code_name,
                            arg_name,
                            defaults,
                            kwdefaults,
                            method_name,
                            target,
                            source,
                            receiver,
                            leaf,
                            base,
                        ] {
                            dec_ref_bits(py, bits);
                        }
                    }
                }
            }
        });
    }

    #[test]
    fn method_shadow_equality_can_delete_selected_function_and_detach_caches() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                for warm in [false, true] {
                    detach_callable_ic_caches().release(py);
                    let class = make_class(
                        py,
                        b"ShadowReentryOwner",
                        builtin_classes(py).object,
                        Some(base_value as *const ()),
                    );
                    let receiver = crate::call::class_init::alloc_instance_for_class(
                        py,
                        obj_from_bits(class).as_ptr().unwrap(),
                    );
                    let key_class =
                        make_class(py, b"ShadowReentryKey", builtin_classes(py).object, None);
                    for (name, target, arity) in [
                        (
                            b"__hash__".as_slice(),
                            colliding_shadow_hash as *const (),
                            1,
                        ),
                        (b"__eq__".as_slice(), colliding_shadow_eq as *const (), 2),
                    ] {
                        let name_bits = attr_name_bits_from_bytes(py, name).unwrap();
                        let function = crate::builtins::functions::alloc_runtime_function_obj(
                            py,
                            crate::provenance::abi::expose_function_address(target),
                            arity,
                        );
                        assert!(!function.is_null());
                        let function_bits = MoltObject::from_ptr(function).bits();
                        crate::molt_set_attr_name(key_class, name_bits, function_bits);
                        dec_ref_bits(py, name_bits);
                        dec_ref_bits(py, function_bits);
                    }
                    let key = crate::call::class_init::alloc_instance_for_class(
                        py,
                        obj_from_bits(key_class).as_ptr().unwrap(),
                    );
                    let name = attr_name_bits_from_bytes(py, b"value").unwrap();
                    let hash = crate::molt_hash_builtin(name);
                    SHADOW_HASH.store(hash, Ordering::SeqCst);
                    SHADOW_CLASS.store(class, Ordering::SeqCst);
                    SHADOW_NAME.store(name, Ordering::SeqCst);
                    SHADOW_CALLS.store(0, Ordering::SeqCst);
                    SHADOW_ARMED.store(false, Ordering::SeqCst);
                    let dict_name = attr_name_bits_from_bytes(py, b"__dict__").unwrap();
                    let dictionary = crate::molt_get_attr_name(receiver, dict_name);
                    let dict_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
                    assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);
                    assert!(crate::dict_set_in_place(
                        py,
                        dict_ptr,
                        key,
                        MoltObject::none().bits()
                    ));
                    assert!(!exception_pending(py));
                    let call = || {
                        method_call(
                            py,
                            MoltObject::from_int(822).bits(),
                            receiver,
                            b"value",
                            &[],
                        )
                    };
                    if warm {
                        let result = call();
                        assert_eq!(obj_from_bits(result).as_int(), Some(1));
                        dec_ref_bits(py, result);
                        assert!(method_ic_lookup(py, 822).is_some());
                    }
                    SHADOW_ARMED.store(true, Ordering::SeqCst);
                    let result = call();
                    assert_eq!(SHADOW_CALLS.load(Ordering::SeqCst), 1);
                    assert_eq!(obj_from_bits(result).as_int(), Some(1));
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, result);
                    assert!(method_ic_lookup(py, 822).is_none());
                    // Only the following operation sees the deleted method.
                    let result = call();
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "AttributeError"
                    ));
                    crate::molt_exception_clear();
                    dec_ref_bits(py, error);
                    dec_ref_bits(py, result);
                    assert!(method_ic_lookup(py, 822).is_none());
                    detach_callable_ic_caches().release(py);
                    for bits in [
                        dictionary, dict_name, hash, name, key, key_class, receiver, class,
                    ] {
                        dec_ref_bits(py, bits);
                    }
                    SHADOW_HASH.store(0, Ordering::SeqCst);
                    SHADOW_CLASS.store(0, Ordering::SeqCst);
                    SHADOW_NAME.store(0, Ordering::SeqCst);
                }
            }
        });
    }

    #[test]
    fn method_cache_snapshot_pins_selected_edges_across_detachment() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            detach_callable_ic_caches().release(py);
            let pointer = crate::alloc_list(py, &[]);
            assert!(!pointer.is_null());
            let bits = MoltObject::from_ptr(pointer).bits();
            let count =
                || unsafe { (*crate::object::header_from_obj_ptr(pointer)).ref_count_snapshot() };
            inc_ref_bits(py, bits); // The name owner transfers into the cache.
            method_ic_insert(
                py,
                823,
                MethodIcEntry {
                    func_bits: bits,
                    attr_bits: bits,
                    valid: true,
                    ..EMPTY_METHOD_IC_ENTRY
                },
            );
            assert_eq!(count(), 3);
            let selected = method_ic_lookup(py, 823).unwrap();
            assert_eq!(count(), 5);
            detach_callable_ic_caches().release(py);
            assert_eq!(
                count(),
                3,
                "the selected name and callable outlive TLS residency"
            );
            drop(selected);
            assert_eq!(count(), 1);
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn cached_direct_call_rechecks_live_metadata_after_set_and_delete() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let pointer = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "metadata_cache_base_value",
                        base_value as *const (),
                    ),
                    1,
                );
                assert!(!pointer.is_null());
                let function = MoltObject::from_ptr(pointer).bits();
                let entry = call_bind_ic_entry_for_call(py, function).unwrap();
                let mut args =
                    CallArguments::retained(py, None, &[MoltObject::from_int(3).bits()], &[], &[])
                        .unwrap();
                assert_eq!(
                    try_call_bind_ic_fast(py, entry, function, &mut args),
                    Some(MoltObject::from_int(1).bits())
                );
                let name = attr_name_bits_from_bytes(py, b"__defaults__").unwrap();
                let defaults = crate::alloc_tuple(py, &[MoltObject::from_int(5).bits()]);
                assert!(!defaults.is_null());
                let defaults = MoltObject::from_ptr(defaults).bits();
                crate::molt_set_attr_name(function, name, defaults);
                dec_ref_bits(py, defaults);
                assert!(!exception_pending(py));
                assert!(try_call_bind_ic_fast(py, entry, function, &mut args).is_none());
                crate::molt_del_attr_name(function, name);
                assert!(!exception_pending(py));
                assert!(try_call_bind_ic_fast(py, entry, function, &mut args).is_none());
                let refreshed = call_bind_ic_entry_for_call(py, function).unwrap();
                assert_eq!(
                    try_call_bind_ic_fast(py, refreshed, function, &mut args),
                    Some(MoltObject::from_int(1).bits())
                );
                drop(args);
                for bits in [name, function] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn callable_cache_detach_publishes_all_empty_before_releasing_any_edge() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            detach_callable_ic_caches().release(py);
            let pointer = crate::alloc_list(py, &[]);
            assert!(!pointer.is_null());
            let bits = MoltObject::from_ptr(pointer).bits();
            let count =
                || unsafe { (*crate::object::header_from_obj_ptr(pointer)).ref_count_snapshot() };
            ic_tls_insert(
                py,
                811,
                CallBindIcEntry {
                    target_bits: bits,
                    kind: CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC,
                    ..EMPTY_CALL_IC_ENTRY
                },
            );
            inc_ref_bits(py, bits); // Transfer the method cache's owned name edge.
            method_ic_insert(
                py,
                812,
                MethodIcEntry {
                    func_bits: bits,
                    attr_bits: bits,
                    valid: true,
                    ..EMPTY_METHOD_IC_ENTRY
                },
            );
            let selected = SuperIcEntry {
                start_class_bits: bits,
                self_class_bits: bits,
                func_bits: bits,
                attr_bits: bits,
                valid: true,
                ..EMPTY_SUPER_IC_ENTRY
            }
            .pin(py);
            super_ic_insert(py, 813, &selected);
            drop(selected);
            assert_eq!(count(), 8);

            let detached = detach_callable_ic_caches();
            assert!(ic_tls_lookup(811).is_none());
            assert!(method_ic_lookup(py, 812).is_none());
            assert!(super_ic_entry(813).is_none());
            assert_eq!(count(), 8, "detachment cannot run a finalizer");
            assert!(detached.release(py));
            assert_eq!(
                count(),
                1,
                "retirement releases each displaced owner exactly once"
            );
            assert!(!detach_callable_ic_caches().release(py));
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn method_and_super_cache_versions_reject_mutation_without_tls_flush() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            detach_callable_ic_caches().release(py);
            let base = make_class(
                py,
                b"VersionedCacheBase",
                builtin_classes(py).object,
                Some(base_value as *const ()),
            );
            let leaf = make_class(py, b"VersionedCacheLeaf", base, None);
            let receiver = unsafe {
                crate::call::class_init::alloc_instance_for_class(
                    py,
                    obj_from_bits(leaf).as_ptr().unwrap(),
                )
            };
            let name = b"value";
            let method_site = MoltObject::from_int(814).bits();
            let super_site = MoltObject::from_int(815).bits();
            let method_call = || unsafe { method_call(py, method_site, receiver, name, &[]) };
            let super_call =
                || unsafe { super_method_call(py, super_site, leaf, receiver, name, &[]) };
            for result in [method_call(), super_call()] {
                assert_eq!(obj_from_bits(result).as_int(), Some(1));
                dec_ref_bits(py, result);
            }
            assert!(!exception_pending(py));
            let selected = method_ic_lookup(py, 814).expect("must warm method shape");
            let cached = selected.entry;
            let defaults_name = attr_name_bits_from_bytes(py, b"__defaults__").unwrap();
            let defaults_ptr = crate::alloc_tuple(py, &[MoltObject::from_int(99).bits()]);
            assert!(!defaults_ptr.is_null());
            let defaults = MoltObject::from_ptr(defaults_ptr).bits();
            crate::molt_set_attr_name(cached.func_bits, defaults_name, defaults);
            assert!(!exception_pending(py));
            let version = unsafe {
                function_mutation_version(obj_from_bits(cached.func_bits).as_ptr().unwrap())
            };
            assert_ne!(version, cached.function_version);
            // Deliberately retain stale TLS entries, just as a different thread's
            // cache survives a shared function mutation on the publishing thread.
            assert_eq!(
                method_ic_lookup(py, 814).unwrap().entry.function_version,
                cached.function_version
            );
            assert_eq!(
                super_ic_entry(815).unwrap().function_version,
                cached.function_version
            );
            for result in [method_call(), super_call()] {
                assert_eq!(obj_from_bits(result).as_int(), Some(1));
                dec_ref_bits(py, result);
            }
            assert!(!exception_pending(py));
            assert_eq!(
                method_ic_lookup(py, 814).unwrap().entry.function_version,
                version
            );
            assert_eq!(super_ic_entry(815).unwrap().function_version, version);
            detach_callable_ic_caches().release(py);
            for bits in [defaults_name, defaults, receiver, leaf, base] {
                dec_ref_bits(py, bits);
            }
        });
    }

    extern "C" fn base_value(_self_bits: u64) -> u64 {
        MoltObject::from_int(1).bits()
    }

    extern "C" fn mid_value(_self_bits: u64) -> u64 {
        MoltObject::from_int(2).bits()
    }

    fn make_class(py: &PyToken<'_>, name: &[u8], base: u64, method: Option<*const ()>) -> u64 {
        let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
        let bases = crate::alloc_tuple(py, &[base]);
        let ns = crate::alloc_dict_with_pairs(py, &[]);
        assert!(!bases.is_null() && !ns.is_null());
        let bases_bits = MoltObject::from_ptr(bases).bits();
        let ns_bits = MoltObject::from_ptr(ns).bits();
        if let Some(method) = method {
            let function = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::provenance::abi::expose_function_address(method),
                1,
            );
            assert!(!function.is_null());
            let function_bits = MoltObject::from_ptr(function).bits();
            let method_name = crate::attr_name_bits_from_bytes(py, b"value").unwrap();
            unsafe {
                assert!(crate::dict_set_in_place(py, ns, method_name, function_bits));
            }
            dec_ref_bits(py, method_name);
            dec_ref_bits(py, function_bits);
        }
        let result = crate::molt_type_new(
            builtin_classes(py).type_obj,
            name,
            bases_bits,
            ns_bits,
            MoltObject::none().bits(),
        );
        for bits in [name, bases_bits, ns_bits] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
        result
    }

    #[test]
    fn warm_super_site_rechecks_start_class_and_rejects_non_type() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            clear_super_ic_cache(py);
            let base = make_class(
                py,
                b"SuperCacheBase",
                builtin_classes(py).object,
                Some(base_value as *const ()),
            );
            let mid = make_class(py, b"SuperCacheMid", base, Some(mid_value as *const ()));
            let leaf = make_class(py, b"SuperCacheLeaf", mid, None);
            let receiver = unsafe {
                crate::call::class_init::alloc_instance_for_class(
                    py,
                    obj_from_bits(leaf).as_ptr().unwrap(),
                )
            };
            assert!(!exception_pending(py));
            let site = MoltObject::from_int(817).bits();
            let name = b"value";
            for (start, expected) in [(leaf, 2), (mid, 1), (leaf, 2)] {
                let result = unsafe { super_method_call(py, site, start, receiver, name, &[]) };
                assert!(!exception_pending(py));
                assert_eq!(obj_from_bits(result).as_int(), Some(expected));
                let cached = super_ic_entry(817).expect("regression must warm the IC");
                assert_eq!(cached.start_class_bits, start);
                dec_ref_bits(py, result);
            }
            let result = unsafe {
                super_method_call(
                    py,
                    site,
                    MoltObject::from_int(42).bits(),
                    receiver,
                    name,
                    &[],
                )
            };
            assert!(obj_from_bits(result).is_none());
            let error = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "TypeError"
            ));
            crate::molt_exception_clear();
            dec_ref_bits(py, error);
            dec_ref_bits(py, result);
            clear_super_ic_cache(py);
            for bits in [receiver, leaf, mid, base] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn super_cache_balances_every_edge_and_a_hit_lookup_retains_nothing() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            clear_super_ic_cache(py);
            let pointer = crate::alloc_list(py, &[]);
            assert!(!pointer.is_null());
            let bits = MoltObject::from_ptr(pointer).bits();
            let count =
                || unsafe { (*crate::object::header_from_obj_ptr(pointer)).ref_count_snapshot() };
            let entry = SuperIcEntry {
                start_class_bits: bits,
                self_class_bits: bits,
                self_class_version: 0,
                type_version: 0,
                func_bits: bits,
                function_version: 0,
                attr_bits: bits,
                valid: true,
            }
            .pin(py);
            assert_eq!(count(), 5);
            super_ic_insert(py, 819, &entry);
            assert_eq!(count(), 9);
            let hit = super_ic_entry(819).expect("inserted entry");
            assert_eq!(hit.func_bits, bits);
            assert_eq!(
                count(),
                9,
                "a hit lookup copies the entry without retaining"
            );
            super_ic_insert(py, 819, &entry);
            assert_eq!(count(), 9, "replacement must balance every retained edge");
            clear_super_ic_cache(py);
            assert_eq!(count(), 5);
            drop(entry);
            assert_eq!(count(), 1);
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn super_method_load_of_an_empty_class_cell_raises_load_deref_name_error() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            clear_super_ic_cache(py);
            let receiver = crate::alloc_list(py, &[]);
            assert!(!receiver.is_null());
            let receiver_bits = MoltObject::from_ptr(receiver).bits();
            let class_cell = crate::object::cells::alloc_cell(py, missing_bits(py));
            assert!(!class_cell.is_null());
            let class_cell_bits = MoltObject::from_ptr(class_cell).bits();
            let name = b"append";
            let loaded = unsafe {
                load_super_method(
                    py,
                    MoltObject::from_int(823).bits(),
                    class_cell_bits,
                    receiver_bits,
                    name.as_ptr(),
                    name.len() as u64,
                )
            };
            assert!(obj_from_bits(loaded.callable).is_none());
            assert_eq!(loaded.self_or_null, LoadedMethod::NULL);
            let error = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "NameError"
            ));
            assert_eq!(
                crate::format_exception_message(py, obj_from_bits(error).as_ptr().unwrap()),
                "cannot access free variable '__class__' where it is not associated with a \
                 value in enclosing scope"
            );
            assert!(super_ic_entry(823).is_none());
            crate::molt_exception_clear();
            dec_ref_bits(py, error);
            dec_ref_bits(py, class_cell_bits);
            dec_ref_bits(py, receiver_bits);
        });
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must provide a call-site id in `site_bits` and a valid callargs builder in
/// `builder_bits`.
pub extern "C" fn molt_call_indirect_ic(site_bits: u64, call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_bind_ic_dispatch(_py, site_bits, call_bits, builder_bits) }
    })
}

#[cfg(test)]
#[path = "inline_cache_binding_tests.rs"]
mod binding_tests;
