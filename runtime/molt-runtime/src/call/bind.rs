use crate::builtins::exceptions::{frame_stack_pop, frame_stack_push};
use crate::call::type_policy::{
    InitArgPolicy, callable_matches_runtime_symbol, resolved_constructor_init_policy,
    resolved_new_is_default_object_new,
};
use crate::object::layout::ensure_function_code_bits;
use crate::state::recursion::{recursion_guard_enter, recursion_guard_exit};
use crate::state::tls::FRAME_STACK;
use crate::{
    ALLOC_BYTES_CALLARGS, BIND_KIND_CAPI_METHOD, BIND_KIND_OPEN, CALL_BIND_IC_HIT_COUNT,
    CALL_BIND_IC_MISS_COUNT, GEN_CONTROL_SIZE, INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT,
    MoltHeader, MoltObject, PtrDropGuard, PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_CALLARGS,
    TYPE_ID_CODE, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_FROZENSET,
    TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS, TYPE_ID_OBJECT, TYPE_ID_SET, TYPE_ID_STRING,
    TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_class_obj, alloc_dict_with_pairs,
    alloc_exception_from_class_bits, alloc_instance_for_class,
    alloc_instance_for_default_object_new, alloc_object, alloc_string, alloc_tuple,
    apply_class_slots_layout, attr_lookup_ptr, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes,
    audit::{AuditArgs, audit_capability_decision},
    bits_from_ptr, bound_method_func_bits, bound_method_self_bits, builtin_classes, call_callable0,
    call_callable1, call_class_init_with_args, call_function_obj_vec, class_attr_lookup,
    class_attr_lookup_raw_mro, class_dict_bits, class_layout_version_bits, class_name_bits,
    class_name_for_error, code_argcount, code_filename_bits, code_name_bits, dec_ref_bits,
    dict_del_in_place, dict_fromkeys_method, dict_get_in_place, dict_get_method, dict_order,
    dict_pop_method, dict_setdefault_method, dict_update_apply, dict_update_method,
    dict_update_set_in_place, dict_update_set_via_store, exception_class_bits, exception_pending,
    exception_type_bits_from_name, function_arity, function_attr_bits, function_closure_bits,
    function_fn_ptr, function_name_bits, function_trampoline_ptr, generic_alias_origin_bits,
    has_capability, inc_ref_bits, init_atomic_bits, intern_static_name, is_builtin_class_bits,
    is_trusted, is_truthy, isinstance_bits, issubclass_bits, lookup_call_attr, maybe_ptr_from_bits,
    missing_bits, molt_bytearray_count_slice, molt_bytearray_decode, molt_bytearray_endswith_slice,
    molt_bytearray_find_slice, molt_bytearray_hex, molt_bytearray_index_slice, molt_bytearray_pop,
    molt_bytearray_rfind_slice, molt_bytearray_rindex_slice, molt_bytearray_rsplit_max,
    molt_bytearray_split_max, molt_bytearray_splitlines, molt_bytearray_startswith_slice,
    molt_bytes_count_slice, molt_bytes_decode, molt_bytes_endswith_slice, molt_bytes_find_slice,
    molt_bytes_hex, molt_bytes_index_slice, molt_bytes_maketrans, molt_bytes_rfind_slice,
    molt_bytes_rindex_slice, molt_bytes_rsplit_max, molt_bytes_split_max, molt_bytes_splitlines,
    molt_bytes_startswith_slice, molt_class_set_base, molt_dict_from_obj, molt_dict_new,
    molt_file_reconfigure, molt_frozenset_copy_method, molt_frozenset_difference_multi,
    molt_frozenset_intersection_multi, molt_frozenset_isdisjoint, molt_frozenset_issubset,
    molt_frozenset_issuperset, molt_frozenset_symmetric_difference, molt_frozenset_union_multi,
    molt_generator_new, molt_int_from_bytes, molt_int_new, molt_int_to_bytes, molt_iter,
    molt_iter_next, molt_list_append, molt_list_index_range, molt_list_pop, molt_list_sort,
    molt_memoryview_cast, molt_memoryview_hex, molt_object_init, molt_object_init_subclass,
    molt_object_new_bound, molt_open_builtin, molt_set_clear, molt_set_copy_method,
    molt_set_difference_multi, molt_set_difference_update_multi, molt_set_intersection_multi,
    molt_set_intersection_update_multi, molt_set_isdisjoint, molt_set_issubset,
    molt_set_issuperset, molt_set_symmetric_difference, molt_set_symmetric_difference_update,
    molt_set_union_multi, molt_set_update_multi, molt_string_count_slice, molt_string_encode,
    molt_string_endswith_slice, molt_string_find_slice, molt_string_format_method,
    molt_string_index_slice, molt_string_rfind_slice, molt_string_rindex_slice,
    molt_string_rsplit_max, molt_string_split_max, molt_string_splitlines,
    molt_string_startswith_slice, molt_super_new, molt_tuple_index_range, molt_type_call,
    molt_type_init, molt_type_new, obj_from_bits, object_class_bits, object_set_class_bits,
    object_type_id, profile_hit_unchecked, ptr_from_bits, raise_exception, raise_not_callable,
    raise_not_iterable, runtime_state, seq_vec_ref, string_obj_to_owned, type_name, type_of_bits,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
pub(crate) struct CallArgs {
    pos: Vec<u64>,
    kw_names: Vec<u64>,
    kw_values: Vec<u64>,
    kw_seen: HashSet<String>,
}

pub(crate) unsafe fn dispatch_init_subclass_hooks(
    _py: &PyToken<'_>,
    bases: &[u64],
    class_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> bool {
    unsafe {
        let init_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.init_subclass_name,
            b"__init_subclass__",
        );
        for base_bits in bases.iter().copied() {
            let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
                continue;
            };
            let Some(init_bits) = attr_lookup_ptr_allow_missing(_py, base_ptr, init_name_bits)
            else {
                continue;
            };
            let builder_bits =
                molt_callargs_new((1 + kw_names.len()) as u64, kw_names.len() as u64);
            if builder_bits == 0 {
                dec_ref_bits(_py, init_bits);
                return false;
            }
            let _ = molt_callargs_push_pos(builder_bits, class_bits);
            for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
            }
            let _ = molt_call_bind(init_bits, builder_bits);
            dec_ref_bits(_py, init_bits);
            if exception_pending(_py) {
                return false;
            }
        }
        true
    }
}

fn trace_callargs_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_CALLARGS").as_deref() == Ok("1"))
}

fn trace_call_bind_ic_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_CALL_BIND_IC").as_deref() == Ok("1"))
}

fn trace_function_bind_meta_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_FUNCTION_BIND_META").as_deref() == Ok("1"))
}

fn trace_call_type_builder_enabled_raw(raw: Option<&str>) -> bool {
    raw == Some("1")
}

fn trace_call_type_builder_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        trace_call_type_builder_enabled_raw(
            std::env::var("MOLT_TRACE_CALL_TYPE_BUILDER")
                .ok()
                .as_deref(),
        )
    })
}

fn disable_call_bind_ic_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DISABLE_CALL_BIND_IC").as_deref() == Ok("1"))
}

/// Cached trace mode for `molt_call_bind`.  The env var is read once;
/// subsequent calls use the cached result — eliminates a
/// `std::env::var` syscall on every function call.
#[derive(Copy, Clone)]
enum TraceCallBindMode {
    Off,
    Basic,
    Verbose,
}

fn trace_call_bind_mode() -> TraceCallBindMode {
    static MODE: OnceLock<TraceCallBindMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        match std::env::var("MOLT_TRACE_CALL_BIND")
            .ok()
            .as_deref()
        {
            Some("all" | "verbose") => TraceCallBindMode::Verbose,
            Some("1") => TraceCallBindMode::Basic,
            _ => TraceCallBindMode::Off,
        }
    })
}

#[derive(Copy, Clone)]
struct CallArgsPtr(*mut CallArgs);

// CallArgs allocations are owned by the runtime object they are attached to
// and protected by the GIL-like runtime lock. The registry only preserves
// pointer provenance for lookups from object payload addresses.
unsafe impl Send for CallArgsPtr {}
unsafe impl Sync for CallArgsPtr {}

fn callargs_builder_map() -> &'static Mutex<HashMap<usize, CallArgsPtr>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, CallArgsPtr>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn callargs_storage_registry() -> &'static Mutex<HashSet<usize>> {
    static REGISTRY: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(crate) fn note_callargs_alloc(builder_ptr: *mut u8, args_ptr: *mut CallArgs) {
    if !builder_ptr.is_null() {
        callargs_builder_map()
            .lock()
            .unwrap()
            .insert(builder_ptr as usize, CallArgsPtr(args_ptr));
    }
    if args_ptr.is_null() {
        return;
    }
    callargs_storage_registry()
        .lock()
        .unwrap()
        .insert(args_ptr as usize);
}

pub(crate) fn note_callargs_free(builder_ptr: *mut u8, args_ptr: *mut CallArgs) {
    if trace_callargs_enabled() && !builder_ptr.is_null() {
        eprintln!(
            "[molt callargs] free builder_ptr=0x{:x} args_ptr=0x{:x}",
            builder_ptr as usize, args_ptr as usize,
        );
    }
    if !builder_ptr.is_null() {
        callargs_builder_map()
            .lock()
            .unwrap()
            .remove(&(builder_ptr as usize));
    }
    if args_ptr.is_null() {
        return;
    }
    callargs_storage_registry()
        .lock()
        .unwrap()
        .remove(&(args_ptr as usize));
}

pub(crate) unsafe fn clone_callargs_builder_bits(
    _py: &PyToken<'_>,
    builder_bits: u64,
) -> Result<u64, u64> {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    if unsafe { object_type_id(builder_ptr) } != TYPE_ID_CALLARGS {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    let args_ptr = unsafe { require_callargs_ptr(_py, builder_ptr) }?;
    let args = unsafe { &*args_ptr };
    let clone_bits = molt_callargs_new(
        MoltObject::from_int(args.pos.len() as i64).bits(),
        MoltObject::from_int(args.kw_names.len() as i64).bits(),
    );
    if obj_from_bits(clone_bits).is_none() {
        return Err(clone_bits);
    }
    for &value_bits in &args.pos {
        let pushed = unsafe { molt_callargs_push_pos(clone_bits, value_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, clone_bits);
            return Err(pushed);
        }
    }
    for (&name_bits, &value_bits) in args.kw_names.iter().zip(args.kw_values.iter()) {
        let pushed = unsafe { molt_callargs_push_kw(clone_bits, name_bits, value_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, clone_bits);
            return Err(pushed);
        }
    }
    Ok(clone_bits)
}

#[allow(dead_code)]
pub(crate) unsafe fn callargs_positional_snapshot(
    _py: &PyToken<'_>,
    builder_bits: u64,
) -> Result<Vec<u64>, u64> {
    let builder_ptr = ptr_from_bits(builder_bits);
    if builder_ptr.is_null() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "invalid callargs builder",
        ));
    }
    let args_ptr = unsafe { require_callargs_ptr(_py, builder_ptr) }?;
    let args = unsafe { &*args_ptr };
    if !args.kw_names.is_empty() {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "gpu kernel launch does not support keyword arguments",
        ));
    }
    Ok(args.pos.clone())
}

fn callargs_builder_is_live(builder_ptr: *mut u8) -> bool {
    if builder_ptr.is_null() {
        return false;
    }
    callargs_builder_map()
        .lock()
        .unwrap()
        .contains_key(&(builder_ptr as usize))
}

fn callargs_storage_is_live(args_ptr: *mut CallArgs) -> bool {
    if args_ptr.is_null() {
        return false;
    }
    callargs_storage_registry()
        .lock()
        .unwrap()
        .contains(&(args_ptr as usize))
}

#[derive(Clone, Copy)]
struct CallBindIcEntry {
    fn_ptr: u64,
    target_bits: u64,
    class_bits: u64,
    class_version: u64,
    arity: u8,
    kind: u8,
}

const CALL_BIND_IC_KIND_DIRECT_FUNC: u8 = 1;
const CALL_BIND_IC_KIND_LIST_APPEND: u8 = 2;
const CALL_BIND_IC_KIND_BOUND_DIRECT_FUNC: u8 = 3;
const CALL_BIND_IC_KIND_OBJECT_CALL_SIMPLE_BOUND_FUNC: u8 = 4;
const CALL_BIND_IC_KIND_TYPE_CALL: u8 = 5;

// Thread-local direct-mapped inline cache for call_bind dispatch.
// Each slot stores (site_id, entry). On lookup, we check if the stored site_id
// matches — if so, it's a hit with zero synchronization overhead.
// This replaces a Mutex<HashMap> that required a lock on every call.
const IC_TLS_SIZE: usize = 256; // Must be power of 2

thread_local! {
    static IC_TLS: std::cell::RefCell<[(u64, CallBindIcEntry); IC_TLS_SIZE]> =
        const { std::cell::RefCell::new([(0u64, CallBindIcEntry { fn_ptr: 0, target_bits: 0, class_bits: 0, class_version: 0, arity: 0, kind: 0 }); IC_TLS_SIZE]) };
}

#[inline]
fn ic_tls_lookup(site_id: u64) -> Option<CallBindIcEntry> {
    IC_TLS.with(|cache| {
        let cache = cache.borrow();
        let idx = (site_id as usize) & (IC_TLS_SIZE - 1);
        let (stored_id, entry) = cache[idx];
        if stored_id == site_id && entry.kind != 0 {
            Some(entry)
        } else {
            None
        }
    })
}

#[inline]
fn ic_tls_insert(site_id: u64, entry: CallBindIcEntry) {
    IC_TLS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let idx = (site_id as usize) & (IC_TLS_SIZE - 1);
        cache[idx] = (site_id, entry);
    });
}

// Global mutex cache retained for cross-thread visibility and clear_call_bind_ic_cache().
fn call_bind_ic_cache() -> &'static Mutex<HashMap<u64, CallBindIcEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, CallBindIcEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn clear_call_bind_ic_cache() {
    call_bind_ic_cache().lock().unwrap().clear();
    // Note: thread-local caches will be stale after clear but will miss
    // and re-populate on next access. This is correct behavior.
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

