use super::*;
use crate::llvm_backend::LlvmBackend;
use crate::llvm_backend::runtime_imports::declare_runtime_functions;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use inkwell::attributes::Attribute;
use inkwell::context::Context;
use inkwell::values::AnyValue;
use molt_ir::OpKind;

/// Expected IR spells logical names; project those expectations through the
/// generated execution ABI. The emitted IR is inspected without rewriting it.
/// Naming the hidden formal preserves LLVM's existing logical `%0` numbering.
fn execution_ir_pattern(pattern: &str) -> String {
    use molt_ir::runtime_callable_abi_generated::{RuntimeExecutionContext, runtime_execution_abi};
    let mut result = String::with_capacity(pattern.len());
    let bytes = pattern.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let start = cursor;
        if bytes[cursor] == b'@' {
            result.push('@');
            cursor += 1;
        }
        let identifier = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        let name = &pattern[identifier..cursor];
        let callable = cursor < bytes.len()
            && bytes[cursor] == b'('
            && (identifier != start || name.starts_with("molt_") || name.starts_with("__molt_"));
        if callable {
            let abi = runtime_execution_abi(name);
            let attached = abi.is_none_or(|abi| abi.context == RuntimeExecutionContext::Attached);
            result.push_str(&crate::llvm_backend::execution_abi::execution_symbol(name));
            result.push('(');
            cursor += 1;
            if attached {
                result.push_str("i64 %execution_context");
                if cursor < bytes.len() && bytes[cursor] != b')' {
                    result.push_str(", ");
                }
            }
        } else {
            result.push_str(name);
        }
        if (cursor == start || cursor == identifier) && cursor < bytes.len() {
            let character = pattern[cursor..].chars().next().unwrap();
            result.push(character);
            cursor += character.len_utf8();
        }
    }
    result
}

trait ExecutionIrAssertions {
    fn contains_execution(&self, pattern: &str) -> bool;
    fn matches_execution(&self, pattern: &str) -> std::vec::IntoIter<&str>;
    fn find_execution(&self, pattern: &str) -> Option<usize>;
    fn match_indices_execution(&self, pattern: &str) -> std::vec::IntoIter<(usize, &str)>;
}

impl ExecutionIrAssertions for str {
    fn contains_execution(&self, pattern: &str) -> bool {
        self.contains(&execution_ir_pattern(pattern))
    }
    fn matches_execution(&self, pattern: &str) -> std::vec::IntoIter<&str> {
        self.matches(&execution_ir_pattern(pattern))
            .collect::<Vec<_>>()
            .into_iter()
    }
    fn find_execution(&self, pattern: &str) -> Option<usize> {
        self.find(&execution_ir_pattern(pattern))
    }
    fn match_indices_execution(&self, pattern: &str) -> std::vec::IntoIter<(usize, &str)> {
        let expanded = execution_ir_pattern(pattern);
        self.match_indices(&expanded)
            .map(|(index, matched)| (index, &self[index..index + matched.len()]))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

fn make_backend(ctx: &Context) -> LlvmBackend<'_> {
    let backend = LlvmBackend::new(ctx, "test");
    declare_runtime_functions(ctx, &backend.module);
    backend
}

fn test_native_linkage_abi(
    param_types: Vec<TirType>,
    return_type: Option<TirType>,
) -> crate::NativeFunctionLinkageAbi {
    crate::NativeFunctionLinkageAbi {
        source_signature: crate::ir::ExternFunctionSignature {
            arity: param_types.len(),
            has_closure: false,
            returns_value: return_type.is_some(),
            execution_context: crate::ir::ExecutionContextPolicy::None,
        },
        parameter_custody: Vec::new(),
        param_types,
        return_type,
    }
}

fn has_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
    let kind_id = Attribute::get_named_enum_kind_id(attr_name);
    kind_id == 0
        || func
            .get_enum_attribute(AttributeLoc::Function, kind_id)
            .is_some()
}

fn lacks_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
    let kind_id = Attribute::get_named_enum_kind_id(attr_name);
    kind_id == 0
        || func
            .get_enum_attribute(AttributeLoc::Function, kind_id)
            .is_none()
}

