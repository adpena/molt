//! Operations of a suspended activation. The saved state selects a resume
//! edge in final TIR. The pending sentinel is a single exact word.

use super::builder::CodegenBuilder;
use super::control::Edges;
use super::function::FunctionCx;
use super::scalar;
use super::value_table::HeldCarrier;
use molt_codegen_abi::word::WordComparison;
use molt_ir::runtime_fn_generated::RuntimeFn;
use molt_ir::tir::blocks::BlockId;
use molt_ir::tir::ops::{AttrValue, TirOp};
use molt_ir::tir::values::ValueId;

pub(crate) fn poll_frame<B: super::task::TaskPlacement>(b: &B) -> Option<B::Word> {
    b.poll_parameter()
        .filter(|&value| b.kind(value) == super::builder::MachineKind::Word)
        .map(|value| b.expect_word(value))
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    pub(crate) fn frame_context_set(&mut self, b: &mut B, op: &TirOp) {
        use super::custody::{CallTarget, RuntimeArg};
        use super::function::ResultCustody;
        let [argument_zero, argument_kind, class_cell] = op.operands.as_slice() else {
            self.fatal("frame_context_set requires three context operands");
            return;
        };
        let callee = b.runtime_callee(RuntimeFn::FrameContextSet);
        let args = [*argument_zero, *argument_kind, *class_cell].map(RuntimeArg::Operand);
        self.runtime_call(
            b,
            op,
            CallTarget::Callee(callee),
            &args,
            ResultCustody::canonical(RuntimeFn::FrameContextSet, args.len()),
            "frame_context_set",
            "frame_context_set",
        );
    }

    /// A raw scalar can alias the sentinel's bit pattern. Poll and wait
    /// require an object whose custody stays explicit in TIR.
    fn activation_object(&self, b: &mut B, op: &TirOp) -> B::Word {
        let &[operand] = op.operands.as_slice() else {
            panic!("{:?} in '{}' expects one object", op.opcode, self.func.name);
        };
        let operands = self.operands();
        assert_eq!(
            operands.held(operand),
            HeldCarrier::Boxed,
            "{:?} in '{}' requires an object carrier",
            op.opcode,
            self.func.name
        );
        let value = operands.resolve(b, operand);
        scalar::word_of(b, value)
    }

    pub(crate) fn is_pending(&mut self, b: &mut B, op: &TirOp) {
        let object = self.activation_object(b, op);
        let sentinel = b.constant(molt_codegen_abi::pending_bits() as u64);
        let pending = b.compare(WordComparison::Equal, object, sentinel);
        let bit = b.bit_value(pending);
        b.set_name(bit, "is_pending");
        if let Some(&result) = op.results.first() {
            self.bind(result, bit, HeldCarrier::RawBool);
        }
    }

    pub(crate) fn task_wait(&self, b: &mut B, op: &TirOp, frame: B::Word) {
        let object = self.activation_object(b, op);
        let frame = b.word_to_address(frame);
        b.set_name(frame, "task_wait_frame");
        let address = b.and(object, molt_codegen_abi::POINTER_MASK);
        let future = b.word_to_address(address);
        b.set_name(future, "task_wait_future");
        let register = b.runtime_callee(RuntimeFn::SleepRegister);
        b.call(register, &[frame, future]);
    }

    pub(crate) fn state_set(&self, b: &mut B, op: &TirOp, frame: B::Word) {
        let Some(AttrValue::Int(state)) = op.attrs.get("value") else {
            panic!("state_set in '{}' requires its state", self.func.name);
        };
        let state = b.constant(*state as u64);
        b.call_runtime(RuntimeFn::ObjSetState, &[frame, state]);
    }
}

impl<B: CodegenBuilder> Edges<'_, B> {
    pub(crate) fn dispatch_state(
        &self,
        b: &mut B,
        source: BlockId,
        frame: B::Word,
        cases: &[(i64, BlockId, Vec<ValueId>)],
        default: (BlockId, &[ValueId]),
        diagnostics: &mut Vec<String>,
    ) {
        let state = b
            .call_runtime(RuntimeFn::ObjGetState, &[frame])
            .expect("a task's saved state is a word");
        b.set_name(b.word_value(state), "state_dispatch_state");
        self.switch(
            b,
            source,
            state,
            cases,
            default,
            "state-dispatch",
            diagnostics,
        );
    }
}
