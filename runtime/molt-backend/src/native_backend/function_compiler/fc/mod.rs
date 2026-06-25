//! `fc` — per-op-family Cranelift codegen handlers extracted from the
//! `compile_func_inner` monolith (decomposition program M1).
//!
//! `compile_func_inner` was once a ~34K-line method and remains the largest
//! native-codegen function; its per-op `match op.kind`
//! dispatch shares ~100 `let mut` locals. Splitting the *file* alone buys no
//! incremental-build win — rustc's `codegen-units` partition at *function*
//! boundaries, so a monolithic function is one indivisible codegen unit
//! regardless of how many files surround it (see
//! `docs/design/foundation/dx_baseline.md` §4). The build-throughput lever is
//! decomposing the *function*: each handler here is a standalone `fn` and thus
//! its own codegen unit, lifted out of the monolith.
//!
//! Each family handler is a free `fn` that takes the shared lowering state as
//! explicit split-borrowed `&mut` params — the same idiom the backend already
//! uses for `SimpleBackend::import_func_id_split` and
//! `var_get_boxed_overflow_safe_base`, which take `module` / `import_ids`
//! separately so a concurrent `FunctionBuilder` borrow on `self.ctx.func` can
//! coexist. Arm bodies are moved verbatim (byte-identical Cranelift IR); only
//! the access path to backend fields changes. `compile_func_inner`'s dispatch
//! becomes a thin delegating arm per extracted family.

use super::*;

/// Control-flow signal a family handler returns to `compile_func_inner`'s
/// dispatch loop, so a handler whose arm body used `continue` on the outer
/// `for op_idx` loop replicates that control flow exactly (a bare `continue`
/// skips the per-op epilogue; falling through to it would not be equivalent).
///
/// `Proceed` — fall through to the per-op epilogue (the default; an arm that
/// completed normally). `Continue` — the arm `continue`d the op loop (skips the
/// epilogue). A `Break` variant is added when the first `break`-using family is
/// extracted (none of the currently-extracted families break the op loop).
#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::native_backend::function_compiler) enum OpFlow {
    Proceed,
    Continue,
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn op_prefers_int_lane(
    scalar_fast_paths_enabled: bool,
    representation_plan: &ScalarRepresentationPlan,
    op: &OpIR,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    int_carriers_plan: &ScalarRepresentationPlan,
    bool_primary_vars: &BTreeSet<String>,
) -> bool {
    let name_is_integer_scalar = |name: &str| {
        int_like_vars.contains(name)
            || bool_like_vars.contains(name)
            || int_carriers_plan.is_raw_int_carrier_name(name)
            || bool_primary_vars.contains(name)
    };
    let op_args_are_integer_scalar = op
        .args
        .as_ref()
        .is_some_and(|args| !args.is_empty() && args.iter().all(|arg| name_is_integer_scalar(arg)));
    scalar_fast_paths_enabled
        && (representation_plan.op_scalar_lane(op) == Some(ScalarKind::Int)
            || (matches!(
                op.kind.as_str(),
                "add"
                    | "inplace_add"
                    | "sub"
                    | "inplace_sub"
                    | "mul"
                    | "inplace_mul"
                    | "floordiv"
                    | "inplace_floordiv"
                    | "mod"
                    | "mod_"
                    | "inplace_mod"
                    | "lt"
                    | "le"
                    | "gt"
                    | "ge"
                    | "eq"
                    | "ne"
            ) && op_args_are_integer_scalar))
}

pub(in crate::native_backend::function_compiler) mod arith;
pub(in crate::native_backend::function_compiler) mod attrs;
pub(in crate::native_backend::function_compiler) mod callargs;
pub(in crate::native_backend::function_compiler) mod calls;
pub(in crate::native_backend::function_compiler) mod class_ops;
pub(in crate::native_backend::function_compiler) mod compare;
pub(in crate::native_backend::function_compiler) mod const_literals;
pub(in crate::native_backend::function_compiler) mod context_mgmt;
pub(in crate::native_backend::function_compiler) mod control_flow;
pub(in crate::native_backend::function_compiler) mod coroutine;
pub(in crate::native_backend::function_compiler) mod dataclass;
pub(in crate::native_backend::function_compiler) mod dict_ops;
pub(in crate::native_backend::function_compiler) mod exception_control;
pub(in crate::native_backend::function_compiler) mod exception_stack;
pub(in crate::native_backend::function_compiler) mod exceptions;
pub(in crate::native_backend::function_compiler) mod file_io;
pub(in crate::native_backend::function_compiler) mod funcobj;
pub(in crate::native_backend::function_compiler) mod future_promise;
pub(in crate::native_backend::function_compiler) mod generators;
pub(in crate::native_backend::function_compiler) mod indexing;
pub(in crate::native_backend::function_compiler) mod list_index_fast_path;
pub(in crate::native_backend::function_compiler) mod list_ops;
pub(in crate::native_backend::function_compiler) mod loops;
pub(in crate::native_backend::function_compiler) mod memory;
pub(in crate::native_backend::function_compiler) mod memoryview_buffer;
pub(in crate::native_backend::function_compiler) mod modules;
pub(in crate::native_backend::function_compiler) mod object_construct;
pub(in crate::native_backend::function_compiler) mod op_family;
pub(in crate::native_backend::function_compiler) mod parse_ops;
pub(in crate::native_backend::function_compiler) mod ret_jump;
pub(in crate::native_backend::function_compiler) mod runtime_ops;
pub(in crate::native_backend::function_compiler) mod scalar_builtins;
pub(in crate::native_backend::function_compiler) mod sequence_ops;
pub(in crate::native_backend::function_compiler) mod set_ops;
pub(in crate::native_backend::function_compiler) mod statistics;
pub(in crate::native_backend::function_compiler) mod text_predicates;
pub(in crate::native_backend::function_compiler) mod text_transform;
pub(in crate::native_backend::function_compiler) mod type_checks;
pub(in crate::native_backend::function_compiler) mod type_conversions;
pub(in crate::native_backend::function_compiler) mod unary_logic;
pub(in crate::native_backend::function_compiler) mod value_transfer;
pub(in crate::native_backend::function_compiler) mod vec_reductions;

// Single-source-of-truth op-kind routing (see `op_family`): the dispatch in
// `compile_func_inner` consults `native_op_family` instead of hand-mirroring
// each handler's kind list, so the dispatch can no longer drift from a handler.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) use op_family::{
    NATIVE_NO_CODEGEN_RESULT_KINDS, NativeOpFamily, native_op_family,
};

/// Free-function form of `compile_func_inner`'s op-local
/// `var_get_boxed_overflow_safe` closure: box a variable's value
/// overflow-safely, special-casing bool-primary carriers (raw 0/1 ->
/// TAG_BOOL NaN-box) before delegating to `var_get_boxed_overflow_safe_base`.
///
/// The inline closure captured `bool_primary_vars` + `nbc`; here they are
/// explicit params so extracted family handlers can reconstruct the exact
/// closure shape (capturing these two) and leave their moved arm bodies
/// unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn var_get_boxed_overflow_safe_fn(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    name: &str,
    int_carriers_plan: &ScalarRepresentationPlan,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
) -> Option<crate::VarValue> {
    if bool_primary_vars.contains(name) {
        let raw = vars.get(name).map(|&var| builder.use_var(var))?;
        return Some(crate::VarValue(box_raw_bool_value(builder, raw, nbc)));
    }
    var_get_boxed_overflow_safe_base(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        name,
        int_carriers_plan,
        float_primary_vars,
    )
}
