// Call-site inline-cache authority for call binding, fused method dispatch,
// fused super dispatch, C-ABI IC entry points, and cache lifecycle.

use super::*;
fn trace_call_bind_ic_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_CALL_BIND_IC").as_deref() == Ok("1"))
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
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DISABLE_CALL_BIND_IC").as_deref() == Ok("1"))
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

pub(crate) fn clear_call_bind_ic_cache(_py: &PyToken<'_>) {
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        std::mem::replace(
            &mut cache.borrow_mut().call,
            [(
                0u64,
                CallBindIcEntry {
                    fn_ptr: 0,
                    target_bits: 0,
                    class_bits: 0,
                    class_version: 0,
                    type_version: 0,
                    cached_alloc_size: 0,
                    arity: 0,
                    kind: 0,
                },
            ); IC_TLS_SIZE],
        )
    });
    for (_, entry) in previous {
        if call_bind_ic_owns_target(entry) {
            dec_ref_bits(_py, entry.target_bits);
        }
    }
}

/// Per-site inline cache for fused method / super-method dispatch.
///
/// Keyed on the call-site id, validated against `(class_bits, class_version)`.
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
    attr_bits: u64,
    can_shadow: bool,
    /// Fixed positional parameter count of `func_bits` INCLUDING `self` (the
    /// runtime call ABI arity). The fused `call_direct` fast path invokes the
    /// compiled trampoline at exactly this arity, padding any missing trailing
    /// positionals from the LIVE `__defaults__`. Computed once at insert so the
    /// hit path is a couple of integer compares.
    pub(super) fixed_arity: u8,
    /// `len(__defaults__)` — the number of trailing positional parameters with a
    /// default, so the direct path may supply `[fixed_arity - n_pos_defaults,
    /// fixed_arity]` positionals. 0 when `needs_binder` (irrelevant then).
    pub(super) n_pos_defaults: u8,
    /// Whether `func_bits` needs the full binder: keyword-only params, keyword-
    /// only defaults, `*args`, `**kwargs`, or a builtin bind-kind. When true the
    /// `call_direct` fast path MUST NOT be taken — e.g. `runner.run(coro)`
    /// against `def run(self, coro, *, context=None)` cannot fill the kw-only
    /// `context` default via positional padding and would raise a spurious
    /// `call arity mismatch`. Positional defaults alone do NOT set this.
    pub(super) needs_binder: bool,
    valid: bool,
}

/// Static call-shape plan for a resolved method, used by the fused method-call
/// IC to choose between the allocation-free direct fast path and the full
/// binder.
#[derive(Clone, Copy)]
pub(super) struct MethodIcCallPlan {
    /// Fixed positional parameter count INCLUDING `self` (the runtime call ABI
    /// arity; matches `function_arity`).
    pub(super) fixed_arity: u8,
    /// Number of trailing positional parameters that carry a default
    /// (`len(__defaults__)`), saturated at `u8::MAX`. The direct fast path can
    /// supply anywhere from `fixed_arity - n_pos_defaults` to `fixed_arity`
    /// positionals: the compiled trampoline is invoked at exactly `fixed_arity`
    /// after the IC pads the missing trailing positionals from the LIVE
    /// `__defaults__` tuple.
    pub(super) n_pos_defaults: u8,
    /// Whether this method needs the full argument binder — i.e. it has
    /// keyword-only parameters, `*args`, or `**kwargs`. Positional defaults do
    /// NOT set this: they are fillable allocation-free by the direct path.
    /// Kw-only / vararg / varkw require the binder's keyword routing, vararg
    /// tuple collection, and varkw dict, which the direct path cannot do.
    pub(super) needs_binder: bool,
}

/// Compute the [`MethodIcCallPlan`] for a resolved method function. Returns
/// `None` when `func_bits` is not a plain function object (the fast path is
/// function-only).
///
/// The split between "positional defaults" (direct-fillable) and "needs binder"
/// (kw-only/`*args`/`**kwargs`) is the load-bearing distinction: a method with
/// ONLY positional defaults stays on the allocation-free direct path (the
/// compiled trampoline + IC default-padding), beating the allocating binder.
///
/// # Safety
/// `func_bits` must be a live object reference; the GIL must be held.
pub(super) unsafe fn method_ic_call_plan(
    _py: &PyToken<'_>,
    func_bits: u64,
) -> Option<MethodIcCallPlan> {
    unsafe {
        let func_ptr = obj_from_bits(func_bits).as_ptr()?;
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return None;
        }
        let fixed_arity = function_arity(func_ptr).min(u8::MAX as u64) as u8;
        let needs_binder = function_needs_full_binder(_py, func_ptr);
        let n_pos_defaults = if needs_binder {
            // Irrelevant: a needs-binder method never takes the direct path.
            0
        } else {
            function_positional_default_count(_py, func_ptr).min(u8::MAX as usize) as u8
        };
        Some(MethodIcCallPlan {
            fixed_arity,
            n_pos_defaults,
            needs_binder,
        })
    }
}

const METHOD_IC_TLS_SIZE: usize = 256; // Must be power of 2.

