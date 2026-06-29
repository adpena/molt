use wasm_encoder::{RefType, ValType};

pub(super) fn encoder_ref_type(ty: wasmparser::RefType) -> RefType {
    if ty.is_func_ref() {
        RefType::FUNCREF
    } else if ty.is_extern_ref() {
        RefType::EXTERNREF
    } else {
        panic!("unsupported imported table reference type in WASM rewrite: {ty:?}");
    }
}

pub(super) fn encoder_val_type(ty: wasmparser::ValType) -> ValType {
    match ty {
        wasmparser::ValType::I32 => ValType::I32,
        wasmparser::ValType::I64 => ValType::I64,
        wasmparser::ValType::F32 => ValType::F32,
        wasmparser::ValType::F64 => ValType::F64,
        wasmparser::ValType::V128 => ValType::V128,
        wasmparser::ValType::Ref(r) => ValType::Ref(encoder_ref_type(r)),
    }
}
