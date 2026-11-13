//! CPython's method-form attribute loads: `load_method` (LOAD_ATTR) and
//! `load_super_method` (LOAD_SUPER_ATTR for zero-argument `super()`).

use super::super::super::out_param::WasmOutParam;
use super::super::super::result_sink::store_owned_value;
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{box_int, stable_ic_site_id};
use crate::{FunctionIR, OpIR};
use molt_tir::tir::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};
use wasm_encoder::Instruction;

/// A method site's load body `(record, receiver, self_out)`, whose miss is
/// `molt_method_site_load`, or `load_super_method(site, class, first_arg,
/// name, len, self_out)`: the load returns the callable and writes
/// `self_or_null` (the receiver or the null sentinel) through the
/// out-parameter. Both words are owned; a discarded one is released, which is
/// a no-op for the sentinel.
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
    let operand_count = match op.kind {
        molt_ir::OpKind::LoadMethod => 1,
        molt_ir::OpKind::LoadSuperMethod => 2,
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
    let self_out = WasmOutParam::at(scratch_base);
    if op.kind == molt_ir::OpKind::LoadMethod {
        let site = op
            .method_site
            .unwrap_or_else(|| panic!("a method load in {} is a method site", func_ir.name));
        let record = backend.method_site_record(reloc_enabled, func_index, site, Some(bytes));
        backend.emit_data_ptr(reloc_enabled, func_index, func, record);
        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
        self_out.push_address(func);
        func.instruction(&Instruction::LocalGet(locals.refcount().admission));
        emit_call(func, reloc_enabled, import_ids.method_site_bodies().load());
    } else {
        let data = backend.add_data_segment(reloc_enabled, bytes);
        let source_op_idx = op.required_source_op_index(op_idx, op.kind);
        let site_bits = box_int(stable_ic_site_id(
            func_ir.name.as_str(),
            source_op_idx,
            op.kind.as_str(),
        ));
        func.instruction(&Instruction::I64Const(site_bits));
        for operand in args {
            func.instruction(&Instruction::LocalGet(locals[operand]));
        }
        backend.emit_data_ptr(reloc_enabled, func_index, func, data);
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::I64Const(bytes.len() as i64));
        self_out.push_address(func);
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::LoadSuperMethod],
        );
    }

    let mut results = [None, None];
    visit_simple_ir_results(op, |result| match result.field {
        SimpleIrResultField::Var => results[0] = result.name,
        SimpleIrResultField::Out => results[1] = result.name,
        SimpleIrResultField::Arg(_) => unreachable!("a method load has fixed result roles"),
    });
    let [callable, self_or_null] = results;
    // Read the scratch word before any release: releasing a discarded
    // callable can run `__del__`, whose compiled code may reuse the word.
    self_out.load(func);
    store_owned_value(
        func,
        locals.bound_result_slot(self_or_null),
        import_ids,
        reloc_enabled,
        locals.refcount(),
    );
    store_owned_value(
        func,
        locals.bound_result_slot(callable),
        import_ids,
        reloc_enabled,
        locals.refcount(),
    );
    true
}
