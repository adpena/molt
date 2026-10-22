//! The JIT program image: one executable file per program.
//!
//! `src/molt/cli/jit_image.py` writes it and owns its layout:
//!
//! ```text
//! #!<runner> --image
//! #molt.jit-image.v2 <header bytes> <program bytes>
//! <header: JSON>
//! <program: backend IR, MessagePack>
//! ```
//!
//! The first line makes the runner the image's interpreter: `execve` of the
//! image starts `<runner> --image <image> [args...]` with no shell between,
//! so the program sees the environment its caller passed, and
//! `sys.executable` (argv[0]) re-runs it, as an AOT executable's path does.
//! The CLI publishes the whole file with one rename, so its header and its
//! program always belong together.

use crate::compiler_output::CompilerDiagnostics;
use crate::launch::LaunchPlan;
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The interpreter line starts `#!` and ends with the runner's image flag.
pub(crate) const IMAGE_INTERPRETER_PREFIX: &str = "#!";
pub(crate) const IMAGE_INTERPRETER_FLAG: &str = " --image";
pub(crate) const IMAGE_MARKER: &str = "#molt.jit-image.v2";

/// What the runner needs besides the program.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImageHeader {
    /// The runtime as a shared image, linked from the AOT runtime archive.
    pub(crate) runtime_image: PathBuf,
    /// Request-scoped backend settings for this compilation.
    pub(crate) backend_environment: serde_json::Map<String, serde_json::Value>,
    /// Whether compiler stderr streams or appears only on failure.
    pub(crate) compiler_diagnostics: CompilerDiagnostics,
    pub(crate) launch: LaunchPlan,
}

/// One program image read from disk.
pub(crate) struct JitImage {
    pub(crate) header: ImageHeader,
    pub(crate) bytes: Vec<u8>,
    program_start: usize,
}

impl JitImage {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let mut bytes = Vec::new();
        molt_artifact_publish::open_regular_artifact(path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read JIT image {}: {error}", path.display()))?;
        Self::parse(bytes).map_err(|error| format!("invalid JIT image {}: {error}", path.display()))
    }

    fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        let mut lines = bytes.splitn(3, |byte| *byte == b'\n');
        let interpreter = lines.next().unwrap_or_default();
        if !interpreter.starts_with(IMAGE_INTERPRETER_PREFIX.as_bytes())
            || !interpreter.ends_with(IMAGE_INTERPRETER_FLAG.as_bytes())
        {
            return Err(format!(
                "its first line is not `{IMAGE_INTERPRETER_PREFIX}<runner>{IMAGE_INTERPRETER_FLAG}`"
            ));
        }
        let marker = lines.next().ok_or("it has no image marker")?;
        let marker = std::str::from_utf8(marker).map_err(|_| "its image marker is not UTF-8")?;
        let mut fields = marker.split(' ');
        if fields.next() != Some(IMAGE_MARKER) {
            return Err(format!("its second line is not a `{IMAGE_MARKER}` marker"));
        }
        let mut length = |what: &str| {
            fields
                .next()
                .and_then(|field| field.parse::<usize>().ok())
                .ok_or_else(|| format!("its marker names no {what} length"))
        };
        let header_len = length("header")?;
        let program_len = length("program")?;
        if fields.next().is_some() {
            return Err("its marker has extra fields".to_string());
        }
        let header_start = bytes.len() - lines.next().map_or(0, <[u8]>::len);
        let program_start = header_start
            .checked_add(header_len)
            .ok_or("its header length overflows")?;
        if program_start.checked_add(program_len) != Some(bytes.len()) {
            return Err(format!(
                "it holds {} bytes after its marker; the marker names {header_len} + {program_len}",
                bytes.len() - header_start
            ));
        }
        let header = serde_json::from_slice(&bytes[header_start..program_start])
            .map_err(|error| format!("its header: {error}"))?;
        Ok(Self {
            header,
            bytes,
            program_start,
        })
    }

    /// Decode the program's backend IR document.
    pub(crate) fn program(&self) -> Result<molt_ir::ir::BackendIrDocument, String> {
        molt_ir::ir::BackendIrDocument::from_msgpack_reader(&mut &self.bytes[self.program_start..])
            .map_err(|error| format!("invalid program: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(header: &str, program: &[u8]) -> Vec<u8> {
        let mut bytes = format!(
            "#!/runner{IMAGE_INTERPRETER_FLAG}\n{IMAGE_MARKER} {} {}\n{header}",
            header.len(),
            program.len()
        )
        .into_bytes();
        bytes.extend_from_slice(program);
        bytes
    }

    const HEADER: &str = r#"{"runtime_image":"/r.dylib","backend_environment":{},"compiler_diagnostics":"on_failure","launch":{"environment":{},"runtime_module_roots":[],"compiled_source_paths":[]}}"#;

    #[test]
    fn an_image_splits_into_its_header_and_its_program() {
        let parsed = JitImage::parse(image(HEADER, b"\x93program")).expect("valid image");
        assert_eq!(parsed.header.runtime_image, PathBuf::from("/r.dylib"));
        assert_eq!(
            parsed.header.compiler_diagnostics,
            CompilerDiagnostics::OnFailure
        );
        assert_eq!(&parsed.bytes[parsed.program_start..], b"\x93program");
    }

    #[test]
    fn a_truncated_or_padded_image_is_refused() {
        let whole = image(HEADER, b"program");
        for bytes in [
            whole[..whole.len() - 1].to_vec(),
            [whole.as_slice(), b"x"].concat(),
        ] {
            let error = JitImage::parse(bytes).err().expect("refused");
            assert!(error.contains("after its marker"), "{error}");
        }
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused() {
        for (bytes, expected) in [
            (
                b"{\"schema\":\"molt.jit-launch.v1\"}".to_vec(),
                "first line",
            ),
            // The v1 layout ran the runner through /bin/sh.
            (
                b"#!/bin/sh\nexec x\n#molt.jit-image.v1 1 1\n{}".to_vec(),
                "first line",
            ),
            (
                b"#!/runner --image\n#molt.jit-image.v1 1 1\n{}".to_vec(),
                "marker",
            ),
            (
                b"#!/runner --image\n#molt.jit-image.v2 2\n{}".to_vec(),
                "program length",
            ),
        ] {
            let error = JitImage::parse(bytes).err().expect("refused");
            assert!(error.contains(expected), "{error}");
        }
    }
}
