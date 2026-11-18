//! Control flow: block parameters and the arguments each branch passes them
//! (design: `docs/design/foundation/codegen_walker.md`, W2, "terminators and
//! block parameters").
//!
//! A TIR block argument is a block parameter of the backend's block, the
//! form Cranelift has natively and LLVM builds as a phi. A branch converts each
//! argument to its parameter's carrier where it is emitted, in the branching
//! block before the branch: boxing a raw value for a boxed parameter, unboxing
//! the reverse, and the machine conversion between the two kinds. So no
//! backend inserts code before an existing terminator, and the conversion is
//! written once.

use molt_ir::deterministic_hash::HashMap;

use super::builder::{CodegenBuilder, MachineKind};
use super::scalar;
use super::value_table::{HeldCarrier, ValueTable};
use crate::representation_plan::ProducerCarrierPlan;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// The machine kind a value held as `carrier` has: a raw float is a float, a
/// raw bool a bit, and every other carrier one word.
pub(crate) fn carrier_kind(carrier: HeldCarrier) -> MachineKind {
    match carrier {
        HeldCarrier::RawF64 => MachineKind::Float,
        HeldCarrier::RawBool => MachineKind::Bit,
        HeldCarrier::RawI64 | HeldCarrier::Never | HeldCarrier::Boxed => MachineKind::Word,
    }
}

/// `value`, held as `source`, converted for a destination of type `target`
/// (a block parameter, a return, a function parameter): boxed for a boxed
/// destination, unboxed for a raw one, then in the destination's machine
/// kind. It adds no control flow, so a backend may place it anywhere in a
/// block: a raw integer without an inline-safe proof boxes through the
/// runtime, which mints the heap box the destination then owns.
/// `None` when no conversion exists between the two machine kinds; the
/// lowering is then refused.
pub(crate) fn coerce<B: CodegenBuilder>(
    b: &mut B,
    plan: &ProducerCarrierPlan,
    value: B::Value,
    source: HeldCarrier,
    source_id: Option<ValueId>,
    target: &TirType,
) -> Option<B::Value> {
    let target_carrier = HeldCarrier::of(target);
    let source_type = source.machine_type();
    let value = if source_type == *target {
        value
    } else if target_carrier == HeldCarrier::Boxed && source != HeldCarrier::Boxed {
        let word = if source == HeldCarrier::RawI64 {
            let raw = scalar::word_of(b, value);
            if source_id.is_some_and(|id| plan.is_inline_safe_int(id)) {
                molt_codegen_abi::boxed_word::generate_box_inline_int(b, raw)
            } else {
                scalar::box_int_by_runtime(b, raw)
            }
        } else {
            scalar::box_value(b, value, &source_type)
        };
        b.word_value(word)
    } else if target_carrier != HeldCarrier::Boxed && source == HeldCarrier::Boxed {
        scalar::unbox_value(b, value, target)
    } else {
        value
    };
    in_kind(b, value, carrier_kind(target_carrier))
}

/// `value` in machine kind `kind`, its bits preserved: a narrower integer
/// zero-extends, a word truncates to its low bit, a float and a word
/// reinterpret each other, an address becomes its word.
fn in_kind<B: CodegenBuilder>(b: &mut B, value: B::Value, kind: MachineKind) -> Option<B::Value> {
    let have = b.kind(value);
    if have == kind {
        return Some(value);
    }
    Some(match (have, kind) {
        (MachineKind::Bit | MachineKind::NarrowInt, MachineKind::Word) => {
            let word = b.zero_extend(value);
            b.word_value(word)
        }
        (MachineKind::Word, MachineKind::Bit) => {
            let word = b.expect_word(value);
            let bit = b.low_bit(word);
            b.bit_value(bit)
        }
        (MachineKind::Float, MachineKind::Word) => {
            let word = b.float_to_bits(value);
            b.word_value(word)
        }
        (MachineKind::Word, MachineKind::Float) => {
            let word = b.expect_word(value);
            b.bits_to_float(word)
        }
        (MachineKind::Address, MachineKind::Word) => {
            let word = b.address_to_word(value);
            b.word_value(word)
        }
        _ => return None,
    })
}

/// What a branch reads: TIR, the carrier plan, the lowered values and the
/// backend block of each TIR block.
pub(crate) struct Edges<'a, B: CodegenBuilder> {
    pub(crate) func: &'a TirFunction,
    pub(crate) plan: &'a ProducerCarrierPlan,
    pub(crate) values: &'a ValueTable<B::Value>,
    pub(crate) blocks: &'a HashMap<BlockId, B::Block>,
}

