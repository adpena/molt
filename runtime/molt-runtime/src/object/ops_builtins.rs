#[cfg(test)]
pub(crate) use builtin_collections::molt_sum_builtin_attached_body;
// Call dispatch, builtin wrappers, and type constructor builtins.
// Split from ops.rs for compilation-unit size reduction.

use crate::BuiltinExc;
use crate::builtins::frames::{
    CodeNamespace, CompiledCodeSlot, FrameInvocationGuard, take_invocation_namespace,
};
use crate::builtins::functions::runtime_callable_target_ptr;
use crate::object::ops_string::utf8_char_to_byte_index_cached;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::state::recursion::RecursionGuard;
use crate::*;
use molt_obj_model::MoltObject;
use num_integer::Integer;
use num_traits::{Signed, Zero};

use super::ops::float_result_bits;
use super::ops_sys::{decode_slice_bound, slice_error};

mod builtin_collections;
#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial"))]
pub(crate) use builtin_collections::molt_sorted_builtin_attached_body;
pub use {
    builtin_collections::molt_max_builtin, builtin_collections::molt_min_builtin,
    builtin_collections::molt_sorted_builtin, builtin_collections::molt_sum_builtin,
};

mod dir;
pub(crate) use dir::molt_dir_builtin_attached_body;
pub use {dir::molt_dir_builtin, dir::molt_object_dir_method, dir::molt_type_dir_method};

mod reduction;
pub use {
    reduction::molt_bytearray_reduce, reduction::molt_bytearray_reduce_ex,
    reduction::molt_bytes_getnewargs, reduction::molt_complex_getnewargs,
    reduction::molt_float_getnewargs, reduction::molt_int_getnewargs,
    reduction::molt_object_reduce, reduction::molt_object_reduce_ex, reduction::molt_set_reduce,
    reduction::molt_str_getnewargs, reduction::molt_tuple_getnewargs,
};

#[path = "ops_builtins_constructors.rs"]
mod ops_builtins_constructors;
pub use {
    ops_builtins_constructors::molt_aiter_builtin, ops_builtins_constructors::molt_bool_builtin,
    ops_builtins_constructors::molt_bytearray_builtin,
    ops_builtins_constructors::molt_bytes_builtin,
    ops_builtins_constructors::molt_classmethod_builtin,
    ops_builtins_constructors::molt_complex_builtin, ops_builtins_constructors::molt_dict_builtin,
    ops_builtins_constructors::molt_float_builtin,
    ops_builtins_constructors::molt_frozenset_builtin,
    ops_builtins_constructors::molt_hasattr_builtin, ops_builtins_constructors::molt_int_builtin,
    ops_builtins_constructors::molt_isinstance_builtin,
    ops_builtins_constructors::molt_issubclass_builtin,
    ops_builtins_constructors::molt_iter_builtin, ops_builtins_constructors::molt_list_builtin,
    ops_builtins_constructors::molt_object_builtin,
    ops_builtins_constructors::molt_property_builtin,
    ops_builtins_constructors::molt_range_builtin, ops_builtins_constructors::molt_set_builtin,
    ops_builtins_constructors::molt_slice_builtin,
    ops_builtins_constructors::molt_staticmethod_builtin,
    ops_builtins_constructors::molt_str_builtin, ops_builtins_constructors::molt_super_builtin,
    ops_builtins_constructors::molt_tuple_builtin, ops_builtins_constructors::molt_type_builtin,
};
pub(crate) use {
    ops_builtins_constructors::molt_bool_builtin_attached_body,
    ops_builtins_constructors::molt_list_builtin_attached_body,
};

// Cached debug/trace flags. Reading env vars on every call op (via libc
// `getenv`, which acquires a process-wide mutex) was the dominant cost in
// integer-loop benchmarks. Each flag is read exactly once at first use.

#[inline]
fn trace_enter_slot_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_TRACE_ENTER_SLOT)
}

#[inline]
fn trace_exit_pending_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_TRACE_EXIT_PENDING)
}

#[inline]
fn trace_call_dispatch_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_CALL_DISPATCH)
}

#[inline]
fn trace_call_dispatch_args_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_CALL_DISPATCH_ARGS)
}

pub(crate) fn molt_code_slots_init_attached_body(_py: &crate::PyToken<'_>, count: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if runtime_state(_py).code_slots.get().is_some() {
            return MoltObject::none().bits();
        }
        let Some(count) = usize::try_from(count).ok() else {
            return {
                crate::record_memory_error_without_allocation(_py);
                crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
            };
        };
        let slots = (0..count).map(|_| CompiledCodeSlot::default()).collect();
        let _ = runtime_state(_py).code_slots.set(slots);
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_code_slot_set_attached_body(
    _py: &crate::PyToken<'_>,
    code_id: u64,
    code_bits: u64,
    globals_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(slots) = runtime_state(_py).code_slots.get() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "code slots not initialized",
            );
        };
        let Some(idx) = usize::try_from(code_id).ok() else {
            return raise_exception::<_>(_py, BuiltinExc::IndexError, "code slot out of range");
        };
        if idx >= slots.len() {
            return raise_exception::<_>(_py, BuiltinExc::IndexError, "code slot out of range");
        }
        if let Some(ptr) = obj_from_bits(code_bits).as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_CODE {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::TypeError,
                        "code slot expects code object",
                    );
                }
            }
        } else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "code slot expects code object",
            );
        }
        let globals =
            match crate::builtins::frames::globals_namespace_storage_bits(_py, globals_bits) {
                Ok(storage) => storage,
                Err(()) => return MoltObject::none().bits(),
            };
        if globals.is_none() {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "code slot expects globals dictionary",
            );
        }
        if crate::builtins::frames::compiled_slot_for_code(code_bits)
            .is_some_and(|existing| existing != code_id)
        {
            return raise_exception::<_>(
                _py,
                BuiltinExc::SystemError,
                "code object cannot move between compiled slots",
            );
        }
        // Every synchronous activation of this body takes the frame plan its
        // code object implies on this target; derive it once, here.
        let plan = match crate::builtins::frames::FramePlan::for_code(_py, code_bits) {
            Ok(plan) => plan,
            Err(message) => return raise_exception::<_>(_py, BuiltinExc::SystemError, message),
        };
        unsafe {
            crate::object::layout::code_set_frame_slot_id(
                obj_from_bits(code_bits).as_ptr().unwrap(),
                code_id,
            );
        }
        slots[idx].replace(
            _py,
            CodeNamespace {
                code_bits,
                globals_bits,
            },
        );
        slots[idx].set_frame_plan(plan);
        MoltObject::none().bits()
    })
}

fn code_slot_acquire(_py: &PyToken<'_>, code_id: u64) -> Option<CodeNamespace> {
    let slots = runtime_state(_py).code_slots.get()?;
    let idx = usize::try_from(code_id).ok()?;
    let binding = slots.get(idx)?.acquire(_py);
    if binding.code_bits == 0 {
        binding.release(_py);
        return None;
    }
    Some(binding)
}

pub(crate) fn molt_trace_enter_slot_attached_body(_py: &crate::PyToken<'_>, code_id: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let handed = match take_invocation_namespace(_py, code_id) {
            Ok(handed) => handed,
            Err(()) => {
                TRACE_FRAME_PUSH_STACK.with(|stack| stack.borrow_mut().push(false));
                return MoltObject::none().bits();
            }
        };
        let (binding, builtins_bits, activation_bits) =
            if let Some([globals, builtins, code, activation]) = handed {
                // The invocation already owns every edge. Do not acquire and then
                // discard unrelated lexical owners from the mutable default slot.
                (
                    CodeNamespace {
                        code_bits: code,
                        globals_bits: globals,
                    },
                    builtins,
                    activation,
                )
            } else {
                let Some(binding) = code_slot_acquire(_py, code_id) else {
                    TRACE_FRAME_PUSH_STACK.with(|stack| stack.borrow_mut().push(false));
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::SystemError,
                        "compiled frame slot is unbound",
                    );
                };
                let builtins = match crate::builtins::frames::frame_effective_builtins_bits(
                    _py,
                    binding.globals_bits,
                ) {
                    Ok(builtins) => builtins,
                    Err(()) => {
                        binding.release(_py);
                        TRACE_FRAME_PUSH_STACK.with(|stack| stack.borrow_mut().push(false));
                        return MoltObject::none().bits();
                    }
                };
                inc_ref_bits(_py, builtins);
                (binding, builtins, 0)
            };
        let code_bits = binding.code_bits;
        if trace_enter_slot_enabled() {
            let mut name = "<none>".to_string();
            let mut file = "<none>".to_string();
            if code_bits != 0
                && let Some(ptr) = obj_from_bits(code_bits).as_ptr()
                && unsafe { object_type_id(ptr) == TYPE_ID_CODE }
            {
                let name_bits = unsafe { code_name_bits(ptr) };
                name = string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let file_bits = unsafe { code_filename_bits(ptr) };
                file = string_obj_to_owned(obj_from_bits(file_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
            }
            eprintln!(
                "molt trace_enter_slot code_id={} bits=0x{:x} name={} file={}",
                code_id, code_bits, name, file
            );
        }
        frame_stack_push_owned(
            _py,
            code_bits,
            binding.globals_bits,
            builtins_bits,
            activation_bits,
        );
        TRACE_FRAME_PUSH_STACK.with(|stack| stack.borrow_mut().push(true));
        // A synchronous activation of an optimized body keeps its bindings in
        // homes its code slot's plan sizes; a stateful activation's bindings
        // live in its task payload. The pushed entry is popped by this
        // invocation's exit even when the homes cannot be taken, and compiled
        // code borrows them only after this entry's exception check passed.
        let plan = if activation_bits == 0 {
            runtime_state(_py)
                .code_slots
                .get()
                .and_then(|slots| slots.get(usize::try_from(code_id).ok()?))
                .map(CompiledCodeSlot::frame_plan)
                .unwrap_or_default()
        } else {
            crate::builtins::frames::FramePlan::default()
        };
        if !crate::builtins::frames::frame_stack_enter_homes(_py, plan) {
            crate::record_memory_error_without_allocation(_py);
            return MoltObject::none().bits();
        }
        // The frame stack now owns the code edge; an owned-result sink must
        // never receive that borrowed edge as the result of this mutator.
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_trace_exit_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if trace_exit_pending_enabled()
            && let Some((kind, _)) = crate::builtins::exceptions::pending_exception_diagnostic(_py)
            && let Some((file, line, func, _, _)) =
                crate::builtins::frames::frame_stack_top_info(_py)
        {
            eprintln!(
                "molt trace_exit pending kind={} frame={} file={} line={}",
                kind, func, file, line
            );
        }
        let should_pop =
            TRACE_FRAME_PUSH_STACK.with(|stack| stack.borrow_mut().pop().unwrap_or(false));
        if should_pop {
            frame_stack_pop(_py);
        }
        MoltObject::none().bits()
    })
}

