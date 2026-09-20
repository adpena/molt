use super::super::result_sink::{discard_runtime_result, store_runtime_result};
use super::RuntimeServiceOpContext;
use crate::OpIR;
use crate::wasm_abi::TAG_EXCEPTION_INDEX;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_exception_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let const_cache = context.const_cache;
    let reloc_enabled = context.reloc_enabled;
    let native_eh_enabled = context.native_eh_enabled;

    match op.kind.as_str() {
        "exception_push" => {
            if native_eh_enabled {
                // Native EH: no-op; WASM runtime manages handler stack.
                const_cache.emit_none(func);
            } else {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPush],
                );
            }
            store_runtime_result(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
                crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPush,
            );
        }
        "exception_pop" => {
            if native_eh_enabled {
                const_cache.emit_none(func);
            } else {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPop],
                );
            }
            store_runtime_result(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
                crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPop,
            );
        }
        "raise" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            if native_eh_enabled {
                // Native EH: call host raise to register the exception
                // (traceback, __context__), then throw via WASM EH.
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Raise],
                );
                discard_runtime_result(
                    func,
                    import_ids,
                    reloc_enabled,
                    crate::wasm_abi_generated::WasmRuntimeImport::Raise,
                );
                if context.raise_exits_function {
                    context
                        .frame
                        .emit_const_anchor_releases(func, import_ids, reloc_enabled);
                }
                func.instruction(&Instruction::LocalGet(exc));
                func.instruction(&Instruction::Throw(TAG_EXCEPTION_INDEX));
            } else {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Raise],
                );
                store_runtime_result(
                    func,
                    op,
                    locals,
                    import_ids,
                    reloc_enabled,
                    crate::wasm_abi_generated::WasmRuntimeImport::Raise,
                );
            }
        }
        _ => return false,
    }
    true
}
