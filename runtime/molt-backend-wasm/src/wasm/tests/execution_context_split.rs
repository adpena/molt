use super::support::*;
use crate::ir::ExecutionContextPolicy;
use wasmparser::Operator;

/// Explore the emitted structured control-flow graph, treating data-dependent
/// branches nondeterministically. A static exit-site count is not a frame proof:
/// the split owner has normal, exceptional and chunk-stop return paths. Each
/// reachable path must mint/pop its frame exactly once (inherited chunks: zero).
/// The finite state is (instruction, enters, exits); invalid counts fail before
/// insertion, so loops cannot hide an accumulating frame leak or make this walk
/// unbounded. Unsupported control families fail closed, rather than falling
/// through as if they were arithmetic.
fn frame_paths(
    ops: &[Operator<'_>],
    enter: u32,
    exit: u32,
    owned_frames: u8,
) -> Result<usize, String> {
    let terminal = ops.len();
    let mut ends = vec![terminal; terminal + 1];
    let mut alternatives = vec![None; terminal];
    let mut target_owners = vec![Vec::new(); terminal];
    let mut else_owners = vec![None; terminal];
    let mut stack = vec![terminal]; // The implicit function label.
    for (index, op) in ops.iter().enumerate() {
        // Retain only the labels actually referenced by edges, not a copy of
        // the whole control stack at every instruction: O(instructions + edges)
        // storage even for deeply nested emitted blocks.
        let branch_owner = |depth: u32| -> Result<usize, String> {
            let slot = stack
                .len()
                .checked_sub(depth as usize)
                .and_then(|slot| slot.checked_sub(1))
                .ok_or_else(|| format!("invalid branch depth {depth} at {index}"))?;
            Ok(stack[slot])
        };
        match op {
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                stack.push(index)
            }
            Operator::Else => {
                let owner = *stack.last().ok_or("else without an if")?;
                if owner == terminal || !matches!(ops[owner], Operator::If { .. }) {
                    return Err(format!("else {index} does not belong to an if"));
                }
                alternatives[owner] = Some(index + 1);
                else_owners[index] = Some(owner);
            }
            Operator::End => {
                let owner = stack.pop().ok_or("end without a control label")?;
                ends[owner] = index + 1;
            }
            Operator::Br { relative_depth } | Operator::BrIf { relative_depth } => {
                target_owners[index].push(branch_owner(*relative_depth)?);
            }
            Operator::BrTable { targets } => {
                target_owners[index].push(branch_owner(targets.default())?);
                for depth in targets.targets() {
                    target_owners[index]
                        .push(branch_owner(depth.map_err(|error| error.to_string())?)?);
                }
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("unterminated WASM control label".into());
    }
    let mut pending = vec![(0, 0u8, 0u8)];
    let mut visited = BTreeSet::new();
    let mut returns = BTreeSet::new();
    while let Some((index, mut enters, mut exits)) = pending.pop() {
        if !visited.insert((index, enters, exits)) {
            continue;
        }
        if index == terminal {
            if (enters, exits) != (owned_frames, owned_frames) {
                return Err(format!(
                    "function fallthrough has {enters} entries and {exits} exits"
                ));
            }
            returns.insert(index);
            continue;
        }
        let branch = |owner: usize| -> usize {
            if owner < terminal && matches!(ops[owner], Operator::Loop { .. }) {
                owner + 1
            } else {
                ends[owner]
            }
        };
        let mut successors = Vec::new();
        match &ops[index] {
            Operator::Call { function_index } if *function_index == enter => {
                if enters == owned_frames {
                    return Err(format!("unexpected frame entry at instruction {index}"));
                }
                enters += 1;
                successors.push(index + 1);
            }
            Operator::Call { function_index } if *function_index == exit => {
                if exits == enters {
                    return Err(format!(
                        "frame pop without owned entry at instruction {index}"
                    ));
                }
                exits += 1;
                successors.push(index + 1);
            }
            Operator::If { .. } => {
                successors.extend([index + 1, alternatives[index].unwrap_or(ends[index])]);
            }
            Operator::Else => successors.push(ends[else_owners[index].unwrap()]),
            Operator::Br { .. } => successors.push(branch(target_owners[index][0])),
            Operator::BrIf { .. } => {
                successors.extend([index + 1, branch(target_owners[index][0])])
            }
            Operator::BrTable { .. } => {
                successors.extend(target_owners[index].iter().copied().map(branch));
            }
            Operator::Return => {
                if (enters, exits) != (owned_frames, owned_frames) {
                    return Err(format!(
                        "return {index} has {enters} entries and {exits} exits"
                    ));
                }
                returns.insert(index);
            }
            Operator::Unreachable => {} // A trap is not a normal function return.
            Operator::Block { .. }
            | Operator::Loop { .. }
            | Operator::End
            | Operator::Nop
            | Operator::Call { .. }
            | Operator::CallIndirect { .. }
            | Operator::Drop
            | Operator::Select
            | Operator::LocalGet { .. }
            | Operator::LocalSet { .. }
            | Operator::LocalTee { .. }
            | Operator::GlobalGet { .. }
            | Operator::GlobalSet { .. }
            | Operator::I32Const { .. }
            | Operator::I64Const { .. }
            | Operator::F32Const { .. }
            | Operator::F64Const { .. }
            | Operator::I32Load { .. }
            | Operator::I64Load { .. }
            | Operator::I32Store { .. }
            | Operator::I64Store { .. }
            | Operator::I32Eqz
            | Operator::I64Eqz
            | Operator::I32Eq
            | Operator::I32Ne
            | Operator::I64Eq
            | Operator::I64Ne
            | Operator::I32WrapI64
            | Operator::I64ExtendI32U
            | Operator::I64ExtendI32S
            | Operator::I32Add
            | Operator::I32Sub
            | Operator::I32And
            | Operator::I32Or
            | Operator::I64Add
            | Operator::I64Sub
            | Operator::I64And
            | Operator::I64Or
            | Operator::I64Shl
            | Operator::I64ShrU
            | Operator::I64ShrS => successors.push(index + 1),
            other => {
                return Err(format!(
                    "unsupported frame-proof operator at {index}: {other:?}"
                ));
            }
        }
        pending.extend(successors.into_iter().map(|next| (next, enters, exits)));
    }
    Ok(returns.len())
}

#[test]
fn wasm_compiles_split_local_frame_with_inherited_chunks() {
    let mut ops = vec![OpIR {
        kind: "trace_enter_slot".to_string(),
        value: Some(5),
        ..OpIR::default()
    }];
    for line in 1..=6 {
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(line),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some(format!("v{line}")),
            ..OpIR::default()
        });
    }
    ops.extend([
        OpIR {
            kind: "trace_exit".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
    ]);
    let original = FunctionIR {
        name: "wasm_framed_large".to_string(),
        ops,
        execution_context: ExecutionContextPolicy::Local,
        ..FunctionIR::default()
    };
    let mut occupied = BTreeSet::from([original.name.clone()]);
    let (stub, chunks) = crate::passes::split_large_function(original, 3, &mut occupied).unwrap();
    let stub_name = stub.name.clone();
    let chunk_names = chunks
        .iter()
        .map(|chunk| chunk.name.clone())
        .collect::<Vec<_>>();
    let ir = SimpleIR {
        functions: std::iter::once(stub).chain(chunks).collect(),
        profile: None,
    };
    crate::validate_simple_ir(&ir).unwrap();
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    let imports = wasm_function_import_names(&wasm);
    assert!(imports.iter().any(|name| name == "trace_enter_slot"));
    assert!(imports.iter().any(|name| name == "trace_exit"));
    let import_indices = wasm_function_import_indices(&wasm);
    let enter = import_indices["trace_enter_slot"];
    let exit = import_indices["trace_exit"];
    let owner_ops = wasm_operators_for_export(&wasm, &stub_name);
    assert!(frame_paths(&owner_ops, enter, exit, 1).unwrap() > 0);
    let exports = wasm_function_export_indices(&wasm);
    for chunk_name in chunk_names {
        assert!(
            exports.contains_key(&chunk_name),
            "missing split chunk {chunk_name}"
        );
        let calls = wasm_direct_call_indices_for_export(&wasm, &chunk_name);
        assert!(!calls.contains(&enter), "inherited chunk minted a frame");
        assert!(!calls.contains(&exit), "inherited chunk popped its caller");
        let operators = wasm_operators_for_export(&wasm, &chunk_name);
        assert!(frame_paths(&operators, enter, exit, 0).unwrap() > 0);
    }
}

#[test]
fn frame_path_proof_checks_branch_targets_and_rejects_balanced_site_count_lies() {
    use wasmparser::BlockType;
    let enter = || Operator::Call { function_index: 0 };
    let exit = || Operator::Call { function_index: 1 };
    let good = vec![
        enter(),
        Operator::Block {
            blockty: BlockType::Empty,
        },
        Operator::BrIf { relative_depth: 0 },
        exit(),
        Operator::Return,
        Operator::End,
        exit(),
        Operator::Return,
        Operator::End,
    ];
    assert_eq!(frame_paths(&good, 0, 1, 1).unwrap(), 2);
    // One entry/one exit in the byte stream, but the taken branch leaks its frame.
    let missing = vec![
        enter(),
        Operator::Block {
            blockty: BlockType::Empty,
        },
        Operator::BrIf { relative_depth: 0 },
        exit(),
        Operator::Return,
        Operator::End,
        Operator::Return,
        Operator::End,
    ];
    assert!(
        frame_paths(&missing, 0, 1, 1)
            .unwrap_err()
            .contains("1 entries and 0 exits")
    );
    let double = vec![enter(), exit(), exit(), Operator::Return, Operator::End];
    assert!(
        frame_paths(&double, 0, 1, 1)
            .unwrap_err()
            .contains("pop without owned entry")
    );
    let reentry = vec![
        enter(),
        Operator::Loop {
            blockty: BlockType::Empty,
        },
        exit(),
        enter(),
        Operator::BrIf { relative_depth: 0 },
        Operator::End,
        exit(),
        Operator::Return,
        Operator::End,
    ];
    assert!(
        frame_paths(&reentry, 0, 1, 1)
            .unwrap_err()
            .contains("unexpected frame entry")
    );
    assert!(frame_paths(&[enter(), exit(), Operator::End], 0, 1, 0).is_err());

    let alternatives = vec![
        enter(),
        Operator::If {
            blockty: BlockType::Empty,
        },
        exit(),
        Operator::Return,
        Operator::Else,
        exit(),
        Operator::Return,
        Operator::End,
        Operator::Unreachable,
        // Statically present, but not executable on either branch.
        exit(),
        Operator::End,
    ];
    assert_eq!(frame_paths(&alternatives, 0, 1, 1).unwrap(), 2);
    let loop_without_frame_mutation = vec![
        enter(),
        Operator::Loop {
            blockty: BlockType::Empty,
        },
        Operator::BrIf { relative_depth: 0 },
        Operator::End,
        exit(),
        Operator::End,
    ];
    assert_eq!(
        frame_paths(&loop_without_frame_mutation, 0, 1, 1).unwrap(),
        1
    );
    let escape_function = vec![
        enter(),
        Operator::Br { relative_depth: 0 },
        exit(),
        Operator::End,
    ];
    assert!(
        frame_paths(&escape_function, 0, 1, 1)
            .unwrap_err()
            .contains("fallthrough")
    );
}

#[test]
fn exported_body_inspection_counts_import_entries_not_unique_names() {
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
        ImportSection, Instruction, Module, TypeSection,
    };
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("first", "same_name", EntityType::Function(0));
    imports.import("second", "same_name", EntityType::Function(0));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("owner", ExportKind::Func, 2);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(1));
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);
    let wasm = module.finish();
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    assert_eq!(wasm_direct_call_indices_for_export(&wasm, "owner"), vec![1]);
    assert_eq!(wasm_operator_debug_for_export(&wasm, "owner").len(), 2);
}
