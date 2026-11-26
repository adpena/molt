//! Guard the successful prefix of a checked packed wrapper. Until the packed
//! read commits, every operation borrows its inputs and can abandon the attempt
//! for the original invocation without releasing a credit or servicing work.

use molt_ir::ir::GlobalSlotRef;

use super::packed_splice::test_block;
use super::packed_wrapper::{Check, Event, Frame, PackedWrapper, Source};
use crate::tir::blocks::BlockId;
use crate::tir::call_targets::static_call;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::spliced_frame::{Binding, KeySpace, Level};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

pub(super) struct Prefix<'p> {
    summary: &'p PackedWrapper,
    wrapper_function: ValueId,
    arguments: [ValueId; 3],
    helper_cell: &'p GlobalSlotRef,
    leaf_cell: &'p GlobalSlotRef,
    literal: &'p [u8],
    observations: Observations<'p>,
    span: Option<(u32, u32)>,
}

#[derive(Clone, Copy)]
pub(super) struct Snapshot<'p> {
    pub location: Option<i64>,
    pub homes: &'p [(i64, usize)],
}

struct Observations<'p> {
    wrapper_at_leaf: Snapshot<'p>,
    leaf_check: &'p Check,
    leaf_retire: Snapshot<'p>,
    wrapper_check: &'p Check,
    wrapper_retire: Snapshot<'p>,
}

pub(super) struct BoundFrame {
    pub function: ValueId,
    /// Physical caller SSA words in this frame's own formal numbering.
    pub arguments: [ValueId; 3],
}

impl BoundFrame {
    pub(super) fn level(&self, at: Snapshot<'_>, identity: ValueId, present: ValueId) -> Level {
        Level {
            function: self.function,
            identity,
            location: at.location,
            key_space: KeySpace::CodeSlot,
            bindings: at
                .homes
                .iter()
                .map(|&(key, formal)| Binding {
                    key,
                    value: self.arguments[formal],
                    present,
                })
                .collect(),
        }
    }
}

/// State available strictly on a hit. Both frame identities must survive any
/// post-read service until the matching retirement. The original method receiver
/// and callable have independent binding_alias credits at the leaf call. The
/// field needs its physical owned credit across the leaf's pending-service arm.
pub(super) struct ReadState<'p> {
    pub wrapper: BoundFrame,
    pub leaf: BoundFrame,
    pub receiver: ValueId,
    pub row: ValueId,
    pub column: ValueId,
    pub offset: i64,
    pub wrapper_at_leaf: Snapshot<'p>,
    pub leaf_check: &'p Check,
    pub leaf_retire: Snapshot<'p>,
    pub wrapper_check: &'p Check,
    pub wrapper_retire: Snapshot<'p>,
}

