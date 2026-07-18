use std::fs::File;
use std::io;

pub(super) fn backend_ir_document_from_json_path(
    path: &str,
) -> Result<molt_backend::BackendIrDocument, String> {
    let file = File::open(path).map_err(|err| format!("failed to open ir_path {path:?}: {err}"))?;
    serde_json::from_reader(io::BufReader::new(file))
        .map_err(|err| format!("failed to parse ir_path {path:?}: {err}"))
}
