//! Entry-block signature and semantic-type/representation acceptance checks.
//!
//! Moved move-only from the monolithic `verify_lir.rs`; no logic changes.

use super::super::lir::{LirFunction, LirRepr, LirValue};
use super::super::types::TirType;
use super::LirVerifyError;

pub(super) fn verify_entry_block_signature(func: &LirFunction, errors: &mut Vec<LirVerifyError>) {
    if func.param_names.len() != func.param_types.len() {
        errors.push(LirVerifyError::func(format!(
            "function {} declares {} param names but {} param types",
            func.name,
            func.param_names.len(),
            func.param_types.len()
        )));
    }

    let Some(entry_block) = func.blocks.get(&func.entry_block) else {
        return;
    };

    if entry_block.args.len() != func.param_types.len() {
        errors.push(LirVerifyError::block(
            func.entry_block,
            format!(
                "entry block ^{} expects {} params but function signature declares {}",
                func.entry_block,
                entry_block.args.len(),
                func.param_types.len()
            ),
        ));
        return;
    }

    for (idx, (actual, expected_ty)) in entry_block
        .args
        .iter()
        .zip(func.param_types.iter())
        .enumerate()
    {
        let expected_repr = LirRepr::for_type(expected_ty);
        if !signature_value_accepts_type(expected_ty, actual) {
            errors.push(LirVerifyError::block(
                func.entry_block,
                format!(
                    "entry block ^{} type mismatch for param {}: expected {:?}, found {:?}",
                    func.entry_block, idx, expected_ty, actual.ty
                ),
            ));
        }
        if !signature_value_accepts_repr(expected_ty, actual) {
            errors.push(LirVerifyError::block(
                func.entry_block,
                format!(
                    "entry block ^{} representation mismatch for param {}: expected {:?}, found {:?}",
                    func.entry_block, idx, expected_repr, actual.repr
                ),
            ));
        }
    }
}

pub(super) fn signature_value_accepts_type(expected_ty: &TirType, actual: &LirValue) -> bool {
    signature_type_accepts_type(expected_ty, &actual.ty)
}

pub(super) fn signature_type_accepts_type(expected_ty: &TirType, actual_ty: &TirType) -> bool {
    if expected_ty == actual_ty {
        return true;
    }
    if LirRepr::for_type(expected_ty) == LirRepr::DynBox && matches!(actual_ty, TirType::DynBox) {
        return true;
    }
    match expected_ty {
        TirType::DynBox => true,
        TirType::Union(members) => members
            .iter()
            .any(|member| signature_type_accepts_type(member, actual_ty)),
        _ => actual_ty == expected_ty,
    }
}

pub(super) fn signature_value_accepts_repr(expected_ty: &TirType, actual: &LirValue) -> bool {
    let expected_repr = LirRepr::for_type(expected_ty);
    if actual.repr == expected_repr {
        return true;
    }
    if matches!(expected_ty, TirType::DynBox)
        && actual.repr == LirRepr::Ref64
        && matches!(actual.ty, TirType::UserClass(_))
    {
        return true;
    }
    matches!(
        (expected_ty, &actual.ty, actual.repr),
        (TirType::UserClass(expected), TirType::UserClass(actual), LirRepr::DynBox)
        | (TirType::UserClass(expected), TirType::UserClass(actual), LirRepr::Ref64)
            if expected == actual
    )
}
