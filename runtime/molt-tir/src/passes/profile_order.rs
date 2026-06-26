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
    let mut ranks: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, name) in profile.hot_functions.iter().enumerate() {
        ranks.entry(name.clone()).or_insert(idx);
    }
    let mut original: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, func) in ir.functions.iter().enumerate() {
        original.entry(func.name.clone()).or_insert(idx);
    }
    ir.functions.sort_by(|left, right| {
        let left_rank = ranks.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_rank = ranks.get(&right.name).copied().unwrap_or(usize::MAX);
        if left_rank != right_rank {
            return left_rank.cmp(&right_rank);
        }
        let left_idx = original.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_idx = original.get(&right.name).copied().unwrap_or(usize::MAX);
        left_idx
            .cmp(&right_idx)
            .then_with(|| left.name.cmp(&right.name))
    });
}
