use crate::builtins::functions::runtime_callable_target_ptr;
use crate::object::layout::function_call_target_ptr;
use crate::object::ops::string_obj_to_owned;
use crate::{
    CALL_DISPATCH_COUNT, HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN,
    HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED, PyToken, TYPE_ID_FUNCTION, TYPE_ID_TUPLE,
    ensure_function_code_bits, exception_pending, exception_stack_baseline_get,
    exception_stack_baseline_set, frame_stack_pop, frame_stack_push, function_arity,
    function_attr_bits, function_closure_bits, function_fn_ptr, function_name_bits,
    function_trampoline_ptr, header_from_obj_ptr, intern_static_name, is_truthy,
    molt_exception_clear, obj_from_bits, object_type_id, profile_hit, raise_exception,
    recursion_guard_enter, recursion_guard_exit, runtime_state, seq_vec_ref, type_name,
};

#[cfg(target_arch = "wasm32")]
use crate::MoltObject;
#[cfg(target_arch = "wasm32")]
use crate::{
    inc_ref_bits,
    molt_call_indirect0, molt_call_indirect1, molt_call_indirect2, molt_call_indirect3,
    molt_call_indirect4, molt_call_indirect5, molt_call_indirect6, molt_call_indirect7,
    molt_call_indirect8, molt_call_indirect9, molt_call_indirect10, molt_call_indirect11,
    molt_call_indirect12, molt_call_indirect13,
};

#[cfg(target_arch = "wasm32")]
#[inline]
fn wasm_direct_call_table_idx(fn_ptr: u64) -> u64 {
    crate::builtins::functions::normalize_runtime_callable_ptr(fn_ptr)
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn select_wasm_fixed_arity_call_target(direct_target: u64, tramp_ptr: u64) -> u64 {
    if u32::try_from(direct_target).is_ok() {
        return direct_target;
    }
    if tramp_ptr != 0 {
        return tramp_ptr;
    }
    direct_target
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[inline]
fn fixed_arity_call_target_ptr(fn_ptr: u64, tramp_ptr: u64) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        let direct_target = wasm_direct_call_table_idx(fn_ptr);
        let normalized_tramp =
            crate::builtins::functions::normalize_runtime_trampoline_ptr(fn_ptr, tramp_ptr);
        select_wasm_fixed_arity_call_target(direct_target, normalized_tramp)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = tramp_ptr;
        fn_ptr
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[inline]
fn fixed_arity_trampoline_target_ptr(fn_ptr: u64, tramp_ptr: u64) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        let normalized_tramp =
            crate::builtins::functions::normalize_runtime_trampoline_ptr(fn_ptr, tramp_ptr);
        if normalized_tramp != 0 {
            return normalized_tramp;
        }
        wasm_direct_call_table_idx(fn_ptr)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if tramp_ptr != 0 { tramp_ptr } else { fn_ptr }
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn can_use_fixed_arity_wasm_trampoline(fn_ptr: u64, tramp_ptr: u64) -> bool {
    let direct = crate::builtins::functions::normalize_runtime_callable_ptr(fn_ptr);
    let normalized_tramp =
        crate::builtins::functions::normalize_runtime_trampoline_ptr(fn_ptr, tramp_ptr);
    u32::try_from(direct).is_ok() && u32::try_from(normalized_tramp).is_ok()
}

#[inline]
unsafe fn normalized_function_trampoline_ptr(func_ptr: *mut u8, fn_ptr: u64) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        crate::builtins::functions::normalize_runtime_trampoline_ptr(
            fn_ptr,
            unsafe { function_trampoline_ptr(func_ptr) },
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = fn_ptr;
        unsafe { function_trampoline_ptr(func_ptr) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
unsafe fn function_call_target_or_legacy_ptr(func_ptr: *mut u8, fn_ptr: u64) -> *const () {
    let target = unsafe { function_call_target_ptr(func_ptr) };
    if target.is_null() {
        fn_ptr as usize as *const ()
    } else {
        target
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
unsafe fn function_runtime_call_target_ptr(func_ptr: *mut u8, fn_ptr: u64) -> Option<*const ()> {
    let target = unsafe { function_call_target_ptr(func_ptr) };
    if !target.is_null() {
        return Some(target);
    }
    runtime_callable_target_ptr(fn_ptr)
}

#[cfg(not(target_arch = "wasm32"))]
macro_rules! call_native_fixed_arity {
    ($func_ptr:expr, $fn_ptr:expr, $runtime_ty:ty, $compiled_ty:ty, ($($arg:expr),* $(,)?)) => {{
        if let Some(runtime_target) = function_runtime_call_target_ptr($func_ptr, $fn_ptr) {
            let func: $runtime_ty = std::mem::transmute(runtime_target);
            func($($arg),*) as u64
        } else {
            let call_target = function_call_target_or_legacy_ptr($func_ptr, $fn_ptr);
            let func: $compiled_ty = std::mem::transmute(call_target);
            func($($arg),*) as u64
        }
    }};
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[inline]
fn should_force_trampoline_for_fixed_arity_call(
    direct_target: u64,
    tramp_ptr: u64,
    task_trampoline_needed: bool,
) -> bool {
    let _ = direct_target;
    task_trampoline_needed || tramp_ptr != 0
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

fn trace_call_vec_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_CALL_FUNCTION_VEC")
                .ok()
                .as_deref(),
            Some("1")
        )
    })
}

fn assert_no_pending_on_success_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_ASSERT_NO_PENDING_ON_SUCCESS")
                .ok()
                .as_deref(),
            Some("1")
        )
    })
}

