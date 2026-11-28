use crate::SimpleIR;
use std::collections::BTreeMap;

pub fn apply_profile_order(ir: &mut SimpleIR) {
    apply_profile_order_with(ir, |_| {});
}

pub fn apply_profile_order_with(ir: &mut SimpleIR, permute: impl FnOnce(&[usize])) {
    let Some(profile) = ir.profile.as_ref() else {
        return;
    };
    if profile.hot_functions.is_empty() {
        return;
    }
    // The first function is semantic entry identity, not a layout candidate.
    // Native and WASM run reachability after this shared ordering pass.
    if ir.functions.is_empty() {
        return;
    }
    // Profile ranks are keyed by linker symbol: the profile is an external
    // artifact of a previous build.
    let mut ranks = BTreeMap::new();
    for (idx, name) in profile.hot_functions.iter().enumerate() {
        ranks.entry(name.as_str()).or_insert(idx);
    }
    // Stable sorting preserves source order at equal/unranked priority. Rows
    // move, so every callee is rewritten to its row's new id (I4).
    let mut order: Vec<usize> = (1..ir.functions.len()).collect();
    order.sort_by_key(|&row| {
        ranks
            .get(ir.functions[row].name.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    order.insert(0, 0);
    permute(&order);
    let mut map = vec![None; order.len()];
    for (position, &row) in order.iter().enumerate() {
        map[row] = Some(molt_ir::FunctionId::from_index(position));
    }
    let mut rows: Vec<Option<crate::FunctionIR>> = std::mem::take(&mut ir.functions)
        .into_iter()
        .map(Some)
        .collect();
    ir.functions = order
        .iter()
        .map(|&row| rows[row].take().expect("each row moves once"))
        .collect();
    molt_ir::function_table::remap_function_ids(&mut ir.functions, &map);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionIR, OpIR, PgoProfileIR};

    fn program(names: &[&str], hot: &[&str]) -> SimpleIR {
        SimpleIR {
            functions: names
                .iter()
                .map(|name| FunctionIR {
                    return_abi: molt_ir::FunctionReturnAbi::Void,
                    name: (*name).into(),
                    ..FunctionIR::default()
                })
                .collect(),
            profile: Some(PgoProfileIR {
                hot_functions: hot.iter().map(|name| (*name).into()).collect(),
                ..PgoProfileIR::default()
            }),
        }
    }

    #[test]
    fn profile_order_preserves_entry_stable_ties_and_first_duplicate_rank() {
        for hot in [
            vec!["hot", "entry", "hot", "missing"],
            vec!["missing", "hot"],
        ] {
            let mut ir = program(&["entry", "cold_a", "hot", "cold_b"], &hot);
            apply_profile_order(&mut ir);
            let names: Vec<_> = ir
                .functions
                .iter()
                .map(|function| function.name.as_str())
                .collect();
            assert_eq!(names, ["entry", "hot", "cold_a", "cold_b"]);
            let once = names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>();
            apply_profile_order(&mut ir);
            assert_eq!(
                ir.functions
                    .iter()
                    .map(|function| function.name.clone())
                    .collect::<Vec<_>>(),
                once
            );
        }
        for names in [vec![], vec!["entry"]] {
            let mut ir = program(&names, &["entry", "missing"]);
            apply_profile_order(&mut ir);
            assert_eq!(
                ir.functions
                    .iter()
                    .map(|function| function.name.as_str())
                    .collect::<Vec<_>>(),
                names
            );
        }
    }

    #[test]
    fn profile_heat_cannot_replace_the_program_reachability_root() {
        for hot in [vec!["dead_hot"], vec!["live_callee", "dead_hot"], vec![]] {
            let mut ir = program(&["application_entry", "dead_hot", "live_callee"], &hot);
            ir.functions[0].ops.push(OpIR {
                kind: molt_ir::OpKind::CallInternal,
                s_value: Some("live_callee".into()),
                ..OpIR::new(molt_ir::OpKind::CallInternal)
            });
            ir.resolve_function_references().unwrap();
            apply_profile_order(&mut ir);
            super::super::dead_functions::eliminate_dead_functions(&mut ir);
            assert_eq!(
                ir.functions
                    .iter()
                    .map(|function| function.name.as_str())
                    .collect::<Vec<_>>(),
                ["application_entry", "live_callee"]
            );
        }
    }
}
