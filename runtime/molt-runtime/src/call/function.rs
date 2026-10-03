use crate::builtins::functions::runtime_callable_target_ptr;
use crate::call::ExceptionBaselineGuard;
use crate::object::layout::{
    CodeExecutionKind, EntryCustody, code_execution_kind, function_call_target_ptr,
    function_code_bits, function_entry_custody,
};
use crate::object::ops::string_obj_to_owned;
use crate::{
    CALL_DISPATCH_COUNT, HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE, PyToken, TYPE_ID_CODE,
    TYPE_ID_FUNCTION, exception_pending, function_arity, function_execution_closure_bits,
    function_fn_ptr, function_name_bits, function_trampoline_ptr, header_from_obj_ptr,
    molt_exception_clear, obj_from_bits, object_type_id, profile_hit, raise_exception, type_name,
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
pub(crate) fn fixed_arity_call_target_ptr(fn_ptr: u64, tramp_ptr: u64) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        let direct_target = wasm_direct_call_table_idx(fn_ptr);
        let normalized_tramp =
            crate::builtins::functions::normalize_runtime_trampoline_ptr(fn_ptr, tramp_ptr);
        // A `Direct`-dispatch reserved callable must resolve to its direct
        // table slot on the fixed-arity lane, no matter whether its stored
        // identity/target/trampoline pointer lands in the direct or trampoline
        // region.
        for candidate in [direct_target, fn_ptr, tramp_ptr] {
            if let Some(direct_slot) =
                crate::builtins::functions::reserved_wasm_runtime_direct_slot_for_any_reserved_slot(
                    candidate,
                )
            {
                return direct_slot;
            }
        }
        if crate::builtins::functions::reserved_wasm_runtime_callable_info_for_table_idx(
            direct_target,
        )
        .is_some()
            && crate::builtins::functions::reserved_wasm_runtime_direct_callable_info_for_table_idx(
                direct_target,
            )
            .is_none()
            && normalized_tramp != 0
        {
            return normalized_tramp;
        }
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
        let direct_target = wasm_direct_call_table_idx(fn_ptr);
        // A `Direct`-dispatch reserved callable must be reached through the
        // direct table region on the fixed-arity lane. Recognize the callable
        // whether the stored identity/target/trampoline pointer already sits in
        // the direct region (`fn_ptr`, `direct_target`) or the trampoline
        // region — either way the fixed-arity call site emits
        // `molt_call_indirectN` with N positional args, which the host only
        // interprets correctly against the direct slot.
        for candidate in [direct_target, fn_ptr, tramp_ptr] {
            if let Some(direct_slot) =
                crate::builtins::functions::reserved_wasm_runtime_direct_slot_for_any_reserved_slot(
                    candidate,
                )
            {
                return direct_slot;
            }
        }
        let normalized_tramp =
            crate::builtins::functions::normalize_runtime_trampoline_ptr(fn_ptr, tramp_ptr);
        if normalized_tramp != 0 {
            return normalized_tramp;
        }
        direct_target
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if tramp_ptr != 0 { tramp_ptr } else { fn_ptr }
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn fixed_arity_call_requires_trampoline(
    fn_ptr: u64,
    tramp_ptr: u64,
    task_trampoline_needed: bool,
) -> bool {
    let direct_target = wasm_direct_call_table_idx(fn_ptr);
    should_force_trampoline_for_fixed_arity_call(direct_target, tramp_ptr, task_trampoline_needed)
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
        let normalized_tramp =
            crate::builtins::functions::normalize_runtime_trampoline_ptr(fn_ptr, unsafe {
                function_trampoline_ptr(func_ptr)
            });
        if normalized_tramp != 0 {
            return normalized_tramp;
        }
        let direct_target = wasm_direct_call_table_idx(fn_ptr);
        if direct_target != 0 {
            return direct_target;
        }
        u64::MAX
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = fn_ptr;
        unsafe { function_trampoline_ptr(func_ptr) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) unsafe fn function_required_call_target_ptr(
    func_ptr: *mut u8,
    fn_ptr: u64,
) -> Option<*const ()> {
    let target = unsafe { function_call_target_ptr(func_ptr) };
    if !target.is_null() {
        return Some(target);
    }
    runtime_callable_target_ptr(fn_ptr)
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
unsafe fn function_runtime_call_target_ptr(func_ptr: *mut u8, fn_ptr: u64) -> Option<*const ()> {
    let runtime_target = runtime_callable_target_ptr(fn_ptr)?;
    let target = unsafe { function_call_target_ptr(func_ptr) };
    if !target.is_null() {
        return Some(target);
    }
    Some(runtime_target)
}

fn missing_function_call_target(_py: &PyToken<'_>, context: &str, fn_ptr: u64) -> u64 {
    let msg = format!("{context}: function call target 0x{fn_ptr:x} is not initialized");
    raise_exception::<_>(_py, "RuntimeError", &msg)
}

#[cfg(not(target_arch = "wasm32"))]
macro_rules! call_native_fixed_arity {
    ($py:expr, $func_ptr:expr, $fn_ptr:expr, $runtime_ty:ty, $compiled_ty:ty, ($($arg:expr),* $(,)?)) => {{
        if let Some(runtime_target) = function_runtime_call_target_ptr($func_ptr, $fn_ptr) {
            let func: $runtime_ty = std::mem::transmute(runtime_target);
            func($($arg),*) as u64
        } else if let Some(call_target) = function_required_call_target_ptr($func_ptr, $fn_ptr) {
            let func: $compiled_ty = std::mem::transmute(call_target);
            func($($arg),*) as u64
        } else {
            return missing_function_call_target($py, "fixed arity call", $fn_ptr);
        }
    }};
}

macro_rules! required_native_call_target {
    ($py:expr, $func_ptr:expr, $fn_ptr:expr, $context:expr) => {{
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(call_target) = function_required_call_target_ptr($func_ptr, $fn_ptr) else {
                return missing_function_call_target($py, $context, $fn_ptr);
            };
            call_target
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = $py;
            let _ = $func_ptr;
            let _ = $context;
            let Some(call_target) = crate::provenance::abi::function_ptr($fn_ptr) else {
                return missing_function_call_target($py, $context, $fn_ptr);
            };
            call_target
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
    if task_trampoline_needed || tramp_ptr == 0 {
        return task_trampoline_needed;
    }
    #[cfg(target_arch = "wasm32")]
    {
        // A `Direct`-dispatch reserved callable is dispatched on the fixed-arity
        // direct lane, never forced onto the trampoline lane — recognize it
        // whether `direct_target` sits in the direct or trampoline region.
        if crate::builtins::functions::reserved_wasm_runtime_direct_slot_for_any_reserved_slot(
            direct_target,
        )
        .is_some()
        {
            return false;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = direct_target;
    }
    true
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
    let closure_bits = unsafe { function_execution_closure_bits(func_ptr) };
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
        let reserved_info = crate::builtins::functions::reserved_wasm_runtime_callable_info(fn_ptr);
        let Ok(task_trampoline_needed) = function_needs_task_trampoline(_py, func_bits) else {
            return Some(crate::MoltObject::none().bits());
        };
        let force_trampoline =
            fixed_arity_call_requires_trampoline(fn_ptr, tramp_ptr, task_trampoline_needed);
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

unsafe fn call_function_obj_bound1(_py: &PyToken<'_>, func_bits: u64, arg0_bits: u64) -> u64 {
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
        if let Some(res) =
            maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, &[arg0_bits])
        {
            return res;
        }
        let arity = function_arity(func_ptr);
        if arity != 1 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 1);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
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
            let mut name = if name_bits != 0 {
                string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<unnamed>".to_string())
            } else {
                "<unnamed>".to_string()
            };
            let mut file = "<none>".to_string();
            if name == "<unnamed>" {
                let code_bits = crate::object::layout::ensure_function_code_bits(_py, func_ptr);
                if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() {
                    let code_name_bits = crate::code_name_bits(code_ptr);
                    name = string_obj_to_owned(obj_from_bits(code_name_bits))
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    let file_bits = crate::code_filename_bits(code_ptr);
                    file = string_obj_to_owned(obj_from_bits(file_bits))
                        .unwrap_or_else(|| "<none>".to_string());
                }
            }
            if name == "<unknown>" && file == "<molt-builtin>" {
                // Reverse fn_ptr -> name lookup for the debug trace line. Use the
                // per-app resolver (not the monolithic `resolve_symbol`) so this
                // native-reachable site does not keep `resolve_core_symbol` — and
                // with it every intrinsic address-of expression — alive against
                // dead-strip. On native the resolver only knows the app's manifest
                // intrinsics, which is sufficient for a best-effort debug name.
                if let Some(spec) = crate::intrinsics::INTRINSICS.iter().find(|spec| {
                    crate::intrinsics::try_app_resolve_symbol(spec.symbol) == Some(fn_ptr)
                }) {
                    name = spec.symbol.to_string();
                }
            }
            eprintln!(
                "[molt call_function_obj1] name={name} file={file} fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} closure_bits={closure_bits} arity={arity}"
            );
        }
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(closure_bits, arg0_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
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
                    ) as u64
                } else {
                    // SAFETY: all published Python callables, including None-returning
                    // intrinsics, use the boxed-result ABI. Arity == 1, no closure.
                    let func: extern "C" fn(u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(arg0_bits) as u64
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
                } else if let Some(call_target) =
                    function_required_call_target_ptr(func_ptr, fn_ptr)
                {
                    let func: extern "C" fn(u64) -> i64 = std::mem::transmute(call_target);
                    func(arg0_bits) as u64
                } else {
                    return missing_function_call_target(_py, "call_function_obj1", fn_ptr);
                }
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj1");
        res
    }
}

unsafe fn function_needs_task_trampoline(_py: &PyToken<'_>, func_bits: u64) -> Result<bool, ()> {
    unsafe {
        if exception_pending(_py) {
            return Err(());
        }
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return Ok(false);
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return Ok(false);
        }
        if let Some(kind) = function_code_execution_kind(func_ptr) {
            return Ok(kind.requires_task_trampoline());
        }
        Ok(false)
    }
}

#[inline]
pub(crate) unsafe fn function_code_execution_kind(func_ptr: *mut u8) -> Option<CodeExecutionKind> {
    unsafe {
        let code_ptr = obj_from_bits(function_code_bits(func_ptr)).as_ptr()?;
        (object_type_id(code_ptr) == TYPE_ID_CODE).then(|| code_execution_kind(code_ptr))
    }
}

#[inline]
pub(crate) unsafe fn function_has_variadic_trampoline(func_ptr: *mut u8) -> bool {
    unsafe {
        ((*header_from_obj_ptr(func_ptr)).load_metadata_flags()
            & HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE)
            != 0
    }
}

/// Binder metadata shared by mutation admission and callable-shape guards.
/// Dictionary writes can bypass attribute setters, so a mutation stamp alone
/// does not establish that these fields still have their published values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FunctionBindingField {
    Defaults,
    KeywordDefaults,
    ArgumentNames,
    PositionalOnly,
    KeywordOnlyNames,
    Varargs,
    VarKeywords,
    BindKind,
}

impl FunctionBindingField {
    pub(crate) const ALL: [Self; 8] = [
        Self::Defaults,
        Self::KeywordDefaults,
        Self::ArgumentNames,
        Self::PositionalOnly,
        Self::KeywordOnlyNames,
        Self::Varargs,
        Self::VarKeywords,
        Self::BindKind,
    ];

    pub(crate) const fn metadata_field(
        self,
    ) -> crate::object::function_metadata::FunctionMetadataField {
        use crate::object::function_metadata::FunctionMetadataField as Field;
        match self {
            Self::Defaults => Field::Defaults,
            Self::KeywordDefaults => Field::KeywordDefaults,
            Self::ArgumentNames => Field::ArgumentNames,
            Self::PositionalOnly => Field::PositionalOnly,
            Self::KeywordOnlyNames => Field::KeywordOnlyNames,
            Self::Varargs => Field::Varargs,
            Self::VarKeywords => Field::VarKeywords,
            Self::BindKind => Field::BindKind,
        }
    }

    pub(crate) const fn name(self) -> &'static [u8] {
        self.metadata_field().name().as_bytes()
    }

    pub(crate) fn from_name(name: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.name() == name)
    }
}