#[inline]
fn method_ic_lookup(site_id: u64) -> Option<MethodIcEntry> {
    REF_OWNING_IC_TLS.with(|cache| {
        let cache = cache.borrow();
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        let (stored_id, entry) = cache.method[idx];
        if stored_id == site_id && entry.valid {
            Some(entry)
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

pub(crate) fn clear_method_ic_cache(_py: &PyToken<'_>) {
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        std::mem::replace(
            &mut cache.borrow_mut().method,
            [(
                0u64,
                MethodIcEntry {
                    class_bits: 0,
                    class_version: 0,
                    type_version: 0,
                    func_bits: 0,
                    attr_bits: 0,
                    can_shadow: true,
                    fixed_arity: 0,
                    n_pos_defaults: 0,
                    needs_binder: true,
                    valid: false,
                },
            ); METHOD_IC_TLS_SIZE],
        )
    });
    for (_, entry) in previous {
        if entry.valid {
            if entry.attr_bits != 0 {
                dec_ref_bits(_py, entry.attr_bits);
            }
            if entry.func_bits != 0 {
                dec_ref_bits(_py, entry.func_bits);
            }
        }
    }
}

/// Per-site inline cache for fused `super().method(args)` dispatch.  Keyed on
/// the call-site id (which fixes the defining `__class__`) and validated against
/// `(type(self), version)`.  Super resolution bypasses the instance dict, so no
/// shadow check is needed; the IC reuses the resolved class function and the
/// pre-interned attribute name on hits.
#[derive(Clone, Copy)]
struct SuperIcEntry {
    self_class_bits: u64,
    self_class_version: u64,
    type_version: u64,
    func_bits: u64,
    attr_bits: u64,
    valid: bool,
}

const EMPTY_CALL_IC_ENTRY: CallBindIcEntry = CallBindIcEntry {
    fn_ptr: 0,
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
    attr_bits: 0,
    can_shadow: true,
    fixed_arity: 0,
    n_pos_defaults: 0,
    needs_binder: true,
    valid: false,
};
const EMPTY_SUPER_IC_ENTRY: SuperIcEntry = SuperIcEntry {
    self_class_bits: 0,
    self_class_version: 0,
    type_version: 0,
    func_bits: 0,
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

#[inline]
fn super_ic_lookup(site_id: u64) -> Option<SuperIcEntry> {
    REF_OWNING_IC_TLS.with(|cache| {
        let cache = cache.borrow();
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        let (stored_id, entry) = cache.super_method[idx];
        if stored_id == site_id && entry.valid {
            Some(entry)
        } else {
            None
        }
    })
}

#[inline]
pub(super) unsafe fn cached_attr_matches_bytes(attr_bits: u64, expected: &[u8]) -> bool {
    unsafe {
        let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr() else {
            return false;
        };
        if object_type_id(attr_ptr) != TYPE_ID_STRING
            || crate::string_len(attr_ptr) != expected.len()
        {
            return false;
        }
        std::slice::from_raw_parts(crate::string_bytes(attr_ptr), expected.len()) == expected
    }
}

#[inline]
fn super_ic_insert(_py: &PyToken<'_>, site_id: u64, entry: SuperIcEntry) {
    inc_ref_bits(_py, entry.func_bits);
    let prev = REF_OWNING_IC_TLS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let idx = (site_id as usize) & (METHOD_IC_TLS_SIZE - 1);
        let (_, prev) = cache.super_method[idx];
        cache.super_method[idx] = (site_id, entry);
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

pub(crate) fn clear_super_ic_cache(_py: &PyToken<'_>) {
    let previous = REF_OWNING_IC_TLS.with(|cache| {
        std::mem::replace(
            &mut cache.borrow_mut().super_method,
            [(
                0u64,
                SuperIcEntry {
                    self_class_bits: 0,
                    self_class_version: 0,
                    type_version: 0,
                    func_bits: 0,
                    attr_bits: 0,
                    valid: false,
                },
            ); METHOD_IC_TLS_SIZE],
        )
    });
    for (_, entry) in previous {
        if entry.valid {
            if entry.attr_bits != 0 {
                dec_ref_bits(_py, entry.attr_bits);
            }
            if entry.func_bits != 0 {
                dec_ref_bits(_py, entry.func_bits);
            }
        }
    }
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
                if fn_ptr == fn_addr!(molt_list_append) {
                    Some(CallBindIcEntry {
                        fn_ptr,
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
                // Builtin types have dedicated fast paths in call_type_with_builder;
                // the IC is for user-defined classes only.
                if is_builtin_class_bits(_py, class_bits) {
                    trace_call_bind_ic_bypass("type_call", "builtin_class");
                    return None;
                }
                // The first layout lookup may publish the class's lazy internal
                // layout-size memo and therefore advance the global type epoch.
                // Warm that memo before opening the optimistic MRO-resolution
                // transaction, then read it again inside the sampled epoch. The
                // second read is the cheap memoized path; any genuine concurrent
                // mutation during it or the following descriptor lookups still
                // makes the final epoch check reject publication.
                let _ = crate::call::class_init::class_layout_size_cached(_py, call_ptr);
                let type_version = crate::object::global_type_version();
                let layout_size = crate::call::class_init::class_layout_size_cached(_py, call_ptr)?;
                trace_call_bind_ic_epoch_stage(type_version, "layout_memo_read");
                // Cache installation must use the same metaclass-call policy as
                // the slow path. A custom metaclass __call__ owns construction
                // semantics and must never be bypassed by TYPE_CALL after the
                // first miss. Inspect the raw MRO descriptor without binding it:
                // cache classification is observationally pure.
                let metaclass_bits = object_class_bits(call_ptr);
                let metaclass_ptr = obj_from_bits(metaclass_bits).as_ptr()?;
                if object_type_id(metaclass_ptr) != TYPE_ID_TYPE {
                    trace_call_bind_ic_bypass("type_call", "metaclass_not_type");
                    return None;
                }
                let call_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
                let metaclass_call_bits =
                    class_attr_lookup_raw_mro(_py, metaclass_ptr, call_name_bits)?;
                trace_call_bind_ic_epoch_stage(type_version, "metaclass_call_lookup");
                if !is_default_type_call(_py, metaclass_call_bits) {
                    trace_call_bind_ic_bypass("type_call", "custom_metaclass_call");
                    return None;
                }
                // Only cacheable when __new__ is the default object.__new__.
                let new_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
                let new_bits = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
                trace_call_bind_ic_epoch_stage(type_version, "new_lookup");
                if !resolved_new_is_default_object_new(new_bits) {
                    trace_call_bind_ic_bypass("type_call", "custom_new");
                    return None;
                }
                // Resolve __init__ and ensure it is a simple direct-callable function.
                let init_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
                let init_bits = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits)?;
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
                let call_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
                let func_bits = class_attr_lookup_raw_mro(_py, class_ptr, call_name_bits)?;
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

pub(super) unsafe fn try_call_bind_ic_fast(
    _py: &PyToken<'_>,
    entry: CallBindIcEntry,
    call_bits: u64,
    args_ptr: *mut CallArgs,
) -> Option<u64> {
    unsafe {
        if args_ptr.is_null() {
            return None;
        }
        let args = &*args_ptr;
        if !args.kw_names.is_empty() {
            return None;
        }

        let call_obj = obj_from_bits(call_bits);
        let call_ptr = call_obj.as_ptr()?;

        if entry.kind == CALL_BIND_IC_KIND_LIST_APPEND {
            if object_type_id(call_ptr) != TYPE_ID_BOUND_METHOD || args.pos.len() != 1 {
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
            let arg0 = args.pos[0];
            return Some(molt_list_append(self_bits, arg0));
        }

        if entry.kind == CALL_BIND_IC_KIND_DIRECT_FUNC {
            if object_type_id(call_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            if function_fn_ptr(call_ptr) != entry.fn_ptr {
                return None;
            }
            if args.pos.len() != entry.arity as usize {
                return None;
            }
            let result = call_function_obj_bound_vec(_py, call_bits, args.pos.as_slice());
            return Some(protect_callargs_aliased_return(_py, result, args_ptr));
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
            if args.pos.len() != entry.arity as usize {
                return None;
            }
            let self_bits = bound_method_self_bits(call_ptr);
            let mut argv = [0u64; 5];
            argv[0] = self_bits;
            for (idx, arg) in args.pos.iter().copied().enumerate() {
                argv[idx + 1] = arg;
            }
            let result = call_function_obj_bound_vec(_py, func_bits, &argv[..args.pos.len() + 1]);
            return Some(protect_callargs_aliased_return_with_extra(
                _py,
                result,
                args_ptr,
                &[self_bits],
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
            if args.pos.len() != entry.arity as usize {
                return None;
            }
            let mut argv = [0u64; 5];
            argv[0] = call_bits;
            for (idx, arg) in args.pos.iter().copied().enumerate() {
                argv[idx + 1] = arg;
            }
            let result =
                call_function_obj_bound_vec(_py, entry.target_bits, &argv[..args.pos.len() + 1]);
            return Some(protect_callargs_aliased_return_with_extra(
                _py,
                result,
                args_ptr,
                &[call_bits],
            ));
        }

        // IC fast path for user-class instantiation: TYPE_ID_TYPE with default
        // __new__ and a known simple __init__.  Skips the entire
        // call_type_with_builder resolution (intern __new__/__init__, MRO
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
            if args.pos.len() != entry.arity as usize {
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
            let closure_bits = function_closure_bits(init_ptr);
            let code_bits = ensure_function_code_bits(_py, init_ptr);
            if !recursion_guard_enter() {
                dec_ref_bits(_py, inst_bits);
                return Some(raise_exception::<_>(
                    _py,
                    "RecursionError",
                    "maximum recursion depth exceeded",
                ));
            }
            // Direct IC calls borrow `self` exactly like the generic function
            // call path. The freshly allocated instance's original ref remains
            // the constructor result; no extra callee-owned self lane exists.
            frame_stack_push_function(_py, code_bits, init_ptr);
            let _init_result = if closure_bits != 0 {
                match args.pos.len() {
                    0 => {
                        let f: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(call_target);
                        f(closure_bits, inst_bits) as u64
                    }
                    1 => {
                        let f: extern "C" fn(u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(closure_bits, inst_bits, args.pos[0]) as u64
                    }
                    2 => {
                        let f: extern "C" fn(u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(closure_bits, inst_bits, args.pos[0], args.pos[1]) as u64
                    }
                    3 => {
                        let f: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(
                            closure_bits,
                            inst_bits,
                            args.pos[0],
                            args.pos[1],
                            args.pos[2],
                        ) as u64
                    }
                    _ => {
                        let mut argv = [0u64; 5];
                        argv[0] = inst_bits;
                        for (idx, arg) in args.pos.iter().copied().enumerate() {
                            argv[idx + 1] = arg;
                        }
                        call_function_obj_bound_vec(
                            _py,
                            entry.target_bits,
                            &argv[..args.pos.len() + 1],
                        )
                    }
                }
            } else {
                match args.pos.len() {
                    0 => {
                        let f: extern "C" fn(u64) -> i64 = std::mem::transmute(call_target);
                        f(inst_bits) as u64
                    }
                    1 => {
                        let f: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(call_target);
                        f(inst_bits, args.pos[0]) as u64
                    }
                    2 => {
                        let f: extern "C" fn(u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(inst_bits, args.pos[0], args.pos[1]) as u64
                    }
                    3 => {
                        let f: extern "C" fn(u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(call_target);
                        f(inst_bits, args.pos[0], args.pos[1], args.pos[2]) as u64
                    }
                    _ => {
                        let mut argv = [0u64; 5];
                        argv[0] = inst_bits;
                        for (idx, arg) in args.pos.iter().copied().enumerate() {
                            argv[idx + 1] = arg;
                        }
                        call_function_obj_bound_vec(
                            _py,
                            entry.target_bits,
                            &argv[..args.pos.len() + 1],
                        )
                    }
                }
            };
            frame_stack_pop(_py);
            recursion_guard_exit();
            // Same post-`__init__` resolution as every other constructor path:
            // on a pending exception, drop the instance and yield `none` so the
            // construct-site `check_exception` / IC propagation guards fire. The
            // full-binding lane (call_type_with_builder ForwardArgs) routes
            // through the identical helper — neither can silently swallow a
            // constructor raise (task #60).
            return Some(crate::call::class_init::resolve_construct_after_init(
                _py, inst_bits,
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

/// Fused instance-method dispatch (`obj.method(args...)`) — the CPython
/// `LOAD_METHOD` + `CALL_METHOD` optimisation.
///
/// The legacy lowering split this into `get_attr_generic_ptr` (which allocates
/// a BOUND-METHOD object) followed by `call_bind` (which allocates a CallArgs
/// builder).  Both heap allocations recur every call.  This entry point fuses
/// them with a per-site inline cache: when `object_method_ic_resolve` proves the
/// attribute is a plain class method (no data descriptor, no `__getattribute__`
/// override) and the instance does not shadow it, the resolved function is
/// invoked directly with `self` prepended — ZERO allocations, one GIL crossing.
/// Otherwise it reproduces the exact legacy behaviour (real getattr -> bound
/// method -> callargs -> `molt_call_bind_ic`), preserving semantics bit-for-bit.
///
/// `args` are BORROWED positional argument bits (NOT including `self`); the fast
/// path reads them without consuming, and the slow path inc-refs them into the
/// CallArgs builder exactly as `molt_callargs_push_pos` always has.
///
/// # Safety
/// `recv_bits` must be a live object; `name_ptr`/`name_len` a valid UTF-8 method
/// name; `args` valid for `args.len()` reads.  GIL acquired by the caller.
unsafe fn call_method_ic_dispatch(
    _py: &PyToken<'_>,
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe {
        let Some(name) = crate::provenance::abi::slice(name_ptr, name_len_bits) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "method name range is invalid for the active target",
            );
        };
        let name_len = name.len();
        // Largest `fixed_arity` (including `self`) the allocation-free direct
        // path serves from a stack arg buffer; wider methods fall to the binder.
        const DIRECT_ARGV_MAX: usize = 16;

        let recv_obj = obj_from_bits(recv_bits);
        if let Some(recv_ptr) = recv_obj.as_ptr() {
            // ALLOCATION-FREE FAST PATH: invoke the resolved class function with
            // `[self, args..., <trailing positional defaults>]`, padded to the
            // method's fixed arity so the compiled trampoline runs its own fast
            // (no-rebind) prologue. `self` and `args` stay borrowed; trailing
            // defaults are read LIVE from `func.__defaults__` (so a runtime
            // `Class.method.__defaults__ = (...)` reassignment is honoured — the
            // cached `n_pos_defaults` is only a fast-path GATE, never the source
            // of the values) and are likewise borrowed: `call_function_obj_vec`
            // reads `argv` without consuming. Returns `None` when the call needs
            // more defaults than the live tuple supplies (stale gate) so the
            // binder can raise the correct error.
            //
            // `fixed_arity` includes `self`; `n_pos_defaults` is the cached
            // `len(__defaults__)`. Caller guarantees (via `direct_ok`)
            // `fixed_arity - n_pos_defaults <= args.len() + 1 <= fixed_arity`
            // and `fixed_arity <= DIRECT_ARGV_MAX`.
            let call_direct = |_py: &PyToken<'_>, func_bits: u64, fixed_arity: u8| -> Option<u64> {
                let fixed_arity = fixed_arity as usize;
                let supplied = args.len() + 1; // including self
                let mut argv = [0u64; DIRECT_ARGV_MAX];
                argv[0] = recv_bits;
                for (idx, a) in args.iter().copied().enumerate() {
                    argv[idx + 1] = a;
                }
                if supplied < fixed_arity {
                    // Pad the trailing positionals from the LIVE __defaults__.
                    let func_ptr = obj_from_bits(func_bits).as_ptr()?;
                    let defaults_bits = function_binding_meta(_py, func_ptr, b"__defaults__");
                    let def_ptr = obj_from_bits(defaults_bits).as_ptr()?;
                    if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                        return None;
                    }
                    let def_elems = crate::object::seq_access::pin_tuple(_py, def_ptr)?;
                    let missing = fixed_arity - supplied;
                    if missing > def_elems.len() {
                        // Live defaults cannot cover the gap (e.g. a shrunk
                        // __defaults__ after the gate was cached) — defer.
                        return None;
                    }
                    // The defaults align to the END of the parameter list, so
                    // the missing trailing params take the LAST `missing`
                    // default values.
                    let start = def_elems.len() - missing;
                    argv[supplied..supplied + missing]
                        .copy_from_slice(&def_elems[start..start + missing]);
                }
                let result = call_function_obj_bound_vec(_py, func_bits, &argv[..fixed_arity]);
                Some(protect_borrowed_args_aliased_return(
                    _py,
                    result,
                    &argv[..fixed_arity],
                ))
            };

            // The direct path is sound iff the method needs no full binder (no
            // kw-only params/defaults, `*args`, `**kwargs`, or builtin bind-kind)
            // AND the supplied positional count (including `self`) lands in the
            // range the method's positional defaults can pad to its fixed arity:
            // `[fixed_arity - n_pos_defaults, fixed_arity]`. A too-short call
            // (even with all defaults) or a too-long call must take the binder so
            // it raises the correct `call arity mismatch` (e.g. `runner.run(coro)`
            // against `def run(self, coro, *, context=None)` is kw-only → binder).
            let direct_ok = |fixed_arity: u8, n_pos_defaults: u8, needs_binder: bool| -> bool {
                if needs_binder {
                    return false;
                }
                let fixed_arity = fixed_arity as usize;
                if fixed_arity > DIRECT_ARGV_MAX {
                    return false;
                }
                let supplied = args.len() + 1; // including self
                let min_supplied = fixed_arity.saturating_sub(n_pos_defaults as usize);
                supplied >= min_supplied && supplied <= fixed_arity
            };

            // CACHED-BIND PATH: when the resolved method is already proven
            // class-side (so the IC entry / `info` is authoritative) but the
            // allocation-free direct path does NOT apply (kw-only/`*args`/
            // `**kwargs`, or a positional-count outside the paddable range), bind
            // the cached `func_bits` to `self` and route through the full binder
            // — WITHOUT re-walking the MRO, re-interning the name, or
            // re-resolving the descriptor. `alloc_bound_method_obj` produces the
            // exact object `descriptor_bind` would have for a plain function
            // (`molt_bound_method_new(func, self)`), inc-ref'ing both `func_bits`
            // and `recv_bits` into the new bound method; that reference is
            // balanced when the bound method is dropped below. The cached
            // `func_bits`/`attr_bits` are reused read-only here (no extra
            // inc/dec on either), so IC ownership is untouched.
            let cached_bind = |_py: &PyToken<'_>, func_bits: u64| -> u64 {
                let method_ptr =
                    crate::object::builders::alloc_bound_method_obj(_py, func_bits, recv_bits);
                if method_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let method_bits = MoltObject::from_ptr(method_ptr).bits();
                slow_bind_via_method(_py, site_bits, method_bits, args)
            };

            // Take the direct path when the gate allows it, but if the live
            // default-pad cannot complete (stale gate vs a shrunk __defaults__),
            // transparently fall back to the binder so the correct error/result
            // is produced.
            let dispatch = |_py: &PyToken<'_>, func_bits: u64, plan: MethodIcCallPlan| -> u64 {
                if direct_ok(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder)
                    && let Some(res) = call_direct(_py, func_bits, plan.fixed_arity)
                {
                    return res;
                }
                cached_bind(_py, func_bits)
            };

            // Per-site IC: on a hit, validate the receiver class + layout
            // version, run the (cheap) shadow check only when the class permits
            // shadowing, and dispatch — no name interning, no MRO walk. A
            // class/version-valid, non-shadowed entry is a HIT regardless of the
            // call shape: positional-default methods take the allocation-free
            // direct path (padding defaults inline); kw-only/`*args`/`**kwargs`
            // methods take `cached_bind`, which still reuses the cached
            // resolution (no re-resolve). Only a class/version mismatch, or an
            // instance that shadows the method, falls through to the genuine
            // resolve+insert below — preserving invalidation and stale-shape
            // semantics exactly.
            if let Some(site_id) = ic_site_from_bits(site_bits)
                && let Some(entry) = method_ic_lookup(site_id)
            {
                let class_bits = object_class_bits(recv_ptr);
                if type_epoch_matches(entry.type_version)
                    && class_bits == entry.class_bits
                    && cached_attr_matches_bytes(entry.attr_bits, name)
                    && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && class_layout_version_bits(class_ptr) == entry.class_version
                {
                    let shadowed = entry.can_shadow
                        && crate::builtins::attr::object_instance_shadows(
                            _py,
                            recv_ptr,
                            class_ptr,
                            entry.attr_bits,
                        );
                    if !shadowed {
                        return dispatch(
                            _py,
                            entry.func_bits,
                            MethodIcCallPlan {
                                fixed_arity: entry.fixed_arity,
                                n_pos_defaults: entry.n_pos_defaults,
                                needs_binder: entry.needs_binder,
                            },
                        );
                    }
                    // Shadowed instance: the cached class method does not apply
                    // for THIS instance; fall through to real resolution (which
                    // finds the instance attribute).
                }
            }

            // IC miss: resolve the method class-side, install the IC, dispatch.
            if let Some(attr_bits) = attr_name_bits_from_bytes(_py, name) {
                let type_version = crate::object::global_type_version();
                let info =
                    crate::builtins::attr::object_method_ic_resolve(_py, recv_ptr, attr_bits);
                if let Some(info) = info {
                    let shadowed = info.can_shadow
                        && crate::builtins::attr::object_instance_shadows(
                            _py,
                            recv_ptr,
                            obj_from_bits(info.class_bits)
                                .as_ptr()
                                .unwrap_or(std::ptr::null_mut()),
                            attr_bits,
                        );
                    if !shadowed && type_resolution_epoch_is_stable(type_version) {
                        // Compute the call plan once and record it in the IC so
                        // future hits decide the path with a couple of integer
                        // compares. A method needing the full binder (kw-only/
                        // varargs) is cached with `needs_binder` set so subsequent
                        // hits take the `cached_bind` path (binder, but no
                        // re-resolve); positional-default methods stay on the
                        // direct path.
                        let plan =
                            method_ic_call_plan(_py, info.func_bits).unwrap_or(MethodIcCallPlan {
                                fixed_arity: 0,
                                n_pos_defaults: 0,
                                needs_binder: true,
                            });
                        if let Some(site_id) = ic_site_from_bits(site_bits) {
                            // Transfer the owned `attr_bits` ref into the IC; it
                            // is released by `method_ic_insert`/`clear` on reuse.
                            method_ic_insert(
                                _py,
                                site_id,
                                MethodIcEntry {
                                    class_bits: info.class_bits,
                                    class_version: info.class_version,
                                    type_version,
                                    func_bits: info.func_bits,
                                    attr_bits,
                                    can_shadow: info.can_shadow,
                                    fixed_arity: plan.fixed_arity,
                                    n_pos_defaults: plan.n_pos_defaults,
                                    needs_binder: plan.needs_binder,
                                    valid: true,
                                },
                            );
                            // `attr_bits` ownership was transferred to the IC; do
                            // NOT dec-ref it here.
                            return dispatch(_py, info.func_bits, plan);
                        }
                        // No stable site id — cannot cache; dispatch then release
                        // the name ref we own (it was never cached).
                        let res = dispatch(_py, info.func_bits, plan);
                        dec_ref_bits(_py, attr_bits);
                        return res;
                    } else {
                        dec_ref_bits(_py, attr_bits);
                    }
                } else {
                    dec_ref_bits(_py, attr_bits);
                }
            }
        }

        // SLOW PATH: byte-identical to the legacy `get_attr_generic_ptr` +
        // `call_bind` lowering.  `molt_get_attr_generic` materialises the bound
        // method (or raises); the shared `slow_bind_via_method` consumes it via
        // the CallArgs binder.  Reached only when the receiver is not an OBJECT/
        // DATACLASS the fused fast path covers, when the attribute is not a
        // plain method (custom `__getattribute__`, data descriptor, instance
        // shadow, non-function attr), or when name interning fails.
        let recv_ptr = recv_obj.as_ptr().unwrap_or(std::ptr::null_mut());
        let method_bits = crate::molt_get_attr_generic(recv_ptr, name_ptr, name_len as u64) as u64;
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        slow_bind_via_method(_py, site_bits, method_bits, args)
    }
}

/// Shared tail for the fused method-call slow/bind paths: build a CallArgs
/// builder from the BORROWED positional `args`, dispatch the OWNED bound-method
/// (or other callable) `method_bits` through the full binder IC, then release
/// the caller's reference to `method_bits`.
///
/// `method_bits` is consumed (dec-ref'd) here; `args` are borrowed and
/// inc-ref'd into the builder by `molt_callargs_push_pos` exactly as the legacy
/// lowering did. Returns `None` (and drops `method_bits`) on a builder
/// allocation failure or a pending exception.
///
/// # Safety
/// `method_bits` must be a live owned reference (or a falsey sentinel after a
/// pending exception, already handled by the caller); the GIL must be held.
unsafe fn slow_bind_via_method(
    _py: &PyToken<'_>,
    site_bits: u64,
    method_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe {
        let callargs_bits = molt_callargs_new(MoltObject::from_int(args.len() as i64).bits(), 0);
        if callargs_bits == 0 || exception_pending(_py) {
            dec_ref_bits(_py, method_bits);
            return MoltObject::none().bits();
        }
        for a in args.iter().copied() {
            molt_callargs_push_pos(callargs_bits, a);
        }
        let res = molt_call_bind_ic(site_bits, method_bits, callargs_bits);
        dec_ref_bits(_py, method_bits);
        res
    }
}

/// C-ABI entry for fused method dispatch with 0 positional args (`obj.m()`).
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_method_ic0(
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_method_ic_dispatch(_py, site_bits, recv_bits, name_ptr, name_len_bits, &[]) }
    })
}

/// C-ABI entry for fused method dispatch with 1 positional arg.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_method_ic1(
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_method_ic_dispatch(_py, site_bits, recv_bits, name_ptr, name_len_bits, &[a0])
        }
    })
}

/// C-ABI entry for fused method dispatch with 2 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_method_ic2(
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
    a1: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_method_ic_dispatch(
                _py,
                site_bits,
                recv_bits,
                name_ptr,
                name_len_bits,
                &[a0, a1],
            )
        }
    })
}

/// C-ABI entry for fused method dispatch with 3 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_method_ic3(
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_method_ic_dispatch(
                _py,
                site_bits,
                recv_bits,
                name_ptr,
                name_len_bits,
                &[a0, a1, a2],
            )
        }
    })
}

