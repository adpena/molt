mod compile;
mod partition;
mod prepare;
mod publish;

pub(crate) use compile::plan_stdlib_cache_archive;
#[cfg(test)]
pub(crate) use partition::shared_stdlib_partition_closure_issue;
pub(crate) use partition::{
    prune_and_partition_native_stdlib, shared_stdlib_partition_manifest,
    shared_stdlib_split_function_names, validate_shared_stdlib_partition,
};
#[cfg(test)]
pub(crate) use prepare::MATERIALIZED as STDLIB_MATERIALIZED;
pub(crate) use prepare::{NativeStdlibCachePrepare, prepare_native_application_artifact};
pub(crate) use publish::NativeObjectCache;
pub(crate) use publish::read_stdlib_cache_partition_manifest;
#[cfg(any(unix, test))]
pub(crate) use publish::shared_stdlib_cache_matches;
pub(crate) use publish::{
    admit_or_invalidate_shared_stdlib_cache, publish_shared_stdlib_cache_archive,
    resolved_archive_path, validate_required_stdlib_output, with_shared_stdlib_cache_producer_lock,
};
#[cfg(test)]
pub(crate) use publish::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, stdlib_cache_count_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path, with_shared_stdlib_cache_publish_lock,
    write_shared_stdlib_cache_sidecars,
};
