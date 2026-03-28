// Call dispatch, builtin wrappers, and type constructor builtins.
// Split from ops.rs for compilation-unit size reduction.

use crate::object::ops_string::utf8_char_to_byte_index_cached;
use crate::*;
use molt_obj_model::MoltObject;
use num_integer::Integer;
use num_traits::{Signed, Zero};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::ops::{decode_slice_bound, slice_error};
use super::ops_arith::binary_type_error;

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_slots_init(count: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if runtime_state(_py).code_slots.get().is_some() {
            return MoltObject::none().bits();
        }
        let Some(count) = usize::try_from(count).ok() else {
            return raise_exception::<_>(_py, "MemoryError", "code slot count too large");
        };
        let slots = (0..count).map(|_| AtomicU64::new(0)).collect();
        let _ = runtime_state(_py).code_slots.set(slots);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_slot_set(code_id: u64, code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slots) = runtime_state(_py).code_slots.get() else {
            return raise_exception::<_>(_py, "RuntimeError", "code slots not initialized");
        };
        let Some(idx) = usize::try_from(code_id).ok() else {
            return raise_exception::<_>(_py, "IndexError", "code slot out of range");
        };
        if idx >= slots.len() {
            return raise_exception::<_>(_py, "IndexError", "code slot out of range");
        }
        if let Some(ptr) = obj_from_bits(code_bits).as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_CODE {
                    return raise_exception::<_>(_py, "TypeError", "code slot expects code object");
                }
            }
        } else {
            return raise_exception::<_>(_py, "TypeError", "code slot expects code object");
        }
        if code_bits != 0 {
            inc_ref_bits(_py, code_bits);
        }
        let old_bits = slots[idx].swap(code_bits, AtomicOrdering::AcqRel);
        if old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_enter(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut code_bits = MoltObject::none().bits();
        let func_obj = obj_from_bits(func_bits);
        if let Some(func_ptr) = func_obj.as_ptr() {
            unsafe {
                match object_type_id(func_ptr) {
                    TYPE_ID_FUNCTION => {
                        code_bits = ensure_function_code_bits(_py, func_ptr);
                    }
                    TYPE_ID_BOUND_METHOD => {
                        let bound_func_bits = bound_method_func_bits(func_ptr);
                        if let Some(bound_ptr) = obj_from_bits(bound_func_bits).as_ptr()
                            && object_type_id(bound_ptr) == TYPE_ID_FUNCTION
                        {
                            code_bits = ensure_function_code_bits(_py, bound_ptr);
                        }
                    }
                    _ => {}
                }
            }
        }
        frame_stack_push(_py, code_bits);
        code_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_enter_slot(code_id: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slots) = runtime_state(_py).code_slots.get() else {
            return MoltObject::none().bits();
        };
        let Some(idx) = usize::try_from(code_id).ok() else {
            return MoltObject::none().bits();
        };
        let code_bits = if idx < slots.len() {
            slots[idx].load(AtomicOrdering::Acquire)
        } else {
            MoltObject::none().bits()
        };
        frame_stack_push(_py, code_bits);
        code_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_exit() -> u64 {
    crate::with_gil_entry!(_py, {
        frame_stack_pop(_py);
        MoltObject::none().bits()
    })
}

/// Outlined guarded-call helper: performs recursion guard enter/exit, optional
/// trace enter/exit, and the actual function call via function pointer dispatch.
/// Replaces the multi-block inline sequence previously generated for every
/// `call` op, eliminating ~3 Cranelift blocks and ~12 function-declaration/import
/// operations per call site.
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn molt_guarded_call(
    fn_ptr: u64,
    args_ptr: *const u64,
    nargs: u64,
    code_id: i64,
) -> u64 {
    if !recursion_guard_enter() {
        crate::with_gil_entry!(_py, {
            return raise_exception::<u64>(
                _py,
                "RecursionError",
                "maximum recursion depth exceeded",
            );
        });
    }
    if code_id >= 0 {
        crate::with_gil_entry!(_py, {
            if let Some(slots) = runtime_state(_py).code_slots.get() {
                let idx = code_id as usize;
                let code_bits = if idx < slots.len() {
                    slots[idx].load(AtomicOrdering::Acquire)
                } else {
                    MoltObject::none().bits()
                };
                frame_stack_push(_py, code_bits);
            } else {
                frame_stack_push(_py, MoltObject::none().bits());
            }
        });
    }
    let result: u64 = unsafe {
        let n = nargs as usize;
        molt_guarded_call_dispatch(fn_ptr, args_ptr, n)
    };
    if code_id >= 0 {
        crate::with_gil_entry!(_py, {
            frame_stack_pop(_py);
        });
    }
    recursion_guard_exit();
    result
}

/// Outlined guarded-call helper for dynamic dispatch paths where the callee
/// is identified by its object bits rather than a code slot id.
#[unsafe(no_mangle)]
pub extern "C" fn molt_guarded_call_obj(
    fn_ptr: u64,
    args_ptr: *const u64,
    nargs: u64,
    callee_bits: u64,
) -> u64 {
    if !recursion_guard_enter() {
        crate::with_gil_entry!(_py, {
            return raise_exception::<u64>(
                _py,
                "RecursionError",
                "maximum recursion depth exceeded",
            );
        });
    }
    if callee_bits != 0 {
        crate::with_gil_entry!(_py, {
            let mut code_bits = MoltObject::none().bits();
            let func_obj = obj_from_bits(callee_bits);
            if let Some(func_ptr) = func_obj.as_ptr() {
                unsafe {
                    match object_type_id(func_ptr) {
                        TYPE_ID_FUNCTION => {
                            code_bits = ensure_function_code_bits(_py, func_ptr);
                        }
                        TYPE_ID_BOUND_METHOD => {
                            let bound_func_bits = bound_method_func_bits(func_ptr);
                            if let Some(bound_ptr) = obj_from_bits(bound_func_bits).as_ptr()
                                && object_type_id(bound_ptr) == TYPE_ID_FUNCTION
                            {
                                code_bits = ensure_function_code_bits(_py, bound_ptr);
                            }
                        }
                        _ => {}
                    }
                }
            }
            frame_stack_push(_py, code_bits);
        });
    }
    let result: u64 = unsafe {
        let n = nargs as usize;
        molt_guarded_call_dispatch(fn_ptr, args_ptr, n)
    };
    if callee_bits != 0 {
        crate::with_gil_entry!(_py, {
            frame_stack_pop(_py);
        });
    }
    recursion_guard_exit();
    result
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
/// 4. **All arguments and the return value are MoltObject bit patterns (u64).**
///    The extern "C" ABI used by every target function expects and returns u64
///    values encoding NaN-boxed Molt objects.
///
/// **Violation consequence:** Arity mismatch or invalid fn_ptr causes stack
/// corruption, segfault, or silent data corruption. There is no runtime guard;
/// correctness depends entirely on the compiler backend.
#[inline(never)]
unsafe fn molt_guarded_call_dispatch(fn_ptr: u64, args_ptr: *const u64, n: usize) -> u64 {
    unsafe {
        match n {
            0 => {
                // SAFETY: transmute to 0-arity fn; see function-level invariants.
                let f: extern "C" fn() -> u64 = std::mem::transmute(fn_ptr as usize);
                f()
            }
            1 => {
                // SAFETY: transmute to 1-arity fn; see function-level invariants.
                let f: extern "C" fn(u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr)
            }
            2 => {
                // SAFETY: transmute to 2-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1))
            }
            3 => {
                // SAFETY: transmute to 3-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2))
            }
            4 => {
                // SAFETY: transmute to 4-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                )
            }
            5 => {
                // SAFETY: transmute to 5-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
                    *args_ptr,
                    *args_ptr.add(1),
                    *args_ptr.add(2),
                    *args_ptr.add(3),
                    *args_ptr.add(4),
                )
            }
            6 => {
                // SAFETY: transmute to 6-arity fn; see function-level invariants.
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
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
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
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
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
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
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
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
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
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
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
                    std::mem::transmute(fn_ptr as usize);
                f(
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
                ) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(
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
                ) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(
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
                ) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(
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
                ) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(
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
                ) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(
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
                crate::with_gil_entry!(_py, {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        &format!(
                            "direct dispatch does not support {} arguments; \
                             use callargs dispatch for functions with >16 parameters",
                            n
                        ),
                    );
                })
            }
        }
    }
}

/// Outlined dynamic function call dispatch for the `call_func` op.
///
/// Handles the full Python call protocol:
/// - Handle resolution (promises/futures)
/// - Bound method unwrapping (extracts self + func)
/// - Function object detection and direct fn_ptr dispatch
/// - Closure detection (delegates to callargs for closures)
/// - Arity matching with default arg handling
/// - Recursion guard and tracing
/// - Fallback to `molt_call_bind` for non-function callables
///
/// Arguments:
///   func_bits: the callable (could be function, bound method, or any callable)
///   args_ptr: pointer to array of argument bits (spilled to stack by caller)
///   nargs: number of arguments
///   code_id: unique code ID for this call site (tracing); 0 means no tracing
///
/// Returns: the call result bits
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn molt_call_func_dispatch(
    func_bits: u64,
    args_ptr_bits: u64, // u64 to match WASM all-i64 ABI; cast to *const u64 below
    nargs: u64,
    code_id: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = nargs as usize;
        let args_ptr = args_ptr_bits as usize as *const u64;

        // Read arguments into an inline stack buffer to avoid heap allocation
        // on every function call.  Falls back to Vec only for >16 args (very rare).
        let mut inline_buf = [0u64; 16];
        let heap_args: Vec<u64>;
        let args_slice: &[u64] = if n <= 16 {
            for i in 0..n {
                unsafe {
                    inline_buf[i] = *args_ptr.add(i);
                }
            }
            &inline_buf[..n]
        } else {
            heap_args = unsafe { (0..n).map(|i| *args_ptr.add(i)).collect() };
            &heap_args
        };

        // --- Step 1: Bound method unwrap ---
        // Use a [u64; 17] inline buffer for bound methods (self + up to 16 args).
        let mut bound_buf = [0u64; 17];
        let heap_bound: Vec<u64>;
        let (effective_func, effective_args): (u64, &[u64]) = unsafe {
            if let Some(ptr) = maybe_ptr_from_bits(func_bits) {
                if object_type_id(ptr) == TYPE_ID_BOUND_METHOD {
                    let inner = bound_method_func_bits(ptr);
                    let self_bits = bound_method_self_bits(ptr);
                    let combined_len = n + 1;
                    if combined_len <= 17 {
                        bound_buf[0] = self_bits;
                        for i in 0..n {
                            bound_buf[i + 1] = args_slice[i];
                        }
                        (inner, &bound_buf[..combined_len])
                    } else {
                        let mut v = Vec::with_capacity(combined_len);
                        v.push(self_bits);
                        v.extend_from_slice(args_slice);
                        heap_bound = v;
                        (inner, &heap_bound)
                    }
                } else {
                    (func_bits, args_slice)
                }
            } else {
                (func_bits, args_slice)
            }
        };

        // --- Step 2: Check if it's a plain function object ---
        let func_ptr = match maybe_ptr_from_bits(effective_func) {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_FUNCTION } => ptr,
            _ => {
                // Not a function — use the generic callargs dispatch.
                return molt_call_func_via_callargs(func_bits, effective_args);
            }
        };

        // --- Step 3: Check for closure ---
        // Closures need the full callargs path for env capture setup.
        let has_closure = unsafe { function_closure_bits(func_ptr) } != 0;
        if has_closure {
            return molt_call_func_via_callargs(func_bits, effective_args);
        }
        let has_trampoline = unsafe { function_trampoline_ptr(func_ptr) } != 0;
        if has_trampoline {
            return unsafe { call_function_obj_vec(_py, effective_func, effective_args) };
        }

        // --- Step 4: Direct call fast path ---
        let fn_ptr_val = unsafe { function_fn_ptr(func_ptr) };
        let func_arity = unsafe { function_arity(func_ptr) } as usize;
        let eff_nargs = effective_args.len();

        if func_arity == eff_nargs {
            // Exact arity match — fast path.
            return molt_call_func_direct(_py, fn_ptr_val, effective_args, code_id, func_bits);
        }

        // --- Step 5: Handle missing args with defaults ---
        // Use an inline [u64; 18] buffer for padded args (up to 16 effective + 2 defaults).
        // This same buffer is reused for the generic __defaults__ fallback below,
        // eliminating a second heap allocation.
        if eff_nargs < func_arity {
            let missing = func_arity - eff_nargs;
            let mut padded_buf = [0u64; 18];
            if missing <= 2 {
                let default_kind = molt_function_default_kind(effective_func);
                padded_buf[..eff_nargs].copy_from_slice(effective_args);
                let mut padded_len = eff_nargs;

                let filled = match (missing, default_kind) {
                    (1, FUNC_DEFAULT_NONE) => {
                        padded_buf[padded_len] = MoltObject::none().bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_DICT_POP) => {
                        padded_buf[padded_len] = MoltObject::from_int(1).bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_DICT_UPDATE) => {
                        padded_buf[padded_len] = missing_bits(_py);
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_ZERO) => {
                        padded_buf[padded_len] = MoltObject::from_int(0).bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_NEG_ONE) => {
                        padded_buf[padded_len] = MoltObject::from_int(-1).bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_MISSING) => {
                        padded_buf[padded_len] = missing_bits(_py);
                        padded_len += 1;
                        true
                    }
                    (2, FUNC_DEFAULT_NONE2) => {
                        padded_buf[padded_len] = MoltObject::none().bits();
                        padded_buf[padded_len + 1] = MoltObject::none().bits();
                        padded_len += 2;
                        true
                    }
                    (2, FUNC_DEFAULT_DICT_POP) => {
                        padded_buf[padded_len] = MoltObject::none().bits();
                        padded_buf[padded_len + 1] = MoltObject::from_int(0).bits();
                        padded_len += 2;
                        true
                    }
                    _ => false,
                };

                if filled {
                    return molt_call_func_direct(
                        _py,
                        fn_ptr_val,
                        &padded_buf[..padded_len],
                        code_id,
                        func_bits,
                    );
                }
            }

            // Generic fallback: consult __defaults__ tuple on the function.
            // This handles user-defined functions with keyword default
            // arguments (e.g. `def f(a, b, lo=0, hi=100)`) that the compact
            // default_kind encoding cannot represent.
            // Reuses padded_buf from above to avoid a second heap allocation.
            unsafe {
                let defaults_bits = function_attr_bits(
                    _py,
                    func_ptr,
                    intern_static_name(
                        _py,
                        &runtime_state(_py).interned.defaults_name,
                        b"__defaults__",
                    ),
                );
                if let Some(dbits) = defaults_bits {
                    if !obj_from_bits(dbits).is_none() {
                        if let Some(def_ptr) = obj_from_bits(dbits).as_ptr() {
                            if object_type_id(def_ptr) == TYPE_ID_TUPLE {
                                let defaults = seq_vec_ref(def_ptr);
                                let n_defaults = defaults.len();
                                if missing <= n_defaults {
                                    let total = eff_nargs + missing;
                                    if total <= 18 {
                                        // Reuse the stack-allocated padded_buf.
                                        padded_buf[..eff_nargs].copy_from_slice(effective_args);
                                        let start = n_defaults - missing;
                                        for i in 0..missing {
                                            padded_buf[eff_nargs + i] = defaults[start + i];
                                        }
                                        return molt_call_func_direct(
                                            _py,
                                            fn_ptr_val,
                                            &padded_buf[..total],
                                            code_id,
                                            func_bits,
                                        );
                                    } else {
                                        // >18 padded args: fall back to Vec (extremely rare).
                                        let mut padded = Vec::with_capacity(total);
                                        padded.extend_from_slice(effective_args);
                                        let start = n_defaults - missing;
                                        for i in start..n_defaults {
                                            padded.push(defaults[i]);
                                        }
                                        return molt_call_func_direct(
                                            _py, fn_ptr_val, &padded, code_id, func_bits,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // --- Step 5b: __kwdefaults__ fallback for keyword-only params ---
            // For `def f(x, *, key=None)` called as `f(1)`:
            //   arity=2, eff_nargs=1, __defaults__=(), __kwdefaults__={"key": None}
            //   __molt_kwonly_names__=("key",)
            // The kwonly params occupy the LAST slots in the compiled arity.
            // We fill positional defaults from __defaults__ and kwonly defaults
            // from __kwdefaults__ by name lookup.
            unsafe {
                // Get __molt_kwonly_names__ tuple
                let kwonly_bits = function_attr_bits(
                    _py,
                    func_ptr,
                    intern_static_name(
                        _py,
                        &runtime_state(_py).interned.molt_kwonly_names,
                        b"__molt_kwonly_names__",
                    ),
                );
                if let Some(kw_bits) = kwonly_bits {
                    if !obj_from_bits(kw_bits).is_none() {
                        if let Some(kw_ptr) = obj_from_bits(kw_bits).as_ptr() {
                            if object_type_id(kw_ptr) == TYPE_ID_TUPLE {
                                let kwonly_names = seq_vec_ref(kw_ptr);
                                let n_kwonly = kwonly_names.len();
                                if n_kwonly > 0 {
                                    // Get __kwdefaults__ dict
                                    let kwdef_bits = function_attr_bits(
                                        _py,
                                        func_ptr,
                                        intern_static_name(
                                            _py,
                                            &runtime_state(_py).interned.kwdefaults_name,
                                            b"__kwdefaults__",
                                        ),
                                    );
                                    if let Some(kd_bits) = kwdef_bits {
                                        if !obj_from_bits(kd_bits).is_none() {
                                            if let Some(kd_ptr) = obj_from_bits(kd_bits).as_ptr() {
                                                if object_type_id(kd_ptr) == TYPE_ID_DICT {
                                                    // n_positional = arity - n_kwonly
                                                    let n_positional = func_arity - n_kwonly;
                                                    let pos_missing =
                                                        n_positional.saturating_sub(eff_nargs);

                                                    // Get __defaults__ for positional defaults
                                                    let pos_defaults = function_attr_bits(
                                                        _py,
                                                        func_ptr,
                                                        intern_static_name(
                                                            _py,
                                                            &runtime_state(_py)
                                                                .interned
                                                                .defaults_name,
                                                            b"__defaults__",
                                                        ),
                                                    );
                                                    let mut pos_def_vec: &[u64] = &[];
                                                    let pos_def_owned;
                                                    if let Some(pd_bits) = pos_defaults {
                                                        if !obj_from_bits(pd_bits).is_none() {
                                                            if let Some(pd_ptr) =
                                                                obj_from_bits(pd_bits).as_ptr()
                                                            {
                                                                if object_type_id(pd_ptr)
                                                                    == TYPE_ID_TUPLE
                                                                {
                                                                    pos_def_owned =
                                                                        seq_vec_ref(pd_ptr).clone();
                                                                    pos_def_vec = &pos_def_owned;
                                                                }
                                                            }
                                                        }
                                                    }

                                                    // Check positional defaults cover pos_missing
                                                    if pos_missing <= pos_def_vec.len() {
                                                        // Try to fill all kwonly from __kwdefaults__
                                                        let mut kw_vals: Vec<u64> =
                                                            Vec::with_capacity(n_kwonly);
                                                        let mut all_found = true;
                                                        for i in 0..n_kwonly {
                                                            if let Some(val) = dict_get_in_place(
                                                                _py,
                                                                kd_ptr,
                                                                kwonly_names[i],
                                                            ) {
                                                                kw_vals.push(val);
                                                            } else {
                                                                all_found = false;
                                                                break;
                                                            }
                                                        }
                                                        if all_found {
                                                            let total = func_arity;
                                                            if total <= 18 {
                                                                // Copy provided positional args
                                                                padded_buf[..eff_nargs]
                                                                    .copy_from_slice(
                                                                        effective_args,
                                                                    );
                                                                // Fill missing positional defaults
                                                                if pos_missing > 0 {
                                                                    let start = pos_def_vec.len()
                                                                        - pos_missing;
                                                                    for i in 0..pos_missing {
                                                                        padded_buf[eff_nargs + i] =
                                                                            pos_def_vec[start + i];
                                                                    }
                                                                }
                                                                // Fill kwonly defaults
                                                                for i in 0..n_kwonly {
                                                                    padded_buf[n_positional + i] =
                                                                        kw_vals[i];
                                                                }
                                                                return molt_call_func_direct(
                                                                    _py,
                                                                    fn_ptr_val,
                                                                    &padded_buf[..total],
                                                                    code_id,
                                                                    func_bits,
                                                                );
                                                            } else {
                                                                let mut padded =
                                                                    Vec::with_capacity(total);
                                                                padded.extend_from_slice(
                                                                    effective_args,
                                                                );
                                                                if pos_missing > 0 {
                                                                    let start = pos_def_vec.len()
                                                                        - pos_missing;
                                                                    for i in
                                                                        start..pos_def_vec.len()
                                                                    {
                                                                        padded.push(pos_def_vec[i]);
                                                                    }
                                                                }
                                                                padded.extend_from_slice(&kw_vals);
                                                                return molt_call_func_direct(
                                                                    _py, fn_ptr_val, &padded,
                                                                    code_id, func_bits,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Arity mismatch we can't handle inline — fallback.
        molt_call_func_via_callargs(func_bits, effective_args)
    })
}

/// Direct function call through fn_ptr with recursion guard and optional tracing.
fn molt_call_func_direct(
    _py: &crate::concurrency::PyToken<'_>,
    fn_ptr: u64,
    args: &[u64],
    code_id: u64,
    callable_bits: u64,
) -> u64 {
    if !recursion_guard_enter() {
        return raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded");
    }
    if code_id != 0 {
        if let Some(func_ptr) = obj_from_bits(callable_bits).as_ptr() {
            unsafe {
                let code_bits = match object_type_id(func_ptr) {
                    TYPE_ID_FUNCTION => ensure_function_code_bits(_py, func_ptr),
                    TYPE_ID_BOUND_METHOD => {
                        let bf = bound_method_func_bits(func_ptr);
                        if let Some(bp) = obj_from_bits(bf).as_ptr() {
                            if object_type_id(bp) == TYPE_ID_FUNCTION {
                                ensure_function_code_bits(_py, bp)
                            } else {
                                MoltObject::none().bits()
                            }
                        } else {
                            MoltObject::none().bits()
                        }
                    }
                    _ => MoltObject::none().bits(),
                };
                frame_stack_push(_py, code_bits);
            }
        }
    }
    let result = unsafe { molt_guarded_call_dispatch(fn_ptr, args.as_ptr(), args.len()) };
    if code_id != 0 {
        frame_stack_pop(_py);
    }
    recursion_guard_exit();
    result
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
#[inline(always)]
unsafe fn direct_call_0(fn_ptr: u64) -> u64 {
    unsafe {
        let f: extern "C" fn() -> u64 = std::mem::transmute(fn_ptr as usize);
        f()
    }
}

/// Direct fn_ptr call for exactly 1 arg — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_1(fn_ptr: u64, a0: u64) -> u64 {
    unsafe {
        let f: extern "C" fn(u64) -> u64 = std::mem::transmute(fn_ptr as usize);
        f(a0)
    }
}

/// Direct fn_ptr call for exactly 2 args — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_2(fn_ptr: u64, a0: u64, a1: u64) -> u64 {
    unsafe {
        let f: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
        f(a0, a1)
    }
}

/// Direct fn_ptr call for exactly 3 args — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_3(fn_ptr: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    unsafe {
        let f: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
        f(a0, a1, a2)
    }
}

/// Probe the callable: if it's a non-closure function with matching arity,
/// return Some(fn_ptr). Otherwise None.
#[inline(always)]
unsafe fn probe_simple_func(func_bits: u64, expected_arity: usize) -> Option<u64> {
    unsafe {
        let obj = obj_from_bits(func_bits);
        let ptr = obj.as_ptr()?;
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return None;
        }
        if function_trampoline_ptr(ptr) != 0 {
            return None;
        }
        if function_closure_bits(ptr) != 0 {
            return None;
        }
        if (function_arity(ptr) as usize) != expected_arity {
            return None;
        }
        Some(function_fn_ptr(ptr))
    }
}

/// Fast 0-argument function call. No args — minimal dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast0(func_bits: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 0) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded",
                    )
                });
            }
            let result = direct_call_0(fn_ptr);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args: [u64; 0] = [];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 0, 0)
}

/// Fast 1-argument function call. Args passed in registers — no stack spill.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast1(func_bits: u64, a0: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 1) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded",
                    )
                });
            }
            let result = direct_call_1(fn_ptr, a0);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args = [a0];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 1, 0)
}

/// Fast 2-argument function call. Args passed in registers — no stack spill.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast2(func_bits: u64, a0: u64, a1: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 2) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded",
                    )
                });
            }
            let result = direct_call_2(fn_ptr, a0, a1);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args = [a0, a1];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 2, 0)
}