/// C-ABI entry for fused method dispatch with 4 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_method_ic4(
    site_bits: u64,
    recv_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_method_ic_dispatch(
                _py,
                site_bits,
                recv_bits,
                name_ptr,
                name_len_bits,
                &[a0, a1, a2, a3],
            )
        }
    })
}

/// Fused `super().method(args...)` dispatch.
///
/// The legacy lowering allocated a `super` object (`super_new`), a bound method
/// (`get_attr_generic_obj`), and a CallArgs builder (`callargs_new`) on EVERY
/// call.  This entry point resolves the MRO-next plain method directly via
/// `super_resolve_method_unbound` and invokes it with `self` prepended — zero
/// allocations on the fast path.  Any shape the fast path does not cover
/// (class-bound super, builtin-class method, non-function descriptor) falls
/// back to the exact legacy `super_new` + `get_attr` + `call_bind` sequence.
///
/// `start_class_bits` is the defining class (`__class__`); `self_bits` the
/// instance; `args` the BORROWED positional args (excluding `self`).
///
/// # Safety
/// `self_bits` must be live; `name_ptr`/`name_len` valid UTF-8; `args` readable.
/// GIL acquired by the caller.
unsafe fn call_super_method_ic_dispatch(
    _py: &PyToken<'_>,
    site_bits: u64,
    start_class_bits: u64,
    self_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe {
        let Some(name) = crate::provenance::abi::slice(name_ptr, name_len_bits) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "method name range is invalid for the active target",
            );
        };
        let name_len = name.len();
        let call_direct = |_py: &PyToken<'_>, func_bits: u64| -> u64 {
            let mut argv = [0u64; 13];
            argv[0] = self_bits;
            for (idx, a) in args.iter().copied().enumerate() {
                argv[idx + 1] = a;
            }
            let result = call_function_obj_bound_vec(_py, func_bits, &argv[..args.len() + 1]);
            protect_borrowed_args_aliased_return(_py, result, &argv[..args.len() + 1])
        };

        // Per-site super IC: validate `type(self)` + layout version and
        // dispatch directly — no super object, no name interning, no MRO walk.
        if let Some(self_ptr) = obj_from_bits(self_bits).as_ptr()
            && let Some(site_id) = ic_site_from_bits(site_bits)
            && let Some(entry) = super_ic_lookup(site_id)
        {
            let self_class_bits = type_of_bits(_py, self_bits);
            if type_epoch_matches(entry.type_version)
                && self_class_bits == entry.self_class_bits
                && cached_attr_matches_bytes(entry.attr_bits, name)
                && let Some(self_class_ptr) = obj_from_bits(self_class_bits).as_ptr()
                && class_layout_version_bits(self_class_ptr) == entry.self_class_version
            {
                let _ = self_ptr;
                return call_direct(_py, entry.func_bits);
            }
        }

        if let Some(attr_bits) = attr_name_bits_from_bytes(_py, name) {
            let type_version = crate::object::global_type_version();
            let resolved = crate::builtins::attr::super_resolve_method_unbound(
                _py,
                start_class_bits,
                self_bits,
                attr_bits,
            );
            if let Some(info) = resolved
                && type_resolution_epoch_is_stable(type_version)
            {
                if let Some(site_id) = ic_site_from_bits(site_bits) {
                    // Transfer the owned `attr_bits` ref into the IC.
                    super_ic_insert(
                        _py,
                        site_id,
                        SuperIcEntry {
                            self_class_bits: info.self_class_bits,
                            self_class_version: info.self_class_version,
                            type_version,
                            func_bits: info.func_bits,
                            attr_bits,
                            valid: true,
                        },
                    );
                    return call_direct(_py, info.func_bits);
                }
                let res = call_direct(_py, info.func_bits);
                dec_ref_bits(_py, attr_bits);
                return res;
            }
            dec_ref_bits(_py, attr_bits);
        }

        // SLOW PATH: byte-identical to the legacy lowering.
        let super_bits = molt_super_new(start_class_bits, self_bits);
        if super_bits == 0 || exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let super_ptr = obj_from_bits(super_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        let method_bits = crate::molt_get_attr_generic(super_ptr, name_ptr, name_len as u64) as u64;
        if exception_pending(_py) {
            dec_ref_bits(_py, super_bits);
            return MoltObject::none().bits();
        }
        let callargs_bits = molt_callargs_new(MoltObject::from_int(args.len() as i64).bits(), 0);
        if callargs_bits == 0 || exception_pending(_py) {
            dec_ref_bits(_py, method_bits);
            dec_ref_bits(_py, super_bits);
            return MoltObject::none().bits();
        }
        for a in args.iter().copied() {
            molt_callargs_push_pos(callargs_bits, a);
        }
        let res = molt_call_bind_ic(site_bits, method_bits, callargs_bits);
        dec_ref_bits(_py, method_bits);
        dec_ref_bits(_py, super_bits);
        res
    }
}

