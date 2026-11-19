use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
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
