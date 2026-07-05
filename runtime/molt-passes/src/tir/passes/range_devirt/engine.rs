use crate::tir::function::TirFunction;

use super::super::PassStats;
use super::candidate::find_candidates;
use super::transform::apply_transform;

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "range_devirt",
        ..Default::default()
    };

    let candidates = find_candidates(func);
    if candidates.is_empty() {
        return stats;
    }

    for candidate in candidates {
        apply_transform(func, &candidate, &mut stats);
    }

    stats
}
