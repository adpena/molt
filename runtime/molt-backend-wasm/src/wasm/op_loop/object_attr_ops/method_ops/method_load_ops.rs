//! CPython's method-form attribute loads: `load_method` (LOAD_ATTR) and
//! `load_super_method` (LOAD_SUPER_ATTR for zero-argument `super()`).

use super::super::super::out_param::WasmOutParam;
use super::super::super::result_sink::store_owned_value;
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{box_int, stable_ic_site_id};
use crate::{FunctionIR, OpIR};
use molt_tir::tir::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};
use wasm_encoder::{Function, Instruction};

/// `load_method(site, receiver, name, len, self_out)` or
/// `load_super_method(site, class, first_arg, name, len, self_out)`: the entry
/// returns the callable and writes `self_or_null` (the receiver or the null
/// sentinel) through the out-parameter. Both words are owned; a discarded one
/// is released, which is a no-op for the sentinel.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_method_load_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
    scratch_base: u32,
) -> bool {
    let (import, operand_count) = match op.kind.as_str() {
        "load_method" => (WasmRuntimeImport::LoadMethod, 1),
        "load_super_method" => (WasmRuntimeImport::LoadSuperMethod, 2),
        _ => return false,
    };
    let args = op
        .args
        .as_ref()
        .unwrap_or_else(|| panic!("{} requires operands", op.kind));
    assert_eq!(args.len(), operand_count, "{} operands", op.kind);
    let name = op
        .s_value
        .as_ref()
        .unwrap_or_else(|| panic!("{} has no attribute name", op.kind));
    let bytes = name.as_bytes();
    let data = backend.add_data_segment(reloc_enabled, bytes);
    let source_op_idx = op.required_source_op_index(op_idx, op.kind.as_str());
    let site_bits = box_int(stable_ic_site_id(
        func_ir.name.as_str(),
        source_op_idx,
        op.kind.as_str(),
    ));
    let self_out = WasmOutParam::at(scratch_base);
    func.instruction(&Instruction::I64Const(site_bits));
    for operand in args {
        func.instruction(&Instruction::LocalGet(locals[operand]));
    }
    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::I64Const(bytes.len() as i64));
    self_out.push_address(func);
    emit_call(func, reloc_enabled, import_ids[import]);

    let mut results = [None, None];
    visit_simple_ir_results(op, |result| match result.field {
        SimpleIrResultField::Var => results[0] = result.name,
        SimpleIrResultField::Out => results[1] = result.name,
        SimpleIrResultField::Arg(_) => unreachable!("a method load has fixed result roles"),
    });
    let [callable, self_or_null] = results;
    store_owned_value(
        func,
        locals.bound_result_slot(callable),
        import_ids,
        reloc_enabled,
    );
    self_out.load(func);
    store_owned_value(
        func,
        locals.bound_result_slot(self_or_null),
        import_ids,
        reloc_enabled,
    );
    true
}
