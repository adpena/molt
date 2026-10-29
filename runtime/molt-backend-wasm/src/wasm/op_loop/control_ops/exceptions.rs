use super::{ControlKind, ControlOpContext};
use crate::OpIR;
use crate::wasm_abi::TAG_EXCEPTION_INDEX;
use crate::wasm_binary::emit_call;
use std::borrow::Cow;
use wasm_encoder::{BlockType, Catch, Function, Instruction, ValType};

pub(super) fn emit_exception_control_op(
    context: &mut ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind {
        molt_ir::OpKind::TryStart => emit_try_start(context, func),
        molt_ir::OpKind::TryEnd => emit_try_end(context, func),
        kind if kind.is_exception_check() => emit_check_exception(context, func, op),
        _ => return false,
    }
    true
}

fn emit_try_start(context: &mut ControlOpContext<'_>, func: &mut Function) {
    if context.native_eh_enabled {
        func.instruction(&Instruction::Block(BlockType::Result(ValType::I64)));
        context.control_stack.push(ControlKind::Block);
        func.instruction(&Instruction::TryTable(
            BlockType::Empty,
            Cow::Borrowed(&[Catch::One {
                tag: TAG_EXCEPTION_INDEX,
                label: 0,
            }]),
        ));
        context.control_stack.push(ControlKind::Try);
        context.try_stack.push(context.control_stack.len() - 1);
    } else {
        func.instruction(&Instruction::Block(BlockType::Empty));
        context.control_stack.push(ControlKind::Try);
        context.try_stack.push(context.control_stack.len() - 1);
    }
}

fn emit_try_end(context: &mut ControlOpContext<'_>, func: &mut Function) {
    if context.native_eh_enabled {
        func.instruction(&Instruction::End);
        context.control_stack.pop();
        context.try_stack.pop();
        context.const_cache.emit_none(func);
        func.instruction(&Instruction::End);
        context.control_stack.pop();
        func.instruction(&Instruction::Drop);
    } else {
        func.instruction(&Instruction::End);
        context.control_stack.pop();
        context.try_stack.pop();
    }
}