/// Defaults are admitted separately; other typed binder fields must be absent.
/// Public dictionary entries cannot alter the executable binding contract.
pub(crate) unsafe fn function_has_default_only_binding_metadata(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
) -> bool {
    unsafe {
        FunctionBindingField::ALL.into_iter().all(|field| {
            field == FunctionBindingField::Defaults
                || crate::object::function_metadata::metadata_bits(func_ptr, field.name()).is_none()
        })
    }
}

/// Read canonical typed metadata without allocating or consulting __dict__.
/// Published code signature facts override their pre-publication setup fields.
pub(crate) unsafe fn function_metadata_bits(
    _py: &PyToken<'_>,
    func_ptr: *mut u8,
    name: &[u8],
) -> u64 {
    unsafe {
        crate::object::function_metadata::metadata_bits(func_ptr, name)
            .unwrap_or_else(|| crate::MoltObject::none().bits())
    }
}

/// Commit derived facts before displaced metadata can run a finalizer.
pub(crate) unsafe fn commit_function_metadata_change(
    py: &PyToken<'_>,
    func_ptr: *mut u8,
    name: &[u8],
    user: bool,
) {
    unsafe {
        if user && FunctionBindingField::from_name(name).is_some() {
            crate::object::layout::bump_function_mutation_version(func_ptr);
        }
        crate::call::bind::refresh_function_requires_binder_flag(py, func_ptr);
    }
}

unsafe fn call_function_obj_bound0(_py: &PyToken<'_>, func_bits: u64) -> u64 {
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
        if let Some(res) = maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, &[]) {
            return res;
        }
        let arity = function_arity(func_ptr);
        if arity != 0 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 0);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    let func: extern "C" fn(u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(closure_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
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
                    molt_call_indirect0(fixed_arity_trampoline_target_ptr(fn_ptr, tramp_ptr)) as u64
                } else {
                    // SAFETY: `fn_ptr` is a valid extern "C" function pointer from
                    // `function_fn_ptr`. Arity == 0, no closure, so the nullary signature
                    // `fn() -> i64` is correct. The compiler must guarantee fn_ptr is non-null
                    // and targets a 0-arg extern "C" function. UB if fn_ptr is null or has a
                    // different calling convention or arity.
                    let func: extern "C" fn() -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
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
                } else if let Some(call_target) =
                    function_required_call_target_ptr(func_ptr, fn_ptr)
                {
                    let func: extern "C" fn() -> i64 = std::mem::transmute(call_target);
                    func() as u64
                } else {
                    return missing_function_call_target(_py, "call_function_obj0", fn_ptr);
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
        res
    }
}

unsafe fn call_function_obj_bound2(
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
        if let Some(res) =
            maybe_call_function_obj_trampoline(_py, func_bits, func_ptr, &[arg0_bits, arg1_bits])
        {
            return res;
        }
        let arity = function_arity(func_ptr);
        if arity != 2 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 2);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    let func: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(closure_bits, arg0_bits, arg1_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
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
                    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
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
                } else if let Some(call_target) =
                    function_required_call_target_ptr(func_ptr, fn_ptr)
                {
                    let func: extern "C" fn(u64, u64) -> i64 = std::mem::transmute(call_target);
                    func(arg0_bits, arg1_bits) as u64
                } else {
                    return missing_function_call_target(_py, "call_function_obj2", fn_ptr);
                }
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj2");
        res
    }
}

unsafe fn call_function_obj_bound3(
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits, arg2_bits],
        ) {
            return res;
        }
        let arity = function_arity(func_ptr);
        if arity != 3 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 3);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    let func: extern "C" fn(u64, u64, u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(closure_bits, arg0_bits, arg1_bits, arg2_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
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
                    let func: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(arg0_bits, arg1_bits, arg2_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64) -> i64,
                    (arg0_bits, arg1_bits, arg2_bits)
                )
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj3");
        res
    }
}

unsafe fn call_function_obj_bound4(
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits, arg2_bits, arg3_bits],
        ) {
            return res;
        }
        let arity = function_arity(func_ptr);
        if arity != 4 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 4);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(closure_bits, arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
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
                    let func: extern "C" fn(u64, u64, u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(arg0_bits, arg1_bits, arg2_bits, arg3_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64) -> i64,
                    (arg0_bits, arg1_bits, arg2_bits, arg3_bits)
                )
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj4");
        res
    }
}

