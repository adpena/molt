use molt_backend::FunctionIR;
use sha2::{Digest, Sha256};

/// Symbol radix buckets isolate unrelated insertions/removals. Start with one
/// bucket and refine only when the existing count/op budgets require it; there
/// is no fixed worker-count floor. Bounds never spill a bucket into a neighbor.
/// Full digest sorting and symbol ordering make traversal order irrelevant.
pub(super) fn stable_stdlib_batches(
    functions: Vec<FunctionIR>,
    max_functions: usize,
    max_ops: usize,
) -> Vec<Vec<FunctionIR>> {
    let mut ordered = Vec::new();
    for function in functions {
        let mut digest = Sha256::new();
        digest.update(b"native-stdlib-symbol-bucket-v1\0");
        digest.update(function.name.as_bytes());
        let digest: [u8; 32] = digest.finalize().into();
        ordered.push((digest, function));
    }
    let mut batches = Vec::new();
    if !ordered.is_empty() {
        ordered.sort_by(|(a, left), (b, right)| a.cmp(b).then(left.name.cmp(&right.name)));
        split_bucket(
            ordered,
            0,
            max_functions.max(1),
            max_ops.max(1),
            &mut batches,
        );
    }
    if batches.is_empty() {
        // The archive contract requires one real relocatable object, including
        // the ABI anchor, even when no stdlib body is reachable.
        batches.push(Vec::new());
    }
    batches
}

fn split_bucket(
    mut functions: Vec<([u8; 32], FunctionIR)>,
    bit: usize,
    max_functions: usize,
    max_ops: usize,
    out: &mut Vec<Vec<FunctionIR>>,
) {
    let ops = functions
        .iter()
        .fold(0usize, |sum, (_, f)| sum.saturating_add(f.ops.len()));
    if functions.len() <= 1 || (functions.len() <= max_functions && ops <= max_ops) {
        functions.sort_by(|(_, left), (_, right)| left.name.cmp(&right.name));
        out.push(
            functions
                .into_iter()
                .map(|(_, function)| function)
                .collect(),
        );
        return;
    }
    assert!(
        bit < 256,
        "distinct native stdlib symbols collide in the complete SHA256 bucket identity"
    );
    let midpoint = functions.partition_point(|(hash, _)| hash[bit / 8] & (1 << (7 - bit % 8)) == 0);
    let right = functions.split_off(midpoint);
    if !functions.is_empty() {
        split_bucket(functions, bit + 1, max_functions, max_ops, out);
    }
    if !right.is_empty() {
        split_bucket(right, bit + 1, max_functions, max_ops, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            ..FunctionIR::default()
        }
    }

    fn names(batches: Vec<Vec<FunctionIR>>) -> Vec<Vec<String>> {
        batches
            .into_iter()
            .map(|batch| batch.into_iter().map(|f| f.name).collect())
            .collect()
    }

    #[test]
    fn stable_buckets_ignore_input_order_and_do_not_spill_unrelated_roots() {
        let functions: Vec<_> = (0..128)
            .map(|index| fixture(&format!("stdlib_{index}")))
            .collect();
        let baseline = names(stable_stdlib_batches(functions.clone(), 4, 100));
        let mut reversed = functions.clone();
        reversed.reverse();
        assert_eq!(baseline, names(stable_stdlib_batches(reversed, 4, 100)));
        let removed = &functions[0].name;
        let bucket = |name: &str| {
            let mut hash = Sha256::new();
            hash.update(b"native-stdlib-symbol-bucket-v1\0");
            hash.update(name.as_bytes());
            hash.finalize()[0] >> 7
        };
        let unchanged = |batches: Vec<Vec<String>>| {
            batches
                .into_iter()
                .filter(|batch| bucket(&batch[0]) != bucket(removed))
                .collect::<Vec<_>>()
        };
        let after = names(stable_stdlib_batches(functions[1..].to_vec(), 4, 100));
        assert_eq!(unchanged(baseline), unchanged(after));
    }
}
