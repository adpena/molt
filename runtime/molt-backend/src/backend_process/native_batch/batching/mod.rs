mod limits;
mod memory;
mod partition;
mod types;

pub(crate) use limits::{resolved_batch_op_budget_limit, resolved_batch_size_limit};
pub(crate) use memory::release_native_backend_batch_memory_to_os;
pub(crate) use partition::{
    ExternalFunctionDeclarations, append_referenced_external_declarations,
    batch_external_function_names, deduplicate_functions_by_name, external_function_declarations,
    partition_functions_for_batches,
};
pub(crate) use types::{
    NativeApplicationObjectOptions, NativeApplicationObjectResult, NativeBatchJobSpec,
    NativeBatchModuleMetadata, NativeBatchObjectJob,
};