unsafe fn is_default_type_call(_py: &PyToken<'_>, call_bits: u64) -> bool {
    unsafe {
        let call_obj = obj_from_bits(call_bits);
        let Some(call_ptr) = call_obj.as_ptr() else {
            return false;
        };
        match object_type_id(call_ptr) {
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(call_ptr);
                is_default_type_call(_py, func_bits)
            }
            TYPE_ID_FUNCTION => crate::builtins::functions::runtime_callable_represents_symbol(
                function_fn_ptr(call_ptr),
                function_trampoline_ptr(call_ptr),
                fn_addr!(molt_type_call),
            ),
            _ => false,
        }
    }
}

unsafe fn call_type_with_builder(
    _py: &PyToken<'_>,
    call_ptr: *mut u8,
    builder_ptr: *mut u8,
    builder_bits: u64,
    builder_guard: &mut PtrDropGuard,
) -> u64 {
    unsafe {
        let class_bits = MoltObject::from_ptr(call_ptr).bits();
        let builtins = builtin_classes(_py);
        let args_ptr = if builder_ptr.is_null() {
            None
        } else {
            match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => Some(ptr),
                Err(err) => return err,
            }
        };
        if let Some(ptr) = args_ptr {
            let pos_args = (*ptr).pos.as_slice();
            let kw_names = (*ptr).kw_names.as_slice();
            let kw_values = (*ptr).kw_values.as_slice();
            if class_bits == builtins.type_obj && pos_args.len() == 3 {
                return build_class_from_args(
                    _py,
                    class_bits,
                    pos_args[0],
                    pos_args[1],
                    pos_args[2],
                    kw_names,
                    kw_values,
                );
            }
            if class_bits == builtins.type_obj && pos_args.len() == 1 && kw_names.is_empty() {
                let bits = type_of_bits(_py, pos_args[0]);
                inc_ref_bits(_py, bits);
                return bits;
            }
        }
        let abstract_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.abstractmethods_name,
            b"__abstractmethods__",
        );
        if let Some(abstract_bits) = class_attr_lookup_raw_mro(_py, call_ptr, abstract_name_bits)
            && !obj_from_bits(abstract_bits).is_none()
            && is_truthy(_py, obj_from_bits(abstract_bits))
        {
            let class_name = class_name_for_error(class_bits);
            let msg = format!("Can't instantiate abstract class {class_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if is_builtin_class_bits(_py, class_bits) {
            let (pos_args, kw_names, kw_values) = if let Some(ptr) = args_ptr {
                (
                    (*ptr).pos.as_slice(),
                    (*ptr).kw_names.as_slice(),
                    (*ptr).kw_values.as_slice(),
                )
            } else {
                (&[] as &[u64], &[] as &[u64], &[] as &[u64])
            };

            // `super` is a builtin type (CPython parity). We must handle it here so that
            // indirect/bound calls like `SYMBOL = builtins.super; SYMBOL()` use CPython-shaped
            // RuntimeError/TypeError behavior instead of falling through to generic type-call.
            if class_bits == builtins.super_type {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "super() takes no keyword arguments",
                    );
                }
                match pos_args.len() {
                    0 => {
                        // CPython distinguishes between calling from module scope (no args at all)
                        // and calling from a function/method frame without a `__class__` cell.
                        let has_pos_args = FRAME_STACK.with(|stack| {
                            let frame = stack.borrow().last().copied();
                            let Some(frame) = frame else {
                                return false;
                            };
                            let Some(code_ptr) = obj_from_bits(frame.code_bits).as_ptr() else {
                                return false;
                            };
                            if object_type_id(code_ptr) != TYPE_ID_CODE {
                                return false;
                            }
                            code_argcount(code_ptr) > 0
                        });
                        let msg = if has_pos_args {
                            "super(): __class__ cell not found"
                        } else {
                            "super(): no arguments"
                        };
                        return raise_exception::<_>(_py, "RuntimeError", msg);
                    }
                    1 => return molt_super_new(pos_args[0], MoltObject::none().bits()),
                    2 => return molt_super_new(pos_args[0], pos_args[1]),
                    n => {
                        let msg = format!("super() expected at most 2 arguments, got {n}");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            }

            if class_bits == builtins.enumerate {
                if pos_args.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "enumerate() missing required argument 'iterable' (pos 1)",
                    );
                }
                if pos_args.len() > 2 {
                    let msg = format!(
                        "enumerate expected at most 2 arguments, got {}",
                        pos_args.len()
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let iterable_bits = pos_args[0];
                let mut start_opt = if pos_args.len() == 2 {
                    Some(pos_args[1])
                } else {
                    None
                };
                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    if name != "start" {
                        let msg =
                            format!("enumerate() got an unexpected keyword argument '{name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if start_opt.is_some() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "enumerate() got multiple values for argument 'start'",
                        );
                    }
                    start_opt = Some(val_bits);
                }
                return crate::object::ops::enumerate_new_impl(_py, iterable_bits, start_opt);
            }

            if class_bits == builtins.bool {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "bool() takes no keyword arguments",
                    );
                }
                if pos_args.len() > 1 {
                    let msg = format!("bool expected at most 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if pos_args.is_empty() {
                    return MoltObject::from_bool(false).bits();
                }
                let result = is_truthy(_py, obj_from_bits(pos_args[0]));
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_bool(result).bits();
            }

            if class_bits == builtins.float {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "float() takes no keyword arguments",
                    );
                }
                if pos_args.len() > 1 {
                    let msg = format!("float expected at most 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if pos_args.is_empty() {
                    return MoltObject::from_float(0.0).bits();
                }
                return crate::molt_float_from_obj(pos_args[0]);
            }

            if class_bits == builtins.complex {
                if pos_args.len() > 2 {
                    let msg = format!(
                        "complex expected at most 2 arguments, got {}",
                        pos_args.len()
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let mut real_opt = pos_args.first().copied();
                let mut imag_opt = if pos_args.len() == 2 {
                    Some(pos_args[1])
                } else {
                    None
                };
                let mut has_imag = imag_opt.is_some();

                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    match name.as_str() {
                        "real" => {
                            if real_opt.is_some() {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "complex() got multiple values for argument 'real'",
                                );
                            }
                            real_opt = Some(val_bits);
                        }
                        "imag" => {
                            if imag_opt.is_some() {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "complex() got multiple values for argument 'imag'",
                                );
                            }
                            imag_opt = Some(val_bits);
                            has_imag = true;
                        }
                        _ => {
                            let msg =
                                format!("complex() got an unexpected keyword argument '{name}'");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    }
                }

                let real_bits = real_opt.unwrap_or_else(|| MoltObject::from_int(0).bits());
                let imag_bits = imag_opt.unwrap_or_else(|| MoltObject::from_int(0).bits());
                let has_imag_bits = MoltObject::from_int(if has_imag { 1 } else { 0 }).bits();
                return crate::molt_complex_from_obj(real_bits, imag_bits, has_imag_bits);
            }

            if class_bits == builtins.reversed {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "reversed() takes no keyword arguments",
                    );
                }
                if pos_args.len() != 1 {
                    let msg = format!("reversed expected 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return crate::object::ops::reversed_new_impl(_py, pos_args[0]);
            }

            if class_bits == builtins.map {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "map() takes no keyword arguments",
                    );
                }
                if pos_args.len() < 2 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "map() must have at least two arguments",
                    );
                }
                return crate::object::ops::map_new_impl(_py, pos_args[0], &pos_args[1..]);
            }

            if class_bits == builtins.filter {
                if !kw_names.is_empty() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "filter() takes no keyword arguments",
                    );
                }
                if pos_args.len() != 2 {
                    let msg = format!("filter expected 2 arguments, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return crate::object::ops::filter_new_impl(_py, pos_args[0], pos_args[1]);
            }

            if class_bits == builtins.zip {
                let mut strict = false;
                for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<name>".to_string());
                    if name != "strict" {
                        let msg = format!("zip() got an unexpected keyword argument '{name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    strict = is_truthy(_py, obj_from_bits(val_bits));
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                return crate::object::ops::zip_new_impl(_py, pos_args, strict);
            }

            if class_bits == builtins.dict {
                let dict_bits = match pos_args.len() {
                    0 => molt_dict_new(0),
                    1 => molt_dict_from_obj(pos_args[0]),
                    _ => {
                        let msg =
                            format!("dict expected at most 1 argument, got {}", pos_args.len());
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if obj_from_bits(dict_bits).is_none() {
                    return MoltObject::none().bits();
                }
                for (name_bits, val_bits) in kw_names.iter().copied().zip(kw_values.iter().copied())
                {
                    dict_update_set_via_store(_py, dict_bits, name_bits, val_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, dict_bits);
                        return MoltObject::none().bits();
                    }
                }
                return dict_bits;
            }
            if class_bits == builtins.text_io_wrapper
                && let Some(ptr) = args_ptr
                && !(*ptr).kw_names.is_empty()
            {
                if let Some(bound_args) = bind_builtin_class_text_io_wrapper(_py, &*ptr) {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if class_bits == builtins.string_io
                && let Some(ptr) = args_ptr
                && !(*ptr).kw_names.is_empty()
            {
                if let Some(bound_args) = bind_builtin_class_string_io(_py, &*ptr) {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if let Some(ptr) = args_ptr {
                if !(*ptr).kw_names.is_empty() {
                    let class_name = class_name_for_error(class_bits);
                    let msg = format!("{class_name}() takes no keyword arguments");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                return call_class_init_with_args(_py, call_ptr, &(*ptr).pos);
            }
            return call_class_init_with_args(_py, call_ptr, &[]);
        }
        let mut resolved_new_bits = None;
        let is_exc_subclass = issubclass_bits(class_bits, builtins.base_exception);
        if trace_call_type_builder_enabled() {
            let class_name = class_name_for_error(class_bits);
            eprintln!(
                "[DEBUG] call_type_with_builder: class={} bits={:#x} is_exc_subclass={}",
                class_name, class_bits, is_exc_subclass
            );
        }
        let inst_bits = if is_exc_subclass {
            let new_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
            if let Some(new_bits) = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits) {
                if obj_from_bits(new_bits).as_ptr().is_some() {
                    inc_ref_bits(_py, new_bits);
                }
                // Detect whether __new__ is the default Exception.__new__
                // (molt_exception_new_bound).  The default __new__ only
                // accepts positional args (cls, *args); keyword args must be
                // forwarded exclusively to __init__.  A user-defined __new__
                // may accept keyword args, so forward everything in that case.
                let default_new = if let Some(new_ptr) = obj_from_bits(new_bits).as_ptr() {
                    let func_ptr = match object_type_id(new_ptr) {
                        TYPE_ID_FUNCTION => Some(new_ptr),
                        TYPE_ID_BOUND_METHOD => {
                            let inner = bound_method_func_bits(new_ptr);
                            obj_from_bits(inner)
                                .as_ptr()
                                .filter(|p| object_type_id(*p) == TYPE_ID_FUNCTION)
                        }
                        _ => None,
                    };
                    func_ptr.is_some_and(|fp| {
                        function_fn_ptr(fp)
                            == fn_addr!(crate::builtins::exceptions::molt_exception_new_bound)
                    })
                } else {
                    false
                };
                // When __new__ is the default Exception.__new__, bypass
                // call_bind dispatch and use alloc_exception_from_class_bits
                // directly.  The default __new__ expects (cls, args_tuple),
                // not a callargs builder, and does not accept keyword args.
                // Keywords will be forwarded to __init__ later.
                let inst_bits = if default_new {
                    let args_bits = if builder_ptr.is_null() {
                        let tp = alloc_tuple(_py, &[]);
                        if tp.is_null() {
                            return MoltObject::none().bits();
                        }
                        MoltObject::from_ptr(tp).bits()
                    } else {
                        let ap = callargs_ptr(builder_ptr);
                        let tp = if ap.is_null() {
                            alloc_tuple(_py, &[])
                        } else {
                            alloc_tuple(_py, &(*ap).pos)
                        };
                        if tp.is_null() {
                            return MoltObject::none().bits();
                        }
                        MoltObject::from_ptr(tp).bits()
                    };
                    let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
                    dec_ref_bits(_py, args_bits);
                    if exc_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(exc_ptr).bits()
                } else {
                    // Custom __new__: forward all args including keywords.
                    let (pos_len, kw_len) = if builder_ptr.is_null() {
                        (1usize, 0usize)
                    } else {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if args_ptr.is_null() {
                            (1usize, 0usize)
                        } else {
                            (1 + (*args_ptr).pos.len(), (*args_ptr).kw_names.len())
                        }
                    };
                    let new_builder_bits = molt_callargs_new(pos_len as u64, kw_len as u64);
                    if new_builder_bits == 0 {
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(new_builder_bits, class_bits);
                    if !builder_ptr.is_null() {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if !args_ptr.is_null() {
                            for &arg in (*args_ptr).pos.iter() {
                                let _ = molt_callargs_push_pos(new_builder_bits, arg);
                            }
                            for (&name_bits, &val_bits) in (*args_ptr)
                                .kw_names
                                .iter()
                                .zip((*args_ptr).kw_values.iter())
                            {
                                let _ =
                                    molt_callargs_push_kw(new_builder_bits, name_bits, val_bits);
                            }
                        }
                    }
                    molt_call_bind(new_bits, new_builder_bits)
                };
                if exception_pending(_py) {
                    dec_ref_bits(_py, new_bits);
                    return MoltObject::none().bits();
                }
                if !isinstance_bits(_py, inst_bits, class_bits) {
                    dec_ref_bits(_py, new_bits);
                    return inst_bits;
                }
                dec_ref_bits(_py, new_bits);
                inst_bits
            } else {
                let args_bits = if builder_ptr.is_null() {
                    let args_ptr = alloc_tuple(_py, &[]);
                    if args_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(args_ptr).bits()
                } else {
                    let args_ptr = callargs_ptr(builder_ptr);
                    let tuple_ptr = if args_ptr.is_null() {
                        alloc_tuple(_py, &[])
                    } else {
                        alloc_tuple(_py, &(*args_ptr).pos)
                    };
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                };
                let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
                dec_ref_bits(_py, args_bits);
                if exc_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(exc_ptr).bits()
            }
        } else {
            let new_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
            if let Some(new_bits) = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits) {
                if trace_call_type_builder_enabled() {
                    let new_obj = obj_from_bits(new_bits);
                    let new_type = type_name(_py, new_obj);
                    let fn_ptr = new_obj.as_ptr().and_then(|ptr| {
                        if object_type_id(ptr) == TYPE_ID_FUNCTION {
                            Some(function_fn_ptr(ptr) as usize)
                        } else {
                            None
                        }
                    });
                    eprintln!(
                        "[DEBUG] call_type_with_builder __new__ type={} bits=0x{:x} fn_ptr={:?}",
                        new_type, new_bits, fn_ptr
                    );
                }
                if obj_from_bits(new_bits).as_ptr().is_some() {
                    inc_ref_bits(_py, new_bits);
                }
                resolved_new_bits = Some(new_bits);
                let default_new = resolved_new_is_default_object_new(resolved_new_bits);
                if default_new {
                    let inst_bits = alloc_instance_for_default_object_new(_py, call_ptr);
                    dec_ref_bits(_py, new_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !isinstance_bits(_py, inst_bits, class_bits) {
                        return inst_bits;
                    }
                    inst_bits
                } else {
                    let (pos_len, kw_len) = if builder_ptr.is_null() {
                        (1usize, 0usize)
                    } else {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if args_ptr.is_null() {
                            (1usize, 0usize)
                        } else {
                            (1 + (*args_ptr).pos.len(), (*args_ptr).kw_names.len())
                        }
                    };
                    let new_builder_bits = molt_callargs_new(pos_len as u64, kw_len as u64);
                    if new_builder_bits == 0 {
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(new_builder_bits, class_bits);
                    if !builder_ptr.is_null() {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if !args_ptr.is_null() {
                            for &arg in (*args_ptr).pos.iter() {
                                let _ = molt_callargs_push_pos(new_builder_bits, arg);
                            }
                            for (&name_bits, &val_bits) in (*args_ptr)
                                .kw_names
                                .iter()
                                .zip((*args_ptr).kw_values.iter())
                            {
                                let _ =
                                    molt_callargs_push_kw(new_builder_bits, name_bits, val_bits);
                            }
                        }
                    }
                    let inst_bits = molt_call_bind(new_bits, new_builder_bits);
                    dec_ref_bits(_py, new_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !isinstance_bits(_py, inst_bits, class_bits) {
                        return inst_bits;
                    }
                    inst_bits
                }
            } else {
                alloc_instance_for_class(_py, call_ptr)
            }
        };
        let init_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
        let Some(init_bits) = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits) else {
            return inst_bits;
        };
        if trace_call_type_builder_enabled() {
            let init_obj = obj_from_bits(init_bits);
            let init_type = type_name(_py, init_obj);
            let fn_ptr = init_obj.as_ptr().and_then(|ptr| {
                if object_type_id(ptr) == TYPE_ID_FUNCTION {
                    Some(function_fn_ptr(ptr) as usize)
                } else {
                    None
                }
            });
            eprintln!(
                "[DEBUG] call_type_with_builder __init__ type={} bits=0x{:x} fn_ptr={:?}",
                init_type, init_bits, fn_ptr
            );
        }
        if obj_from_bits(init_bits).as_ptr().is_some() {
            inc_ref_bits(_py, init_bits);
        }
        let init_policy = if is_exc_subclass {
            InitArgPolicy::ForwardArgs
        } else {
            resolved_constructor_init_policy(resolved_new_bits, Some(init_bits))
        };
        match init_policy {
            InitArgPolicy::RejectConstructorArgs if !builder_ptr.is_null() => {
                let args_ptr = callargs_ptr(builder_ptr);
                if !args_ptr.is_null()
                    && (!(*args_ptr).pos.is_empty() || !(*args_ptr).kw_names.is_empty())
                {
                    let class_name = class_name_for_error(class_bits);
                    let msg = format!("{class_name}() takes no arguments");
                    dec_ref_bits(_py, init_bits);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
            InitArgPolicy::RejectConstructorArgs | InitArgPolicy::SkipObjectInit => {
                dec_ref_bits(_py, init_bits);
                return inst_bits;
            }
            InitArgPolicy::ForwardArgs => {}
        }
        if builder_ptr.is_null() {
            dec_ref_bits(_py, init_bits);
            return inst_bits;
        }
        builder_guard.release();
        let args_ptr = callargs_ptr(builder_ptr);
        if !args_ptr.is_null() {
            // The CallArgs builder owns its slots; take a reference for the instance we inject.
            inc_ref_bits(_py, inst_bits);
            (*args_ptr).pos.insert(0, inst_bits);
        }
        let _ = molt_call_bind(init_bits, builder_bits);
        dec_ref_bits(_py, init_bits);
        inst_bits
    }
}

unsafe fn build_class_from_args(
    _py: &PyToken<'_>,
    metaclass_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    unsafe {
        let strip_internal_namespace_keys = |namespace_bits: u64| -> Result<(), u64> {
            let Some(namespace_ptr) = obj_from_bits(namespace_bits).as_ptr() else {
                return Ok(());
            };
            if object_type_id(namespace_ptr) != TYPE_ID_DICT {
                return Ok(());
            }
            {
                let key = b"__classdictcell__".as_slice();
                let Some(key_bits) = attr_name_bits_from_bytes(_py, key) else {
                    return Err(MoltObject::none().bits());
                };
                dict_del_in_place(_py, namespace_ptr, key_bits);
                dec_ref_bits(_py, key_bits);
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
            }
            Ok(())
        };

        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        }

        let mut bases_vec: Vec<u64> = Vec::new();
        let mut bases_tuple_bits = bases_bits;
        let mut bases_owned = false;
        if obj_from_bits(bases_bits).is_none() || bases_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
        } else if let Some(bases_ptr) = obj_from_bits(bases_bits).as_ptr() {
            match object_type_id(bases_ptr) {
                TYPE_ID_TUPLE => {
                    bases_vec = seq_vec_ref(bases_ptr).clone();
                }
                TYPE_ID_TYPE => {
                    let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    bases_owned = true;
                    bases_vec.push(bases_bits);
                }
                _ => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "bases must be a tuple of types",
                    );
                }
            }
        }

        if bases_vec.is_empty() {
            let builtins = builtin_classes(_py);
            let tuple_ptr = alloc_tuple(_py, &[builtins.object]);
            if tuple_ptr.is_null() {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            bases_owned = true;
            bases_vec.push(builtins.object);
        }

        let mut winner_bits = metaclass_bits;
        for base_bits in bases_vec.iter().copied() {
            let base_meta_bits = type_of_bits(_py, base_bits);
            if issubclass_bits(winner_bits, base_meta_bits) {
                continue;
            }
            if issubclass_bits(base_meta_bits, winner_bits) {
                winner_bits = base_meta_bits;
                continue;
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            );
        }

        if winner_bits != metaclass_bits {
            if let Err(err) = strip_internal_namespace_keys(namespace_bits) {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return err;
            }
            let builder_bits =
                molt_callargs_new((3 + kw_names.len()) as u64, kw_names.len() as u64);
            if builder_bits == 0 {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            let _ = molt_callargs_push_pos(builder_bits, name_bits);
            let _ = molt_callargs_push_pos(builder_bits, bases_tuple_bits);
            let _ = molt_callargs_push_pos(builder_bits, namespace_bits);
            for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
                let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
            }
            let class_bits = molt_call_bind(winner_bits, builder_bits);
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return class_bits;
        }

        let class_ptr = alloc_class_obj(_py, name_bits);
        if class_ptr.is_null() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        object_set_class_bits(_py, class_ptr, metaclass_bits);
        inc_ref_bits(_py, metaclass_bits);

        if let Err(err) = strip_internal_namespace_keys(namespace_bits) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return err;
        }

        let dict_bits = class_dict_bits(class_ptr);
        let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, namespace_bits);
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }

        let _ = molt_class_set_base(class_bits, bases_tuple_bits);
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        if !apply_class_slots_layout(_py, class_ptr) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }

        if !dispatch_init_subclass_hooks(_py, &bases_vec, class_bits, kw_names, kw_values) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }

        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        class_bits
    }
}

pub(crate) unsafe fn callargs_ptr(ptr: *mut u8) -> *mut CallArgs {
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    callargs_builder_map()
        .lock()
        .unwrap()
        .get(&(ptr as usize))
        .copied()
        .map_or(std::ptr::null_mut(), |raw| raw.0)
}

unsafe fn require_callargs_ptr(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
) -> Result<*mut CallArgs, u64> {
    unsafe {
        if builder_ptr.is_null() {
            return Ok(std::ptr::null_mut());
        }
        if !callargs_builder_is_live(builder_ptr) {
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] invalid_builder builder_ptr=0x{:x}",
                    builder_ptr as usize,
                );
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "invalid callargs builder",
            ));
        }
        let args_ptr = callargs_ptr(builder_ptr);
        if args_ptr.is_null() || !callargs_storage_is_live(args_ptr) {
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] invalid_storage builder_ptr=0x{:x} args_ptr=0x{:x}",
                    builder_ptr as usize, args_ptr as usize,
                );
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "invalid callargs storage",
            ));
        }
        Ok(args_ptr)
    }
}