fn emit_check_exception(context: &ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let async_work_poll = op.is_async_work_poll();
    // The observer is semantically independent of exception-region nesting.
    // A poll outside `try` must still run, then transfer to the explicit
    // function-level exception label carried by the SimpleIR op.  Silently
    // dropping the call here would make straight-line/native-EH code immune to
    // pending calls until some unrelated later safepoint.
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[if async_work_poll {
            crate::wasm_abi_generated::WasmRuntimeImport::AsyncWorkPollAndExceptionPending
        } else {
            crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending
        }],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    // A cleanup landing can lie inside the surrounding handler's try region.
    // Its explicit exceptional successor is the authority on every emission
    // route, exactly as in jumpful/stateful dispatch; the lexical try stack
    // cannot replace it without skipping that cleanup.
    let target = op
        .value
        .unwrap_or_else(|| panic!("{} missing exception label", op.kind));
    let depth = super::branches::label_branch_depth(context, target, op.kind);
    func.instruction(&Instruction::BrIf(depth));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::representation_plan::ScalarRepresentationPlan;
    use crate::wasm::WasmFrameLocals;
    use crate::wasm_import_tracking::TrackedImportIds;
    use crate::wasm_values::ConstantCache;
    use std::collections::BTreeMap;
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, FunctionSection, ImportSection, Module,
        TypeSection,
    };
    use wasmparser::{Operator, Parser, Payload, Validator};

    fn plain_poll_module(
        native_eh_enabled: bool,
        cleanup_inside_try: bool,
    ) -> (Vec<u8>, TrackedImportIds) {
        let poll_import = if cleanup_inside_try {
            crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending
        } else {
            crate::wasm_abi_generated::WasmRuntimeImport::AsyncWorkPollAndExceptionPending
        };
        let trace_import = crate::wasm_abi_generated::WasmRuntimeImport::TraceSetLine;
        let import_ids =
            TrackedImportIds::new(BTreeMap::from([(poll_import, 0), (trace_import, 1)]));
        let func_ir = crate::FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "plain_poll".into(),
            ..Default::default()
        };
        let (_, frame) =
            crate::wasm::function_frame::WasmFunctionFramePlan::for_codegen_fixture(&func_ir)
                .into_function_and_frame();
        let locals = WasmFrameLocals::default();
        let const_cache = ConstantCache::default();
        let scalar_plan = ScalarRepresentationPlan::default();
        let mut control_stack = if cleanup_inside_try {
            vec![ControlKind::Try, ControlKind::Block]
        } else {
            vec![ControlKind::Block]
        };
        let mut try_stack = if cleanup_inside_try {
            vec![0]
        } else {
            Vec::new()
        };
        let mut label_stack = vec![41];
        let mut label_depths = BTreeMap::from([(41, usize::from(cleanup_inside_try))]);
        let op = OpIR {
            kind: if cleanup_inside_try {
                molt_ir::OpKind::CheckException
            } else {
                molt_ir::OpKind::AsyncWorkPoll
            },
            value: Some(41),
            ..OpIR::new(if cleanup_inside_try {
                molt_ir::OpKind::CheckException
            } else {
                molt_ir::OpKind::AsyncWorkPoll
            })
        };
        let mut body = Function::new([]);
        body.instruction(&Instruction::Block(BlockType::Empty));
        if cleanup_inside_try {
            body.instruction(&Instruction::Block(BlockType::Empty));
        }
        let mut context = ControlOpContext {
            return_depth: 0,
            func_ir: &func_ir,
            import_ids: &import_ids,
            locals: &locals,
            const_cache: &const_cache,
            scalar_plan: &scalar_plan,
            frame: &frame,
            control_stack: &mut control_stack,
            try_stack: &mut try_stack,
            label_stack: &mut label_stack,
            label_depths: &mut label_depths,
            reloc_enabled: false,
            native_eh_enabled,
            op_idx: 0,
        };
        assert!(emit_exception_control_op(&mut context, &mut body, &op));
        if cleanup_inside_try {
            body.instruction(&Instruction::I64Const(1));
            emit_call(&mut body, false, import_ids[trace_import]);
            body.instruction(&Instruction::Drop);
            // Success bypasses the exceptional cleanup and enters the outer
            // continuation. Pending failure first lands at the inner label.
            body.instruction(&Instruction::Br(1));
            body.instruction(&Instruction::End);
            body.instruction(&Instruction::I64Const(2));
            emit_call(&mut body, false, import_ids[trace_import]);
            body.instruction(&Instruction::Drop);
            body.instruction(&Instruction::End);
            body.instruction(&Instruction::I64Const(3));
            emit_call(&mut body, false, import_ids[trace_import]);
            body.instruction(&Instruction::Drop);
        } else {
            body.instruction(&Instruction::End);
        }
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::Return);
        body.instruction(&Instruction::End);

        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I64]);
        types.ty().function([ValType::I64], [ValType::I64]);
        module.section(&types);
        let mut imports = ImportSection::new();
        imports.import("molt_runtime", "pending", EntityType::Function(0));
        imports.import("molt_runtime", "record", EntityType::Function(1));
        module.section(&imports);
        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);
        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, 2);
        module.section(&exports);
        let mut code = CodeSection::new();
        code.function(&body);
        module.section(&code);
        (module.finish(), import_ids)
    }

    #[test]
    fn plain_poll_outside_try_calls_observer_and_branches_to_function_exit() {
        for native_eh_enabled in [false, true] {
            let (module, import_ids) = plain_poll_module(native_eh_enabled, false);
            Validator::new().validate_all(&module).unwrap();
            assert!(import_ids.is_used(
                crate::wasm_abi_generated::WasmRuntimeImport::AsyncWorkPollAndExceptionPending
            ));
            let operators = Parser::new(0)
                .parse_all(&module)
                .filter_map(Result::ok)
                .find_map(|payload| match payload {
                    Payload::CodeSectionEntry(body) => Some(
                        body.get_operators_reader()
                            .unwrap()
                            .into_iter()
                            .collect::<Result<Vec<_>, _>>()
                            .unwrap(),
                    ),
                    _ => None,
                })
                .unwrap();
            assert!(matches!(
                operators.get(1),
                Some(Operator::Call { function_index: 0 })
            ));
            assert!(matches!(
                operators.get(4),
                Some(Operator::BrIf { relative_depth: 0 })
            ));
        }
    }

    #[test]
    fn explicit_cleanup_label_inside_try_executes_before_outer_continuation() {
        use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
        let node = std::path::PathBuf::from("node");
        let (module, _) = plain_poll_module(false, true);
        Validator::new().validate_all(&module).unwrap();
        let (temp, _cleanup) = wasm_test_temp_dir();
        let path = temp.join("explicit-cleanup.wasm");
        std::fs::write(&path, module).unwrap();
        run_node_test_script(
            &node,
            r#"
const fs = require('fs'), assert = require('assert/strict');
const module = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
for (const pending of [false, true]) {
  const events = [];
  const app = new WebAssembly.Instance(module, {molt_runtime: {
    pending: () => pending ? 1n : 0n,
    record: line => { events.push(Number(line)); return 0n; },
  }}).exports;
  assert.equal(app.run(), 0n);
  assert.deepEqual(events, pending ? [2, 3] : [1, 3], 'explicit inner cleanup must precede the outer continuation');
}
"#,
            &[&path],
            "explicit exception cleanup inside try",
        );
    }
}
