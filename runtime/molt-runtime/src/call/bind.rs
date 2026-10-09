#[cfg(test)]
use crate::alloc_string;
use crate::builtins::frames::FrameInvocationGuard;
use crate::call::function::{
    ArgumentTransfer, FunctionBindingField, call_function_obj_moved, function_bits_adopt_arguments,
};
use crate::call::type_policy::{
    callable_matches_runtime_symbol, resolved_new_is_default_object_new,
};
use crate::call::{
    CallAttrLookup, StaticmethodCallTarget, lookup_call_attr, require_call_attr,
    resolve_staticmethod_call_target,
};
use crate::state::recursion::RecursionGuard;
use crate::state::tls::FRAME_STACK;
use crate::{
    ALLOC_BYTES_CALLARGS, BIND_KIND_CAPI_METHOD, BIND_KIND_CLINIC_NAMED, BIND_KIND_TYPE_NEW_INIT,
    CALL_BIND_IC_HIT_COUNT, CALL_BIND_IC_MISS_COUNT, HEADER_FLAG_FUNC_REQUIRES_BINDER,
    INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT, MoltHeader, MoltObject, PtrDropGuard, PyToken,
    TYPE_ID_BOUND_METHOD, TYPE_ID_CALLARGS, TYPE_ID_DICT, TYPE_ID_FOREIGN, TYPE_ID_FROZENSET,
    TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS, TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, alloc_dict_with_pairs, alloc_instance_for_default_object_new, alloc_object,
    alloc_tuple,
    audit::{AuditArgs, audit_capability_decision},
    bits_from_ptr, bound_method_func_bits, bound_method_self_bits, builtin_classes,
    call_class_init_with_args, call_function_obj_bound_vec, class_attr_lookup_raw_mro,
    class_layout_version_bits, class_name_bits, class_name_for_error, code_filename_bits,
    code_name_bits, dec_ref_bits, dict_fromkeys_method, dict_get_in_place, dict_get_method,
    dict_len, dict_live_entries, dict_setdefault_method, dict_update_method,
    dict_update_set_via_store, exception_pending, function_arity, function_arity_usize,
    function_attr_bits, function_execution_closure_bits, function_fn_ptr, function_name_bits,
    function_trampoline_ptr, generic_alias_origin_bits, has_capability, header_from_obj_ptr,
    inc_ref_bits, intern_static_name, is_builtin_class_bits, is_trusted, is_truthy,
    issubclass_bits, maybe_ptr_from_bits, missing_bits, molt_bytearray_count_slice,
    molt_bytearray_decode, molt_bytearray_endswith_slice, molt_bytearray_find_slice,
    molt_bytearray_hex, molt_bytearray_index_slice, molt_bytearray_pop, molt_bytearray_rfind_slice,
    molt_bytearray_rindex_slice, molt_bytearray_rsplit_max, molt_bytearray_split_max,
    molt_bytearray_splitlines, molt_bytearray_startswith_slice, molt_bytes_count_slice,
    molt_bytes_decode, molt_bytes_endswith_slice, molt_bytes_find_slice, molt_bytes_hex,
    molt_bytes_index_slice, molt_bytes_maketrans, molt_bytes_rfind_slice, molt_bytes_rindex_slice,
    molt_bytes_rsplit_max, molt_bytes_split_max, molt_bytes_splitlines,
    molt_bytes_startswith_slice, molt_dict_pop_method, molt_file_reconfigure,
    molt_frozenset_copy_method, molt_frozenset_difference_multi, molt_frozenset_intersection_multi,
    molt_frozenset_isdisjoint, molt_frozenset_issubset, molt_frozenset_issuperset,
    molt_frozenset_symmetric_difference, molt_frozenset_union_multi, molt_int_from_bytes,
    molt_int_to_bytes, molt_list_append, molt_list_index_range, molt_list_pop, molt_list_sort,
    molt_memoryview_cast, molt_memoryview_hex, molt_object_init, molt_object_init_subclass,
    molt_object_new_bound, molt_set_clear, molt_set_copy_method, molt_set_difference_multi,
    molt_set_difference_update_multi, molt_set_intersection_multi,
    molt_set_intersection_update_multi, molt_set_isdisjoint, molt_set_issubset,
    molt_set_issuperset, molt_set_symmetric_difference, molt_set_symmetric_difference_update,
    molt_set_union_multi, molt_set_update_multi, molt_string_count_slice, molt_string_encode,
    molt_string_endswith_slice, molt_string_find_slice, molt_string_format_method,
    molt_string_index_slice, molt_string_rfind_slice, molt_string_rindex_slice,
    molt_string_rsplit_max, molt_string_split_max, molt_string_splitlines,
    molt_string_startswith_slice, molt_tuple_index_range, molt_type_call, molt_type_init,
    molt_type_new, obj_from_bits, object_class_bits, object_type_id, profile_hit_unchecked,
    ptr_from_bits, raise_exception, raise_not_callable, runtime_state, runtime_state_for_gil,
    string_obj_to_owned, type_name, type_of_bits,
};
use std::collections::{HashMap, HashSet};
use std::sync::{MutexGuard, OnceLock};