/// Fast 3-argument function call. Args passed in registers — no stack spill.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast3(func_bits: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 3) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded",
                    )
                });
            }
            let result = direct_call_3(fn_ptr, a0, a1, a2);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args = [a0, a1, a2];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 3, 0)
}

/// Fallback: build a CallArgs and dispatch through `molt_call_bind`.
fn molt_call_func_via_callargs(callable_bits: u64, args: &[u64]) -> u64 {
    let nargs = args.len() as u64;
    let pos_cap = MoltObject::from_int(nargs as i64).bits();
    let kw_cap = MoltObject::from_int(0).bits();
    let callargs_bits = molt_callargs_new(pos_cap, kw_cap);
    for &arg in args {
        unsafe { molt_callargs_push_pos(callargs_bits, arg) };
    }
    molt_call_bind(callable_bits, callargs_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_set_line(line_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_obj = obj_from_bits(line_bits);
        let line = if line_obj.is_int() || line_obj.is_bool() {
            to_i64(line_obj).unwrap_or(0)
        } else {
            line_bits as i64
        };
        frame_stack_set_line(line);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_repr_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_repr_from_obj(val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_format_builtin(val_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec_ptr) = spec_obj.as_ptr() else {
            let msg = format!(
                "format() argument 2 must be str, not {}",
                type_name(_py, spec_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(spec_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "format() argument 2 must be str, not {}",
                    type_name(_py, spec_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let spec_text = string_obj_to_owned(spec_obj).unwrap_or_default();
        if let Some(obj_ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_OBJECT || type_id == TYPE_ID_DATACLASS {
                    let class_bits = object_class_bits(obj_ptr);
                    if class_bits != 0
                        && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && object_type_id(class_ptr) == TYPE_ID_TYPE
                    {
                        let format_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.format_name,
                            b"__format__",
                        );
                        if let Some(call_bits) =
                            class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), format_bits)
                        {
                            return call_callable1(_py, call_bits, spec_bits);
                        }
                    }
                }
            }
        }
        let supports_format = obj.as_int().is_some()
            || obj.as_bool().is_some()
            || obj.as_float().is_some()
            || bigint_ptr_from_bits(obj.bits()).is_some()
            || obj
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
                .unwrap_or(false)
            || obj
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_COMPLEX })
                .unwrap_or(false);
        if supports_format {
            return molt_string_format(val_bits, spec_bits);
        }
        if spec_text.is_empty() {
            return molt_str_from_obj(val_bits);
        }
        let type_label = type_name(_py, obj);
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_callable_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_is_callable(val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_round_builtin(val_bits: u64, ndigits_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let has_ndigits = ndigits_bits != missing;
        let has_ndigits_bits = MoltObject::from_bool(has_ndigits).bits();
        let ndigits = if has_ndigits {
            ndigits_bits
        } else {
            MoltObject::none().bits()
        };
        molt_round(val_bits, ndigits, has_ndigits_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_any_builtin(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    return MoltObject::from_bool(false).bits();
                }
                if is_truthy(_py, obj_from_bits(val_bits)) {
                    return MoltObject::from_bool(true).bits();
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_all_builtin(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    return MoltObject::from_bool(true).bits();
                }
                if !is_truthy(_py, obj_from_bits(val_bits)) {
                    return MoltObject::from_bool(false).bits();
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abs_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if let Some(i) = to_i64(obj) {
            return int_bits_from_i128(_py, (i as i128).abs());
        }
        if let Some(big) = to_bigint(obj) {
            let abs_val = big.abs();
            if let Some(i) = bigint_to_inline(&abs_val) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, abs_val);
        }
        if let Some(f) = to_f64(obj) {
            return MoltObject::from_float(f.abs()).bits();
        }
        if let Some(ptr) = complex_ptr_from_bits(val_bits) {
            let value = unsafe { *complex_ref(ptr) };
            return MoltObject::from_float(value.re.hypot(value.im)).bits();
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits)
            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__abs__")
        {
            unsafe {
                let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("bad operand type for abs(): '{type_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_divmod_builtin(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a_bits);
        let rhs = obj_from_bits(b_bits);
        // If either operand is a float, skip ALL integer paths so that
        // divmod(7, 2.0) returns (3.0, 1.0) instead of (3, 1).
        // Note: to_i64 / to_bigint coerce exact-integer floats (e.g. 2.0 -> 2),
        // so we must guard the bigint path too, not just the i64 fast path.
        let either_float = lhs.is_float() || rhs.is_float();
        if !either_float && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let li128 = li as i128;
            let ri128 = ri as i128;
            let mut rem = li128 % ri128;
            if rem != 0 && (rem > 0) != (ri128 > 0) {
                rem += ri128;
            }
            let quot = (li128 - rem) / ri128;
            let q_bits = int_bits_from_i128(_py, quot);
            let r_bits = int_bits_from_i128(_py, rem);
            let tuple_ptr = alloc_tuple(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        if !either_float && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let quot = l_big.div_floor(&r_big);
            let rem = l_big.mod_floor(&r_big);
            let q_bits = if let Some(i) = bigint_to_inline(&quot) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, quot)
            };
            let r_bits = if let Some(i) = bigint_to_inline(&rem) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, rem)
            };
            let tuple_ptr = alloc_tuple(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "float divmod()");
            }
            let quot = (lf / rf).floor();
            let mut rem = lf % rf;
            if rem != 0.0 && (rem > 0.0) != (rf > 0.0) {
                rem += rf;
            }
            let q_bits = MoltObject::from_float(quot).bits();
            let r_bits = MoltObject::from_float(rem).bits();
            let tuple_ptr = alloc_tuple(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let left = class_name_for_error(type_of_bits(_py, a_bits));
        let right = class_name_for_error(type_of_bits(_py, b_bits));
        let msg = format!("unsupported operand type(s) for divmod(): '{left}' and '{right}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[inline]
fn minmax_compare(_py: &PyToken<'_>, best_key_bits: u64, cand_key_bits: u64) -> CompareOutcome {
    compare_objects(
        _py,
        obj_from_bits(cand_key_bits),
        obj_from_bits(best_key_bits),
    )
}

fn molt_minmax_builtin(
    _py: &PyToken<'_>,
    args_bits: u64,
    key_bits: u64,
    default_bits: u64,
    want_max: bool,
    name: &str,
) -> u64 {
    let missing = missing_bits(_py);
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        let msg = format!("{name} expected at least 1 argument, got 0");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            let msg = format!("{name} expected at least 1 argument, got 0");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let args = seq_vec_ref(args_ptr);
        if args.is_empty() {
            let msg = format!("{name} expected at least 1 argument, got 0");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let has_default = default_bits != missing;
        if args.len() > 1 && has_default {
            let msg =
                format!("Cannot specify a default for {name}() with multiple positional arguments");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let mut best_bits;
        let mut best_key_bits: u64;
        if args.len() == 1 {
            let iter_bits = molt_iter(args[0]);
            if obj_from_bits(iter_bits).is_none() {
                return raise_not_iterable(_py, args[0]);
            }
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                if has_default {
                    inc_ref_bits(_py, default_bits);
                    return default_bits;
                }
                let msg = format!("{name}() iterable argument is empty");
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            best_bits = val_bits;
            if use_key {
                best_key_bits = call_callable1(_py, key_bits, best_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            } else {
                best_key_bits = best_bits;
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                    }
                    inc_ref_bits(_py, best_bits);
                    return best_bits;
                }
                let cand_key_bits = if use_key {
                    let res_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                let replace = match minmax_compare(_py, best_key_bits, cand_key_bits) {
                    CompareOutcome::Ordered(ordering) => {
                        if want_max {
                            ordering == Ordering::Greater
                        } else {
                            ordering == Ordering::Less
                        }
                    }
                    CompareOutcome::Unordered => false,
                    CompareOutcome::NotComparable => {
                        if use_key {
                            dec_ref_bits(_py, best_key_bits);
                            dec_ref_bits(_py, cand_key_bits);
                        }
                        return compare_type_error(
                            _py,
                            obj_from_bits(cand_key_bits),
                            obj_from_bits(best_key_bits),
                            if want_max { ">" } else { "<" },
                        );
                    }
                    CompareOutcome::Error => {
                        if use_key {
                            dec_ref_bits(_py, best_key_bits);
                            dec_ref_bits(_py, cand_key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if replace {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                    }
                    best_bits = val_bits;
                    best_key_bits = cand_key_bits;
                } else if use_key {
                    dec_ref_bits(_py, cand_key_bits);
                }
            }
        }
        best_bits = args[0];
        if use_key {
            best_key_bits = call_callable1(_py, key_bits, best_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        } else {
            best_key_bits = best_bits;
        }
        for &val_bits in args.iter().skip(1) {
            let cand_key_bits = if use_key {
                let res_bits = call_callable1(_py, key_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                res_bits
            } else {
                val_bits
            };
            let replace = match minmax_compare(_py, best_key_bits, cand_key_bits) {
                CompareOutcome::Ordered(ordering) => {
                    if want_max {
                        ordering == Ordering::Greater
                    } else {
                        ordering == Ordering::Less
                    }
                }
                CompareOutcome::Unordered => false,
                CompareOutcome::NotComparable => {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                        dec_ref_bits(_py, cand_key_bits);
                    }
                    return compare_type_error(
                        _py,
                        obj_from_bits(cand_key_bits),
                        obj_from_bits(best_key_bits),
                        if want_max { ">" } else { "<" },
                    );
                }
                CompareOutcome::Error => {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                        dec_ref_bits(_py, cand_key_bits);
                    }
                    return MoltObject::none().bits();
                }
            };
            if replace {
                if use_key {
                    dec_ref_bits(_py, best_key_bits);
                }
                best_bits = val_bits;
                best_key_bits = cand_key_bits;
            } else if use_key {
                dec_ref_bits(_py, cand_key_bits);
            }
        }
        if use_key {
            dec_ref_bits(_py, best_key_bits);
        }
        inc_ref_bits(_py, best_bits);
        best_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_min_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, false, "min")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_max_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, true, "max")
    })
}

struct SortItem {
    key_bits: u64,
    value_bits: u64,
}

enum SortError {
    NotComparable(u64, u64),
    Exception,
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sorted_builtin(iter_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let reverse = is_truthy(_py, obj_from_bits(reverse_bits));
        let mut items: Vec<SortItem> = Vec::new();
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(_py, item.key_bits);
                    }
                }
                // If an exception is pending, propagate it; otherwise the
                // iterator returned a non-pointer sentinel — treat as done
                // and fall through to build the (possibly empty) sorted list.
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                break;
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if use_key {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if use_key {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_val_bits = if use_key {
                    let res_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                items.push(SortItem {
                    key_bits: key_val_bits,
                    value_bits: val_bits,
                });
            }
        }
        let mut error: Option<SortError> = None;
        items.sort_by(|left, right| {
            if error.is_some() {
                return Ordering::Equal;
            }
            let outcome = compare_objects(
                _py,
                obj_from_bits(left.key_bits),
                obj_from_bits(right.key_bits),
            );
            match outcome {
                CompareOutcome::Ordered(ordering) => {
                    if reverse {
                        ordering.reverse()
                    } else {
                        ordering
                    }
                }
                CompareOutcome::Unordered => Ordering::Equal,
                CompareOutcome::NotComparable => {
                    error = Some(SortError::NotComparable(left.key_bits, right.key_bits));
                    Ordering::Equal
                }
                CompareOutcome::Error => {
                    error = Some(SortError::Exception);
                    Ordering::Equal
                }
            }
        });
        if let Some(error) = error {
            if use_key {
                for item in items.drain(..) {
                    dec_ref_bits(_py, item.key_bits);
                }
            }
            match error {
                SortError::NotComparable(left_bits, right_bits) => {
                    let msg = format!(
                        "'<' not supported between instances of '{}' and '{}'",
                        type_name(_py, obj_from_bits(left_bits)),
                        type_name(_py, obj_from_bits(right_bits)),
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                SortError::Exception => {
                    return MoltObject::none().bits();
                }
            }
        }
        let mut out: Vec<u64> = Vec::with_capacity(items.len());
        for item in items.iter() {
            out.push(item.value_bits);
        }
        if use_key {
            for item in items.drain(..) {
                dec_ref_bits(_py, item.key_bits);
            }
        }
        let list_ptr = alloc_list(_py, &out);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sum_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let start_obj = obj_from_bits(start_bits);
        if let Some(ptr) = start_obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum strings [use ''.join(seq) instead]",
                    );
                }
                if type_id == TYPE_ID_BYTES {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum bytes [use b''.join(seq) instead]",
                    );
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum bytearray [use b''.join(seq) instead]",
                    );
                }
            }
        }
        // Fast path: if the iterable is a list or tuple of integers, sum
        // directly without going through the iterator protocol.  This avoids
        // allocating a (value, done) tuple per element.
        {
            let iter_obj_check = obj_from_bits(iter_bits);
            if let Some(ptr) = iter_obj_check.as_ptr() {
                let type_id = unsafe { object_type_id(ptr) };
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    let elems = unsafe { seq_vec_ref(ptr) };
                    let start_int = to_i64(start_obj);
                    if let Some(_) = start_int {
                        let mut acc128 = start_int.unwrap() as i128;
                        let mut all_int = true;
                        for &bits in elems.iter() {
                            let elem = obj_from_bits(bits);
                            if let Some(i) = to_i64(elem) {
                                acc128 += i as i128;
                            } else {
                                all_int = false;
                                break;
                            }
                        }
                        if all_int {
                            use crate::builtins::numbers::int_bits_from_i128;
                            return int_bits_from_i128(_py, acc128);
                        }
                    }
                }
            }
        }
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        // CPython >= 3.12 uses Neumaier compensated summation for float sums.
        // Detect float accumulation and switch to compensated mode.
        let mut total_bits = start_bits;
        let mut total_owned = false;
        let start_f = to_f64(start_obj);
        // If start is a number, try Neumaier compensated path.
        if let Some(start_val) = start_f {
            let mut fsum = start_val;
            let mut comp = 0.0_f64; // Neumaier compensation term
            let mut all_numeric = true;
            let mut has_float = start_obj.as_float().is_some();
            loop {
                let pair_bits = molt_iter_next(iter_obj);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                };
                unsafe {
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                    }
                    let elems = seq_vec_ref(pair_ptr);
                    if elems.len() < 2 {
                        return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                    }
                    let val_bits = elems[0];
                    let done_bits = elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        if all_numeric {
                            let result = fsum + comp;
                            if has_float {
                                return MoltObject::from_float(result).bits();
                            } else {
                                return MoltObject::from_int(result as i64).bits();
                            }
                        }
                        if !total_owned {
                            inc_ref_bits(_py, total_bits);
                        }
                        return total_bits;
                    }
                    let val_obj = obj_from_bits(val_bits);
                    if all_numeric {
                        // Check if value is float-coercible and stay in compensated mode
                        let item_f = if let Some(f) = val_obj.as_float() {
                            has_float = true;
                            Some(f)
                        } else if let Some(i) = to_i64(val_obj) {
                            Some(i as f64)
                        } else {
                            None
                        };
                        if let Some(x) = item_f {
                            // Neumaier compensated summation step
                            let t = fsum + x;
                            if fsum.abs() >= x.abs() {
                                comp += (fsum - t) + x;
                            } else {
                                comp += (x - t) + fsum;
                            }
                            fsum = t;
                            total_bits = MoltObject::from_float(fsum).bits();
                            total_owned = true;
                            continue;
                        }
                        // Non-numeric value: fall back to generic sum.
                        // total_owned must be set here because the done-check
                        // at the top of the next iteration reads it.
                        all_numeric = false;
                        total_bits = MoltObject::from_float(fsum + comp).bits();
                        #[allow(unused_assignments)]
                        {
                            total_owned = true;
                        }
                    }
                    let next_bits = molt_add(total_bits, val_bits);
                    if obj_from_bits(next_bits).is_none() {
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return binary_type_error(
                            _py,
                            obj_from_bits(total_bits),
                            obj_from_bits(val_bits),
                            "+",
                        );
                    }
                    total_bits = next_bits;
                    total_owned = true;
                }
            }
        }
        // Non-numeric start: generic path
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    if !total_owned {
                        inc_ref_bits(_py, total_bits);
                    }
                    return total_bits;
                }
                let next_bits = molt_add(total_bits, val_bits);
                if obj_from_bits(next_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return binary_type_error(
                        _py,
                        obj_from_bits(total_bits),
                        obj_from_bits(val_bits),
                        "+",
                    );
                }
                total_bits = next_bits;
                total_owned = true;
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if default_bits == missing {
            return molt_get_attr_name(obj_bits, name_bits);
        }
        molt_get_attr_name_default(obj_bits, name_bits, default_bits)
    })
}

