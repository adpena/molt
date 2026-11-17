//! Typed native field-access consumer contracts; no dependence on CLIF spelling.
use super::*;
use cranelift_codegen::dominator_tree::DominatorTree;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{Function, InstructionData, Opcode};
use molt_ir::OpKind;
use molt_ir::deterministic_hash::HashSet;

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
    let mut seen = HashSet::default();
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

/// Every memory access addressed off `receiver` (its header, its fields)
/// lies in a block the receiver's pointer-tag test dominates: a non-pointer
/// receiver (a failed allocation's sentinel) is never dereferenced.
fn assert_receiver_reads_are_guarded(func: &Function, receiver: Value) {
    let cfg = ControlFlowGraph::with_function(func);
    let domtree = DominatorTree::with_function(func, &cfg);
    let guarded: Vec<_> = func
        .layout
        .blocks()
        .filter_map(|block| {
            let branch = func.layout.last_inst(block)?;
            let InstructionData::Brif { blocks, .. } = &func.dfg.insts[branch] else {
                return None;
            };
            let condition = func.dfg.inst_args(branch)[0];
            (pointer_guard_receiver(func, condition)
                .is_some_and(|tested| func.dfg.resolve_aliases(tested) == receiver))
            .then(|| blocks[0].block(&func.dfg.value_lists))
        })
        .collect();
    assert!(
        !guarded.is_empty(),
        "receiver pointer test: {}",
        func.display()
    );
    let mut accesses = 0;
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            if !matches!(
                func.dfg.insts[inst].opcode(),
                Opcode::Load | Opcode::Store | Opcode::AtomicRmw
            ) {
                continue;
            }
            let args = func.dfg.inst_args(inst);
            let address = if func.dfg.insts[inst].opcode() == Opcode::Store {
                args[1]
            } else {
                args[0]
            };
            if !depends_on(func, address, receiver) {
                continue;
            }
            accesses += 1;
            assert!(
                guarded
                    .iter()
                    .any(|&then| domtree.block_dominates(then, block)),
                "unguarded receiver access: {}",
                func.display()
            );
        }
    }
    assert!(
        accesses > 0,
        "the access reads the receiver: {}",
        func.display()
    );
}

/// A proven read and store test the receiver's type id and its sticky
/// instance-dictionary bit before they touch the inline word
/// (`molt_codegen_abi::field_slot`).
#[test]
fn proven_field_accesses_admit_the_receiver_before_its_words() {
    for kind in [OpKind::Load, OpKind::Store] {
        let store = kind == OpKind::Store;
        let params: Vec<String> = if store {
            vec!["object".into(), "value".into()]
        } else {
            vec!["object".into()]
        };
        let ops = vec![
            OpIR {
                args: Some(params.clone()),
                out: (!store).then(|| "result".into()),
                value: Some(8),
                ..OpIR::new(kind)
            },
            if store {
                OpIR::new(OpKind::RetVoid)
            } else {
                OpIR {
                    args: Some(vec!["result".into()]),
                    ..OpIR::new(OpKind::Ret)
                }
            },
        ];
        let function = FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "proven_field".into(),
            params,
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let clif = compile_function_to_clif(vec![function], "proven_field");
        let entry = clif.layout.entry_block().unwrap();
        // Parameter 0 is the execution context; the receiver follows.
        let receiver = clif.dfg.block_params(entry)[1];
        assert_receiver_reads_are_guarded(&clif, receiver);
        let tests_dictionary_bit = clif.layout.blocks().any(|block| {
            clif.layout.block_insts(block).any(|inst| {
                clif.dfg.insts[inst].opcode() == Opcode::Band
                    && masked_operand(
                        &clif,
                        clif.dfg.first_result(inst),
                        i64::from(molt_codegen_abi::HEADER_FLAG_HAS_INSTANCE_DICT),
                    )
                    .is_some()
            })
        });
        assert!(tests_dictionary_bit, "{kind}: {}", clif.display());
    }
}

#[test]
fn native_field_reads_keep_runtime_missing_and_owner_resolution() {
    {
        let kind = OpKind::GuardedFieldGet;
        let guarded = kind == OpKind::GuardedFieldGet;
        let params = if guarded {
            vec!["object".into(), "version".into()]
        } else {
            vec!["object".into()]
        };
        let function = FunctionIR {
            module: None,
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
        // Entry reads of the thread record (the attached context's record,
        // then its admission and profile words) are execution state, not
        // field reads.
        let context = clif.dfg.block_params(clif.layout.entry_block().unwrap())[0];
        let reads_thread_record = |inst| {
            let address = clif.dfg.inst_args(inst)[0];
            address == context
                || definition(&clif, address).is_some_and(|def| {
                    clif.dfg.insts[def].opcode() == Opcode::Load
                        && clif.dfg.inst_args(def)[0] == context
                })
        };
        let i64_loads: Vec<_> = instructions
            .iter()
            .copied()
            .filter(|&inst| {
                clif.dfg.insts[inst].opcode() == Opcode::Load
                    && clif.dfg.value_type(clif.dfg.first_result(inst)) == types::I64
                    && !reads_thread_record(inst)
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
            module: None,
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
    use crate::native_backend::native_ssa::FunctionBuilder;
    use cranelift_codegen::ir::InstBuilder;
    use cranelift_frontend::FunctionBuilderContext;

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