/// Outlined guarded-call helper: performs recursion guard enter/exit, optional
/// trace enter/exit, and the actual function call via function pointer dispatch.
/// Replaces the multi-block inline sequence previously generated for every
/// `call` op, eliminating ~3 Cranelift blocks and ~12 function-declaration/import
/// operations per call site.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub(crate) fn molt_guarded_call_attached_body(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    args_ptr: *const u64,
    nargs: u64,
) -> u64 {
    let Some(fn_target) = crate::provenance::abi::function_ptr(fn_ptr) else {
        return crate::with_attached_entry_nopanic!(_py, _py, {
            raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "call target address exceeds the active address space",
            )
        });
    };
    let Some(args) = (unsafe { crate::provenance::abi::slice(args_ptr, nargs) }) else {
        return crate::with_attached_entry_nopanic!(_py, _py, {
            raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "call argument range is invalid for the active target",
            )
        });
    };
    crate::with_attached_entry_nopanic!(_py, _py, {
        let _baseline = crate::call::ExceptionBaselineGuard::new();
        let Some(_recursion) = RecursionGuard::enter(_py) else {
            return MoltObject::none().bits();
        };
        unsafe { molt_guarded_call_dispatch(_py, fn_target, args.as_ptr(), args.len()) }
    })
}

/// Outlined guarded-call helper for dynamic dispatch paths where the callee
/// is identified by its object bits rather than a code slot id.
///
/// # Safety
///
/// `args_ptr` must reference `nargs` contiguous `u64` argument values for the
/// duration of this call.
pub(crate) unsafe fn molt_guarded_call_obj_attached_body(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    args_ptr: *const u64,
    nargs: u64,
    callee_bits: u64,
) -> u64 {
    let Some(fn_target) = crate::provenance::abi::function_ptr(fn_ptr) else {
        return crate::with_attached_entry_nopanic!(_py, _py, {
            raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "call target address exceeds the active address space",
            )
        });
    };
    let Some(args) = (unsafe { crate::provenance::abi::slice(args_ptr, nargs) }) else {
        return crate::with_attached_entry_nopanic!(_py, _py, {
            raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "call argument range is invalid for the active target",
            )
        });
    };
    crate::with_attached_entry_nopanic!(_py, _py, {
        let _baseline = crate::call::ExceptionBaselineGuard::new();
        let Some(_recursion) = RecursionGuard::enter(_py) else {
            return MoltObject::none().bits();
        };
        let _invocation = FrameInvocationGuard::for_callable(_py, callee_bits);
        unsafe { molt_guarded_call_dispatch(_py, fn_target, args.as_ptr(), args.len()) }
    })
}

/// Shared dispatch table: call fn_ptr with n arguments read from args_ptr.
///
/// # Safety
///
/// All `transmute` calls in this function share the same invariants:
///
/// 1. **fn_ptr is a valid function pointer.** The caller (compiled Python code or
///    `molt_call_func_dispatch`) guarantees that `fn_ptr` was obtained from a
///    `TYPE_ID_FUNCTION` object's code slot, which is populated by the compiler
///    backend when emitting WASM or native code. A zero/dangling fn_ptr here is
///    undefined behavior (segfault or worse).
///
/// 2. **Arity (`n`) matches the target function's actual parameter count.** The
///    compiler emits call sites with statically-known arity that must agree with
///    the function definition. Mismatch corrupts the stack (extern "C" ABI does
///    not check argument counts).
///
/// 3. **`args_ptr` points to a contiguous array of at least `n` u64 values.**
///    The caller allocates this on its own stack frame. Out-of-bounds reads are
///    UB.
///
/// 4. **The leading argument is the caller's attached execution context.**
///    The remaining arguments and the return value are MoltObject bit patterns (u64).
///    The extern "C" ABI used by every target function expects and returns u64
///    values encoding NaN-boxed Molt objects.
///
/// **Violation consequence:** Arity mismatch or invalid fn_ptr causes stack
/// corruption, segfault, or silent data corruption. There is no runtime guard;
/// correctness depends entirely on the compiler backend.
#[cfg(target_arch = "wasm32")]
#[inline(always)]
unsafe fn molt_guarded_call_dispatch_wasm(
    _py: &PyToken<'_>,
    call_target: *const (),
    args_ptr: *const u64,
    n: usize,
) -> Option<u64> {
    let func_idx = call_target as usize as u64;
    unsafe {
        Some(match n {
            0 => molt_call_indirect0(_py.attached_context(), func_idx) as u64,
            1 => molt_call_indirect1(_py.attached_context(), func_idx, *args_ptr) as u64,
            2 => molt_call_indirect2(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
            ) as u64,
            3 => molt_call_indirect3(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
            ) as u64,
            4 => molt_call_indirect4(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
            ) as u64,
            5 => molt_call_indirect5(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
            ) as u64,
            6 => molt_call_indirect6(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
            ) as u64,
            7 => molt_call_indirect7(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
            ) as u64,
            8 => molt_call_indirect8(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
                *args_ptr.add(7),
            ) as u64,
            9 => molt_call_indirect9(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
                *args_ptr.add(7),
                *args_ptr.add(8),
            ) as u64,
            10 => molt_call_indirect10(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
                *args_ptr.add(7),
                *args_ptr.add(8),
                *args_ptr.add(9),
            ) as u64,
            11 => molt_call_indirect11(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
                *args_ptr.add(7),
                *args_ptr.add(8),
                *args_ptr.add(9),
                *args_ptr.add(10),
            ) as u64,
            12 => molt_call_indirect12(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
                *args_ptr.add(7),
                *args_ptr.add(8),
                *args_ptr.add(9),
                *args_ptr.add(10),
                *args_ptr.add(11),
            ) as u64,
            13 => molt_call_indirect13(
                _py.attached_context(),
                func_idx,
                *args_ptr,
                *args_ptr.add(1),
                *args_ptr.add(2),
                *args_ptr.add(3),
                *args_ptr.add(4),
                *args_ptr.add(5),
                *args_ptr.add(6),
                *args_ptr.add(7),
                *args_ptr.add(8),
                *args_ptr.add(9),
                *args_ptr.add(10),
                *args_ptr.add(11),
                *args_ptr.add(12),
            ) as u64,
            _ => return None,
        })
    }
}

#[inline(never)]
unsafe fn molt_guarded_call_dispatch(
    _py: &PyToken<'_>,
    call_target: *const (),
    args_ptr: *const u64,
    n: usize,
) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(result) =
            unsafe { molt_guarded_call_dispatch_wasm(_py, call_target, args_ptr, n) }
        {
            return result;
        }
        crate::with_attached_entry_nopanic!(_py, _py, {
            raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "WASM indirect function call arity exceeds manifest call_indirect max",
            )
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    unsafe {
        match n {
            0 => {
                // SAFETY: transmute to 0-arity fn; see function-level invariants.
                let f: extern "C" fn(u64) -> u64 = std::mem::transmute(call_target);
                f(_py.attached_context())
            }
            1 => {
                // SAFETY: transmute to 1-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(call_target);
                f(_py.attached_context(), *args_ptr)
            }
            2 => {
                // SAFETY: transmute to 2-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(call_target);
                f(_py.attached_context(), *args_ptr, *args_ptr.add(1))
            }
            3 => {
                // SAFETY: transmute to 3-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                )
            }
            4 => {
                // SAFETY: transmute to 4-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                )
            }
            5 => {
                // SAFETY: transmute to 5-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                )
            }
            6 => {
                // SAFETY: transmute to 6-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                )
            }
            7 => {
                // SAFETY: transmute to 7-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                )
            }
            8 => {
                // SAFETY: transmute to 8-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                )
            }
            9 => {
                // SAFETY: transmute to 9-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                )
            }
            10 => {
                // SAFETY: transmute to 10-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                )
            }
            11 => {
                // SAFETY: transmute to 11-arity fn; see function-level invariants.
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                    *args_ptr.add(10),
                )
            }
            12 => {
                // SAFETY: transmute to 12-arity fn; see function-level invariants.
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                    *args_ptr.add(10),
                    *args_ptr.add(11),
                )
            }
            13 => {
                // SAFETY: transmute to 13-arity fn; see function-level invariants.
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                    *args_ptr.add(10),
                    *args_ptr.add(11),
                    *args_ptr.add(12),
                )
            }
            14 => {
                // SAFETY: transmute to 14-arity fn; see function-level invariants.
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                    *args_ptr.add(10),
                    *args_ptr.add(11),
                    *args_ptr.add(12),
                    *args_ptr.add(13),
                )
            }
            15 => {
                // SAFETY: transmute to 15-arity fn; see function-level invariants.
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                    *args_ptr.add(10),
                    *args_ptr.add(11),
                    *args_ptr.add(12),
                    *args_ptr.add(13),
                    *args_ptr.add(14),
                )
            }
            16 => {
                // SAFETY: transmute to 16-arity fn; see function-level invariants.
                let f: extern "C" fn(
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                    u64,
                ) -> u64 = std::mem::transmute(call_target);
                f(
                    _py.attached_context(),
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                    *args_ptr.add(5),
                    *args_ptr.add(6),
                    *args_ptr.add(7),
                    *args_ptr.add(8),
                    *args_ptr.add(9),
                    *args_ptr.add(10),
                    *args_ptr.add(11),
                    *args_ptr.add(12),
                    *args_ptr.add(13),
                    *args_ptr.add(14),
                    *args_ptr.add(15),
                )
            }
            _ => {
                // Arity > 16: raise a clear error instead of silently failing.
                // This path is only reachable if a function genuinely has 17+
                // parameters AND is called via the direct fn_ptr dispatch table.
                // molt_call_func_dispatch handles arbitrary arities via callargs,
                // so this should never be reached in practice.
                crate::with_attached_entry_nopanic!(_py, _py, {
                    raise_exception::<u64>(
                        _py,
                        BuiltinExc::RuntimeError,
                        &format!(
                            "direct dispatch does not support {} arguments; \
                             use callargs dispatch for functions with >16 parameters",
                            n
                        ),
                    )
                })
            }
        }
    }
}