/// Python `setattr(obj, name, value)` builtin.
#[unsafe(no_mangle)]
pub extern "C" fn molt_setattr_builtin(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_object_setattr(obj_bits, name_bits, val_bits);
        MoltObject::none().bits()
    })
}

/// Python `delattr(obj, name)` builtin.
#[unsafe(no_mangle)]
pub extern "C" fn molt_delattr_builtin(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_object_delattr(obj_bits, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vars_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython parity: vars() == locals() when called with no arguments.
            // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
            // re-entrant and uses the existing token.
            return crate::molt_locals_builtin();
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let dict_bits = molt_get_attr_name_default(obj_bits, dict_name_bits, missing);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if dict_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "vars() argument must have __dict__ attribute",
            );
        }
        dict_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_getstate(_self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(_self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != crate::TYPE_ID_OBJECT && type_id != crate::TYPE_ID_DATACLASS {
            return MoltObject::none().bits();
        }

        // 1. Collect __dict__ entries.
        let mut dict_state_bits: Option<u64> = None;
        let dict_bits = if type_id == crate::TYPE_ID_DATACLASS {
            unsafe { crate::dataclass_dict_bits(ptr) }
        } else {
            unsafe { crate::instance_dict_bits(ptr) }
        };
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            inc_ref_bits(_py, dict_bits);
            dict_state_bits = Some(dict_bits);
        }

        // 2. Collect typed/slot field values.
        let slot_state_bits = if type_id == crate::TYPE_ID_DATACLASS {
            dataclass_getstate_slot_state(_py, ptr)
        } else {
            object_getstate_slot_state(_py, ptr)
        };

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
            (None, None) => {
                // CPython returns self.__dict__ which may be empty {}.
                let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(dict_ptr).bits()
            }
        }
    })
}

