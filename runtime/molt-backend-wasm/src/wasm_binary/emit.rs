use wasm_encoder::{ConstExpr, Function, Instruction};

pub(crate) fn encode_u32_leb128_padded(mut value: u32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn encode_i32_sleb128_padded(mut value: i32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

pub(crate) fn emit_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if func_index == u32::MAX {
        // Sentinel: this import was stripped in pure profile mode.
        // Trap if the code path is actually reached at runtime.
        func.instruction(&Instruction::Unreachable);
        return;
    }
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x10);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::Call(func_index));
    }
}

/// Emit a simple N-arg import call: push args, call, store result.
pub(crate) fn emit_simple_call(
    func: &mut Function,
    reloc_enabled: bool,
    import_id: u32,
    arg_locals: &[u32],
    out_local: u32,
) {
    for &arg in arg_locals {
        func.instruction(&Instruction::LocalGet(arg));
    }
    emit_call(func, reloc_enabled, import_id);
    func.instruction(&Instruction::LocalSet(out_local));
}

/// Emit a `return_call` instruction (WASM tail calls proposal).
/// The callee's return value becomes the caller's return value without growing the stack.
pub(crate) fn emit_return_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if func_index == u32::MAX {
        // Sentinel: this import was stripped in pure profile mode.
        func.instruction(&Instruction::Unreachable);
        return;
    }
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x12); // return_call opcode
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::ReturnCall(func_index));
    }
}

pub(crate) fn emit_call_indirect(func: &mut Function, reloc_enabled: bool, ty: u32, table: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(11);
        bytes.push(0x11);
        encode_u32_leb128_padded(ty, &mut bytes);
        encode_u32_leb128_padded(table, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::CallIndirect {
            type_index: ty,
            table_index: table,
        });
    }
}

pub(crate) fn emit_i32_const(func: &mut Function, reloc_enabled: bool, value: i32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x41);
        encode_i32_sleb128_padded(value, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::I32Const(value));
    }
}

pub(crate) fn emit_ref_func(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0xD2);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::RefFunc(func_index));
    }
}

fn emit_table_index_i32(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_i32_const(func, reloc_enabled, table_index as i32);
}

pub(crate) fn emit_table_index_i64(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_table_index_i32(func, reloc_enabled, table_index);
    func.instruction(&Instruction::I64ExtendI32U);
}

pub(crate) fn const_expr_i32_const_padded(value: i32) -> ConstExpr {
    let mut bytes = Vec::with_capacity(6);
    bytes.push(0x41);
    encode_i32_sleb128_padded(value, &mut bytes);
    ConstExpr::raw(bytes)
}
