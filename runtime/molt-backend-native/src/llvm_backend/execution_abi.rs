//! Physical execution ABI projection. Logical arity and linkage facts exclude
//! the hidden context; declarations and calls cross that boundary here.

use inkwell::builder::{Builder, BuilderError};
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, FunctionType};
use inkwell::values::{BasicMetadataValueEnum, CallSiteValue, FunctionValue};
use molt_ir::runtime_callable_abi_generated::{
    ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX, RuntimeExecutionContext, compiled_execution_symbol,
    runtime_execution_abi,
};

pub(crate) fn execution_symbol(logical: &str) -> String {
    runtime_execution_abi(logical)
        .map(|abi| abi.physical_symbol.to_owned())
        .unwrap_or_else(|| compiled_execution_symbol(logical))
}

pub(crate) fn prepend_execution_context<'ctx>(logical: FunctionType<'ctx>) -> FunctionType<'ctx> {
    let context = logical.get_context();
    let mut params: Vec<BasicMetadataTypeEnum<'ctx>> =
        Vec::with_capacity(logical.count_param_types() as usize + 1);
    params.push(context.i64_type().into());
    params.extend(
        logical
            .get_param_types()
            .into_iter()
            .map(BasicMetadataTypeEnum::from),
    );
    match logical.get_return_type() {
        Some(result) => result.fn_type(&params, logical.is_var_arg()),
        None => context.void_type().fn_type(&params, logical.is_var_arg()),
    }
}

pub(crate) fn runtime_execution_type<'ctx>(
    name: &str,
    logical: FunctionType<'ctx>,
) -> FunctionType<'ctx> {
    let abi = runtime_execution_abi(name)
        .unwrap_or_else(|| panic!("LLVM runtime import `{name}` has no execution ABI"));
    match abi.context {
        RuntimeExecutionContext::Attached => prepend_execution_context(logical),
        RuntimeExecutionContext::Boundary => logical,
    }
}

pub(crate) fn name_execution_context(function: FunctionValue<'_>) {
    function
        .get_first_param()
        .expect("attached function has a context parameter")
        .into_int_value()
        .set_name("execution_context");
}

pub(crate) trait ExecutionModuleExt<'ctx> {
    fn get_execution_function(&self, logical: &str) -> Option<FunctionValue<'ctx>>;
    fn add_compiled_function(
        &self,
        logical: &str,
        signature: FunctionType<'ctx>,
        linkage: Option<Linkage>,
    ) -> FunctionValue<'ctx>;
}

impl<'ctx> ExecutionModuleExt<'ctx> for Module<'ctx> {
    fn get_execution_function(&self, logical: &str) -> Option<FunctionValue<'ctx>> {
        self.get_function(&execution_symbol(logical))
    }

    fn add_compiled_function(
        &self,
        logical: &str,
        signature: FunctionType<'ctx>,
        linkage: Option<Linkage>,
    ) -> FunctionValue<'ctx> {
        let function = self.add_function(
            &compiled_execution_symbol(logical),
            prepend_execution_context(signature),
            linkage,
        );
        name_execution_context(function);
        function
    }
}

pub(crate) trait ExecutionBuilderExt<'ctx> {
    fn build_execution_call(
        &self,
        function: FunctionValue<'ctx>,
        logical_args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<CallSiteValue<'ctx>, BuilderError>;
}

impl<'ctx> ExecutionBuilderExt<'ctx> for Builder<'ctx> {
    fn build_execution_call(
        &self,
        function: FunctionValue<'ctx>,
        logical_args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<CallSiteValue<'ctx>, BuilderError> {
        if !function
            .get_name()
            .to_str()
            .expect("LLVM function symbol is UTF-8")
            .ends_with(ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX)
        {
            return self.build_call(function, logical_args, name);
        }
        let caller = self
            .get_insert_block()
            .ok_or(BuilderError::UnsetPosition)?
            .get_parent()
            .expect("attached call belongs to a function");
        assert!(
            caller
                .get_name()
                .to_str()
                .expect("LLVM caller symbol is UTF-8")
                .ends_with(ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX),
            "attached call must originate in a compiled execution scope"
        );
        let context = caller
            .get_first_param()
            .expect("attached caller has a leading execution context");
        let mut args = Vec::with_capacity(logical_args.len() + 1);
        args.push(context.into());
        args.extend_from_slice(logical_args);
        self.build_call(function, &args, name)
    }
}