/// Extract typed field values from `__molt_field_offsets__` into a new dict.
fn object_getstate_slot_state(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
        return None;
    }
    let class_dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
    let class_dict_ptr = obj_from_bits(class_dict_bits).as_ptr()?;
    if unsafe { object_type_id(class_dict_ptr) } != crate::TYPE_ID_DICT {
        return None;
    }
    let offsets_name_bits =
        crate::builtins::attr::attr_name_bits_from_bytes(_py, b"__molt_field_offsets__")?;
    let offsets_bits = unsafe { crate::dict_get_in_place(_py, class_dict_ptr, offsets_name_bits) };
    dec_ref_bits(_py, offsets_name_bits);
    if exception_pending(_py) {
        return None;
    }
    let offsets_bits = offsets_bits?;
    let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
    if unsafe { object_type_id(offsets_ptr) } != crate::TYPE_ID_DICT {
        return None;
    }

    let state_ptr = crate::alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return None;
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;
    let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let name_bits = pairs[idx];
        let offset_bits = pairs[idx + 1];
        idx += 2;
        let offset = obj_from_bits(offset_bits).as_int().filter(|&v| v >= 0)?;
        let value_bits = unsafe { crate::object_field_get_ptr_raw(_py, ptr, offset as usize) };
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return None;
        }
        if crate::builtins::methods::is_missing_bits(_py, value_bits) {
            dec_ref_bits(_py, value_bits);
            continue;
        }
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, value_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return None;
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return None;
    }
    Some(state_bits)
}

