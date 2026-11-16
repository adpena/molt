use molt_backend::BackendIrDocument;
use std::io;

use super::cli_args::BackendTarget;
use super::io_limits::ensure_output_parent_dir;

#[cfg(feature = "native-backend")]
mod native;

#[cfg(feature = "native-backend")]
use native::emit_native_target;

/// One admitted document to compile for one target; see
/// `BackendTarget::admit_document`.
pub(crate) struct BackendTargetEmitRequest<'a> {
    pub(crate) document: BackendIrDocument,
    pub(crate) output_path: Option<&'a str>,
    pub(crate) target: BackendTarget,
    pub(crate) capture_origin: bool,
}

#[cfg(feature = "native-backend")]
type EmissionResult = Option<crate::backend_process::origin::EmittedArtifact>;
#[cfg(not(feature = "native-backend"))]
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
    }
}