/// C-ABI entry for fused super dispatch with 0 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_super_method_ic0(
    site_bits: u64,
    start_class_bits: u64,
    self_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_super_method_ic_dispatch(
                _py,
                site_bits,
                start_class_bits,
                self_bits,
                name_ptr,
                name_len_bits,
                &[],
            )
        }
    })
}

/// C-ABI entry for fused super dispatch with 1 positional arg.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_super_method_ic1(
    site_bits: u64,
    start_class_bits: u64,
    self_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_super_method_ic_dispatch(
                _py,
                site_bits,
                start_class_bits,
                self_bits,
                name_ptr,
                name_len_bits,
                &[a0],
            )
        }
    })
}

/// C-ABI entry for fused super dispatch with 2 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_super_method_ic2(
    site_bits: u64,
    start_class_bits: u64,
    self_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
    a1: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_super_method_ic_dispatch(
                _py,
                site_bits,
                start_class_bits,
                self_bits,
                name_ptr,
                name_len_bits,
                &[a0, a1],
            )
        }
    })
}

/// C-ABI entry for fused super dispatch with 3 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_super_method_ic3(
    site_bits: u64,
    start_class_bits: u64,
    self_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_super_method_ic_dispatch(
                _py,
                site_bits,
                start_class_bits,
                self_bits,
                name_ptr,
                name_len_bits,
                &[a0, a1, a2],
            )
        }
    })
}