/// Invoke a borrowed Python callable through the canonical argument binder.
/// The argument buffer contains visible Python arguments, never packed ABI slots.
/// `code_id` identifies this call site in optional dispatch tracing.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub(crate) fn molt_call_func_dispatch_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    args_ptr_bits: u64, // u64 to match WASM all-i64 ABI; cast to *const u64 below
    nargs: u64,
    code_id: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        // A callback can reuse the WASM argument scratch area. Stabilize its
        // values before admission, using the same buffer as the owned entry.
        let Some(args) = (unsafe { crate::call::bind::EntryArguments::copy(args_ptr_bits, nargs) })
        else {
            return raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "call argument range is invalid for the active target",
            );
        };
        let raw_args = args.as_slice();
        if trace_call_dispatch_enabled() {
            eprintln!(
                "[molt dispatch] call_func_dispatch callable_type={} nargs={} code_id={code_id}",
                crate::type_name(_py, obj_from_bits(func_bits)),
                raw_args.len(),
            );
            if trace_call_dispatch_args_enabled() {
                for (idx, &bits) in raw_args.iter().enumerate() {
                    eprintln!(
                        "  [molt dispatch arg] idx={idx} type={} bits=0x{bits:x}",
                        crate::type_name(_py, obj_from_bits(bits)),
                    );
                }
            }
        }
        unsafe {
            if obj_from_bits(func_bits)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_FUNCTION)
            {
                crate::call::function::call_function_obj_vec(_py, func_bits, raw_args)
            } else {
                crate::call::bind::call_bind_borrowed(_py, func_bits, None, raw_args, &[], &[])
            }
        }
    })
}

/// An ordinary source call instruction (CPython's CALL) that adopted its
/// callable and its positional arguments (`argument_custody`): `call_func`,
/// `call_method` and the fallback leg of `call_guarded`. `func_bits` and every
/// reference in `args_ptr[..nargs]` belong to this call: a temporary bound
/// method ends before its function runs, an adopting frame takes the
/// arguments over, and a borrowing callee borrows them until the call releases
/// them as CPython's `DECREF_INPUTS` does, its callable last. The caller never
/// releases any of them. The signature mirrors `molt_call_func_dispatch`, so
/// both lanes share one machine type; `code_id` is the same optional site
/// identity, which the owned lane does not consult.
///
/// Arguments:
///   func_bits: the adopted callable
///   args_ptr: pointer to the adopted argument bits (spilled by the caller)
///   nargs: number of arguments
///   code_id: call-site identity, 0 when absent
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub(crate) fn molt_call_func_owned_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    args_ptr_bits: u64, // u64 to match the WASM all-i64 ABI
    nargs: u64,
    code_id: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let _ = code_id;
        let Some(args) = (unsafe { crate::call::bind::EntryArguments::copy(args_ptr_bits, nargs) })
        else {
            // The arguments are unreachable; the callable still ends here.
            dec_ref_bits(_py, func_bits);
            return raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "call argument range is invalid for the active target",
            );
        };
        unsafe { crate::call::bind::call_owned_arguments(_py, func_bits, args.as_slice()) }
    })
}

/// A compiled call instruction's adopted inputs that no callee took over:
/// once a borrowing callee returns, or on a failure before invocation
/// (recursion or frame admission). They end as CALL's `DECREF_INPUTS` ends
/// them: `args_ptr[..nargs]` in the target version's order, then
/// `callable_bits` unless it is zero. Every backend releases through this one
/// order authority instead of spelling its own.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub(crate) fn molt_call_inputs_release_attached_body(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    args_ptr_bits: u64,
    nargs: u64,
) {
    crate::with_attached_entry_nopanic!(_py, _py, {
        match unsafe { crate::call::bind::EntryArguments::copy(args_ptr_bits, nargs) } {
            Some(args) => crate::call::bind::release_stack_arguments(_py, None, args.as_slice()),
            None => {
                let _ = raise_exception::<u64>(
                    _py,
                    BuiltinExc::RuntimeError,
                    "call argument range is invalid for the active target",
                );
            }
        }
        if callable_bits != 0 {
            dec_ref_bits(_py, callable_bits);
        }
    })
}

/// Execute already-bound fixed ABI slots with one recursion/frame boundary.
pub(crate) fn molt_call_func_direct(
    _py: &crate::concurrency::PyToken<'_>,
    fn_ptr: u64,
    args: &[u64],
    _code_id: u64,
    callable_bits: u64,
) -> u64 {
    let _baseline = crate::call::ExceptionBaselineGuard::new();
    let Some(call_target) = (unsafe { direct_call_target_for_callable(callable_bits, fn_ptr) })
    else {
        return missing_direct_call_target(_py, fn_ptr);
    };
    let Some(_recursion) = RecursionGuard::enter(_py) else {
        return MoltObject::none().bits();
    };
    let _invocation = FrameInvocationGuard::for_callable(_py, callable_bits);
    unsafe { molt_guarded_call_dispatch(_py, call_target, args.as_ptr(), args.len()) }
}

/// Ultra-fast inline dispatch for `call_func` with known small arities.
///
/// These functions receive args as register values (no stack spill/reload),
/// skip GIL re-acquisition (caller already holds it in the compiled code
/// context), and do a minimal type check + direct fn_ptr call.
///
/// Fast path: func_bits is a non-closure TYPE_ID_FUNCTION with exact arity.
/// Slow path: falls back to the full `molt_call_func_dispatch`.
/// Direct fn_ptr call for exactly 0 args — fully inlined, no match dispatch.
///
/// # Safety
///
/// `fn_ptr` must be a valid pointer to an `extern "C" fn(u64) -> u64` function.
/// The caller (`probe_simple_func` fast path) verifies the target is a
/// `TYPE_ID_FUNCTION` with matching arity before reaching here.
/// Invalid fn_ptr causes a segfault or stack corruption.
#[inline(always)]
unsafe fn direct_call_target_for_function(func_ptr: *mut u8, fn_ptr: u64) -> Option<*const ()> {
    let target = unsafe { crate::object::layout::function_call_target_ptr(func_ptr) };
    if !target.is_null() {
        return Some(target);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let tramp_ptr = unsafe { function_trampoline_ptr(func_ptr) };
        let call_target = crate::call::function::fixed_arity_call_target_ptr(fn_ptr, tramp_ptr);
        crate::provenance::abi::function_ptr(call_target)
    }
    #[cfg(not(target_arch = "wasm32"))]
    runtime_callable_target_ptr(fn_ptr)
}

#[inline(always)]
unsafe fn direct_call_target_for_callable(callable_bits: u64, fn_ptr: u64) -> Option<*const ()> {
    let ptr = obj_from_bits(callable_bits).as_ptr()?;
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_FUNCTION => direct_call_target_for_function(ptr, fn_ptr),
            TYPE_ID_BOUND_METHOD => {
                let func_bits = bound_method_func_bits(ptr);
                let func_ptr = obj_from_bits(func_bits).as_ptr()?;
                if object_type_id(func_ptr) == TYPE_ID_FUNCTION {
                    direct_call_target_for_function(func_ptr, fn_ptr)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

fn missing_direct_call_target(_py: &crate::concurrency::PyToken<'_>, fn_ptr: u64) -> u64 {
    let msg = format!("direct function call target 0x{fn_ptr:x} is not initialized");
    raise_exception::<u64>(_py, BuiltinExc::RuntimeError, &msg)
}

#[inline(always)]
unsafe fn direct_call_0(_py: &PyToken<'_>, call_target: *const ()) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { molt_call_indirect0(_py.attached_context(), call_target as usize as u64) as u64 }
    }
    #[cfg(not(target_arch = "wasm32"))]
    unsafe {
        let f: extern "C" fn(u64) -> u64 = std::mem::transmute(call_target);
        f(_py.attached_context())
    }
}

/// Probe the callable: if it's a non-closure function with matching arity and
/// a materialized call target, return its identity and executable target.
#[inline(always)]
unsafe fn probe_simple_func(
    _py: &crate::concurrency::PyToken<'_>,
    func_bits: u64,
    expected_arity: usize,
) -> Option<(u64, *const ())> {
    unsafe {
        let ptr = direct_call_function(_py, func_bits, expected_arity, false, false)?;
        if function_trampoline_ptr(ptr) != 0 {
            return None;
        }
        let fn_ptr = function_fn_ptr(ptr);
        let call_target = direct_call_target_for_function(ptr, fn_ptr)?;
        Some((fn_ptr, call_target))
    }
}

/// Admit a raw compiled call only after its Python shape, execution kind and
/// argument custody agree with the call site: a borrowing call site may only
/// reach a borrowing entry, and an adopting one only an adopting entry. Any
/// other entry is reached through the runtime's invocation authority.
/// Backends must not reproduce this classifier.
unsafe fn direct_call_function(
    py: &PyToken<'_>,
    func_bits: u64,
    supplied: usize,
    expects_closure: bool,
    expects_adoption: bool,
) -> Option<*mut u8> {
    unsafe {
        let ptr = obj_from_bits(func_bits).as_ptr()?;
        if object_type_id(ptr) != TYPE_ID_FUNCTION
            || function_arity_usize(ptr)? != supplied
            || function_has_execution_closure(ptr) != expects_closure
            || (crate::object::layout::function_entry_custody(ptr)
                == crate::object::layout::EntryCustody::Adopting)
                != expects_adoption
            || crate::call::bind::function_raw_positional_call_needs_binding(py, ptr, supplied)
        {
            return None;
        }
        match crate::call::function::function_code_execution_kind(ptr) {
            Some(crate::object::layout::CodeExecutionKind::Direct) => {}
            Some(_) => return None,
            None if function_trampoline_ptr(ptr) != 0 => return None,
            None => {}
        }
        Some(ptr)
    }
}

/// Whether compiled code may call `func_bits`'s direct entry itself. `shape`
/// describes the call site: bit 0 is set when the entry takes an execution
/// closure first, and bit 1 when the call instruction adopted its arguments.
/// No other bit occurs; a malformed shape is never eligible.
pub(crate) fn molt_function_direct_call_eligible_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    supplied: u64,
    shape: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Ok(supplied) = usize::try_from(supplied) else {
            return 0;
        };
        if shape & !0b11 != 0 {
            return 0;
        }
        u64::from(unsafe {
            direct_call_function(
                _py,
                func_bits,
                supplied,
                shape & 0b01 != 0,
                shape & 0b10 != 0,
            )
            .is_some()
        })
    })
}

