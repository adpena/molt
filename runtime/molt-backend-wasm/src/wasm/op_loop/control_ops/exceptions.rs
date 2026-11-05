use super::{ControlKind, ControlOpContext};
use crate::OpIR;
use crate::wasm_abi::TAG_EXCEPTION_INDEX;
use crate::wasm_binary::Function;
use std::borrow::Cow;
use wasm_encoder::{BlockType, Catch, Instruction, ValType};

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
    if async_work_poll {
        super::super::super::pending_work::emit_poll(
            func,
            context.locals,
            context.import_ids,
            context.reloc_enabled,
            false,
        );
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::I64Ne);
    } else {
        super::super::super::pending_work::emit_exception_pending_test(
            func,
            super::super::super::pending_work::thread_record_local(context.locals),
            context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
            context.reloc_enabled,
        );
    }
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
    use crate::wasm_binary::emit_call;
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
        let mut locals = WasmFrameLocals::default();
        let mut local_types = Vec::new();
        let mut local_count = 1;
        for kind in [
            crate::wasm::WasmFrameSyntheticLocal::ThreadRecord,
            crate::wasm::WasmFrameSyntheticLocal::PendingWorkPtr,
        ] {
            locals.ensure_synthetic(kind, &mut local_types, &mut local_count);
        }
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
        let mut body = Function::new_with_locals_types(local_types);
        let mut abis = crate::wasm_binary::WasmExecutionCallAbis::default();
        abis.register_attached(0, &[]);
        abis.register_attached(1, &[ValType::I64]);
        body.attach_context(1, std::rc::Rc::new(abis));
        body.instruction(&Instruction::I32Const(256));
        body.instruction(&Instruction::LocalSet(1));
        body.instruction(&Instruction::I32Const(512));
        body.instruction(&Instruction::LocalSet(2));
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
            body.instruction(&Instruction::I64Const(4));
            emit_call(&mut body, false, import_ids[trace_import]);
            body.instruction(&Instruction::Drop);
            body.instruction(&Instruction::End);
        }
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::Return);
        body.instruction(&Instruction::End);

        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([ValType::I64], [ValType::I64]);
        types
            .ty()
            .function([ValType::I64, ValType::I64], [ValType::I64]);
        module.section(&types);
        let mut imports = ImportSection::new();
        imports.import("molt_runtime", poll_import.name(), EntityType::Function(0));
        imports.import("molt_runtime", trace_import.name(), EntityType::Function(1));
        module.section(&imports);
        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);
        let mut memory = wasm_encoder::MemorySection::new();
        memory.memory(wasm_encoder::MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memory);
        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, 2);
        exports.export("memory", ExportKind::Memory, 0);
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
            let call = operators
                .iter()
                .position(|op| matches!(op, Operator::Call { function_index: 0 }))
                .unwrap();
            assert!(matches!(operators[call - 1], Operator::If { .. }));
            assert!(matches!(operators[call + 1], Operator::Else));
            assert!(
                operators
                    .iter()
                    .any(|op| matches!(op, Operator::BrIf { relative_depth: 0 }))
            );
        }
    }

    #[test]
    fn inline_poll_executes_only_notified_service_and_preserves_pending_exception() {
        use crate::wasm::test_execution::{run_node_test_script, wasm_test_temp_dir};
        let (module, _) = plain_poll_module(false, false);
        Validator::new().validate_all(&module).unwrap();
        let (temp, _cleanup) = wasm_test_temp_dir();
        let path = temp.join("inline-poll.wasm");
        std::fs::write(&path, module).unwrap();
        run_node_test_script(
            &std::path::PathBuf::from("node"),
            r#"
const fs=require('fs'), assert=require('assert/strict');
const module=new WebAssembly.Module(fs.readFileSync(process.argv[1]));
for(const [word,pending,coldError] of [[0,0,0],[8,0,0],[8,0,1],[0,1,0]]) {
  let calls=0; const trace=[];
  const app=new WebAssembly.Instance(module,attachedImports({molt_runtime:{
    async_work_poll_and_exception_pending:()=>{calls++;return BigInt(coldError);},
    trace_set_line:line=>{trace.push(Number(line));return 0n;}
  }})).exports;
  const view=new DataView(app.memory.buffer);
  view.setUint8(256,pending); view.setUint32(512,word,true);
  app.run(executionContext);
  assert.equal(calls,word?1:0,'an empty poll must not enter the runtime');
  assert.deepEqual(trace,(pending||coldError)?[]:[4]);
}
// Positive control: published callback data alone cannot schedule a poll.
// Suppressing its producer bit must keep the cold callback unexecuted.
"#,
            &[&path],
            "inline async-work producer control",
        );
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
// The thread record (local 0, address 256) holds the pending byte the check
// tests inline; the reader confirms a set byte. A set byte whose reader
// repairs it (a stale task slot) is no exception.
for (const [byte, pending] of [[0, false], [1, true], [1, false]]) {
  const events = [];
  let reads = 0;
  const app = new WebAssembly.Instance(module, attachedImports({molt_runtime: {
    exception_pending: () => { reads++; return pending ? 1n : 0n; },
    trace_set_line: line => { events.push(Number(line)); return 0n; },
  }})).exports;
  new Uint8Array(app.memory.buffer)[256] = byte;
  assert.equal(app.run(executionContext), 0n);
  assert.deepEqual(events, pending ? [2, 3] : [1, 3], 'explicit inner cleanup must precede the outer continuation');
  assert.equal(reads, byte, 'the reader runs only when the pending byte is set');
}
"#,
            &[&path],
            "explicit exception cleanup inside try",
        );
    }
}
