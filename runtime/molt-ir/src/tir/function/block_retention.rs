//! Atomic block retirement and the metadata whose identity depends on blocks.

use std::collections::HashSet;

use super::TirFunction;
use crate::tir::blocks::{BlockId, LoopRole};
use crate::tir::clone_support::exception_label_of;
use crate::tir::dominators::exception_label_to_block;

impl TirFunction {
    /// Blocks that carry live structural obligations, including both endpoints
    /// of loop relations. These roots are not extra executable CFG edges.
    pub fn structural_block_roots(&self) -> HashSet<BlockId> {
        let mut roots = self.loop_metadata_roots_after_retirement(&HashSet::new());
        roots.insert(self.entry_block);
        roots
    }

    fn retired_loop_end_roles(&self, headers: &HashSet<BlockId>) -> HashSet<BlockId> {
        let mut retired: HashSet<_> = self
            .loop_pairs
            .iter()
            .filter(|(header, _)| headers.contains(*header))
            .map(|(_, &end)| end)
            .collect();
        for (&header, &end) in &self.loop_pairs {
            if !headers.contains(&header) {
                retired.remove(&end);
            }
        }
        retired
    }

    fn loop_metadata_roots_after_retirement(&self, headers: &HashSet<BlockId>) -> HashSet<BlockId> {
        let ends = self.retired_loop_end_roles(headers);
        let mut roots = HashSet::new();
        roots.extend(self.loop_roles.iter().filter_map(|(&id, role)| {
            (!headers.contains(&id) && !(role == &LoopRole::LoopEnd && ends.contains(&id)))
                .then_some(id)
        }));
        roots.extend(
            self.loop_break_kinds
                .keys()
                .copied()
                .filter(|id| !headers.contains(id)),
        );
        for relation in [&self.loop_pairs, &self.loop_cond_blocks] {
            for (&owner, &target) in relation {
                if !headers.contains(&owner) {
                    roots.extend([owner, target]);
                }
            }
        }
        roots
    }

    /// Explicitly retire the structured description of a loop replaced by a
    /// transform. Its blocks and ordinary/exception edges are left untouched.
    /// A paired end role belongs to this loop unless another surviving loop
    /// still references it. Never silently drop only one endpoint of a pair.
    pub fn retire_loop_metadata(&mut self, header: BlockId) {
        let ends = self.retired_loop_end_roles(&HashSet::from([header]));
        self.loop_pairs.remove(&header);
        self.loop_roles.remove(&header);
        self.loop_break_kinds.remove(&header);
        self.loop_cond_blocks.remove(&header);
        for end in ends {
            if self.loop_roles.get(&end) == Some(&LoopRole::LoopEnd) {
                self.loop_roles.remove(&end);
            }
        }
    }

