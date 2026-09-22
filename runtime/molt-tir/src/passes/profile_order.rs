use crate::SimpleIR;
use std::collections::BTreeMap;

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn apply_profile_order(ir: &mut SimpleIR) {
    let Some(profile) = ir.profile.as_ref() else {
        return;
    };
    if profile.hot_functions.is_empty() {
        return;
    }
    // The first function is semantic entry identity, not a layout candidate.
    // Native and WASM run reachability after this shared ordering pass.
    let Some((_, functions)) = ir.functions.split_first_mut() else {
        return;
    };
    let mut ranks = BTreeMap::new();
    for (idx, name) in profile.hot_functions.iter().enumerate() {
        ranks.entry(name.as_str()).or_insert(idx);
    }
    // Stable sorting already preserves source order at equal/unranked priority;
    // a second name-to-original-index map is unnecessary.
    functions.sort_by_key(|function| {
        ranks
            .get(function.name.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
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
                kind: "call_internal".into(),
                s_value: Some("live_callee".into()),
                ..OpIR::default()
            });
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
