//! Typed native field-access consumer contracts; no dependence on CLIF spelling.
use super::*;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{Function, Inst, InstructionData, Opcode, ValueDef};
use std::collections::HashSet;

fn definition(func: &Function, value: Value) -> Option<Inst> {
    match func.dfg.value_def(func.dfg.resolve_aliases(value)) {
        ValueDef::Result(inst, _) => Some(inst),
        ValueDef::Param(_, _) | ValueDef::Union(_, _) => None,
    }
}

fn constant(func: &Function, value: Value) -> Option<i64> {
    match func.dfg.insts[definition(func, value)?] {
        InstructionData::UnaryImm {
            opcode: Opcode::Iconst,
            imm,
        } => Some(imm.bits()),
        _ => None,
    }
}

fn masked_operand(func: &Function, value: Value, mask: i64) -> Option<Value> {
    let inst = definition(func, value)?;
    if func.dfg.insts[inst].opcode() != Opcode::Band {
        return None;
    }
    let [left, right] = func.dfg.inst_args(inst) else {
        return None;
    };
    if constant(func, *left) == Some(mask) {
        Some(*right)
    } else if constant(func, *right) == Some(mask) {
        Some(*left)
    } else {
        None
    }
}

fn comparison_operand(
    func: &Function,
    condition: Value,
    code: IntCC,
    immediate: i64,
) -> Option<Value> {
    let inst = definition(func, condition)?;
    if func.dfg.insts[inst].cond_code() != Some(code) {
        return None;
    }
    let [left, right] = func.dfg.inst_args(inst) else {
        return None;
    };
    if constant(func, *left) == Some(immediate) {
        Some(*right)
    } else if constant(func, *right) == Some(immediate) {
        Some(*left)
    } else {
        None
    }
}

fn pointer_guard_receiver(func: &Function, condition: Value) -> Option<Value> {
    let tagged = comparison_operand(
        func,
        condition,
        IntCC::Equal,
        molt_codegen_abi::QNAN_TAG_PTR_I64,
    )?;
    masked_operand(func, tagged, molt_codegen_abi::QNAN_TAG_MASK_I64)
}

fn depends_on(func: &Function, value: Value, source: Value) -> bool {
    let source = func.dfg.resolve_aliases(source);
    let mut pending = vec![value];
    let mut seen = HashSet::new();
    while let Some(value) = pending.pop() {
        let value = func.dfg.resolve_aliases(value);
        if value == source {
            return true;
        }
        if seen.insert(value)
            && let Some(inst) = definition(func, value)
        {
            pending.extend_from_slice(func.dfg.inst_args(inst));
        }
    }
    false
}

fn assert_field_store_guards(func: &Function) {
    let cfg = ControlFlowGraph::with_function(func);
    let mut header_count = 0;
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            let InstructionData::Load {
                offset,
                arg: address,
                ..
            } = func.dfg.insts[inst]
            else {
                continue;
            };
            let header = func.dfg.first_result(inst);
            if i32::from(offset) != molt_codegen_abi::HEADER_FLAGS_OFFSET
                || func.dfg.value_type(header) != types::I32
            {
                continue;
            }
            header_count += 1;
            let predecessors: Vec<_> = cfg.pred_iter(block).collect();
            assert!(
                !predecessors.is_empty(),
                "header must be guarded: {}",
                func.display()
            );
            for predecessor in predecessors {
                let InstructionData::Brif { blocks, .. } = &func.dfg.insts[predecessor.inst] else {
                    panic!("unguarded header predecessor: {}", func.display());
                };
                assert_eq!(blocks[0].block(&func.dfg.value_lists), block);
                let condition = func.dfg.inst_args(predecessor.inst)[0];
                let receiver = pointer_guard_receiver(func, condition)
                    .expect("header requires a pointer-tag admission");
                assert!(
                    depends_on(func, address, receiver),
                    "guard must admit this header's receiver"
                );
            }
            let mask = func
                .layout
                .block_insts(block)
                .find_map(|candidate| {
                    if func.dfg.insts[candidate].opcode() != Opcode::Band {
                        return None;
                    }
                    let result = func.dfg.first_result(candidate);
                    masked_operand(
                        func,
                        result,
                        i64::from(molt_codegen_abi::HEADER_FLAG_HAS_PTRS),
                    )
                    .filter(|value| depends_on(func, *value, header))
                    .map(|_| result)
                })
                .expect("header HAS_PTRS exclusion must use the loaded flags");
            let branch = func
                .layout
                .last_inst(block)
                .expect("guard block terminator");
            let InstructionData::Brif { blocks, .. } = &func.dfg.insts[branch] else {
                panic!("backing admission must branch");
            };
            let condition = definition(func, func.dfg.inst_args(branch)[0])
                .expect("backing admission requires an explicit predicate");
            assert_eq!(func.dfg.insts[condition].opcode(), Opcode::Bor);
            let [left, right] = func.dfg.inst_args(condition) else {
                panic!("slow admission must combine backing and value predicates");
            };
            let is_backed = |value| {
                comparison_operand(func, value, IntCC::NotEqual, 0)
                    .is_some_and(|value| func.dfg.resolve_aliases(value) == mask)
            };
            let new_value = if is_backed(*left) {
                pointer_guard_receiver(func, *right)
            } else if is_backed(*right) {
                pointer_guard_receiver(func, *left)
            } else {
                panic!("HAS_PTRS != 0 must select the slow path");
            }
            .expect("pointer values must also select the slow path");
            let slow = blocks[0].block(&func.dfg.value_lists);
            let fast = blocks[1].block(&func.dfg.value_lists);
            assert!(
                func.layout
                    .block_insts(slow)
                    .any(|inst| func.dfg.insts[inst].opcode() == Opcode::Call)
            );
            let store = func
                .layout
                .block_insts(fast)
                .find(|&inst| func.dfg.insts[inst].opcode() == Opcode::Store)
                .expect("unbacked non-pointer values must use inline storage");
            assert_eq!(
                func.dfg.resolve_aliases(func.dfg.inst_args(store)[0]),
                func.dfg.resolve_aliases(new_value),
                "value admission must test the actual stored value"
            );
        }
    }
    assert_eq!(
        header_count, 2,
        "each ordinary store must retain its receiver/backing guard"
    );
}