impl<B: CodegenBuilder> Edges<'_, B> {
    /// The type `target`'s parameters hold: the plan's carrier for each.
    pub(crate) fn param_types(&self, target: BlockId) -> Vec<TirType> {
        self.func.blocks[&target]
            .args
            .iter()
            .map(|param| self.plan.effective_value_type(param.id, &param.ty))
            .collect()
    }

    /// The arguments an edge from `source` passes to `target`'s parameters,
    /// each converted to its parameter's carrier here, before the branch. A
    /// missing or unlowered argument is a fatal diagnostic; a placeholder
    /// keeps the function well formed for the next one.
    pub(crate) fn arguments(
        &self,
        b: &mut B,
        source: BlockId,
        edge: &str,
        target: BlockId,
        args: &[ValueId],
        diagnostics: &mut Vec<String>,
    ) -> Vec<B::Value> {
        let mut passed = Vec::with_capacity(args.len());
        for (index, ty) in self.param_types(target).iter().enumerate() {
            let kind = carrier_kind(HeldCarrier::of(ty));
            let Some(&arg) = args.get(index) else {
                diagnostics.push(format!(
                    "predecessor block {source:?} {edge} branches to {target:?} with {} argument(s), but phi argument index {index} is required",
                    args.len()
                ));
                passed.push(b.placeholder(kind));
                continue;
            };
            let Some(value) = self.values.value(arg) else {
                diagnostics.push(format!(
                    "predecessor block {source:?} passes undefined ValueId %{} to phi argument {index} in block {target:?}",
                    arg.0
                ));
                passed.push(b.placeholder(kind));
                continue;
            };
            let held = self.values.repr(arg).unwrap_or(HeldCarrier::Boxed);
            let converted = coerce(b, self.plan, value, held, Some(arg), ty).unwrap_or_else(|| {
                diagnostics.push(format!(
                    "no conversion of %{} from {held:?} to block {target:?} parameter {index} ({ty:?})",
                    arg.0
                ));
                b.placeholder(kind)
            });
            passed.push(converted);
        }
        passed
    }

    /// Lower a branch terminator (`Branch`, `CondBranch`, `Switch`,
    /// `Unreachable`). Returns false for a terminator the caller lowers
    /// (`Return`, `StateDispatch`), which reads frame state.
    pub(crate) fn terminator(
        &self,
        b: &mut B,
        source: BlockId,
        terminator: &Terminator,
        diagnostics: &mut Vec<String>,
    ) -> bool {
        match terminator {
            Terminator::Branch { target, args } => {
                let args = self.arguments(b, source, "branch", *target, args, diagnostics);
                b.jump(self.blocks[target], &args);
            }
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let condition = match self.values.value(*cond) {
                    Some(value) => {
                        let held = self.values.repr(*cond).unwrap_or(HeldCarrier::Boxed);
                        scalar::truth(b, value, held)
                    }
                    None => {
                        diagnostics.push(format!(
                            "ValueId %{} was used before being defined during lowering",
                            cond.0
                        ));
                        let placeholder = b.placeholder(MachineKind::Bit);
                        b.expect_bit(placeholder)
                    }
                };
                let yes =
                    self.arguments(b, source, "then-edge", *then_block, then_args, diagnostics);
                let no =
                    self.arguments(b, source, "else-edge", *else_block, else_args, diagnostics);
                b.branch_if(
                    condition,
                    (self.blocks[then_block], &yes),
                    (self.blocks[else_block], &no),
                );
            }
            Terminator::Switch {
                value,
                cases,
                default,
                default_args,
            } => {
                let selector = match self.values.value(*value) {
                    Some(selector) => scalar::word_of(b, selector),
                    None => {
                        diagnostics.push(format!(
                            "ValueId %{} was used before being defined during lowering",
                            value.0
                        ));
                        b.constant(0)
                    }
                };
                self.switch(
                    b,
                    source,
                    selector,
                    cases,
                    (*default, default_args),
                    "switch",
                    diagnostics,
                );
            }
            Terminator::Unreachable => b.unreachable(),
            Terminator::Return { .. } | Terminator::StateDispatch { .. } => return false,
        }
        true
    }

    /// A switch on `selector` whose edges pass arguments: a TIR `Switch`,
    /// or a generator's state dispatch once the caller read its state word.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn switch(
        &self,
        b: &mut B,
        source: BlockId,
        selector: B::Word,
        cases: &[(i64, BlockId, Vec<ValueId>)],
        default: (BlockId, &[ValueId]),
        edge: &str,
        diagnostics: &mut Vec<String>,
    ) {
        let default_args = self.arguments(b, source, edge, default.0, default.1, diagnostics);
        let cases: Vec<(i64, B::Block, Vec<B::Value>)> = cases
            .iter()
            .map(|(value, target, args)| {
                let args = self.arguments(b, source, edge, *target, args, diagnostics);
                (*value, self.blocks[target], args)
            })
            .collect();
        b.switch(selector, &cases, (self.blocks[&default.0], &default_args));
    }
}