    /// Reusable read-only metadata constraints for transforms that rewire
    /// ordinary edges without remapping labels. The entry is not a metadata
    /// constraint: a transform may explicitly replace it; retain_blocks checks
    /// the finished entry. Calling once amortizes the scan across diamonds.
    pub fn block_retirement_metadata_roots(
        &self,
        retired_loop_headers: &HashSet<BlockId>,
    ) -> Result<HashSet<BlockId>, String> {
        let mut protected = self.loop_metadata_roots_after_retirement(retired_loop_headers);
        let mut roots: Vec<_> = protected.iter().copied().collect();
        roots.sort_unstable();
        for root in roots {
            if !self.blocks.contains_key(&root) {
                return Err(format!(
                    "function `{}` retirement sees missing structural loop root ^{}",
                    self.name, root.0
                ));
            }
        }
        let targets = exception_label_to_block(self);
        let mut ids: Vec<_> = self.blocks.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            for (index, op) in self.blocks[&id].ops.iter().enumerate() {
                if let Some(label) = exception_label_of(op) {
                    let target = targets.get(&label).ok_or_else(|| format!(
                        "function `{}` retirement sees ambiguous or missing label {label} in block ^{} op {index}", self.name, id.0
                    ))?;
                    if !self.blocks.contains_key(target) {
                        return Err(format!(
                            "function `{}` retirement sees label {label} on missing block ^{}",
                            self.name, target.0
                        ));
                    }
                    protected.insert(*target);
                }
            }
        }
        Ok(protected)
    }

    /// Read-only eligibility for a planned region retirement. Every ordinary
    /// edge from outside `retired` must either survive or target a block in
    /// `rewired_targets`, whose incoming edges the transform explicitly replaces.
    /// This includes unreachable and metadata-only predecessors: reachability
    /// does not authorize leaving a retained block with a dangling terminator.
    /// Edges originating inside the retired region need no rewiring.
    ///
    /// `replaces_entry` declares replacement of a retired function entry, not
    /// permission to remove it without installing a new entry. Planned loop
    /// retirement uses the same metadata authority as commit. All exception
    /// label references count, including those in the cloned source region;
    /// ordinary-edge rewiring never authorizes loss of label custody.
    ///
    /// A rejection occurs before ids, operations, statistics or metadata change.
    /// The transform must fulfill its declared rewiring; retain_blocks validates
    /// the finished graph at commit. No second metadata-only admission path.
    pub fn validate_block_retirement(
        &self,
        retired: &HashSet<BlockId>,
        retired_loop_headers: &HashSet<BlockId>,
        rewired_targets: &HashSet<BlockId>,
        replaces_entry: bool,
    ) -> Result<(), String> {
        if let Some(target) = rewired_targets.difference(retired).min() {
            return Err(format!(
                "function `{}` retirement rewires non-retired target ^{}",
                self.name, target.0
            ));
        }
        if retired.contains(&self.entry_block) != replaces_entry {
            return Err(format!(
                "function `{}` retirement entry replacement does not match retired entry ^{}",
                self.name, self.entry_block.0
            ));
        }
        let mut retired_ids: Vec<_> = retired.iter().copied().collect();
        retired_ids.sort_unstable();
        for id in retired_ids {
            if !self.blocks.contains_key(&id) {
                return Err(format!(
                    "function `{}` retirement requests missing block ^{}",
                    self.name, id.0
                ));
            }
        }
        let roots = self.block_retirement_metadata_roots(retired_loop_headers)?;
        if let Some(root) = retired.intersection(&roots).min() {
            return Err(format!(
                "function `{}` retirement would lose protected metadata block ^{}",
                self.name, root.0
            ));
        }
        let mut retained_ids: Vec<_> = self
            .blocks
            .keys()
            .copied()
            .filter(|id| !retired.contains(id))
            .collect();
        retained_ids.sort_unstable();
        for id in retained_ids {
            for target in self.blocks[&id].terminator.successors() {
                if retired.contains(&target) && !rewired_targets.contains(&target) {
                    return Err(format!(
                        "function `{}` retirement leaves retained block ^{} with an unrewired edge to retired block ^{}",
                        self.name, id.0, target.0
                    ));
                }
            }
        }
        Ok(())
    }

    /// Retain a caller-proved block set after edge rewrites. This is an atomic,
    /// fail-closed operation: entry/structural roots, every retained terminator
    /// edge, and every retained exception-label reference (including TryEnd)
    /// must survive. A transform replacing a loop must explicitly retire its
    /// metadata first, rather than losing a surviving loop's end/condition.
    /// Returns the number of deleted blocks; labels and value facts belonging
    /// to those blocks are retired in the same operation. No caller-side
    /// key-only loop-map cleanup is needed or permitted.
    pub fn retain_blocks(&mut self, retained: &HashSet<BlockId>) -> Result<usize, String> {
        let mut roots: Vec<_> = self.structural_block_roots().into_iter().collect();
        roots.sort_unstable();
        for root in roots {
            if !self.blocks.contains_key(&root) || !retained.contains(&root) {
                return Err(format!(
                    "function `{}` block retention would remove or leave missing structural root ^{}; retire replaced loop metadata explicitly",
                    self.name, root.0
                ));
            }
        }
        let labels = exception_label_to_block(self);
        let mut ids: Vec<_> = retained.iter().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let block = self.blocks.get(&id).ok_or_else(|| {
                format!(
                    "function `{}` block retention requests missing block ^{}",
                    self.name, id.0
                )
            })?;
            for target in block.terminator.successors() {
                if !retained.contains(&target) || !self.blocks.contains_key(&target) {
                    return Err(format!(
                        "function `{}` retained block ^{} has an edge to retired or missing block ^{}",
                        self.name, id.0, target.0
                    ));
                }
            }
            for (index, op) in block.ops.iter().enumerate() {
                if let Some(label) = exception_label_of(op) {
                    let target = labels.get(&label).ok_or_else(|| format!(
                        "function `{}` retained block ^{} op {index} references exception label {label} without a unique target",
                        self.name, id.0
                    ))?;
                    if !retained.contains(target) || !self.blocks.contains_key(target) {
                        return Err(format!(
                            "function `{}` retained block ^{} op {index} references exception label {label} on retired or missing block ^{}",
                            self.name, id.0, target.0
                        ));
                    }
                }
            }
        }
        let removed = self.blocks.len() - retained.len();
        self.blocks.retain(|id, _| retained.contains(id));
        self.label_id_map
            .retain(|id, _| retained.contains(&BlockId(*id)));
        let values: HashSet<_> = self
            .blocks
            .values()
            .flat_map(|block| {
                block
                    .args
                    .iter()
                    .map(|arg| arg.id)
                    .chain(block.ops.iter().flat_map(|op| op.results.iter().copied()))
            })
            .collect();
        self.value_types.retain(|value, _| values.contains(value));
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopBreakKind, Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::serialize::serialize_tir_function;
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn fixture() -> TirFunction {
        let mut func = TirFunction::new("retention".into(), vec![], TirType::None);
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        for id in 1..=4 {
            func.blocks.insert(
                BlockId(id),
                TirBlock {
                    id: BlockId(id),
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }
        func.next_block = 5;
        func
    }

    fn labelled_op(opcode: OpCode, label: i64) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::from([("value".into(), AttrValue::Int(label))]),
            source_span: None,
        }
    }

    #[test]
    fn block_retirement_cleans_label_and_value_projections_together() {
        let mut func = fixture();
        func.label_id_map.insert(1, 99);
        func.blocks.get_mut(&BlockId(1)).unwrap().ops.push(TirOp {
            results: vec![ValueId(20)],
            ..labelled_op(OpCode::ConstInt, 4)
        });
        func.value_types.insert(ValueId(20), TirType::I64);
        assert_eq!(
            func.retain_blocks(&HashSet::from([func.entry_block]))
                .unwrap(),
            4
        );
        assert!(func.label_id_map.is_empty());
        assert!(func.value_types.is_empty());
    }

    #[test]
    fn block_retirement_rejects_dangling_edges_labels_and_entry_atomically() {
        for kind in [
            None,
            Some(OpCode::CheckException),
            Some(OpCode::TryStart),
            Some(OpCode::TryEnd),
        ] {
            let mut func = fixture();
            func.label_id_map.insert(1, 99);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            if let Some(opcode) = kind {
                entry.ops.push(labelled_op(opcode, 99));
            } else {
                entry.terminator = Terminator::Branch {
                    target: BlockId(1),
                    args: vec![],
                };
            }
            let before = serialize_tir_function(&func).unwrap();
            assert!(
                func.retain_blocks(&HashSet::from([func.entry_block]))
                    .is_err()
            );
            assert_eq!(serialize_tir_function(&func).unwrap(), before);
            assert!(func.retain_blocks(&HashSet::from([BlockId(1)])).is_err());
            assert_eq!(serialize_tir_function(&func).unwrap(), before);
        }
    }

    #[test]
    fn block_retirement_does_not_resolve_ambiguous_labels_by_deleting_an_alias() {
        let mut func = fixture();
        func.label_id_map.extend([(1, 99), (2, 99)]);
        func.blocks
            .get_mut(&func.entry_block)
            .unwrap()
            .ops
            .push(labelled_op(OpCode::TryEnd, 99));
        let before = serialize_tir_function(&func).unwrap();
        let error = func
            .retain_blocks(&HashSet::from([func.entry_block, BlockId(1)]))
            .unwrap_err();
        assert!(error.contains("without a unique target"));
        assert_eq!(serialize_tir_function(&func).unwrap(), before);
    }

    #[test]
    fn loop_keys_and_endpoints_require_explicit_whole_loop_retirement() {
        let mut func = fixture();
        let (header, end, condition) = (BlockId(1), BlockId(2), BlockId(3));
        func.loop_roles
            .extend([(header, LoopRole::LoopHeader), (end, LoopRole::LoopEnd)]);
        func.loop_pairs.insert(header, end);
        func.loop_cond_blocks.insert(header, condition);
        func.loop_break_kinds
            .insert(header, LoopBreakKind::BreakIfTrue);
        assert_eq!(
            func.structural_block_roots(),
            HashSet::from([func.entry_block, header, end, condition])
        );
        for removed in [header, end, condition] {
            let retained = func
                .blocks
                .keys()
                .copied()
                .filter(|&id| id != removed)
                .collect();
            let before = serialize_tir_function(&func).unwrap();
            assert!(
                func.retain_blocks(&retained)
                    .unwrap_err()
                    .contains("structural root")
            );
            assert_eq!(serialize_tir_function(&func).unwrap(), before);
        }
        func.retire_loop_metadata(header);
        assert!(func.loop_roles.is_empty());
        assert!(func.loop_pairs.is_empty());
        assert!(func.loop_cond_blocks.is_empty());
        assert!(func.loop_break_kinds.is_empty());
        assert_eq!(
            func.retain_blocks(&HashSet::from([func.entry_block]))
                .unwrap(),
            4
        );
    }

    #[test]
    fn loop_retirement_preserves_other_loop_owners_and_diagnoses_stale_roots() {
        let mut func = fixture();
        func.loop_pairs
            .extend([(BlockId(1), BlockId(3)), (BlockId(2), BlockId(3))]);
        func.loop_roles.insert(BlockId(3), LoopRole::LoopEnd);
        func.retire_loop_metadata(BlockId(1));
        assert_eq!(func.loop_roles.get(&BlockId(3)), Some(&LoopRole::LoopEnd));
        func.retire_loop_metadata(BlockId(2));
        assert!(!func.loop_roles.contains_key(&BlockId(3)));
        func.loop_cond_blocks.insert(BlockId(1), BlockId(999));
        let retained = func.blocks.keys().copied().collect();
        assert!(
            func.retain_blocks(&retained)
                .unwrap_err()
                .contains("structural root ^999")
        );
    }

    #[test]
    fn reusable_metadata_constraints_match_preflight_and_explicit_retirement() {
        let mut func = fixture();
        let (header, end, condition) = (BlockId(1), BlockId(2), BlockId(3));
        func.loop_roles
            .extend([(header, LoopRole::LoopHeader), (end, LoopRole::LoopEnd)]);
        func.loop_pairs.insert(header, end);
        func.loop_cond_blocks.insert(header, condition);
        let region = HashSet::from([header, end, condition]);
        assert!(
            func.validate_block_retirement(&region, &HashSet::new(), &HashSet::new(), false)
                .is_err()
        );
        assert!(
            func.validate_block_retirement(
                &region,
                &HashSet::from([header]),
                &HashSet::new(),
                false
            )
            .is_ok()
        );
        func.label_id_map.insert(header.0, 99);
        // A cloned instruction would carry this original label into the new body.
        func.blocks
            .get_mut(&header)
            .unwrap()
            .ops
            .push(labelled_op(OpCode::TryEnd, 99));
        let roots = func
            .block_retirement_metadata_roots(&HashSet::from([header]))
            .unwrap();
        assert_eq!(roots, HashSet::from([header]));
        assert!(
            func.validate_block_retirement(
                &region,
                &HashSet::from([header]),
                &HashSet::new(),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn retirement_preflight_checks_retained_incoming_edges_before_any_mutation() {
        let mut func = fixture();
        let (header, body, outside) = (BlockId(1), BlockId(2), BlockId(3));
        let retired = HashSet::from([header, body]);
        let rewired = HashSet::from([header]);
        // Internal edges disappear with their source. The external block has
        // no entry path, but is retained and still requires valid transport.
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
        func.blocks.get_mut(&outside).unwrap().terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
        for structural in [false, true] {
            if structural {
                func.loop_cond_blocks.insert(func.entry_block, outside);
            }
            let before = serialize_tir_function(&func).unwrap();
            let error = func
                .validate_block_retirement(&retired, &HashSet::new(), &rewired, false)
                .unwrap_err();
            assert!(error.contains("retained block ^3"));
            assert!(error.contains("unrewired edge to retired block ^2"));
            assert_eq!(serialize_tir_function(&func).unwrap(), before);
        }
        func.blocks.get_mut(&outside).unwrap().terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
        assert!(
            func.validate_block_retirement(&retired, &HashSet::new(), &rewired, false)
                .is_ok()
        );
        // Ordinary transport does not remap labels inside the cloned region.
        func.label_id_map.insert(header.0, 99);
        func.blocks
            .get_mut(&body)
            .unwrap()
            .ops
            .push(labelled_op(OpCode::TryEnd, 99));
        assert!(
            func.validate_block_retirement(&retired, &HashSet::new(), &rewired, false)
                .unwrap_err()
                .contains("protected metadata block")
        );
    }

    #[test]
    fn retirement_preflight_requires_honest_entry_and_rewired_target_intent() {
        let func = fixture();
        let before = serialize_tir_function(&func).unwrap();
        let none = HashSet::new();
        let region = HashSet::from([BlockId(1)]);
        assert!(
            func.validate_block_retirement(&region, &none, &HashSet::from([BlockId(2)]), false)
                .unwrap_err()
                .contains("non-retired target")
        );
        assert!(
            func.validate_block_retirement(&region, &none, &none, true)
                .is_err()
        );
        let entry = HashSet::from([func.entry_block]);
        assert!(
            func.validate_block_retirement(&entry, &none, &none, false)
                .is_err()
        );
        assert!(
            func.validate_block_retirement(&entry, &none, &none, true)
                .is_ok()
        );
        assert!(
            func.validate_block_retirement(&HashSet::from([BlockId(999)]), &none, &none, false)
                .unwrap_err()
                .contains("missing block")
        );
        assert_eq!(serialize_tir_function(&func).unwrap(), before);
    }
}
