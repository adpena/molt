mod closure;
mod manifest;
mod symbols;

pub(crate) use closure::{
    shared_stdlib_partition_closure_issue, shared_stdlib_split_function_names,
    stdlib_partition_reference_kind, validate_shared_stdlib_partition,
};
pub(crate) use manifest::{
    STDLIB_PARTITION_MANIFEST_SCHEMA, shared_stdlib_partition_manifest, update_fnv1a64,
};
pub(crate) use symbols::{
    emitted_module_symbol, emitted_name_matches_module_symbol, is_user_owned_symbol,
    prune_and_partition_native_stdlib,
};
