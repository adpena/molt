//! Shared construction of complete TIR fixture signatures.
use molt_ir::ParameterCustody;
use molt_ir::tir::{
    function::TirFunction,
    types::TirType,
    values::{TirValue, ValueId},
};

/// Add an annotated input without losing any existing parameter's custody.
/// An annotation is not a proof of an exact scalar runtime carrier.
pub(crate) fn append_parameter(func: &mut TirFunction, ty: TirType) -> ValueId {
    assert_eq!(func.param_names.len(), func.param_types.len());
    assert_eq!(
        func.blocks[&func.entry_block].args.len(),
        func.param_types.len()
    );
    let mut custody: Vec<_> = (0..func.param_types.len())
        .map(|position| func.parameter_custody(position))
        .collect();
    let value = func.fresh_value();
    func.param_names
        .push(format!("__fixture_input_{}", value.0));
    func.param_types.push(ty.clone());
    func.value_types.insert(value, ty.clone());
    func.blocks
        .get_mut(&func.entry_block)
        .unwrap()
        .args
        .push(TirValue { id: value, ty });
    custody.push(ParameterCustody::Borrowed);
    func.set_parameter_custody(&custody);
    value
}