#[cfg(test)]
mod direct_call_tests {
    use super::*;
    use crate::object::layout::{
        CodeExecutionKind, code_publish_execution_kind, function_set_closure_bits,
    };

    static ADOPTING_ENTRY_REFS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    extern "C" fn adopting_identity_entry(context: u64, argument: u64) -> u64 {
        assert_eq!(context, crate::state::molt_attached_context());
        let _py = unsafe { crate::PyToken::from_attached_context(context) };
        let references = obj_from_bits(argument).as_ptr().map_or(0, |ptr| unsafe {
            (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot()
        });
        ADOPTING_ENTRY_REFS.store(references, std::sync::atomic::Ordering::SeqCst);
        // The adopted argument edge becomes the owned return edge.
        argument
    }

    extern "C" fn adopting_identity_trampoline(
        context: u64,
        _closure: u64,
        argv: u64,
        _argc: u64,
    ) -> i64 {
        assert_eq!(context, crate::state::molt_attached_context());
        let _py = unsafe { crate::PyToken::from_attached_context(context) };
        adopting_identity_entry(context, unsafe { *(argv as *const u64) }) as i64
    }

    #[test]
    fn admission_tracks_actual_shape_binding_and_code_kind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let function = alloc_function_obj(py, 17, 1);
                assert!(!function.is_null());
                let bits = MoltObject::from_ptr(function).bits();
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                    1
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 0, 0),
                    0
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 2, 0),
                    0
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 1),
                    0
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(
                        py,
                        MoltObject::none().bits(),
                        0,
                        0
                    ),
                    0
                );

                let empty = alloc_tuple(py, &[]);
                assert!(!empty.is_null());
                let empty_bits = MoltObject::from_ptr(empty).bits();
                function_set_closure_bits(
                    py,
                    function,
                    empty_bits,
                    crate::FunctionCallAbi::OpaqueContextFirst,
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                    0
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 1),
                    1
                );
                function_set_closure_bits(
                    py,
                    function,
                    empty_bits,
                    crate::FunctionCallAbi::Positional,
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                    1
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 1),
                    0
                );
                function_set_closure_bits(py, function, 0, crate::FunctionCallAbi::Positional);

                let name_ptr = alloc_string(py, b"raw-call-shape");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                for kind in [
                    CodeExecutionKind::Generator,
                    CodeExecutionKind::Coroutine,
                    CodeExecutionKind::AsyncGenerator,
                    CodeExecutionKind::Direct,
                ] {
                    let code = alloc_code_obj(
                        py,
                        name_bits,
                        name_bits,
                        1,
                        MoltObject::none().bits(),
                        empty_bits,
                        empty_bits,
                        1,
                        0,
                        0,
                    );
                    assert!(!code.is_null());
                    let code_bits = MoltObject::from_ptr(code).bits();
                    assert_eq!(code_publish_execution_kind(code, kind), Ok(()));
                    assert!(function_set_code_bits(py, function, code_bits));
                    assert_eq!(
                        molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                        u64::from(kind == CodeExecutionKind::Direct)
                    );
                    dec_ref_bits(py, code_bits);
                }
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                    1
                );

                let vararg_name = dynamic_attr_name_bits(py, b"__molt_vararg__").unwrap();
                assert!(crate::call::class_init::function_set_attr_bits(
                    py,
                    function,
                    vararg_name,
                    name_bits
                ));
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                    0
                );
                assert!(crate::call::class_init::function_set_attr_bits(
                    py,
                    function,
                    vararg_name,
                    MoltObject::none().bits()
                ));
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0),
                    1
                );

                dec_ref_bits(py, bits);
                dec_ref_bits(py, name_bits);
                dec_ref_bits(py, empty_bits);
            }
        });
    }

    #[test]
    fn direct_entry_admission_requires_matching_custody() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let function = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    adopting_identity_entry as *const () as usize as u64,
                    1,
                );
                assert!(!function.is_null());
                let bits = MoltObject::from_ptr(function).bits();
                // A borrowing entry admits only a borrowing call site.
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b00),
                    1
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b10),
                    0
                );
                assert_eq!(
                    crate::object::layout::function_publish_entry_custody(
                        function,
                        crate::object::layout::EntryCustody::Adopting,
                    ),
                    Err("an adopting entry needs a trampoline for borrowed invocation")
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b00),
                    1
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b10),
                    0
                );
                dec_ref_bits(py, bits);

                let bits = crate::builtins::functions::molt_func_new_attached_body(
                    py,
                    adopting_identity_entry as *const () as usize as u64,
                    adopting_identity_trampoline as *const () as usize as u64,
                    1,
                    molt_codegen_abi::ENTRY_CUSTODY_ADOPTS,
                );
                assert!(!exception_pending(py));
                let function = obj_from_bits(bits).as_ptr().expect("compiled function");
                // A trampoline alone does not certify a direct execution kind.
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b10),
                    0
                );
                let name = alloc_string(py, b"custody_argument");
                let empty = alloc_tuple(py, &[]);
                assert!(!name.is_null() && !empty.is_null());
                let name_bits = MoltObject::from_ptr(name).bits();
                let empty_bits = MoltObject::from_ptr(empty).bits();
                let varnames = alloc_tuple(py, &[name_bits]);
                assert!(!varnames.is_null());
                let varnames_bits = MoltObject::from_ptr(varnames).bits();
                let code = alloc_code_obj(
                    py,
                    name_bits,
                    name_bits,
                    1,
                    MoltObject::none().bits(),
                    varnames_bits,
                    empty_bits,
                    1,
                    0,
                    0,
                );
                assert!(!code.is_null());
                let code_bits = MoltObject::from_ptr(code).bits();
                assert_eq!(
                    code_publish_execution_kind(code, CodeExecutionKind::Direct),
                    Ok(())
                );
                assert!(function_set_code_bits(py, function, code_bits));
                dec_ref_bits(py, code_bits);
                // An adopting entry admits only an adopting call site.
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b00),
                    0
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b10),
                    1
                );
                assert_eq!(
                    molt_function_direct_call_eligible_attached_body(py, bits, 1, 0b100),
                    0
                );

                let argument = alloc_list(py, &[]);
                assert!(!argument.is_null());
                let argument_bits = MoltObject::from_ptr(argument).bits();
                let references =
                    (*crate::object::header_from_obj_ptr(argument)).ref_count_snapshot();
                let result =
                    crate::call::function::call_function_obj_trampoline(py, bits, &[argument_bits]);
                assert!(!exception_pending(py));
                assert_eq!(result, argument_bits);
                assert_eq!(
                    ADOPTING_ENTRY_REFS.load(std::sync::atomic::Ordering::SeqCst),
                    references + 1,
                    "the borrowed invocation must supply exactly one adopted edge"
                );
                assert_eq!(
                    (*crate::object::header_from_obj_ptr(argument)).ref_count_snapshot(),
                    references + 1,
                    "the adopted edge must remain owned by the return value"
                );
                dec_ref_bits(py, result);
                assert_eq!(
                    (*crate::object::header_from_obj_ptr(argument)).ref_count_snapshot(),
                    references,
                    "releasing the result must discharge only the return edge"
                );
                dec_ref_bits(py, argument_bits);
                dec_ref_bits(py, bits);
                dec_ref_bits(py, varnames_bits);
                dec_ref_bits(py, empty_bits);
                dec_ref_bits(py, name_bits);
                assert!(!exception_pending(py));
            }
        });
    }
}

/// Fast 0-argument function call. No args — minimal dispatch.
pub(crate) fn molt_call_func_fast0_attached_body(_py: &crate::PyToken<'_>, func_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let _baseline = crate::call::ExceptionBaselineGuard::new();
        unsafe {
            if let Some((_fn_ptr, call_target)) = probe_simple_func(_py, func_bits, 0) {
                let Some(_recursion) = RecursionGuard::enter(_py) else {
                    return MoltObject::none().bits();
                };
                let _invocation = FrameInvocationGuard::for_callable(_py, func_bits);
                return direct_call_0(_py, call_target);
            }
            let args: [u64; 0] = [];
            molt_call_func_dispatch_attached_body(_py, func_bits, args.as_ptr() as u64, 0, 0)
        }
    })
}

/// Fast 1-argument function call. Args passed in registers — no stack spill.
pub(crate) fn molt_call_func_fast1_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    a0: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        unsafe {
            if let Some((fn_ptr, _call_target)) = probe_simple_func(_py, func_bits, 1) {
                if runtime_callable_target_ptr(fn_ptr).is_some() {
                    return crate::call::function::call_function_obj1(_py, func_bits, a0);
                }
                let args = [a0];
                return molt_call_func_direct(_py, fn_ptr, &args, 0, func_bits);
            }
            let args = [a0];
            molt_call_func_dispatch_attached_body(_py, func_bits, args.as_ptr() as u64, 1, 0)
        }
    })
}