pub(crate) unsafe fn callargs_dec_ref_all(_py: &PyToken<'_>, args_ptr: *mut CallArgs) {
    unsafe {
        if args_ptr.is_null() {
            return;
        }
        let args = &*args_ptr;
        for &bits in args.pos.iter() {
            dec_ref_bits(_py, bits);
        }
        for &bits in args.kw_names.iter() {
            dec_ref_bits(_py, bits);
        }
        for &bits in args.kw_values.iter() {
            dec_ref_bits(_py, bits);
        }
    }
}

/// Protect a call result from callargs cleanup.
///
/// When `molt_call_bind` (or its IC fast-path) calls the target function, the
/// callee may return one of the values that was passed through the CallArgs
/// builder. The `PtrDropGuard` will dec-ref the entire CallArgs (including all
/// stored positional and keyword values) as soon as the enclosing scope exits.
/// If the return value aliases a stored value, that dec-ref would free it before
/// the caller can use it — a use-after-free.
///
/// This function checks whether `result` bit-equals any value in the CallArgs
/// positional or keyword-value slots. If so, it inc-refs `result` so that the
/// caller receives an independently-owned reference that survives the CallArgs
/// cleanup.
///
/// # Safety
/// `args_ptr` must be null or point to a valid `CallArgs`.
unsafe fn protect_callargs_aliased_return(
    _py: &PyToken<'_>,
    result: u64,
    args_ptr: *mut CallArgs,
) -> u64 {
    unsafe { protect_callargs_aliased_return_with_extra(_py, result, args_ptr, &[]) }
}

/// Protect a call result from cleanup of builder-owned values plus any
/// additional synthesized owned arguments passed outside the builder.
///
/// Some fast-path IC lanes synthesize a receiver locally instead of pushing it
/// through `CallArgs`. If the callee returns that synthesized receiver (for
/// example `return self`), the caller must still receive an owned reference
/// that survives surrounding cleanup.
unsafe fn protect_callargs_aliased_return_with_extra(
    _py: &PyToken<'_>,
    result: u64,
    args_ptr: *mut CallArgs,
    extra_owned: &[u64],
) -> u64 {
    unsafe {
        if extra_owned.contains(&result) {
            inc_ref_bits(_py, result);
            return result;
        }
        if !args_ptr.is_null() {
            let args = &*args_ptr;
            for &val in args.pos.iter().chain(args.kw_values.iter()) {
                if val == result {
                    inc_ref_bits(_py, result);
                    break;
                }
            }
        }
        result
    }
}

