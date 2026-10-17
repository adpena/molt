use super::WasmFunctionLoweringPlan;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use crate::wasm::body::WasmLirFallbackReason;
use crate::{FunctionIR, SimpleIR};
use std::collections::BTreeSet;
use wasm_encoder::Instruction;

fn const_i64_return_func(value: i64) -> TirFunction {
    let mut func = TirFunction::new(
        "const_i64_return".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn literal_const_return_func(
    name: &str,
    opcode: OpCode,
    return_type: TirType,
    attrs: AttrDict,
) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![],
        return_type,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn add_two_i64_params_func() -> TirFunction {
    let mut func = TirFunction::new(
        "add_two_i64_params".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn checked_mul_i64_consts_func() -> TirFunction {
    let mut func = TirFunction::new(
        "checked_mul_i64_consts".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let lhs = func.fresh_value();
    let rhs = func.fresh_value();
    let product = func.fresh_value();
    let overflow = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs, 6), (rhs, 7)] {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![id],
            attrs,
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckedMul,
        operands: vec![lhs, rhs],
        results: vec![product, overflow],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![product],
    };
    func
}

#[test]
fn wasm_lir_fast_plan_respects_proven_value_and_authored_void_returns() {
    let proven = const_i64_return_func(42);
    let plan = plan_refined_fixture(&proven);
    let body = plan
        .lir_fast_body()
        .expect("proven i64 const return must keep the LIR fast plan");
    assert_eq!(plan.generic_reason(), None);
    assert_eq!(body.bail_to_generic_reason(), None);
    assert_eq!(body.result_types, vec![wasm_encoder::ValType::I64]);

    let mut void = proven;
    void.return_abi = molt_ir::FunctionReturnAbi::Void;
    void.return_type = TirType::None;
    void.blocks.get_mut(&void.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    let plan = plan_refined_fixture(&void);
    assert!(plan.lir_fast_body().is_none());
    assert_eq!(
        plan.generic_reason(),
        Some(WasmLirFallbackReason::BoxedI64AbiUnsupported)
    );
}

#[test]
fn wasm_lir_fast_plan_accepts_materialized_literal_consts() {
    let cases = [
        {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("hello".into()));
            (
                "const_str_literal",
                OpCode::ConstStr,
                TirType::Str,
                attrs,
                "string_from_bytes",
            )
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "bytes".into(),
                AttrValue::Bytes(vec![0, 0x80, 0, 0, 0, 0, 0, 0, 0]),
            );
            (
                "const_bigint_literal",
                OpCode::ConstBigInt,
                TirType::DynBox,
                attrs,
                "bigint_from_bytes",
            )
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert("bytes".into(), AttrValue::Bytes(vec![0, 1, 2, 255]));
            (
                "const_bytes_literal",
                OpCode::ConstBytes,
                TirType::Bytes,
                attrs,
                "bytes_from_bytes",
            )
        },
    ];

    for (name, opcode, return_type, attrs, import_name) in cases {
        let plan =
            plan_refined_fixture(&literal_const_return_func(name, opcode, return_type, attrs));
        let body = plan
            .lir_fast_body()
            .unwrap_or_else(|| panic!("{name} must stay on the LIR fast plan"));
        let view = body.test_view();

        assert_eq!(plan.generic_reason(), None);
        assert_eq!(body.bail_to_generic_reason(), None);
        assert!(
            view.runtime_calls.contains(&import_name),
            "{name} must materialize through {import_name}; got {:?}",
            view.runtime_calls
        );
    }
}

#[test]
fn wasm_lir_fast_plan_accepts_boxed_single_return_carriers() {
    let cases = [
        {
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Bool(true));
            ("const_bool_return", OpCode::ConstBool, TirType::Bool, attrs)
        },
        {
            let mut attrs = AttrDict::new();
            attrs.insert("f_value".into(), AttrValue::Float(1.25));
            (
                "const_float_return",
                OpCode::ConstFloat,
                TirType::F64,
                attrs,
            )
        },
    ];

    for (name, opcode, return_type, attrs) in cases {
        let plan =
            plan_refined_fixture(&literal_const_return_func(name, opcode, return_type, attrs));
        let body = plan
            .lir_fast_body()
            .unwrap_or_else(|| panic!("{name} must stay on the boxed-i64 fast plan"));

        assert_eq!(plan.generic_reason(), None);
        assert_eq!(body.result_types, vec![wasm_encoder::ValType::I64]);
        assert_eq!(body.bail_to_generic_reason(), None);
    }
}

#[test]
fn wasm_lir_fast_plan_records_boxed_i64_abi_rejection() {
    let plan = plan_refined_fixture(&add_two_i64_params_func());

    assert_eq!(
        plan.generic_reason(),
        Some(WasmLirFallbackReason::BoxedI64AbiUnsupported)
    );
}

#[test]
fn wasm_lir_fast_plan_accepts_raw_checked_mul_body() {
    let plan = plan_refined_fixture(&checked_mul_i64_consts_func());
    let body = plan.lir_fast_body().unwrap_or_else(|| {
        panic!(
            "raw checked_mul const body must keep the LIR fast plan; reason={:?}",
            plan.generic_reason()
        )
    });
    let view = body.test_view();

    assert_eq!(plan.generic_reason(), None);
    assert_eq!(body.bail_to_generic_reason(), None);
    assert!(
        view.instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Mul)),
        "checked_mul plan body must emit wrapping i64.mul"
    );
    assert!(
        view.instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64DivS)),
        "checked_mul plan body must emit exact overflow division check"
    );
}

