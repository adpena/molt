#[cfg(feature = "native-backend")]
use crate::backend_process::NativeTargetOptions;
#[cfg(feature = "wasm-backend")]
use crate::backend_process::WasmTargetOptions;

#[derive(Debug)]
pub(crate) struct DaemonJobRequest {
    pub(crate) id: String,
    pub(crate) target: DaemonJobTarget,
    pub(crate) output: String,
    pub(crate) cache_key: String,
    pub(crate) function_cache_key: Option<String>,
    pub(crate) skip_module_output_if_synced: bool,
    pub(crate) skip_function_output_if_synced: bool,
    pub(crate) ir_source: DaemonIrSource,
    /// The request's backend configuration digest, which scopes its cache keys.
    pub(crate) config_namespace: Option<String>,
}

/// A cache probe opens no IR. An admitted compile owns exactly one file lease.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DaemonIrSource {
    Probe,
    Lease(String),
}

/// The artifact a daemon job builds. The daemon serves native and WASM jobs
/// for the backends this binary contains; the request parser refuses others.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DaemonJobTarget {
    #[cfg(feature = "native-backend")]
    Native(NativeTargetOptions),
    #[cfg(feature = "wasm-backend")]
    Wasm(WasmTargetOptions),
}

#[derive(Debug)]
pub(crate) struct DaemonRequest {
    pub(crate) version: Option<u32>,
    pub(crate) ping: Option<bool>,
    /// Asks the daemon to stop taking connections and exit once drained.
    pub(crate) retire: Option<bool>,
    pub(crate) include_health: Option<bool>,
    pub(crate) config_digest: Option<String>,
    pub(crate) jobs: Option<Vec<DaemonJobRequest>>,
    /// The request's backend settings, installed per compiling thread.
    pub(crate) environment: std::sync::Arc<molt_backend::backend_environment::BackendEnvironment>,
}

/// The backend-owned request dimensions shared by both caller cache tiers.
/// Deriving structural identity includes every field of the closed target options.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct DaemonCacheContext {
    pub(super) config_namespace: Option<String>,
    pub(super) target: DaemonJobTarget,
    pub(super) environment: molt_backend::backend_environment::SemanticBackendIdentity,
}

/// A cache/flight identity, not a serialized wire key or consumed-IR receipt.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DaemonCacheKey {
    pub(super) context: std::sync::Arc<DaemonCacheContext>,
    pub(super) source: String,
}

impl DaemonCacheKey {
    /// All transitive allocations retained by this Arc key. Sharing with other
    /// tiers/requests is deliberately charged per entry, never deduplicated by
    /// a second ownership registry. Accounting is excluded from typed identity.
    pub(crate) fn retained_bytes(&self) -> usize {
        let Self { context, source } = self;
        let DaemonCacheContext {
            config_namespace,
            target,
            environment: _, // fixed digest is included in the inline context
        } = context.as_ref();
        let target_bytes = match target {
            #[cfg(feature = "native-backend")]
            DaemonJobTarget::Native(NativeTargetOptions {
                target_triple,
                output_kind: _,
            }) => target_triple.as_ref().map_or(0, String::capacity),
            #[cfg(feature = "wasm-backend")]
            DaemonJobTarget::Wasm(WasmTargetOptions {
                link_relocs: _,
                data_base: _,
                table_base: _,
                split_runtime_app_table_base: _,
            }) => 0,
        };
        std::mem::size_of::<Self>()
            .saturating_add(4 * std::mem::size_of::<usize>()) // key/context Arc headers
            .saturating_add(source.capacity())
            .saturating_add(std::mem::size_of::<DaemonCacheContext>())
            .saturating_add(config_namespace.as_ref().map_or(0, String::capacity))
            .saturating_add(target_bytes)
    }
}

/// The admitted cache tiers share their context and may share the same key.
/// No tier key means that tier has no cache or single-flight authority.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DaemonArtifactCacheKeys {
    pub(crate) module: Option<std::sync::Arc<DaemonCacheKey>>,
    pub(crate) function: Option<std::sync::Arc<DaemonCacheKey>>,
}
