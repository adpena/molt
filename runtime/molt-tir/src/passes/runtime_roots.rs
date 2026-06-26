pub(super) fn is_protected_runtime_entrypoint(name: &str) -> bool {
    const RUNTIME_ENTRYPOINTS: &[&str] = &["molt_main", "molt_host_init", "_start"];
    const RUNTIME_ENTRYPOINT_PREFIXES: &[&str] = &["molt_isolate_"];

    RUNTIME_ENTRYPOINTS.contains(&name)
        || RUNTIME_ENTRYPOINT_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}