unsafe fn call_function_obj_bound5(
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
        if let Some(res) = maybe_call_function_obj_trampoline(
            _py,
            func_bits,
            func_ptr,
            &[arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits],
        ) {
            return res;
        }
        let arity = function_arity(func_ptr);
        if arity != 5 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 5);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    _py,
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
                    let func: extern "C" fn(u64, u64, u64, u64, u64) -> i64 = std::mem::transmute(
                        required_native_call_target!(_py, func_ptr, fn_ptr, "fixed arity call"),
                    );
                    func(arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                call_native_fixed_arity!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    extern "C" fn(u64, u64, u64, u64, u64) -> i64,
                    extern "C" fn(u64, u64, u64, u64, u64) -> i64,
                    (arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits)
                )
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj5");
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound6(
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
        let arity = function_arity(func_ptr);
        if arity != 6 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 6);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "call_function_obj6"
                    ));
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
                    func(
                        arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                    ) as u64
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                // SAFETY: `fn_ptr` from `function_fn_ptr` targets a valid extern "C" function.
                // Arity verified above; signature matches. Compiler guarantees ABI match.
                let func: extern "C" fn(u64, u64, u64, u64, u64, u64) -> i64 = std::mem::transmute(
                    required_native_call_target!(_py, func_ptr, fn_ptr, "call_function_obj6"),
                );
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits,
                ) as u64
            }
        };
        let res = enforce_no_pending_on_success(_py, res, "call_function_obj6");
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound7(
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
        let arity = function_arity(func_ptr);
        if arity != 7 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 7);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                ) as u64
            }
        };
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound8(
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
        let arity = function_arity(func_ptr);
        if arity != 8 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 8);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits,
                ) as u64
            }
        };
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound9(
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
        let arity = function_arity(func_ptr);
        if arity != 9 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 9);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                        std::mem::transmute(required_native_call_target!(
                            _py,
                            func_ptr,
                            fn_ptr,
                            "fixed arity call"
                        ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits,
                ) as u64
            }
        };
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound10(
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
        let arity = function_arity(func_ptr);
        if arity != 10 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 10);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                ) -> i64 = std::mem::transmute(required_native_call_target!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    "fixed arity call"
                ));
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                    std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits, arg9_bits,
                ) as u64
            }
        };
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound11(
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
        let arity = function_arity(func_ptr);
        if arity != 11 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 11);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                ) -> i64 = std::mem::transmute(required_native_call_target!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    "fixed arity call"
                ));
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                ) -> i64 = std::mem::transmute(required_native_call_target!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    "fixed arity call"
                ));
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits, arg9_bits, arg10_bits,
                ) as u64
            }
        };
        res
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn call_function_obj_bound12(
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
        let arity = function_arity(func_ptr);
        if arity != 12 && !function_has_variadic_trampoline(func_ptr) {
            return raise_call_arity_mismatch(_py, func_ptr, arity, 12);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let closure_bits = function_execution_closure_bits(func_ptr);
        #[cfg(target_arch = "wasm32")]
        let tramp_ptr = normalized_function_trampoline_ptr(func_ptr, fn_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                ) -> i64 = std::mem::transmute(required_native_call_target!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    "fixed arity call"
                ));
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
                    ) -> i64 = std::mem::transmute(required_native_call_target!(
                        _py,
                        func_ptr,
                        fn_ptr,
                        "fixed arity call"
                    ));
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
                ) -> i64 = std::mem::transmute(required_native_call_target!(
                    _py,
                    func_ptr,
                    fn_ptr,
                    "fixed arity call"
                ));
                func(
                    arg0_bits, arg1_bits, arg2_bits, arg3_bits, arg4_bits, arg5_bits, arg6_bits,
                    arg7_bits, arg8_bits, arg9_bits, arg10_bits, arg11_bits,
                ) as u64
            }
        };
        res
    }
}

/// How a runtime invocation's argument references reach a compiled entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArgumentTransfer {
    /// The caller keeps its references: runtime helpers, callbacks, the C-API
    /// and every other runtime-originated call. An adopting entry receives
    /// references of its own.
    Borrowed,
    /// A call instruction's adopted references move into an adopting entry,
    /// or stay locally owned while a C extension borrows them.
    Moved,
}

/// Moved argument references that no entry has taken over yet. A failure
/// before entry, or a borrowing C callback's return, releases each exactly
/// once in frame order without replacing either pending error channel.
struct PendingMove<'a, 'py> {
    py: &'a PyToken<'py>,
    args: &'a [u64],
}

impl PendingMove<'_, '_> {
    /// The entry now owns every reference.
    fn into_entry(self) {
        std::mem::forget(self);
    }
}

impl Drop for PendingMove<'_, '_> {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            for &bits in self.args {
                crate::dec_ref_bits(self.py, bits);
            }
        });
    }
}

/// The runtime's borrowed lane into a compiled Python entry: every
/// runtime-originated invocation of a compiled function reaches it through
/// this trampoline call, and the caller keeps its references.
pub(crate) unsafe fn call_function_obj_trampoline(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe { invoke_function_trampoline(_py, func_bits, args, ArgumentTransfer::Borrowed) }
}

/// Consume a call instruction's adopted argument references. `args` are the
/// entry's already-bound Python arguments. An adopting entry takes ownership;
/// a C extension borrows them until this transport releases them. The caller
/// never releases them, including when the call fails before entry.
pub(crate) unsafe fn call_function_obj_moved(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> u64 {
    unsafe { invoke_function_trampoline(_py, func_bits, args, ArgumentTransfer::Moved) }
}

/// Whether `func_bits` is a function whose direct entry adopts its Python
/// arguments, so that a caller holding owned references may move them in.
pub(crate) unsafe fn function_bits_adopt_arguments(func_bits: u64) -> bool {
    obj_from_bits(func_bits).as_ptr().is_some_and(|ptr| unsafe {
        object_type_id(ptr) == TYPE_ID_FUNCTION
            && function_entry_custody(ptr) == EntryCustody::Adopting
    })
}

unsafe fn invoke_function_trampoline(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
    transfer: ArgumentTransfer,
) -> u64 {
    unsafe {
        // Constructed only on the moved lane: dropping it releases the moves.
        let pending = (transfer == ArgumentTransfer::Moved).then(|| PendingMove { py: _py, args });
        profile_hit(_py, &CALL_DISPATCH_COUNT);
        let _baseline_guard = ExceptionBaselineGuard::new();
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        };
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
            return raise_exception::<_>(_py, "TypeError", "call expects function object");
        }
        // Published C wrappers use an admitted trampoline. External positional
        // calls must acquire C execution custody before reaching that entry.
        // The shared C dispatcher owns its one recursion/frame activation;
        // intercept before constructing either guard or transferring arguments.
        if !crate::concurrency::execution::current_thread_has_c_extension_execution_context()
            && let Some(result) =
                crate::cpython_abi_hooks::try_call_cext(_py, func_ptr, args, &[], &[])
        {
            return result;
        }
        let adopting = function_entry_custody(func_ptr) == EntryCustody::Adopting;
        // C arguments are borrowed regardless of how they reached this call.
        // Recognize only moved borrowing entries here: already-admitted,
        // borrowed calls keep the generated native/WASM trampoline hot path.
        let moved_cext =
            pending.is_some() && !adopting && crate::cpython_abi_hooks::is_cext_callable(func_ptr);
        if pending.is_some() && !adopting && !moved_cext {
            return raise_exception::<_>(
                _py,
                "SystemError",
                "moved call arguments require an adopting entry",
            );
        }
        trace_function_vec_call(_py, func_ptr, args, "trampoline");
        let arity = function_arity(func_ptr);
        if arity != args.len() as u64 && !function_has_variadic_trampoline(func_ptr) {
            // C conventions own their arity diagnostics. Recognize them only
            // on this cold mismatch path; valid admitted borrowed calls keep
            // the generated transport without an executable-identity lookup.
            if let Some(result) =
                crate::cpython_abi_hooks::try_call_cext(_py, func_ptr, args, &[], &[])
            {
                return result;
            }
            // Both borrowed and moved transport contain already-bound ABI
            // slots. Defaults and keyword binding belong only to the binder.
            return raise_call_arity_mismatch(_py, func_ptr, arity, args.len() as u64);
        }
        let fn_ptr = function_fn_ptr(func_ptr);
        let tramp_ptr = crate::builtins::functions::normalize_runtime_trampoline_ptr(
            fn_ptr,
            function_trampoline_ptr(func_ptr),
        );
        if tramp_ptr == 0 {
            return raise_exception::<_>(_py, "TypeError", "call arity mismatch");
        }
        let closure_bits = function_execution_closure_bits(func_ptr);
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(_py) else {
            return crate::MoltObject::none().bits();
        };
        let Some(_invocation) =
            crate::builtins::frames::FrameInvocationGuard::for_function(_py, func_ptr)
        else {
            return crate::MoltObject::none().bits();
        };
        // Nothing fails between here and the entry. An adopting entry owns one
        // reference to each Python argument: moved ones are the instruction's,
        // borrowed ones are retained here, the borrowed lane's only retain.
        let _borrowed_moves = match pending {
            Some(pending) if adopting => {
                pending.into_entry();
                None
            }
            None if adopting => {
                for &bits in args {
                    crate::inc_ref_bits(_py, bits);
                }
                None
            }
            // A C callback borrows moved operands. Keep their existing owner
            // alive through the callback and preserve its error on release.
            pending => pending,
        };
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
                "[molt call trampoline] name={name} fn_ptr={fn_ptr} tramp_ptr={tramp_ptr} closure_bits={closure_bits} nargs={} execution_kind={:?}",
                args.len(),
                function_code_execution_kind(func_ptr),
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
                // This lane invokes the *variadic trampoline* ABI
                // `fn(closure_bits, argv_ptr, argc)`, which only the compiled
                // `..__molt_trampoline_*` entry implements. It MUST resolve
                // `tramp_ptr` (the trampoline's own address), NOT the function's
                // fixed-arity `call_target` slot.
                //
                // `function_required_call_target_ptr` returns the offset-8
                // `call_target` slot whenever it is populated, ignoring the
                // `tramp_ptr` argument. That slot holds the callee's FIXED-ARITY
                // entry (`fn(arg0, arg1, ...)`) — cached there for direct fixed-
                // arity dispatch by `init_runtime_callable_function_obj`'s
                // `native_direct_target` path once the app-callable resolver
                // publishes it. Feeding that fixed-arity entry the variadic ABI
                // reinterprets `closure_bits` (0 for a plain function) as the
                // first param and the raw `argv` pointer as the second, so every
                // argument is silently replaced by junk NaN-box bits — e.g.
                // `f(1, d=4)` returns `(0.0, <argv-ptr-as-f64>)` instead of
                // `(1, 4)`. `tramp_ptr` is the trampoline's executable address on
                // native (`normalize_runtime_trampoline_ptr` is identity here);
                // resolve it through the runtime-callable registry for
                // canonicalization and fall back to the raw address.
                let call_target = if let Some(target) = runtime_callable_target_ptr(tramp_ptr) {
                    target
                } else {
                    let Some(target) = crate::provenance::abi::function_ptr(tramp_ptr) else {
                        return missing_function_call_target(
                            _py,
                            "variadic trampoline call",
                            tramp_ptr,
                        );
                    };
                    target
                };
                let func: extern "C" fn(u64, u64, u64) -> i64 = std::mem::transmute(call_target);
                func(closure_bits, args.as_ptr() as u64, args.len() as u64) as u64
            }
        };
        // The trampoline is the raw function-call boundary on every target:
        // fresh results and argument aliases both arrive with the callee's one
        // owned result reference. A WASM-only retain here duplicated every
        // fresh heap result before CallArgs teardown and made trampoline
        // dispatch observably leak relative to fixed-arity and native calls.
        res
    }
}