#[test]
fn field_store_modes_do_not_bypass_receiver_and_backing_guards() {
    for fresh in [false, true] {
        let mut ops = Vec::new();
        if fresh {
            ops.push(OpIR {
                kind: "object_new_bound".into(),
                args: Some(vec!["class".into()]),
                out: Some("object".into()),
                value: Some(24),
                ..OpIR::default()
            });
        }
        ops.push(OpIR {
            kind: "const".into(),
            out: Some("scalar".into()),
            value: Some(1),
            ..OpIR::default()
        });
        let store = OpIR {
            kind: "store".into(),
            args: Some(vec!["object".into(), "scalar".into()]),
            value: Some(0),
            ..OpIR::default()
        };
        ops.extend([store.clone(), store]);
        ops.push(OpIR {
            kind: "ret_void".into(),
            ..OpIR::default()
        });
        let function = FunctionIR {
            name: "field_backing_guards".into(),
            params: vec![if fresh {
                "class".into()
            } else {
                "object".into()
            }],
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        };
        let clif = compile_function_to_clif(vec![function], "field_backing_guards");
        assert_field_store_guards(&clif);
    }
}

#[test]
fn native_field_reads_keep_runtime_missing_and_owner_resolution() {
    for kind in ["load", "guarded_field_get"] {
        let guarded = kind == "guarded_field_get";
        let params = if guarded {
            vec!["object".into(), "class".into(), "version".into()]
        } else {
            vec!["object".into()]
        };
        let function = FunctionIR {
            name: "field_read_admission".into(),
            params: params.clone(),
            ops: vec![
                OpIR {
                    kind: kind.into(),
                    args: Some(params),
                    out: Some("value".into()),
                    value: Some(0),
                    s_value: guarded.then(|| "field".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["value".into()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        };
        let clif = compile_function_to_clif(vec![function], "field_read_admission");
        let instructions: Vec<_> = clif
            .layout
            .blocks()
            .flat_map(|block| clif.layout.block_insts(block))
            .collect();
        assert!(
            instructions
                .iter()
                .any(|&inst| clif.dfg.insts[inst].opcode() == Opcode::Call)
        );
        assert!(
            !instructions.iter().any(|&inst| {
                clif.dfg.insts[inst].opcode() == Opcode::Load
                    && clif.dfg.value_type(clif.dfg.first_result(inst)) == types::I64
            }),
            "native reads must retain runtime missing/owner resolution: {}",
            clif.display()
        );
    }
}

#[test]
fn native_guarded_object_helpers_receive_the_tagged_receiver() {
    for kind in ["guarded_field_get", "guarded_field_set", "guard_layout"] {
        let params: Vec<String> = match kind {
            "guarded_field_set" => vec!["object", "class", "version", "value"],
            _ => vec!["object", "class", "version"],
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut ops = vec![OpIR {
            kind: kind.into(),
            args: Some(params.clone()),
            out: Some("result".into()),
            value: kind.starts_with("guarded_field").then_some(0),
            s_value: kind.starts_with("guarded_field").then(|| "field".into()),
            ..OpIR::default()
        }];
        ops.push(OpIR {
            kind: "ret".into(),
            args: Some(vec!["result".into()]),
            ..OpIR::default()
        });
        let function = FunctionIR {
            name: "tagged_guarded_receiver".into(),
            params,
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        };
        let clif = compile_function_to_clif(vec![function], "tagged_guarded_receiver");
        let instructions: Vec<_> = clif
            .layout
            .blocks()
            .flat_map(|block| clif.layout.block_insts(block))
            .collect();
        assert!(
            instructions
                .iter()
                .all(|&inst| !matches!(clif.dfg.insts[inst].opcode(), Opcode::Ishl | Opcode::Sshr)),
            "{kind} must preserve the tagged receiver: {}",
            clif.display()
        );
    }
}