struct ExceptionBaselineGuard {
    prev: usize,
}

impl ExceptionBaselineGuard {
    fn new() -> Self {
        Self {
            prev: exception_stack_baseline_get(),
        }
    }
}

impl Drop for ExceptionBaselineGuard {
    fn drop(&mut self) {
        exception_stack_baseline_set(self.prev);
    }
}

unsafe fn enforce_no_pending_on_success(_py: &PyToken<'_>, result: u64, context: &str) -> u64 {
    if !assert_no_pending_on_success_enabled() || !exception_pending(_py) {
        return result;
    }
    let _ = molt_exception_clear();
    eprintln!("pending exception on success path: {context} result=0x{result:x}");
    std::process::abort();
}

unsafe fn trace_function_vec_call(_py: &PyToken<'_>, func_ptr: *mut u8, args: &[u64], lane: &str) {
    if !trace_call_vec_enabled() {
        return;
    }
    let name_bits = unsafe { function_name_bits(_py, func_ptr) };
    let name = if name_bits != 0 {
        string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<unnamed>".to_string())
    } else {
        "<unnamed>".to_string()
    };
    let fn_ptr = unsafe { function_fn_ptr(func_ptr) };
    let tramp_ptr = unsafe { normalized_function_trampoline_ptr(func_ptr, fn_ptr) };
    let closure_bits = unsafe { function_closure_bits(func_ptr) };
    let arity = unsafe { function_arity(func_ptr) };
    eprintln!(
        "[molt call_function_vec] lane={lane} name={name} fn_ptr=0x{fn_ptr:x} tramp_ptr=0x{tramp_ptr:x} closure_bits=0x{closure_bits:x} arity={arity} argc={}",
        args.len()
    );
    for (idx, &arg_bits) in args.iter().enumerate() {
        let arg_obj = obj_from_bits(arg_bits);
        eprintln!(
            "  arg[{idx}] type={} bits=0x{:x}",
            crate::type_name(_py, arg_obj),
            arg_bits
        );
    }
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
unsafe fn maybe_call_function_obj_trampoline(
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
    unsafe {
        let fn_ptr = function_fn_ptr(func_ptr);
        let tramp_ptr = crate::builtins::functions::normalize_runtime_trampoline_ptr(
            fn_ptr,
            function_trampoline_ptr(func_ptr),
        );
        let direct_target = wasm_direct_call_table_idx(fn_ptr);
        let reserved_info = crate::builtins::functions::reserved_wasm_runtime_callable_info(fn_ptr);
        let force_trampoline = should_force_trampoline_for_fixed_arity_call(
            direct_target,
            tramp_ptr,
            function_needs_task_trampoline(_py, func_bits),
        );
        if matches!(
            std::env::var("MOLT_TRACE_TRAMPOLINE_POLICY")
                .ok()
                .as_deref(),
            Some("1")
        ) {
            eprintln!(
                "[molt trampoline policy] fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} nargs={} reserved_info={reserved_info:?} force_trampoline={}",
                args.len(),
                force_trampoline,
            );
        }
        if force_trampoline {
            return Some(call_function_obj_trampoline(_py, func_bits, args));
        }
    }
    None
}

pub(crate) unsafe fn call_function_obj1(_py: &PyToken<'_>, func_bits: u64, arg0_bits: u64) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let _baseline_guard = ExceptionBaselineGuard::new();
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
            maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, &[arg0_bits])
        {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        #[cfg(target_arch = "wasm32")]
        if matches!(
            std::env::var("MOLT_TRACE_CALL_FUNCTION_OBJ1")
                .ok()
                .as_deref(),
            Some("1")
        ) {
            let name_bits = function_name_bits(_py, func_ptr);
            let name = if name_bits != 0 {
                string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<unnamed>".to_string())
            } else {
                "<unnamed>".to_string()
            };
            eprintln!(
                "[molt call_function_obj1] name={name} fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} closure_bits={closure_bits} arity={arity}"
            );
        }
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect2(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
                        closure_bits,
                        arg0_bits,
                    ) as u64
                } else {
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64) -> u64,
                    extern "C" fn(u64, u64) -> i64,
                    (closure_bits, arg0_bits)
                )
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect1(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
                        arg0_bits,
                    )
                        as u64
                } else {
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
                if let Some(runtime_target) = function_runtime_call_target_ptr(func_ptr, fn_ptr) {
                    let func: extern "C" fn(u64) -> u64 = std::mem::transmute(runtime_target);
                    func(arg0_bits)
                } else {
                    let call_target = function_call_target_or_legacy_ptr(func_ptr, fn_ptr);
                    let func: extern "C" fn(u64) -> i64 = std::mem::transmute(call_target);
                    func(arg0_bits) as u64
                }
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj1");
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj7");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, &[]) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        let res = if closure_bits != 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    if matches!(
                        std::env::var("MOLT_TRACE_CALL_FUNCTION_OBJ0")
                            .ok()
                            .as_deref(),
                        Some("1")
                    ) {
                        let name_bits = function_name_bits(_py, func_ptr);
                        let name = if name_bits != 0 {
                            string_obj_to_owned(obj_from_bits(name_bits))
                                .unwrap_or_else(|| "<unnamed>".to_string())
                        } else {
                            "<unnamed>".to_string()
                        };
                        let target = fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr);
                        eprintln!(
                            "[molt call_function_obj0] name={name} fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} target={target} closure_bits={closure_bits}"
                        );
                    }
                    molt_call_indirect1(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
                        closure_bits,
                    ) as u64
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64) -> u64,
                    extern "C" fn(u64) -> i64,
                    (closure_bits)
                )
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    if matches!(
                        std::env::var("MOLT_TRACE_CALL_FUNCTION_OBJ0")
                            .ok()
                            .as_deref(),
                        Some("1")
                    ) {
                        let name_bits = function_name_bits(_py, func_ptr);
                        let name = if name_bits != 0 {
                            string_obj_to_owned(obj_from_bits(name_bits))
                                .unwrap_or_else(|| "<unnamed>".to_string())
                        } else {
                            "<unnamed>".to_string()
                        };
                        let target = fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr);
                        eprintln!(
                            "[molt call_function_obj0] name={name} fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} target={target} closure_bits={closure_bits}"
                        );
                    }
                    molt_call_indirect0(fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr))
                        as u64
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
                if let Some(runtime_target) = function_runtime_call_target_ptr(func_ptr, fn_ptr) {
                    let func: extern "C" fn() -> u64 = std::mem::transmute(runtime_target);
                    func()
                } else {
                    let call_target = function_call_target_or_legacy_ptr(func_ptr, fn_ptr);
                    let func: extern "C" fn() -> i64 = std::mem::transmute(call_target);
                    func() as u64
                }
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj0");
        if matches!(
            std::env::var("MOLT_TRACE_CALL_RETURN").ok().as_deref(),
            Some("1")
        ) {
            let name_bits = function_name_bits(_py, func_ptr);
            let name = if name_bits != 0 {
                string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<unnamed>".to_string())
            } else {
                "<unnamed>".to_string()
            };
            eprintln!(
                "[molt call_return0] name={} type={} bits=0x{:x}",
                name,
                type_name(_py, obj_from_bits(res)),
                res
            );
        }
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj8");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) =
            maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, &[arg0_bits, arg1_bits])
        {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64) -> u64,
                    extern "C" fn(u64, u64, u64) -> i64,
                    (closure_bits, arg0_bits, arg1_bits)
                )
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect2(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
                        arg0_bits,
                        arg1_bits,
                    ) as u64
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
                if let Some(runtime_target) = function_runtime_call_target_ptr(func_ptr, fn_ptr) {
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(runtime_target);
                    func(arg0_bits, arg1_bits)
                } else {
                    let call_target = function_call_target_or_legacy_ptr(func_ptr, fn_ptr);
                    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(call_target);
                    func(arg0_bits, arg1_bits) as u64
                }
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj2");
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj9");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
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
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64) -> i64,
                    (closure_bits, arg0_bits, arg1_bits, arg2_bits)
                )
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect3(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64) -> i64,
                    (arg0_bits, arg1_bits, arg2_bits)
                )
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj3");
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj10");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits, arg2_bits, arg3_bits],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64, u64) -> i64,
                    (closure_bits, arg0_bits, arg1_bits, arg2_bits, arg3_bits)
                )
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect4(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64) -> i64,
                    (arg0_bits, arg1_bits, arg2_bits, arg3_bits)
                )
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj4");
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj11");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64,
                    (
                        closure_bits,
                        arg0_bits,
                        arg1_bits,
                        arg2_bits,
                        arg3_bits,
                        arg4_bits
                    )
                )
            }
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if tramp_ptr != 0 {
                    molt_call_indirect5(
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                call_native_fixed_arity!(
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64, u64) -> i64,
                    (arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits)
                )
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj5");
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj12");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj6");
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj_trampoline");
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                arg7_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                arg7_bits, arg8_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                arg7_bits, arg8_bits, arg9_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                arg7_bits, arg8_bits, arg9_bits, arg10_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr),
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
        let _baseline_guard = ExceptionBaselineGuard::new();
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[
                arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                arg7_bits, arg8_bits, arg9_bits, arg10_bits, arg11_bits,
            ],
        ) {
            return res;
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
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
                        fixed_arity_call_target_ptr(fn_ptr, tramp_ptr),
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
                        fixed_arity_call_target_ptr(fn_ptr, tramp_ptr),
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

pub(crate) unsafe fn call_function_obj_trampoline(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe {
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let _baseline_guard = ExceptionBaselineGuard::new();
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        trace_function_vec_call(_py, func_ptr, args, "trampoline");
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
                if let Some(dbits) = defaults_bits
                    && !obj_from_bits(dbits).is_none()
                    && let Some(def_ptr) = obj_from_bits(dbits).as_ptr()
                    && object_type_id(def_ptr) == TYPE_ID_TUPLE
                {
                    let defaults = seq_vec_ref(def_ptr);
                    let n_defaults = defaults.len();
                    let missing = a - n;
                    if missing <= n_defaults {
                        let mut padded = Vec::with_capacity(a);
                        padded.extend_from_slice(args);
                        let start = n_defaults - missing;
                        padded.extend(defaults.iter().take(n_defaults).skip(start).copied());
                        // Recurse with the padded args — arity now matches.
                        return call_function_obj_trampoline(_py, func_bits, &padded);
                    }
                }
            }
            // Could not resolve the mismatch via __defaults__.
            // Return a clear arity mismatch error. The __defaults__ fast path
            // handles the common case; varargs/kwargs dispatch is handled by
            // the CallArgs path which is entered from a different call site.
            return raise_call_arity_mismatch(_py, func_ptr, a as u64, n as u64);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let tramp_ptr = crate::builtins::functions::normalize_runtime_trampoline_ptr(
            fn_ptr,
            function_trampoline_ptr(func_ptr),
        );
        if tramp_ptr == 0 {
            return raise_exception::<_>(_py, "TypeError", "call arity mismatch");
        }
        let closure_bits = function_closure_bits(func_ptr);
        let code_bits = ensure_function_code_bits(_py, func_ptr);
        if !recursion_guard_enter() {
            return raise_exception::<_>(_py, "RecursionError", "maximum recursion depth exceeded");
        }
        frame_stack_push(_py, code_bits);
        #[cfg(target_arch = "wasm32")]
        if matches!(
            std::env::var("MOLT_TRACE_CALL_FUNCTION_TRAMPOLINE")
                .ok()
                .as_deref(),
            Some("1")
        ) {
            let name_bits = function_name_bits(_py, func_ptr);
            let name = if name_bits != 0 {
                string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<unnamed>".to_string())
            } else {
                "<unnamed>".to_string()
            };
            eprintln!(
                "[molt call trampoline] name={name} fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} closure_bits={closure_bits} nargs={} task_trampoline={}",
                args.len(),
                function_needs_task_trampoline(_py, func_bits),
            );
        }
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
                let func: extern "C" fn(u64, u64, u64) -> i64 =
                    std::mem::transmute(tramp_ptr as usize);
                func(closure_bits, args.as_ptr() as u64, args.len() as u64) as u64
            }
        };
        #[cfg(target_arch = "wasm32")]
        inc_ref_bits(_py, res);
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
            trace_function_vec_call(_py, func_ptr, args, "vec");
            if let Some(res) = maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, args) {
                return res;
            }
            let arity = function_arity(func_ptr);
            if function_trampoline_ptr(func_ptr) != 0
                && (args.len() > 12 || arity != args.len() as u64)
            {
                return call_function_obj_trampoline(_py, func_bits, args);
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

#[cfg(test)]
mod tests {
    use super::{
        enforce_no_pending_on_success, fixed_arity_call_target_ptr,
        fixed_arity_trampoline_target_ptr,
        should_force_trampoline_for_fixed_arity_call,
    };
    use molt_obj_model::MoltObject;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            let _ = crate::lifecycle::init();
        });
        let _ = crate::molt_exception_clear();
    }

    fn int(v: i64) -> u64 {
        MoltObject::from_int(v).bits()
    }

    fn string_bits(text: &str) -> u64 {
        let mut out = 0u64;
        let rc =
            unsafe { crate::molt_string_from_bytes(text.as_ptr(), text.len() as u64, &mut out) };
        assert_eq!(rc, 0);
        out
    }

    struct EnvGuard(&'static str);

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var(self.0);
            }
        }
    }

    #[test]
    fn fixed_arity_call_target_uses_fn_ptr_without_trampoline() {
        assert_eq!(fixed_arity_call_target_ptr(293, 0), 293);
    }

    #[test]
    fn fixed_arity_trampoline_target_prefers_trampoline_slot() {
        assert_eq!(fixed_arity_trampoline_target_ptr(293, 4097), 4097);
    }

    #[test]
    fn fixed_arity_trampoline_target_falls_back_to_direct_slot() {
        assert_eq!(fixed_arity_trampoline_target_ptr(293, 0), 293);
    }

    #[test]
    fn fixed_arity_call_policy_uses_trampoline_when_present() {
        assert!(should_force_trampoline_for_fixed_arity_call(293, 4097, false));
    }

    #[test]
    fn fixed_arity_call_policy_uses_vector_path_for_raw_targets_with_trampoline() {
        assert!(should_force_trampoline_for_fixed_arity_call(
            u64::from(u32::MAX) + 1,
            4097,
            false,
        ));
    }

    #[test]
    fn fixed_arity_call_policy_keeps_task_trampolines_on_vector_path() {
        assert!(should_force_trampoline_for_fixed_arity_call(293, 4097, true));
    }

    fn spawn_child(test_name: &str, envs: &[(&str, &str)]) -> std::process::Output {
        let exe = std::env::current_exe().expect("current test executable");
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--exact").arg(test_name).arg("--nocapture");
        cmd.env("MOLT_ASSERT_CHILD", "1");
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.output().expect("spawn assert child")
    }

    #[test]
    fn assert_no_pending_on_success_traps_stale_exception() {
        if std::env::var("MOLT_ASSERT_CHILD").as_deref() == Ok("1") {
            return;
        }
        let output = spawn_child(
            "call::function::tests::assert_no_pending_on_success_child",
            &[("MOLT_ASSERT_NO_PENDING_ON_SUCCESS", "1")],
        );
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("pending exception on success path"));
    }

    #[test]
    fn assert_no_pending_on_success_child() {
        if std::env::var("MOLT_ASSERT_CHILD").as_deref() != Ok("1") {
            return;
        }
        init();
        unsafe {
            std::env::set_var("MOLT_ASSERT_NO_PENDING_ON_SUCCESS", "1");
        }
        let _guard = EnvGuard("MOLT_ASSERT_NO_PENDING_ON_SUCCESS");
        crate::with_gil_entry!(_py, {
            let kind_bits = string_bits("RuntimeError");
            let msg_bits = string_bits("stale pending");
            let args_list = crate::molt_list_builtin(crate::molt_missing());
            let _ = crate::molt_list_append(args_list, msg_bits);
            let args_bits = crate::molt_tuple_from_list(args_list);
            let exc_bits = crate::builtins::exceptions::molt_exception_new(kind_bits, args_bits);
            let _ = crate::molt_exception_set_last(exc_bits);
            let _ = unsafe { enforce_no_pending_on_success(_py, int(7), "call_function_obj0") };
        });
    }
}