/// Runtime-originated fixed-arity calls contain Python arguments. They use
/// the same receiver admission and signature binding as vector calls; matching
/// the machine arity does not prove that an argument vector is already bound.
macro_rules! raw_function_call {
    ($name:ident $(, $arg:ident)*) => {
        #[inline]
        pub(crate) unsafe fn $name(py: &PyToken<'_>, function: u64, $($arg: u64),*) -> u64 {
            unsafe { call_function_obj_vec(py, function, &[$($arg),*]) }
        }
    };
}

#[cfg(test)]
raw_function_call!(call_function_obj0);
raw_function_call!(call_function_obj1, arg0);
raw_function_call!(call_function_obj2, arg0, arg1);
raw_function_call!(call_function_obj3, arg0, arg1, arg2);
#[cfg(test)]
raw_function_call!(call_function_obj4, arg0, arg1, arg2, arg3);

pub(crate) unsafe fn call_function_obj_vec(_py: &PyToken<'_>, func_bits: u64, args: &[u64]) -> u64 {
    unsafe {
        if !crate::builtins::functions::native_callable::admit_native_callable(_py, func_bits, args)
        {
            return crate::MoltObject::none().bits();
        }

        let func_obj = obj_from_bits(func_bits);
        if let Some(func_ptr) = func_obj.as_ptr()
            && object_type_id(func_ptr) == TYPE_ID_FUNCTION
            && crate::call::bind::function_raw_positional_call_needs_binding(
                _py,
                func_ptr,
                args.len(),
            )
        {
            return crate::call::bind::call_bind_borrowed(_py, func_bits, None, args, &[], &[]);
        }
        call_function_obj_bound_vec(_py, func_bits, args)
    }
}

