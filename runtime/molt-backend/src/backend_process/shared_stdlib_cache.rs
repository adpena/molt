mod compile;
mod partition;
mod prepare;
mod publish;

pub(crate) use compile::compile_stdlib_cache_object;
pub(crate) use partition::{
    STDLIB_PARTITION_MANIFEST_SCHEMA, emitted_module_symbol, emitted_name_matches_module_symbol,
    is_user_owned_symbol, prune_and_partition_native_stdlib, shared_stdlib_partition_closure_issue,
    shared_stdlib_partition_manifest, shared_stdlib_split_function_names,
    stdlib_partition_reference_kind, update_fnv1a64, validate_shared_stdlib_partition,
};
pub(crate) use prepare::{NativeStdlibCachePrepare, prepare_native_application_object};
pub(crate) use publish::{
    publish_shared_stdlib_cache_object, read_stdlib_cache_key, read_stdlib_cache_manifest,
    read_stdlib_cache_partition_manifest, remove_shared_stdlib_cache_artifacts, sha256_file_hex,
    shared_stdlib_cache_matches, stdlib_cache_count_sidecar_path, stdlib_cache_key_sidecar_path,
    stdlib_cache_manifest_sidecar_path, stdlib_cache_object_digest_sidecar_path,
    stdlib_cache_partition_manifest_sidecar_path, stdlib_cache_publish_lock_path,
    stdlib_cache_temp_publish_path, with_shared_stdlib_cache_publish_lock,
    write_shared_stdlib_cache_sidecars,
};