mod arguments;
mod builtin_args;
mod constructors;
mod frame_binding;
#[cfg(test)]
mod test_support;

use arguments::{Admission, ArgumentCustody, callee_custody};
pub(crate) use arguments::{
    CallArguments, CallBindRuntimeState, CallForm, EntryArguments, callargs_detach_owned,
    callargs_ptr, callargs_visit_owned, release_stack_arguments,
};
#[cfg(feature = "molt_gpu_primitives")]
pub(crate) use arguments::{
    callargs_positional_snapshot, clone_callargs_builder_bits, molt_callargs_new_expanded,
};
pub use arguments::{
    molt_callargs_expand_kwstar, molt_callargs_expand_star, molt_callargs_new,
    molt_callargs_push_kw, molt_callargs_push_pos,
};
pub(crate) use constructors::dispatch_init_subclass_hooks;
use constructors::{call_type_with_arguments, is_default_type_call};
#[cfg(feature = "molt_gpu_primitives")]
pub(crate) use frame_binding::bind_python_frame_tuple;
use frame_binding::{
    call_function_with_arguments, call_owned_function, takes_positional_arguments_over,
};
pub(crate) use frame_binding::{
    function_raw_positional_call_needs_binding, function_requires_binder_flag,
    refresh_function_requires_binder_flag,
};

mod inline_cache;
use inline_cache::{call_bind_ic_entry_for_call, try_call_bind_ic_fast};
pub(crate) use inline_cache::{
    clear_call_bind_ic_cache, clear_method_ic_cache, clear_super_ic_cache,
    detach_callable_ic_caches,
};

#[cfg(test)]
pub(crate) fn call_bind_ic_site_cached_for_test(site_id: u64) -> bool {
    inline_cache::ic_tls_lookup(site_id).is_some()
}
#[allow(unused_imports)]
pub use inline_cache::{
    molt_call_bind_ic, molt_call_bind_ic_owned, molt_call_indirect_ic, molt_call_method_ic_owned,
    molt_call_method_ic0, molt_call_method_ic1, molt_call_method_ic2, molt_call_method_ic3,
    molt_call_method_ic4, molt_call_super_method_ic_owned, molt_call_super_method_ic0,
    molt_call_super_method_ic1, molt_call_super_method_ic2, molt_call_super_method_ic3,
    molt_call_super_method_ic4, molt_invoke_ffi_ic,
};

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

