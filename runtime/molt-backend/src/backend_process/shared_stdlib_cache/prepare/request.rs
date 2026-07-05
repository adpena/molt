pub(crate) struct NativeStdlibCachePrepare<'a> {
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_obj_path: Option<&'a str>,
    pub(crate) expected_cache_key: Option<&'a str>,
    pub(crate) expected_cache_manifest: Option<&'a str>,
    pub(crate) have_entry_module: bool,
    pub(crate) entry_module: &'a str,
    pub(crate) explicit_stdlib_module_symbols: Option<&'a std::collections::BTreeSet<String>>,
    pub(crate) log_prefix: &'a str,
    /// Per-build module registry (import bedrock).  Its init symbols root the
    /// stdlib-partition dead-function elimination and it is forwarded to the
    /// application-object compile for blob emission.
    pub(crate) module_registry: Option<molt_backend::ModuleRegistryIR>,
}
