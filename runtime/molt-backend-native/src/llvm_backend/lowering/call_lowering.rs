use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn ensure_function_symbol(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let requested_arity = arity + usize::from(has_closure);
        let linkage_abi = self
            .backend
            .function_linkage_abis
            .get(name)
            .unwrap_or_else(|| {
                panic!(
                    "LLVM function symbol `{name}` has no exact native linkage ABI; refusing to invent one"
                )
            });
        assert_eq!(
            requested_arity,
            linkage_abi.param_types.len(),
            "LLVM caller/linkage arity mismatch for `{name}`"
        );
        crate::llvm_backend::lowering::linked_compiled_function(self.backend, name)
    }
}
