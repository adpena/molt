use crate::builtins::frames::{frame_stack_pop, frame_stack_push_function};
use crate::call::function::protect_borrowed_args_aliased_return;
use crate::call::type_policy::{
    InitArgPolicy, callable_matches_runtime_symbol, resolved_constructor_init_policy,
    resolved_new_is_default_object_new,
};
use crate::object::layout::ensure_function_code_bits;
use crate::state::recursion::{recursion_guard_enter, recursion_guard_exit};
use crate::state::tls::FRAME_STACK;
use crate::{
    ALLOC_BYTES_CALLARGS, BIND_KIND_CAPI_METHOD, BIND_KIND_OPEN, CALL_BIND_IC_HIT_COUNT,
    CALL_BIND_IC_MISS_COUNT, GEN_CONTROL_SIZE, HEADER_FLAG_FUNC_REQUIRES_BINDER,
    INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT, MoltHeader, MoltObject, PtrDropGuard, PyToken,
    TYPE_ID_BOUND_METHOD, TYPE_ID_CALLARGS, TYPE_ID_CODE, TYPE_ID_DATACLASS, TYPE_ID_DICT,
    TYPE_ID_EXCEPTION, TYPE_ID_FROZENSET, TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS, TYPE_ID_OBJECT,
    TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_class_obj,
    alloc_dict_with_pairs, alloc_exception_from_class_bits, alloc_instance_for_class,
    alloc_instance_for_default_object_new, alloc_object, alloc_object_zeroed, alloc_string,
    alloc_tuple, apply_class_slots_layout, attr_lookup_ptr, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes,
    audit::{AuditArgs, audit_capability_decision},
    bits_from_ptr, bound_method_func_bits, bound_method_self_bits, builtin_classes, call_callable0,
    call_callable1, call_class_init_with_args, call_function_obj_bound_vec, class_attr_lookup,
    class_attr_lookup_raw_mro, class_dict_bits, class_layout_version_bits, class_name_bits,
    class_name_for_error, code_argcount, code_filename_bits, code_name_bits, dec_ref_bits,
    dict_del_in_place, dict_fromkeys_method, dict_get_in_place, dict_get_method, dict_order,
    dict_setdefault_method, dict_update_apply, dict_update_method, dict_update_set_in_place,
    dict_update_set_via_store, exception_class_bits, exception_pending,
    exception_type_bits_from_name, function_arity, function_attr_bits, function_closure_bits,
    function_fn_ptr, function_name_bits, function_trampoline_ptr, generic_alias_origin_bits,
    has_capability, header_from_obj_ptr, inc_ref_bits, init_atomic_bits, intern_static_name,
    is_builtin_class_bits, is_trusted, is_truthy, isinstance_bits, issubclass_bits,
    lookup_call_attr, maybe_ptr_from_bits, missing_bits, molt_bytearray_count_slice,
    molt_bytearray_decode, molt_bytearray_endswith_slice, molt_bytearray_find_slice,
    molt_bytearray_hex, molt_bytearray_index_slice, molt_bytearray_pop, molt_bytearray_rfind_slice,
    molt_bytearray_rindex_slice, molt_bytearray_rsplit_max, molt_bytearray_split_max,
    molt_bytearray_splitlines, molt_bytearray_startswith_slice, molt_bytes_count_slice,
    molt_bytes_decode, molt_bytes_endswith_slice, molt_bytes_find_slice, molt_bytes_hex,
    molt_bytes_index_slice, molt_bytes_maketrans, molt_bytes_rfind_slice, molt_bytes_rindex_slice,
    molt_bytes_rsplit_max, molt_bytes_split_max, molt_bytes_splitlines,
    molt_bytes_startswith_slice, molt_class_set_base, molt_dict_from_obj, molt_dict_new,
    molt_dict_pop_method, molt_file_reconfigure, molt_frozenset_copy_method,
    molt_frozenset_difference_multi, molt_frozenset_intersection_multi, molt_frozenset_isdisjoint,
    molt_frozenset_issubset, molt_frozenset_issuperset, molt_frozenset_symmetric_difference,
    molt_frozenset_union_multi, molt_generator_new, molt_int_from_bytes, molt_int_new,
    molt_int_to_bytes, molt_iter, molt_iter_next, molt_list_append, molt_list_index_range,
    molt_list_pop, molt_list_sort, molt_memoryview_cast, molt_memoryview_hex, molt_object_init,
    molt_object_init_subclass, molt_object_new_bound, molt_open_builtin, molt_set_clear,
    molt_set_copy_method, molt_set_difference_multi, molt_set_difference_update_multi,
    molt_set_intersection_multi, molt_set_intersection_update_multi, molt_set_isdisjoint,
    molt_set_issubset, molt_set_issuperset, molt_set_symmetric_difference,
    molt_set_symmetric_difference_update, molt_set_union_multi, molt_set_update_multi,
    molt_string_count_slice, molt_string_encode, molt_string_endswith_slice,
    molt_string_find_slice, molt_string_format_method, molt_string_index_slice,
    molt_string_rfind_slice, molt_string_rindex_slice, molt_string_rsplit_max,
    molt_string_split_max, molt_string_splitlines, molt_string_startswith_slice, molt_super_new,
    molt_tuple_index_range, molt_type_call, molt_type_init, molt_type_new, obj_from_bits,
    object_class_bits, object_set_class_bits, object_type_id, profile_hit_unchecked, ptr_from_bits,
    raise_exception, raise_not_callable, raise_not_iterable, runtime_state, runtime_state_for_gil,
    seq_vec_ref, string_obj_to_owned, type_name, type_of_bits,
};
use std::collections::{HashMap, HashSet};
use std::sync::{MutexGuard, OnceLock};