/// Extract dataclass field values from the descriptor layout into a new dict.
fn dataclass_getstate_slot_state(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let desc_ptr = unsafe { crate::dataclass_desc_ptr(ptr) };
    if desc_ptr.is_null() {
        return None;
    }
    let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
    let field_names = unsafe { &(*desc_ptr).field_names };
    if field_names.is_empty() {
        return None;
    }

    let state_ptr = crate::alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return None;
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;
    for (name, &value_bits) in field_names.iter().zip(field_values.iter()) {
        if crate::builtins::methods::is_missing_bits(_py, value_bits) {
            continue;
        }
        let Some(name_bits) =
            crate::builtins::attr::attr_name_bits_from_bytes(_py, name.as_bytes())
        else {
            dec_ref_bits(_py, state_bits);
            return None;
        };
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return None;
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return None;
    }
    Some(state_bits)
}

fn dir_runtime_python_at_least(_py: &PyToken<'_>, major: i64, minor: i64) -> bool {
    let state = runtime_state(_py);
    let guard = state.sys_version_info.lock().unwrap();
    let (runtime_major, runtime_minor) = guard
        .as_ref()
        .map(|info| (info.major, info.minor))
        .unwrap_or((3, 12));
    runtime_major > major || (runtime_major == major && runtime_minor >= minor)
}

