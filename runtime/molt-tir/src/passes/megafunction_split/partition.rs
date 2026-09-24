/// Pack complete statements against a target and a 2x hard source-op limit.
/// `statements` supplies increasing, nonzero boundaries below `op_count`;
/// `is_forbidden` owns control and cleanup admission for a proposed chunk.
pub(super) fn split_boundaries(
    op_count: usize,
    target: usize,
    statements: impl Iterator<Item = usize>,
    mut is_forbidden: impl FnMut(usize, usize) -> bool,
) -> Option<Vec<usize>> {
    if target == 0 {
        return None;
    }
    let hard_limit = target.saturating_mul(2);
    let mut boundaries = vec![0];
    let mut start = 0;
    let mut pending = Vec::new();
    let mut unchecked = 0;
    for end in statements.chain(std::iter::once(op_count)) {
        // Inspect candidates only when extending a complete statement would
        // cross the target. Checking every preceding line repeatedly scans
        // control/live-in facts and turns straight-line planning quadratic in
        // the chunk budget. The latest legal cut packs the current chunk.
        while end - start > target {
            let mut candidate = None;
            while unchecked > 0 {
                unchecked -= 1;
                let boundary = pending[unchecked];
                if !is_forbidden(boundary, start) {
                    candidate = Some((unchecked, boundary));
                    break;
                }
            }
            let Some((index, boundary)) = candidate else {
                break;
            };
            boundaries.push(boundary);
            start = boundary;
            // A previously forbidden later candidate may become safe with a
            // new chunk's input environment. Retain and revalidate it.
            pending.drain(..=index);
            unchecked = pending.len();
        }
        if end - start > hard_limit {
            // No earlier candidate admits a complete chunk within the limit.
            return None;
        }
        if end == op_count {
            boundaries.push(end);
            break;
        }
        if end - start >= target {
            if !is_forbidden(end, start) {
                boundaries.push(end);
                start = end;
                pending.clear();
                unchecked = 0;
            } else {
                // Already rejected for this start; only reconsider after
                // choosing an earlier cut changes the input environment.
                pending.push(end);
            }
        } else {
            pending.push(end);
            unchecked = pending.len();
        }
    }
    (boundaries.len() > 2).then_some(boundaries)
}

#[cfg(test)]
mod tests {
    use super::split_boundaries;

    #[test]
    fn terminal_statement_is_budgeted_before_committing_the_previous_cut() {
        assert_eq!(
            split_boundaries(18, 3, [3, 5, 7, 9, 11, 13].into_iter(), |_, _| false),
            Some(vec![0, 3, 5, 7, 9, 11, 13, 18])
        );
    }

    #[test]
    fn straight_line_admission_cost_tracks_chunks_not_statement_prefixes() {
        let mut admissions = 0;
        let boundaries = split_boundaries(100_000, 2_000, 1..100_000, |_, _| {
            admissions += 1;
            false
        })
        .unwrap();
        assert_eq!(boundaries, (0..=100_000).step_by(2_000).collect::<Vec<_>>());
        assert_eq!(admissions, 49);
    }

    #[test]
    fn pending_candidates_are_revalidated_for_the_changed_chunk_start() {
        let mut queried = Vec::new();
        let boundaries = split_boundaries(11, 4, [2, 3, 6, 7].into_iter(), |end, start| {
            queried.push((end, start));
            matches!((end, start), (3, 0) | (6, 2) | (7, 2))
        });
        assert_eq!(boundaries, Some(vec![0, 2, 3, 7, 11]));
        assert!(queried.contains(&(3, 0)));
        assert!(queried.contains(&(3, 2)));
    }

    #[test]
    fn indivisible_regions_and_invalid_budgets_refuse() {
        assert!(split_boundaries(10, 3, [2].into_iter(), |_, _| false).is_none());
        assert!(split_boundaries(10, 3, 1..10, |_, _| true).is_none());
        assert!(split_boundaries(10, 0, 1..10, |_, _| false).is_none());
        assert!(split_boundaries(10, usize::MAX, 1..10, |_, _| false).is_none());
    }

    #[test]
    fn rejected_candidates_are_not_repeated_for_an_unchanged_start() {
        let mut admissions = 0;
        assert!(
            split_boundaries(100_000, 2_000, 1..100_000, |_, _| {
                admissions += 1;
                true
            })
            .is_none()
        );
        assert_eq!(admissions, 4_000);
    }
}