/// C-API methods take `(args, kwargs)` containers and may retain either. The
/// call's argument vector keeps its own references until the call ends.
unsafe fn call_capi_method_with_bound_args(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        let keywords = match args.keyword_mapping() {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let tuple_ptr = alloc_tuple(_py, args.positional());
        if tuple_ptr.is_null() {
            dec_ref_bits(_py, keywords);
            return MoltObject::none().bits();
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let result = call_function_obj_bound_vec(_py, func_bits, &[tuple_bits, keywords]);
        dec_ref_bits(_py, tuple_bits);
        dec_ref_bits(_py, keywords);
        result
    }
}

/// Call `call_bits` with operands its runtime caller keeps. The call retains
/// its own argument vector, which binding moves into the callee frame, so the
/// caller's references stay the last owners of its operands. `receiver` is
/// prepended, as `type.__call__` lends its arguments to `__new__` and
/// `__init__`. No heap builder or builder registry entry is involved.
pub(crate) unsafe fn call_bind_borrowed(
    _py: &PyToken<'_>,
    call_bits: u64,
    receiver: Option<u64>,
    positional: &[u64],
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    unsafe {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        match CallArguments::retained(_py, receiver, positional, kw_names, kw_values) {
            Ok(arguments) => call_bind_with_arguments(_py, call_bits, arguments),
            Err(err) => err,
        }
    }
}

/// Public dictionary-call ingress. The existing argument owner retains the
/// mapping through redispatch and performs zero-hash unpacking for vector users.
pub(crate) unsafe fn call_bind_capi(
    py: &PyToken<'_>,
    callable: u64,
    receiver: Option<u64>,
    positional: &[u64],
    mapping: u64,
) -> u64 {
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    match CallArguments::capi(py, receiver, positional, mapping) {
        Ok(arguments) => unsafe { call_bind_with_arguments(py, callable, arguments) },
        Err(error) => error,
    }
}

/// Public vector ingress enters the same dispatcher without a temporary dict.
pub(crate) unsafe fn call_bind_capi_vector(
    py: &PyToken<'_>,
    callable: u64,
    positional: &[u64],
    names: &[u64],
    values: &[u64],
) -> u64 {
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    match CallArguments::capi_vector(py, positional, names, values) {
        Ok(arguments) => unsafe { call_bind_with_arguments(py, callable, arguments) },
        Err(error) => error,
    }
}

/// Route a call on a `TYPE_ID_FOREIGN` wrapper through the wrapped C object's
/// own `tp_call`. Materializes the call's positional arguments into a Molt
/// tuple and passes its keyword mapping, then hands them to the ABI bridge
/// (which builds a C-layout args tuple the callee can read). The C callee may
/// retain either container; the call's argument vector releases its own
/// references after the call. Returns the call result as an owned Molt handle,
/// or the error sentinel with an exception set.
///
/// # Safety
/// `call_ptr` must be a live `TYPE_ID_FOREIGN` object.
unsafe fn call_foreign_with_arguments(
    _py: &PyToken<'_>,
    call_ptr: *mut u8,
    args: &CallArguments<'_, '_>,
) -> u64 {
    let c_ptr = unsafe { crate::object::foreign::foreign_ptr_from_obj(call_ptr) };
    let tuple_ptr = crate::alloc_tuple(_py, args.positional());
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
    let kwargs_bits = match args.keyword_mapping() {
        Ok(bits) if obj_from_bits(bits).is_none() => 0,
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, args_bits);
            return err;
        }
    };
    let result =
        unsafe { molt_cpython_abi::bridge::molt_foreign_call(c_ptr, args_bits, kwargs_bits) };
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        if args_bits != 0 {
            dec_ref_bits(_py, args_bits);
        }
        if kwargs_bits != 0 {
            dec_ref_bits(_py, kwargs_bits);
        }
    });
    match result.decode() {
        molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => bits,
        molt_cpython_abi::hooks::DecodedHandleResult::Missing
        | molt_cpython_abi::hooks::DecodedHandleResult::Error => {
            crate::cpython_abi_hooks::propagate_native_failure(_py, "foreign object call");
            MoltObject::none().bits()
        }
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `builder_bits` is a live CallArgs builder whose reference
/// this call consumes.
pub extern "C" fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let builder_ptr = ptr_from_bits(builder_bits);
            let builder_guard = PtrDropGuard::new(builder_ptr);
            // A pending error means argument preparation failed: the builder
            // is still that call's value stack and releases as one.
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let args = match CallArguments::from_builder(_py, builder_ptr) {
                Ok(args) => args,
                Err(err) => return err,
            };
            // T1 is complete; the builder owns no argument edge any longer.
            drop(builder_guard);
            call_bind_with_arguments(_py, call_bits, args)
        }
    })
}

