use super::*;
use cranelift_codegen::ir::{Block, ExternalName, FuncRef, Inst, InstructionData, ValueDef};
use cranelift_frontend::Variable;

#[test]
fn cold_module_chunk_codegen_classification_only_matches_module_chunks() {
    assert!(is_cold_module_chunk_function(
        "molt_gpu_tensor__molt_module_chunk_2"
    ));
    assert!(is_cold_module_chunk_function(
        "builtins__molt_module_chunk_4"
    ));
    assert!(!is_cold_module_chunk_function(
        "main_molt__Attention___call__"
    ));
    assert!(!is_cold_module_chunk_function(
        "molt_gpu_tensor__Tensor__broadcast_op"
    ));
    assert!(!is_cold_module_chunk_function("molt_main"));
}

/// Lower `op` through `lower` with `wide` and `wider` bound to full-width raw
/// integer parameters, so boxing either operand mints an owner.
fn lower_with_wide_operands(
    op: &OpIR,
    lower: impl FnOnce(
        &mut SimpleBackend,
        &mut FunctionBuilder<'_>,
        &mut BTreeMap<&'static str, FuncRef>,
        &mut BTreeSet<Block>,
        &BTreeMap<String, Variable>,
        &ScalarRepresentationPlan,
    ),
) -> (Function, BTreeMap<&'static str, u32>) {
    let plan = representation_plan_for_ops(&[
        OpIR {
            kind: "const_int".into(),
            value: Some(1_i64 << 31),
            out: Some("limb".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "checked_mul".into(),
            args: Some(vec!["limb".into(), "limb".into()]),
            var: Some("wide".into()),
            out: Some("none".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "checked_add".into(),
            args: Some(vec!["wide".into(), "limb".into()]),
            var: Some("wider".into()),
            out: Some("none".into()),
            ..OpIR::default()
        },
        op.clone(),
    ]);
    for name in op.args.iter().flatten() {
        assert!(
            plan.is_full_deopt_int_name(name),
            "{name}: require a physical box"
        );
    }
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params = vec![AbiParam::new(types::I64); 2];
    let mut function = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let mut vars = BTreeMap::new();
        for (index, name) in ["wide", "wider"].into_iter().enumerate() {
            let var = builder.declare_var(types::I64);
            let raw = builder.block_params(entry)[index];
            builder.def_var(var, raw);
            vars.insert(name.to_string(), var);
        }
        if let Some(out) = op.out.as_deref() {
            vars.insert(out.to_string(), builder.declare_var(types::I64));
        }
        let mut refs = BTreeMap::new();
        let mut sealed = BTreeSet::from([entry]);
        lower(
            &mut backend,
            &mut builder,
            &mut refs,
            &mut sealed,
            &vars,
            &plan,
        );
        builder.ins().return_(&[]);
        builder.finalize();
    }
    verify_function(&function, &settings::Flags::new(settings::builder()))
        .unwrap_or_else(|errors| panic!("{errors}\n{}", function.display()));
    let ids = backend
        .import_ids
        .iter()
        .map(|(&symbol, (id, _))| (symbol, id.as_u32()))
        .collect();
    (function, ids)
}

/// Lower one `slice_new` through its production family handler.
fn lower_slice_new(args: &[&str], out: Option<&str>) -> (Function, BTreeMap<&'static str, u32>) {
    let slice = OpIR {
        kind: "slice_new".into(),
        args: Some(args.iter().map(|arg| arg.to_string()).collect()),
        out: out.map(str::to_string),
        ..OpIR::default()
    };
    lower_with_wide_operands(&slice, |backend, builder, refs, sealed, vars, plan| {
        let (mut tracked_obj, mut tracked_ptr) = (BTreeMap::new(), BTreeMap::new());
        super::super::fc::slice_ops::handle_slice_op(
            &slice,
            0,
            "slice_bounds",
            &mut backend.module,
            &mut backend.import_ids,
            builder,
            refs,
            sealed,
            vars,
            plan,
            &crate::NanBoxConsts::new(),
            &mut tracked_obj,
            &mut tracked_ptr,
        );
    })
}

/// Calls to `symbol`, identified by module function id so every local FuncRef
/// alias counts (the owned-result sink declares its own release reference).
fn import_calls(function: &Function, ids: &BTreeMap<&'static str, u32>, symbol: &str) -> Vec<Inst> {
    let Some(&id) = ids.get(symbol) else {
        return Vec::new();
    };
    function
        .layout
        .blocks()
        .flat_map(|block| function.layout.block_insts(block))
        .filter(|&inst| {
            let InstructionData::Call { func_ref, .. } = function.dfg.insts[inst] else {
                return false;
            };
            let ExternalName::User(name) = function.dfg.ext_funcs[func_ref].name else {
                return false;
            };
            let name = &function.params.user_named_funcs()[name];
            name.namespace == 0 && name.index == id
        })
        .collect()
}

#[test]
fn slice_bounds_share_the_fixed_aggregate_transaction_without_range_storage() {
    for out in [Some("result"), None] {
        let (function, ids) = lower_slice_new(&["wide", "wide", "wider"], out);
        let count = |symbol: &str| import_calls(&function, &ids, symbol).len();
        // One box per distinct bound, each followed by the first-failure stop.
        assert_eq!(count("molt_int_from_i64"), 2, "{}", function.display());
        assert_eq!(count("molt_exception_pending_fast"), 2);
        // Both minted owners are released after the constructor retained them,
        // and a discarded slice is released by the owned-result sink.
        assert_eq!(count("molt_dec_ref_obj"), 2 + usize::from(out.is_none()));
        assert!(
            function.sized_stack_slots.is_empty(),
            "direct bounds need no range storage:\n{}",
            function.display()
        );
        let &[constructor] = import_calls(&function, &ids, "molt_slice_new").as_slice() else {
            panic!("one slice constructor:\n{}", function.display());
        };
        let bounds = function.dfg.inst_args(constructor);
        assert_eq!(bounds[0], bounds[1], "a repeated bound reuses its one box");
        assert_ne!(bounds[1], bounds[2]);
    }
}

#[test]
fn omitted_slice_bounds_are_none_direct_arguments() {
    let cases: [&[&str]; 2] = [&["wide"], &[]];
    for bounds in cases {
        let (function, ids) = lower_slice_new(bounds, Some("result"));
        let count = |symbol: &str| import_calls(&function, &ids, symbol).len();
        assert_eq!(count("molt_int_from_i64"), bounds.len());
        assert_eq!(count("molt_exception_pending_fast"), bounds.len());
        assert!(function.sized_stack_slots.is_empty());
        let &[constructor] = import_calls(&function, &ids, "molt_slice_new").as_slice() else {
            panic!("one slice constructor:\n{}", function.display());
        };
        let arguments = function.dfg.inst_args(constructor);
        assert_eq!(arguments.len(), 3);
        for &omitted in &arguments[bounds.len()..] {
            let ValueDef::Result(definition, _) = function.dfg.value_def(omitted) else {
                panic!("an omitted bound is a constant:\n{}", function.display());
            };
            let InstructionData::UnaryImm { imm, .. } = function.dfg.insts[definition] else {
                panic!("an omitted bound is an immediate:\n{}", function.display());
            };
            assert_eq!(imm.bits(), molt_codegen_abi::box_none_bits());
        }
    }
}

#[test]
fn bound_borrowed_transaction_result_is_secured_before_temporary_release() {
    let op = OpIR {
        kind: "dict_set".into(),
        args: Some(vec!["wide".into(), "wide".into(), "wider".into()]),
        out: Some("result".into()),
        ..OpIR::default()
    };
    let (function, ids) =
        lower_with_wide_operands(&op, |backend, builder, refs, sealed, vars, plan| {
            let (mut tracked_obj, mut tracked_ptr) = (BTreeMap::new(), BTreeMap::new());
            super::super::shared::emit_operand_transaction_call(
                &op,
                op.args.as_deref().unwrap(),
                "molt_dict_set",
                &mut backend.module,
                &mut backend.import_ids,
                builder,
                refs,
                sealed,
                vars,
                plan,
                &crate::NanBoxConsts::new(),
                &mut tracked_obj,
                &mut tracked_ptr,
            );
        });
    let count = |symbol: &str| import_calls(&function, &ids, symbol).len();
    assert_eq!(count("molt_int_from_i64"), 2, "{}", function.display());
    assert_eq!(count("molt_exception_pending_fast"), 2);
    let &[call] = import_calls(&function, &ids, "molt_dict_set").as_slice() else {
        panic!("one runtime call:\n{}", function.display());
    };
    let arguments = function.dfg.inst_args(call);
    assert_eq!(
        arguments[0], arguments[1],
        "a repeated operand reuses its one box"
    );
    assert_ne!(arguments[1], arguments[2]);
    // The borrowed return may alias the first minted box. It acquires its own
    // credit in the call's block, before the join releases both owners.
    let &[retain] = import_calls(&function, &ids, "molt_inc_ref_obj").as_slice() else {
        panic!(
            "one retain for the bound borrowed result:\n{}",
            function.display()
        );
    };
    assert_eq!(
        function.dfg.inst_args(retain),
        function.dfg.inst_results(call)
    );
    let call_block = function.layout.inst_block(call);
    assert_eq!(function.layout.inst_block(retain), call_block);
    let releases = import_calls(&function, &ids, "molt_dec_ref_obj");
    assert_eq!(
        releases.len(),
        2,
        "release both minted owners, never the result"
    );
    for release in releases {
        assert_ne!(
            function.layout.inst_block(release),
            call_block,
            "{}",
            function.display()
        );
    }
}
