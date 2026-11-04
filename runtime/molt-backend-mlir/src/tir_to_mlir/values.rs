use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    ir::{Type, Value, ValueLike},
};
use molt_backend::{
    representation_plan::ProducerCarrierPlan,
    tir::{function::TirFunction, types::TirType, values::ValueId},
};

pub(super) struct ValueMap<'c, 'a> {
    values: HashMap<ValueId, Value<'c, 'a>>,
    types: HashMap<ValueId, TirType>,
    pub(super) return_type: TirType,
}

impl<'c, 'a> ValueMap<'c, 'a> {
    pub(super) fn new(func: &TirFunction, carriers: &ProducerCarrierPlan) -> Self {
        let types = func
            .blocks
            .values()
            .flat_map(|block| {
                block.args.iter().map(|arg| (arg.id, &arg.ty)).chain(
                    block
                        .ops
                        .iter()
                        .flat_map(|op| &op.results)
                        .map(|&id| (id, func.value_types.get(&id).unwrap_or(&TirType::DynBox))),
                )
            })
            .map(|(id, declared)| (id, carriers.effective_value_type(id, declared)))
            .collect();
        Self {
            values: HashMap::new(),
            types,
            return_type: carriers.effective_return_type(func),
        }
    }

    pub(super) fn insert(&mut self, id: ValueId, value: Value<'c, 'a>) {
        self.values.insert(id, value);
    }

    pub(super) fn get(&self, id: &ValueId) -> Option<&Value<'c, 'a>> {
        self.values.get(id)
    }

    pub(super) fn type_of(&self, id: ValueId) -> Option<&TirType> {
        self.types.get(&id)
    }

    pub(super) fn is_float_value(
        &self,
        id: ValueId,
        value: Value<'c, '_>,
        ctx: &'c MlirContext,
    ) -> bool {
        self.type_of(id) == Some(&TirType::F64) && value.r#type() == Type::float64(ctx)
    }
}

pub(super) fn resolve_value<'c, 'a>(
    value_map: &ValueMap<'c, 'a>,
    vid: ValueId,
) -> Result<Value<'c, 'a>, String> {
    if !matches!(
        value_map.type_of(vid),
        Some(TirType::I64 | TirType::F64 | TirType::Bool)
    ) {
        return Err(format!(
            "value %{} has no producer-proven machine carrier (annotation-only or boxed value); MLIR has no Molt runtime provider",
            vid.0
        ));
    }
    value_map
        .get(&vid)
        .copied()
        .ok_or_else(|| format!("TIR ValueId %{} not found in MLIR value map", vid.0))
}
