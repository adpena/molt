use std::collections::BTreeSet;

pub(crate) const APP_RESOLVER_SYMBOL: &str = "molt_app_resolve_intrinsic";
pub(crate) const APP_RESOLVER_NAMES_SYMBOL: &str = "molt_app_intrinsic_names";
pub(crate) const APP_RESOLVER_TABLE_SYMBOL: &str = "molt_app_intrinsic_table";
pub(crate) const APP_RESOLVER_RECORD_BYTES: usize = 16;

pub(crate) fn dump_intrinsic_manifest(manifest_names: &BTreeSet<String>) {
    if std::env::var("MOLT_DUMP_INTRINSIC_MANIFEST").as_deref() != Ok("1") {
        return;
    }
    eprintln!("MOLT_INTRINSIC_MANIFEST: count={}", manifest_names.len());
    for name in manifest_names {
        eprintln!("MOLT_INTRINSIC_MANIFEST: {name}");
    }
}