/// Execute the binder's ABI slots without interpreting them as Python arguments.
/// Receiver validation and binding must have happened before this boundary.
/// Fixed arity, packed native `(args, kwargs)`, and compiled frame slots share
/// this transport, but never use slot count to infer admission.
pub(crate) unsafe fn call_function_obj_bound_vec(
    _py: &PyToken<'_>,
    func_bits: u64,
    args: &[u64],
) -> u64 {
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
        let Ok(task_trampoline_needed) = function_needs_task_trampoline(_py, func_bits) else {
            return crate::MoltObject::none().bits();
        };
        if task_trampoline_needed {
            return call_function_obj_trampoline(_py, func_bits, args);
        }
        if let Some(func_ptr) = func_obj.as_ptr()
            && object_type_id(func_ptr) == TYPE_ID_FUNCTION
            && (13..=16).contains(&args.len())
            && function_arity(func_ptr) == args.len() as u64
            && function_execution_closure_bits(func_ptr) == 0
            && function_trampoline_ptr(func_ptr) == 0
            && function_entry_custody(func_ptr) == EntryCustody::Borrowing
        {
            // Retain native direct ABI widths through 16 (WASM through 13)
            // for runtime-created
            // functions without a compiled trampoline. Admission and
            // packing are already complete, including native descriptors.
            return crate::object::ops_builtins::molt_call_func_direct(
                _py,
                function_fn_ptr(func_ptr),
                args,
                0,
                func_bits,
            );
        }
        match args.len() {
            0 => call_function_obj_bound0(_py, func_bits),
            1 => call_function_obj_bound1(_py, func_bits, args[0]),
            2 => call_function_obj_bound2(_py, func_bits, args[0], args[1]),
            3 => call_function_obj_bound3(_py, func_bits, args[0], args[1], args[2]),
            4 => call_function_obj_bound4(_py, func_bits, args[0], args[1], args[2], args[3]),
            5 => call_function_obj_bound5(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4],
            ),
            6 => call_function_obj_bound6(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5],
            ),
            7 => call_function_obj_bound7(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
            ),
            8 => call_function_obj_bound8(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7],
            ),
            9 => call_function_obj_bound9(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8],
            ),
            10 => call_function_obj_bound10(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8], args[9],
            ),
            11 => call_function_obj_bound11(
                _py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                args[7], args[8], args[9], args[10],
            ),
            12 => call_function_obj_bound12(
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
        fixed_arity_trampoline_target_ptr, should_force_trampoline_for_fixed_arity_call,
    };
    use crate::object::builders::{alloc_dict_with_pairs, alloc_list, alloc_tuple};
    use crate::{dec_ref_bits, header_from_obj_ptr, obj_from_bits};
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

    extern "C" fn identity_returns_owned_arg(arg_bits: u64) -> i64 {
        crate::molt_inc_ref_obj(arg_bits);
        arg_bits as i64
    }

    extern "C" fn return_varargs_tuple(_self_bits: u64, args_bits: u64, _kwargs_bits: u64) -> i64 {
        crate::molt_inc_ref_obj(args_bits);
        args_bits as i64
    }

    fn intern_metadata_name(_py: &crate::PyToken<'_>, name: &'static [u8]) -> u64 {
        let interned = &crate::runtime_state(_py).interned;
        let slot = match name {
            b"__molt_arg_names__" => &interned.molt_arg_names,
            b"__molt_posonly__" => &interned.molt_posonly,
            b"__molt_kwonly_names__" => &interned.molt_kwonly_names,
            b"__molt_vararg__" => &interned.molt_vararg,
            b"__molt_varkw__" => &interned.molt_varkw,
            b"__molt_bind_kind__" => &interned.molt_bind_kind,
            b"__defaults__" => &interned.defaults_name,
            b"__kwdefaults__" => &interned.kwdefaults_name,
            other => panic!("unknown metadata name {:?}", other),
        };
        crate::intern_static_name(_py, slot, name)
    }

    unsafe fn set_function_metadata_attr(
        _py: &crate::PyToken<'_>,
        func_ptr: *mut u8,
        name: &'static [u8],
        value_bits: u64,
    ) {
        let attr_bits = intern_metadata_name(_py, name);
        unsafe {
            assert!(crate::call::class_init::function_set_attr_bits(
                _py, func_ptr, attr_bits, value_bits
            ))
        };
    }

    fn ref_count(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn shared_code_execution_kind_ignores_public_marker_mutation() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name_bits = string_bits("code-task-kind");
            let empty_tuple_ptr = alloc_tuple(_py, &[]);
            assert!(!empty_tuple_ptr.is_null());
            let empty_tuple_bits = MoltObject::from_ptr(empty_tuple_ptr).bits();
            let code_ptr = crate::alloc_code_obj(
                _py,
                name_bits,
                name_bits,
                1,
                MoltObject::none().bits(),
                empty_tuple_bits,
                empty_tuple_bits,
                0,
                0,
                0,
            );
            assert!(!code_ptr.is_null());
            let code_bits = MoltObject::from_ptr(code_ptr).bits();
            let first_func = crate::alloc_function_obj(_py, 17, 0);
            let second_func = crate::alloc_function_obj(_py, 17, 0);
            assert!(!first_func.is_null() && !second_func.is_null());
            let first_bits = MoltObject::from_ptr(first_func).bits();
            let second_bits = MoltObject::from_ptr(second_func).bits();
            unsafe {
                assert!(crate::function_set_code_bits(_py, first_func, code_bits));
                assert!(crate::function_set_code_bits(_py, second_func, code_bits));
                assert_eq!(
                    crate::object::layout::code_publish_execution_kind(
                        code_ptr,
                        crate::object::layout::CodeExecutionKind::Generator,
                    ),
                    Ok(())
                );
                assert_eq!(
                    crate::object::layout::code_publish_execution_kind(
                        code_ptr,
                        crate::object::layout::CodeExecutionKind::Coroutine,
                    ),
                    Err(crate::object::layout::CodeExecutionKind::Generator)
                );

                let marker = b"__molt_is_generator__";
                let set_result = crate::molt_set_attr_object(
                    first_bits,
                    marker.as_ptr(),
                    marker.len() as u64,
                    MoltObject::from_bool(true).bits(),
                );
                assert_eq!(set_result, MoltObject::none().bits());
                assert!(!crate::exception_pending(_py));
                assert_eq!(
                    crate::object::layout::code_execution_kind(code_ptr),
                    crate::object::layout::CodeExecutionKind::Generator,
                );
                assert_eq!(
                    super::function_needs_task_trampoline(_py, second_bits),
                    Ok(true)
                );

                let del_result =
                    crate::molt_del_attr_object(first_bits, marker.as_ptr(), marker.len() as u64);
                assert_eq!(del_result, MoltObject::none().bits());
                assert!(!crate::exception_pending(_py));
                assert_eq!(
                    crate::object::layout::code_execution_kind(code_ptr),
                    crate::object::layout::CodeExecutionKind::Generator,
                );
                assert_eq!(
                    super::function_needs_task_trampoline(_py, second_bits),
                    Ok(true)
                );
            }
            dec_ref_bits(_py, first_bits);
            dec_ref_bits(_py, second_bits);
            dec_ref_bits(_py, code_bits);
            dec_ref_bits(_py, empty_tuple_bits);
            dec_ref_bits(_py, name_bits);
        });
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
        assert!(should_force_trampoline_for_fixed_arity_call(
            293, 4097, false
        ));
    }

    #[test]
    fn public_vec_call_preserves_callee_owned_arg_alias_return() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                identity_returns_owned_arg as *const () as usize as u64,
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let list_ptr = alloc_list(_py, &[int(11), int(13)]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let result = unsafe { super::call_function_obj_vec(_py, func_bits, &[list_bits]) };
            assert_eq!(result, list_bits);
            assert_eq!(
                ref_count(result),
                2,
                "public direct function calls return an owned alias"
            );

            dec_ref_bits(_py, result);
            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn fixed_arity_entry_routes_varargs_functions_through_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                return_varargs_tuple as *const () as usize as u64,
                3,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let self_name = string_bits("self");
            let arg_names_ptr = alloc_tuple(_py, &[self_name]);
            assert!(!arg_names_ptr.is_null());
            let arg_names_bits = MoltObject::from_ptr(arg_names_ptr).bits();
            let empty_tuple_ptr = alloc_tuple(_py, &[]);
            assert!(!empty_tuple_ptr.is_null());
            let empty_tuple_bits = MoltObject::from_ptr(empty_tuple_ptr).bits();
            let vararg_bits = string_bits("args");
            let varkw_bits = string_bits("kwargs");
            let none_bits = MoltObject::none().bits();

            unsafe {
                set_function_metadata_attr(_py, func_ptr, b"__molt_arg_names__", arg_names_bits);
                set_function_metadata_attr(
                    _py,
                    func_ptr,
                    b"__molt_posonly__",
                    MoltObject::from_int(0).bits(),
                );
                set_function_metadata_attr(
                    _py,
                    func_ptr,
                    b"__molt_kwonly_names__",
                    empty_tuple_bits,
                );
                set_function_metadata_attr(_py, func_ptr, b"__molt_vararg__", vararg_bits);
                set_function_metadata_attr(_py, func_ptr, b"__molt_varkw__", varkw_bits);
                set_function_metadata_attr(_py, func_ptr, b"__defaults__", none_bits);
                set_function_metadata_attr(_py, func_ptr, b"__kwdefaults__", none_bits);
                crate::call::bind::refresh_function_requires_binder_flag(_py, func_ptr);
            }

            let result_bits = unsafe { super::call_function_obj1(_py, func_bits, int(7)) };
            assert!(
                !crate::exception_pending(_py),
                "fixed-arity helper must not raise before binder dispatch"
            );
            let result_ptr = obj_from_bits(result_bits).as_ptr().expect("tuple result");
            assert_eq!(
                unsafe { crate::object_type_id(result_ptr) },
                crate::TYPE_ID_TUPLE
            );
            assert!(
                unsafe { crate::object::seq_access::len(result_ptr) } == 0,
                "binder must pack no extra positional arguments into an empty *args tuple"
            );

            dec_ref_bits(_py, result_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    // Correct trampoline behavior: read argv[0] from the packed args array.
    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn trampoline_returns_first_argv(_closure: u64, argv_ptr: u64, _argc: u64) -> i64 {
        unsafe { *(argv_ptr as *const u64) as i64 }
    }

    // Fixed-arity entry: returns its first positional param. If the trampoline
    // lane wrongly invokes THIS with the variadic ABI, `first` is `closure_bits`
    // (0 for a plain function), not the real argument.
    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn fixed_arity_returns_first_param(first: u64, _second: u64) -> i64 {
        first as i64
    }

    // Regression guard: the variadic trampoline lane must dispatch to the
    // trampoline entry `fn(closure, argv_ptr, argc)`, NEVER the function's
    // fixed-arity `call_target` slot. When the app-callable resolver caches the
    // fixed-arity entry in that slot, invoking it with the trampoline ABI
    // reinterprets `closure_bits`/`argv_ptr` as the callee's first two params --
    // the silent keyword-call miscompile (`f(1, d=4)` -> `(0.0, junk)`).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn trampoline_lane_dispatches_trampoline_not_fixed_arity_call_target() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init();
        crate::with_gil_entry_nopanic!(_py, {
            // `alloc_runtime_function_obj` registers the fixed-arity fn_ptr and
            // caches it in the offset-8 `call_target` slot -- exactly the state
            // the app-callable resolver produces on main.
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                fixed_arity_returns_first_param as *const () as usize as u64,
                2,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            // Publish a distinct variadic trampoline entry in the trampoline slot.
            unsafe {
                crate::object::layout::function_set_trampoline_ptr(
                    func_ptr,
                    trampoline_returns_first_argv as *const () as usize as u64,
                );
            }
            assert!(
                !unsafe { crate::object::layout::function_call_target_ptr(func_ptr) }.is_null(),
                "precondition: fixed-arity call_target slot must be populated to reproduce the poison",
            );

            let result =
                unsafe { super::call_function_obj_trampoline(_py, func_bits, &[int(11), int(13)]) };
            assert!(!crate::exception_pending(_py));
            assert_eq!(
                result,
                int(11),
                "trampoline lane must invoke the trampoline entry (returns argv[0]=11), not the \
                 fixed-arity call_target (which would return closure_bits=0)",
            );

            dec_ref_bits(_py, func_bits);
        });
    }

    // An adopting entry owns one reference to its argument: it records the
    // count it observes, then releases its own reference as its frame would.
    #[cfg(not(target_arch = "wasm32"))]
    static ADOPTED_ARGUMENT_REFS: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0);

    #[cfg(not(target_arch = "wasm32"))]
    extern "C" fn adopting_trampoline(_closure: u64, argv_ptr: u64, _argc: u64) -> i64 {
        let arg = unsafe { *(argv_ptr as *const u64) };
        ADOPTED_ARGUMENT_REFS.store(ref_count(arg), std::sync::atomic::Ordering::SeqCst);
        crate::molt_dec_ref_obj(arg);
        MoltObject::none().bits() as i64
    }

    // The borrowed lane retains for an adopting entry, the moved lane hands the
    // instruction's reference over, and a moved call that never reaches its
    // entry releases each moved reference exactly once. Treating the entry as
    // borrowing would under-release on the borrowed lane; releasing after a
    // moved call, or forgetting the failure, would unbalance the moved lane.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn adopting_entries_own_one_reference_on_the_borrowed_and_moved_lanes() {
        use std::sync::atomic::Ordering::SeqCst;
        init();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                fixed_arity_returns_first_param as *const () as usize as u64,
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            unsafe {
                crate::object::layout::function_set_trampoline_ptr(
                    func_ptr,
                    adopting_trampoline as *const () as usize as u64,
                );
                assert_eq!(
                    crate::object::layout::function_publish_entry_custody(
                        func_ptr,
                        crate::object::layout::EntryCustody::Adopting,
                    ),
                    Ok(())
                );
            }
            let list_ptr = alloc_list(_py, &[int(1)]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let baseline = ref_count(list_bits);

            let result =
                unsafe { super::call_function_obj_trampoline(_py, func_bits, &[list_bits]) };
            assert!(obj_from_bits(result).is_none());
            assert_eq!(ADOPTED_ARGUMENT_REFS.load(SeqCst), baseline + 1);
            assert_eq!(ref_count(list_bits), baseline);

            crate::molt_inc_ref_obj(list_bits);
            let result = unsafe { super::call_function_obj_moved(_py, func_bits, &[list_bits]) };
            assert!(obj_from_bits(result).is_none());
            assert_eq!(ADOPTED_ARGUMENT_REFS.load(SeqCst), baseline + 1);
            assert_eq!(ref_count(list_bits), baseline);

            crate::molt_inc_ref_obj(list_bits);
            crate::molt_inc_ref_obj(list_bits);
            let result =
                unsafe { super::call_function_obj_moved(_py, func_bits, &[list_bits, list_bits]) };
            assert!(obj_from_bits(result).is_none());
            assert!(crate::exception_pending(_py));
            let _ = crate::molt_exception_clear();
            assert_eq!(ref_count(list_bits), baseline);

            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    #[derive(Default)]
    struct CextDispatchObservation {
        calls: usize,
        recursion_depth: usize,
        frame_depth: usize,
        execution_context: bool,
        positional: usize,
        keywords: usize,
        fail: bool,
        raised: u64,
    }

    thread_local! {
        static CEXT_DISPATCH: std::cell::RefCell<CextDispatchObservation> =
            std::cell::RefCell::new(CextDispatchObservation::default());
    }

    unsafe extern "C" fn cext_dispatch_probe(
        _receiver: *mut molt_cpython_abi::abi_types::PyObject,
        args: *mut *mut molt_cpython_abi::abi_types::PyObject,
        count: molt_cpython_abi::abi_types::Py_ssize_t,
        names: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject {
        use molt_cpython_abi::api::{numbers, refcount, sequences};
        let fail = CEXT_DISPATCH.with(|state| {
            let mut state = state.borrow_mut();
            state.calls += 1;
            state.recursion_depth = crate::state::recursion::recursion_depth();
            state.frame_depth = crate::FRAME_STACK.with(|stack| stack.borrow().len());
            state.execution_context =
                crate::concurrency::execution::current_thread_has_c_extension_execution_context();
            state.positional = count as usize;
            state.keywords = if names.is_null() {
                0
            } else {
                unsafe { sequences::PyTuple_Size(names) as usize }
            };
            state.fail
        });
        if fail {
            crate::with_gil(|py| {
                crate::raise_exception::<u64>(&py, "ValueError", "C dispatch failure");
                CEXT_DISPATCH.with(|state| {
                    state.borrow_mut().raised = crate::molt_exception_last_pending();
                });
            });
            return std::ptr::null_mut();
        }
        unsafe {
            if count == 0 {
                numbers::PyLong_FromLongLong(197)
            } else {
                // A real argument alias exercises both C and runtime result custody.
                let result = *args;
                refcount::Py_INCREF(result);
                result
            }
        }
    }

    unsafe fn cext_dispatch_function_for(target: *const (), flags: std::os::raw::c_int) -> u64 {
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        let name = b"dispatch_probe";
        unsafe {
            (molt_cpython_abi::hooks::hooks_or_stubs().register_c_function)(
                crate::provenance::abi::expose_function_address(target),
                flags,
                MoltObject::none().bits(),
                false,
                MoltObject::none().bits(),
                name.as_ptr(),
                name.len(),
            )
        }
    }

    unsafe fn cext_dispatch_function() -> u64 {
        use molt_cpython_abi::abi_types::{METH_FASTCALL, METH_KEYWORDS};
        unsafe {
            cext_dispatch_function_for(
                cext_dispatch_probe as *const (),
                METH_FASTCALL | METH_KEYWORDS,
            )
        }
    }

    unsafe extern "C" fn cext_dispatch_noargs(
        receiver: *mut molt_cpython_abi::abi_types::PyObject,
        _args: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject {
        unsafe { cext_dispatch_probe(receiver, std::ptr::null_mut(), 0, std::ptr::null_mut()) }
    }

    unsafe extern "C" fn cext_dispatch_one(
        receiver: *mut molt_cpython_abi::abi_types::PyObject,
        mut arg: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject {
        unsafe { cext_dispatch_probe(receiver, &raw mut arg, 1, std::ptr::null_mut()) }
    }

    fn assert_cext_dispatch(recursion: usize, frames: usize, positional: usize, keywords: usize) {
        CEXT_DISPATCH.with(|state| {
            let state = state.borrow();
            assert_eq!(state.calls, 1);
            assert_eq!(state.recursion_depth, recursion + 1);
            // C wrappers have no compiled Python frame slot.
            assert_eq!(state.frame_depth, frames);
            assert!(state.execution_context);
            assert_eq!(state.positional, positional);
            assert_eq!(state.keywords, keywords);
        });
        assert_eq!(crate::state::recursion::recursion_depth(), recursion);
        assert_eq!(
            crate::FRAME_STACK.with(|stack| stack.borrow().len()),
            frames
        );
    }

    unsafe fn cext_bound_call(py: &crate::PyToken<'_>, func_bits: u64, args: &[u64]) -> u64 {
        unsafe {
            match args.len() {
                0 => super::call_function_obj_bound0(py, func_bits),
                1 => super::call_function_obj_bound1(py, func_bits, args[0]),
                2 => super::call_function_obj_bound2(py, func_bits, args[0], args[1]),
                3 => super::call_function_obj_bound3(py, func_bits, args[0], args[1], args[2]),
                4 => super::call_function_obj_bound4(
                    py, func_bits, args[0], args[1], args[2], args[3],
                ),
                5 => super::call_function_obj_bound5(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4],
                ),
                6 => super::call_function_obj_bound6(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5],
                ),
                7 => super::call_function_obj_bound7(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                ),
                8 => super::call_function_obj_bound8(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                    args[7],
                ),
                9 => super::call_function_obj_bound9(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                    args[7], args[8],
                ),
                10 => super::call_function_obj_bound10(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                    args[7], args[8], args[9],
                ),
                11 => super::call_function_obj_bound11(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                    args[7], args[8], args[9], args[10],
                ),
                12 => super::call_function_obj_bound12(
                    py, func_bits, args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                    args[7], args[8], args[9], args[10], args[11],
                ),
                _ => super::call_function_obj_trampoline(py, func_bits, args),
            }
        }
    }

    #[test]
    fn cext_positional_family_and_keywords_own_one_invocation() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil(|py| unsafe {
            use crate::concurrency::execution::{
                RuntimeExecutionGuard, current_thread_has_c_extension_execution_context,
            };
            let function = cext_dispatch_function();
            assert_ne!(function, 0);
            let list = alloc_list(&py, &[]);
            assert!(!list.is_null());
            let value = MoltObject::from_ptr(list).bits();
            // The first borrowed C view owns one stable runtime hold.
            // Measure per-call balance only after that canonical publication.
            let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(value);
            assert!(!view.is_null());
            let baseline = ref_count(value);
            let recursion = crate::state::recursion::recursion_depth();
            let frames = crate::FRAME_STACK.with(|stack| stack.borrow().len());
            assert!(!current_thread_has_c_extension_execution_context());
            for admitted in [false, true] {
                let _execution = admitted.then(RuntimeExecutionGuard::enter);
                for width in 0..=13 {
                    let args = vec![value; width];
                    for lane in 0..4 {
                        CEXT_DISPATCH.with(|state| *state.borrow_mut() = Default::default());
                        let result = match lane {
                            0 => cext_bound_call(&py, function, &args),
                            1 => super::call_function_obj_bound_vec(&py, function, &args),
                            2 => super::call_function_obj_vec(&py, function, &args),
                            _ => super::call_function_obj_trampoline(&py, function, &args),
                        };
                        assert!(!crate::exception_pending(&py));
                        assert_eq!(result, if width == 0 { int(197) } else { value });
                        assert_cext_dispatch(recursion, frames, width, 0);
                        assert_eq!(current_thread_has_c_extension_execution_context(), admitted);
                        dec_ref_bits(&py, result);
                        assert_eq!(ref_count(value), baseline);
                    }
                }
                // Keyword binding must use the same dispatcher without an outer activation.
                CEXT_DISPATCH.with(|state| *state.borrow_mut() = Default::default());
                let builder = crate::molt_callargs_new(1, 1);
                assert_ne!(builder, 0);
                let key = string_bits("named");
                crate::molt_callargs_push_pos(builder, value);
                crate::molt_callargs_push_kw(builder, key, value);
                let result = crate::molt_call_bind(function, builder);
                assert_eq!(result, value);
                assert!(!crate::exception_pending(&py));
                assert_cext_dispatch(recursion, frames, 1, 1);
                assert_eq!(current_thread_has_c_extension_execution_context(), admitted);
                dec_ref_bits(&py, result);
                dec_ref_bits(&py, key);
                assert_eq!(ref_count(value), baseline);
            }
            assert!(!current_thread_has_c_extension_execution_context());
            dec_ref_bits(&py, value);
            dec_ref_bits(&py, function);
        });
    }

    #[test]
    fn cext_fixed_bound_arms_preserve_c_convention_arity_errors() {
        use crate::concurrency::execution::{
            RuntimeExecutionGuard, current_thread_has_c_extension_execution_context,
        };
        use molt_cpython_abi::abi_types::{METH_NOARGS, METH_O, PyExc_TypeError};
        use molt_cpython_abi::api::{errors, refcount, strings, typeobj};

        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil(|py| unsafe {
            for (flags, target, width, expected) in [
                (
                    METH_NOARGS,
                    cext_dispatch_noargs as *const (),
                    1,
                    "dispatch_probe() takes no arguments (1 given)",
                ),
                (
                    METH_O,
                    cext_dispatch_one as *const (),
                    0,
                    "dispatch_probe() takes exactly one argument (0 given)",
                ),
                (
                    METH_O,
                    cext_dispatch_one as *const (),
                    2,
                    "dispatch_probe() takes exactly one argument (2 given)",
                ),
            ] {
                let function = cext_dispatch_function_for(target, flags);
                assert_ne!(function, 0);
                let args = [int(10), int(20)];
                for admitted in [false, true] {
                    let _execution = admitted.then(RuntimeExecutionGuard::enter);
                    CEXT_DISPATCH.with(|state| *state.borrow_mut() = Default::default());
                    // Call private fixed arms directly: the public vector entry
                    // can intercept the trampoline before these arms are reached.
                    let result = cext_bound_call(&py, function, &args[..width]);
                    dec_ref_bits(&py, result);
                    CEXT_DISPATCH.with(|state| assert_eq!(state.borrow().calls, 0));
                    {
                        let error =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert!(
                            !error.as_ptr().is_null(),
                            "flags={flags:#x}, admitted={admitted}"
                        );
                        let class = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Type(
                            error.as_ptr(),
                        ));
                        assert_eq!(class.as_ptr(), (&raw mut PyExc_TypeError).cast());
                        let message = refcount::OwnedPyObject::from_owned(typeobj::PyObject_Str(
                            error.as_ptr(),
                        ));
                        assert!(!message.as_ptr().is_null());
                        let text = strings::PyUnicode_AsUTF8(message.as_ptr());
                        assert!(!text.is_null());
                        assert_eq!(
                            std::ffi::CStr::from_ptr(text).to_bytes(),
                            expected.as_bytes(),
                            "flags={flags:#x}, admitted={admitted}",
                        );
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(&py));
                    assert_eq!(current_thread_has_c_extension_execution_context(), admitted);
                }
                dec_ref_bits(&py, function);
            }
            assert!(!current_thread_has_c_extension_execution_context());
        });
    }

    #[test]
    fn cext_moved_arguments_remain_owned_through_success_and_exact_failure() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil(|py| unsafe {
            use crate::concurrency::execution::{
                RuntimeExecutionGuard, current_thread_has_c_extension_execution_context,
            };
            let function = cext_dispatch_function();
            assert_ne!(function, 0);
            let list = alloc_list(&py, &[]);
            assert!(!list.is_null());
            let value = MoltObject::from_ptr(list).bits();
            // The first borrowed C view owns one stable runtime hold.
            // Measure per-call balance only after that canonical publication.
            let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(value);
            assert!(!view.is_null());
            let baseline = ref_count(value);
            let recursion = crate::state::recursion::recursion_depth();
            let frames = crate::FRAME_STACK.with(|stack| stack.borrow().len());
            assert!(!current_thread_has_c_extension_execution_context());
            for admitted in [false, true] {
                let _execution = admitted.then(RuntimeExecutionGuard::enter);
                for fail in [false, true] {
                    CEXT_DISPATCH.with(|state| {
                        *state.borrow_mut() = CextDispatchObservation {
                            fail,
                            ..Default::default()
                        };
                    });
                    // The same object occupies two separately owned operands.
                    crate::inc_ref_bits(&py, value);
                    crate::inc_ref_bits(&py, value);
                    let result = super::call_function_obj_moved(&py, function, &[value, value]);
                    assert_cext_dispatch(recursion, frames, 2, 0);
                    assert_eq!(current_thread_has_c_extension_execution_context(), admitted);
                    if fail {
                        assert!(crate::exception_pending(&py));
                        let raised = CEXT_DISPATCH.with(|state| state.borrow().raised);
                        let pending = crate::molt_exception_last_pending();
                        assert_ne!(raised, 0);
                        assert_eq!(
                            pending, raised,
                            "argument cleanup replaced the callback error"
                        );
                        crate::molt_exception_clear();
                        molt_cpython_abi::api::errors::PyErr_Clear();
                        dec_ref_bits(&py, raised);
                        dec_ref_bits(&py, pending);
                    } else {
                        assert!(!crate::exception_pending(&py));
                        assert_eq!(result, value);
                    }
                    dec_ref_bits(&py, result);
                    assert_eq!(ref_count(value), baseline);
                }
                // The C borrowing rule does not authorize moved input to arbitrary
                // borrowing functions, which are not identified by the C trampoline.
                let ordinary = crate::builtins::functions::alloc_runtime_function_obj(
                    &py,
                    identity_returns_owned_arg as *const () as usize as u64,
                    1,
                );
                assert!(!ordinary.is_null());
                let ordinary = MoltObject::from_ptr(ordinary).bits();
                crate::inc_ref_bits(&py, value);
                let result = super::call_function_obj_moved(&py, ordinary, &[value]);
                assert!(crate::exception_pending(&py));
                let pending = crate::molt_exception_last_pending();
                assert_eq!(
                    crate::builtins::exceptions::exception_class(&py, pending)
                        .unwrap()
                        .bits(),
                    crate::builtins::exceptions::exception_type_bits_from_name(&py, "SystemError"),
                );
                crate::molt_exception_clear();
                molt_cpython_abi::api::errors::PyErr_Clear();
                dec_ref_bits(&py, pending);
                dec_ref_bits(&py, result);
                dec_ref_bits(&py, ordinary);
                assert_eq!(ref_count(value), baseline);
            }
            assert!(!current_thread_has_c_extension_execution_context());
            dec_ref_bits(&py, value);
            dec_ref_bits(&py, function);
        });
    }

    // An ordinary call owns its callable. A temporary bound method ends before
    // its function runs, so at entry the receiver's only references are the
    // test's and the frame's `self`. A lane that kept the method until the
    // call returned would show one more, and a lane that forgot to release it
    // would leave the receiver and the function retained afterwards.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn an_adopted_bound_method_ends_before_its_function_runs() {
        use std::sync::atomic::Ordering::SeqCst;
        init();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                fixed_arity_returns_first_param as *const () as usize as u64,
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            unsafe {
                crate::object::layout::function_set_trampoline_ptr(
                    func_ptr,
                    adopting_trampoline as *const () as usize as u64,
                );
                assert_eq!(
                    crate::object::layout::function_publish_entry_custody(
                        func_ptr,
                        crate::object::layout::EntryCustody::Adopting,
                    ),
                    Ok(())
                );
            }
            let receiver_ptr = alloc_list(_py, &[int(1)]);
            assert!(!receiver_ptr.is_null());
            let receiver_bits = MoltObject::from_ptr(receiver_ptr).bits();
            let baseline = ref_count(receiver_bits);
            let function_baseline = ref_count(func_bits);
            let method_ptr =
                crate::object::builders::alloc_bound_method_obj(_py, func_bits, receiver_bits);
            assert!(!method_ptr.is_null());
            let method_bits = MoltObject::from_ptr(method_ptr).bits();
            assert_eq!(ref_count(receiver_bits), baseline + 1);

            // The call adopts the method's only reference.
            let result = unsafe { crate::call::bind::call_owned_arguments(_py, method_bits, &[]) };
            assert!(obj_from_bits(result).is_none());
            assert!(!crate::exception_pending(_py));
            assert_eq!(ADOPTED_ARGUMENT_REFS.load(SeqCst), baseline + 1);
            assert_eq!(ref_count(receiver_bits), baseline);
            assert_eq!(ref_count(func_bits), function_baseline);

            dec_ref_bits(_py, receiver_bits);
            dec_ref_bits(_py, func_bits);
        });
    }

    #[test]
    fn runtime_invocation_family_restores_baseline_without_clearing_exception() {
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};
        static ENTRIES: AtomicUsize = AtomicUsize::new(0);
        static RAISED: AtomicU64 = AtomicU64::new(0);
        extern "C" fn changes_baseline_and_raises() -> u64 {
            crate::with_gil_entry_nopanic!(py, {
                ENTRIES.fetch_add(1, SeqCst);
                crate::exception_stack_baseline_set(0);
                let result = crate::raise_exception::<u64>(py, "ValueError", "callback failure");
                // Retain the callback's actual exception independently of pending state.
                RAISED.store(crate::molt_exception_last_pending(), SeqCst);
                result
            })
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let _baseline = crate::call::ExceptionBaselineGuard::new();
                let address = changes_baseline_and_raises as *const () as usize as u64;
                let pointer =
                    crate::builtins::functions::alloc_runtime_function_obj(py, address, 0);
                assert!(!pointer.is_null());
                let function = MoltObject::from_ptr(pointer).bits();
                let empty: [u64; 0] = [];
                for lane in 0..5 {
                    ENTRIES.store(0, SeqCst);
                    crate::exception_stack_baseline_set(1);
                    let result = match lane {
                        0 => super::call_function_obj0(py, function),
                        1 => super::call_function_obj_vec(py, function, &[]),
                        2 => crate::molt_call_func_fast0(function),
                        3 => crate::molt_guarded_call(address, empty.as_ptr(), 0),
                        _ => crate::molt_guarded_call_obj(address, empty.as_ptr(), 0, function),
                    };
                    assert_eq!(ENTRIES.load(SeqCst), 1, "lane {lane} did not invoke once");
                    assert_eq!(crate::exception_stack_baseline_get(), 1, "lane {lane}");
                    assert!(
                        crate::exception_pending(py),
                        "lane {lane} erased the exception"
                    );
                    let raised = RAISED.swap(0, SeqCst);
                    let pending = crate::molt_exception_last_pending();
                    assert_eq!(
                        pending, raised,
                        "lane {lane} replaced the callback exception"
                    );
                    assert_eq!(
                        crate::builtins::exceptions::exception_class(py, pending)
                            .unwrap()
                            .bits(),
                        crate::builtins::exceptions::exception_type_bits_from_name(
                            py,
                            "ValueError"
                        ),
                        "lane {lane} changed the exception class"
                    );
                    crate::molt_exception_clear();
                    dec_ref_bits(py, raised);
                    dec_ref_bits(py, pending);
                    dec_ref_bits(py, result);
                }
                dec_ref_bits(py, function);
            }
        });
    }

    #[test]
    fn borrowed_dispatch_binds_defaults_before_wide_fixed_abi_execution() {
        extern "C" fn last_of_thirteen(
            _a: u64,
            _b: u64,
            _c: u64,
            _d: u64,
            _e: u64,
            _f: u64,
            _g: u64,
            _h: u64,
            _i: u64,
            _j: u64,
            _k: u64,
            _l: u64,
            last: u64,
        ) -> i64 {
            identity_returns_owned_arg(last)
        }
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    last_of_thirteen as *const () as usize as u64,
                    13,
                );
                assert!(!function_ptr.is_null());
                let function = MoltObject::from_ptr(function_ptr).bits();
                let value_ptr = alloc_list(py, &[int(31)]);
                assert!(!value_ptr.is_null());
                let value = MoltObject::from_ptr(value_ptr).bits();
                let defaults_ptr = alloc_tuple(py, &[value]);
                assert!(!defaults_ptr.is_null());
                let defaults = MoltObject::from_ptr(defaults_ptr).bits();
                set_function_metadata_attr(py, function_ptr, b"__defaults__", defaults);
                let baseline = ref_count(value);
                let mut arguments = vec![int(1); 12];
                for supplied in [12, 13] {
                    if supplied == 13 {
                        arguments.push(value);
                    }
                    let result = crate::molt_call_func_dispatch(
                        function,
                        arguments.as_ptr() as u64,
                        arguments.len() as u64,
                        0,
                    );
                    assert!(!crate::exception_pending(py));
                    assert_eq!(result, value);
                    assert_eq!(ref_count(value), baseline + 1);
                    dec_ref_bits(py, result);
                    assert_eq!(ref_count(value), baseline);
                }
                for bits in [function, defaults, value] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn descriptor_calls_bind_python_arguments_before_packed_abi_execution() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let none = MoltObject::none().bits();
                let getter_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    identity_returns_owned_arg as *const () as usize as u64,
                    1,
                );
                assert!(!getter_ptr.is_null());
                let getter = MoltObject::from_ptr(getter_ptr).bits();
                let property_ptr = crate::alloc_property_obj(py, none, none, none);
                assert!(!property_ptr.is_null());
                let property = MoltObject::from_ptr(property_ptr).bits();
                let init = crate::builtins::methods::builtin_class_method_bits(
                    py,
                    crate::builtin_classes(py).property,
                    "__init__",
                )
                .unwrap();
                let get = crate::builtins::methods::builtin_class_method_bits(
                    py,
                    crate::builtin_classes(py).property,
                    "__get__",
                )
                .unwrap();
                let set_name = crate::builtins::methods::builtin_class_method_bits(
                    py,
                    crate::builtin_classes(py).property,
                    "__set_name__",
                )
                .unwrap();

                // Two visible arguments happen to equal the native packed ABI
                // arity. They still require binding into (args, kwargs).
                let result = super::call_function_obj2(py, init, property, getter);
                assert!(!crate::exception_pending(py));
                assert_eq!(result, none);
                assert_eq!(crate::property_get_bits(property_ptr), getter);
                dec_ref_bits(py, result);

                let receiver_ptr = alloc_list(py, &[int(7)]);
                assert!(!receiver_ptr.is_null());
                let receiver = MoltObject::from_ptr(receiver_ptr).bits();
                let baseline = ref_count(receiver);
                let result = super::call_function_obj2(py, get, property, receiver);
                assert!(!crate::exception_pending(py));
                assert_eq!(result, receiver);
                assert_eq!(ref_count(receiver), baseline + 1);
                dec_ref_bits(py, result);
                assert_eq!(ref_count(receiver), baseline);

                // Class construction invokes the bound special method with
                // owner/name; neither the owner nor packed tuple is its self.
                let bound = crate::builtins::attr::descriptor_bind(
                    py,
                    set_name,
                    Some(crate::builtin_classes(py).property),
                    Some(property),
                )
                .unwrap();
                let name = string_bits("field");
                let result = crate::call::bind::call_bind_borrowed(
                    py,
                    bound,
                    None,
                    &[crate::builtin_classes(py).object, name],
                    &[],
                    &[],
                );
                assert!(!crate::exception_pending(py));
                assert_eq!(result, none);
                assert_eq!(
                    crate::object::layout::property_name_bits(property_ptr),
                    name
                );
                dec_ref_bits(py, result);

                // Public None is a supplied receiver, not the __get__ sentinel.
                let result = super::call_function_obj2(py, get, none, receiver);
                assert!(crate::exception_pending(py));
                crate::molt_exception_clear();
                dec_ref_bits(py, result);
                assert_eq!(ref_count(receiver), baseline);
                for bits in [bound, property, getter, receiver, name] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn fixed_arity_type_constructor_builtins_route_visible_args_through_binder() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init();
        crate::with_gil_entry_nopanic!(_py, {
            let type_new_bits =
                crate::builtins::methods::type_method_bits(_py, "__new__").expect("type.__new__");
            let type_init_bits =
                crate::builtins::methods::type_method_bits(_py, "__init__").expect("type.__init__");

            for func_bits in [type_new_bits, type_init_bits] {
                let func_ptr = obj_from_bits(func_bits).as_ptr().expect("function object");
                let bind_kind_name = intern_metadata_name(_py, b"__molt_bind_kind__");
                let bind_kind_bits =
                    unsafe { crate::function_attr_bits(_py, func_ptr, bind_kind_name) }
                        .expect("constructor builtin bind kind");
                dec_ref_bits(_py, bind_kind_name);
                assert_eq!(
                    obj_from_bits(bind_kind_bits).as_int(),
                    Some(crate::BIND_KIND_TYPE_NEW_INIT),
                    "constructor builtin binding policy must live in metadata"
                );
                assert!(
                    unsafe { crate::call::bind::function_requires_binder_flag(func_ptr) },
                    "constructor builtin must publish builtin binding policy"
                );
                assert!(
                    unsafe {
                        crate::call::bind::function_raw_positional_call_needs_binding(
                            _py, func_ptr, 4,
                        )
                    },
                    "visible constructor arity must bind before raw runtime arity checks"
                );
            }

            let name_bits = string_bits("MoltBinderProbe");
            let bases_ptr = alloc_tuple(_py, &[]);
            assert!(!bases_ptr.is_null());
            let bases_bits = MoltObject::from_ptr(bases_ptr).bits();
            let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!namespace_ptr.is_null());
            let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();

            let cls_bits = unsafe {
                super::call_function_obj4(
                    _py,
                    type_new_bits,
                    crate::builtin_classes(_py).type_obj,
                    name_bits,
                    bases_bits,
                    namespace_bits,
                )
            };
            assert!(
                !crate::exception_pending(_py),
                "type.__new__ visible-arity call must bind instead of raising raw arity mismatch"
            );
            let cls_ptr = obj_from_bits(cls_bits).as_ptr().expect("created type");
            assert_eq!(
                unsafe { crate::object_type_id(cls_ptr) },
                crate::TYPE_ID_TYPE
            );

            let init_result = unsafe {
                super::call_function_obj4(
                    _py,
                    type_init_bits,
                    cls_bits,
                    name_bits,
                    bases_bits,
                    namespace_bits,
                )
            };
            assert!(
                !crate::exception_pending(_py),
                "type.__init__ visible-arity call must bind instead of raising raw arity mismatch"
            );
            assert!(obj_from_bits(init_result).is_none());

            dec_ref_bits(_py, cls_bits);
            dec_ref_bits(_py, namespace_bits);
            dec_ref_bits(_py, bases_bits);
            dec_ref_bits(_py, name_bits);
        });
    }

    #[test]
    fn call_func_fast1_preserves_callee_owned_arg_alias_return() {
        let _test = crate::test_support::RuntimeTestTransaction::new();
        init();
        crate::with_gil_entry_nopanic!(_py, {
            let func_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                identity_returns_owned_arg as *const () as usize as u64,
                1,
            );
            assert!(!func_ptr.is_null());
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let list_ptr = alloc_list(_py, &[int(17)]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let result = crate::molt_call_func_fast1(func_bits, list_bits);
            assert_eq!(result, list_bits);
            assert_eq!(
                ref_count(result),
                2,
                "call_func fast path returns an owned alias"
            );

            dec_ref_bits(_py, result);
            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, func_bits);
        });
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
        assert!(should_force_trampoline_for_fixed_arity_call(
            293, 4097, true
        ));
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
    #[cfg_attr(miri, ignore = "Miri does not support spawning child test processes")]
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
        crate::with_gil_entry_nopanic!(_py, {
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
