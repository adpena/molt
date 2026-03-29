use crate::object::ops::string_obj_to_owned;
use crate::{
    CALL_DISPATCH_COUNT, HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN,
    HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED, PyToken, TYPE_ID_FUNCTION, TYPE_ID_TUPLE,
    ensure_function_code_bits, frame_stack_pop, frame_stack_push, function_arity,
    function_attr_bits, function_closure_bits, function_fn_ptr, function_name_bits,
    function_trampoline_ptr, header_from_obj_ptr, intern_static_name, is_truthy, obj_from_bits,
    object_type_id, profile_hit, raise_exception, recursion_guard_enter, recursion_guard_exit,
    runtime_state, seq_vec_ref,
};

#[cfg(target_arch = "wasm32")]
use crate::MoltObject;
#[cfg(target_arch = "wasm32")]
use crate::{
    molt_call_indirect0, molt_call_indirect1, molt_call_indirect2, molt_call_indirect3,
    molt_call_indirect4, molt_call_indirect5, molt_call_indirect6, molt_call_indirect7,
    molt_call_indirect8, molt_call_indirect9, molt_call_indirect10, molt_call_indirect11,
    molt_call_indirect12, molt_call_indirect13,
};

#[cfg(target_arch = "wasm32")]
#[inline]
fn wasm_direct_call_table_idx(fn_ptr: u64) -> u64 {
    crate::builtins::functions::reserved_wasm_runtime_callable_ptr(fn_ptr).unwrap_or(fn_ptr)
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn is_reserved_wasm_runtime_callable(fn_ptr: u64) -> bool {
    crate::builtins::functions::reserved_wasm_runtime_callable_ptr(fn_ptr).is_some()
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn is_void_wasm_call1_target(fn_ptr: u64) -> bool {
    const VOID_INTRINSICS: [&str; 9] = [
        "molt_email_message_drop",
        "molt_process_drop",
        "molt_stream_reader_drop",
        "molt_stream_close",
        "molt_stream_drop",
        "molt_ws_close",
        "molt_ws_drop",
        "molt_socket_reader_drop",
        "molt_socket_drop",
    ];
    for name in VOID_INTRINSICS {
        if crate::intrinsics::resolve_symbol(name) == Some(fn_ptr) {
            return true;
        }
    }
    false
}

unsafe fn raise_call_arity_mismatch(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    expected: u64,
    got: u64,
) -> u64 {
    unsafe {
        let mut msg = format!("call arity mismatch (expected {expected}, got {got})");
        let name_bits = function_name_bits(_py, func_ptr);
        if name_bits != 0
            && let Some(name) = string_obj_to_owned(obj_from_bits(name_bits))
        {
            msg.push_str(" for ");
            msg.push_str(&name);
        }
        raise_exception::<_>(_py, "TypeError", &msg)
    }
}

#[inline]
unsafe fn maybe_call_function_obj_trampoline_native(
    _py: &PyToken<'_>,
    func_bits: u64,
    func_ptr: *mut u8,
    args: &[u64],
) -> Option<u64> {
    #[cfg(not(target_arch = "wasm32"))]
    unsafe {
        if function_trampoline_ptr(func_ptr) != 0 {
            return Some(call_function_obj_trampoline(_py, func_bits, args));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (_py, func_bits, func_ptr, args);
    None
}

pub(crate) unsafe fn call_function_obj1(_py: &PyToken<'_>, func_bits: u64, arg0_bits: u64) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 1 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 1);
        }
        if let Some(res) =
            maybe_call_function_obj_trampoline_native(_py, func_bits, func_ptr, &[arg0_bits])
        {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let debug_enabled = {
            use std::sync::atomic::{AtomicU8, Ordering};
            static CACHED: AtomicU8 = AtomicU8::new(2); // 2 = unchecked
            let c = CACHED.load(Ordering::Relaxed);
            if c == 2 {
                let v = std::env::var("MOLT_WASM_CALL_DEBUG").as_deref() == Ok("1");
                CACHED.store(v as u8, Ordering::Relaxed);
                v
            } else {
                c == 1
            }
        };
        #[cfg(target_arch = "wasm32")]
        let debug_name = if debug_enabled {
            let name_bits = function_name_bits(_py, func_ptr);
            if name_bits != 0 {
                string_obj_to_owned(obj_from_bits(name_bits))
            } else {
                None
            }
        } else {
            None
        };
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    if debug_enabled {
                        eprintln!(
                            "molt wasm call1 indirect2: name={} fn=0x{fn_ptr:x} tramp=0x{tramp_ptr:x}",
                            debug_name.as_deref().unwrap_or("<unnamed>"),
                        );
                    }
                    molt_call_indirect2(wasm_direct_call_table_idx(fn_ptr), closure_bits, arg0_bits)
                        as u64
                } else {
                    if debug_enabled {
                        eprintln!(
                            "molt wasm call1 direct2: name={} fn=0x{fn_ptr:x}",
                            debug_name.as_deref().unwrap_or("<unnamed>"),
                        );
                    }
                    // SAFETY: `fn_ptr` was read from the function object via `function_fn_ptr`,
                    // which returns the code pointer set by the compiler during code generation
                    // (see `emit_call` in wasm.rs). The arity was verified to be 1 above, plus
                    // closure_bits != 0 so we use the 2-arg signature (closure, arg0). If fn_ptr
                    // is null or points to a function with a different ABI, this is UB — the
                    // compiler must emit valid non-null pointers with matching extern "C" ABI.
                    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(closure_bits, arg0_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: Same invariant as the wasm32 path above — fn_ptr is a valid extern "C"
                // function pointer from `function_fn_ptr`. Arity == 1, closure present, so the
                // 2-arg (closure, arg0) signature matches. The compiler guarantees this pointer
                // was emitted for a 1-arg closure function. UB if fn_ptr is null or mistyped.
                let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(closure_bits, arg0_bits) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    if debug_enabled {
                        eprintln!(
                            "molt wasm call1 indirect1: name={} fn=0x{fn_ptr:x} tramp=0x{tramp_ptr:x}",
                            debug_name.as_deref().unwrap_or("<unnamed>"),
                        );
                    }
                    molt_call_indirect1(wasm_direct_call_table_idx(fn_ptr), arg0_bits) as u64
                } else {
                    if debug_enabled {
                        eprintln!(
                            "molt wasm call1 direct1: name={} fn=0x{fn_ptr:x}",
                            debug_name.as_deref().unwrap_or("<unnamed>"),
                        );
                    }
                    if is_void_wasm_call1_target(fn_ptr) {
                        // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                        // `function_fn_ptr`. This branch handles void intrinsics (drop/close
                        // functions) identified by `is_void_wasm_call1_target` — these return
                        // nothing, so the void signature `fn(u64)` is correct. The compiler and
                        // intrinsic registry must guarantee fn_ptr targets a void-returning
                        // function. UB if fn_ptr is null or the target actually returns a value.
                        let func: extern "C" fn(u64) = std::mem::transmute(fn_ptr as usize);
                        func(arg0_bits);
                        MoltObject::none().bits()
                    } else {
                        // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                        // `function_fn_ptr`. Arity == 1, no closure, so the 1-arg signature
                        // `fn(u64) -> i64` is correct. The compiler guarantees this pointer
                        // was emitted for a 1-arg non-closure function. UB if fn_ptr is null
                        // or the target has a different calling convention.
                        let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                        func(arg0_bits) as u64
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` is a valid extern "C" function pointer from `function_fn_ptr`.
                // Arity == 1, no closure, so the 1-arg signature `fn(u64) -> i64` matches. The
                // compiler must emit a valid non-null pointer for this function. UB if fn_ptr is
                // null or points to a function with a different signature.
                let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(arg0_bits) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

unsafe fn function_needs_task_trampoline(_py: &PyToken<'_>, func_bits: u64) -> bool {
    unsafe {
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return false;
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return false;
        }
        if let Some(cached) = function_task_trampoline_cached(func_ptr) {
            return cached;
        }
        refresh_function_task_trampoline_cache(_py, func_ptr)
    }
}

unsafe fn function_task_trampoline_cached(func_ptr: *mut u8) -> Option<bool> {
    unsafe {
        let header = header_from_obj_ptr(func_ptr);
        let flags = (*header).flags;
        if (flags & HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN) == 0 {
            return None;
        }
        Some((flags & HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED) != 0)
    }
}

pub(crate) unsafe fn refresh_function_task_trampoline_cache(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> bool {
    unsafe {
        let needed = compute_function_task_trampoline_needed(_py, func_ptr);
        let header = header_from_obj_ptr(func_ptr);
        let mut flags = (*header).flags | HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN;
        if needed {
            flags |= HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED;
        } else {
            flags &= !HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED;
        }
        (*header).flags = flags;
        needed
    }
}

unsafe fn compute_function_task_trampoline_needed(_py: &PyToken<'_>, func_ptr: *mut u8) -> bool {
    unsafe {
        let interned = &runtime_state(_py).interned;
        let gen_name =
            intern_static_name(_py, &interned.molt_is_generator, b"__molt_is_generator__");
        if let Some(bits) = function_attr_bits(_py, func_ptr, gen_name)
            && is_truthy(_py, obj_from_bits(bits))
        {
            return true;
        }
        let coro_name =
            intern_static_name(_py, &interned.molt_is_coroutine, b"__molt_is_coroutine__");
        if let Some(bits) = function_attr_bits(_py, func_ptr, coro_name)
            && is_truthy(_py, obj_from_bits(bits))
        {
            return true;
        }
        let asyncgen_name = intern_static_name(
            _py,
            &interned.molt_is_async_generator,
            b"__molt_is_async_generator__",
        );
        if let Some(bits) = function_attr_bits(_py, func_ptr, asyncgen_name)
            && is_truthy(_py, obj_from_bits(bits))
        {
            return true;
        }
        false
    }
}

pub(crate) unsafe fn call_function_obj0(_py: &PyToken<'_>, func_bits: u64) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 0 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 0);
        }
        if let Some(res) = maybe_call_function_obj_trampoline_native(_py, func_bits, func_ptr, &[])
        {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect1(wasm_direct_call_table_idx(fn_ptr), closure_bits) as u64
                } else {
                    // SAFETY: `fn_ptr` is a valid extern "C" function pointer obtained from
                    // `function_fn_ptr(func_ptr)`, which reads the code pointer stored in the
                    // function object by the compiler (see `emit_call` in wasm.rs). Arity == 0
                    // and closure_bits != 0, so the 1-arg signature `fn(u64) -> i64` is correct
                    // (the single arg is the closure environment). The compiler must guarantee
                    // fn_ptr is non-null and targets a matching ABI. UB if violated.
                    let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(closure_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: Same invariant as the wasm32 closure path — fn_ptr from
                // `function_fn_ptr` is a valid extern "C" pointer for a 0-arg function with
                // closure. The 1-arg signature passes the closure env. The compiler must emit
                // a non-null pointer with matching ABI. UB if fn_ptr is null or mistyped.
                let func: extern "C" fn(u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(closure_bits) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect0(wasm_direct_call_table_idx(fn_ptr)) as u64
                } else {
                    // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                    // `function_fn_ptr`. Arity == 0, no closure, so the nullary signature
                    // `fn() -> i64` is correct. The compiler must guarantee fn_ptr is non-null
                    // and targets a 0-arg extern "C" function. UB if fn_ptr is null or has a
                    // different calling convention or arity.
                    let func: extern "C" fn() -> i64 = std::mem::transmute(fn_ptr as usize);
                    func() as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                // `function_fn_ptr`. Arity == 0, no closure, so the nullary signature
                // `fn() -> i64` is correct. The compiler must emit a valid non-null pointer.
                // UB if fn_ptr is null or points to a function expecting arguments.
                let func: extern "C" fn() -> i64 = std::mem::transmute(fn_ptr as usize);
                func() as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

pub(crate) unsafe fn call_function_obj2(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 2 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 2);
        }
        if let Some(res) = maybe_call_function_obj_trampoline_native(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect3(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                    // `function_fn_ptr(func_ptr)`, set by the compiler during code generation.
                    // Arity == 2 and closure_bits != 0, so the 3-arg signature
                    // `fn(u64, u64, u64) -> i64` is correct (closure + 2 args). The compiler
                    // must guarantee fn_ptr is non-null and targets a matching ABI. UB if
                    // fn_ptr is null or the target has a different parameter count.
                    let func: extern "C" fn(u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(closure_bits, arg0_bits, arg1_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: Same invariant as the wasm32 closure path — fn_ptr from
                // `function_fn_ptr` is a valid extern "C" pointer for a 2-arg function with
                // closure. The 3-arg signature passes (closure, arg0, arg1). The compiler must
                // emit a non-null pointer with matching ABI. UB if fn_ptr is null or mistyped.
                let func: extern "C" fn(u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(closure_bits, arg0_bits, arg1_bits) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect2(wasm_direct_call_table_idx(fn_ptr), arg0_bits, arg1_bits)
                        as u64
                } else {
                    // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                    // `function_fn_ptr`. Arity == 2, no closure, so the 2-arg signature
                    // `fn(u64, u64) -> i64` is correct. The compiler must guarantee fn_ptr is
                    // non-null and targets a matching ABI. UB if fn_ptr is null or mistyped.
                    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(arg0_bits, arg1_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: Same invariant as the wasm32 non-closure path — fn_ptr from
                // `function_fn_ptr` targets a 2-arg extern "C" function. The compiler must
                // emit a valid non-null pointer. UB if fn_ptr is null or has wrong arity.
                let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(arg0_bits, arg1_bits) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

pub(crate) unsafe fn call_function_obj3(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 3 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 3);
        }
        if let Some(res) = maybe_call_function_obj_trampoline_native(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits, arg2_bits],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect4(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(closure_bits, arg0_bits, arg1_bits, arg2_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(closure_bits, arg0_bits, arg1_bits, arg2_bits) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect3(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(arg0_bits, arg1_bits, arg2_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(arg0_bits, arg1_bits, arg2_bits) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

pub(crate) unsafe fn call_function_obj4(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 4 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 4);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect5(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(closure_bits, arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(closure_bits, arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect4(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

unsafe fn call_function_obj5(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 5 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 5);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect6(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect5(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj6(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 6 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 6);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect7(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect6(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj7(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 7 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 7);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect8(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                    arg6_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect7(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj8(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
    arg7_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 8 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 8);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect9(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                    arg6_bits,
                    arg7_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect8(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                        arg6_bits, arg7_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj9(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
    arg7_bits: u64,
    arg8_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 9 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 9);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect10(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(
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
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                    arg6_bits,
                    arg7_bits,
                    arg8_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect9(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                        std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                        arg6_bits, arg7_bits, arg8_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj10(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
    arg7_bits: u64,
    arg8_bits: u64,
    arg9_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 10 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 10);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect11(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                    ) as u64
                } else {
                    let func: extern "C" fn(
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
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let func: extern "C" fn(
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
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                ) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                    arg6_bits,
                    arg7_bits,
                    arg8_bits,
                    arg9_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect10(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                    ) as u64
                } else {
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    let func: extern "C" fn(
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
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                        arg6_bits, arg7_bits, arg8_bits, arg9_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i64 =
                    std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits, arg9_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj11(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
    arg7_bits: u64,
    arg8_bits: u64,
    arg9_bits: u64,
    arg10_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 11 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 11);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect12(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                        arg10_bits,
                    ) as u64
                } else {
                    let func: extern "C" fn(
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
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                        arg10_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let func: extern "C" fn(
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
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                ) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                    arg6_bits,
                    arg7_bits,
                    arg8_bits,
                    arg9_bits,
                    arg10_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect11(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                        arg10_bits,
                    ) as u64
                } else {
                    let func: extern "C" fn(
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
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                        arg6_bits, arg7_bits, arg8_bits, arg9_bits, arg10_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let func: extern "C" fn(
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
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                ) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits, arg9_bits, arg10_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj12(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg0_bits: u64,
    arg1_bits: u64,
    arg2_bits: u64,
    arg3_bits: u64,
    arg4_bits: u64,
    arg5_bits: u64,
    arg6_bits: u64,
    arg7_bits: u64,
    arg8_bits: u64,
    arg9_bits: u64,
    arg10_bits: u64,
    arg11_bits: u64,
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != 12 {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 12);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect13(
                        wasm_direct_call_table_idx(fn_ptr),
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                        arg10_bits,
                        arg11_bits,
                    ) as u64
                } else {
                    let func: extern "C" fn(
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
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                        arg10_bits,
                        arg11_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let func: extern "C" fn(
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
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                ) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(
                    closure_bits,
                    arg0_bits,
                    arg1_bits,
                    arg2_bits,
                    arg3_bits,
                    arg4_bits,
                    arg5_bits,
                    arg6_bits,
                    arg7_bits,
                    arg8_bits,
                    arg9_bits,
                    arg10_bits,
                    arg11_bits,
                ) as u64
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect12(
                        wasm_direct_call_table_idx(fn_ptr),
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits,
                        arg5_bits,
                        arg6_bits,
                        arg7_bits,
                        arg8_bits,
                        arg9_bits,
                        arg10_bits,
                        arg11_bits,
                    ) as u64
                } else {
                    let func: extern "C" fn(
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
                    // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                    // Arity verified above; signature matches. Compiler guarantees ABI match.
                    ) -> i64 = std::mem::transmute(fn_ptr as usize);
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                        arg6_bits, arg7_bits, arg8_bits, arg9_bits, arg10_bits, arg11_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let func: extern "C" fn(
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
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                ) -> i64 = std::mem::transmute(fn_ptr as usize);
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits, arg9_bits, arg10_bits, arg11_bits,
                ) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

unsafe fn call_function_obj_trampoline(_py: &PyToken<'_>, func_bits: u64, args: &[u64]) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        let arity = function_arity(func_ptr);
        if arity != args.len() as u64 {
            // Arity mismatch: the caller provided a different number of args
            // than the function's stored arity.  Instead of immediately
            // erroring, try to resolve via __defaults__ (for too-few args) or
            // fall back to the full argument-binding path (for too-many args
            // or when defaults are insufficient).
            //
            // This handles the WASM dispatch case where a user function with
            // keyword default arguments (e.g. `def f(a, b, lo=0, hi=100)`)
            // is called through the trampoline path with only the required
            // positional args — the previous code raised immediately without
            // consulting __defaults__.
            let n = args.len();
            let a = arity as usize;
            if n < a {
                // Try to pad missing args from __defaults__ tuple.
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
                                let missing = a - n;
                                if missing <= n_defaults {
                                    let mut padded = Vec::with_capacity(a);
                                    padded.extend_from_slice(args);
                                    let start = n_defaults - missing;
                                    for i in start..n_defaults {
                                        padded.push(defaults[i]);
                                    }
                                    // Recurse with the padded args — arity now matches.
                                    return call_function_obj_trampoline(_py, func_bits, &padded);
                                }
                            }
                        }
                    }
                }
            }
            // Could not resolve the mismatch via __defaults__.
            // Return a clear arity mismatch error. The __defaults__ fast path
            // handles the common case; varargs/kwargs dispatch is handled by
            // the CallArgs path which is entered from a different call site.
            return raise_call_arity_mismatch(_py, func_ptr, a as u64, n as u64);
        }
        let tramp_ptr = function_trampoline_ptr(func_ptr);
        if tramp_ptr == 0 {
            return raise_exception::<_>(_py, "TypeError", "call arity mismatch");
        }
        let closure_bits = function_closure_bits(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = {
            #[cfg(target_arch = "wasm32")]
            {
                molt_call_indirect3(
                    tramp_ptr,
                    closure_bits,
                    args.as_ptr() as u64,
                    args.len() as u64,
                ) as u64
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `tramp_ptr` from `function_trampoline_ptr` is a valid extern "C"
                // fn pointer emitted by the compiler. Arity is verified above. UB if null.
                let func: extern "C" fn(u64, u64, u64) -> i64 =
                    std::mem::transmute(tramp_ptr as usize);
                func(closure_bits, args.as_ptr() as u64, args.len() as u64) as u64
            }
        };
        frame_stack_pop(_py);
        recursion_guard_exit();
        res
    }
}

pub(crate) unsafe fn call_function_obj_vec(_py: &PyToken<'_>, func_bits: u64, args: &[u64]) -> u64 {
    unsafe {
        let func_obj = obj_from_bits(func_bits);
        if let Some(func_ptr) = func_obj.as_ptr()
            && object_type_id(func_ptr) == TYPE_ID_FUNCTION
        {
            let tramp_ptr = function_trampoline_ptr(func_ptr);
            if tramp_ptr != 0 {
                #[cfg(target_arch = "wasm32")]
                {
                    if !is_reserved_wasm_runtime_callable(function_fn_ptr(func_ptr)) {
                        return call_function_obj_trampoline(_py, func_bits, args);
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    return call_function_obj_trampoline(_py, func_bits, args);
                }
            }
        }
        if function_needs_task_trampoline(_py, func_bits) {
            return call_function_obj_trampoline(_py, func_bits, args);
        }
        match args.len() {
            0 => call_function_obj0(_py, func_bits),
            1 => call_function_obj1(_py, func_bits, args[0]),
            2 => call_function_obj2(_py, func_bits, args[0], args[1]),
            3 => call_function_obj3(_py, func_bits, args[0], args[1], args[2]),
            4 => call_function_obj4(_py, func_bits, args[0], args[1], args[2], args[3]),
            5 => call_function_obj5(_py, func_bits, args[0], args[1], args[2], args[3], args[4]),
            6 => call_function_obj6(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5],
            ),
            7 => call_function_obj7(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
            ),
            8 => call_function_obj8(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7],
            ),
            9 => call_function_obj9(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8],
            ),
            10 => call_function_obj10(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8], args[9],
            ),
            11 => call_function_obj11(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8], args[9], args[10],
            ),
            12 => call_function_obj12(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8], args[9], args[10], args[11],
            ),
            _ => call_function_obj_trampoline(_py, func_bits, args),
        }
    }
}