mod builtin_args;
mod inline_cache;
use inline_cache::{call_bind_ic_entry_for_call, try_call_bind_ic_fast};
pub(crate) use inline_cache::{
    clear_call_bind_ic_cache, clear_method_ic_cache, clear_super_ic_cache,
};
#[allow(unused_imports)]
pub use inline_cache::{
    molt_call_bind_ic, molt_call_indirect_ic, molt_call_method_ic0, molt_call_method_ic1,
    molt_call_method_ic2, molt_call_method_ic3, molt_call_method_ic4, molt_call_super_method_ic0,
    molt_call_super_method_ic1, molt_call_super_method_ic2, molt_call_super_method_ic3,
    molt_call_super_method_ic4, molt_invoke_ffi_ic,
};
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
    *MODE.get_or_init(
        || match std::env::var("MOLT_TRACE_CALL_BIND").ok().as_deref() {
            Some("all" | "verbose") => TraceCallBindMode::Verbose,
            Some("1") => TraceCallBindMode::Basic,
            _ => TraceCallBindMode::Off,
        },
    )
}

#[derive(Copy, Clone)]
struct CallArgsPtr(*mut CallArgs);

// CallArgs allocations are owned by the runtime object they are attached to
// and protected by the GIL-like runtime lock. The registry only preserves
// pointer provenance for lookups from object payload addresses.
unsafe impl Send for CallArgsPtr {}
unsafe impl Sync for CallArgsPtr {}

pub(crate) struct CallBindRuntimeState {
    callargs_builder_map: HashMap<usize, CallArgsPtr>,
    callargs_storage_registry: HashSet<usize>,
}

impl CallBindRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            callargs_builder_map: HashMap::new(),
            callargs_storage_registry: HashSet::new(),
        }
    }
}

