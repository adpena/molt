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
    match op.kind.as_str() {
        "try_start" => emit_try_start(context, func),
        "try_end" => emit_try_end(context, func),
        "check_exception" | "async_work_poll" => emit_check_exception(context, func, op),
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
    let async_work_poll = op.kind == "async_work_poll";
    if !async_work_poll
        && (context.native_eh_enabled
            || context
                .exception_handler_region_indices
                .contains(&context.op_idx))
    {
        return;
    }

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
    let depth = if let Some(&try_index) = context.try_stack.last() {
        context.control_stack.len().saturating_sub(1 + try_index) as u32
    } else {
        let target = op
            .value
            .unwrap_or_else(|| panic!("{} missing function exception label", op.kind));
        super::branches::label_branch_depth(context, target, op.kind.as_str())
    };
    func.instruction(&Instruction::BrIf(depth));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::representation_plan::ScalarRepresentationPlan;
    use crate::wasm::WasmFrameLocals;
    use crate::wasm_import_tracking::TrackedImportIds;
    use crate::wasm_values::ConstantCache;
    use std::collections::{BTreeMap, BTreeSet};
    use wasm_encoder::{
        CodeSection, EntityType, FunctionSection, ImportSection, Module, TypeSection,
    };
    use wasmparser::{Operator, Parser, Payload, Validator};

    fn plain_poll_module(native_eh_enabled: bool) -> (Vec<u8>, TrackedImportIds) {
        let poll_import =
            crate::wasm_abi_generated::WasmRuntimeImport::AsyncWorkPollAndExceptionPending;
        let import_ids = TrackedImportIds::new(BTreeMap::from([(poll_import, 0)]));
        let func_ir = crate::FunctionIR {
            name: "plain_poll".into(),
            ..Default::default()
        };
        let locals = WasmFrameLocals::default();
        let const_cache = ConstantCache::default();
        let scalar_plan = ScalarRepresentationPlan::default();
        let exception_regions = BTreeSet::new();
        let mut control_stack = vec![ControlKind::Block];
        let mut try_stack = Vec::new();
        let mut label_stack = vec![41];
        let mut label_depths = BTreeMap::from([(41, 0)]);
        let op = OpIR {
            kind: "async_work_poll".into(),
            value: Some(41),
            ..Default::default()
        };
        let mut body = Function::new([]);
        body.instruction(&Instruction::Block(BlockType::Empty));
        let mut context = ControlOpContext {
            func_ir: &func_ir,
            import_ids: &import_ids,
            locals: &locals,
            const_cache: &const_cache,
            scalar_plan: &scalar_plan,
            exception_handler_region_indices: &exception_regions,
            control_stack: &mut control_stack,
            try_stack: &mut try_stack,
            label_stack: &mut label_stack,
            label_depths: &mut label_depths,
            reloc_enabled: false,
            native_eh_enabled,
            arena_local: None,
            op_idx: 0,
        };
        assert!(emit_exception_control_op(&mut context, &mut body, &op));
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::Return);
        body.instruction(&Instruction::End);

        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I64]);
        module.section(&types);
        let mut imports = ImportSection::new();
        imports.import("molt_runtime", "async_poll", EntityType::Function(0));
        module.section(&imports);
        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);
        let mut code = CodeSection::new();
        code.function(&body);
        module.section(&code);
        (module.finish(), import_ids)
    }

    #[test]
    fn plain_poll_outside_try_calls_observer_and_branches_to_function_exit() {
        for native_eh_enabled in [false, true] {
            let (module, import_ids) = plain_poll_module(native_eh_enabled);
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
}