unsafe fn call_capi_method_with_bound_args(
    _py: &PyToken<'_>,
    func_bits: u64,
    args_ptr: *mut CallArgs,
    args: &CallArgs,
) -> u64 {
    unsafe {
        let tuple_ptr = alloc_tuple(_py, args.pos.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let mut kwargs_owned = false;
        let kwargs_bits = if args.kw_names.is_empty() {
            MoltObject::none().bits()
        } else {
            let mut pairs = Vec::with_capacity(args.kw_names.len().saturating_mul(2));
            for (name_bits, val_bits) in args
                .kw_names
                .iter()
                .copied()
                .zip(args.kw_values.iter().copied())
            {
                pairs.push(name_bits);
                pairs.push(val_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
            if dict_ptr.is_null() {
                dec_ref_bits(_py, tuple_bits);
                return MoltObject::none().bits();
            }
            kwargs_owned = true;
            MoltObject::from_ptr(dict_ptr).bits()
        };
        let mut result = call_function_obj_vec(_py, func_bits, &[tuple_bits, kwargs_bits]);
        if result == tuple_bits || (kwargs_owned && result == kwargs_bits) {
            inc_ref_bits(_py, result);
        }
        dec_ref_bits(_py, tuple_bits);
        if kwargs_owned {
            dec_ref_bits(_py, kwargs_bits);
        }
        result = protect_callargs_aliased_return(_py, result, args_ptr);
        result
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut CallArgs>();
        let ptr = alloc_object(_py, total, TYPE_ID_CALLARGS);
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let decode_capacity = |bits: u64| -> Option<usize> {
                let obj = MoltObject::from_bits(bits);
                if obj.is_int() {
                    let val = obj.as_int().unwrap_or(0);
                    return usize::try_from(val).ok();
                }
                if obj.is_bool() {
                    return Some(if obj.as_bool().unwrap_or(false) { 1 } else { 0 });
                }
                if obj.is_ptr() || obj.is_none() || obj.is_pending() {
                    return None;
                }
                if bits <= usize::MAX as u64 {
                    Some(bits as usize)
                } else {
                    None
                }
            };
            let Some(pos_capacity) = decode_capacity(pos_capacity_bits) else {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "callargs capacity expects an integer",
                );
                return 0;
            };
            let Some(kw_capacity) = decode_capacity(kw_capacity_bits) else {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "callargs capacity expects an integer",
                );
                return 0;
            };
            let args = Box::new(CallArgs {
                pos: Vec::with_capacity(pos_capacity),
                kw_names: Vec::with_capacity(kw_capacity),
                kw_values: Vec::with_capacity(kw_capacity),
                kw_seen: HashSet::with_capacity(kw_capacity),
            });
            // Track heap bytes: the CallArgs struct itself plus the capacity
            // reserved by each inner Vec/HashSet.
            let callargs_bytes = std::mem::size_of::<CallArgs>()
                + pos_capacity * std::mem::size_of::<u64>()
                + kw_capacity * std::mem::size_of::<u64>() * 2
                + kw_capacity * std::mem::size_of::<String>();
            ALLOC_BYTES_CALLARGS
                .fetch_add(callargs_bytes as u64, std::sync::atomic::Ordering::Relaxed);
            let args_ptr = Box::into_raw(args);
            note_callargs_alloc(ptr, args_ptr);
            *(ptr as *mut *mut CallArgs) = args_ptr;
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] new builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} pos_cap={} kw_cap={}",
                    bits_from_ptr(ptr),
                    ptr as usize,
                    args_ptr as usize,
                    pos_capacity,
                    kw_capacity,
                );
            }
        }
        bits_from_ptr(ptr)
    })
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new` and
/// remain owned by the caller for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_builder builder_bits=0x{:x} builder_ptr=0x{:x} live=true",
                    builder_bits, builder_ptr as usize,
                );
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_raw builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} val_type={} val_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    type_name(_py, obj_from_bits(val)),
                    val,
                );
            }
            let args = &mut *args_ptr;
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} len_before={} val_type={} val_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    args.pos.len(),
                    type_name(_py, obj_from_bits(val)),
                    val,
                );
            }
            // CallArgs must keep arguments alive even if the caller drops its temporaries before
            // `molt_call_bind` executes.
            inc_ref_bits(_py, val);
            args.pos.push(val);
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] push_pos_done builder_bits=0x{:x} len_after={}",
                    builder_bits,
                    args.pos.len(),
                );
            }
            MoltObject::none().bits()
        })
    }
}

unsafe fn callargs_push_kw(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
        };
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
        }
        let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
            Ok(ptr) => ptr,
            Err(err) => return err,
        };
        let args = &mut *args_ptr;
        let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if args.kw_seen.contains(&name) {
            let msg = format!("got multiple values for keyword argument '{name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        // CallArgs must keep keyword arguments alive even if the caller drops its temporaries
        // before `molt_call_bind` executes.
        inc_ref_bits(_py, name_bits);
        inc_ref_bits(_py, val_bits);
        args.kw_seen.insert(name);
        args.kw_names.push(name_bits);
        args.kw_values.push(val_bits);
        MoltObject::none().bits()
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
/// `name_bits` must reference a Molt string object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_push_kw(
    builder_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            callargs_push_kw(_py, builder_ptr, name_bits, val_bits)
        })
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            if trace_callargs_enabled() {
                eprintln!(
                    "[molt callargs] expand_star_builder builder_bits=0x{:x} builder_ptr=0x{:x} live=true",
                    builder_bits, builder_ptr as usize,
                );
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            if trace_callargs_enabled() {
                let len = if args_ptr.is_null() {
                    0
                } else {
                    (&*args_ptr).pos.len()
                };
                eprintln!(
                    "[molt callargs] expand_star builder_bits=0x{:x} builder_ptr=0x{:x} args_ptr=0x{:x} len_before={} iterable_type={} iterable_bits=0x{:x}",
                    builder_bits,
                    builder_ptr as usize,
                    args_ptr as usize,
                    len,
                    type_name(_py, obj_from_bits(iterable_bits)),
                    iterable_bits,
                );
            }
            let iter_bits = molt_iter(iterable_bits);
            if obj_from_bits(iter_bits).is_none() {
                return raise_not_iterable(_py, iterable_bits);
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let val_bits = elems[0];
                if trace_callargs_enabled() {
                    eprintln!(
                        "[molt callargs] expand_star_item builder_bits=0x{:x} val_type={} val_bits=0x{:x}",
                        builder_bits,
                        type_name(_py, obj_from_bits(val_bits)),
                        val_bits,
                    );
                }
                let res = molt_callargs_push_pos(builder_bits, val_bits);
                if obj_from_bits(res).is_none() && exception_pending(_py) {
                    return res;
                }
            }
            MoltObject::none().bits()
        })
    }
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_callargs_expand_kwstar(builder_bits: u64, mapping_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let builder_ptr = ptr_from_bits(builder_bits);
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            if !callargs_builder_is_live(builder_ptr) {
                return raise_exception::<_>(_py, "TypeError", "invalid callargs builder");
            }
            let mapping_obj = obj_from_bits(mapping_bits);
            let _args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            let Some(mapping_ptr) = mapping_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            if object_type_id(mapping_ptr) == TYPE_ID_DICT {
                let order = dict_order(mapping_ptr);
                for idx in (0..order.len()).step_by(2) {
                    let key_bits = order[idx];
                    let val_bits = order[idx + 1];
                    let res = callargs_push_kw(_py, builder_ptr, key_bits, val_bits);
                    if obj_from_bits(res).is_none() && exception_pending(_py) {
                        return res;
                    }
                }
                return MoltObject::none().bits();
            }
            let Some(keys_bits) = attr_name_bits_from_bytes(_py, b"keys") else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            let keys_method_bits = attr_lookup_ptr(_py, mapping_ptr, keys_bits);
            dec_ref_bits(_py, keys_bits);
            let Some(keys_method_bits) = keys_method_bits else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            let keys_iterable = call_callable0(_py, keys_method_bits);
            let iter_bits = molt_iter(keys_iterable);
            if obj_from_bits(iter_bits).is_none() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            }
            let Some(getitem_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            let getitem_method_bits = attr_lookup_ptr(_py, mapping_ptr, getitem_bits);
            dec_ref_bits(_py, getitem_bits);
            let Some(getitem_method_bits) = getitem_method_bits else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_bits = elems[0];
                let key_obj = obj_from_bits(key_bits);
                let Some(key_ptr) = key_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                };
                if object_type_id(key_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                }
                let val_bits = call_callable1(_py, getitem_method_bits, key_bits);
                let res = callargs_push_kw(_py, builder_ptr, key_bits, val_bits);
                if obj_from_bits(res).is_none() && exception_pending(_py) {
                    return res;
                }
            }
            MoltObject::none().bits()
        })
    }
}

unsafe fn function_requires_full_binding(_py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
    let attr = |name_bytes: &'static [u8]| unsafe {
        function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                match name_bytes {
                    b"__molt_bind_kind__" => &runtime_state(_py).interned.molt_bind_kind,
                    b"__molt_vararg__" => &runtime_state(_py).interned.molt_vararg,
                    b"__molt_varkw__" => &runtime_state(_py).interned.molt_varkw,
                    b"__molt_kwonly_names__" => &runtime_state(_py).interned.molt_kwonly_names,
                    b"__defaults__" => &runtime_state(_py).interned.defaults_name,
                    b"__kwdefaults__" => &runtime_state(_py).interned.kwdefaults_name,
                    _ => unreachable!("unknown binding metadata attr"),
                },
                name_bytes,
            ),
        )
        .unwrap_or_else(|| MoltObject::none().bits())
    };

    for name in [
        b"__molt_bind_kind__".as_slice(),
        b"__molt_vararg__",
        b"__molt_varkw__",
    ] {
        if !obj_from_bits(attr(name)).is_none() {
            return true;
        }
    }

    let kwonly_bits = attr(b"__molt_kwonly_names__");
    if !obj_from_bits(kwonly_bits).is_none() {
        let Some(ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
            return true;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE
            || !unsafe { seq_vec_ref(ptr) }.is_empty()
        {
            return true;
        }
    }

    let defaults_bits = attr(b"__defaults__");
    if !obj_from_bits(defaults_bits).is_none() {
        let Some(ptr) = obj_from_bits(defaults_bits).as_ptr() else {
            return true;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE
            || !unsafe { seq_vec_ref(ptr) }.is_empty()
        {
            return true;
        }
    }

    let kwdefaults_bits = attr(b"__kwdefaults__");
    if !obj_from_bits(kwdefaults_bits).is_none() {
        let Some(ptr) = obj_from_bits(kwdefaults_bits).as_ptr() else {
            return true;
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT || !unsafe { dict_order(ptr) }.is_empty()
        {
            return true;
        }
    }

    false
}

unsafe fn call_bind_ic_entry_for_call(
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
                        fn_ptr: function_fn_ptr(call_ptr) as u64,
                        target_bits: call_bits,
                        class_bits: 0,
                        class_version: 0,
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
                        fn_ptr: fn_ptr as u64,
                        target_bits: func_bits,
                        class_bits: 0,
                        class_version: 0,
                        arity: 1,
                        kind: CALL_BIND_IC_KIND_LIST_APPEND,
                    })
                } else if !function_requires_full_binding(_py, func_ptr) {
                    let arity = function_arity(func_ptr);
                    if (1..=5).contains(&arity) {
                        Some(CallBindIcEntry {
                            fn_ptr: fn_ptr as u64,
                            target_bits: func_bits,
                            class_bits: 0,
                            class_version: 0,
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
                    return None;
                }
                // Only cacheable when __new__ is the default object.__new__.
                let new_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.new_name,
                    b"__new__",
                );
                let new_bits = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
                if !resolved_new_is_default_object_new(new_bits) {
                    return None;
                }
                // Resolve __init__ and ensure it is a simple direct-callable function.
                let init_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.init_name,
                    b"__init__",
                );
                let init_bits = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits)?;
                let init_ptr = obj_from_bits(init_bits).as_ptr()?;
                if object_type_id(init_ptr) != TYPE_ID_FUNCTION {
                    return None;
                }
                if function_requires_full_binding(_py, init_ptr) {
                    return None;
                }
                let init_arity = function_arity(init_ptr);
                // __init__ arity includes `self`, so cacheable range is 1..=5
                // (0 args up to 4 user args).
                if !(1..=5).contains(&init_arity) {
                    return None;
                }
                Some(CallBindIcEntry {
                    fn_ptr: function_fn_ptr(init_ptr) as u64,
                    target_bits: init_bits,
                    class_bits,
                    class_version: class_layout_version_bits(call_ptr),
                    arity: (init_arity - 1) as u8,
                    kind: CALL_BIND_IC_KIND_TYPE_CALL,
                })
            }
            TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                let call_attr_bits = lookup_call_attr(_py, call_ptr)?;
                let call_attr_ptr = obj_from_bits(call_attr_bits).as_ptr()?;
                if object_type_id(call_attr_ptr) != TYPE_ID_BOUND_METHOD {
                    return None;
                }
                let func_bits = bound_method_func_bits(call_attr_ptr);
                let func_ptr = obj_from_bits(func_bits).as_ptr()?;
                if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                    return None;
                }
                if function_requires_full_binding(_py, func_ptr) {
                    return None;
                }
                let arity = function_arity(func_ptr);
                if !(1..=5).contains(&arity) {
                    return None;
                }
                let class_bits = object_class_bits(call_ptr);
                let class_ptr = obj_from_bits(class_bits).as_ptr()?;
                Some(CallBindIcEntry {
                    fn_ptr: function_fn_ptr(func_ptr) as u64,
                    target_bits: func_bits,
                    class_bits,
                    class_version: class_layout_version_bits(class_ptr),
                    arity: (arity - 1) as u8,
                    kind: CALL_BIND_IC_KIND_OBJECT_CALL_SIMPLE_BOUND_FUNC,
                })
            }
            _ => None,
        }
    }
}

unsafe fn try_call_bind_ic_fast(
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
            if function_fn_ptr(func_ptr) as u64 != entry.fn_ptr {
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
            if function_fn_ptr(call_ptr) as u64 != entry.fn_ptr {
                return None;
            }
            if args.pos.len() != entry.arity as usize {
                return None;
            }
            let pos = args.pos.clone();
            return Some(call_function_obj_vec(_py, call_bits, pos.as_slice()));
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
            if function_fn_ptr(func_ptr) as u64 != entry.fn_ptr {
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
            let result = call_function_obj_vec(_py, func_bits, &argv[..args.pos.len() + 1]);
            return Some(protect_callargs_aliased_return_with_extra(
                _py,
                result,
                args_ptr,
                &[self_bits],
            ));
        }

        if entry.kind == CALL_BIND_IC_KIND_OBJECT_CALL_SIMPLE_BOUND_FUNC {
            if !matches!(object_type_id(call_ptr), TYPE_ID_OBJECT | TYPE_ID_DATACLASS) {
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
            if function_fn_ptr(func_ptr) as u64 != entry.fn_ptr {
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
            let result = call_function_obj_vec(_py, entry.target_bits, &argv[..args.pos.len() + 1]);
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
            if function_fn_ptr(init_ptr) as u64 != entry.fn_ptr {
                return None;
            }
            // Allocate instance (default object.__new__ path).
            let inst_bits = alloc_instance_for_default_object_new(_py, call_ptr);
            if exception_pending(_py) {
                return Some(MoltObject::none().bits());
            }
            // Fast-path __init__ call: bypass call_function_obj_vec to skip
            // profiling, exception baseline, trampoline probe, arity check,
            // and double enforce_no_pending.  We already validated fn_ptr,
            // arity, and no-full-binding in call_bind_ic_entry_for_call.
            let fn_ptr = entry.fn_ptr;
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
            frame_stack_push(_py, code_bits);
            let _init_result = if closure_bits != 0 {
                match args.pos.len() {
                    0 => {
                        let f: extern "C" fn(u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(closure_bits, inst_bits) as u64
                    }
                    1 => {
                        let f: extern "C" fn(u64, u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(closure_bits, inst_bits, args.pos[0]) as u64
                    }
                    2 => {
                        let f: extern "C" fn(u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(closure_bits, inst_bits, args.pos[0], args.pos[1]) as u64
                    }
                    3 => {
                        let f: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(closure_bits, inst_bits, args.pos[0], args.pos[1], args.pos[2])
                            as u64
                    }
                    _ => {
                        let mut argv = [0u64; 5];
                        argv[0] = inst_bits;
                        for (idx, arg) in args.pos.iter().copied().enumerate() {
                            argv[idx + 1] = arg;
                        }
                        call_function_obj_vec(
                            _py,
                            entry.target_bits,
                            &argv[..args.pos.len() + 1],
                        )
                    }
                }
            } else {
                match args.pos.len() {
                    0 => {
                        let f: extern "C" fn(u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(inst_bits) as u64
                    }
                    1 => {
                        let f: extern "C" fn(u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(inst_bits, args.pos[0]) as u64
                    }
                    2 => {
                        let f: extern "C" fn(u64, u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(inst_bits, args.pos[0], args.pos[1]) as u64
                    }
                    3 => {
                        let f: extern "C" fn(u64, u64, u64, u64) -> i64 =
                            std::mem::transmute(fn_ptr as usize);
                        f(inst_bits, args.pos[0], args.pos[1], args.pos[2]) as u64
                    }
                    _ => {
                        let mut argv = [0u64; 5];
                        argv[0] = inst_bits;
                        for (idx, arg) in args.pos.iter().copied().enumerate() {
                            argv[idx + 1] = arg;
                        }
                        call_function_obj_vec(
                            _py,
                            entry.target_bits,
                            &argv[..args.pos.len() + 1],
                        )
                    }
                }
            };
            frame_stack_pop(_py);
            recursion_guard_exit();
            if exception_pending(_py) {
                dec_ref_bits(_py, inst_bits);
                return Some(MoltObject::none().bits());
            }
            return Some(inst_bits);
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
                        CALL_BIND_IC_KIND_OBJECT_CALL_SIMPLE_BOUND_FUNC => {
                            "object_call_simple_bound_func"
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
                return protect_callargs_aliased_return(_py, res, args_ptr);
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
        if let Some(entry) = call_bind_ic_entry_for_call(_py, call_bits) {
            ic_tls_insert(site_id, entry);
            // Also update global cache for cross-thread visibility
            call_bind_ic_cache().lock().unwrap().insert(site_id, entry);
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

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub extern "C" fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let builder_ptr = ptr_from_bits(builder_bits);
            let mut builder_guard = PtrDropGuard::new(builder_ptr);
            let call_obj = obj_from_bits(call_bits);
            let cached_mode = trace_call_bind_mode();
            let trace = !matches!(cached_mode, TraceCallBindMode::Off);
            let trace_verbose = matches!(cached_mode, TraceCallBindMode::Verbose);
            if trace_verbose {
                let callee_type = type_name(_py, call_obj);
                let (pos_len, kw_len, first_pos) = if !builder_ptr.is_null() {
                    match require_callargs_ptr(_py, builder_ptr) {
                        Ok(args_ptr) => (
                            (*args_ptr).pos.len(),
                            (*args_ptr).kw_names.len(),
                            (*args_ptr).pos.first().copied(),
                        ),
                        Err(_) => (0, 0, None),
                    }
                } else {
                    (0, 0, None)
                };
                let first_pos_type = first_pos
                    .map(|bits| type_name(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| std::borrow::Cow::Borrowed("<none>"));
                eprintln!(
                    "molt call_bind enter callee_bits=0x{call_bits:x} callee_type={} pos_len={} kw_len={} first_pos_type={}",
                    callee_type, pos_len, kw_len, first_pos_type
                );
            }
            let Some(call_ptr) = call_obj.as_ptr() else {
                if trace {
                    if let Some(frame) = FRAME_STACK.with(|stack| stack.borrow().last().copied())
                        && let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits)
                    {
                        let (name_bits, file_bits) =
                            (code_name_bits(code_ptr), code_filename_bits(code_ptr));
                        let name = string_obj_to_owned(obj_from_bits(name_bits))
                            .unwrap_or_else(|| "<code>".to_string());
                        let file = string_obj_to_owned(obj_from_bits(file_bits))
                            .unwrap_or_else(|| "<file>".to_string());
                        eprintln!(
                            "molt call_bind frame name={} file={} line={}",
                            name, file, frame.line
                        );
                    }
                    let none_flag = call_obj.is_none();
                    let bool_flag = call_obj.as_bool();
                    let int_flag = call_obj.as_int();
                    let float_flag = call_obj.as_float();
                    eprintln!(
                        "molt call_bind callee bits=0x{call_bits:x} none={} bool={:?} int={:?} float={:?}",
                        none_flag, bool_flag, int_flag, float_flag,
                    );
                    let bt = std::backtrace::Backtrace::force_capture();
                    eprintln!("molt call_bind: not ptr bits=0x{call_bits:x}\n{bt}",);
                    if !builder_ptr.is_null() {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if !args_ptr.is_null() {
                            let pos_slice = &(*args_ptr).pos;
                            let kw_slice = &(*args_ptr).kw_names;
                            let pos_len = pos_slice.len();
                            let kw_len = kw_slice.len();
                            let first_pos = pos_slice.first().copied();
                            let second_pos = pos_slice.get(1).copied();
                            eprintln!(
                                "molt call_bind args pos_len={} kw_len={} first_pos={:?} second_pos={:?}",
                                pos_len, kw_len, first_pos, second_pos,
                            );
                            if let Some(bits) = first_pos {
                                eprintln!(
                                    "molt call_bind args first_pos_bits=0x{bits:x} first_pos_type={}",
                                    type_name(_py, obj_from_bits(bits)),
                                );
                                if let Some(s) = string_obj_to_owned(obj_from_bits(bits)) {
                                    eprintln!("molt call_bind args first_pos_str={}", s);
                                }
                            }
                            if let Some(bits) = second_pos
                                && let Some(s) = string_obj_to_owned(obj_from_bits(bits))
                            {
                                eprintln!("molt call_bind args second_pos_str={}", s);
                            }
                        } else {
                            eprintln!("molt call_bind args ptr is null");
                        }
                    }
                }
                return raise_not_callable(_py, call_obj);
            };
            let mut func_bits = call_bits;
            let mut self_bits = None;
            match object_type_id(call_ptr) {
                TYPE_ID_FUNCTION => {}
                TYPE_ID_BOUND_METHOD => {
                    func_bits = bound_method_func_bits(call_ptr);
                    self_bits = Some(bound_method_self_bits(call_ptr));
                }
                TYPE_ID_TYPE => {
                    let meta_bits = object_class_bits(call_ptr);
                    if meta_bits != 0
                        && let Some(meta_ptr) = obj_from_bits(meta_bits).as_ptr()
                        && object_type_id(meta_ptr) == TYPE_ID_TYPE
                    {
                        let call_name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.call_name,
                            b"__call__",
                        );
                        if let Some(call_attr_bits) = class_attr_lookup(
                            _py,
                            meta_ptr,
                            meta_ptr,
                            Some(call_ptr),
                            call_name_bits,
                        ) && !is_default_type_call(_py, call_attr_bits)
                        {
                            builder_guard.release();
                            return molt_call_bind(call_attr_bits, builder_bits);
                        }
                    }
                    return call_type_with_builder(
                        _py,
                        call_ptr,
                        builder_ptr,
                        builder_bits,
                        &mut builder_guard,
                    );
                }
                TYPE_ID_GENERIC_ALIAS => {
                    let origin_bits = generic_alias_origin_bits(call_ptr);
                    builder_guard.release();
                    return molt_call_bind(origin_bits, builder_bits);
                }
                TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                    let Some(call_attr_bits) = lookup_call_attr(_py, call_ptr) else {
                        return raise_not_callable(_py, call_obj);
                    };
                    if !builder_ptr.is_null() {
                        let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                            Ok(ptr) => ptr,
                            Err(err) => return err,
                        };
                        if let Some(entry) = call_bind_ic_entry_for_call(_py, call_attr_bits)
                            && let Some(res) =
                                try_call_bind_ic_fast(_py, entry, call_attr_bits, args_ptr)
                        {
                            return protect_callargs_aliased_return(_py, res, args_ptr);
                        }
                    }
                    builder_guard.release();
                    return molt_call_bind(call_attr_bits, builder_bits);
                }
                _ => return raise_not_callable(_py, call_obj),
            }
            let func_obj = obj_from_bits(func_bits);
            let Some(func_ptr) = func_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            }
            let fn_ptr = function_fn_ptr(func_ptr);
            if callable_matches_runtime_symbol(Some(func_bits), fn_addr!(molt_type_call)) {
                let Some(self_bits) = self_bits else {
                    return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
                };
                let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
                };
                if object_type_id(self_ptr) != TYPE_ID_TYPE {
                    return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
                }
                return call_type_with_builder(
                    _py,
                    self_ptr,
                    builder_ptr,
                    builder_bits,
                    &mut builder_guard,
                );
            }
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                Ok(ptr) => ptr,
                Err(err) => return err,
            };
            let args = &mut *args_ptr;
            if let Some(self_bits) = self_bits {
                // The CallArgs builder owns its slots (see `molt_callargs_push_pos`), so inserting
                // an extra positional argument must take a reference as well.
                inc_ref_bits(_py, self_bits);
                args.pos.insert(0, self_bits);
            }
            let bind_kind_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_bind_kind,
                    b"__molt_bind_kind__",
                ),
            );
            if let Some(kind_bits) = bind_kind_bits
                && obj_from_bits(kind_bits).as_int() == Some(BIND_KIND_OPEN)
            {
                if let Some(bound_args) = bind_builtin_open(_py, args) {
                    return call_function_obj_vec(_py, func_bits, bound_args.as_slice());
                }
                return MoltObject::none().bits();
            }
            if let Some(kind_bits) = bind_kind_bits
                && obj_from_bits(kind_bits).as_int() == Some(BIND_KIND_CAPI_METHOD)
            {
                return call_capi_method_with_bound_args(_py, func_bits, args_ptr, args);
            }
            if fn_ptr == fn_addr!(dict_update_method) {
                return bind_builtin_dict_update(_py, args);
            }
            if fn_ptr == fn_addr!(molt_open_builtin) {
                if let Some(bound_args) = bind_builtin_open(_py, args) {
                    return call_function_obj_vec(_py, func_bits, bound_args.as_slice());
                }
                return MoltObject::none().bits();
            }

            let arg_names_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_arg_names,
                    b"__molt_arg_names__",
                ),
            );
            let arg_names = if let Some(bits) = arg_names_bits {
                let arg_names_ptr = obj_from_bits(bits).as_ptr();
                let Some(arg_names_ptr) = arg_names_ptr else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(arg_names_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                seq_vec_ref(arg_names_ptr).clone()
            } else {
                if let Some(bound_args) = bind_builtin_call(_py, func_bits, func_ptr, args) {
                    return call_function_obj_vec(_py, func_bits, bound_args.as_slice());
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };

            let posonly_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_posonly,
                    b"__molt_posonly__",
                ),
            )
            .unwrap_or_else(|| MoltObject::from_int(0).bits());
            let posonly = obj_from_bits(posonly_bits).as_int().unwrap_or(0).max(0) as usize;

            let kwonly_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_kwonly_names,
                    b"__molt_kwonly_names__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let mut kwonly_names: Vec<u64> = Vec::new();
            if !obj_from_bits(kwonly_bits).is_none() {
                let Some(kw_ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(kw_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                kwonly_names = seq_vec_ref(kw_ptr).clone();
            }

            let vararg_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_vararg,
                    b"__molt_vararg__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let varkw_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_varkw,
                    b"__molt_varkw__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let has_vararg = !obj_from_bits(vararg_bits).is_none();
            let has_varkw = !obj_from_bits(varkw_bits).is_none();

            let defaults_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.defaults_name,
                    b"__defaults__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let mut defaults: Vec<u64> = Vec::new();
            if !obj_from_bits(defaults_bits).is_none() {
                let Some(def_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                defaults = seq_vec_ref(def_ptr).clone();
            }

            let kwdefaults_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.kwdefaults_name,
                    b"__kwdefaults__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let mut kwdefaults_ptr = None;
            if !obj_from_bits(kwdefaults_bits).is_none() {
                let Some(ptr) = obj_from_bits(kwdefaults_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(ptr) != TYPE_ID_DICT {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                kwdefaults_ptr = Some(ptr);
            }

            if trace_function_bind_meta_enabled() {
                let func_name_bits = function_name_bits(_py, func_ptr);
                let func_name = if func_name_bits == 0 || obj_from_bits(func_name_bits).is_none() {
                    "<unnamed>".to_string()
                } else {
                    string_obj_to_owned(obj_from_bits(func_name_bits))
                        .unwrap_or_else(|| "<unnamed>".to_string())
                };
                eprintln!(
                    "[molt bind_meta] name={} total_pos={} posonly={} kwonly={} has_vararg={} has_varkw={} defaults={} kwdefaults={}",
                    func_name,
                    arg_names.len(),
                    posonly,
                    kwonly_names.len(),
                    has_vararg,
                    has_varkw,
                    defaults.len(),
                    kwdefaults_ptr.map(|ptr| dict_order(ptr).len()).unwrap_or(0),
                );
            }

            let total_pos = arg_names.len();
            let kwonly_start = total_pos + if has_vararg { 1 } else { 0 };
            let total_params = kwonly_start + kwonly_names.len() + if has_varkw { 1 } else { 0 };
            let mut slots: Vec<Option<u64>> = vec![None; total_params];
            let mut extra_pos: Vec<u64> = Vec::new();
            for (idx, val) in args.pos.iter().copied().enumerate() {
                if idx < total_pos {
                    slots[idx] = Some(val);
                } else if has_vararg {
                    extra_pos.push(val);
                } else {
                    let func_name_bits = function_attr_bits(
                        _py,
                        func_ptr,
                        intern_static_name(
                            _py,
                            &runtime_state(_py).interned.name_name,
                            b"__name__",
                        ),
                    );
                    let fname = func_name_bits
                        .and_then(|b| string_obj_to_owned(obj_from_bits(b)))
                        .unwrap_or_else(|| "?".to_string());
                    let arg_names_strs: Vec<String> = arg_names
                        .iter()
                        .map(|&b| {
                            string_obj_to_owned(obj_from_bits(b))
                                .unwrap_or_else(|| format!("<raw:{:x}>", b))
                        })
                        .collect();
                    let msg = format!(
                        "too many positional arguments for {}(): got {} positional, expected {} (arg_names={:?}, kwonly={}, vararg={}, varkw={})",
                        fname,
                        args.pos.len(),
                        total_pos,
                        arg_names_strs,
                        kwonly_names.len(),
                        has_vararg,
                        has_varkw,
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }

            let mut extra_kwargs: Vec<u64> = Vec::new();
            enum KeywordSlot {
                PosOnly,
                Slot(usize),
            }
            let mut keyword_slots: HashMap<String, KeywordSlot> =
                HashMap::with_capacity(total_pos + kwonly_names.len());
            for (idx, param_bits) in arg_names.iter().copied().enumerate() {
                let key = string_obj_to_owned(obj_from_bits(param_bits))
                    .unwrap_or_else(|| "?".to_string());
                let slot = if idx < posonly {
                    KeywordSlot::PosOnly
                } else {
                    KeywordSlot::Slot(idx)
                };
                keyword_slots.entry(key).or_insert(slot);
            }
            for (kw_idx, kw_name_bits) in kwonly_names.iter().copied().enumerate() {
                let key = string_obj_to_owned(obj_from_bits(kw_name_bits))
                    .unwrap_or_else(|| "?".to_string());
                keyword_slots
                    .entry(key)
                    .or_insert(KeywordSlot::Slot(kwonly_start + kw_idx));
            }
            let mut posonly_kw_names: Vec<String> = Vec::new();
            let mut posonly_kw_seen: HashSet<String> = HashSet::new();
            let mut unexpected_kw: Option<String> = None;
            for (name_bits, val_bits) in args
                .kw_names
                .iter()
                .copied()
                .zip(args.kw_values.iter().copied())
            {
                let name_obj = obj_from_bits(name_bits);
                let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                if let Some(slot) = keyword_slots.get(&name) {
                    match slot {
                        KeywordSlot::PosOnly => {
                            if posonly_kw_seen.insert(name.clone()) {
                                posonly_kw_names.push(name);
                            }
                        }
                        KeywordSlot::Slot(slot_idx) => {
                            if slots[*slot_idx].is_some() {
                                let msg = format!("got multiple values for argument '{name}'");
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                            slots[*slot_idx] = Some(val_bits);
                        }
                    }
                    continue;
                }
                if has_varkw {
                    extra_kwargs.push(name_bits);
                    extra_kwargs.push(val_bits);
                } else if unexpected_kw.is_none() {
                    unexpected_kw = Some(name);
                }
            }

            if !posonly_kw_names.is_empty() {
                let func_name_bits = function_name_bits(_py, func_ptr);
                let func_name = if func_name_bits == 0 || obj_from_bits(func_name_bits).is_none() {
                    "function".to_string()
                } else {
                    string_obj_to_owned(obj_from_bits(func_name_bits))
                        .unwrap_or_else(|| "function".to_string())
                };
                if func_name == "islice" {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "islice() takes no keyword arguments",
                    );
                }
                let name_list = posonly_kw_names.join(", ");
                let msg = format!(
                    "{func_name}() got some positional-only arguments passed as keyword arguments: '{name_list}'"
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            if let Some(name) = unexpected_kw {
                let msg = format!("got an unexpected keyword '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }

            let defaults_len = defaults.len();
            let default_start = total_pos.saturating_sub(defaults_len);
            for idx in 0..total_pos {
                if slots[idx].is_some() {
                    continue;
                }
                if idx >= default_start {
                    slots[idx] = Some(defaults[idx - default_start]);
                    continue;
                }
                let name = string_obj_to_owned(obj_from_bits(arg_names[idx]))
                    .unwrap_or_else(|| "?".to_string());
                if matches!(
                    std::env::var("MOLT_TRACE_CALL_BIND_MISSING")
                        .ok()
                        .as_deref(),
                    Some("1")
                ) {
                    let func_name_bits = function_name_bits(_py, func_ptr);
                    let func_name =
                        if func_name_bits == 0 || obj_from_bits(func_name_bits).is_none() {
                            "<function>".to_string()
                        } else {
                            string_obj_to_owned(obj_from_bits(func_name_bits))
                                .unwrap_or_else(|| "<function>".to_string())
                        };
                    eprintln!(
                        "molt call_bind: missing required arg func={} arg={} pos={}",
                        func_name,
                        name,
                        idx + 1
                    );
                }
                let msg = format!("missing required argument '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }

            for (kw_idx, name_bits) in kwonly_names.iter().copied().enumerate() {
                let slot_idx = kwonly_start + kw_idx;
                if slots[slot_idx].is_some() {
                    continue;
                }
                let mut default = None;
                if let Some(dict_ptr) = kwdefaults_ptr {
                    default = dict_get_in_place(_py, dict_ptr, name_bits);
                }
                if let Some(val) = default {
                    slots[slot_idx] = Some(val);
                    continue;
                }
                let name = string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "?".to_string());
                let msg = format!("missing required keyword-only argument '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }

            if has_vararg {
                let tuple_ptr = alloc_tuple(_py, extra_pos.as_slice());
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                slots[total_pos] = Some(MoltObject::from_ptr(tuple_ptr).bits());
            }

            if has_varkw {
                let dict_ptr = alloc_dict_with_pairs(_py, extra_kwargs.as_slice());
                if dict_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let varkw_idx = kwonly_start + kwonly_names.len();
                slots[varkw_idx] = Some(MoltObject::from_ptr(dict_ptr).bits());
            }

            let mut final_args: Vec<u64> = Vec::with_capacity(slots.len());
            for slot in slots {
                let Some(val) = slot else {
                    return raise_exception::<_>(_py, "TypeError", "call binding failed");
                };
                final_args.push(val);
            }
            let is_gen = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_is_generator,
                    b"__molt_is_generator__",
                ),
            )
            .is_some_and(|bits| is_truthy(_py, obj_from_bits(bits)));
            if is_gen {
                let size_bits = function_attr_bits(
                    _py,
                    func_ptr,
                    intern_static_name(
                        _py,
                        &runtime_state(_py).interned.molt_closure_size,
                        b"__molt_closure_size__",
                    ),
                )
                .unwrap_or_else(|| MoltObject::none().bits());
                let Some(size_val) = obj_from_bits(size_bits).as_int() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if size_val < 0 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "closure size must be non-negative",
                    );
                }
                let closure_size = size_val as usize;
                let fn_ptr = function_fn_ptr(func_ptr);
                let closure_bits = function_closure_bits(func_ptr);
                let mut payload: Vec<u64> =
                    Vec::with_capacity(final_args.len() + if closure_bits != 0 { 1 } else { 0 });
                if closure_bits != 0 {
                    payload.push(closure_bits);
                }
                payload.extend(final_args.iter().copied());
                let base = GEN_CONTROL_SIZE;
                let needed = base + payload.len() * std::mem::size_of::<u64>();
                if closure_size < needed {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                let obj_bits = molt_generator_new(fn_ptr, closure_size as u64);
                let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
                    return MoltObject::none().bits();
                };
                let mut offset = base;
                for val_bits in payload {
                    let slot = obj_ptr.add(offset) as *mut u64;
                    *slot = val_bits;
                    inc_ref_bits(_py, val_bits);
                    offset += std::mem::size_of::<u64>();
                }
                return obj_bits;
            }
            let result = call_function_obj_vec(_py, func_bits, final_args.as_slice());
            protect_callargs_aliased_return(_py, result, args_ptr)
        }
    })
}

unsafe fn bind_builtin_call(
    _py: &PyToken<'_>,
    func_bits: u64,
    func_ptr: *mut u8,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    unsafe {
        let fn_ptr = function_fn_ptr(func_ptr);
        if fn_ptr == fn_addr!(crate::builtins::exceptions::molt_exception_init)
            || fn_ptr == fn_addr!(crate::builtins::exceptions::molt_exception_new_bound)
        {
            return bind_builtin_exception_args(_py, args);
        }
        if callable_matches_runtime_symbol(
            Some(MoltObject::from_ptr(func_ptr).bits()),
            fn_addr!(molt_object_init),
        ) || callable_matches_runtime_symbol(
            Some(MoltObject::from_ptr(func_ptr).bits()),
            fn_addr!(molt_object_init_subclass),
        ) {
            let self_bits = args
                .pos
                .first()
                .copied()
                .unwrap_or_else(|| MoltObject::none().bits());
            return Some(vec![self_bits]);
        }
        if callable_matches_runtime_symbol(
            Some(MoltObject::from_ptr(func_ptr).bits()),
            fn_addr!(molt_object_new_bound),
        ) {
            let self_bits = args
                .pos
                .first()
                .copied()
                .unwrap_or_else(|| MoltObject::none().bits());
            return Some(vec![self_bits]);
        }
        if fn_ptr == fn_addr!(molt_int_new) {
            return bind_builtin_int_new(_py, args);
        }
        if fn_ptr == fn_addr!(molt_int_to_bytes) {
            return bind_builtin_int_bytes_codec(_py, args, "length", "byteorder");
        }
        if fn_ptr == fn_addr!(molt_int_from_bytes) {
            return bind_builtin_int_bytes_codec(_py, args, "bytes", "byteorder");
        }
        if fn_ptr == fn_addr!(molt_open_builtin) {
            return bind_builtin_open(_py, args);
        }
        if fn_ptr == fn_addr!(crate::object::ops_builtins::molt_print_builtin) {
            return bind_builtin_print(_py, args);
        }
        if fn_ptr == fn_addr!(molt_type_new) || fn_ptr == fn_addr!(molt_type_init) {
            if matches!(
                std::env::var("MOLT_TRACE_TYPE_NEW_INIT").ok().as_deref(),
                Some("1")
            ) {
                let kind = if fn_ptr == fn_addr!(molt_type_new) {
                    "type.__new__"
                } else {
                    "type.__init__"
                };
                let self_bits = args.pos.first().copied().unwrap_or(0);
                let mut meta_label = "<unknown>".to_string();
                let self_label = if let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() {
                    let self_type_id = object_type_id(self_ptr);
                    if self_type_id == TYPE_ID_TYPE {
                        let label = string_obj_to_owned(obj_from_bits(class_name_bits(self_ptr)))
                            .unwrap_or_else(|| "<type>".to_string());
                        let meta_bits = object_class_bits(self_ptr);
                        if meta_bits != 0
                            && let Some(meta_ptr) = obj_from_bits(meta_bits).as_ptr()
                            && object_type_id(meta_ptr) == TYPE_ID_TYPE
                        {
                            meta_label =
                                string_obj_to_owned(obj_from_bits(class_name_bits(meta_ptr)))
                                    .unwrap_or_else(|| "<meta>".to_string());
                        }
                        label
                    } else {
                        format!("<type_id={self_type_id}>")
                    }
                } else {
                    type_name(_py, obj_from_bits(self_bits)).to_string()
                };
                eprintln!(
                    "molt bind: {} self={} meta={} pos_len={} kw_len={}",
                    kind,
                    self_label,
                    meta_label,
                    args.pos.len(),
                    args.kw_names.len(),
                );
                if matches!(
                    std::env::var("MOLT_TRACE_TYPE_NEW_INIT_BT").ok().as_deref(),
                    Some("1")
                ) {
                    eprintln!("{:?}", std::backtrace::Backtrace::force_capture());
                }
            }
            return bind_builtin_type_new_init(_py, args);
        }
        if fn_ptr == fn_addr!(dict_get_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["key", "default"],
                Some(MoltObject::none().bits()),
                None,
            );
        }
        if fn_ptr == fn_addr!(dict_setdefault_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["key", "default"],
                Some(MoltObject::none().bits()),
                None,
            );
        }
        if fn_ptr == fn_addr!(dict_fromkeys_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["iterable", "value"],
                Some(MoltObject::none().bits()),
                None,
            );
        }
        if fn_ptr == fn_addr!(dict_update_method) {
            return bind_builtin_keywords(_py, args, &["other"], Some(missing_bits(_py)), None);
        }
        if fn_ptr == fn_addr!(dict_pop_method) {
            return bind_builtin_pop(_py, args);
        }
        if fn_ptr == fn_addr!(molt_list_sort) {
            return bind_builtin_list_sort(_py, args);
        }
        if fn_ptr == fn_addr!(molt_list_pop) {
            return bind_builtin_list_pop(_py, args);
        }
        if fn_ptr == fn_addr!(molt_bytearray_pop) {
            return bind_builtin_list_pop(_py, args);
        }
        if fn_ptr == fn_addr!(molt_list_index_range) || fn_ptr == fn_addr!(molt_tuple_index_range) {
            return bind_builtin_list_index_range(_py, args);
        }
        if fn_ptr == fn_addr!(molt_string_find_slice) {
            return bind_builtin_string_find(_py, args, "find");
        }
        if fn_ptr == fn_addr!(molt_string_rfind_slice) {
            return bind_builtin_string_find(_py, args, "rfind");
        }
        if fn_ptr == fn_addr!(molt_string_index_slice)
            || fn_ptr == fn_addr!(molt_bytes_index_slice)
            || fn_ptr == fn_addr!(molt_bytearray_index_slice)
        {
            return bind_builtin_string_find(_py, args, "index");
        }
        if fn_ptr == fn_addr!(molt_string_rindex_slice)
            || fn_ptr == fn_addr!(molt_bytes_rindex_slice)
            || fn_ptr == fn_addr!(molt_bytearray_rindex_slice)
        {
            return bind_builtin_string_find(_py, args, "rindex");
        }
        if fn_ptr == fn_addr!(molt_bytes_find_slice)
            || fn_ptr == fn_addr!(molt_bytearray_find_slice)
        {
            return bind_builtin_string_find(_py, args, "find");
        }
        if fn_ptr == fn_addr!(molt_bytes_rfind_slice)
            || fn_ptr == fn_addr!(molt_bytearray_rfind_slice)
        {
            return bind_builtin_string_find(_py, args, "rfind");
        }
        if fn_ptr == fn_addr!(molt_string_split_max)
            || fn_ptr == fn_addr!(molt_bytes_split_max)
            || fn_ptr == fn_addr!(molt_bytearray_split_max)
        {
            return bind_builtin_split(_py, args, "split");
        }
        if fn_ptr == fn_addr!(molt_string_rsplit_max)
            || fn_ptr == fn_addr!(molt_bytes_rsplit_max)
            || fn_ptr == fn_addr!(molt_bytearray_rsplit_max)
        {
            return bind_builtin_split(_py, args, "rsplit");
        }
        if fn_ptr == fn_addr!(molt_string_count_slice)
            || fn_ptr == fn_addr!(molt_bytes_count_slice)
            || fn_ptr == fn_addr!(molt_bytearray_count_slice)
        {
            return bind_builtin_count(_py, args, "count");
        }
        if fn_ptr == fn_addr!(molt_string_startswith_slice) {
            return bind_builtin_prefix_check(_py, args, "startswith", "prefix");
        }
        if fn_ptr == fn_addr!(molt_string_endswith_slice) {
            return bind_builtin_prefix_check(_py, args, "endswith", "suffix");
        }
        if fn_ptr == fn_addr!(molt_bytes_startswith_slice)
            || fn_ptr == fn_addr!(molt_bytearray_startswith_slice)
        {
            return bind_builtin_prefix_check(_py, args, "startswith", "prefix");
        }
        if fn_ptr == fn_addr!(molt_bytes_endswith_slice)
            || fn_ptr == fn_addr!(molt_bytearray_endswith_slice)
        {
            return bind_builtin_prefix_check(_py, args, "endswith", "suffix");
        }
        if fn_ptr == fn_addr!(molt_bytes_hex)
            || fn_ptr == fn_addr!(molt_bytearray_hex)
            || fn_ptr == fn_addr!(molt_memoryview_hex)
        {
            return bind_builtin_bytes_hex(_py, args);
        }
        if fn_ptr == fn_addr!(molt_string_format_method) {
            return bind_builtin_string_format(_py, args);
        }
        if fn_ptr == fn_addr!(molt_string_splitlines)
            || fn_ptr == fn_addr!(molt_bytes_splitlines)
            || fn_ptr == fn_addr!(molt_bytearray_splitlines)
        {
            return bind_builtin_splitlines(_py, args);
        }
        if fn_ptr == fn_addr!(molt_set_union_multi) {
            return bind_builtin_set_multi(_py, args, "union", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_union_multi) {
            return bind_builtin_set_multi(_py, args, "union", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_intersection_multi) {
            return bind_builtin_set_multi(_py, args, "intersection", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_intersection_multi) {
            return bind_builtin_set_multi(
                _py,
                args,
                "intersection",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_difference_multi) {
            return bind_builtin_set_multi(_py, args, "difference", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_difference_multi) {
            return bind_builtin_set_multi(_py, args, "difference", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_update_multi) {
            return bind_builtin_set_multi(_py, args, "update", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_set_intersection_update_multi) {
            return bind_builtin_set_multi(_py, args, "intersection_update", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_set_difference_update_multi) {
            return bind_builtin_set_multi(_py, args, "difference_update", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_set_symmetric_difference) {
            return bind_builtin_set_single(_py, args, "symmetric_difference", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_symmetric_difference) {
            return bind_builtin_set_single(
                _py,
                args,
                "symmetric_difference",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_symmetric_difference_update) {
            return bind_builtin_set_single(
                _py,
                args,
                "symmetric_difference_update",
                "set",
                TYPE_ID_SET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_isdisjoint) {
            return bind_builtin_set_single(_py, args, "isdisjoint", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_isdisjoint) {
            return bind_builtin_set_single(
                _py,
                args,
                "isdisjoint",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_issubset) {
            return bind_builtin_set_single(_py, args, "issubset", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_issubset) {
            return bind_builtin_set_single(_py, args, "issubset", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_issuperset) {
            return bind_builtin_set_single(_py, args, "issuperset", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_issuperset) {
            return bind_builtin_set_single(
                _py,
                args,
                "issuperset",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_copy_method) {
            return bind_builtin_set_noargs(_py, args, "copy", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_copy_method) {
            return bind_builtin_set_noargs(_py, args, "copy", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_clear) {
            return bind_builtin_set_noargs(_py, args, "clear", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_string_encode) {
            return bind_builtin_text_codec(_py, args, "encode");
        }
        if fn_ptr == fn_addr!(molt_bytes_decode) || fn_ptr == fn_addr!(molt_bytearray_decode) {
            return bind_builtin_text_codec(_py, args, "decode");
        }
        if fn_ptr == fn_addr!(molt_memoryview_cast) {
            return bind_builtin_memoryview_cast(_py, args);
        }
        if fn_ptr == fn_addr!(molt_file_reconfigure) {
            return bind_builtin_file_reconfigure(_py, args);
        }

        if !args.kw_names.is_empty() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "keywords are not supported for this builtin",
            );
        }

        let mut out = args.pos.clone();
        let arity = function_arity(func_ptr) as usize;
        if fn_ptr == fn_addr!(molt_bytes_maketrans) && out.len() != 2 {
            let msg = format!("maketrans expected 2 arguments, got {}", out.len());
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if out.len() > arity {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        let missing = arity - out.len();
        if missing == 0 {
            return Some(out);
        }
        // Consult __defaults__ tuple stored on the function.
        // This handles user-defined functions with keyword default arguments
        // that end up in the builtin bind path (e.g. on WASM when
        // __molt_arg_names__ is not found on the function object).
        let defaults_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.defaults_name,
                b"__defaults__",
            ),
        );
        if let Some(dbits) = defaults_bits
            && !obj_from_bits(dbits).is_none()
            && let Some(def_ptr) = obj_from_bits(dbits).as_ptr()
            && object_type_id(def_ptr) == TYPE_ID_TUPLE
        {
            let defaults = seq_vec_ref(def_ptr);
            let n_defaults = defaults.len();
            if missing <= n_defaults {
                // The defaults tuple covers the last n_defaults
                // parameters.  We need the last `missing` entries.
                let start = n_defaults - missing;
                out.extend(defaults.iter().take(n_defaults).skip(start).copied());
                return Some(out);
            }
        }

        // Diagnostic: log what function failed to bind
        if std::env::var("MOLT_DEBUG_BIND").is_ok() {
            let func_name = crate::type_name(_py, molt_obj_model::MoltObject::from_bits(func_bits));
            eprintln!(
                "[bind] missing required arguments: func={} pos_given={} missing={}",
                func_name,
                args.pos.len(),
                missing,
            );
        }
        raise_exception::<_>(_py, "TypeError", "missing required arguments")
    }
}

unsafe fn bind_builtin_exception_args(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required arguments");
        }
        if !args.kw_names.is_empty() {
            let head = args.pos[0];
            let head_obj = obj_from_bits(head);
            let Some(head_ptr) = head_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "keywords are not supported for this builtin",
                );
            };
            let allow_kw = match object_type_id(head_ptr) {
                TYPE_ID_TYPE => true,
                TYPE_ID_EXCEPTION => {
                    let oserror_bits = exception_type_bits_from_name(_py, "OSError");
                    issubclass_bits(exception_class_bits(head_ptr), oserror_bits)
                }
                _ => false,
            };
            if !allow_kw {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "keywords are not supported for this builtin",
                );
            }
        }
        let head = args.pos[0];
        let rest = &args.pos[1..];
        let tuple_ptr = alloc_tuple(_py, rest);
        if tuple_ptr.is_null() {
            return None;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        Some(vec![head, tuple_bits])
    }
}

unsafe fn bind_builtin_int_new(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'cls'");
    }
    if args.pos.len() > 3 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let cls_bits = args.pos[0];
    let mut value_bits = args.pos.get(1).copied();
    let mut base_bits = args.pos.get(2).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "x" => {
                if value_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                value_bits = Some(val_bits);
            }
            "base" => {
                if base_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                base_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let value_bits = value_bits.unwrap_or_else(|| MoltObject::from_int(0).bits());
    let base_bits = base_bits.unwrap_or_else(|| missing_bits(_py));
    Some(vec![cls_bits, value_bits, base_bits])
}

unsafe fn bind_builtin_dict_update(_py: &PyToken<'_>, args: &CallArgs) -> u64 {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let positional = args.pos.len().saturating_sub(1);
        if positional > 1 {
            let msg = format!("update expected at most 1 argument, got {}", positional);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let dict_bits = args.pos[0];
        if positional == 1 {
            let other_bits = args.pos[1];
            let dict_obj = obj_from_bits(dict_bits);
            if let Some(dict_ptr) = dict_obj.as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, other_bits);
                } else {
                    let _ =
                        dict_update_apply(_py, dict_bits, dict_update_set_via_store, other_bits);
                }
            } else {
                let _ = dict_update_apply(_py, dict_bits, dict_update_set_via_store, other_bits);
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        if !args.kw_names.is_empty() {
            for (name_bits, val_bits) in args
                .kw_names
                .iter()
                .copied()
                .zip(args.kw_values.iter().copied())
            {
                let name_obj = obj_from_bits(name_bits);
                let Some(name_ptr) = name_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                };
                if object_type_id(name_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                }
                dict_update_set_via_store(_py, dict_bits, name_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    }
}

fn default_open_mode_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.open_default_mode,
        || {
            let ptr = alloc_string(_py, b"r");
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        },
    )
}

unsafe fn bind_builtin_bytes_hex(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 3 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let self_bits = args.pos[0];
    let mut sep_bits = args.pos.get(1).copied();
    let mut bytes_per_sep_bits = args.pos.get(2).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "sep" => {
                if sep_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                sep_bits = Some(val_bits);
            }
            "bytes_per_sep" => {
                if bytes_per_sep_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                bytes_per_sep_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let sep_bits = sep_bits.unwrap_or_else(|| missing_bits(_py));
    let bytes_per_sep_bits = bytes_per_sep_bits.unwrap_or_else(|| missing_bits(_py));
    Some(vec![self_bits, sep_bits, bytes_per_sep_bits])
}

unsafe fn bind_builtin_keywords(
    _py: &PyToken<'_>,
    args: &CallArgs,
    names: &[&str],
    default_bits: Option<u64>,
    extra_bits: Option<u64>,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut values: Vec<Option<u64>> = vec![None; names.len()];
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        let idx = pos_idx - 1;
        if idx >= names.len() {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        values[idx] = Some(args.pos[pos_idx]);
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in names.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    for (idx, val) in values.iter_mut().enumerate() {
        if val.is_none() {
            if let Some(bits) = default_bits {
                *val = Some(bits);
                continue;
            }
            let name = names[idx];
            let msg = format!("missing required argument '{name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    for val in values.into_iter().flatten() {
        out.push(val);
    }
    if let Some(extra) = extra_bits {
        out.push(extra);
    }
    Some(out)
}

unsafe fn bind_builtin_int_bytes_codec(
    _py: &PyToken<'_>,
    args: &CallArgs,
    required_0: &str,
    required_1: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 4 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut first_bits = args.pos.get(1).copied();
    let mut second_bits = args.pos.get(2).copied();
    let mut signed_bits = args.pos.get(3).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            _ if name_str == required_0 => {
                if first_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                first_bits = Some(val_bits);
            }
            _ if name_str == required_1 => {
                if second_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                second_bits = Some(val_bits);
            }
            "signed" => {
                if signed_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                signed_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let Some(first_bits) = first_bits else {
        let msg = format!("missing required argument '{required_0}'");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    let Some(second_bits) = second_bits else {
        let msg = format!("missing required argument '{required_1}'");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    let signed_bits = signed_bits.unwrap_or_else(|| MoltObject::from_bool(false).bits());
    Some(vec![args.pos[0], first_bits, second_bits, signed_bits])
}

unsafe fn bind_builtin_class_text_io_wrapper(
    _py: &PyToken<'_>,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    const NAMES: [&str; 6] = [
        "buffer",
        "encoding",
        "errors",
        "newline",
        "line_buffering",
        "write_through",
    ];
    if args.pos.len() > NAMES.len() {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut values: Vec<Option<u64>> = vec![None; NAMES.len()];
    for (idx, &val) in args.pos.iter().enumerate() {
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'buffer'");
    }
    for slot in values.iter_mut().take(4).skip(1) {
        if slot.is_none() {
            *slot = Some(MoltObject::none().bits());
        }
    }
    for slot in values.iter_mut().take(6).skip(4) {
        if slot.is_none() {
            *slot = Some(MoltObject::from_bool(false).bits());
        }
    }
    Some(values.into_iter().flatten().collect())
}

unsafe fn bind_builtin_class_string_io(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    const NAMES: [&str; 2] = ["initial_value", "newline"];
    if args.pos.len() > NAMES.len() {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut values: [Option<u64>; 2] = [None; 2];
    for (idx, &val) in args.pos.iter().enumerate() {
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        values[0] = Some(MoltObject::none().bits());
    }
    if values[1].is_none() {
        values[1] = Some(MoltObject::none().bits());
    }
    Some(values.into_iter().flatten().collect())
}

/// Bind `print(*args, sep=' ', end='\n', file=None, flush=False)`.
///
/// `molt_print_builtin` takes 5 positional C params:
///   (args_tuple, sep, end, file, flush)
/// The first param is a tuple of the `*args` vararg.
unsafe fn bind_builtin_print(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    // Build the *args tuple from positional arguments.
    let args_ptr = crate::object::builders::alloc_tuple(_py, &args.pos);
    let args_tuple = MoltObject::from_ptr(args_ptr).bits();
    // Keyword-only defaults.
    let default_sep = crate::object::builders::alloc_string(_py, b" ");
    let default_end = crate::object::builders::alloc_string(_py, b"\n");
    let sep_default = MoltObject::from_ptr(default_sep).bits();
    let end_default = MoltObject::from_ptr(default_end).bits();
    let file_default = MoltObject::none().bits();
    let flush_default = MoltObject::from_bool(false).bits();
    let mut sep = sep_default;
    let mut end = end_default;
    let mut file = file_default;
    let mut flush = flush_default;
    // Match keywords.
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_default();
        match name_str.as_str() {
            "sep" => sep = val_bits,
            "end" => end = val_bits,
            "file" => file = val_bits,
            "flush" => flush = val_bits,
            _ => {
                let msg = format!("'{}' is an invalid keyword argument for print", name_str);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args_tuple, sep, end, file, flush])
}

unsafe fn bind_builtin_open(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    const NAMES: [&str; 8] = [
        "file",
        "mode",
        "buffering",
        "encoding",
        "errors",
        "newline",
        "closefd",
        "opener",
    ];
    let mut values: [Option<u64>; 8] = [None; 8];
    for (idx, val) in args.pos.iter().copied().enumerate() {
        if idx >= values.len() {
            let msg = format!(
                "open() takes at most 8 arguments ({} given)",
                args.pos.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = if idx < args.pos.len() {
                        format!(
                            "argument for open() given by name ('{name_str}') and position ({})",
                            idx + 1
                        )
                    } else {
                        format!("open() got multiple values for argument '{name_str}'")
                    };
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("open() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "open() missing required argument 'file' (pos 1)",
        );
    }
    if values[1].is_none() {
        let mode_bits = default_open_mode_bits(_py);
        if obj_from_bits(mode_bits).is_none() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        values[1] = Some(mode_bits);
    }
    if values[2].is_none() {
        values[2] = Some(MoltObject::from_int(-1).bits());
    }
    if values[3].is_none() {
        values[3] = Some(MoltObject::none().bits());
    }
    if values[4].is_none() {
        values[4] = Some(MoltObject::none().bits());
    }
    if values[5].is_none() {
        values[5] = Some(MoltObject::none().bits());
    }
    if values[6].is_none() {
        values[6] = Some(MoltObject::from_bool(true).bits());
    }
    if values[7].is_none() {
        values[7] = Some(MoltObject::none().bits());
    }
    let mut out = Vec::with_capacity(values.len());
    for val in values {
        out.push(val.unwrap());
    }
    Some(out)
}

unsafe fn bind_builtin_type_new_init(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'cls'");
        }
        let mut values: [Option<u64>; 3] = [None, None, None];
        for (idx, val) in args.pos.iter().copied().enumerate().skip(1) {
            let pos_idx = idx - 1;
            if pos_idx >= values.len() {
                return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
            }
            values[pos_idx] = Some(val);
        }
        let mut extra_pairs: Vec<u64> = Vec::new();
        for (name_bits, val_bits) in args
            .kw_names
            .iter()
            .copied()
            .zip(args.kw_values.iter().copied())
        {
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            }
            let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
            let slot = match name_str.as_str() {
                "name" => Some(0usize),
                "bases" => Some(1usize),
                "dict" | "namespace" => Some(2usize),
                _ => None,
            };
            if let Some(idx) = slot {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
            } else {
                extra_pairs.push(name_bits);
                extra_pairs.push(val_bits);
            }
        }
        let names = ["name", "bases", "dict"];
        for (idx, val) in values.iter().enumerate() {
            if val.is_none() {
                if matches!(
                    std::env::var("MOLT_TRACE_TYPE_NEW_INIT").ok().as_deref(),
                    Some("1")
                ) {
                    eprintln!(
                        "molt bind: type.__new__/__init__ missing {} (pos_len={} kw_len={})",
                        names[idx],
                        args.pos.len(),
                        args.kw_names.len(),
                    );
                }
                let msg = format!("missing required argument '{}'", names[idx]);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let mut out = vec![args.pos[0]];
        for val in values.into_iter().flatten() {
            out.push(val);
        }
        if extra_pairs.is_empty() {
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &extra_pairs);
        if dict_ptr.is_null() {
            return Some(out);
        }
        out.push(MoltObject::from_ptr(dict_ptr).bits());
        Some(out)
    }
}

unsafe fn bind_builtin_list_sort(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut key_bits = MoltObject::none().bits();
    let mut reverse_bits = MoltObject::from_bool(false).bits();
    let mut key_set = false;
    let mut reverse_set = false;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "key" => {
                if key_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                key_bits = val_bits;
                key_set = true;
            }
            "reverse" => {
                if reverse_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                reverse_bits = val_bits;
                reverse_set = true;
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], key_bits, reverse_bits])
}

unsafe fn bind_builtin_list_pop(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }
    if args.pos.len() > 2 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut out = args.pos.clone();
    if out.len() == 1 {
        out.push(MoltObject::none().bits());
    }
    Some(out)
}

unsafe fn bind_builtin_list_index_range(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }
    if args.pos.len() < 2 {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'value'");
    }
    if args.pos.len() > 4 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut out = args.pos.clone();
    let missing = missing_bits(_py);
    if out.len() == 2 {
        out.push(missing);
        out.push(missing);
    } else if out.len() == 3 {
        out.push(missing);
    }
    Some(out)
}

unsafe fn bind_builtin_string_find(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_sub = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_sub = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sub" => (&mut needle_bits, &mut saw_sub),
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        raise_exception::<_>(_py, "TypeError", "missing required argument 'sub'")
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_count(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_sub = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_sub = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sub" => (&mut needle_bits, &mut saw_sub),
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        raise_exception::<_>(_py, "TypeError", "missing required argument 'sub'")
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_split(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 2 {
        let msg = format!(
            "{func_name}() takes at most 2 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut sep_bits: Option<u64> = None;
    let mut maxsplit_bits: Option<u64> = None;
    let mut saw_sep = false;
    let mut saw_maxsplit = false;
    if positional >= 1 {
        sep_bits = Some(args.pos[1]);
        saw_sep = true;
    }
    if positional >= 2 {
        maxsplit_bits = Some(args.pos[2]);
        saw_maxsplit = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sep" => (&mut sep_bits, &mut saw_sep),
            "maxsplit" => (&mut maxsplit_bits, &mut saw_maxsplit),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let sep_bits = sep_bits.unwrap_or_else(|| MoltObject::none().bits());
    let maxsplit_bits = maxsplit_bits.unwrap_or_else(|| MoltObject::from_int(-1).bits());
    Some(vec![args.pos[0], sep_bits, maxsplit_bits])
}

unsafe fn bind_builtin_splitlines(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 1 {
        let msg = format!(
            "splitlines() takes at most 1 argument ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut keepends_bits: Option<u64> = None;
    let mut saw_keepends = false;
    if positional == 1 {
        keepends_bits = Some(args.pos[1]);
        saw_keepends = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str != "keepends" {
            let msg = format!("splitlines() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if saw_keepends {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "splitlines() got multiple values for argument 'keepends'",
            );
        }
        keepends_bits = Some(val_bits);
        saw_keepends = true;
    }
    let keepends_bits = keepends_bits.unwrap_or_else(|| MoltObject::from_bool(false).bits());
    Some(vec![args.pos[0], keepends_bits])
}

unsafe fn bind_builtin_set_multi(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let self_obj = obj_from_bits(args.pos[0]);
        let mut is_owner = false;
        if let Some(self_ptr) = self_obj.as_ptr() {
            is_owner = object_type_id(self_ptr) == owner_type_id;
        }
        if !is_owner {
            let msg = format!(
                "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if !args.kw_names.is_empty() {
            let msg = format!(
                "{}.{method}() takes no keyword arguments",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let tuple_ptr = alloc_tuple(_py, &args.pos[1..]);
        if tuple_ptr.is_null() {
            return None;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        Some(vec![args.pos[0], tuple_bits])
    }
}

unsafe fn bind_builtin_set_single(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let self_obj = obj_from_bits(args.pos[0]);
        let mut is_owner = false;
        if let Some(self_ptr) = self_obj.as_ptr() {
            is_owner = object_type_id(self_ptr) == owner_type_id;
        }
        if !is_owner {
            let msg = format!(
                "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if !args.kw_names.is_empty() {
            let msg = format!(
                "{}.{method}() takes no keyword arguments",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let positional = args.pos.len().saturating_sub(1);
        if positional != 1 {
            let msg = format!(
                "{}.{method}() takes exactly one argument ({} given)",
                type_name(_py, self_obj),
                positional
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        Some(vec![args.pos[0], args.pos[1]])
    }
}

unsafe fn bind_builtin_set_noargs(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let self_obj = obj_from_bits(args.pos[0]);
        let mut is_owner = false;
        if let Some(self_ptr) = self_obj.as_ptr() {
            is_owner = object_type_id(self_ptr) == owner_type_id;
        }
        if !is_owner {
            let msg = format!(
                "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if !args.kw_names.is_empty() {
            let msg = format!(
                "{}.{method}() takes no keyword arguments",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let positional = args.pos.len().saturating_sub(1);
        if positional != 0 {
            let msg = format!(
                "{}.{method}() takes no arguments ({} given)",
                type_name(_py, self_obj),
                positional
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        Some(vec![args.pos[0]])
    }
}

unsafe fn bind_builtin_prefix_check(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
    needle_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_needle = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_needle = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ if name_str == needle_name => (&mut needle_bits, &mut saw_needle),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        let msg = format!("missing required argument '{needle_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_string_format(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let tuple_ptr = alloc_tuple(_py, &args.pos[1..]);
        if tuple_ptr.is_null() {
            return None;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let mut pairs = Vec::with_capacity(args.kw_names.len() * 2);
        for (name_bits, val_bits) in args
            .kw_names
            .iter()
            .copied()
            .zip(args.kw_values.iter().copied())
        {
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            }
            pairs.push(name_bits);
            pairs.push(val_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            return None;
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        Some(vec![args.pos[0], tuple_bits, dict_bits])
    }
}

unsafe fn bind_builtin_memoryview_cast(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let provided = args.pos.len().saturating_sub(1);
    if provided == 0 {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "cast() missing required argument 'format' (pos 1)",
        );
    }
    if provided > 2 {
        let msg = format!("cast() takes at most 2 arguments ({provided} given)");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let format_bits = args.pos[1];
    let mut shape_bits: Option<u64> = None;
    if provided == 2 {
        shape_bits = Some(args.pos[2]);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str != "shape" {
            let msg = format!("cast() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if shape_bits.is_some() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cast() got multiple values for argument 'shape'",
            );
        }
        shape_bits = Some(val_bits);
    }
    let (shape_bits, has_shape_bits) = if let Some(bits) = shape_bits {
        (bits, MoltObject::from_bool(true).bits())
    } else {
        (
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        )
    };
    Some(vec![args.pos[0], format_bits, shape_bits, has_shape_bits])
}

unsafe fn bind_builtin_file_reconfigure(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "reconfigure() takes no positional arguments",
        );
    }
    let missing = missing_bits(_py);
    let mut encoding_bits = missing;
    let mut errors_bits = missing;
    let mut newline_bits = missing;
    let mut line_buffering_bits = missing;
    let mut write_through_bits = missing;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "encoding" => {
                if encoding_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                encoding_bits = val_bits;
            }
            "errors" => {
                if errors_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                errors_bits = val_bits;
            }
            "newline" => {
                if newline_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                newline_bits = val_bits;
            }
            "line_buffering" => {
                if line_buffering_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                line_buffering_bits = val_bits;
            }
            "write_through" => {
                if write_through_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                write_through_bits = val_bits;
            }
            _ => {
                let msg = format!("'{name_str}' is an invalid keyword argument for reconfigure()");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![
        args.pos[0],
        encoding_bits,
        errors_bits,
        newline_bits,
        line_buffering_bits,
        write_through_bits,
    ])
}

unsafe fn bind_builtin_text_codec(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 2 {
        let msg = format!("{func_name}() takes at most 2 arguments ({positional} given)");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let missing = missing_bits(_py);
    let mut encoding_bits = if positional >= 1 {
        args.pos[1]
    } else {
        missing
    };
    let mut errors_bits = if positional >= 2 {
        args.pos[2]
    } else {
        missing
    };
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "encoding" => {
                if encoding_bits != missing {
                    let msg = format!("{func_name}() got multiple values for argument 'encoding'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                encoding_bits = val_bits;
            }
            "errors" => {
                if errors_bits != missing {
                    let msg = format!("{func_name}() got multiple values for argument 'errors'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                errors_bits = val_bits;
            }
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], encoding_bits, errors_bits])
}

unsafe fn bind_builtin_pop(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut key: Option<u64> = None;
    let mut default: Option<u64> = None;
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        if key.is_none() {
            key = Some(args.pos[pos_idx]);
        } else if default.is_none() {
            default = Some(args.pos[pos_idx]);
        } else {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str == "key" {
            if key.is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            key = Some(val_bits);
        } else if name_str == "default" {
            if default.is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            default = Some(val_bits);
        } else {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    let Some(key_bits) = key else {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'key'");
    };
    let (default_bits, has_default) = if let Some(bits) = default {
        (bits, MoltObject::from_int(1).bits())
    } else {
        (MoltObject::none().bits(), MoltObject::from_int(0).bits())
    };
    out.push(key_bits);
    out.push(default_bits);
    out.push(has_default);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{protect_callargs_aliased_return_with_extra, trace_call_type_builder_enabled_raw};
    use crate::object::builders::alloc_list;
    use molt_obj_model::MoltObject;
    use std::sync::atomic::Ordering;

    #[test]
    fn trace_call_type_builder_gate_requires_explicit_opt_in() {
        assert!(!trace_call_type_builder_enabled_raw(None));
        assert!(!trace_call_type_builder_enabled_raw(Some("0")));
        assert!(!trace_call_type_builder_enabled_raw(Some("true")));
        assert!(trace_call_type_builder_enabled_raw(Some("1")));
    }

    #[test]
    fn protect_aliased_return_with_extra_inc_refs_synthesized_owner() {
        crate::with_gil_entry_nopanic!(_py, {
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(1).bits()]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let before = unsafe {
                (*crate::object::header_from_obj_ptr(list_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            let protected = unsafe {
                protect_callargs_aliased_return_with_extra(
                    _py,
                    list_bits,
                    std::ptr::null_mut(),
                    &[list_bits],
                )
            };
            assert_eq!(protected, list_bits);
            let after = unsafe {
                (*crate::object::header_from_obj_ptr(list_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(after, before + 1);
            crate::dec_ref_bits(_py, list_bits);
            crate::dec_ref_bits(_py, list_bits);
        });
    }
}