/// Fast 2-argument function call. Args passed in registers — no stack spill.
pub(crate) fn molt_call_func_fast2_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    a0: u64,
    a1: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        unsafe {
            if let Some((fn_ptr, _call_target)) = probe_simple_func(_py, func_bits, 2) {
                if runtime_callable_target_ptr(fn_ptr).is_some() {
                    return crate::call::function::call_function_obj2(_py, func_bits, a0, a1);
                }
                let args = [a0, a1];
                return molt_call_func_direct(_py, fn_ptr, &args, 0, func_bits);
            }
            let args = [a0, a1];
            molt_call_func_dispatch_attached_body(_py, func_bits, args.as_ptr() as u64, 2, 0)
        }
    })
}

/// Fast 3-argument function call. Args passed in registers — no stack spill.
pub(crate) fn molt_call_func_fast3_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
    a0: u64,
    a1: u64,
    a2: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        unsafe {
            if let Some((fn_ptr, _call_target)) = probe_simple_func(_py, func_bits, 3) {
                if runtime_callable_target_ptr(fn_ptr).is_some() {
                    return crate::call::function::call_function_obj3(_py, func_bits, a0, a1, a2);
                }
                let args = [a0, a1, a2];
                return molt_call_func_direct(_py, fn_ptr, &args, 0, func_bits);
            }
            let args = [a0, a1, a2];
            molt_call_func_dispatch_attached_body(_py, func_bits, args.as_ptr() as u64, 3, 0)
        }
    })
}

pub(crate) fn molt_trace_set_line_attached_body(_py: &crate::PyToken<'_>, line_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let line_obj = obj_from_bits(line_bits);
        let line = if line_obj.is_int() || line_obj.is_bool() {
            to_i64(line_obj).unwrap_or(0)
        } else {
            line_bits as i64
        };
        if trace_line_enabled() {
            eprintln!("MOLT_TRACE_LINE {}", line);
        }
        if trace_line_pending_enabled()
            && let Some((kind, detail)) =
                crate::builtins::exceptions::pending_exception_diagnostic(_py)
        {
            eprintln!("MOLT_TRACE_LINE_PENDING {} {} {}", line, kind, detail);
        }
        MoltObject::none().bits()
    })
}

fn trace_line_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_LINE)
}

fn trace_line_pending_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_LINE_PENDING)
}

pub(crate) fn molt_repr_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        molt_repr_from_obj_attached_body(_py, val_bits)
    })
}

pub(crate) fn molt_format_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let obj = obj_from_bits(val_bits);
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec_ptr) = spec_obj.as_ptr() else {
            let msg = format!(
                "format() argument 2 must be str, not {}",
                type_name(_py, spec_obj)
            );
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
        };
        unsafe {
            if object_type_id(spec_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "format() argument 2 must be str, not {}",
                    type_name(_py, spec_obj)
                );
                return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
            }
        }
        if let Some(rendered) = super::ops_format::format_override(_py, obj, spec_bits) {
            return rendered;
        }
        let supports_format = crate::builtins::numbers::index_integral_payload_bits(val_bits)
            .is_some()
            || crate::object::ops::as_float_extended(obj).is_some()
            || obj
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
                .unwrap_or(false)
            || obj
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_COMPLEX })
                .unwrap_or(false);
        if supports_format {
            return molt_string_format_attached_body(_py, val_bits, spec_bits);
        }
        if unsafe { string_len(spec_ptr) } == 0 {
            return molt_str_from_obj_attached_body(_py, val_bits);
        }
        let type_label = type_name(_py, obj);
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_callable_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { molt_is_callable_attached_body(_py, val_bits) })
}

pub(crate) fn molt_round_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    ndigits_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        let has_ndigits = ndigits_bits != missing;
        let has_ndigits_bits = MoltObject::from_bool(has_ndigits).bits();
        let ndigits = if has_ndigits {
            ndigits_bits
        } else {
            MoltObject::none().bits()
        };
        molt_round_attached_body(_py, val_bits, ndigits, has_ndigits_bits)
    })
}

pub(crate) fn molt_any_builtin_attached_body(_py: &crate::PyToken<'_>, iter_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(mut iterator) = crate::object::iterable::OwnedIterator::new(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        loop {
            let value = match iterator.next() {
                Ok(Some(value)) => value,
                Ok(None) => return MoltObject::from_bool(false).bits(),
                Err(_) => return MoltObject::none().bits(),
            };
            let truth = is_truthy(_py, obj_from_bits(value));
            dec_ref_bits(_py, value);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if truth {
                return MoltObject::from_bool(true).bits();
            }
        }
    })
}

pub(crate) fn molt_all_builtin_attached_body(_py: &crate::PyToken<'_>, iter_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(mut iterator) = crate::object::iterable::OwnedIterator::new(_py, iter_bits) else {
            return MoltObject::none().bits();
        };
        loop {
            let value = match iterator.next() {
                Ok(Some(value)) => value,
                Ok(None) => return MoltObject::from_bool(true).bits(),
                Err(_) => return MoltObject::none().bits(),
            };
            let truth = is_truthy(_py, obj_from_bits(value));
            dec_ref_bits(_py, value);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !truth {
                return MoltObject::from_bool(false).bits();
            }
        }
    })
}

pub(crate) fn molt_abs_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let obj = obj_from_bits(val_bits);
        if let Some(result) =
            super::ops_arith::unary_subtype_result(_py, obj, crate::NameId::DunderAbs)
        {
            return result;
        }
        if let Some(i) = index_i64_integral_bits(obj.bits()) {
            return int_bits_from_i128(_py, (i as i128).abs());
        }
        if let Some(big) = crate::builtins::numbers::index_bigint_integral_bits(obj.bits()) {
            let abs_val = big.abs();
            if let Some(i) = bigint_to_inline(&abs_val) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, abs_val);
        }
        if let Some(f) = to_f64(obj) {
            return float_result_bits(_py, f.abs());
        }
        if let Some(ptr) = complex_ptr_from_bits(val_bits)
            && unsafe { crate::object::iterable::builtin_receiver(_py, ptr) }
        {
            let value = unsafe { *complex_ref(ptr) };
            return float_result_bits(_py, value.re.hypot(value.im));
        }
        if let Some(method) = unsafe {
            match crate::builtins::attr::lookup_special_method(
                _py,
                val_bits,
                crate::NameId::DunderAbs,
            )
            .into_result()
            {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            }
        } {
            let result = unsafe { call_callable0(_py, method) };
            dec_ref_bits(_py, method);
            return result;
        }
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("bad operand type for abs(): '{type_name}'");
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_divmod_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    a_bits: u64,
    b_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let lhs = obj_from_bits(a_bits);
        let rhs = obj_from_bits(b_bits);
        if let Some(result) = super::ops_arith::numeric_subtype_binary_result(
            _py,
            lhs,
            rhs,
            crate::NameId::DunderDivmod,
            crate::NameId::DunderRdivmod,
            "divmod()",
        ) {
            return result;
        }

        // Integer-only projections select the integer arithmetic family.
        if let (Some(li), Some(ri)) = (
            index_i64_integral_bits(lhs.bits()),
            index_i64_integral_bits(rhs.bits()),
        ) {
            if ri == 0 {
                return super::ops_arith::raise_numeric_error(_py, molt_runtime_core::numeric_error_policy_generated::NumericErrorContext::IntegerDivmod);
            }
            let (quot, rem) =
                molt_runtime_core::numeric_error_policy_generated::python_integer_divmod(
                    i128::from(li),
                    i128::from(ri),
                )
                .expect("nonzero i64 operands fit i128 division");
            let q_bits = int_bits_from_i128(_py, quot);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let r_bits = int_bits_from_i128(_py, rem);
            if exception_pending(_py) {
                dec_ref_bits(_py, q_bits);
                return MoltObject::none().bits();
            }
            let tuple_ptr = crate::object::builders::alloc_tuple_owned(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                dec_ref_bits(_py, r_bits);
                dec_ref_bits(_py, q_bits);
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        if let (Some(l_big), Some(r_big)) = (
            crate::builtins::numbers::index_bigint_integral_bits(lhs.bits()),
            crate::builtins::numbers::index_bigint_integral_bits(rhs.bits()),
        ) {
            if r_big.is_zero() {
                return super::ops_arith::raise_numeric_error(_py, molt_runtime_core::numeric_error_policy_generated::NumericErrorContext::IntegerDivmod);
            }
            let quot = l_big.div_floor(&r_big);
            let rem = l_big.mod_floor(&r_big);
            let q_bits = if let Some(i) = bigint_to_inline(&quot) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, quot)
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let r_bits = if let Some(i) = bigint_to_inline(&rem) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, rem)
            };
            if exception_pending(_py) {
                dec_ref_bits(_py, q_bits);
                return MoltObject::none().bits();
            }
            let tuple_ptr = crate::object::builders::alloc_tuple_owned(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                dec_ref_bits(_py, r_bits);
                dec_ref_bits(_py, q_bits);
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        match float_pair_from_obj(_py, lhs, rhs) {
            Ok(Some((lf, rf))) => {
                if rf == 0.0 {
                    return super::ops_arith::raise_numeric_error(
                        _py,
                        molt_runtime_core::numeric_error_policy_generated::NumericErrorContext::FloatDivmod,
                    );
                }
                let (quot, rem) =
                    molt_runtime_core::numeric_error_policy_generated::python_float_divmod(lf, rf)
                        .expect("nonzero divisor checked");
                let q_bits = float_result_bits(_py, quot);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let r_bits = float_result_bits(_py, rem);
                if exception_pending(_py) {
                    dec_ref_bits(_py, q_bits);
                    return MoltObject::none().bits();
                }
                let tuple_ptr = crate::object::builders::alloc_tuple_owned(_py, &[q_bits, r_bits]);
                if tuple_ptr.is_null() {
                    dec_ref_bits(_py, r_bits);
                    dec_ref_bits(_py, q_bits);
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            Err(()) => return MoltObject::none().bits(),
            Ok(None) => {}
        }
        let left = class_name_for_error(type_of_bits(_py, a_bits));
        let right = class_name_for_error(type_of_bits(_py, b_bits));
        let msg = format!("unsupported operand type(s) for divmod(): '{left}' and '{right}'");
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_getattr_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if default_bits == missing {
            return molt_get_attr_name_attached_body(_py, obj_bits, name_bits);
        }
        molt_get_attr_name_default_attached_body(_py, obj_bits, name_bits, default_bits)
    })
}

