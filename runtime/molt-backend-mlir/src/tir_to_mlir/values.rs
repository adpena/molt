use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    ir::{Type, Value},
};
use molt_backend::tir::{types::TirType, values::ValueId};

pub(super) struct ValueMap<'c, 'a> {
    values: HashMap<ValueId, Value<'c, 'a>>,
    types: HashMap<ValueId, TirType>,
}

impl<'c, 'a> ValueMap<'c, 'a> {
    pub(super) fn new(types: &HashMap<ValueId, TirType>) -> Self {
        Self {
            values: HashMap::new(),
            types: types.clone(),
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
        match self.type_of(id) {
            Some(TirType::F64) => true,
            Some(_) => false,
            None => value.r#type() == Type::float64(ctx),
        }
    }
}

pub(super) fn resolve_value<'c, 'a>(
    value_map: &ValueMap<'c, 'a>,
    vid: ValueId,
) -> Result<Value<'c, 'a>, String> {
    value_map
        .get(&vid)
        .copied()
        .ok_or_else(|| format!("TIR ValueId %{} not found in MLIR value map", vid.0))
}