impl<'p> Prefix<'p> {
    /// Bind the original pre-inlining call, before any captures are retained
    /// or its result projected into a scalar continuation.
    pub(super) fn prepare(
        summary: &'p PackedWrapper,
        body: &TirFunction,
        op: &TirOp,
    ) -> Option<Self> {
        let call = static_call(op)?;
        if call.callee != summary.wrapper.entry.function
            || op.results.len() != 1
            || body.has_closure_parameter()
        {
            return None;
        }
        let args = call.bind(body)?;
        let arguments: [ValueId; 3] = args.positional.try_into().ok()?;
        let wrapper_function = call.callable?;
        let positions = [summary.receiver, summary.row, summary.column];
        let mut sorted = positions;
        sorted.sort_unstable();
        if sorted != [0, 1, 2]
            || body.exact_param_types.get(summary.receiver) != Some(&TirType::Class(summary.class))
        {
            return None;
        }

        // The summary has already proved all these transports and nonservice
        // checks. They cannot raise on this path: all frames are admitted, the
        // only literal is published, identity is exact and both coordinates fit.
        let actions: Vec<_> = summary
            .events
            .iter()
            .filter(|event| {
                !matches!(
                    event,
                    Event::Transport
                        | Event::Check(super::packed_wrapper::Check {
                            services_pending: false,
                            ..
                        })
                )
            })
            .collect();
        use Event::*;
        let [
            Enter(outer),
            Cell {
                site: first,
                slot: helper_cell,
            },
            Literal {
                site: literal,
                bytes,
            },
            Invoke(helper_call),
            Enter(helper),
            OwnedAlias {
                site: returned,
                source: forwarded,
                ..
            },
            Retire {
                site: helper_retire,
                ..
            },
            Check(helper_check),
            Cell {
                site: last,
                slot: leaf_cell,
            },
            OwnedAlias {
                site: bound_function,
                source: function_source,
                result: held_fn,
                ..
            },
            OwnedAlias {
                site: bound_receiver,
                source: receiver_source,
                result: held_receiver,
                ..
            },
            Invoke(leaf_call),
            Enter(leaf),
            LoadField(field),
            ReadPacked(read),
            Check(leaf_check),
            Retire {
                site: leaf_exit,
                location: leaf_at_exit,
                homes: leaf_homes,
            },
            Check(wrapper_check),
            Retire {
                site: wrapper_exit,
                location: wrapper_at_exit,
                homes: wrapper_homes,
            },
        ] = actions.as_slice()
        else {
            return None;
        };
        let helper_id = summary.identity.entry.function;
        let leaf_id = summary.leaf.entry.function;
        let wrapper_id = summary.wrapper.entry.function;
        if *outer != summary.wrapper.entry
            || *helper != summary.identity.entry
            || *leaf != summary.leaf.entry
            || *helper_call != summary.identity_call.site
            || *leaf_call != summary.leaf_call.site
            || returned.function != helper_id
            || helper_retire.function != helper_id
            || *forwarded != Source::Formal(summary.identity_return_parameter)
            || helper_check.site.function != wrapper_id
            || bound_function.function != wrapper_id
            || bound_receiver.function != wrapper_id
            || field.function != leaf_id
            || read.function != leaf_id
            || leaf_check.site.function != leaf_id
            || leaf_exit.function != leaf_id
            || wrapper_check.site.function != wrapper_id
            || wrapper_exit.function != wrapper_id
            || *function_source != Source::Cell(*last)
            || *receiver_source != Source::CallResult(*helper_call)
            || summary.identity_call.callable.source != Source::Cell(*first)
            || summary.leaf_call.callable.source != *function_source
            || summary.leaf_call.callable.word != *held_fn
            || summary
                .leaf_call
                .arguments
                .get(summary.leaf_formals[0])?
                .word
                != *held_receiver
            || summary.identity_call.arguments.len() != 2
            || summary
                .identity_call
                .arguments
                .iter()
                .enumerate()
                .any(|(i, a)| {
                    if i == summary.identity_return_parameter {
                        a.source != Source::Formal(summary.receiver)
                    } else {
                        a.source != Source::Literal(*literal)
                    }
                })
            || helper_cell == leaf_cell
            || ![helper_cell, leaf_cell]
                .iter()
                .all(|s| s.cell && s.init_once)
            || !leaf_check.payload.is_empty()
            || leaf_check.checked_word.is_some()
            || !wrapper_check.payload.is_empty()
            || wrapper_check.checked_word.is_some()
        {
            return None;
        }
        let observations = Observations {
            wrapper_at_leaf: Snapshot {
                location: summary.leaf_call.location,
                homes: &summary.leaf_call.homes,
            },
            leaf_check,
            leaf_retire: Snapshot {
                location: *leaf_at_exit,
                homes: leaf_homes,
            },
            wrapper_check,
            wrapper_retire: Snapshot {
                location: *wrapper_at_exit,
                homes: wrapper_homes,
            },
        };
        Some(Self {
            summary,
            wrapper_function,
            arguments,
            helper_cell,
            leaf_cell,
            literal: bytes,
            observations,
            span: op.source_span,
        })
    }

