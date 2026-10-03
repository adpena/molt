//! Identity-based C3 linearization shared by managed classes and native types.
//! Inputs are immutable snapshots; no class registry or retained cache is involved.

use std::collections::HashMap;

/// Merge base MROs followed by the ordered direct bases. Tail occurrence counts
/// avoid rescanning every remaining tail or removing vector fronts. Hashing only
/// answers membership; candidate order always follows the input sequences.
pub fn c3_merge<T: Copy + Eq + std::hash::Hash>(seqs: &[Vec<T>]) -> Option<Vec<T>> {
    let mut result = Vec::new();
    let mut heads = vec![0usize; seqs.len()];
    let mut tail_counts: HashMap<T, usize> = HashMap::new();
    for seq in seqs {
        for &value in seq.iter().skip(1) {
            *tail_counts.entry(value).or_insert(0) += 1;
        }
    }
    loop {
        let mut remaining = 0usize;
        for (idx, seq) in seqs.iter().enumerate() {
            if heads[idx] < seq.len() {
                remaining += 1;
            }
        }
        if remaining == 0 {
            return Some(result);
        }
        let mut candidate = None;
        'outer: for (seq_idx, seq) in seqs.iter().enumerate() {
            let head_idx = heads[seq_idx];
            if head_idx >= seq.len() {
                continue;
            }
            let head = seq[head_idx];
            if tail_counts.get(&head).copied().unwrap_or(0) == 0 {
                candidate = Some(head);
                break 'outer;
            }
        }
        let cand = candidate?;
        result.push(cand);
        for (idx, seq) in seqs.iter().enumerate() {
            let head_idx = heads[idx];
            if head_idx < seq.len() && seq[head_idx] == cand {
                heads[idx] += 1;
                let next_head_idx = heads[idx];
                if next_head_idx < seq.len() {
                    let next_head = seq[next_head_idx];
                    if let Some(count) = tail_counts.get_mut(&next_head) {
                        if *count <= 1 {
                            tail_counts.remove(&next_head);
                        } else {
                            *count -= 1;
                        }
                    }
                }
            }
        }
    }
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutConflict;

/// Select the dominant physical layout while retaining the first direct base
/// for equal solid owners. Representations supply owners and the actual subtype
/// relation; the dominance rule is shared across both construction paths.
pub fn dominant_layout_base<T: Copy, I: Copy>(
    selected: Option<(T, I)>,
    candidate: (T, I),
    mut is_subtype: impl FnMut(I, I) -> bool,
) -> Result<(T, I), LayoutConflict> {
    let Some(winner) = selected else { return Ok(candidate); };
    if is_subtype(winner.1, candidate.1) { Ok(winner) }
    else if is_subtype(candidate.1, winner.1) { Ok(candidate) }
    else { Err(LayoutConflict) }
}

#[cfg(test)]
mod tests {
    use super::{c3_merge, dominant_layout_base, LayoutConflict};

    #[test]
    fn preserves_diamond_precedence_and_rejects_inconsistent_orders() {
        assert_eq!(c3_merge(&[vec![1, 3], vec![2, 3], vec![1, 2]]), Some(vec![1, 2, 3]));
        assert_eq!(c3_merge(&[vec![1, 2, 3], vec![2, 1, 3], vec![1, 2]]), None);
        assert_eq!(c3_merge(&[vec![1, 3], vec![1, 3], vec![1, 1]]), None);
        assert_eq!(c3_merge::<u64>(&[]), Some(vec![]));
    }
    #[test]
    fn layout_dominance_retains_first_equal_owner_and_rejects_conflicts() {
        assert_eq!(dominant_layout_base(Some((10, 1)), (20, 1), |a, b| a == b), Ok((10, 1)));
        assert_eq!(dominant_layout_base(Some((10, 1)), (20, 2), |a, b| a == b), Err(LayoutConflict));
        assert_eq!(dominant_layout_base(Some((10, 1)), (20, 2), |a, b| a >= b), Ok((20, 2)));
    }

}
