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

fn make_backend(ctx: &Context) -> LlvmBackend<'_> {
    let backend = LlvmBackend::new(ctx, "test");
    declare_runtime_functions(ctx, &backend.module);
    backend
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
    let mut func = TirFunction::new(name.into(), vec![], TirType::DynBox);
    let operands: Vec<ValueId> = (0..operand_count).map(|_| func.fresh_value()).collect();
    let result = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for &value in &operands {
            entry.ops.push(const_none_def(value));
        }
        entry.ops.push(TirOp {
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
        state_resume_blocks: HashMap::new(),
        try_stack_baselines: Vec::new(),
        call_site_counter: 0,
        diagnostics: RefCell::new(Vec::new()),
        repr_facts: crate::representation_plan::LlvmReprFacts::default(),
    }
}

/// Build the trivial `fn add(a: i64, b: i64) -> i64 { return a + b }` TIR
/// used by the overflow-safety gating tests below.
fn build_i64_add_func() -> (TirFunction, ValueId) {
    let mut func = TirFunction::new(
        "add_i64".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );
    let v_sum = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
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
/// carrying `_original_kind = kind` with `n_operands` ConstNone operands and
/// (optionally) a result, then lower it and return the printed IR. Shared by
/// the preserved-op passthrough-class regressions below.
#[cfg(feature = "llvm")]
fn lower_preserved_kind_ir(
    backend: &LlvmBackend<'_>,
    kind: &str,
    n_operands: usize,
    with_result: bool,
    s_value: Option<&str>,
) -> Result<String, LlvmLoweringError> {
    let mut func = TirFunction::new(format!("preserved_{kind}"), vec![], TirType::DynBox);
    let operands: Vec<ValueId> = (0..n_operands).map(|_| func.fresh_value()).collect();
    let result = with_result.then(|| func.fresh_value());
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for &o in &operands {
        entry.ops.push(const_none_def(o));
    }
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
    if let Some(s) = s_value {
        attrs.insert("s_value".into(), AttrValue::Str(s.to_string()));
    }
    entry.ops.push(TirOp {
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

/// Helper: build a function with `num_blocks` empty blocks (terminators
/// initialized to `Unreachable`; tests overwrite them).
fn make_func_with_blocks(name: &str, num_blocks: u32) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], TirType::I64);
    for _ in 1..num_blocks {
        let bid = func.fresh_block();
        func.blocks.insert(
            bid,
            TirBlock {
                id: bid,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );
    }
    func
}

fn set_term(func: &mut TirFunction, b: BlockId, term: Terminator) {
    func.blocks.get_mut(&b).unwrap().terminator = term;
}

fn position_of(rpo: &[BlockId], b: BlockId) -> usize {
    rpo.iter()
        .position(|x| *x == b)
        .unwrap_or_else(|| panic!("BlockId {:?} not present in RPO {:?}", b, rpo))
}

mod arithmetic;
mod calls_and_containers;
mod control_flow;
mod dynamic_attrs;
mod preserved_ops;
mod rpo;
mod runtime_declarations;
mod scalar_ops;
