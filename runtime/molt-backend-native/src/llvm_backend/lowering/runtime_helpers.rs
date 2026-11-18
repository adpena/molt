use super::*;
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Lower a preserved op through its admitted boxed runtime ABI
    /// (`FunctionCx::preserved_runtime_call`).
    pub(super) fn try_lower_preserved_runtime_call(&mut self, op: &TirOp, kind: OpKind) -> bool {
        let (mut cx, mut b) = self.walker();
        cx.preserved_runtime_call(&mut b, op, kind)
    }

    /// Bind a transferred object owner, or retire it when the op discards it.
    /// Dedicated mixed-ABI calls use this only after establishing ownership;
    /// a borrowed runtime result must be explicitly retained first.
    pub(super) fn bind_owned_runtime_result(&mut self, op: &TirOp, result: BasicValueEnum<'ctx>) {
        let (mut cx, mut b) = self.walker();
        cx.bind_owned(&mut b, op, result);
    }

    /// `codegen_ssa::calls::with_callargs` for a consumer that is still LLVM's.
    pub(super) fn with_callargs(
        &mut self,
        args: &[inkwell::values::IntValue<'ctx>],
        consume: impl FnOnce(
            &mut Self,
            inkwell::values::IntValue<'ctx>,
        ) -> inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let mut builder = None;
        crate::codegen_ssa::calls::with_callargs(&mut self.codegen(), args, |_, word| {
            builder = Some(word);
            word
        });
        consume(&mut *self, builder.expect("the CallArgs builder"))
    }

    pub(super) fn emit_call_bind_runtime(
        &mut self,
        callable: ValueId,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let (cx, mut b) = self.walker();
        cx.call_bind_runtime(&mut b, callable, arg_ids).into()
    }

    pub(super) fn emit_call_func_runtime(
        &mut self,
        callable: ValueId,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let (cx, mut b) = self.walker();
        cx.call_func_runtime(&mut b, callable, arg_ids).into()
    }

    pub(super) fn emit_call_func_owned_runtime(&mut self, op: &TirOp) -> BasicValueEnum<'ctx> {
        let (cx, mut b) = self.walker();
        cx.call_func_owned_runtime(&mut b, op).into()
    }

    pub(super) fn emit_call_func_or_bind_runtime(
        &mut self,
        callable: ValueId,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let (cx, mut b) = self.walker();
        cx.call_func_or_bind_runtime(&mut b, callable, arg_ids)
            .into()
    }

    pub(super) fn next_call_site_bits(&mut self, lane: &str) -> inkwell::values::IntValue<'ctx> {
        let (cx, mut b) = self.walker();
        cx.next_call_site(&mut b, lane)
    }

    /// Compiled polls receive the attached context and one raw frame address,
    /// independent of Python-visible parameter names and constructor arity.
    pub(super) fn poll_frame_address(&self) -> inkwell::values::IntValue<'ctx> {
        assert_eq!(
            self.llvm_fn.count_params(),
            2,
            "compiled poll requires an execution context and one frame parameter"
        );
        self.llvm_fn
            .get_nth_param(1)
            .expect("compiled poll requires its frame address")
            .into_int_value()
    }
}

// Declared legality is a compile-time fact: LLVM lowers every staged call
// through its generic entrypoint.
const _: () = assert!(
    molt_ir::runtime_calls::RuntimeCall::all_runtime_realized(
        molt_ir::tir::target_info::TargetKind::Llvm
    ),
    "a staged runtime call is not runtime-realized on LLVM"
);
