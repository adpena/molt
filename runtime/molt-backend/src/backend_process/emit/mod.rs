use molt_backend::BackendIrDocument;
use std::io;

use super::cli_args::BackendTarget;
use super::io_limits::ensure_output_parent_dir;

#[cfg(feature = "luau-backend")]
mod luau;
#[cfg(any(feature = "luau-backend", test))]
mod luau_pipeline;
#[cfg(feature = "native-backend")]
mod native;
#[cfg(feature = "rust-backend")]
mod rust;
#[cfg(feature = "wasm-backend")]
mod wasm;

#[cfg(feature = "luau-backend")]
use luau::emit_luau_target;
#[cfg(test)]
pub(crate) use luau_pipeline::run_luau_tir_module_pipeline;
#[cfg(feature = "native-backend")]
use native::emit_native_target;
#[cfg(feature = "rust-backend")]
use rust::emit_rust_target;
#[cfg(all(feature = "rust-backend", test))]
pub(crate) use rust::rust_source_for_ir;
#[cfg(feature = "wasm-backend")]
use wasm::emit_wasm_target;
#[cfg(all(molt_backend_daemon, feature = "wasm-backend"))]
pub(crate) use wasm::validate_wasm_module_catalog;

/// One admitted document to compile for one target; see
/// `BackendTarget::admit_document`.
pub(crate) struct BackendTargetEmitRequest<'a> {
    pub(crate) document: BackendIrDocument,
    pub(crate) output_path: Option<&'a str>,
    pub(crate) target: BackendTarget,
}

pub(crate) fn emit_backend_target(request: BackendTargetEmitRequest<'_>) -> io::Result<()> {
    let BackendTargetEmitRequest {
        document,
        output_path,
        target,
    } = request;
    let output_file = output_path.unwrap_or(target.default_output_path());
    ensure_output_parent_dir(output_file).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to create backend output parent for '{output_file}': {err}"),
        )
    })?;

    match target {
        #[cfg(feature = "native-backend")]
        BackendTarget::Native(options) => emit_native_target(
            document.ir,
            document.module_registry,
            output_file,
            options.target_triple.as_deref(),
            options.output_kind,
        ),
        #[cfg(feature = "wasm-backend")]
        BackendTarget::Wasm(options) => {
            emit_wasm_target(document.ir, document.module_registry, output_file, options)
        }
        // Admission refused a module catalog for the source emitters.
        #[cfg(feature = "luau-backend")]
        BackendTarget::Luau => emit_luau_target(document.ir, output_file),
        #[cfg(feature = "rust-backend")]
        BackendTarget::Rust => emit_rust_target(&document.ir, output_file),
    }
}