    /// `miss` owns the complete original call and its original check. The
    /// continuation must keep the source receiver and wrapper callable alive
    /// through the read and reproduce original custody across an observer.
    pub(super) fn emit(
        self,
        caller: &mut TirFunction,
        miss: BlockId,
        continuation: impl FnOnce(&mut TirFunction, ReadState<'p>) -> BlockId,
    ) -> BlockId {
        let helper = caller.fresh_value();
        let leaf = caller.fresh_value();
        for word in [helper, leaf] {
            caller.value_types.insert(word, TirType::DynBox);
        }
        let p = self.summary;
        let [receiver, row, column] =
            [p.receiver, p.row, p.column].map(|position| self.arguments[position]);
        let mut leaf_arguments = self.arguments;
        for (formal, word) in p.leaf_formals.into_iter().zip([receiver, row, column]) {
            leaf_arguments[formal] = word;
        }
        let obs = self.observations;
        let ready = continuation(
            caller,
            ReadState {
                wrapper: BoundFrame {
                    function: self.wrapper_function,
                    arguments: self.arguments,
                },
                leaf: BoundFrame {
                    function: leaf,
                    arguments: leaf_arguments,
                },
                receiver,
                row,
                column,
                offset: p.offset,
                wrapper_at_leaf: obs.wrapper_at_leaf,
                leaf_check: obs.leaf_check,
                leaf_retire: obs.leaf_retire,
                wrapper_check: obs.wrapper_check,
                wrapper_retire: obs.wrapper_retire,
            },
        );
        // Allocate blocks backwards; along the hit edge each capture happens
        // at the original position, including the leaf load after the first
        // poll. A ready literal read and these immutable cells mint no credit.
        let mut test = |opcode, inputs, attrs: Vec<(&str, AttrValue)>, ops, yes, no| {
            let mut op = TirOp::new(opcode, inputs, vec![caller.fresh_value()]);
            op.source_span = self.span;
            if opcode == OpCode::StringLiteralCellReady {
                op.callee = Some(p.wrapper.entry.function);
            }
            op.attrs
                .extend(attrs.into_iter().map(|(k, v)| (k.to_owned(), v)));
            test_block(caller, ops, op, yes, no)
        };
        let cell = |slot: &GlobalSlotRef, word| {
            let mut op = TirOp::new(OpCode::StaticCellLoad, vec![], vec![word]);
            op.set_global_slot(Some(slot));
            op.source_span = self.span;
            op
        };
        let entry = |frame: &Frame| {
            let mut attrs = vec![
                ("s_value", AttrValue::Str(frame.owner.clone())),
                ("value", AttrValue::Int(frame.shape.entry_value())),
            ];
            attrs.extend(frame.prefix.attrs());
            attrs
        };
        let leaf_entry = test(
            OpCode::FrameEntryAdmits,
            vec![leaf],
            entry(&p.leaf),
            vec![cell(self.leaf_cell, leaf)],
            ready,
            miss,
        );
        let helper_poll = test(
            OpCode::AsyncWorkPending,
            vec![],
            vec![],
            vec![],
            miss,
            leaf_entry,
        );
        let helper_entry = test(
            OpCode::FrameEntryAdmits,
            vec![helper],
            entry(&p.identity),
            vec![],
            helper_poll,
            miss,
        );
        let literal = test(
            OpCode::StringLiteralCellReady,
            vec![],
            vec![("bytes", AttrValue::Bytes(self.literal.to_vec()))],
            vec![cell(self.helper_cell, helper)],
            helper_entry,
            miss,
        );
        let column = test(
            OpCode::IsInlineInt,
            vec![column],
            vec![],
            vec![],
            literal,
            miss,
        );
        let row = test(OpCode::IsInlineInt, vec![row], vec![], vec![], column, miss);
        test(
            OpCode::FrameEntryAdmits,
            vec![self.wrapper_function],
            entry(&p.wrapper),
            vec![],
            row,
            miss,
        )
    }
}

/// Exercise the very same permuted-formal, checked-wrapper fixture as the
/// summary, with one physical residual for every guard.
#[cfg(test)]
pub(super) fn verify_prefix(wrapper: &TirFunction, plan: &PackedWrapper) {
    use crate::tir::blocks::Terminator;

    let mut caller = wrapper.clone();
    let captured = caller.fresh_value();
    let args: Vec<_> = caller.blocks[&caller.entry_block]
        .args
        .iter()
        .map(|p| p.id)
        .collect();
    let mut physical = TirOp::new(
        OpCode::Call,
        std::iter::once(captured)
            .chain(args.iter().copied())
            .collect(),
        vec![caller.fresh_value()],
    );
    physical.origin = Some(molt_ir::OpKind::CallGuarded);
    physical.set_proven_target(true);
    physical.callee = Some(plan.wrapper.entry.function);
    let prefix = Prefix::prepare(plan, wrapper, &physical).expect("checked prefix");
    let miss = caller.fresh_block();
    let hit = caller.fresh_block();
    let mut block = prefix.emit(&mut caller, miss, |_, state| {
        assert_eq!(state.wrapper.function, captured);
        assert_eq!(
            [state.receiver, state.row, state.column],
            [args[2], args[0], args[1]]
        );
        assert_eq!(state.leaf.arguments, [args[1], args[2], args[0]]);
        assert_eq!(
            [
                state.leaf_check.location,
                state.leaf_retire.location,
                state.wrapper_check.location,
                state.wrapper_retire.location
            ],
            [Some(6), Some(4), Some(5), Some(1)]
        );
        hit
    });
    let mut ops = Vec::new();
    let mut slots = Vec::new();
    let mut prefixes = Vec::new();
    while block != hit {
        let b = &caller.blocks[&block];
        for op in &b.ops {
            ops.push(op.opcode);
            if let Some(slot) = op.global_slot() {
                slots.push(slot.index);
            }
            if op.opcode == OpCode::FrameEntryAdmits {
                let prefix = molt_ir::frame_entry::FramePrefix::from_attrs(&op.attrs).unwrap();
                prefixes.push((prefix.homes, prefix.depth));
            }
            if op.opcode == OpCode::StringLiteralCellReady {
                assert_eq!(op.callee, Some(plan.wrapper.entry.function));
                assert_eq!(op.attrs["bytes"], AttrValue::Bytes(vec![237, 160, 128]));
            }
        }
        let Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } = b.terminator
        else {
            panic!("read-only guard")
        };
        if b.ops.last().unwrap().opcode == OpCode::AsyncWorkPending {
            assert_eq!(then_block, miss);
            block = else_block;
        } else {
            assert_eq!(else_block, miss);
            block = then_block;
        }
    }
    use OpCode::*;
    assert_eq!(
        ops,
        [
            FrameEntryAdmits,
            IsInlineInt,
            IsInlineInt,
            StaticCellLoad,
            StringLiteralCellReady,
            FrameEntryAdmits,
            AsyncWorkPending,
            StaticCellLoad,
            FrameEntryAdmits
        ]
    );
    assert_eq!(slots, [7, 20]);
    assert_eq!(prefixes, [(0, 0), (3, 1), (3, 1)]);

    let mut changed = plan.clone();
    let check = changed
        .events
        .iter_mut()
        .find_map(|event| match event {
            Event::Check(check) if !check.services_pending => Some(check),
            _ => None,
        })
        .unwrap();
    check.services_pending = true;
    assert!(Prefix::prepare(&changed, wrapper, &physical).is_none());
}
