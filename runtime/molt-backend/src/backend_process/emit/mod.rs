use molt_backend::BackendIrDocument;
use std::io;

use super::cli_args::BackendTarget;
use super::io_limits::ensure_output_parent_dir;

#[cfg(feature = "native-backend")]
mod native;
#[cfg(feature = "wasm-backend")]
mod wasm;

#[cfg(feature = "native-backend")]
use native::emit_native_target;
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
    pub(crate) capture_origin: bool,
}

#[cfg(any(feature = "native-backend", feature = "wasm-backend"))]
type EmissionResult = Option<crate::backend_process::origin::EmittedArtifact>;
#[cfg(not(any(feature = "native-backend", feature = "wasm-backend")))]
type EmissionResult = ();

pub(crate) fn emit_backend_target(
    request: BackendTargetEmitRequest<'_>,
) -> io::Result<EmissionResult> {
    let BackendTargetEmitRequest {
        document,
        output_path,
        target,
        capture_origin,
    } = request;
    #[cfg(not(feature = "wasm-backend"))]
    let _ = capture_origin;
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
            document.code_slots,
            output_file,
            options.target_triple.as_deref(),
            options.output_kind,
        )
        .map(Some),
        #[cfg(feature = "wasm-backend")]
        BackendTarget::Wasm(options) => emit_wasm_target(
            document.ir,
            document.module_registry,
            document.code_slots,
            output_file,
            options,
            capture_origin,
        ),
    }
}
