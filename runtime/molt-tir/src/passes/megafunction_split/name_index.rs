//! Sparse linear def/use facts for split planning.
//!
//! Events borrow names from the immutable source body. Storage is proportional
//! to real reads/definitions, not body length times the number of live names.
//! Reads precede definitions within an op, matching the canonical def/use API.

use crate::OpIR;
use crate::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};
use std::collections::BTreeMap;

#[derive(Default)]
struct NameEvents {
    reads: Vec<usize>,
    definitions: Vec<usize>,
}

impl NameEvents {
    fn live_before(&self, at: usize) -> bool {
        let Some(&read) = self
            .reads
            .get(self.reads.partition_point(|&index| index < at))
        else {
            return false;
        };
        self.definitions
            .get(self.definitions.partition_point(|&index| index < at))
            .is_none_or(|&definition| read <= definition)
    }

    fn defined_in(&self, start: usize, end: usize) -> bool {
        self.definitions
            .get(self.definitions.partition_point(|&index| index < start))
            .is_some_and(|&definition| definition < end)
    }
}

pub(super) struct SplitNameIndex<'a> {
    events: BTreeMap<&'a str, NameEvents>,
}

impl<'a> SplitNameIndex<'a> {
    pub(super) fn new(ops: &'a [OpIR]) -> Self {
        let mut events = BTreeMap::<&str, NameEvents>::new();
        for (index, op) in ops.iter().enumerate() {
            visit_simple_ir_reads(op, |source| {
                events.entry(source.name).or_default().reads.push(index);
            });
            visit_simple_ir_defined_names(op, |name| {
                events.entry(name).or_default().definitions.push(index);
            });
        }
        Self { events }
    }

    pub(super) fn live_names(&self, at: usize) -> impl Iterator<Item = &'a str> + '_ {
        self.events
            .iter()
            .filter_map(move |(&name, events)| events.live_before(at).then_some(name))
    }

    pub(super) fn is_live_before(&self, name: &str, at: usize) -> bool {
        self.events
            .get(name)
            .is_some_and(|events| events.live_before(at))
    }

    pub(super) fn is_defined_before(&self, name: &str, at: usize) -> bool {
        self.events
            .get(name)
            .is_some_and(|events| events.definitions.first().is_some_and(|&index| index < at))
    }

    pub(super) fn is_defined_in(&self, name: &str, start: usize, end: usize) -> bool {
        self.events
            .get(name)
            .is_some_and(|events| events.defined_in(start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn sparse_queries_match_reverse_and_forward_dataflow_at_every_boundary() {
        let ops = vec![
            OpIR {
                kind: "copy".into(),
                args: Some(vec!["input".into()]),
                out: Some("x".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy".into(),
                args: Some(vec!["x".into()]),
                out: Some("x".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy".into(),
                args: Some(vec!["external".into()]),
                out: Some("x".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["x".into(), "input".into()]),
                out: Some("sum".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".into(),
                args: Some(vec!["sum".into()]),
                ..OpIR::default()
            },
        ];
        let index = SplitNameIndex::new(&ops);
        let mut live = BTreeSet::new();
        for at in (0..=ops.len()).rev() {
            if let Some(op) = ops.get(at) {
                visit_simple_ir_defined_names(op, |name| {
                    live.remove(name);
                });
                visit_simple_ir_reads(op, |source| {
                    live.insert(source.name);
                });
            }
            assert_eq!(
                index.live_names(at).collect::<BTreeSet<_>>(),
                live,
                "boundary {at}"
            );
        }
        for start in 0..=ops.len() {
            for end in start..=ops.len() {
                let mut defined = BTreeSet::new();
                for op in &ops[start..end] {
                    visit_simple_ir_defined_names(op, |name| {
                        defined.insert(name);
                    });
                }
                for name in ["input", "x", "external", "sum", "absent"] {
                    assert_eq!(
                        index.is_defined_in(name, start, end),
                        defined.contains(name)
                    );
                    if start == 0 {
                        assert_eq!(index.is_defined_before(name, end), defined.contains(name));
                    }
                }
            }
        }
    }

    #[test]
    fn storage_tracks_events_not_dense_live_set_snapshots() {
        let ops = (0..2048)
            .map(|index| OpIR {
                kind: "copy".into(),
                args: Some(vec![format!("input_{index}")]),
                out: Some(format!("result_{index}")),
                ..OpIR::default()
            })
            .collect::<Vec<_>>();
        let index = SplitNameIndex::new(&ops);
        assert_eq!(index.events.len(), 4096);
        assert_eq!(
            index
                .events
                .values()
                .map(|events| events.reads.len() + events.definitions.len())
                .sum::<usize>(),
            4096
        );
        assert_eq!(index.live_names(0).count(), 2048);
        assert_eq!(index.live_names(2048).count(), 0);
    }
}
