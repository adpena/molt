//! Native reads share the canonical SimpleIR definition/dominance authority.
use super::*;
use cranelift_frontend::FunctionBuilder as CraneliftFunctionBuilder;

pub(crate) struct FunctionBuilder<'a> {
    inner: CraneliftFunctionBuilder<'a>,
    current_op: Option<usize>,
    ssa_values: Option<function_compiler::NativeSsaValues>,
}

impl<'a> std::ops::Deref for FunctionBuilder<'a> {
    type Target = CraneliftFunctionBuilder<'a>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for FunctionBuilder<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<'a> FunctionBuilder<'a> {
    pub(crate) fn new(func: &'a mut Function, ctx: &'a mut FunctionBuilderContext) -> Self {
        Self {
            inner: CraneliftFunctionBuilder::new(func, ctx),
            current_op: None,
            ssa_values: None,
        }
    }
    pub(super) fn install_ssa_values(&mut self, values: function_compiler::NativeSsaValues) {
        self.ssa_values = Some(values);
    }
    pub(crate) fn begin_op(&mut self, index: usize) {
        self.current_op = Some(index);
    }
    pub(crate) fn end_op(&mut self) {
        self.current_op = None;
    }
    pub(crate) fn def_var(&mut self, var: Variable, value: Value) {
        #[cfg(debug_assertions)]
        if let Some(captured) = self
            .ssa_values
            .as_ref()
            .and_then(|values| values.captured_value(var))
        {
            debug_assert_eq!(
                self.inner.func.dfg.resolve_aliases(value),
                self.inner.func.dfg.resolve_aliases(captured),
                "captured native SSA variable {var:?} redefined at {:?}; missing mutable exclusion or explicit rebind boundary",
                self.current_op,
            );
        }
        self.inner.def_var(var, value);
    }
    /// Semantic incoming transport can rebind a carrier even when its original
    /// definition does not dominate this block. Only actual block parameters
    /// enter through this boundary; ordinary native writes use `def_var`.
    pub(crate) fn bind_var_to_block_param(&mut self, var: Variable, block: Block, index: usize) {
        debug_assert_eq!(self.inner.current_block(), Some(block));
        let value = self.inner.block_params(block)[index];
        self.inner.def_var(var, value);
    }
    /// One typed read boundary for semantic and internal Variables alike.
    pub(crate) fn use_var(&mut self, var: Variable) -> Value {
        if let Some(value) = self
            .current_op
            .and_then(|op| self.ssa_values.as_ref()?.read(var, op))
        {
            return value;
        }
        self.inner.use_var(var)
    }
    pub(crate) fn canonical_ssa_value(&self, id: u32) -> Option<Value> {
        self.ssa_values.as_ref()?.emitted(id)
    }
    pub(crate) fn capture_materialized_phi(&mut self, var: Variable) {
        if let Some(mut values) = self.ssa_values.take() {
            values.capture_materialized_phi(self, var);
            self.ssa_values = Some(values);
        }
    }
    pub(crate) fn capture_operation(
        &mut self,
        index: usize,
        op: &OpIR,
        names: &crate::tir::cfg_liveness::SimpleNameTable,
    ) {
        if let Some(mut values) = self.ssa_values.take() {
            // Capture the final value after internal diamonds join.
            values.capture_operation(self, index, op, names);
            self.ssa_values = Some(values);
        }
    }
    pub(crate) fn finalize(self, config: cranelift_codegen::isa::TargetFrontendConfig) {
        self.inner.finalize(config);
    }
}
