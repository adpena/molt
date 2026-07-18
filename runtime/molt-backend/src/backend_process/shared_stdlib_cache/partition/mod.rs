mod closure;
mod manifest;
mod symbols;

pub(crate) use closure::{
    shared_stdlib_partition_closure_issue, shared_stdlib_split_function_names,
    validate_shared_stdlib_partition,
};
pub(crate) use manifest::shared_stdlib_partition_manifest;
pub(crate) use symbols::{is_user_owned_symbol, prune_and_partition_native_stdlib};
