use molt_ir::backend_environment::{self as backend_env, BackendEnv};

pub(crate) const APP_RESOLVER_SYMBOL: &str = "molt_app_resolve_callable";
pub(crate) const APP_RESOLVER_NAMES_SYMBOL: &str = "molt_app_callable_names";
pub(crate) const APP_RESOLVER_TABLE_SYMBOL: &str = "molt_app_callable_table";
pub(crate) const APP_RESOLVER_RECORD_BYTES: usize = 16;

pub(crate) fn dump_app_callable_manifest(manifest: &crate::passes::AppCallableManifest) {
    if backend_env::var(BackendEnv::MOLT_DUMP_APP_CALLABLE_MANIFEST).as_deref() != Ok("1") {
        return;
    }
    let entries = manifest.entries();
    eprintln!("MOLT_APP_CALLABLE_MANIFEST: count={}", entries.len());
    for (name, kind) in entries {
        eprintln!("MOLT_APP_CALLABLE_MANIFEST: {name} {kind:?}");
    }
}