/// C-ABI entry for fused super dispatch with 4 positional args.
///
/// # Safety
/// `name_ptr`/`name_len_bits` describe a valid UTF-8 method name.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_super_method_ic4(
    site_bits: u64,
    start_class_bits: u64,
    self_bits: u64,
    name_ptr: *const u8,
    name_len_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            call_super_method_ic_dispatch(
                _py,
                site_bits,
                start_class_bits,
                self_bits,
                name_ptr,
                name_len_bits,
                &[a0, a1, a2, a3],
            )
        }
    })
}

unsafe fn call_bind_ic_dispatch(
    _py: &PyToken<'_>,
    site_bits: u64,
    call_bits: u64,
    builder_bits: u64,
) -> u64 {
    unsafe {
        let Some(site_id) = ic_site_from_bits(site_bits) else {
            return molt_call_bind(call_bits, builder_bits);
        };
        let builder_ptr = ptr_from_bits(builder_bits);
        let mut builder_guard = PtrDropGuard::new(builder_ptr);

        if disable_call_bind_ic_enabled() {
            if trace_call_bind_ic_enabled() {
                eprintln!(
                    "[molt call_bind_ic] bypass site={} reason=disabled_via_env",
                    site_id
                );
            }
            builder_guard.release();
            return molt_call_bind(call_bits, builder_bits);
        }

        if !builder_ptr.is_null() {
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            // Thread-local IC lookup — zero synchronization overhead on hits.
            if let Some(entry) = ic_tls_lookup(site_id)
                && let Some(res) = try_call_bind_ic_fast(_py, entry, call_bits, args_ptr)
            {
                if trace_call_bind_ic_enabled() {
                    let kind = match entry.kind {
                        CALL_BIND_IC_KIND_DIRECT_FUNC => "direct_func",
                        CALL_BIND_IC_KIND_LIST_APPEND => "list_append",
                        CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC => "bound_direct_func",
                        CALL_BIND_IC_KIND_HEAP_CALL_SIMPLE_BOUND_FUNC => {
                            "heap_call_simple_bound_func"
                        }
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
        }

        profile_hit_unchecked(&CALL_BIND_IC_MISS_COUNT);
        if trace_call_bind_ic_enabled() {
            let call_type = type_name(_py, obj_from_bits(call_bits));
            let (pos_len, kw_len) = if !builder_ptr.is_null() {
                match require_callargs_ptr(_py, builder_ptr) {
                    Ok(args_ptr) => ((*args_ptr).pos.len(), (*args_ptr).kw_names.len()),
                    Err(_) => (0, 0),
                }
            } else {
                (0, 0)
            };
            eprintln!(
                "[molt call_bind_ic] miss site={} callee_type={} pos_len={} kw_len={}",
                site_id, call_type, pos_len, kw_len
            );
        }
        builder_guard.release();
        let res = molt_call_bind(call_bits, builder_bits);
        // Only populate the inline cache when the call completed WITHOUT a
        // pending exception. Building an IC entry runs class-attribute lookups
        // (`__new__`/`__init__` MRO probes in `call_bind_ic_entry_for_call`)
        // that reset the exception-pending baseline — which would silently
        // swallow an exception the call just raised (task #60: a full-binding
        // constructor `__init__` raise reached here, was reported via the
        // `none` result + pending flag, then the IC-entry probe cleared the
        // flag before the caller's `check_exception` could observe it). A call
        // that raised is also not a useful thing to cache. Skip the cache and
        // hand back the result with the pending exception intact.
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

#[unsafe(no_mangle)]
/// # Safety
/// Caller must provide a call-site id in `site_bits` and a valid callargs builder in
/// `builder_bits`.
pub extern "C" fn molt_call_indirect_ic(site_bits: u64, call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { call_bind_ic_dispatch(_py, site_bits, call_bits, builder_bits) }
    })
}
