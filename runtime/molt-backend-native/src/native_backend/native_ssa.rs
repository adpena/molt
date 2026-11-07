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
