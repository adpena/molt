//! Module globals (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "global slots"; call_anatomy.md 4.5): compiled global slot reads, static
//! cells and closed imports, written once. The meaning of each read and
//! store is a shared `molt_codegen_abi` generator, placed by the backend as
//! a site body.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::global_slot::GLOBAL_SLOT_BASE_SHIFT;
use molt_codegen_abi::static_cell::generate_static_cell_load;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, IntBinary, SiteBody};
use super::custody::{CallTarget, RuntimeArg};
use super::function::{FunctionCx, ResultCustody};
use super::scalar;
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// The boxed word of `None`.
const NONE_WORD: u64 = nanbox::QNAN | nanbox::TAG_NONE;

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// Lower a module-global op: `StaticCellBase`, `StaticCellLoad`,
    /// `StaticCellSet`, `GlobalSlotBase`, `GlobalSlotGet` or
    /// `ModuleImportClosed`.
    pub(crate) fn global_op(&mut self, b: &mut B, op: &TirOp) {
        match op.opcode {
            OpCode::StaticCellBase => self.static_cell_base(b, op),
            OpCode::StaticCellLoad => self.static_cell_load(b, op),
            OpCode::StaticCellSet => self.static_cell_set(b, op),
            OpCode::GlobalSlotBase => {
                let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::GlobalSlotBase));
                self.runtime_call(
                    b,
                    op,
                    callee,
                    &[RuntimeArg::Operand(op.operands[0])],
                    ResultCustody::Unowned,
                    "global_slot_base",
                    RuntimeFn::GlobalSlotBase.symbol(),
                );
            }
            OpCode::GlobalSlotGet => self.global_slot_get(b, op),
            OpCode::ModuleImportClosed => self.module_import_closed(b, op),
            other => unreachable!("{other:?} is not a module-global op"),
        }
    }

    /// `id`'s word as a site body takes it.
    fn word(&self, b: &mut B, id: ValueId) -> B::Word {
        let value = self.operands().resolve(b, id);
        scalar::word_of(b, value)
    }

    /// `StaticCellBase`: the module's cell block, a link-time constant
    /// address, as the boxed inline int `GlobalSlotGet` takes for its base.
    fn static_cell_base(&mut self, b: &mut B, op: &TirOp) {
        let Some(AttrValue::Str(module)) = op.attrs.get("s_value") else {
            self.fatal("StaticCellBase names no module");
            return;
        };
        let block = b.static_cell_block(module);
        let address = b.address_to_word(block);
        let named = b.word_value(address);
        b.set_name(named, "static_cells");
        let shift = b.constant(u64::from(GLOBAL_SLOT_BASE_SHIFT));
        let shifted = b.int_binary(IntBinary::ShiftRightLogical, address, shift, false);
        let payload = b.and(shifted, nanbox::INT_MASK);
        let base = b.or(payload, nanbox::QNAN | nanbox::TAG_INT);
        let base = b.word_value(base);
        b.set_name(base, "static_cells_base");
        // A boxed inline int, as `GlobalSlotBase` returns: a call that takes
        // it (the init's `molt_static_cells_bind`) passes it as is.
        self.bind(op.results[0], base, HeldCarrier::of(&TirType::DynBox));
    }

    /// `StaticCellLoad`: an init-dominated read (HF-846), one load from the
    /// module's block at the cell's offset. The cell keeps its reference.
    fn static_cell_load(&mut self, b: &mut B, op: &TirOp) {
        let Some(slot) = op.global_slot() else {
            self.fatal("StaticCellLoad carries no slot");
            return;
        };
        let block = b.static_cell_block(&slot.module);
        let word = generate_static_cell_load(slot.index, |offset| b.load_word(block, offset));
        let value = b.word_value(word);
        b.set_name(value, "static_cell_word");
        self.bind(op.results[0], value, HeldCarrier::of(&TirType::DynBox));
    }

    /// `StaticCellSet` [base, value]: the shared store body for its cell.
    /// The cell holds a boxed word, so a raw carrier is boxed under the
    /// borrowed-operand custody every runtime argument takes.
    fn static_cell_set(&mut self, b: &mut B, op: &TirOp) {
        let Some(slot) = op.global_slot() else {
            self.fatal("StaticCellSet carries no slot");
            return;
        };
        let admission = b.refcount_admission();
        let base = self.word(b, op.operands[0]);
        let view = self.operands();
        let mut custody = view.begin(b, &[op.operands[1]], "static_cell_set");
        let value = view.operand(b, &mut custody, op.operands[1]);
        let args = [
            b.word_value(base),
            b.word_value(value),
            b.word_value(admission),
        ];
        b.site(SiteBody::StaticCellStore { index: slot.index }, &args);
        let none = b.constant(NONE_WORD);
        view.finish(b, custody, none, "static_cell_set_result", |_| {});
    }

    /// `GlobalSlotGet` [base, module, name]: the slot read body for its
    /// index; an unbound slot takes the by-name read.
    fn global_slot_get(&mut self, b: &mut B, op: &TirOp) {
        let Some(slot) = op.global_slot() else {
            self.fatal("GlobalSlotGet carries no slot");
            return;
        };
        let admission = b.refcount_admission();
        let mut args: Vec<B::Value> = op.operands[..3]
            .iter()
            .map(|&id| {
                let word = self.word(b, id);
                b.word_value(word)
            })
            .collect();
        args.push(b.word_value(admission));
        let value = b.site(SiteBody::GlobalSlotRead { index: slot.index }, &args);
        b.set_name(value, "global_slot");
        self.bind_owned(b, op, value);
    }

    /// `ModuleImportClosed` [boxed id]: the shared closed-import body. The
    /// result is owned.
    fn module_import_closed(&mut self, b: &mut B, op: &TirOp) {
        let admission = b.refcount_admission();
        let id = self.word(b, op.operands[0]);
        let args = [b.word_value(id), b.word_value(admission)];
        let value = b.site(SiteBody::ModuleImportClosed, &args);
        b.set_name(value, "module");
        self.bind_owned(b, op, value);
    }
}

#[cfg(test)]
mod tests {
    //! The module-global ops whose meaning is not a site body, on both
    //! machines; the site bodies are the shared generators that Cranelift's
    //! inline expansion and LLVM's helpers run.

    use std::cell::{Cell, RefCell};

    use super::super::differential::{both, every_runtime_entry, kernel};
    use super::super::scalar::tests::Value;
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};

    #[test]
    fn a_global_slot_base_borrows_its_activation_word_from_the_runtime() {
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::GlobalSlotBase,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs: AttrDict::default(),
            source_span: None,
            origin: None,
            runtime: None,
            callee: None,
        };
        let lower = kernel!(op: TirOp = op; |b, inputs| -> () {
            let func = TirFunction::new(
                "globals".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(2);
            values.set_value(ValueId(0), inputs[0]);
            values.set_repr(ValueId(0), HeldCarrier::Boxed);
            let diagnostics = RefCell::new(Vec::new());
            let runtime = every_runtime_entry();
            let sites = Cell::new(0);
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &runtime,
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &sites,
            };
            cx.global_op(b, op);
            ((), vec![values.value(ValueId(1)).unwrap()])
        });
        let module = (0x7ff8 << 48) | (4 << 48) | 0x5000;
        let run = both(&lower, &[Value::Word(module)]);
        assert_eq!(run.runtime_calls, [RuntimeFn::GlobalSlotBase]);
        assert_eq!(
            run.results,
            [Value::Word(super::super::scalar::tests::literal_word(
                RuntimeFn::GlobalSlotBase,
                &module.to_le_bytes()
            ))]
        );
    }
}