/// Python `setattr(obj, name, value)` builtin.
pub(crate) fn molt_setattr_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        molt_set_attr_name_attached_body(_py, obj_bits, name_bits, val_bits);
        MoltObject::none().bits()
    })
}

/// Python `delattr(obj, name)` builtin.
pub(crate) fn molt_delattr_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let res = molt_del_attr_name_attached_body(_py, obj_bits, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res
    })
}

pub(crate) fn molt_vars_builtin_attached_body(_py: &crate::PyToken<'_>, obj_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython parity: vars() == locals() when called with no arguments.
            // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
            // re-entrant and uses the existing token.
            return crate::molt_locals_builtin_attached_body(_py);
        }
        let dict_name_bits = crate::NameId::DunderDict.bits(_py);
        match lookup_attribute(_py, obj_bits, dict_name_bits) {
            AttributeLookup::Found(bits) => bits,
            AttributeLookup::Error => MoltObject::none().bits(),
            AttributeLookup::Missing => raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "vars() argument must have __dict__ attribute",
            ),
        }
    })
}

pub(crate) fn molt_object_getstate_attached_body(_py: &crate::PyToken<'_>, _self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        object_getstate_default(_py, _self_bits, false)
    })
}

// CPython typeobject.c object_getstate_default: reduction requires that a
// default state can represent the complete instance, not an opaque payload.
// Explicit object.__getstate__ calls do not impose reconstruction admission.
fn object_getstate_default(_py: &crate::PyToken<'_>, _self_bits: u64, required: bool) -> u64 {
    let Some(ptr) = obj_from_bits(_self_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    let type_id = unsafe { object_type_id(ptr) };
    if required && !matches!(type_id, crate::TYPE_ID_OBJECT) {
        return crate::raise_exception::<u64>(
            _py,
            crate::BuiltinExc::TypeError,
            &format!(
                "cannot pickle '{}' object",
                crate::type_name(_py, obj_from_bits(_self_bits))
            ),
        );
    }
    if !unsafe { crate::object::object_has_class_shape(ptr) }
        && !unsafe { crate::object::native_instance::has_fields(ptr) }
    {
        return MoltObject::none().bits();
    }

    // 1. Collect __dict__ entries.
    let mut dict_state_bits: Option<u64> = None;
    let dict_bits = unsafe {
        if super::field_storage::allows_dictionary(_py, ptr) {
            super::field_storage::materialize(_py, ptr).unwrap_or(0)
        } else {
            0
        }
    };
    if dict_bits != 0
        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
        && unsafe { object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
        && unsafe { crate::dict_len(dict_ptr) } != 0
    {
        inc_ref_bits(_py, dict_bits);
        dict_state_bits = Some(dict_bits);
    }

    if exception_pending(_py) {
        if let Some(bits) = dict_state_bits {
            dec_ref_bits(_py, bits);
        }
        return MoltObject::none().bits();
    }

    // 2. Collect genuine slots; inferred attributes belong only to __dict__.
    let slot_state_bits = object_getstate_slot_state(_py, ptr);
    if exception_pending(_py) {
        if let Some(bits) = dict_state_bits {
            dec_ref_bits(_py, bits);
        }
        if let Some(bits) = slot_state_bits {
            dec_ref_bits(_py, bits);
        }
        return MoltObject::none().bits();
    }

    // 3. Combine following CPython's (dict, slots) tuple convention.
    match (dict_state_bits, slot_state_bits) {
        (Some(d), Some(s)) => {
            let tuple_ptr = crate::alloc_tuple(_py, &[d, s]);
            dec_ref_bits(_py, d);
            dec_ref_bits(_py, s);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
        (None, Some(s)) => {
            let none_bits = MoltObject::none().bits();
            let tuple_ptr = crate::alloc_tuple(_py, &[none_bits, s]);
            dec_ref_bits(_py, s);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
        (Some(d), None) => d,
        (None, None) => MoltObject::none().bits(),
    }
}

/// Default state uses normal attribute semantics for each declared slot name.
/// Custom getters, descriptors and same-name class shadows are observable here;
/// only AttributeError means absent. Physical backing traversal belongs to GC.
fn object_getstate_slot_state(py: &crate::PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    unsafe {
        let object = MoltObject::from_ptr(ptr).bits();
        inc_ref_bits(py, object);
        let _object_owner = crate::PtrDropGuard::new(py, ptr);
        let names = super::field_storage::slot_state_names(py, ptr)?;
        if names.is_empty() {
            return None;
        }
        let state = super::builders::alloc_dict_with_capacity_and_pairs(py, names.len(), &[]);
        if state.is_null() {
            return None;
        }
        let mut owner = crate::PtrDropGuard::new(py, state);
        for &name in names.iter() {
            let value = match lookup_attribute(py, object, name) {
                AttributeLookup::Found(value) => value,
                AttributeLookup::Missing => continue,
                AttributeLookup::Error => return None,
            };
            let completed = dict_set_in_place(py, state, name, value);
            dec_ref_bits(py, value);
            if !completed {
                return None;
            }
        }
        if dict_len(state) == 0 {
            return None;
        }
        owner.release();
        Some(MoltObject::from_ptr(state).bits())
    }
}

pub(crate) fn molt_object_format_method_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec_ptr) = spec_obj
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_STRING)
        else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "format_spec must be str");
        };
        if unsafe { string_len(spec_ptr) } == 0 {
            return molt_str_from_obj_attached_body(_py, self_bits);
        }
        let type_label = type_name(_py, obj_from_bits(self_bits));
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_object_lt_method_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { not_implemented_bits(_py) })
}

pub(crate) fn molt_object_le_method_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { not_implemented_bits(_py) })
}

pub(crate) fn molt_object_gt_method_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { not_implemented_bits(_py) })
}

pub(crate) fn molt_object_ge_method_attached_body(
    _py: &crate::PyToken<'_>,
    _self_bits: u64,
    _other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { not_implemented_bits(_py) })
}

pub(crate) fn molt_int_bool_method_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::object::ops_arith::native_slots::int_bool_slot(_py.attached_context(), self_bits)
}

pub(crate) fn molt_int_ceil_method_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

pub(crate) fn molt_int_abs_method_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::object::ops_arith::native_slots::int_abs_slot(_py.attached_context(), self_bits)
}

pub(crate) fn molt_int_add_method_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::object::ops_arith::native_slots::int_add_slot(
        _py.attached_context(),
        self_bits,
        other_bits,
    )
}

pub(crate) fn molt_int_and_method_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::object::ops_arith::native_slots::int_and_slot(
        _py.attached_context(),
        self_bits,
        other_bits,
    )
}

pub(crate) fn molt_int_divmod_method_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::object::ops_arith::native_slots::int_divmod_slot(
        _py.attached_context(),
        self_bits,
        other_bits,
    )
}

pub(crate) fn molt_str_add_method_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        crate::object::ops_arith::native_slots::sequence_add(_py, self_bits, other_bits)
    })
}

pub(crate) fn molt_object_init_attached_body(_py: &crate::PyToken<'_>, _self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { MoltObject::none().bits() })
}

pub(crate) fn molt_object_init_subclass_attached_body(
    _py: &crate::PyToken<'_>,
    _cls_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { MoltObject::none().bits() })
}

pub(crate) fn molt_object_getattribute_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
) -> u64 {
    match object_getattribute(_py, obj_bits, name_bits, false) {
        Ok(Some(bits)) => bits,
        Ok(None) | Err(()) => MoltObject::none().bits(),
    }
}

/// Explicit object lookup with the C generic API's dictionary-error policy.
pub(crate) fn object_getattribute(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
    suppress: bool,
) -> Result<Option<u64>, ()> {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return {
                let _ = raise_attr_name_type_error(_py, name_bits);
                Err(())
            };
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return {
                    let _ = raise_attr_name_type_error(_py, name_bits);
                    Err(())
                };
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if maybe_ptr_from_bits(obj_bits).is_none()
                || crate::builtins::attributes::is_numeric_scalar_attr_receiver(_py, obj_bits)
            {
                if let Some(val) =
                    crate::builtins::attributes::resolve_scalar_attr(_py, obj_bits, name_bits)
                        .into_result()?
                {
                    return Ok(Some(val));
                }
                if suppress {
                    return Ok(None);
                }
                return {
                    let _ = attr_error_with_obj(
                        _py,
                        type_name(_py, obj_from_bits(obj_bits)),
                        &attr_name,
                        obj_bits,
                    );
                    Err(())
                };
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                let found = match type_id {
                    crate::TYPE_ID_FOREIGN => crate::builtins::attributes::foreign_attr_lookup(
                        _py,
                        obj_ptr,
                        name_bits,
                        molt_cpython_abi::hooks::AttributeAccess::Generic,
                    )
                    .into_result(),
                    type_id if crate::object::heap_kind_has_class_shape(type_id) => {
                        crate::builtins::attr::object_attr_lookup_with_policy(
                            _py, obj_ptr, name_bits, suppress,
                        )
                        .into_result()
                    }
                    TYPE_ID_TYPE => {
                        // Generic lookup sees the physical object dictionary:
                        // heap types alias their own namespace; static types
                        // have a distinct initially empty dictionary. Bases
                        // and native namespace publication remain type lookup.
                        crate::builtins::attr::object_attr_lookup_with_policy(
                            _py, obj_ptr, name_bits, suppress,
                        )
                        .into_result()
                    }
                    _ => crate::builtins::attributes::attr_lookup_ptr_default_with_suppression(
                        _py, obj_ptr, name_bits, suppress,
                    )
                    .into_result(),
                };
                if let Some(val) = found? {
                    return Ok(Some(val));
                }
                if suppress {
                    return Ok(None);
                }

                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return {
                        let _ = attr_error_with_obj_message(
                            _py,
                            &msg,
                            &attr_name,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                        Err(())
                    };
                }
                return {
                    let _ = attr_error_with_obj(
                        _py,
                        type_name(_py, MoltObject::from_ptr(obj_ptr)),
                        &attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                    Err(())
                };
            }
            let obj = obj_from_bits(obj_bits);
            {
                let _ = attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits);
                Err(())
            }
        }
    })
}

