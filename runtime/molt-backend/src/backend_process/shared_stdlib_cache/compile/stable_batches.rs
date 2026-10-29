use molt_backend::FunctionIR;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Existing source-file identity separates compilation units before symbol radix
/// splitting. An unrelated source cannot repack existing units. Missing source
/// metadata forms one legitimate group; bounds and traversal ordering remain
/// owned by the same radix splitter within every group.
pub(super) fn stable_stdlib_batches(
    functions: Vec<FunctionIR>,
    max_functions: usize,
    max_ops: usize,
) -> Vec<Vec<FunctionIR>> {
    let mut sources: BTreeMap<Option<String>, Vec<_>> = BTreeMap::new();
    for function in functions {
        let mut digest = Sha256::new();
        digest.update(b"native-stdlib-symbol-bucket-v1\0");
        digest.update(function.name.as_bytes());
        let digest: [u8; 32] = digest.finalize().into();
        sources
            .entry(function.source_file.clone())
            .or_default()
            .push((digest, function));
    }
    let mut batches = Vec::new();
    for mut ordered in sources.into_values() {
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

    fn sourced_fixture(name: &str, source: Option<&str>, ops: usize) -> FunctionIR {
        FunctionIR {
            source_file: source.map(str::to_owned),
            ops: vec![molt_backend::OpIR::new(molt_ir::OpKind::Nop); ops],
            ..fixture(name)
        }
    }

    #[test]
    fn unrelated_source_insertion_preserves_every_existing_unit() {
        let functions: Vec<_> = (0..24)
            .map(|index| {
                sourced_fixture(
                    &format!("existing_{index}"),
                    Some(if index < 12 {
                        "stdlib/alpha.py"
                    } else {
                        "stdlib/beta.py"
                    }),
                    2,
                )
            })
            .collect();
        let baseline = names(stable_stdlib_batches(functions.clone(), 128, 8000));
        let mut extended = functions;
        extended.extend(
            (0..14).map(|index| {
                sourced_fixture(&format!("new_{index}"), Some("stdlib/unrelated.py"), 3)
            }),
        );
        let after = names(stable_stdlib_batches(extended.clone(), 128, 8000));
        // Whole units, not just surviving symbols, must be reusable. A global
        // symbol bucket would add new bodies to the previous unit and fail.
        for unit in &baseline {
            assert!(after.contains(unit), "existing unit was repacked: {unit:?}");
        }
        assert_eq!(after.len(), baseline.len() + 1);
        extended.reverse();
        assert_eq!(after, names(stable_stdlib_batches(extended, 128, 8000)));
    }

    #[test]
    fn oversized_source_uses_existing_count_and_operation_bounds() {
        let functions: Vec<_> = (0..80)
            .map(|index| sourced_fixture(&format!("large_{index}"), Some("large.py"), 3))
            .collect();
        for (count, ops) in [(4, 8000), (128, 10)] {
            let batches = stable_stdlib_batches(functions.clone(), count, ops);
            assert!(batches.len() > 1);
            assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), functions.len());
            for batch in &batches {
                assert!(batch.len() <= count);
                assert!(batch.iter().map(|f| f.ops.len()).sum::<usize>() <= ops);
                assert!(
                    batch
                        .iter()
                        .all(|f| f.source_file.as_deref() == Some("large.py"))
                );
            }
            let mut reversed = functions.clone();
            reversed.reverse();
            assert_eq!(
                names(batches),
                names(stable_stdlib_batches(reversed, count, ops))
            );
        }
        let indivisible = sourced_fixture("indivisible", Some("large.py"), 11);
        let batches = stable_stdlib_batches(vec![indivisible], 4, 10);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0][0].ops.len(), 11);
    }

    #[test]
    fn absent_and_empty_source_metadata_remain_distinct_and_empty_archive_has_one_anchor() {
        let functions = vec![
            sourced_fixture("missing_a", None, 1),
            sourced_fixture("missing_b", None, 1),
            sourced_fixture("empty_path", Some(""), 1),
            sourced_fixture("named", Some("module.py"), 1),
        ];
        let batches = names(stable_stdlib_batches(functions, 128, 8000));
        assert_eq!(
            batches,
            vec![
                vec!["missing_a".to_owned(), "missing_b".to_owned()],
                vec!["empty_path".to_owned()],
                vec!["named".to_owned()],
            ]
        );
        let empty = stable_stdlib_batches(Vec::new(), 128, 8000);
        assert_eq!(empty.len(), 1);
        assert!(empty[0].is_empty());
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