#[test]
fn wasm_lir_fast_plan_records_escaped_callable_reason() {
    let name = "pkg____molt_globals_builtin__escaped".to_string();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: name.clone(),
            params: vec![],
            ops: vec![],
            param_types: Some(vec![]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let escaped = BTreeSet::from([name.clone()]);
    let plans = crate::wasm::final_functions::WasmFinalFunctions::lowering_plans_for_fixtures(
        &ir, &escaped,
    );

    assert_eq!(
        plans
            .get(&name)
            .and_then(WasmFunctionLoweringPlan::generic_reason),
        Some(WasmLirFallbackReason::EscapedCallableTarget)
    );
}

fn plan_refined_fixture(function: &TirFunction) -> WasmFunctionLoweringPlan {
    let mut function = function.clone();
    crate::tir::type_refine::refine_types(&mut function);
    let ranges = crate::representation_plan::value_range_for(&function);
    let repr = crate::representation_plan::repr_by_value_for(&function, Some(&ranges));
    super::prepare_lir_wasm_fast_plan(&function, &repr, &ranges)
}

#[test]
fn wasm_final_body_and_facts_follow_typed_custody_through_retention_and_escape() {
    use crate::wasm::final_functions::WasmFinalFunctions;
    // Source is intentionally stale: the final SSA owner has replaced its
    // floating result with an exact integer. Re-lifting this source would
    // silently recover the wrong value and the wrong physical carrier.
    let root = "pkg____molt_globals_builtin__root";
    let source = |name: &str| FunctionIR {
        name: name.into(),
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops: vec![
            crate::OpIR {
                kind: "const_float".into(),
                out: Some("stale_value".into()),
                f_value: Some(1.25),
                ..Default::default()
            },
            crate::OpIR {
                kind: "ret".into(),
                args: Some(vec!["stale_value".into()]),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let external = FunctionIR {
        name: "external".into(),
        is_extern: true,
        ..source("external")
    };
    let ir = SimpleIR {
        functions: vec![source(root), external.clone(), source("dead")],
        profile: None,
    };
    let typed = |name: &str, value: i64| {
        let mut function = const_i64_return_func(value);
        function.name = name.into();
        crate::tir::type_refine::refine_types(&mut function);
        function
    };
    // Owned module assembly groups externs before locals; the final owner must
    // preserve authored declaration order and retire whole unreachable rows.
    let mut final_functions = WasmFinalFunctions::from_final_tir(
        ir,
        vec![
            (true, crate::tir::lower_from_simple::lower_to_tir(&external)),
            (false, typed(root, 42)),
            (false, typed("dead", 99)),
        ],
    );
    final_functions.retain_reachable(&BTreeSet::from([root.into(), "external".into()]));
    final_functions.exclude_escaped_callables(&BTreeSet::from([root.into()]));
    let rows: Vec<_> = final_functions.into_functions().collect();
    assert_eq!(
        rows.iter()
            .map(|(body, _)| body.name.as_str())
            .collect::<Vec<_>>(),
        vec![root, "external"]
    );
    assert!(
        rows[1].1.is_none(),
        "extern declarations must not own executable facts"
    );
    let (body, facts) = &rows[0];
    let facts = facts.as_ref().unwrap();
    assert!(!body.ops.iter().any(|op| op.kind == "const_float"));
    let literal = body
        .ops
        .iter()
        .find(|op| op.kind == "const" && op.value == Some(42))
        .expect("final typed value");
    assert!(
        facts
            .representation
            .is_inline_safe_int_name(literal.out.as_deref().unwrap())
    );
    assert!(!facts.representation.is_inline_safe_int_name("stale_value"));
    assert_eq!(
        facts
            .lowering
            .as_ref()
            .and_then(WasmFunctionLoweringPlan::generic_reason),
        Some(WasmLirFallbackReason::EscapedCallableTarget)
    );
}

#[test]
fn wasm_terminal_reachability_preserves_unused_import_effects_and_balances_ownership() {
    let mut function = const_i64_return_func(7);
    let dead = function.fresh_value();
    let imported = function.fresh_value();
    let entry = function.blocks.get_mut(&function.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![dead],
        attrs: AttrDict::from([("value".into(), AttrValue::Int(123))]),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Import,
        operands: vec![],
        results: vec![imported],
        attrs: AttrDict::from([("module".into(), AttrValue::Str("side_effect_module".into()))]),
        source_span: None,
    });
    crate::tir::drop_phase::finalize_function_drops(
        &mut function,
        &crate::tir::TargetInfo::wasm_release_fast(),
    );
    let ops: Vec<_> = function
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .collect();
    assert!(
        !ops.iter().any(|op| op.results.contains(&dead)),
        "dead pure definition must disappear before ownership insertion"
    );
    assert_eq!(
        ops.iter().filter(|op| op.opcode == OpCode::Import).count(),
        1,
        "unused import still executes initialization and may raise"
    );
    assert_eq!(
        ops.iter()
            .filter(|op| op.opcode == OpCode::DecRef && op.operands.contains(&imported))
            .count(),
        1,
        "the surviving owned import result needs exactly one release"
    );
    let projected = crate::tir::lower_to_simple::lower_to_simple_ir(&function);
    assert!(projected.iter().any(|op| op.kind == "import"
        && op.s_value.as_deref() == Some("side_effect_module")
        && op.out.is_some()
        && op.var.is_none()));
}

#[test]
fn wasm_final_owner_consumes_module_and_terminal_cached_pipeline_modes() {
    use crate::tir::pipeline_cache::{
        TirOwnedModulePipelineMode, TirOwnedModulePipelineOptions, TirPipelineCacheFlavor,
        TirPipelineInput, TirPipelineRunOptions, run_cached_tir_pipeline,
        run_owned_module_pipeline_from_cached_tir, tir_optimization_resource_plan,
    };
    use crate::wasm::final_functions::WasmFinalFunctions;
    let target = crate::tir::TargetInfo::wasm_release_fast();
    let non_inlinable = std::collections::HashSet::new();
    for mode in [
        TirOwnedModulePipelineMode::ModulePhase,
        TirOwnedModulePipelineMode::TerminalDropsOnly,
    ] {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".into(),
                return_abi: molt_ir::FunctionReturnAbi::Value,
                ops: vec![
                    crate::OpIR {
                        kind: "const".into(),
                        value: Some(7),
                        out: Some("answer".into()),
                        ..Default::default()
                    },
                    crate::OpIR {
                        kind: "ret".into(),
                        args: Some(vec!["answer".into()]),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            profile: None,
        };
        let mut run = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut ir.functions),
            TirPipelineRunOptions {
                target_info: target.clone(),
                cache_flavor: TirPipelineCacheFlavor::Wasm,
                cache_dir: None,
                process_externs: false,
                verify_lir: true,
                tir_dump: false,
                tir_stats: false,
                progress_prefix: None,
                resource_plan: tir_optimization_resource_plan(),
            },
            |_| {},
        );
        // Custody is already transferred. A consumer that re-lifts this source
        // would emit 900 instead of 7 despite a successful optimization run.
        ir.functions[0].ops = vec![
            crate::OpIR {
                kind: "const".into(),
                value: Some(900),
                out: Some("poison".into()),
                ..Default::default()
            },
            crate::OpIR {
                kind: "ret".into(),
                args: Some(vec!["poison".into()]),
                ..Default::default()
            },
        ];
        let typed = run_owned_module_pipeline_from_cached_tir(
            &ir.functions,
            &mut run.cached_tir,
            TirOwnedModulePipelineOptions {
                target_info: &target,
                module_name: "wasm_final_modes",
                non_inlinable: &non_inlinable,
                missing_tir_context: "WASM final owner test",
                mode,
            },
        );
        let (body, facts) = WasmFinalFunctions::from_final_tir(ir, typed)
            .into_functions()
            .next()
            .unwrap();
        let literal = body
            .ops
            .iter()
            .find(|op| op.kind == "const" && op.value == Some(7))
            .expect("retained optimized SSA result");
        assert!(!body.ops.iter().any(|op| op.value == Some(900)));
        assert!(
            facts
                .unwrap()
                .representation
                .is_inline_safe_int_name(literal.out.as_deref().unwrap())
        );
        assert_eq!(
            body.ops
                .iter()
                .filter(|op| op.kind == "drop_inserted")
                .count(),
            1,
            "{mode:?} must project one terminal ownership cut"
        );
    }
}