fn dir_add_builtin_method_surface(
    _py: &PyToken<'_>,
    target_class_bits: u64,
    add_name: &mut dyn FnMut(&[u8]) -> bool,
) -> bool {
    let builtins = builtin_classes(_py);
    if target_class_bits == builtins.str {
        for name in [
            &b"capitalize"[..],
            &b"casefold"[..],
            &b"center"[..],
            &b"count"[..],
            &b"encode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"find"[..],
            &b"format"[..],
            &b"format_map"[..],
            &b"index"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdecimal"[..],
            &b"isdigit"[..],
            &b"isidentifier"[..],
            &b"islower"[..],
            &b"isnumeric"[..],
            &b"isprintable"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.bytes {
        for name in [
            &b"capitalize"[..],
            &b"center"[..],
            &b"count"[..],
            &b"decode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"find"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"index"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdigit"[..],
            &b"islower"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.bytearray {
        for name in [
            &b"append"[..],
            &b"capitalize"[..],
            &b"center"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"count"[..],
            &b"decode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"extend"[..],
            &b"find"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"index"[..],
            &b"insert"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdigit"[..],
            &b"islower"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"reverse"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"resize"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.int || target_class_bits == builtins.bool {
        for name in [
            &b"as_integer_ratio"[..],
            &b"bit_count"[..],
            &b"bit_length"[..],
            &b"conjugate"[..],
            &b"from_bytes"[..],
            &b"is_integer"[..],
            &b"to_bytes"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.float {
        for name in [
            &b"as_integer_ratio"[..],
            &b"conjugate"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"is_integer"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"from_number"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.complex {
        if !add_name(&b"conjugate"[..]) {
            return false;
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"from_number"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.list {
        for name in [
            &b"append"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"count"[..],
            &b"extend"[..],
            &b"index"[..],
            &b"insert"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"reverse"[..],
            &b"sort"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.tuple {
        return add_name(&b"count"[..]) && add_name(&b"index"[..]);
    }
    if target_class_bits == builtins.range {
        return add_name(&b"count"[..]) && add_name(&b"index"[..]);
    }
    if target_class_bits == builtins.dict {
        for name in [
            &b"clear"[..],
            &b"copy"[..],
            &b"fromkeys"[..],
            &b"get"[..],
            &b"items"[..],
            &b"keys"[..],
            &b"pop"[..],
            &b"popitem"[..],
            &b"setdefault"[..],
            &b"update"[..],
            &b"values"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.set {
        for name in [
            &b"add"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"difference"[..],
            &b"difference_update"[..],
            &b"discard"[..],
            &b"intersection"[..],
            &b"intersection_update"[..],
            &b"isdisjoint"[..],
            &b"issubset"[..],
            &b"issuperset"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"symmetric_difference"[..],
            &b"symmetric_difference_update"[..],
            &b"union"[..],
            &b"update"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.frozenset {
        for name in [
            &b"copy"[..],
            &b"difference"[..],
            &b"intersection"[..],
            &b"isdisjoint"[..],
            &b"issubset"[..],
            &b"issuperset"[..],
            &b"symmetric_difference"[..],
            &b"union"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.memoryview {
        for name in [
            &b"_from_flags"[..],
            &b"cast"[..],
            &b"hex"[..],
            &b"release"[..],
            &b"tobytes"[..],
            &b"tolist"[..],
            &b"toreadonly"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14)
            && (!add_name(&b"count"[..]) || !add_name(&b"index"[..]))
        {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.property {
        return add_name(&b"getter"[..]) && add_name(&b"setter"[..]) && add_name(&b"deleter"[..]);
    }
    if target_class_bits == builtins.base_exception_group
        || issubclass_bits(target_class_bits, builtins.base_exception_group)
    {
        return add_name(&b"add_note"[..])
            && add_name(&b"with_traceback"[..])
            && add_name(&b"derive"[..])
            && add_name(&b"split"[..])
            && add_name(&b"subgroup"[..]);
    }
    if target_class_bits == builtins.base_exception
        || issubclass_bits(target_class_bits, builtins.base_exception)
    {
        return add_name(&b"add_note"[..]) && add_name(&b"with_traceback"[..]);
    }
    if target_class_bits == builtins.slice {
        return add_name(&b"indices"[..]);
    }
    if target_class_bits == builtins.type_obj {
        return add_name(&b"mro"[..]);
    }
    true
}

unsafe fn dir_default_collect(_py: &PyToken<'_>, obj_bits: u64) -> u64 {
    unsafe {
        crate::gil_assert();

        let mut names: Vec<u64> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut extra_owned: Vec<u64> = Vec::new();

        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_TYPE {
                dir_collect_from_class_bits(obj_bits, &mut seen, &mut names);
            } else {
                dir_collect_from_instance(_py, obj_ptr, &mut seen, &mut names);
                dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
            }
        } else {
            dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
        }

        // Our runtime keeps many builtin methods in fast method caches rather than in
        // `type.__dict__`. CPython's dir() includes those names, so ensure they're visible.
        let mut add_name = |name: &[u8]| -> bool {
            let Ok(name_str) = std::str::from_utf8(name) else {
                return true;
            };
            if !seen.insert(name_str.to_string()) {
                return true;
            }
            let Some(bits) = attr_name_bits_from_bytes(_py, name) else {
                return false;
            };
            extra_owned.push(bits);
            names.push(bits);
            true
        };

        // Object surface (ordering-critical names appear early in CPython's sorted dir()).
        for name in [
            &b"__class__"[..],
            &b"__delattr__"[..],
            &b"__dir__"[..],
            &b"__doc__"[..],
            &b"__eq__"[..],
            &b"__format__"[..],
            &b"__ge__"[..],
            &b"__getattribute__"[..],
            &b"__getstate__"[..],
            &b"__gt__"[..],
            &b"__hash__"[..],
            &b"__init__"[..],
            &b"__init_subclass__"[..],
            &b"__le__"[..],
            &b"__lt__"[..],
            &b"__ne__"[..],
            &b"__new__"[..],
            &b"__repr__"[..],
            &b"__setattr__"[..],
            &b"__str__"[..],
        ] {
            if !add_name(name) {
                for owned in extra_owned {
                    dec_ref_bits(_py, owned);
                }
                return MoltObject::none().bits();
            }
        }

        let builtins = builtin_classes(_py);
        let target_class_bits = if maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_TYPE)
        {
            obj_bits
        } else {
            type_of_bits(_py, obj_bits)
        };

        if target_class_bits == builtins.int || target_class_bits == builtins.bool {
            for name in [
                &b"__abs__"[..],
                &b"__add__"[..],
                &b"__and__"[..],
                &b"__bool__"[..],
                &b"__ceil__"[..],
                &b"__divmod__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.str {
            for name in [&b"__add__"[..], &b"__contains__"[..], &b"__getitem__"[..]] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.list {
            for name in [
                &b"__add__"[..],
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.dict {
            for name in [
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
                &b"__getitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.none_type && !add_name(&b"__bool__"[..]) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }
        if !dir_add_builtin_method_surface(_py, target_class_bits, &mut add_name) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }

        // Hide names that CPython deliberately excludes from dir() output (even though the
        // attributes exist).
        let hide_module = is_builtin_class_bits(_py, target_class_bits);
        names.retain(|&bits| {
            let Some(name) = string_obj_to_owned(obj_from_bits(bits)) else {
                return true;
            };
            if name == "__mro__" || name == "__bases__" || name == "__text_signature__" {
                return false;
            }
            if name.starts_with("__molt_") {
                return false;
            }
            if hide_module && name == "__module__" {
                return false;
            }
            true
        });

        let list_ptr = alloc_list(_py, &names);
        for owned in extra_owned {
            dec_ref_bits(_py, owned);
        }
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let reverse_bits = MoltObject::from_int(0).bits();
        let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        list_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_dir_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { unsafe { dir_default_collect(_py, self_bits) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_format_method(self_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec) = string_obj_to_owned(spec_obj) else {
            return raise_exception::<_>(_py, "TypeError", "format_spec must be str");
        };
        if spec.is_empty() {
            return molt_str_from_obj(self_bits);
        }
        let type_label = type_name(_py, obj_from_bits(self_bits));
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_lt_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_le_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_gt_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_ge_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_bool_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(is_truthy(_py, obj_from_bits(self_bits))).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_ceil_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_abs_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_abs_builtin(self_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_add_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.int && other_ty != builtins.bool {
            return not_implemented_bits(_py);
        }
        molt_add(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_and_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.int && other_ty != builtins.bool {
            return not_implemented_bits(_py);
        }
        molt_bit_and(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_divmod_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.int && other_ty != builtins.bool {
            return not_implemented_bits(_py);
        }
        molt_divmod_builtin(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_str_add_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.str {
            return not_implemented_bits(_py);
        }
        molt_add(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dir_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython: dir() (no args) lists the caller's local scope.
            unsafe {
                // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
                // re-entrant and many runtime helpers rely on nested calls.
                let locals_bits = crate::molt_locals_builtin();
                if exception_pending(_py) {
                    if !obj_from_bits(locals_bits).is_none() {
                        dec_ref_bits(_py, locals_bits);
                    }
                    return MoltObject::none().bits();
                }
                let list_bits = list_from_iter_bits(_py, locals_bits)
                    .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(locals_bits).is_none() {
                    dec_ref_bits(_py, locals_bits);
                }
                if obj_from_bits(list_bits).is_none() || exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let none_bits = MoltObject::none().bits();
                let reverse_bits = MoltObject::from_int(0).bits();
                let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return list_bits;
            }
        }

        let mut names: Vec<u64> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut extra_owned: Vec<u64> = Vec::new();
        let _obj = obj_from_bits(obj_bits);
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            unsafe {
                // CPython's dir() respects user-defined `__dir__`, but it must *not* dispatch
                // to our internal fast-path method-cache implementation (which would recurse
                // back into this builtin).
                //
                // So: only consult instance `__dict__` and the class `__dict__` MRO chain,
                // skipping method caches entirely.
                static DIR_NAME: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let dir_name_bits = intern_static_name(_py, &DIR_NAME, b"__dir__");
                let mut override_bits: u64 = 0;

                let dict_bits = instance_dict_bits(obj_ptr);
                if dict_bits != 0
                    && !obj_from_bits(dict_bits).is_none()
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, dir_name_bits)
                {
                    inc_ref_bits(_py, val_bits);
                    override_bits = val_bits;
                }

                if override_bits == 0 {
                    let class_bits = type_of_bits(_py, obj_bits);
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && let Some(attr_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, dir_name_bits)
                    {
                        let bound_opt = descriptor_bind(_py, attr_bits, class_ptr, Some(obj_ptr));
                        dec_ref_bits(_py, attr_bits);

                        if exception_pending(_py) {
                            // `descriptor_bind` can create a temporary bound object; avoid leaks.
                            if let Some(bound_bits) = bound_opt
                                && !obj_from_bits(bound_bits).is_none()
                            {
                                dec_ref_bits(_py, bound_bits);
                            }
                            return MoltObject::none().bits();
                        }

                        if let Some(bound_bits) = bound_opt {
                            override_bits = bound_bits;
                        }
                    }
                }

                if override_bits != 0 && !obj_from_bits(override_bits).is_none() {
                    let res_bits = call_callable0(_py, override_bits);
                    dec_ref_bits(_py, override_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return res_bits;
                }
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_TYPE {
                    dir_collect_from_class_bits(obj_bits, &mut seen, &mut names);
                } else {
                    dir_collect_from_instance(_py, obj_ptr, &mut seen, &mut names);
                    dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
                }
            }
        } else {
            unsafe {
                dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
            }
        }

        // Our runtime keeps many builtin methods in fast method caches rather than in
        // `type.__dict__`. CPython's dir() includes those names, so ensure they're visible.
        let mut add_name = |name: &[u8]| -> bool {
            let Ok(name_str) = std::str::from_utf8(name) else {
                return true;
            };
            if !seen.insert(name_str.to_string()) {
                return true;
            }
            let Some(bits) = attr_name_bits_from_bytes(_py, name) else {
                return false;
            };
            extra_owned.push(bits);
            names.push(bits);
            true
        };

        // Object surface (ordering-critical names appear early in CPython's sorted dir()).
        for name in [
            &b"__class__"[..],
            &b"__delattr__"[..],
            &b"__dir__"[..],
            &b"__doc__"[..],
            &b"__eq__"[..],
            &b"__format__"[..],
            &b"__ge__"[..],
            &b"__getattribute__"[..],
            &b"__getstate__"[..],
            &b"__gt__"[..],
            &b"__hash__"[..],
            &b"__init__"[..],
            &b"__init_subclass__"[..],
            &b"__le__"[..],
            &b"__lt__"[..],
            &b"__ne__"[..],
            &b"__new__"[..],
            &b"__repr__"[..],
            &b"__setattr__"[..],
            &b"__str__"[..],
        ] {
            if !add_name(name) {
                for owned in extra_owned {
                    dec_ref_bits(_py, owned);
                }
                return MoltObject::none().bits();
            }
        }

        let builtins = builtin_classes(_py);
        let target_class_bits = if maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
        {
            obj_bits
        } else {
            type_of_bits(_py, obj_bits)
        };

        if target_class_bits == builtins.int || target_class_bits == builtins.bool {
            for name in [
                &b"__abs__"[..],
                &b"__add__"[..],
                &b"__and__"[..],
                &b"__bool__"[..],
                &b"__ceil__"[..],
                &b"__divmod__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.str {
            for name in [&b"__add__"[..], &b"__contains__"[..], &b"__getitem__"[..]] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.list {
            for name in [
                &b"__add__"[..],
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.dict {
            for name in [
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
                &b"__getitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.none_type && !add_name(&b"__bool__"[..]) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }
        if !dir_add_builtin_method_surface(_py, target_class_bits, &mut add_name) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }

        // Hide names that CPython deliberately excludes from dir() output (even though the
        // attributes exist).
        let hide_module = is_builtin_class_bits(_py, target_class_bits);
        names.retain(|&bits| {
            let Some(name) = string_obj_to_owned(obj_from_bits(bits)) else {
                return true;
            };
            if name == "__mro__" || name == "__bases__" || name == "__text_signature__" {
                return false;
            }
            if name.starts_with("__molt_") {
                return false;
            }
            if hide_module && name == "__module__" {
                return false;
            }
            true
        });

        let list_ptr = alloc_list(_py, &names);
        for owned in extra_owned {
            dec_ref_bits(_py, owned);
        }
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let reverse_bits = MoltObject::from_int(0).bits();
        let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        list_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_init(_self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_init_subclass(_cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_getattribute(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                let found = match type_id {
                    TYPE_ID_OBJECT => object_attr_lookup_raw(_py, obj_ptr, name_bits),
                    TYPE_ID_DATACLASS => dataclass_attr_lookup_raw(_py, obj_ptr, name_bits),
                    _ => attr_lookup_ptr(_py, obj_ptr, name_bits),
                };
                if let Some(val) = found {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                if type_id == TYPE_ID_DATACLASS {
                    let desc_ptr = dataclass_desc_ptr(obj_ptr);
                    if !desc_ptr.is_null() && (*desc_ptr).slots {
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        return attr_error_with_obj(
                            _py,
                            type_label,
                            &attr_name,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        ) as u64;
                    }
                    let type_label = if !desc_ptr.is_null() {
                        let name = &(*desc_ptr).name;
                        if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        }
                    } else {
                        "dataclass"
                    };
                    return attr_error_with_obj(
                        _py,
                        type_label,
                        &attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    ) as u64;
                }
                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return attr_error_with_obj_message(
                        _py,
                        &msg,
                        &attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    ) as u64;
                }
                return attr_error_with_obj(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    &attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                ) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            if (obj.is_int() || obj.is_bool())
                && let Some(func_bits) = crate::builtins::methods::int_method_bits(_py, &attr_name)
            {
                return crate::molt_bound_method_new(func_bits, obj_bits);
            }
            if obj.is_float()
                && let Some(func_bits) =
                    crate::builtins::methods::float_method_bits(_py, &attr_name)
            {
                return crate::molt_bound_method_new(func_bits, obj_bits);
            }
            // Inline int/float/bool: fall back to class-based resolution
            // so that inherited methods (e.g. object.__init__) are found.
            {
                let builtins = builtin_classes(_py);
                let class_bits = if obj.is_float() {
                    builtins.float
                } else if obj.is_bool() {
                    builtins.bool
                } else if obj.is_int() {
                    builtins.int
                } else {
                    0
                };
                if class_bits != 0 {
                    if let Some(func_bits) = crate::builtins::methods::builtin_class_method_bits(
                        _py, class_bits, &attr_name,
                    ) {
                        return crate::molt_bound_method_new(func_bits, obj_bits);
                    }
                    if let Some(func_bits) = crate::builtins::methods::builtin_class_method_bits(
                        _py,
                        builtins.object,
                        &attr_name,
                    ) {
                        return crate::molt_bound_method_new(func_bits, obj_bits);
                    }
                }
            }
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_getattribute(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                    return molt_object_getattribute(obj_bits, name_bits);
                }
                let found = crate::builtins::attributes::type_attr_lookup_ptr_default(
                    _py, obj_ptr, name_bits,
                );
                if let Some(val) = found {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return attr_error_with_message(_py, &msg) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_call(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            }
            if matches!(
                std::env::var("MOLT_TRACE_TYPE_CALL").ok().as_deref(),
                Some("1")
            ) {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_setattr(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, attr_name.as_bytes()) else {
                return MoltObject::none().bits();
            };
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_TYPE {
                    dec_ref_bits(_py, attr_bits);
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "can't apply this __setattr__ to type object",
                    );
                }
                let class_bits = object_class_bits(obj_ptr);
                let builtins = builtin_classes(_py);
                let is_dict_subclass =
                    type_id == TYPE_ID_DICT && class_bits != 0 && class_bits != builtins.dict;
                let res = if type_id == TYPE_ID_OBJECT || is_dict_subclass {
                    object_setattr_raw(_py, obj_ptr, attr_bits, &attr_name, val_bits)
                } else if type_id == TYPE_ID_DATACLASS {
                    dataclass_setattr_raw_unchecked(_py, obj_ptr, attr_bits, &attr_name, val_bits)
                } else {
                    let bytes = string_bytes(name_ptr);
                    let len = string_len(name_ptr);
                    molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits)
                };
                dec_ref_bits(_py, attr_bits);
                return res as u64;
            }
            let obj = obj_from_bits(obj_bits);
            let res = attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64;
            dec_ref_bits(_py, attr_bits);
            res
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_delattr(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, attr_name.as_bytes()) else {
                return MoltObject::none().bits();
            };
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_TYPE {
                    dec_ref_bits(_py, attr_bits);
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "can't apply this __delattr__ to type object",
                    );
                }
                let class_bits = object_class_bits(obj_ptr);
                let builtins = builtin_classes(_py);
                let is_dict_subclass =
                    type_id == TYPE_ID_DICT && class_bits != 0 && class_bits != builtins.dict;
                let res = if type_id == TYPE_ID_OBJECT || is_dict_subclass {
                    object_delattr_raw(_py, obj_ptr, attr_bits, &attr_name)
                } else if type_id == TYPE_ID_DATACLASS {
                    dataclass_delattr_raw_unchecked(_py, obj_ptr, attr_bits, &attr_name)
                } else {
                    del_attr_ptr(_py, obj_ptr, attr_bits, &attr_name)
                };
                dec_ref_bits(_py, attr_bits);
                return res as u64;
            }
            let obj = obj_from_bits(obj_bits);
            let res = attr_error(_py, type_name(_py, obj), &attr_name) as u64;
            dec_ref_bits(_py, attr_bits);
            res
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(true).bits();
        }
        not_implemented_bits(_py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_ne(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(false).bits();
        }
        not_implemented_bits(_py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_print_builtin(
    args_bits: u64,
    sep_bits: u64,
    end_bits: u64,
    file_bits: u64,
    flush_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        fn print_string_arg_bits(
            _py: &PyToken<'_>,
            bits: u64,
            default: &[u8],
            label: &str,
        ) -> Option<u64> {
            let obj = obj_from_bits(bits);
            if obj.is_none() {
                let ptr = alloc_string(_py, default);
                if ptr.is_null() {
                    return None;
                }
                return Some(MoltObject::from_ptr(ptr).bits());
            }
            let Some(ptr) = obj.as_ptr() else {
                let msg = format!(
                    "{} must be None or a string, not {}",
                    label,
                    type_name(_py, obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "{} must be None or a string, not {}",
                        label,
                        type_name(_py, obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
            inc_ref_bits(_py, bits);
            Some(bits)
        }

        fn string_bits_is_empty(bits: u64) -> bool {
            let obj = obj_from_bits(bits);
            let Some(ptr) = obj.as_ptr() else {
                return false;
            };
            unsafe { string_len(ptr) == 0 }
        }

        fn string_bits_contains_newline(bits: u64) -> bool {
            let obj = obj_from_bits(bits);
            let Some(ptr) = obj.as_ptr() else {
                return false;
            };
            unsafe {
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                bytes.contains(&b'\n')
            }
        }

        fn encode_print_bytes(
            _py: &PyToken<'_>,
            bits: u64,
            encoding: &str,
            errors: &str,
        ) -> Result<Vec<u8>, u64> {
            let obj = obj_from_bits(bits);
            let Some(ptr) = obj.as_ptr() else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "print expects a string",
                ));
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "print expects a string",
                    ));
                }
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                match encode_string_with_errors(bytes, encoding, Some(errors)) {
                    Ok(out) => Ok(out),
                    Err(EncodeError::UnknownEncoding(name)) => {
                        let msg = format!("unknown encoding: {name}");
                        Err(raise_exception::<_>(_py, "LookupError", &msg))
                    }
                    Err(EncodeError::UnknownErrorHandler(name)) => {
                        let msg = format!("unknown error handler name '{name}'");
                        Err(raise_exception::<_>(_py, "LookupError", &msg))
                    }
                    Err(EncodeError::InvalidChar {
                        encoding,
                        code,
                        pos,
                        limit,
                    }) => {
                        let reason = encode_error_reason(encoding, code, limit);
                        Err(raise_unicode_encode_error::<_>(
                            _py,
                            encoding,
                            bits,
                            pos,
                            pos + 1,
                            &reason,
                        ))
                    }
                }
            }
        }

        let args_obj = obj_from_bits(args_bits);
        let Some(args_ptr) = args_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "print expects a tuple");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "print expects a tuple");
            }
            let mut sep_bits_opt = match print_string_arg_bits(_py, sep_bits, b" ", "sep") {
                Some(bits) => Some(bits),
                None => return MoltObject::none().bits(),
            };
            let mut end_bits_opt = match print_string_arg_bits(_py, end_bits, b"\n", "end") {
                Some(bits) => Some(bits),
                None => {
                    if let Some(bits) = sep_bits_opt {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
            };
            if let Some(bits) = sep_bits_opt
                && string_bits_is_empty(bits)
            {
                dec_ref_bits(_py, bits);
                sep_bits_opt = None;
            }
            if let Some(bits) = end_bits_opt
                && string_bits_is_empty(bits)
            {
                dec_ref_bits(_py, bits);
                end_bits_opt = None;
            }

            let mut resolved_file_bits = file_bits;
            let mut file_from_sys = false;
            if obj_from_bits(resolved_file_bits).is_none() {
                let sys_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.sys_name, b"sys");
                if !obj_from_bits(sys_name_bits).is_none() {
                    let sys_bits = molt_module_cache_get(sys_name_bits);
                    if !obj_from_bits(sys_bits).is_none() {
                        let stdout_name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.stdout_name,
                            b"stdout",
                        );
                        resolved_file_bits = molt_module_get_attr(sys_bits, stdout_name_bits);
                        dec_ref_bits(_py, sys_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        file_from_sys = true;
                    }
                }
            }

            let elems = seq_vec_ref(args_ptr);
            let do_flush = is_truthy(_py, obj_from_bits(flush_bits));

            // Fall back to raw C stdio when the resolved file is None —
            // this handles both "sys not imported" AND "sys.stdout is None"
            // (e.g., during sys.py bootstrap before intrinsics register stdout).
            if obj_from_bits(resolved_file_bits).is_none() {
                let encoding = "utf-8";
                let errors = "surrogateescape";
                let mut stdout = std::io::stdout();
                let mut wrote_newline = false;
                let sep_bytes = if let Some(bits) = sep_bits_opt {
                    match encode_print_bytes(_py, bits, encoding, errors) {
                        Ok(bytes) => Some(bytes),
                        Err(bits) => {
                            if let Some(end_bits) = end_bits_opt {
                                dec_ref_bits(_py, end_bits);
                            }
                            dec_ref_bits(_py, bits);
                            return bits;
                        }
                    }
                } else {
                    None
                };
                let end_bytes = if let Some(bits) = end_bits_opt {
                    match encode_print_bytes(_py, bits, encoding, errors) {
                        Ok(bytes) => Some(bytes),
                        Err(bits) => {
                            if let Some(sep_bits) = sep_bits_opt {
                                dec_ref_bits(_py, sep_bits);
                            }
                            dec_ref_bits(_py, bits);
                            return bits;
                        }
                    }
                } else {
                    None
                };
                for (idx, &val_bits) in elems.iter().enumerate() {
                    if idx > 0
                        && let Some(bytes) = sep_bytes.as_deref()
                    {
                        if bytes.contains(&b'\n') {
                            wrote_newline = true;
                        }
                        let _ = stdout.write_all(bytes);
                    }
                    let str_bits = molt_str_from_obj(val_bits);
                    if exception_pending(_py) {
                        if let Some(sep_bits) = sep_bits_opt {
                            dec_ref_bits(_py, sep_bits);
                        }
                        if let Some(end_bits) = end_bits_opt {
                            dec_ref_bits(_py, end_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let bytes = match encode_print_bytes(_py, str_bits, encoding, errors) {
                        Ok(bytes) => bytes,
                        Err(bits) => {
                            dec_ref_bits(_py, str_bits);
                            if let Some(sep_bits) = sep_bits_opt {
                                dec_ref_bits(_py, sep_bits);
                            }
                            if let Some(end_bits) = end_bits_opt {
                                dec_ref_bits(_py, end_bits);
                            }
                            return bits;
                        }
                    };
                    if bytes.contains(&b'\n') {
                        wrote_newline = true;
                    }
                    let _ = stdout.write_all(&bytes);
                    dec_ref_bits(_py, str_bits);
                }
                if let Some(bytes) = end_bytes.as_deref() {
                    if bytes.contains(&b'\n') {
                        wrote_newline = true;
                    }
                    let _ = stdout.write_all(bytes);
                }
                if do_flush || wrote_newline {
                    let _ = stdout.flush();
                }
                if let Some(bits) = sep_bits_opt {
                    dec_ref_bits(_py, bits);
                }
                if let Some(bits) = end_bits_opt {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }

            let sep_bits = sep_bits_opt;
            let end_bits = end_bits_opt;
            let end_has_newline = end_bits.map(string_bits_contains_newline).unwrap_or(false);

            let mut write_bits = MoltObject::none().bits();
            let mut use_file_handle = false;
            if let Some(ptr) = obj_from_bits(resolved_file_bits).as_ptr() {
                use_file_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
            if !use_file_handle {
                let write_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.write_name, b"write");
                write_bits = molt_get_attr_name(resolved_file_bits, write_name_bits);
                if exception_pending(_py) {
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = end_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }

            for (idx, &val_bits) in elems.iter().enumerate() {
                if idx > 0
                    && let Some(bits) = sep_bits
                {
                    if use_file_handle {
                        let _ = molt_file_write(resolved_file_bits, bits);
                    } else {
                        let res_bits = call_callable1(_py, write_bits, bits);
                        dec_ref_bits(_py, res_bits);
                    }
                    if exception_pending(_py) {
                        if !use_file_handle {
                            dec_ref_bits(_py, write_bits);
                        }
                        if let Some(bits) = sep_bits {
                            dec_ref_bits(_py, bits);
                        }
                        if let Some(bits) = end_bits {
                            dec_ref_bits(_py, bits);
                        }
                        if file_from_sys {
                            dec_ref_bits(_py, resolved_file_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
                let str_bits = molt_str_from_obj(val_bits);
                if exception_pending(_py) {
                    if !use_file_handle {
                        dec_ref_bits(_py, write_bits);
                    }
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = end_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
                if use_file_handle {
                    let _ = molt_file_write(resolved_file_bits, str_bits);
                } else {
                    let res_bits = call_callable1(_py, write_bits, str_bits);
                    dec_ref_bits(_py, res_bits);
                }
                dec_ref_bits(_py, str_bits);
                if exception_pending(_py) {
                    if !use_file_handle {
                        dec_ref_bits(_py, write_bits);
                    }
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = end_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            if let Some(bits) = end_bits {
                if use_file_handle {
                    let _ = molt_file_write(resolved_file_bits, bits);
                } else {
                    let res_bits = call_callable1(_py, write_bits, bits);
                    dec_ref_bits(_py, res_bits);
                }
                if exception_pending(_py) {
                    if !use_file_handle {
                        dec_ref_bits(_py, write_bits);
                    }
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    dec_ref_bits(_py, bits);
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            if !use_file_handle {
                dec_ref_bits(_py, write_bits);
            }
            if let Some(bits) = sep_bits {
                dec_ref_bits(_py, bits);
            }
            if let Some(bits) = end_bits {
                dec_ref_bits(_py, bits);
            }

            if do_flush || (file_from_sys && use_file_handle && end_has_newline) {
                if use_file_handle {
                    let _ = molt_file_flush(resolved_file_bits);
                } else {
                    let flush_name_bits =
                        intern_static_name(_py, &runtime_state(_py).interned.flush_name, b"flush");
                    let flush_method_bits = molt_get_attr_name(resolved_file_bits, flush_name_bits);
                    if exception_pending(_py) {
                        if file_from_sys {
                            dec_ref_bits(_py, resolved_file_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let flush_res_bits = call_callable0(_py, flush_method_bits);
                    dec_ref_bits(_py, flush_method_bits);
                    dec_ref_bits(_py, flush_res_bits);
                    if exception_pending(_py) {
                        if file_from_sys {
                            dec_ref_bits(_py, resolved_file_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
            }
            if file_from_sys {
                dec_ref_bits(_py, resolved_file_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_input_builtin(prompt_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sys_name_bits = intern_static_name(_py, &runtime_state(_py).interned.sys_name, b"sys");
        if obj_from_bits(sys_name_bits).is_none() {
            return raise_exception::<_>(_py, "RuntimeError", "sys module name missing");
        }
        let sys_bits = molt_module_cache_get(sys_name_bits);
        if obj_from_bits(sys_bits).is_none() {
            return raise_exception::<_>(_py, "RuntimeError", "sys module unavailable");
        }

        let stdout_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.stdout_name, b"stdout");
        let stdout_bits = molt_module_get_attr(sys_bits, stdout_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, sys_bits);
            return MoltObject::none().bits();
        }
        if obj_from_bits(stdout_bits).is_none() {
            dec_ref_bits(_py, sys_bits);
            return raise_exception::<_>(_py, "RuntimeError", "sys.stdout unavailable");
        }

        let prompt_str_bits = molt_str_from_obj(prompt_bits);
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
            let _ = molt_file_write(stdout_bits, prompt_str_bits);
        } else {
            let write_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.write_name, b"write");
            let write_method_bits = molt_get_attr_name(stdout_bits, write_name_bits);
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
            let _ = molt_file_flush(stdout_bits);
        } else {
            let missing = missing_bits(_py);
            let flush_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.flush_name, b"flush");
            let flush_bits = molt_getattr_builtin(stdout_bits, flush_name_bits, missing);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
            if flush_bits != missing {
                let callable_bits = molt_is_callable(flush_bits);
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

        let stdin_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.stdin_name, b"stdin");
        let stdin_bits = molt_module_get_attr(sys_bits, stdin_name_bits);
        dec_ref_bits(_py, sys_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if obj_from_bits(stdin_bits).is_none() {
            return raise_exception::<_>(_py, "RuntimeError", "sys.stdin unavailable");
        }

        let mut stdin_is_handle = false;
        if let Some(ptr) = obj_from_bits(stdin_bits).as_ptr() {
            unsafe {
                stdin_is_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
        }
        let line_bits = if stdin_is_handle {
            molt_file_readline(stdin_bits, MoltObject::from_int(-1).bits())
        } else {
            let readline_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.readline_name, b"readline");
            let method_bits = molt_get_attr_name(stdin_bits, readline_name_bits);
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
            return raise_exception::<_>(_py, "TypeError", "input() returned non-string");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "input() returned non-string");
            }
            let bytes = std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            if bytes.is_empty() {
                dec_ref_bits(_py, line_bits);
                return raise_exception::<_>(_py, "EOFError", "");
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_builtin(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_super_new(type_bits, obj_bits) })
}

// ---------------------------------------------------------------------------
// Type-constructor builtins: thin `extern "C"` wrappers so the compiler can
// emit direct calls to `molt_<type>_builtin` for Python's builtin types.
// ---------------------------------------------------------------------------

/// `int(x=0, base=10)` — wraps `molt_int_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_int_builtin(val_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            // int() with no args => 0
            return MoltObject::from_int(0).bits();
        }
        let has_base = base_bits != missing;
        let has_base_bits = if has_base { 1u64 } else { 0u64 };
        let actual_base = if has_base {
            base_bits
        } else {
            MoltObject::from_int(10).bits()
        };
        molt_int_from_obj(val_bits, actual_base, has_base_bits)
    })
}

/// `float(x=0.0)` — wraps `molt_float_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_float_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_float(0.0).bits();
        }
        molt_float_from_obj(val_bits)
    })
}

/// `bool(x=False)` — wraps `is_truthy`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bool_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_bool(false).bits();
        }
        MoltObject::from_bool(is_truthy(_py, obj_from_bits(val_bits))).bits()
    })
}

/// `str(object='')` — wraps `molt_str_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_str_from_obj(val_bits)
    })
}

/// `bytes(source=b'')` — wraps `molt_bytes_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytes_from_obj(val_bits)
    })
}

/// `bytearray(source=bytearray())` — wraps `molt_bytearray_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytearray(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytearray_from_obj(val_bits)
    })
}

/// `list(iterable=())` — constructs a list from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        unsafe {
            let Some(bits) = list_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `tuple(iterable=())` — constructs a tuple from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        unsafe {
            let Some(bits) = tuple_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `dict(mapping_or_iterable=None)` — wraps `molt_dict_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_dict_new(0);
        }
        molt_dict_from_obj(val_bits)
    })
}

/// `set(iterable=())` — constructs a set from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_set_new(0);
        }
        let set_bits = molt_set_new(0);
        if obj_from_bits(set_bits).is_none() {
            return MoltObject::none().bits();
        }
        let _ = molt_set_update(set_bits, val_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, set_bits);
            return MoltObject::none().bits();
        }
        set_bits
    })
}

/// `frozenset(iterable=())` — constructs a frozenset from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_frozenset_new(0);
        }
        unsafe {
            let Some(bits) = frozenset_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `range(stop)` / `range(start, stop[, step])` — wraps `molt_range_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_range_builtin(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "range expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // range(stop) — single-arg form
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            return molt_range_new(zero, start_bits, one);
        }
        let actual_step = if step_bits == missing {
            MoltObject::from_int(1).bits()
        } else {
            step_bits
        };
        molt_range_new(start_bits, stop_bits, actual_step)
    })
}

/// `slice(stop)` / `slice(start, stop[, step])` — wraps `molt_slice_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_builtin(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "slice expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // slice(stop) — single-arg form
            return molt_slice_new(none, start_bits, none);
        }
        let actual_step = if step_bits == missing {
            none
        } else {
            step_bits
        };
        molt_slice_new(start_bits, stop_bits, actual_step)
    })
}

/// `object()` — wraps `molt_object_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_builtin() -> u64 {
    molt_object_new()
}

/// `type(object)` — wraps `molt_builtin_type`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_type_builtin(val_bits: u64) -> u64 {
    molt_builtin_type(val_bits)
}

/// `complex(real=0, imag=0)` — wraps `molt_complex_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_builtin(real_bits: u64, imag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let actual_real = if real_bits == missing {
            MoltObject::from_int(0).bits()
        } else {
            real_bits
        };
        let has_imag = imag_bits != missing;
        let has_imag_bits = if has_imag { 1u64 } else { 0u64 };
        let actual_imag = if has_imag {
            imag_bits
        } else {
            MoltObject::from_int(0).bits()
        };
        molt_complex_from_obj(actual_real, actual_imag, has_imag_bits)
    })
}

/// `memoryview(obj)` — wraps `molt_memoryview_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_builtin(val_bits: u64) -> u64 {
    molt_memoryview_new(val_bits)
}

/// `classmethod(func)` — wraps `molt_classmethod_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_builtin(func_bits: u64) -> u64 {
    molt_classmethod_new(func_bits)
}

/// `staticmethod(func)` — wraps `molt_staticmethod_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_builtin(func_bits: u64) -> u64 {
    molt_staticmethod_new(func_bits)
}

/// `property(fget=None, fset=None, fdel=None)` — wraps `molt_property_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_builtin(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        let g = if get_bits == missing { none } else { get_bits };
        let s = if set_bits == missing { none } else { set_bits };
        let d = if del_bits == missing { none } else { del_bits };
        molt_property_new(g, s, d)
    })
}

/// `isinstance(obj, classinfo)` — wraps `molt_isinstance`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isinstance_builtin(val_bits: u64, class_bits: u64) -> u64 {
    molt_isinstance(val_bits, class_bits)
}

/// `issubclass(sub, classinfo)` — wraps `molt_issubclass`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_issubclass_builtin(sub_bits: u64, class_bits: u64) -> u64 {
    molt_issubclass(sub_bits, class_bits)
}

/// `hasattr(obj, name)` — wraps `molt_has_attr_name`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_hasattr_builtin(obj_bits: u64, name_bits: u64) -> u64 {
    molt_has_attr_name(obj_bits, name_bits)
}

/// `aiter(async_iterable)` — wraps `molt_aiter`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_aiter_builtin(obj_bits: u64) -> u64 {
    molt_aiter(obj_bits)
}

/// `iter(object)` — wraps `molt_iter_checked`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_builtin(obj_bits: u64) -> u64 {
    molt_iter_checked(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice(obj_bits: u64, start_bits: u64, end_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let start_obj = obj_from_bits(start_bits);
        let end_obj = obj_from_bits(end_bits);
        if let Some(ptr) = obj.as_ptr() {
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
                    let len = memoryview_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let base_offset = memoryview_offset(ptr);
                        let stride = memoryview_stride(ptr);
                        let out_ptr = alloc_memoryview(
                            _py,
                            memoryview_owner_bits(ptr),
                            base_offset + start * stride,
                            0,
                            memoryview_itemsize(ptr),
                            stride,
                            memoryview_readonly(ptr),
                            memoryview_format_bits(ptr),
                        );
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let base_offset = memoryview_offset(ptr);
                    let new_offset = base_offset + start * memoryview_stride(ptr);
                    let new_len = (end - start) as usize;
                    let out_ptr = alloc_memoryview(
                        _py,
                        memoryview_owner_bits(ptr),
                        new_offset,
                        new_len,
                        memoryview_itemsize(ptr),
                        memoryview_stride(ptr),
                        memoryview_readonly(ptr),
                        memoryview_format_bits(ptr),
                    );
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
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
                    let elems = seq_vec_ref(ptr);
                    let slice = &elems[start as usize..end as usize];
                    let out = alloc_list(_py, slice);
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
                    let elems = seq_vec_ref(ptr);
                    let slice = &elems[start as usize..end as usize];
                    let out = alloc_tuple(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
            }
        }
        let slice_bits = molt_slice_new(start_bits, end_bits, MoltObject::none().bits());
        if obj_from_bits(slice_bits).is_none() {
            return MoltObject::none().bits();
        }
        let res_bits = molt_index(obj_bits, slice_bits);
        dec_ref_bits(_py, slice_bits);
        res_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_intarray_from_seq(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    seq_vec_ref(ptr)
                } else {
                    return MoltObject::none().bits();
                };
                let mut out = Vec::with_capacity(elems.len());
                for &elem in elems {
                    let val = MoltObject::from_bits(elem);
                    if let Some(i) = val.as_int() {
                        out.push(i);
                    } else {
                        return MoltObject::none().bits();
                    }
                }
                let out_ptr = alloc_intarray(_py, &out);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_from_list(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_TUPLE {
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                if type_id == TYPE_ID_LIST {
                    let elems = seq_vec_ref(ptr);
                    let out_ptr = alloc_tuple(_py, elems);
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
