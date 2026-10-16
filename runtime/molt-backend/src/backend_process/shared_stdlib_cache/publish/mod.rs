mod admission;
mod archive;
mod files;
mod lock;
mod objects;
mod paths;
pub(crate) use objects::StdlibObjectCache;
mod sidecars;

#[cfg(any(unix, test))]
pub(crate) use admission::shared_stdlib_cache_matches;
pub(crate) use admission::{
    admit_or_invalidate_shared_stdlib_cache, hydrate_shared_stdlib_cache_archive,
};
pub(crate) use archive::publish_shared_stdlib_cache_archive;
pub(crate) use lock::with_shared_stdlib_cache_producer_lock;
#[cfg(test)]
pub(crate) use lock::with_shared_stdlib_cache_publish_lock;
#[cfg(test)]
pub(crate) use paths::{
    stdlib_cache_count_sidecar_path, stdlib_cache_partition_manifest_sidecar_path,
};
#[cfg(test)]
pub(crate) use sidecars::{
    read_stdlib_cache_key, read_stdlib_cache_manifest, write_shared_stdlib_cache_sidecars,
};

pub(crate) use paths::{
    canonical_stdlib_cache_root, resolved_archive_path, validate_required_stdlib_output,
};