pub(crate) fn molt_type_getattribute_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                if type_id != TYPE_ID_TYPE {
                    return molt_object_getattribute_attached_body(_py, obj_bits, name_bits);
                }
                let found = crate::builtins::attributes::type_attr_lookup_ptr_default(
                    _py, obj_ptr, name_bits,
                )
                .into_result();
                match found {
                    Ok(Some(val)) => return val,
                    Err(()) => return MoltObject::none().bits(),
                    Ok(None) => {}
                }
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return attr_error_with_message(_py, &msg);
            }
            let obj = obj_from_bits(obj_bits);
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits)
        }
    })
}

pub(crate) fn molt_type_call_attached_body(_py: &crate::PyToken<'_>, cls_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "type.__call__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "type.__call__ expects type",
                );
            }
            if runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_TYPE_CALL) {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(cls_ptr)))
                    .unwrap_or_default();
                let builtins = builtin_classes(_py);
                let kind = if cls_bits == builtins.type_obj {
                    "builtins.type"
                } else {
                    "type"
                };
                eprintln!(
                    "molt direct: type.__call__ invoked kind={} name={} cls_bits={} (no builder args forwarded)",
                    kind, class_name, cls_bits
                );
            }
            call_class_init_with_args(_py, cls_ptr, &[])
        }
    })
}

/// Explicit Python object.__setattr__: admit every receiver before the raw
/// generic primitive. Internal normal stores use molt_set_attr_name instead.
pub(crate) fn molt_object_setattr_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if !crate::builtins::attributes::explicit_object_mutation_admitted(py, obj_bits, false) {
            return MoltObject::none().bits();
        }
        let _ = crate::builtins::attributes::generic_set_attr_name(obj_bits, name_bits, val_bits);
        MoltObject::none().bits()
    })
}

/// Explicit Python object.__delattr__, with the same admission as its setter.
pub(crate) fn molt_object_delattr_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if !crate::builtins::attributes::explicit_object_mutation_admitted(py, obj_bits, true) {
            return MoltObject::none().bits();
        }
        let _ = crate::builtins::attributes::generic_del_attr_name(obj_bits, name_bits);
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_object_eq_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(true).bits();
        }
        not_implemented_bits(_py)
    })
}

pub(crate) fn molt_object_ne_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    other_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let name = crate::NameId::DunderEq.bits(_py);
        let outcome = crate::object::ops_compare::rich_compare_method_value(
            _py,
            obj_from_bits(self_bits),
            obj_from_bits(other_bits),
            name,
        );
        match crate::object::ops_compare::comparison_value_to_bool(_py, outcome) {
            crate::object::ops_compare::CompareBoolOutcome::True => {
                MoltObject::from_bool(false).bits()
            }
            crate::object::ops_compare::CompareBoolOutcome::False => {
                MoltObject::from_bool(true).bits()
            }
            crate::object::ops_compare::CompareBoolOutcome::Error => MoltObject::none().bits(),
            crate::object::ops_compare::CompareBoolOutcome::NotComparable => {
                not_implemented_bits(_py)
            }
        }
    })
}

pub(crate) fn molt_print_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    args_bits: u64,
    sep_bits: u64,
    end_bits: u64,
    file_bits: u64,
    flush_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        use crate::object::builders::PtrDropGuard;

        fn guard_owned<'guard>(py: &'guard PyToken<'guard>, bits: u64) -> PtrDropGuard<'guard> {
            PtrDropGuard::new(
                py,
                obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut()),
            )
        }

        fn separator(_py: &PyToken<'_>, bits: u64, default: &[u8], label: &str) -> Option<u64> {
            let obj = obj_from_bits(bits);
            if obj.is_none() {
                let ptr = alloc_string(_py, default);
                if ptr.is_null() {
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
                }
                return Some(MoltObject::from_ptr(ptr).bits());
            }
            if !obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
            {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    &format!(
                        "{label} must be None or a string, not {}",
                        type_name(_py, obj)
                    ),
                );
            }
            inc_ref_bits(_py, bits);
            Some(bits)
        }

        fn write_piece(_py: &PyToken<'_>, file: u64, value: u64) -> bool {
            let name = crate::NameId::Write.bits(_py);
            if exception_pending(_py) {
                return false;
            }
            // PyFile_WriteObject looks up the live writer before converting
            // this piece. Either callback can replace the next piece's writer.
            let writer = molt_get_attr_name_attached_body(_py, file, name);
            let writer_guard = guard_owned(_py, writer);
            if exception_pending(_py) {
                return false;
            }
            let text = molt_str_from_obj_attached_body(_py, value);
            let text_guard = guard_owned(_py, text);
            if exception_pending(_py) {
                return false;
            }
            let result = unsafe { call_callable1(_py, writer, text) };
            let result_guard = guard_owned(_py, result);
            // Match the consumer's release order, including callback results
            // with finalizers; do not let a generic LIFO cleanup reorder it.
            drop(text_guard);
            drop(writer_guard);
            drop(result_guard);
            !exception_pending(_py)
        }

        let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "print expects a tuple");
        };
        let Some(args) = (unsafe { crate::object::seq_access::pin_tuple(_py, args_ptr) }) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "print expects a tuple");
        };
        // CPython's argument converter tests flush before stream lookup and
        // separator validation. Its exception must prevent all later effects.
        let do_flush = is_truthy(_py, obj_from_bits(flush_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let file = if obj_from_bits(file_bits).is_none() {
            let sys = crate::builtins::modules::interpreter_sys_module(_py)
                .unwrap_or_else(|| MoltObject::none().bits());

            if obj_from_bits(sys).is_none() {
                // Early bootstrap uses the same cached native/WASM stream as
                // sys initialization. Encoding, buffering and I/O errors have
                // one owner; print never writes around the stream authority.
                let stdout = molt_sys_stdout_attached_body(_py);
                if obj_from_bits(stdout).is_none() && !exception_pending(_py) {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::RuntimeError,
                        "sys.stdout unavailable",
                    );
                }
                stdout
            } else {
                let name = crate::NameId::Stdout.bits(_py);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                molt_module_get_attr_attached_body(_py, sys, name)
            }
        } else {
            inc_ref_bits(_py, file_bits);
            file_bits
        };
        let _file_guard = guard_owned(_py, file);
        if exception_pending(_py) || obj_from_bits(file).is_none() {
            // Explicitly disabled stdout is a no-op, even with invalid sep/end.
            return MoltObject::none().bits();
        }
        let Some(sep) = separator(_py, sep_bits, b" ", "sep") else {
            return MoltObject::none().bits();
        };
        let _sep_guard = guard_owned(_py, sep);
        let Some(end) = separator(_py, end_bits, b"\n", "end") else {
            return MoltObject::none().bits();
        };
        let _end_guard = guard_owned(_py, end);

        for (index, &value) in args.iter().enumerate() {
            if index > 0 && !write_piece(_py, file, sep) {
                return MoltObject::none().bits();
            }
            if !write_piece(_py, file, value) {
                return MoltObject::none().bits();
            }
        }
        // Empty separators and terminators are still observable writes.
        if !write_piece(_py, file, end) {
            return MoltObject::none().bits();
        }
        if do_flush {
            let name = crate::NameId::Flush.bits(_py);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let flush = molt_get_attr_name_attached_body(_py, file, name);
            let flush_guard = guard_owned(_py, flush);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let result = unsafe { call_callable0(_py, flush) };
            let result_guard = guard_owned(_py, result);
            drop(flush_guard);
            drop(result_guard);
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod print_stream_tests {
    use super::*;

    #[test]
    fn print_uses_the_shared_memory_stream_and_preserves_return_value() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let none = MoltObject::none().bits();
            let stream = crate::builtins::io::molt_stringio_new_attached_body(
                _py,
                builtin_classes(_py).string_io,
                none,
                none,
            );
            assert!(!exception_pending(_py));
            let args_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            assert!(!args_ptr.is_null());
            let args = MoltObject::from_ptr(args_ptr).bits();
            let result = molt_print_builtin_attached_body(
                _py,
                args,
                none,
                none,
                stream,
                MoltObject::from_bool(true).bits(),
            );
            assert_eq!(result, none);
            assert!(!exception_pending(_py));
            let text = crate::builtins::io::molt_file_getvalue_attached_body(_py, stream);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(text)).as_deref(),
                Some("1 2\n")
            );
            dec_ref_bits(_py, text);
            dec_ref_bits(_py, args);
            dec_ref_bits(_py, stream);
        });
    }

    #[test]
    fn print_closed_stream_error_releases_owned_arguments() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let none = MoltObject::none().bits();
            let stream = crate::builtins::io::molt_stringio_new_attached_body(
                _py,
                builtin_classes(_py).string_io,
                none,
                none,
            );
            assert!(!exception_pending(_py));
            dec_ref_bits(
                _py,
                crate::builtins::io::molt_file_close_attached_body(_py, stream),
            );
            let sep_ptr = alloc_string(_py, b"unique-print-test-separator");
            let end_ptr = alloc_string(_py, b"unique-print-test-terminator");
            let args_ptr = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
            assert!(!sep_ptr.is_null() && !end_ptr.is_null() && !args_ptr.is_null());
            let sep = MoltObject::from_ptr(sep_ptr).bits();
            let end = MoltObject::from_ptr(end_ptr).bits();
            let args = MoltObject::from_ptr(args_ptr).bits();
            let before = unsafe {
                [
                    (*header_from_obj_ptr(sep_ptr)).ref_count_snapshot(),
                    (*header_from_obj_ptr(end_ptr)).ref_count_snapshot(),
                    (*header_from_obj_ptr(args_ptr)).ref_count_snapshot(),
                ]
            };
            let result = molt_print_builtin_attached_body(
                _py,
                args,
                sep,
                end,
                stream,
                MoltObject::from_bool(false).bits(),
            );
            assert_eq!(result, none);
            assert!(exception_pending(_py));
            clear_exception(_py);
            let after = unsafe {
                [
                    (*header_from_obj_ptr(sep_ptr)).ref_count_snapshot(),
                    (*header_from_obj_ptr(end_ptr)).ref_count_snapshot(),
                    (*header_from_obj_ptr(args_ptr)).ref_count_snapshot(),
                ]
            };
            assert_eq!(after, before);
            dec_ref_bits(_py, args);
            dec_ref_bits(_py, end);
            dec_ref_bits(_py, sep);
            dec_ref_bits(_py, stream);
        });
    }

    #[test]
    fn print_disabled_stdout_returns_before_separator_validation() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(_py, {
                let none = MoltObject::none().bits();
                let name_ptr = alloc_string(_py, b"sys");
                let stdout_ptr = alloc_string(_py, b"stdout");
                assert!(!name_ptr.is_null() && !stdout_ptr.is_null());
                let name = MoltObject::from_ptr(name_ptr).bits();
                let stdout = MoltObject::from_ptr(stdout_ptr).bits();
                let module = match molt_module_cache_get_attached_body(_py, name) {
                    bits if obj_from_bits(bits).is_none() => {
                        molt_module_new_attached_body(_py, name)
                    }
                    module => module,
                };
                crate::builtins::module_table::publish_interpreter_sys_for_test(_py, module);
                dec_ref_bits(
                    _py,
                    molt_module_set_attr_attached_body(_py, module, stdout, none),
                );
                assert!(!exception_pending(_py));
                let args_ptr = alloc_tuple(_py, &[MoltObject::from_int(1).bits()]);
                assert!(!args_ptr.is_null());
                let args = MoltObject::from_ptr(args_ptr).bits();
                let result = molt_print_builtin_attached_body(
                    _py,
                    args,
                    MoltObject::from_int(123).bits(),
                    none,
                    none,
                    MoltObject::from_bool(false).bits(),
                );
                assert_eq!(result, none);
                assert!(!exception_pending(_py));
                dec_ref_bits(_py, args);
                dec_ref_bits(_py, module);
                dec_ref_bits(_py, stdout);
                dec_ref_bits(_py, name);
            });
        });
    }
}

