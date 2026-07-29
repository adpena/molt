mod application;
mod batching;
mod link;
mod temp;
mod worker;

pub(crate) use application::compile_native_application_object_to_path;
pub(crate) use batching::{
    ExternalFunctionDeclarations, NativeApplicationObjectOptions, NativeApplicationObjectResult,
    NativeBatchJobSpec, NativeBatchModuleMetadata, NativeBatchObjectJob,
    append_referenced_external_declarations, batch_external_function_names,
    deduplicate_functions_by_name, external_function_declarations, partition_functions_for_batches,
    release_native_backend_batch_memory_to_os, resolved_batch_op_budget_limit,
    resolved_batch_size_limit,
};
pub(crate) use link::merge_relocatable_objects;
#[cfg(test)]
pub(crate) use link::relocatable_linker_binary;
pub(crate) use temp::finish_native_batch_temp_dir;
#[cfg(test)]
pub(crate) use temp::{
    preserve_native_batch_worker_failure_artifacts, remove_native_batch_temp_dir,
};
pub(crate) use worker::{
    compile_native_batch_object_job_file, run_native_batch_worker_with_failure_artifacts,
};
