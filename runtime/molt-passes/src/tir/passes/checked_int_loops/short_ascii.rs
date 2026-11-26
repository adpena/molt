//! Cache the bytes and length of one exact immutable short ASCII string
//! across a loop. No borrowed pointer escapes the one runtime read.
use super::*;
use crate::tir::ops::AttrValue;
use molt_ir::runtime_fn_generated::RuntimeFn;

pub(super) struct ShortAscii {
    preheader: BlockId,
    header: BlockId,
    owner: ValueId,
}

impl ShortAscii {
    pub(super) fn admit(
        facts: &LoopFacts<'_>,
        header: BlockId,
        body: &HashSet<BlockId>,
        vr: &crate::tir::ValueRangeResult,
        preheader: BlockId,
    ) -> Option<Self> {
        let func = facts.scalar.body();
        if func.frame_locals_exposed() {
            return None;
        }
        // Require an actual explicit builtin length and ordinal of the same
        // loop-invariant SSA owner. A dynamic or guarded Python call still
        // takes its own path. The existing edge-sensitive bounds proof also
        // rejects negative, offset, or exception-bypassed index uses.
        let mut owner = None;
        let mut lengths = Vec::new();
        let mut ordinals = Vec::new();
        for bid in body {
            for op in &func.blocks[bid].ops {
                let input = if let Some(input) = op.length_argument() {
                    lengths.push(op.results[0]);
                    input
                } else if op.opcode == OpCode::OrdAt {
                    let ([input, index], [_]) = (&op.operands[..], &op.results[..]) else {
                        return None;
                    };
                    ordinals.push((*bid, *index));
                    *input
                } else {
                    continue;
                };
                if owner.replace(input).is_some_and(|v| v != input) {
                    return None;
                }
            }
        }
        if lengths.is_empty() || ordinals.is_empty() {
            return None;
        }
        let owner = owner?;
        if body.iter().any(|bid| {
            let block = &func.blocks[bid];
            block.args.iter().any(|arg| arg.id == owner)
                || block.ops.iter().any(|op| op.results.contains(&owner))
        }) {
            return None;
        }
        // Passing the original boxed owner may not introduce a fallible box
        // at a location that had no such operation.
        if facts.representations(vr).get(&owner) != Some(&Repr::DynBox) {
            return None;
        }
        let preview = super::super::value_range::under_length_guard(
            &facts.scalar,
            &lengths,
            i64::from(u32::BITS / u8::BITS),
        );
        if !ordinals.iter().all(|&(bid, index)| {
            facts.scalar.types().get(&index) == Some(&TirType::I64)
                && preview.proves_index_lt_len_symbolically(bid, owner, index)
        }) {
            return None;
        }
        Some(Self {
            owner,
            header,
            preheader,
        })
    }

    pub(super) fn version_fast(
        &self,
        func: &mut TirFunction,
        body: &HashSet<BlockId>,
        clones: &HashMap<BlockId, BlockId>,
    ) -> usize {
        let fresh = |f: &mut TirFunction, ty| {
            let id = f.fresh_value();
            f.value_types.insert(id, ty);
            id
        };
        let constant = |f: &mut TirFunction, n| {
            let id = fresh(f, TirType::I64);
            let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![id]);
            op.attrs.insert("value".into(), AttrValue::Int(n));
            (id, op)
        };
        let snapshot = fresh(func, TirType::DynBox);
        let admitted = fresh(func, TirType::Bool);
        let packed = fresh(func, TirType::I64);
        let length = fresh(func, TirType::I64);
        let upper = fresh(func, TirType::I64);
        let (length_shift, shift_op) = constant(func, i64::from(u32::BITS));
        let (length_mask, len_mask_op) = constant(func, 7);
        let (byte_mask, byte_mask_op) = constant(func, 127);
        let (bits_per_byte, width_op) = constant(func, i64::from(u8::BITS));
        let setup = func.fresh_block();
        let preheader = func.blocks.get_mut(&self.preheader).unwrap();
        let Terminator::Branch { args, .. } = &preheader.terminator else {
            unreachable!("verified single plain preheader")
        };
        let args = args.clone();
        let mut read = TirOp::new(OpCode::CallRuntime, vec![self.owner], vec![snapshot]);
        read.runtime = Some(RuntimeFn::ShortAsciiSnapshot);
        preheader.ops.extend([
            read,
            TirOp::new(OpCode::IsInlineInt, vec![snapshot], vec![admitted]),
        ]);
        preheader.terminator = Terminator::CondBranch {
            cond: admitted,
            then_block: setup,
            then_args: vec![],
            else_block: clones[&self.header],
            else_args: args.clone(),
        };
        // Masking makes the range visible to the same interval analysis that
        // bounds the recurrence. Reading a byte never observes header bits.
        let setup_ops = vec![
            TirOp::new(OpCode::UnboxInlineInt, vec![snapshot], vec![packed]),
            shift_op,
            len_mask_op,
            byte_mask_op,
            width_op,
            TirOp::new(OpCode::Shr, vec![packed, length_shift], vec![upper]),
            TirOp::new(OpCode::BitAnd, vec![upper, length_mask], vec![length]),
        ];
        let mut added = 2 + setup_ops.len();
        func.blocks.insert(
            setup,
            TirBlock {
                id: setup,
                args: vec![],
                ops: setup_ops,
                terminator: Terminator::Branch {
                    target: self.header,
                    args,
                },
            },
        );
        let mut ordered: Vec<_> = body.iter().copied().collect();
        ordered.sort_unstable();
        for bid in ordered {
            let old = std::mem::take(&mut func.blocks.get_mut(&bid).unwrap().ops);
            let mut ops = Vec::with_capacity(old.len());
            for original in old {
                if original.length_argument() == Some(self.owner) {
                    ops.push(TirOp::new(OpCode::Copy, vec![length], original.results));
                } else if original.opcode == OpCode::OrdAt {
                    let offset = fresh(func, TirType::I64);
                    let shifted = fresh(func, TirType::I64);
                    ops.extend([
                        TirOp::new(
                            OpCode::Mul,
                            vec![original.operands[1], bits_per_byte],
                            vec![offset],
                        ),
                        TirOp::new(OpCode::Shr, vec![packed, offset], vec![shifted]),
                        TirOp::new(OpCode::BitAnd, vec![shifted, byte_mask], original.results),
                    ]);
                    added += 2;
                } else {
                    ops.push(original);
                }
            }
            func.blocks.get_mut(&bid).unwrap().ops = ops;
        }
        added
    }
}