pub(crate) fn molt_input_builtin_attached_body(_py: &crate::PyToken<'_>, prompt_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(sys_bits) = crate::builtins::modules::interpreter_sys_module(_py) else {
            return raise_exception::<_>(_py, BuiltinExc::RuntimeError, "sys module unavailable");
        };
        inc_ref_bits(_py, sys_bits);

        let stdout_name_bits = crate::NameId::Stdout.bits(_py);
        let stdout_bits = molt_module_get_attr_attached_body(_py, sys_bits, stdout_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, sys_bits);
            return MoltObject::none().bits();
        }
        if obj_from_bits(stdout_bits).is_none() {
            dec_ref_bits(_py, sys_bits);
            return raise_exception::<_>(_py, BuiltinExc::RuntimeError, "sys.stdout unavailable");
        }

        let prompt_str_bits = molt_str_from_obj_attached_body(_py, prompt_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, stdout_bits);
            dec_ref_bits(_py, sys_bits);
            return MoltObject::none().bits();
        }

        let mut stdout_is_handle = false;
        if let Some(ptr) = obj_from_bits(stdout_bits).as_ptr() {
            unsafe {
                stdout_is_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
        }

        if stdout_is_handle {
            let _ = molt_file_write_attached_body(_py, stdout_bits, prompt_str_bits);
        } else {
            let write_name_bits = crate::NameId::Write.bits(_py);
            let write_method_bits =
                molt_get_attr_name_attached_body(_py, stdout_bits, write_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
            let write_res_bits = unsafe { call_callable1(_py, write_method_bits, prompt_str_bits) };
            dec_ref_bits(_py, write_method_bits);
            dec_ref_bits(_py, write_res_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
        }

        // Match CPython: flush stdout after writing the prompt.
        if stdout_is_handle {
            let _ = molt_file_flush_attached_body(_py, stdout_bits);
        } else {
            let missing = missing_bits(_py);
            let flush_name_bits = crate::NameId::Flush.bits(_py);
            let flush_bits =
                molt_getattr_builtin_attached_body(_py, stdout_bits, flush_name_bits, missing);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
            if flush_bits != missing {
                let callable_bits = molt_is_callable_attached_body(_py, flush_bits);
                let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
                dec_ref_bits(_py, callable_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, flush_bits);
                    dec_ref_bits(_py, prompt_str_bits);
                    dec_ref_bits(_py, stdout_bits);
                    dec_ref_bits(_py, sys_bits);
                    return MoltObject::none().bits();
                }
                if is_callable {
                    let flush_res_bits = unsafe { call_callable0(_py, flush_bits) };
                    dec_ref_bits(_py, flush_res_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, flush_bits);
                        dec_ref_bits(_py, prompt_str_bits);
                        dec_ref_bits(_py, stdout_bits);
                        dec_ref_bits(_py, sys_bits);
                        return MoltObject::none().bits();
                    }
                }
                dec_ref_bits(_py, flush_bits);
            }
        }

        dec_ref_bits(_py, prompt_str_bits);
        dec_ref_bits(_py, stdout_bits);

        let stdin_name_bits = crate::NameId::Stdin.bits(_py);
        let stdin_bits = molt_module_get_attr_attached_body(_py, sys_bits, stdin_name_bits);
        dec_ref_bits(_py, sys_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if obj_from_bits(stdin_bits).is_none() {
            return raise_exception::<_>(_py, BuiltinExc::RuntimeError, "sys.stdin unavailable");
        }

        let mut stdin_is_handle = false;
        if let Some(ptr) = obj_from_bits(stdin_bits).as_ptr() {
            unsafe {
                stdin_is_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
        }
        let line_bits = if stdin_is_handle {
            molt_file_readline_attached_body(_py, stdin_bits, MoltObject::from_int(-1).bits())
        } else {
            let readline_name_bits = crate::NameId::Readline.bits(_py);
            let method_bits = molt_get_attr_name_attached_body(_py, stdin_bits, readline_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, stdin_bits);
                return MoltObject::none().bits();
            }
            let out_bits = unsafe { call_callable0(_py, method_bits) };
            dec_ref_bits(_py, method_bits);
            out_bits
        };
        dec_ref_bits(_py, stdin_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let Some(line_ptr) = obj_from_bits(line_bits).as_ptr() else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "input() returned non-string");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "input() returned non-string",
                );
            }
            let bytes = std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            if bytes.is_empty() {
                dec_ref_bits(_py, line_bits);
                return raise_exception::<_>(_py, BuiltinExc::EOFError, "");
            }
            let mut end = bytes.len();
            if bytes[end - 1] == b'\n' {
                end -= 1;
                if end > 0 && bytes[end - 1] == b'\r' {
                    end -= 1;
                }
            } else if bytes[end - 1] == b'\r' {
                end -= 1;
            }
            if end == bytes.len() {
                return line_bits;
            }
            let out_ptr = alloc_string(_py, &bytes[..end]);
            dec_ref_bits(_py, line_bits);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

pub(crate) fn molt_slice_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    start_bits: u64,
    end_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let obj = obj_from_bits(obj_bits);
        let start_obj = obj_from_bits(start_bits);
        let end_obj = obj_from_bits(end_bits);
        if let Some(ptr) = obj.as_ptr()
            && unsafe { crate::object::iterable::builtin_receiver(_py, ptr) }
        {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let total_chars =
                        utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize)) as isize;
                    let start = match decode_slice_bound(_py, start_obj, total_chars, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, total_chars, total_chars) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_string(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let start_byte = utf8_char_to_byte_index_cached(
                        _py,
                        bytes,
                        start as i64,
                        Some(ptr as usize),
                    );
                    let end_byte =
                        utf8_char_to_byte_index_cached(_py, bytes, end as i64, Some(ptr as usize));
                    let slice = &bytes[start_byte..end_byte];
                    let out = alloc_string(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_BYTES {
                    let len = bytes_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_bytes(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len as usize);
                    let slice = &bytes[start as usize..end as usize];
                    let out = alloc_bytes(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    let slice = match crate::object::ops_sys::DecodedSlice::decode(
                        _py,
                        start_obj,
                        end_obj,
                        MoltObject::none(),
                    ) {
                        Ok(slice) => slice,
                        Err(err) => return slice_error(_py, err),
                    };
                    let len = bytes_len(ptr) as isize;
                    let (start, end, _) = slice.adjust(len);
                    if end < start {
                        let out = alloc_bytearray(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len as usize);
                    let slice = &bytes[start as usize..end as usize];
                    let out = alloc_bytearray(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    return crate::object::memoryview::memoryview_slice(
                        _py,
                        ptr,
                        start_bits,
                        end_bits,
                        MoltObject::none().bits(),
                    );
                }
                if type_id == TYPE_ID_LIST {
                    let len = list_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_list(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let Some(elems) = crate::object::seq_access::snapshot(
                        _py,
                        ptr,
                        "list slice snapshot allocation failed",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let start = (start as usize).min(elems.len());
                    let end = (end as usize).min(elems.len()).max(start);
                    let out = alloc_list(_py, &elems[start..end]);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_TUPLE {
                    let len = tuple_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_tuple(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let out = crate::object::seq_access::with_immutable_tuple_slice(ptr, |elems| {
                        let start = (start as usize).min(elems.len());
                        let end = (end as usize).min(elems.len()).max(start);
                        alloc_tuple(_py, &elems[start..end])
                    })
                    .unwrap_or(std::ptr::null_mut());
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
            }
        }
        let slice_bits =
            molt_slice_new_attached_body(_py, start_bits, end_bits, MoltObject::none().bits());
        if obj_from_bits(slice_bits).is_none() {
            return MoltObject::none().bits();
        }
        let res_bits = molt_index_attached_body(_py, obj_bits, slice_bits);
        dec_ref_bits(_py, slice_bits);
        res_bits
    })
}

pub(crate) fn molt_tuple_from_list_attached_body(_py: &crate::PyToken<'_>, bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_TUPLE {
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                if type_id == TYPE_ID_LIST {
                    let Some(elems) = crate::object::seq_access::snapshot(
                        _py,
                        ptr,
                        "tuple conversion snapshot allocation failed",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let out_ptr = alloc_tuple(_py, &elems);
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

include!("../attached_exports_generated/object/ops_builtins.rs");
