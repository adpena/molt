use super::{WasmFunctionLoweringPlan, is_production_lir_wasm_fast_path_name};
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm::context::CompileFuncContext;
use wasm_encoder::Function;

pub(in crate::wasm) fn try_emit_planned_lir_fast_body(
    backend: &mut WasmBackend,
    func_ir: &FunctionIR,
    func_index: u32,
    reloc_enabled: bool,
    ctx: &CompileFuncContext<'_>,
) -> bool {
    if func_ir.is_extern || !is_production_lir_wasm_fast_path_name(&func_ir.name) {
        return false;
    }
    let Some(plan) = ctx.lir_lowering_plans.get(&func_ir.name) else {
        panic!(
            "missing WASM LIR lowering plan for production fast-path function {}",
            func_ir.name
        );
    };
    match plan {
        WasmFunctionLoweringPlan::LirFast(lir_output) => {
            if std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref()
                == Some(func_ir.name.as_str())
            {
                eprintln!(
                    "WASM_SIG_FUNC fast_path name={} lir_param_types={:?} lir_result_types={:?}",
                    func_ir.name, lir_output.param_types, lir_output.result_types
                );
            }
            let mut func = Function::new_with_locals_types(lir_output.locals.clone());
            lir_output.emit_into(
                &func_ir.name,
                backend,
                func_index,
                reloc_enabled,
                ctx.const_str_scratch_segment,
                |name| ctx.import_ids[name],
                &mut func,
            );
            backend.codes.function(&func);
            true
        }
        WasmFunctionLoweringPlan::Generic { reason } => {
            if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1")
                || std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref()
                    == Some(func_ir.name.as_str())
            {
                eprintln!(
                    "[molt-wasm-lir-fast] function={} generic_reason={}",
                    func_ir.name,
                    reason.diagnostic_name()
                );
            }
            false
        }
    }
}
