/// The application-owned program entries hosts call by name
/// (`execution_context_abi.compiled_entries` in `runtime/runtime_abi.toml`).
pub(super) fn is_protected_runtime_entrypoint(name: &str) -> bool {
    molt_ir::runtime_callable_abi_generated::CompiledExecutionEntry::from_logical(name).is_some()
}