/// Dispatch a call that owns its argument vector. Custody is decided from the
/// original callee; callee resolution and every redispatch pass the same owner
/// onward, and nothing returns to a builder.
pub(crate) unsafe fn call_bind_with_arguments(
    _py: &PyToken<'_>,
    call_bits: u64,
    mut args: CallArguments<'_, '_>,
) -> u64 {
    unsafe {
        // User code never starts under an unhandled error (a constructor's
        // `isinstance` or truth callback can leave one); the argument vector
        // then releases as an ended call.
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        args.admit_custody(callee_custody(_py, call_bits, args.form));
        let call_obj = obj_from_bits(call_bits);
        let cached_mode = trace_call_bind_mode();
        let trace = !matches!(cached_mode, TraceCallBindMode::Off);
        let trace_verbose = matches!(cached_mode, TraceCallBindMode::Verbose);
        if trace_verbose {
            let callee_type = type_name(_py, call_obj);
            let first_pos_type = args
                .positional()
                .first()
                .map(|&bits| type_name(_py, obj_from_bits(bits)))
                .unwrap_or_else(|| std::borrow::Cow::Borrowed("<none>"));
            eprintln!(
                "molt call_bind enter callee_bits=0x{call_bits:x} callee_type={} pos_len={} kw_len={} first_pos_type={}",
                callee_type,
                args.positional().len(),
                args.keyword_count(),
                first_pos_type
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
                let positional = args.positional();
                eprintln!(
                    "molt call_bind args pos_len={} kw_len={} first_pos={:?} second_pos={:?}",
                    positional.len(),
                    args.keyword_count(),
                    positional.first(),
                    positional.get(1),
                );
                if let Some(&bits) = positional.first() {
                    eprintln!(
                        "molt call_bind args first_pos_bits=0x{bits:x} first_pos_type={}",
                        type_name(_py, obj_from_bits(bits)),
                    );
                    if let Some(s) = string_obj_to_owned(obj_from_bits(bits)) {
                        eprintln!("molt call_bind args first_pos_str={}", s);
                    }
                }
                if let Some(&bits) = positional.get(1)
                    && let Some(s) = string_obj_to_owned(obj_from_bits(bits))
                {
                    eprintln!("molt call_bind args second_pos_str={}", s);
                }
            }
            return raise_not_callable(_py, call_obj);
        };
        match resolve_staticmethod_call_target(_py, call_bits) {
            StaticmethodCallTarget::Owned(target) => {
                return call_bind_with_arguments(_py, target.bits(), args);
            }
            StaticmethodCallTarget::Raised => return MoltObject::none().bits(),
            StaticmethodCallTarget::NotStaticmethod => {}
        }
        let mut func_bits = call_bits;
        let mut self_bits = None;
        // A public dictionary call lends its mapping directly to native tp_call
        // and tuple-based C methods. Vector C conventions validate after unpack;
        // source CALL still validates at its original instruction boundary.
        let native_capi_mapping = args.capi_mapping().is_some()
            && match object_type_id(call_ptr) {
                TYPE_ID_FOREIGN => true,
                TYPE_ID_FUNCTION => crate::cpython_abi_hooks::is_cext_callable(call_ptr),
                TYPE_ID_BOUND_METHOD => obj_from_bits(bound_method_func_bits(call_ptr))
                    .as_ptr()
                    .is_some_and(|function| crate::cpython_abi_hooks::is_cext_callable(function)),
                _ => false,
            };
        if matches!(
            object_type_id(call_ptr),
            TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE | TYPE_ID_FOREIGN
        ) && !native_capi_mapping
            && !args.validate_keywords()
        {
            return MoltObject::none().bits();
        }
        match object_type_id(call_ptr) {
            TYPE_ID_FUNCTION => {}
            TYPE_ID_BOUND_METHOD => {
                func_bits = bound_method_func_bits(call_ptr);
                self_bits = Some(bound_method_self_bits(call_ptr));
            }
            TYPE_ID_TYPE => {
                match lookup_call_attr(_py, call_ptr) {
                    CallAttrLookup::Found(call_attr_bits) => {
                        if !is_default_type_call(_py, call_attr_bits) {
                            let result = call_bind_with_arguments(_py, call_attr_bits, args);
                            dec_ref_bits(_py, call_attr_bits);
                            return result;
                        }
                        dec_ref_bits(_py, call_attr_bits);
                    }
                    CallAttrLookup::Raised => return MoltObject::none().bits(),
                    CallAttrLookup::Missing => {}
                }
                return call_type_with_arguments(_py, call_ptr, args);
            }
            TYPE_ID_GENERIC_ALIAS => {
                let origin_bits = generic_alias_origin_bits(call_ptr);
                return call_bind_with_arguments(_py, origin_bits, args);
            }
            TYPE_ID_FOREIGN => {
                // Foreign (C-extension) callable: route through the wrapped
                // object's own `tp_call` via the ABI bridge. The foreign call
                // borrows the argument vector, which releases after it returns.
                return call_foreign_with_arguments(_py, call_ptr, &args);
            }
            _ => {
                let call_attr_bits = match require_call_attr(_py, call_ptr, call_obj) {
                    Ok(bits) => bits,
                    Err(result) => return result,
                };
                if let Some(entry) = call_bind_ic_entry_for_call(_py, call_attr_bits)
                    && let Some(res) = try_call_bind_ic_fast(_py, entry, call_attr_bits, &mut args)
                {
                    dec_ref_bits(_py, call_attr_bits);
                    return res;
                }
                let result = call_bind_with_arguments(_py, call_attr_bits, args);
                dec_ref_bits(_py, call_attr_bits);
                return result;
            }
        }
        if let Some(bound_self_bits) = self_bits {
            let target_obj = obj_from_bits(func_bits);
            let target_ptr = target_obj.as_ptr();
            if target_ptr.is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_FUNCTION) {
                if let Err(err) = args.prepend_positional(bound_self_bits) {
                    return err;
                }
                return call_bind_with_arguments(_py, func_bits, args);
            }
        }
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        if callable_matches_runtime_symbol(Some(func_bits), fn_key!(molt_type_call)) {
            let Some(self_bits) = self_bits else {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            };
            let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            };
            if object_type_id(self_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            }
            return call_type_with_arguments(_py, self_ptr, args);
        }
        if let Some(self_bits) = self_bits {
            // The argument vector owns the receiver like any other positional.
            if let Err(err) = args.prepend_positional(self_bits) {
                return err;
            }
        }
        call_function_with_arguments(_py, func_bits, func_ptr, args)
    }
}

