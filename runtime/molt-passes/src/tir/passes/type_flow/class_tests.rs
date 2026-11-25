//! Exact positive Python type tests over an init-once class publication.

use molt_ir::deterministic_hash::HashMap;
use molt_ir::{ClassId, OpKind, ParameterCustody};

use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::value_identity::{build_copy_map, resolve_copy};
use crate::tir::types::TirType;

pub(super) fn fold_exact_isinstance(func: &mut TirFunction) -> bool {
    let ops = || func.blocks.values().flat_map(|block| &block.ops);
    if !ops().any(|op| op.origin == Some(OpKind::Isinstance)) {
        return false;
    }
    let classes: HashMap<_, _> = ops()
        .filter_map(|op| {
            if matches!(op.opcode, OpCode::ModuleGetGlobal | OpCode::StaticCellLoad)
                && let [result] = op.results.as_slice()
                && let Some(AttrValue::Str(definition)) = op.attrs.get("class_definition")
            {
                return Some((*result, ClassId::of_definition(definition)));
            }
            None
        })
        .collect();
    if classes.is_empty() {
        return false;
    }
    let exact = crate::tir::type_refine::extract_exact_class_map(func);
    if exact.is_empty() {
        return false;
    }
    let copies = build_copy_map(func);
    let mut changed = false;
    for op in func.blocks.values_mut().flat_map(|block| &mut block.ops) {
        if op.origin == Some(OpKind::Isinstance)
            && let [instance, classinfo] = op.operands.as_slice()
            && let Some(instance_class) = exact.get(instance)
            && let Some(&class) = classes.get(&resolve_copy(&copies, *classinfo))
            && instance_class == &TirType::Class(class)
            && (0..2).all(|index| op.operand_custody(index) == ParameterCustody::Borrowed)
        {
            // Python answers an exact match before consulting the metaclass or
            // reading __class__. Keep the producers and their custody intact.
            let mut constant =
                TirOp::new(OpCode::ConstBool, vec![], std::mem::take(&mut op.results));
            constant.attrs.insert("value".into(), AttrValue::Bool(true));
            constant.source_span = op.source_span;
            *op = constant;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::lower_from_simple::lower_to_tir;

    #[test]
    fn only_the_exact_published_class_folds() {
        let mut function = lower_to_tir(
            &serde_json::from_value(serde_json::json!({
                "name": "check", "params": ["obj", "dynamic"], "return_abi": "value",
                "ops": [
                    {"kind":"static_cell_load","out":"a","class_definition":"class:m:A",
                     "global_slot":{"module":"m","index":0,"cell":true,"init_once":true}},
                    {"kind":"static_cell_load","out":"b","class_definition":"class:m:B",
                     "global_slot":{"module":"m","index":1,"cell":true,"init_once":true}},
                    {"kind":"isinstance","args":["obj","a"],"out":"exact"},
                    {"kind":"isinstance","args":["obj","b"],"out":"other"},
                    {"kind":"isinstance","args":["obj","dynamic"],"out":"unknown"},
                    {"kind":"ret","args":["exact"]}
                ]
            }))
            .unwrap(),
        );
        assert!(!fold_exact_isinstance(&mut function));
        function.exact_param_types = vec![
            TirType::Class(ClassId::of_definition("class:m:A")),
            TirType::DynBox,
        ];
        assert!(fold_exact_isinstance(&mut function));
        let ops = &function.blocks[&function.entry_block].ops;
        assert_eq!(
            ops.iter()
                .filter(|op| op.origin == Some(OpKind::Isinstance))
                .count(),
            2
        );
        assert!(ops.iter().any(|op| op.opcode == OpCode::ConstBool
            && op.attrs.get("value") == Some(&AttrValue::Bool(true))));
    }
}
