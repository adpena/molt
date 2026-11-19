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

    /// A compiled function's linkage row owns its machine parameter list. An
    /// operation's `value` is a Python-level arity claim that may be absent (a
    /// task poll entry registers with none) and is checked only where present;
    /// a missing row still fails closed inside `ensure_function_symbol`.
    pub(super) fn ensure_linked_function_symbol(
        &self,
        name: &str,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let linkage_arity = self
            .backend
            .function_linkage_abis
            .get(name)
            .map(|abi| {
                abi.param_types
                    .len()
                    .saturating_sub(usize::from(has_closure))
            })
            .unwrap_or(0);
        self.ensure_function_symbol(name, linkage_arity, has_closure)
    }
}