/// CPython's CALL on a callable reference the call instruction owns: an
/// ordinary source call (`call_func`, `call_method`, the fallback leg of
/// `call_guarded`, and `call_bind`/`call_indirect` of a stack-form builder).
/// A bound method hands its receiver to the argument vector and ends before
/// its function runs, so a temporary bound method's receiver is then owned by
/// the callee frame alone. Any other callable ends after the call, once the
/// argument vector has ended, as `DECREF_INPUTS` releases the callable last.
/// `dispatch` runs the call on a callable this call keeps alive.
unsafe fn call_with_adopted_callable<'a, 'py>(
    py: &'a PyToken<'py>,
    callable_bits: u64,
    mut args: CallArguments<'a, 'py>,
    dispatch: impl FnOnce(u64, CallArguments<'a, 'py>) -> u64,
) -> u64 {
    unsafe {
        if let Some(method_ptr) = obj_from_bits(callable_bits).as_ptr()
            && object_type_id(method_ptr) == TYPE_ID_BOUND_METHOD
        {
            let func_bits = bound_method_func_bits(method_ptr);
            // The argument vector retains the receiver and the call holds the
            // function: releasing the bound method runs no finalizer of either.
            if let Err(err) = args.prepend_positional(bound_method_self_bits(method_ptr)) {
                drop(args);
                dec_ref_bits(py, callable_bits);
                return err;
            }
            inc_ref_bits(py, func_bits);
            dec_ref_bits(py, callable_bits);
            let result = dispatch(func_bits, args);
            dec_ref_bits(py, func_bits);
            return result;
        }
        let result = dispatch(callable_bits, args);
        dec_ref_bits(py, callable_bits);
        result
    }
}

/// An ordinary source call instruction (CPython's CALL) whose callable and
/// positional arguments this call now owns: `call_func`, `call_method` and the
/// fallback leg of `call_guarded`. A temporary bound method ends before its
/// function runs; the arguments move into an adopting frame or end as
/// `DECREF_INPUTS` once a borrowing callee returns. None of them ever returns
/// to the caller.
pub(crate) unsafe fn call_owned_arguments(
    _py: &PyToken<'_>,
    callable_bits: u64,
    positional: &[u64],
) -> u64 {
    unsafe {
        if exception_pending(_py) {
            // A pending error means argument preparation failed: the operands
            // end as the ended instruction's inputs, the callable last.
            release_stack_arguments(_py, None, positional);
            dec_ref_bits(_py, callable_bits);
            return MoltObject::none().bits();
        }
        if takes_positional_arguments_over(_py, callable_bits, positional.len()) {
            let result = call_function_obj_moved(_py, callable_bits, positional);
            dec_ref_bits(_py, callable_bits);
            return result;
        }
        // A bound method of such a function: its receiver moves into the
        // frame as `self` and the method ends before the function runs.
        if let Some(method_ptr) = obj_from_bits(callable_bits).as_ptr()
            && object_type_id(method_ptr) == TYPE_ID_BOUND_METHOD
        {
            let func_bits = bound_method_func_bits(method_ptr);
            let self_bits = bound_method_self_bits(method_ptr);
            if positional.len() < 16
                && takes_positional_arguments_over(_py, func_bits, positional.len() + 1)
            {
                inc_ref_bits(_py, self_bits);
                inc_ref_bits(_py, func_bits);
                dec_ref_bits(_py, callable_bits);
                let result = call_owned_function(_py, func_bits, Some(self_bits), positional);
                dec_ref_bits(_py, func_bits);
                return result;
            }
        }
        match CallArguments::moved(_py, None, positional) {
            Ok(arguments) => {
                call_with_adopted_callable(_py, callable_bits, arguments, |callable, arguments| {
                    call_bind_with_arguments(_py, callable, arguments)
                })
            }
            Err(err) => {
                dec_ref_bits(_py, callable_bits);
                err
            }
        }
    }
}
