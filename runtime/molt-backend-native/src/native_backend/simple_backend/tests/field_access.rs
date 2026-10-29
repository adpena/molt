//! Typed native field-access consumer contracts; no dependence on CLIF spelling.
use super::*;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{Function, InstructionData, Opcode};
use molt_ir::OpKind;
use std::collections::HashSet;

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
                args: Some(vec!["class".into()]),
                out: Some("object".into()),
                value: Some(24),
                ..OpIR::new(OpKind::ObjectNewBound)
            });
        }
        ops.push(OpIR {
            out: Some("scalar".into()),
            value: Some(1),
            ..OpIR::new(OpKind::Const)
        });
        let store = OpIR {
            args: Some(vec!["object".into(), "scalar".into()]),
            value: Some(0),
            ..OpIR::new(OpKind::Store)
        };
        ops.extend([store.clone(), store]);
        ops.push(OpIR {
            ..OpIR::new(OpKind::RetVoid)
        });
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
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
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let clif = compile_function_to_clif(vec![function], "field_backing_guards");
        assert_field_store_guards(&clif);
    }
}

#[test]
fn native_field_reads_keep_runtime_missing_and_owner_resolution() {
    for kind in [OpKind::Load, OpKind::GuardedFieldGet] {
        let guarded = kind == OpKind::GuardedFieldGet;
        let params = if guarded {
            vec!["object".into(), "version".into()]
        } else {
            vec!["object".into()]
        };
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "field_read_admission".into(),
            params: params.clone(),
            ops: vec![
                OpIR {
                    args: Some(params),
                    out: Some("value".into()),
                    value: Some(0),
                    s_value: guarded.then(|| "field".into()),
                    class_definition: guarded.then(|| "class:m:Owner".into()),
                    ..OpIR::new(kind)
                },
                OpIR {
                    args: Some(vec!["value".into()]),
                    ..OpIR::new(OpKind::Ret)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
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
        // HF-220: a guarded read loads its class statement's program slot
        // from the slot binding symbol; no other 64-bit load may appear.
        let loads_program_slot = |inst| {
            let address = clif.dfg.inst_args(inst)[0];
            definition(&clif, address)
                .is_some_and(|def| clif.dfg.insts[def].opcode() == Opcode::SymbolValue)
        };
        let i64_loads: Vec<_> = instructions
            .iter()
            .copied()
            .filter(|&inst| {
                clif.dfg.insts[inst].opcode() == Opcode::Load
                    && clif.dfg.value_type(clif.dfg.first_result(inst)) == types::I64
            })
            .collect();
        assert_eq!(
            i64_loads
                .iter()
                .filter(|&&inst| loads_program_slot(inst))
                .count(),
            usize::from(guarded),
            "{kind}: {}",
            clif.display()
        );
        assert!(
            i64_loads.iter().all(|&inst| loads_program_slot(inst)),
            "native reads must retain runtime missing/owner resolution: {}",
            clif.display()
        );
    }
}

#[test]
fn native_guarded_object_helpers_receive_the_tagged_receiver() {
    for kind in [
        OpKind::GuardedFieldGet,
        OpKind::GuardedFieldSet,
        OpKind::GuardLayout,
    ] {
        let params: Vec<String> = match kind {
            OpKind::GuardedFieldSet => vec!["object", "version", "value"],
            _ => vec!["object", "version"],
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut ops = vec![OpIR {
            args: Some(params.clone()),
            out: Some("result".into()),
            value: matches!(kind, OpKind::GuardedFieldGet | OpKind::GuardedFieldSet).then_some(0),
            s_value: matches!(kind, OpKind::GuardedFieldGet | OpKind::GuardedFieldSet)
                .then(|| "field".into()),
            class_definition: Some("class:m:Owner".into()),
            ..OpIR::new(kind)
        }];
        ops.push(OpIR {
            args: Some(vec!["result".into()]),
            ..OpIR::new(OpKind::Ret)
        });
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "tagged_guarded_receiver".into(),
            params,
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
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

#[test]
fn pointer_decode_preserves_unsigned_bit_47_in_emitted_arithmetic() {
    use cranelift_codegen::ir::InstBuilder;
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    let mut function = Function::new();
    function
        .signature
        .params
        .push(cranelift_codegen::ir::AbiParam::new(types::I64));
    let mut context = FunctionBuilderContext::new();
    let (input, output) = {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let input = builder.append_block_param(entry, types::I64);
        builder.switch_to_block(entry);
        let output = crate::native_backend::simple_backend::value_encoding::unbox_ptr_value(
            &mut builder,
            input,
        );
        builder.ins().return_(&[]);
        builder.seal_all_blocks();
        builder.finalize(crate::test_support::host_frontend_config());
        (input, output)
    };
    // Check the actual emitted result, not a second decoder implementation.
    // This equality excludes any arithmetic shift/sign extension downstream of
    // the mask, including the former bit-47 corruption on AArch64 stacks.
    assert_eq!(
        masked_operand(&function, output, 0x0000_ffff_ffff_ffff),
        Some(input),
        "pointer recovery must preserve every unsigned payload bit: {}",
        function.display()
    );
}
