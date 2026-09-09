mod compile;
mod partition;
mod prepare;
mod publish;

pub(crate) use compile::compile_stdlib_cache_archive;
#[cfg(test)]
pub(crate) use partition::{is_user_owned_symbol, shared_stdlib_partition_closure_issue};
pub(crate) use partition::{
    prune_and_partition_native_stdlib, shared_stdlib_partition_manifest,
    shared_stdlib_split_function_names, validate_shared_stdlib_partition,
};
pub(crate) use prepare::{NativeStdlibCachePrepare, prepare_native_application_artifact};
pub(crate) use publish::{
    admit_or_invalidate_shared_stdlib_cache, publish_shared_stdlib_cache_archive,
    shared_stdlib_cache_matches, stdlib_cache_temp_publish_path,
};
#[cfg(test)]
pub(crate) use publish::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, stdlib_cache_count_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path, with_shared_stdlib_cache_publish_lock,
    write_shared_stdlib_cache_sidecars,
};
