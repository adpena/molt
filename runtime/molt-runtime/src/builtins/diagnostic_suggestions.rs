//! Shared CPython diagnostic suggestion policy (v3.13.16 Python/suggestions.c).
//! Work is bounded by the candidate limit and the trimmed UTF-8 byte lengths.

pub(crate) const MAX_CANDIDATE_ITEMS: usize = 750;
const MAX_STRING_SIZE: usize = 40;
const MOVE_COST: usize = 2;

fn substitution_cost(a: u8, b: u8) -> usize {
    if a == b {
        0
    } else if a.eq_ignore_ascii_case(&b) {
        1
    } else {
        MOVE_COST
    }
}

fn edit_cost<'a>(mut a: &'a [u8], mut b: &'a [u8], max_cost: usize) -> usize {
    while !a.is_empty() && !b.is_empty() && a[0] == b[0] {
        a = &a[1..];
        b = &b[1..];
    }
    while !a.is_empty() && !b.is_empty() && a.last() == b.last() {
        a = &a[..a.len() - 1];
        b = &b[..b.len() - 1];
    }
    if a.is_empty() || b.is_empty() {
        return (a.len() + b.len()) * MOVE_COST;
    }
    if a.len() > MAX_STRING_SIZE || b.len() > MAX_STRING_SIZE {
        return max_cost + 1;
    }
    if b.len() < a.len() {
        std::mem::swap(&mut a, &mut b);
    }
    if (b.len() - a.len()) * MOVE_COST > max_cost {
        return max_cost + 1;
    }
    let mut row = [0usize; MAX_STRING_SIZE];
    for (index, slot) in row[..a.len()].iter_mut().enumerate() {
        *slot = (index + 1) * MOVE_COST;
    }
    let mut result = 0;
    for (b_index, &code) in b.iter().enumerate() {
        let mut distance = b_index * MOVE_COST;
        result = distance;
        let mut minimum = usize::MAX;
        for (index, &other) in a.iter().enumerate() {
            let substitute = distance + substitution_cost(code, other);
            distance = row[index];
            result = (result.min(distance) + MOVE_COST).min(substitute);
            row[index] = result;
            minimum = minimum.min(result);
        }
        if minimum > max_cost {
            return max_cost + 1;
        }
    }
    result
}

pub(crate) fn calculate_suggestion<'a>(name: &str, candidates: &[&'a str]) -> Option<&'a str> {
    if candidates.len() >= MAX_CANDIDATE_ITEMS {
        return None;
    }
    let mut best_distance = usize::MAX;
    let mut best = None;
    for &candidate in candidates {
        if candidate == name {
            continue;
        }
        let max_distance =
            ((name.len() + candidate.len() + 3) * MOVE_COST / 6).min(best_distance - 1);
        let distance = edit_cost(name.as_bytes(), candidate.as_bytes(), max_distance);
        if distance <= max_distance {
            best = Some(candidate);
            best_distance = distance;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_obey_utf8_costs_ties_affixes_and_candidate_cap() {
        assert_eq!(edit_cost("é".as_bytes(), b"e", 4), 4);
        assert_eq!(calculate_suggestion("abd", &["abc", "abe"]), Some("abc"));
        assert_eq!(
            calculate_suggestion("abc", &["abc", "abd", "abC"]),
            Some("abC")
        );
        assert_eq!(calculate_suggestion("abcdef", &["ghijkl"]), None);
        assert_eq!(
            calculate_suggestion("ordr", &["order"; MAX_CANDIDATE_ITEMS]),
            None
        );
        assert_eq!(edit_cost(&[b'a'; 41], &[b'b'; 41], 100), 101);
        let prefix = "x".repeat(60);
        assert_eq!(
            edit_cost(
                format!("{prefix}a").as_bytes(),
                format!("{prefix}b").as_bytes(),
                2
            ),
            2
        );
    }
}