fn assert_lowering_error_contains(err: &LlvmLoweringError, needle: &str) {
    let joined = err.diagnostics().join("\n");
    assert!(
        joined.contains(needle),
        "expected lowering diagnostic containing {needle:?}, got:\n{joined}"
    );
}

fn const_none_def(result: ValueId) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn lowering_error_for_single_op(
    name: &str,
    dialect: Dialect,
    opcode: OpCode,
    operand_count: usize,
) -> (LlvmLoweringError, LlvmBackend<'static>) {
    let ctx = Box::leak(Box::new(Context::create()));
    let backend = make_backend(ctx);
    let mut func = TirFunction::new(
        name.into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let operands: Vec<ValueId> = (0..operand_count).map(|_| func.fresh_value()).collect();
    let result = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for &value in &operands {
            entry.ops.push(const_none_def(value));
        }
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect,
            opcode,
            operands,
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
    }

    (
        try_lower_tir_to_llvm(&func, &backend)
            .expect_err("removed runtime delegate must fail LLVM lowering"),
        backend,
    )
}

fn const_int_def(result: ValueId, value: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

fn make_dummy_lowering<'ctx, 'func>(
    backend: &'func LlvmBackend<'ctx>,
    func: &'func TirFunction,
    llvm_fn: FunctionValue<'ctx>,
) -> FunctionLowering<'ctx, 'func> {
    FunctionLowering {
        backend,
        func,
        llvm_fn,
        entry_trampoline_bb: None,
        block_map: HashMap::new(),
        values: HashMap::new(),
        value_types: HashMap::new(),
        pending_phis: Vec::new(),
        phi_edges: Vec::new(),
        pgo_branch_weights: None,
        pgo_weight_index: 0,
        const_str_counter: 0,
        synthetic_block_counter: 0,
        all_llvm_blocks: Vec::new(),
        llvm_pred_map: HashMap::new(),
        call_site_counter: 0,
        diagnostics: RefCell::new(Vec::new()),
        repr_facts: crate::representation_plan::ProducerCarrierPlan::default(),
        guard_facts: molt_tir::passes::SsaRuntimeGuardFacts::for_function(func),
        guard_profile_flag: None,
        poll_context: None,
        polls_async_work: super::function_polls_async_work(func),
        frame_homes: None,
        refcount_admission: std::cell::Cell::new(None),
        owned_frame_entered: None,
    }
}

/// Build the trivial `fn add(a: i64, b: i64) -> i64 { return a + b }` TIR
/// used by the overflow-safety gating tests below.
fn build_i64_add_func() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "add_i64".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let v_sum = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v_sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_sum],
    };
    (func, v_sum)
}

/// Build a single-block function whose only op is a preserved `Copy`
/// carrying `origin = kind` with `n_operands` ConstNone operands and
/// (optionally) a result, then lower it and return the printed IR. Shared by
/// the preserved-op passthrough-class regressions below.
#[cfg(feature = "llvm")]
fn lower_preserved_kind_ir(
    backend: &LlvmBackend<'_>,
    kind: OpKind,
    n_operands: usize,
    with_result: bool,
    s_value: Option<&str>,
) -> Result<String, LlvmLoweringError> {
    let mut func = TirFunction::new(
        format!("preserved_{kind}"),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let operands: Vec<ValueId> = (0..n_operands).map(|_| func.fresh_value()).collect();
    let result = with_result.then(|| func.fresh_value());
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for &o in &operands {
        entry.ops.push(const_none_def(o));
    }
    let mut attrs = AttrDict::new();
    let origin = Some(kind);
    if let Some(s) = s_value {
        attrs.insert("s_value".into(), AttrValue::Str(s.to_string()));
    }
    entry.ops.push(TirOp {
        origin,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results: result.into_iter().collect(),
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: result.into_iter().collect(),
    };
    try_lower_tir_to_llvm(&func, backend).map(|f| f.print_to_string().to_string())
}

mod activation_exits;
mod arithmetic;
mod calls_and_containers;
mod constructor_operands;
mod control_flow;
mod dynamic_attrs;
mod preserved_ops;
mod runtime_declarations;
mod scalar_ops;
mod task_trampoline;
