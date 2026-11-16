mod closure;
mod manifest;
mod symbols;

#[cfg(test)]
pub(crate) use closure::shared_stdlib_partition_closure_issue;
pub(crate) use closure::{shared_stdlib_split_function_names, validate_shared_stdlib_partition};
pub(crate) use manifest::shared_stdlib_partition_manifest;
pub(crate) use symbols::prune_and_partition_native_stdlib;
