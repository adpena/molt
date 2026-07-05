use std::path::Path;

#[cfg(feature = "native-backend")]
use crate::backend_process::shared_stdlib_cache::shared_stdlib_cache_matches;

use super::super::super::protocol::DaemonJobRequest;

pub(crate) fn daemon_memory_cache_allowed_for_job(job: &DaemonJobRequest) -> bool {
    if job.is_wasm {
        return true;
    }
    #[cfg(feature = "native-backend")]
    {
        let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
            return true;
        };
        shared_stdlib_cache_matches(
            Path::new(&stdlib_obj_path),
            std::env::var("MOLT_STDLIB_CACHE_KEY").ok().as_deref(),
            std::env::var("MOLT_STDLIB_CACHE_MANIFEST").ok().as_deref(),
            None,
        )
    }
    #[cfg(not(feature = "native-backend"))]
    {
        false
    }
}