#[cfg(test)]
mod tests {
    //! Edges on the interpreting builder. The oracle is the NaN-box encoding
    //! and Python's value of each argument: a parameter receives the value
    //! its argument holds, in the parameter's carrier.

    use super::super::scalar::tests::{Edge, Interpreter, Value};
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::values::TirValue;
    use molt_ir::repr::Repr;

    /// An inline int's word, written out by hand: a quiet NaN, tag 1 at bit
    /// 48, the 47-bit two's-complement payload.
    fn inline(value: i64) -> u64 {
        (0x7ff8 << 48) | (1 << 48) | ((value as u64) & ((1 << 47) - 1))
    }

    fn plan(facts: &[(ValueId, Repr)]) -> ProducerCarrierPlan {
        let mut plan = ProducerCarrierPlan::default();
        plan.repr_by_value.extend(facts.iter().copied());
        plan
    }

    /// A function whose block 1 takes one parameter of each listed
    /// declared type, numbered from 100.
    fn target_with(params: &[TirType]) -> (TirFunction, BlockId) {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Value,
        );
        let target = func.fresh_block();
        func.blocks.insert(
            target,
            TirBlock {
                id: target,
                args: params
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| TirValue {
                        id: ValueId(100 + i as u32),
                        ty: ty.clone(),
                    })
                    .collect(),
                ops: Vec::new(),
                terminator: Terminator::Unreachable,
            },
        );
        (func, target)
    }

    #[test]
    fn an_edge_converts_each_argument_to_its_parameters_carrier() {
        // Parameters: a boxed word, a raw int, a raw bool, a boxed word.
        let (func, target) = target_with(&[
            TirType::DynBox,
            TirType::I64,
            TirType::Bool,
            TirType::DynBox,
        ]);
        let plan = plan(&[
            (ValueId(100), Repr::DynBox),
            (ValueId(101), Repr::RawI64Safe),
            (ValueId(102), Repr::Bool),
            (ValueId(103), Repr::DynBox),
            (ValueId(1), Repr::RawI64Safe),
        ]);
        let mut b = Interpreter::default();
        let block = b.block();
        let mut blocks = HashMap::default();
        blocks.insert(target, block);
        for ty in plan_types(&func, &plan, target) {
            b.append_block_param(block, carrier_kind(HeldCarrier::of(&ty)));
        }
        let mut values: ValueTable<Value> = ValueTable::with_capacity(4);
        // %1: a raw int the plan proves inline-safe; %2: a boxed inline int;
        // %3: a raw float.
        values.set_value(ValueId(1), Value::Word(7));
        values.set_repr(ValueId(1), HeldCarrier::RawI64);
        values.set_value(ValueId(2), Value::Word(inline(-2)));
        values.set_repr(ValueId(2), HeldCarrier::Boxed);
        values.set_value(ValueId(3), Value::Float(1.5));
        values.set_repr(ValueId(3), HeldCarrier::RawF64);
        let edges = Edges::<Interpreter> {
            func: &func,
            plan: &plan,
            values: &values,
            blocks: &blocks,
        };
        let mut diagnostics = Vec::new();
        let args = [ValueId(1), ValueId(2), ValueId(2), ValueId(3)];
        let passed = edges.arguments(
            &mut b,
            BlockId(0),
            "branch",
            target,
            &args,
            &mut diagnostics,
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            passed,
            [
                // The proven raw 7 boxes inline, with no runtime call.
                Value::Word(inline(7)),
                // The boxed -2 decodes to its raw payload.
                Value::Word(-2i64 as u64),
                // Its truth for the raw bool parameter is its low bit.
                Value::Bit(false),
                // A float is its own boxed word.
                Value::Word(1.5f64.to_bits()),
            ]
        );
        assert_eq!(
            b.runtime_calls,
            [molt_ir::runtime_fn_generated::RuntimeFn::IntAsI64]
        );
    }

    fn plan_types(func: &TirFunction, plan: &ProducerCarrierPlan, target: BlockId) -> Vec<TirType> {
        let values = ValueTable::<Value>::with_capacity(0);
        let blocks = HashMap::default();
        Edges::<Interpreter> {
            func,
            plan,
            values: &values,
            blocks: &blocks,
        }
        .param_types(target)
    }

    #[test]
    fn an_unproven_raw_int_boxes_through_the_runtime_for_a_boxed_parameter() {
        // A full-range raw int may leave the inline window: the runtime mints
        // the heap int, which the parameter then owns.
        let (func, target) = target_with(&[TirType::DynBox]);
        let plan = plan(&[(ValueId(100), Repr::DynBox)]);
        let mut b = Interpreter::default();
        let block = b.block();
        b.append_block_param(block, MachineKind::Word);
        let mut blocks = HashMap::default();
        blocks.insert(target, block);
        let mut values: ValueTable<Value> = ValueTable::with_capacity(2);
        values.set_value(ValueId(1), Value::Word(1 << 50));
        values.set_repr(ValueId(1), HeldCarrier::RawI64);
        let edges = Edges::<Interpreter> {
            func: &func,
            plan: &plan,
            values: &values,
            blocks: &blocks,
        };
        let mut diagnostics = Vec::new();
        let terminator = Terminator::Branch {
            target,
            args: vec![ValueId(1)],
        };
        assert!(edges.terminator(&mut b, BlockId(0), &terminator, &mut diagnostics));
        assert!(diagnostics.is_empty());
        assert_eq!(
            b.runtime_calls,
            [molt_ir::runtime_fn_generated::RuntimeFn::IntFromI64]
        );
        let [Edge::Jump(to, passed)] = b.edges.as_slice() else {
            panic!("one jump: {:?}", b.edges);
        };
        assert_eq!(*to, block);
        assert_eq!(passed.len(), 1);
    }

    #[test]
    fn a_missing_or_unlowered_argument_is_a_fatal_diagnostic() {
        let (func, target) = target_with(&[TirType::DynBox, TirType::DynBox]);
        let plan = plan(&[(ValueId(100), Repr::DynBox), (ValueId(101), Repr::DynBox)]);
        let mut b = Interpreter::default();
        let block = b.block();
        b.append_block_param(block, MachineKind::Word);
        b.append_block_param(block, MachineKind::Word);
        let mut blocks = HashMap::default();
        blocks.insert(target, block);
        let values: ValueTable<Value> = ValueTable::with_capacity(2);
        let edges = Edges::<Interpreter> {
            func: &func,
            plan: &plan,
            values: &values,
            blocks: &blocks,
        };
        let mut diagnostics = Vec::new();
        // One argument for two parameters, and that one never lowered.
        let passed = edges.arguments(
            &mut b,
            BlockId(0),
            "branch",
            target,
            &[ValueId(1)],
            &mut diagnostics,
        );
        assert_eq!(passed.len(), 2, "a placeholder keeps the edge well formed");
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics[0].contains("undefined ValueId %1"));
        assert!(diagnostics[1].contains("phi argument index 1 is required"));
    }

    #[test]
    fn a_conditional_branch_tests_its_condition_and_passes_each_edges_arguments() {
        let (mut func, then_block) = target_with(&[TirType::I64]);
        let else_block = func.fresh_block();
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: Vec::new(),
                ops: Vec::new(),
                terminator: Terminator::Unreachable,
            },
        );
        let plan = plan(&[(ValueId(100), Repr::RawI64Safe)]);
        let mut b = Interpreter::default();
        let (yes, no) = (b.block(), b.block());
        b.append_block_param(yes, MachineKind::Word);
        let mut blocks = HashMap::default();
        blocks.insert(then_block, yes);
        blocks.insert(else_block, no);
        let mut values: ValueTable<Value> = ValueTable::with_capacity(3);
        values.set_value(ValueId(1), Value::Bit(true));
        values.set_repr(ValueId(1), HeldCarrier::RawBool);
        values.set_value(ValueId(2), Value::Word(41));
        values.set_repr(ValueId(2), HeldCarrier::RawI64);
        let edges = Edges::<Interpreter> {
            func: &func,
            plan: &plan,
            values: &values,
            blocks: &blocks,
        };
        let mut diagnostics = Vec::new();
        let terminator = Terminator::CondBranch {
            cond: ValueId(1),
            then_block,
            then_args: vec![ValueId(2)],
            else_block,
            else_args: vec![],
        };
        assert!(edges.terminator(&mut b, BlockId(0), &terminator, &mut diagnostics));
        assert!(diagnostics.is_empty());
        assert_eq!(
            b.edges,
            [Edge::Branch(
                true,
                (yes, vec![Value::Word(41)]),
                (no, vec![])
            )]
        );
        // A return reads frame state: the caller lowers it.
        let ret = Terminator::Return { values: vec![] };
        assert!(!edges.terminator(&mut b, BlockId(0), &ret, &mut diagnostics));
    }
}