fn call_bind_runtime_state(_py: &PyToken<'_>) -> MutexGuard<'static, CallBindRuntimeState> {
    runtime_state(_py)
        .call_bind
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn call_bind_runtime_state_if_available() -> Option<MutexGuard<'static, CallBindRuntimeState>> {
    runtime_state_for_gil().map(|state| {
        state
            .call_bind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    })
}

pub(crate) fn note_callargs_alloc(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    args_ptr: *mut CallArgs,
) {
    let mut state = call_bind_runtime_state(_py);
    if !builder_ptr.is_null() {
        state
            .callargs_builder_map
            .insert(builder_ptr as usize, CallArgsPtr(args_ptr));
    }
    if args_ptr.is_null() {
        return;
    }
    state.callargs_storage_registry.insert(args_ptr as usize);
}

pub(crate) fn note_callargs_free(_py: &PyToken<'_>, builder_ptr: *mut u8, args_ptr: *mut CallArgs) {
    if trace_callargs_enabled() && !builder_ptr.is_null() {
        eprintln!(
            "[molt callargs] free builder_ptr=0x{:x} args_ptr=0x{:x}",
            builder_ptr as usize, args_ptr as usize,
        );
    }
    let mut state = call_bind_runtime_state(_py);
    if !builder_ptr.is_null() {
        state.callargs_builder_map.remove(&(builder_ptr as usize));
    }
    if args_ptr.is_null() {
        return;
    }
    state.callargs_storage_registry.remove(&(args_ptr as usize));
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

fn callargs_builder_is_live(_py: &PyToken<'_>, builder_ptr: *mut u8) -> bool {
    if builder_ptr.is_null() {
        return false;
    }
    call_bind_runtime_state(_py)
        .callargs_builder_map
        .contains_key(&(builder_ptr as usize))
}

fn callargs_storage_is_live(_py: &PyToken<'_>, args_ptr: *mut CallArgs) -> bool {
    if args_ptr.is_null() {
        return false;
    }
    call_bind_runtime_state(_py)
        .callargs_storage_registry
        .contains(&(args_ptr as usize))
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
            // Custom metaclass (subclass of type) with 3 args:
            // Meta(name, bases, namespace).  CPython's `type.__call__` dispatches
            // to `Meta.__new__(Meta, name, bases, namespace, **kwds)` and then
            // `Meta.__init__(cls, name, bases, namespace, **kwds)`.  Honor user
            // overrides of either method.
            if pos_args.len() == 3 && issubclass_bits(class_bits, builtins.type_obj) {
                // Build the kwargs dict once; reused for the fast path
                // (`molt_type_new`) and to dec-ref at exit.
                let kwargs_bits = if kw_names.is_empty() {
                    MoltObject::none().bits()
                } else {
                    let mut pairs = Vec::with_capacity(kw_names.len() * 2);
                    for (k, v) in kw_names.iter().zip(kw_values.iter()) {
                        pairs.push(*k);
                        pairs.push(*v);
                    }
                    let ptr = alloc_dict_with_pairs(_py, &pairs);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(ptr).bits()
                };

                // Look up `__new__` on the metaclass.  If the user did not
                // override it, the lookup resolves to the inherited
                // `type.__new__` (intrinsic `molt_type_new`); use the fast
                // path that also runs `__init_subclass__` and class slot
                // setup inline.  Otherwise dispatch to the user's override.
                let new_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
                let new_lookup = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits);
                let new_is_default = new_lookup
                    .map(|bits| {
                        let obj = obj_from_bits(bits);
                        let Some(p) = obj.as_ptr() else { return true };
                        if object_type_id(p) != TYPE_ID_FUNCTION {
                            return false;
                        }
                        function_fn_ptr(p) == fn_addr!(molt_type_new)
                    })
                    .unwrap_or(true);

                // `class_attr_lookup_raw_mro` returns borrowed bits.  Match
                // the OLD code path's lifetime contract: never dec-ref the
                // looked-up function bits.
                let new_class_bits = if new_is_default {
                    molt_type_new(
                        class_bits,
                        pos_args[0],
                        pos_args[1],
                        pos_args[2],
                        kwargs_bits,
                    )
                } else {
                    let new_bits = new_lookup.expect("non-default __new__ must resolve");
                    let new_builder =
                        molt_callargs_new((4 + kw_names.len()) as u64, kw_names.len() as u64);
                    if new_builder == 0 {
                        if !kw_names.is_empty() {
                            dec_ref_bits(_py, kwargs_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(new_builder, class_bits);
                    let _ = molt_callargs_push_pos(new_builder, pos_args[0]);
                    let _ = molt_callargs_push_pos(new_builder, pos_args[1]);
                    let _ = molt_callargs_push_pos(new_builder, pos_args[2]);
                    for (&k, &v) in kw_names.iter().zip(kw_values.iter()) {
                        let _ = molt_callargs_push_kw(new_builder, k, v);
                    }
                    molt_call_bind(new_bits, new_builder)
                };

                if exception_pending(_py) {
                    if !kw_names.is_empty() {
                        dec_ref_bits(_py, kwargs_bits);
                    }
                    return MoltObject::none().bits();
                }

                // CPython: only invoke `__init__` when `__new__` returned an
                // instance of `cls` (here, of the metaclass).  This matches
                // `type.__call__` semantics.
                let new_class_obj = obj_from_bits(new_class_bits);
                let returned_instance = if let Some(p) = new_class_obj.as_ptr() {
                    let inst_class_bits = object_class_bits(p);
                    inst_class_bits != 0 && issubclass_bits(inst_class_bits, class_bits)
                } else {
                    false
                };

                if returned_instance {
                    // Call Meta.__init__(new_class, name, bases, namespace, **kwds).
                    // `class_attr_lookup_raw_mro` returns borrowed bits — do
                    // not dec-ref.
                    let init_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.init_name,
                        b"__init__",
                    );
                    if let Some(init_bits) =
                        class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits)
                    {
                        let init_builder =
                            molt_callargs_new((4 + kw_names.len()) as u64, kw_names.len() as u64);
                        if init_builder != 0 {
                            let _ = molt_callargs_push_pos(init_builder, new_class_bits);
                            let _ = molt_callargs_push_pos(init_builder, pos_args[0]);
                            let _ = molt_callargs_push_pos(init_builder, pos_args[1]);
                            let _ = molt_callargs_push_pos(init_builder, pos_args[2]);
                            for (&k, &v) in kw_names.iter().zip(kw_values.iter()) {
                                let _ = molt_callargs_push_kw(init_builder, k, v);
                            }
                            let _init_result = molt_call_bind(init_bits, init_builder);
                        }
                        if exception_pending(_py) {
                            if !kw_names.is_empty() {
                                dec_ref_bits(_py, kwargs_bits);
                            }
                            return MoltObject::none().bits();
                        }
                    }
                }

                if !kw_names.is_empty() {
                    dec_ref_bits(_py, kwargs_bits);
                }
                return new_class_bits;
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
                if let Some(bound_args) =
                    builtin_args::bind_builtin_class_text_io_wrapper(_py, &*ptr)
                {
                    return call_class_init_with_args(_py, call_ptr, &bound_args);
                }
                return MoltObject::none().bits();
            }
            if class_bits == builtins.string_io
                && let Some(ptr) = args_ptr
                && !(*ptr).kw_names.is_empty()
            {
                if let Some(bound_args) = builtin_args::bind_builtin_class_string_io(_py, &*ptr) {
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
            // The CallArgs builder owns the injected slot; every function call
            // path, compiled or runtime, borrows parameters from that builder.
            // Builder teardown releases this retain after `__init__`, leaving
            // the constructor's original owning result as the single live ref.
            inc_ref_bits(_py, inst_bits);
            (*args_ptr).pos.insert(0, inst_bits);
        }
        let _ = molt_call_bind(init_bits, builder_bits);
        dec_ref_bits(_py, init_bits);
        // Full-binding `__init__` (`*args`/`**kwargs`/keyword-only) lands here
        // instead of the IC fast path. If it raised, surface the exception
        // rather than returning the partially-constructed instance — the IC fast
        // path (try_call_bind_ic_fast TYPE_CALL lane) already does this; routing
        // through the shared helper keeps the two paths from re-diverging
        // (task #60).
        crate::call::class_init::resolve_construct_after_init(_py, inst_bits)
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
        crate::object::class_finish_definition(_py, class_ptr);

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
    let Some(state) = call_bind_runtime_state_if_available() else {
        return std::ptr::null_mut();
    };
    state
        .callargs_builder_map
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
        if !callargs_builder_is_live(_py, builder_ptr) {
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
        if args_ptr.is_null() || !callargs_storage_is_live(_py, args_ptr) {
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

unsafe fn protect_bound_args_or_callargs_aliased_return(
    _py: &PyToken<'_>,
    result: u64,
    args_ptr: *mut CallArgs,
    bound_args: &[u64],
) -> u64 {
    unsafe {
        if bound_args.contains(&result) {
            inc_ref_bits(_py, result);
            return result;
        }
        protect_callargs_aliased_return(_py, result, args_ptr)
    }
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
        let mut result = call_function_obj_bound_vec(_py, func_bits, &[tuple_bits, kwargs_bits]);
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
            note_callargs_alloc(_py, ptr, args_ptr);
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
            if !callargs_builder_is_live(_py, builder_ptr) {
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
            if !callargs_builder_is_live(_py, builder_ptr) {
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
            if !callargs_builder_is_live(_py, builder_ptr) {
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
            if !callargs_builder_is_live(_py, builder_ptr) {
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

/// Read one binding-metadata attribute (`__molt_*__`/`__defaults__`/
/// `__kwdefaults__`) from a function's `__dict__`, returning `none` bits when
/// absent. Shared by the granular binder-shape classifiers below.
///
/// # Safety
/// `func_ptr` must be a live function object; the GIL must be held.
unsafe fn function_binding_meta(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    name_bytes: &'static [u8],
) -> u64 {
    unsafe {
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
    }
}

/// Whether a function needs the FULL argument binder for the fused method-call
/// fast path — i.e. it has keyword-only parameters, keyword-only defaults,
/// `*args`, `**kwargs`, or a builtin bind-kind. This is exactly
/// [`function_requires_full_binding`] MINUS the positional `__defaults__` test:
/// positional defaults are fillable allocation-free by the direct path (the
/// trampoline pads from `__defaults__`), so they must NOT force the binder.
///
/// A malformed `__defaults__` (present but not a tuple) is conservatively
/// treated as needing the binder so the direct path never mis-pads.
///
/// # Safety
/// `func_ptr` must be a live function object; the GIL must be held.
pub(crate) unsafe fn function_needs_full_binder(_py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
    unsafe {
        for name in [
            b"__molt_bind_kind__".as_slice(),
            b"__molt_vararg__",
            b"__molt_varkw__",
        ] {
            if !obj_from_bits(function_binding_meta(_py, func_ptr, name)).is_none() {
                return true;
            }
        }

        let kwonly_bits = function_binding_meta(_py, func_ptr, b"__molt_kwonly_names__");
        if !obj_from_bits(kwonly_bits).is_none() {
            let Some(ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
                return true;
            };
            if object_type_id(ptr) != TYPE_ID_TUPLE || !seq_vec_ref(ptr).is_empty() {
                return true;
            }
        }

        let kwdefaults_bits = function_binding_meta(_py, func_ptr, b"__kwdefaults__");
        if !obj_from_bits(kwdefaults_bits).is_none() {
            let Some(ptr) = obj_from_bits(kwdefaults_bits).as_ptr() else {
                return true;
            };
            if object_type_id(ptr) != TYPE_ID_DICT || !dict_order(ptr).is_empty() {
                return true;
            }
        }

        // A malformed `__defaults__` (non-None, non-tuple) cannot be padded
        // safely by the direct path — defer to the binder.
        let defaults_bits = function_binding_meta(_py, func_ptr, b"__defaults__");
        if !obj_from_bits(defaults_bits).is_none() {
            match obj_from_bits(defaults_bits).as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_TUPLE => {}
                _ => return true,
            }
        }

        false
    }
}

/// Count of trailing positional parameters carrying a default
/// (`len(__defaults__)`), for a function the caller has already established does
/// NOT need the full binder. Returns 0 when `__defaults__` is absent/empty.
///
/// # Safety
/// `func_ptr` must be a live function object; the GIL must be held.
unsafe fn function_positional_default_count(_py: &PyToken<'_>, func_ptr: *mut u8) -> usize {
    unsafe {
        let defaults_bits = function_binding_meta(_py, func_ptr, b"__defaults__");
        if obj_from_bits(defaults_bits).is_none() {
            return 0;
        }
        match obj_from_bits(defaults_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TUPLE => seq_vec_ref(ptr).len(),
            _ => 0,
        }
    }
}

pub(crate) unsafe fn refresh_function_requires_binder_flag(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> bool {
    unsafe {
        let needs_binder = function_needs_full_binder(_py, func_ptr);
        let header = header_from_obj_ptr(func_ptr);
        if needs_binder {
            (*header).flags |= HEADER_FLAG_FUNC_REQUIRES_BINDER;
        } else {
            (*header).flags &= !HEADER_FLAG_FUNC_REQUIRES_BINDER;
        }
        needs_binder
    }
}

pub(crate) unsafe fn function_requires_binder_flag(func_ptr: *mut u8) -> bool {
    unsafe {
        let header = header_from_obj_ptr(func_ptr);
        ((*header).flags & HEADER_FLAG_FUNC_REQUIRES_BINDER) != 0
    }
}

pub(crate) unsafe fn function_raw_positional_call_needs_binding(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    supplied: usize,
) -> bool {
    unsafe {
        if function_requires_binder_flag(func_ptr) || function_needs_full_binder(_py, func_ptr) {
            return true;
        }
        let positional_defaults = function_positional_default_count(_py, func_ptr);
        positional_defaults != 0 && supplied != function_arity(func_ptr) as usize
    }
}

pub(crate) unsafe fn call_function_obj_via_positional_bind(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe {
        let builder_bits = molt_callargs_new(MoltObject::from_int(args.len() as i64).bits(), 0);
        if builder_bits == 0 || exception_pending(_py) {
            return MoltObject::none().bits();
        }
        for &arg in args {
            let res = molt_callargs_push_pos(builder_bits, arg);
            if !obj_from_bits(res).is_none() || exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        molt_call_bind(func_bits, builder_bits)
    }
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
            if let Some(bound_self_bits) = self_bits {
                let target_obj = obj_from_bits(func_bits);
                let target_ptr = target_obj.as_ptr();
                if target_ptr.is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_FUNCTION) {
                    if builder_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let args_ptr = match require_callargs_ptr(_py, builder_ptr) {
                        Ok(ptr) => ptr,
                        Err(err) => return err,
                    };
                    inc_ref_bits(_py, bound_self_bits);
                    (*args_ptr).pos.insert(0, bound_self_bits);
                    builder_guard.release();
                    return molt_call_bind(func_bits, builder_bits);
                }
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
                if let Some(bound_args) = builtin_args::bind_builtin_open(_py, args) {
                    let result = call_function_obj_bound_vec(_py, func_bits, bound_args.as_slice());
                    return protect_bound_args_or_callargs_aliased_return(
                        _py,
                        result,
                        args_ptr,
                        bound_args.as_slice(),
                    );
                }
                return MoltObject::none().bits();
            }
            if let Some(kind_bits) = bind_kind_bits
                && obj_from_bits(kind_bits).as_int() == Some(BIND_KIND_CAPI_METHOD)
            {
                return call_capi_method_with_bound_args(_py, func_bits, args_ptr, args);
            }
            if fn_ptr == fn_addr!(dict_update_method) {
                return builtin_args::bind_builtin_dict_update(_py, args);
            }
            if fn_ptr == fn_addr!(molt_open_builtin) {
                if let Some(bound_args) = builtin_args::bind_builtin_open(_py, args) {
                    let result = call_function_obj_bound_vec(_py, func_bits, bound_args.as_slice());
                    return protect_bound_args_or_callargs_aliased_return(
                        _py,
                        result,
                        args_ptr,
                        bound_args.as_slice(),
                    );
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
                if let Some(bound_args) =
                    builtin_args::bind_builtin_call(_py, func_bits, func_ptr, args)
                {
                    let result = call_function_obj_bound_vec(_py, func_bits, bound_args.as_slice());
                    return protect_bound_args_or_callargs_aliased_return(
                        _py,
                        result,
                        args_ptr,
                        bound_args.as_slice(),
                    );
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
            let result = call_function_obj_bound_vec(_py, func_bits, final_args.as_slice());
            protect_bound_args_or_callargs_aliased_return(
                _py,
                result,
                args_ptr,
                final_args.as_slice(),
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::inline_cache::{
        CALL_BIND_IC_KIND_DIRECT_FUNC, CALL_BIND_IC_KIND_TYPE_CALL, CallBindIcEntry,
        clear_call_bind_ic_cache, ic_tls_insert, ic_tls_lookup, method_ic_call_plan,
        try_call_bind_ic_fast,
    };
    use super::{protect_callargs_aliased_return_with_extra, trace_call_type_builder_enabled_raw};
    use crate::object::builders::{alloc_list, alloc_tuple};
    use crate::{
        TYPE_ID_OBJECT, dec_ref_bits, obj_from_bits, object_type_id, ptr_from_bits, runtime_state,
    };
    use molt_obj_model::MoltObject;
    use std::sync::atomic::Ordering;

    extern "C" fn compiled_init_borrows_self_for_type_call_ic(self_bits: u64) -> i64 {
        crate::with_gil_entry_nopanic!(_py, {
            assert!(!obj_from_bits(self_bits).is_none());
            MoltObject::none().bits()
        }) as i64
    }

    extern "C" fn compiled_identity_returns_arg(arg_bits: u64) -> i64 {
        arg_bits as i64
    }

    extern "C" fn compiled_second_arg_returns_arg(_first_bits: u64, second_bits: u64) -> i64 {
        second_bits as i64
    }

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

    #[test]
    fn call_bind_builtin_full_binding_promotes_callargs_aliased_return() {
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                compiled_identity_returns_arg as *const () as usize as u64,
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(13).bits()]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let builder_bits = super::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let _ = unsafe { super::molt_callargs_push_pos(builder_bits, list_bits) };

            dec_ref_bits(_py, list_bits);
            let result_bits = super::molt_call_bind(func_bits, builder_bits);
            assert_eq!(
                result_bits, list_bits,
                "identity callable must return the argument bits unchanged"
            );
            let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live result");
            assert_eq!(result_ptr, list_ptr);
            let rc = unsafe {
                (*crate::object::header_from_obj_ptr(result_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(
                rc, 1,
                "call_bind must promote argument aliases before dropping CallArgs"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn call_bind_builtin_default_padded_argv_promotes_aliased_return() {
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                compiled_second_arg_returns_arg as *const () as usize as u64,
                2,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();

            let default_ptr = alloc_list(_py, &[MoltObject::from_int(17).bits()]);
            assert!(!default_ptr.is_null());
            let default_bits = MoltObject::from_ptr(default_ptr).bits();
            let defaults_ptr = alloc_tuple(_py, &[default_bits]);
            assert!(!defaults_ptr.is_null());
            let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
            let defaults_name = intern_metadata_name(_py, b"__defaults__");
            unsafe {
                crate::call::class_init::function_set_attr_bits(
                    _py,
                    func_ptr,
                    defaults_name,
                    defaults_bits,
                );
            }
            dec_ref_bits(_py, defaults_bits);
            dec_ref_bits(_py, default_bits);

            let before_call = unsafe {
                (*crate::object::header_from_obj_ptr(default_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(
                before_call, 1,
                "function __defaults__ tuple should be the only default owner before call"
            );

            let builder_bits = super::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let _ = unsafe {
                super::molt_callargs_push_pos(builder_bits, MoltObject::from_int(5).bits())
            };

            let result_bits = super::molt_call_bind(func_bits, builder_bits);
            assert_eq!(result_bits, default_bits);
            let result_ptr = obj_from_bits(result_bits)
                .as_ptr()
                .expect("live default result");
            assert_eq!(result_ptr, default_ptr);
            let after_call = unsafe {
                (*crate::object::header_from_obj_ptr(result_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(
                after_call, 2,
                "call_bind must promote returns aliasing default-padded argv"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    // ------------------------------------------------------------------
    // task #60: the constructor `__init__`-exception resolution invariant.
    //
    // `resolve_construct_after_init` is the single authority every construct
    // path (the IC fast path AND the full-binding `call_type_with_builder`
    // ForwardArgs arm AND `call_class_init_with_args`) routes through after
    // `__init__` runs. The invariant: it returns the instance iff no exception
    // is pending; on a pending exception it drops the instance's owning
    // reference and returns the `none` sentinel so the construct-site
    // `check_exception` / IC propagation guards fire. A constructor `__init__`
    // raise must NEVER be swallowed (it was, for full-binding `__init__`,
    // before this fix).
    // ------------------------------------------------------------------

    #[test]
    fn resolve_construct_after_init_no_pending_returns_instance_unchanged() {
        crate::with_gil_entry_nopanic!(_py, {
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(7).bits()]);
            assert!(!list_ptr.is_null());
            let inst_bits = MoltObject::from_ptr(list_ptr).bits();
            let before = unsafe {
                (*crate::object::header_from_obj_ptr(list_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(crate::molt_exception_pending(), 0, "no exception expected");
            // No pending exception: the owning reference is handed back as-is.
            let out =
                unsafe { crate::call::class_init::resolve_construct_after_init(_py, inst_bits) };
            assert_eq!(out, inst_bits, "must return the constructed instance");
            let after = unsafe {
                (*crate::object::header_from_obj_ptr(list_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(after, before, "success path must not perturb the refcount");
            dec_ref_bits(_py, inst_bits);
        });
    }

    #[test]
    fn resolve_construct_after_init_pending_drops_instance_and_returns_none() {
        crate::with_gil_entry_nopanic!(_py, {
            // Hold an extra owning reference so the helper's drop is observable
            // without freeing the object out from under the test.
            let list_ptr = alloc_list(_py, &[MoltObject::from_int(9).bits()]);
            assert!(!list_ptr.is_null());
            let inst_bits = MoltObject::from_ptr(list_ptr).bits();
            super::inc_ref_bits(_py, inst_bits);
            let before = unsafe {
                (*crate::object::header_from_obj_ptr(list_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };

            // Simulate `__init__` having raised: set a pending exception, then
            // resolve. The helper must drop the instance's owning reference and
            // surface the raise via a `none` result.
            let _: u64 = crate::builtins::exceptions::raise_exception(
                _py,
                "ValueError",
                "task60 init raise",
            );
            assert_eq!(
                crate::molt_exception_pending(),
                1,
                "exception must be pending"
            );

            let out =
                unsafe { crate::call::class_init::resolve_construct_after_init(_py, inst_bits) };
            assert!(
                MoltObject::from_bits(out).is_none(),
                "a pending __init__ exception must yield the None sentinel, not the instance"
            );
            assert_eq!(
                crate::molt_exception_pending(),
                1,
                "the helper must not clear the pending exception — the caller propagates it"
            );
            let after = unsafe {
                (*crate::object::header_from_obj_ptr(list_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(
                after,
                before - 1,
                "the exception path must drop exactly one (the instance's) owning reference"
            );

            let _ = crate::molt_exception_clear();
            assert_eq!(crate::molt_exception_pending(), 0);
            // Release the extra reference taken above.
            dec_ref_bits(_py, inst_bits);
        });
    }

    #[test]
    fn type_call_ic_returns_single_owned_constructor_result_after_borrowed_init() {
        crate::with_gil_entry_nopanic!(_py, {
            clear_call_bind_ic_cache();
            let init_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                compiled_init_borrows_self_for_type_call_ic as *const () as usize as u64,
                1,
            );
            assert!(!init_ptr.is_null());
            let init_bits = MoltObject::from_ptr(init_ptr).bits();
            let builtins = crate::builtins::classes::builtin_classes(_py);
            let name_ptr = super::alloc_string(_py, b"IcCtor");
            let init_name_ptr = super::alloc_string(_py, b"__init__");
            assert!(!name_ptr.is_null());
            assert!(!init_name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let init_name_bits = MoltObject::from_ptr(init_name_ptr).bits();
            let attrs = [init_name_bits, init_bits];
            let bases = [builtins.object];
            let class_bits = unsafe {
                crate::object::ops::molt_guarded_class_def(
                    name_bits,
                    bases.as_ptr() as usize as u64,
                    bases.len() as u64,
                    attrs.as_ptr() as usize as u64,
                    1,
                    std::mem::size_of::<u64>() as i64,
                    0,
                    0,
                )
            };
            assert!(!obj_from_bits(class_bits).is_none());
            let class_ptr = obj_from_bits(class_bits).as_ptr().expect("class ptr");
            let layout_size =
                unsafe { crate::call::class_init::class_layout_size_cached(_py, class_ptr) };
            let entry = CallBindIcEntry {
                fn_ptr: compiled_init_borrows_self_for_type_call_ic as *const () as usize as u64,
                target_bits: init_bits,
                class_bits,
                class_version: unsafe { crate::class_layout_version_bits(class_ptr) },
                cached_alloc_size: (layout_size + std::mem::size_of::<crate::object::MoltHeader>())
                    as u32,
                arity: 0,
                kind: CALL_BIND_IC_KIND_TYPE_CALL,
            };
            let builder_bits = super::molt_callargs_new(0, 0);
            let builder_ptr = ptr_from_bits(builder_bits);
            let args_ptr = unsafe { super::callargs_ptr(builder_ptr) };
            let result_bits = unsafe {
                try_call_bind_ic_fast(_py, entry, class_bits, args_ptr)
                    .expect("type-call IC entry should apply")
            };
            let result_ptr = obj_from_bits(result_bits).as_ptr().expect("live instance");
            assert_eq!(unsafe { object_type_id(result_ptr) }, TYPE_ID_OBJECT);
            let ref_count = unsafe {
                (*crate::object::header_from_obj_ptr(result_ptr))
                    .ref_count
                    .load(Ordering::Relaxed)
            };
            assert_eq!(
                ref_count, 1,
                "type-call IC must return exactly the constructor result owner; borrowed __init__ self must not leave a hidden retain"
            );
            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, builder_bits);
            dec_ref_bits(_py, init_name_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, init_bits);
        });
    }

    #[test]
    fn callargs_registries_are_runtime_scoped() {
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            {
                let mut guard = state.call_bind.lock().unwrap();
                guard.callargs_builder_map.clear();
                guard.callargs_storage_registry.clear();
            }

            let builder_bits = super::molt_callargs_new(1, 0);
            assert!(!obj_from_bits(builder_bits).is_none());
            let builder_ptr = ptr_from_bits(builder_bits);
            assert!(!builder_ptr.is_null());
            let args_ptr = unsafe { super::callargs_ptr(builder_ptr) };
            assert!(!args_ptr.is_null());
            {
                let guard = state.call_bind.lock().unwrap();
                assert_eq!(guard.callargs_builder_map.len(), 1);
                assert_eq!(guard.callargs_storage_registry.len(), 1);
                assert!(
                    guard
                        .callargs_builder_map
                        .contains_key(&(builder_ptr as usize))
                );
                assert!(
                    guard
                        .callargs_storage_registry
                        .contains(&(args_ptr as usize))
                );
            }

            dec_ref_bits(_py, builder_bits);
            {
                let guard = state.call_bind.lock().unwrap();
                assert!(guard.callargs_builder_map.is_empty());
                assert!(guard.callargs_storage_registry.is_empty());
            }
        });
    }

    #[test]
    fn clear_call_bind_ic_cache_clears_thread_local_cache() {
        let entry = CallBindIcEntry {
            fn_ptr: 11,
            target_bits: 22,
            class_bits: 0,
            class_version: 33,
            cached_alloc_size: 44,
            arity: 1,
            kind: CALL_BIND_IC_KIND_DIRECT_FUNC,
        };
        ic_tls_insert(99, entry);
        assert!(ic_tls_lookup(99).is_some());
        clear_call_bind_ic_cache();
        assert!(ic_tls_lookup(99).is_none());
    }

    // ------------------------------------------------------------------
    // Fused method-call IC: call-plan classification + direct-vs-binder gate.
    //
    // `method_ic_call_plan` returns `(fixed_arity_including_self,
    // n_pos_defaults, needs_binder)`. The fused fast path's `direct_ok` gate is
    //   !needs_binder
    //     && fixed_arity <= DIRECT_ARGV_MAX
    //     && (fixed_arity - n_pos_defaults) <= supplied+1 <= fixed_arity
    // (the `+1` is `self`). When the gate is false the call routes to the
    // cached-bind path (full binder). The load-bearing distinction: POSITIONAL
    // defaults are direct-fillable (gate may be true), while kw-only / `*args` /
    // `**kwargs` set `needs_binder` (gate always false). These tests pin the
    // classifier + gate exhaustively over the reviewer-enumerated shapes:
    // no-default exact arity (direct), positional default (direct, paddable
    // range), kw-only ±default (binder), *args (binder), **kwargs (binder), and
    // an arity mismatch (binder).
    // ------------------------------------------------------------------

    /// Mirror of the production `direct_ok` closure in `call_method_ic_dispatch`
    /// (kept in lock-step). `supplied_pos` excludes `self`; the production
    /// `DIRECT_ARGV_MAX` is 16.
    fn direct_ok_gate(
        fixed_arity: u8,
        n_pos_defaults: u8,
        needs_binder: bool,
        supplied_pos: usize,
    ) -> bool {
        if needs_binder {
            return false;
        }
        let fixed_arity = fixed_arity as usize;
        if fixed_arity > 16 {
            return false;
        }
        let supplied = supplied_pos + 1;
        let min_supplied = fixed_arity.saturating_sub(n_pos_defaults as usize);
        supplied >= min_supplied && supplied <= fixed_arity
    }

    /// Build a `TYPE_ID_FUNCTION` with the given fixed arity (INCLUDING `self`)
    /// and optional binding metadata, then return its bits. The function never
    /// runs in these tests — only its shape metadata is read.
    unsafe fn make_test_function(
        _py: &crate::PyToken<'_>,
        arity_including_self: u64,
        meta: &[(&'static [u8], u64)],
    ) -> u64 {
        use crate::object::builders::alloc_function_obj;
        // fn_ptr is irrelevant for shape classification; use a dummy non-null.
        let func_ptr = alloc_function_obj(_py, 1, arity_including_self);
        assert!(!func_ptr.is_null());
        for (name, val_bits) in meta.iter().copied() {
            let attr_bits = intern_metadata_name(_py, name);
            unsafe {
                crate::call::class_init::function_set_attr_bits(_py, func_ptr, attr_bits, val_bits)
            };
        }
        MoltObject::from_ptr(func_ptr).bits()
    }

    /// Intern one of the binding-metadata attribute names against the same slot
    /// the production classifiers consult, so the test exercises the real
    /// classifier rather than an ad-hoc key.
    fn intern_metadata_name(_py: &crate::PyToken<'_>, name: &'static [u8]) -> u64 {
        use crate::runtime_state;
        use crate::state::cache::intern_static_name;
        let interned = &runtime_state(_py).interned;
        let slot = match name {
            b"__molt_bind_kind__" => &interned.molt_bind_kind,
            b"__molt_vararg__" => &interned.molt_vararg,
            b"__molt_varkw__" => &interned.molt_varkw,
            b"__molt_kwonly_names__" => &interned.molt_kwonly_names,
            b"__defaults__" => &interned.defaults_name,
            b"__kwdefaults__" => &interned.kwdefaults_name,
            other => panic!("unknown metadata name {:?}", other),
        };
        intern_static_name(_py, slot, name)
    }

    #[test]
    fn method_ic_plan_no_default_exact_arity_is_direct() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x): ...  called as obj.m(arg)  -> direct
            let func_bits = unsafe { make_test_function(_py, 2, &[]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 2);
            assert_eq!(plan.n_pos_defaults, 0);
            assert!(!plan.needs_binder, "no metadata => no binder");
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "1 supplied + self == arity 2 -> direct"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_positional_default_is_direct_over_paddable_range() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x, bump=1): ...  -> direct (positional default), NOT
            // binder. __defaults__ = (1,) (a non-empty tuple).
            let one = MoltObject::from_int(1).bits();
            let defaults_ptr = crate::object::builders::alloc_tuple(_py, &[one]);
            let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 3, &[(b"__defaults__", defaults_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 3);
            assert_eq!(plan.n_pos_defaults, 1, "len(__defaults__) == 1");
            assert!(!plan.needs_binder, "positional default => NOT binder");
            // obj.m(x)        -> supplied 2, pad bump  -> direct
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "x supplied, bump padded -> direct"
            );
            // obj.m(x, bump)  -> supplied 3 == arity   -> direct (no pad)
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 2),
                "x+bump supplied -> direct"
            );
            // obj.m()         -> supplied 1 < min 2    -> binder (arity error)
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 0),
                "0 supplied (self only) below min -> binder"
            );
            // obj.m(a,b,c)    -> supplied 4 > arity 3  -> binder (arity error)
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 3),
                "too many positionals -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, defaults_bits);
        });
    }

    #[test]
    fn method_ic_plan_two_positional_defaults_widen_paddable_range() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, a, b, c=1, d=2): ...  -> arity 5, 2 defaults.
            let one = MoltObject::from_int(1).bits();
            let two = MoltObject::from_int(2).bits();
            let defaults_ptr = crate::object::builders::alloc_tuple(_py, &[one, two]);
            let defaults_bits = MoltObject::from_ptr(defaults_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 5, &[(b"__defaults__", defaults_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 5);
            assert_eq!(plan.n_pos_defaults, 2);
            assert!(!plan.needs_binder);
            // min supplied = 5 - 2 = 3 (self,a,b); max = 5 (self,a,b,c,d).
            for supplied_pos in 2..=4usize {
                // supplied incl self = 3,4,5 -> all direct.
                assert!(
                    direct_ok_gate(
                        plan.fixed_arity,
                        plan.n_pos_defaults,
                        plan.needs_binder,
                        supplied_pos
                    ),
                    "supplied_pos={} should be direct",
                    supplied_pos
                );
            }
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "only a supplied (self,a=2) below min 3 -> binder"
            );
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 5),
                "6 incl self > arity 5 -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, defaults_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwonly_with_default_needs_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x, *, ctx=None): ...  -> binder (kwonly name present).
            let name_ptr = crate::object::builders::alloc_string(_py, b"ctx");
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let kwonly_ptr = crate::object::builders::alloc_tuple(_py, &[name_bits]);
            let kwonly_bits = MoltObject::from_ptr(kwonly_ptr).bits();
            // kwdefaults present too (ctx=None), but the kwonly NAME alone forces
            // the binder.
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__molt_kwonly_names__", kwonly_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "kw-only param => binder");
            assert!(!direct_ok_gate(
                plan.fixed_arity,
                plan.n_pos_defaults,
                plan.needs_binder,
                1
            ));
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, kwonly_bits);
            crate::dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwonly_without_default_needs_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, x, *, ctx): ...  (kwonly, no default) -> binder.
            // The kw-only NAME alone forces the binder; defaults are orthogonal.
            let name_ptr = crate::object::builders::alloc_string(_py, b"ctx");
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let kwonly_ptr = crate::object::builders::alloc_tuple(_py, &[name_bits]);
            let kwonly_bits = MoltObject::from_ptr(kwonly_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__molt_kwonly_names__", kwonly_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "kw-only param (no default) => binder");
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, kwonly_bits);
            crate::dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwdefaults_only_needs_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            // A non-empty __kwdefaults__ dict (kw-only defaults) forces the
            // binder even if the kwonly-names tuple was not explicitly recorded.
            let none_bits = MoltObject::none().bits();
            let key_ptr = crate::object::builders::alloc_string(_py, b"ctx");
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let dict_ptr =
                crate::object::builders::alloc_dict_with_pairs(_py, &[key_bits, none_bits]);
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__kwdefaults__", dict_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "non-empty __kwdefaults__ => binder");
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, dict_bits);
            crate::dec_ref_bits(_py, key_bits);
        });
    }

    #[test]
    fn method_ic_plan_varargs_needs_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, *args): ...  -> binder (*args present).
            let star_ptr = crate::object::builders::alloc_string(_py, b"args");
            let star_bits = MoltObject::from_ptr(star_ptr).bits();
            let func_bits =
                unsafe { make_test_function(_py, 1, &[(b"__molt_vararg__", star_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "*args => binder");
            assert!(!direct_ok_gate(
                plan.fixed_arity,
                plan.n_pos_defaults,
                plan.needs_binder,
                3
            ));
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, star_bits);
        });
    }

    #[test]
    fn method_ic_plan_bind_kind_needs_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            let bind_kind_bits = MoltObject::from_int(crate::BIND_KIND_PACKED_BUILTIN).bits();
            let func_bits =
                unsafe { make_test_function(_py, 2, &[(b"__molt_bind_kind__", bind_kind_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "bind kind => binder");
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "bind-kind functions cannot use the direct positional path"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_kwargs_needs_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, **kwargs): ...  -> binder (**kwargs present).
            let kw_ptr = crate::object::builders::alloc_string(_py, b"kwargs");
            let kw_bits = MoltObject::from_ptr(kw_ptr).bits();
            let func_bits = unsafe { make_test_function(_py, 1, &[(b"__molt_varkw__", kw_bits)]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert!(plan.needs_binder, "**kwargs => binder");
            crate::dec_ref_bits(_py, func_bits);
            crate::dec_ref_bits(_py, kw_bits);
        });
    }

    #[test]
    fn method_ic_plan_arity_mismatch_blocks_direct_without_binder() {
        crate::with_gil_entry_nopanic!(_py, {
            // def m(self, a, b): ...  (no defaults). Direct only at exact arity.
            let func_bits = unsafe { make_test_function(_py, 3, &[]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 3);
            assert_eq!(plan.n_pos_defaults, 0);
            assert!(!plan.needs_binder);
            // No defaults => min == max == arity 3 (incl self).
            assert!(
                direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 2),
                "2 supplied + self == 3 OK"
            );
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 1),
                "1 supplied + self < 3 -> binder"
            );
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 3),
                "3 supplied + self > 3 -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_wide_arity_over_argv_max_blocks_direct() {
        crate::with_gil_entry_nopanic!(_py, {
            // A method whose fixed arity exceeds DIRECT_ARGV_MAX (16) must take
            // the binder even with no binder-forcing features, since the direct
            // path's stack arg buffer cannot hold the call.
            let func_bits = unsafe { make_test_function(_py, 17, &[]) };
            let plan = unsafe { method_ic_call_plan(_py, func_bits) }
                .expect("plain function must classify");
            assert_eq!(plan.fixed_arity, 17);
            assert!(!plan.needs_binder);
            assert!(
                !direct_ok_gate(plan.fixed_arity, plan.n_pos_defaults, plan.needs_binder, 16),
                "arity 17 > DIRECT_ARGV_MAX -> binder"
            );
            crate::dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn method_ic_plan_non_function_classifies_none() {
        crate::with_gil_entry_nopanic!(_py, {
            // A non-function callable bits value must not classify (the fast path
            // is function-only).
            let list_ptr = crate::object::builders::alloc_list(_py, &[]);
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            assert!(unsafe { method_ic_call_plan(_py, list_bits) }.is_none());
            crate::dec_ref_bits(_py, list_bits);
        });
    }
}
