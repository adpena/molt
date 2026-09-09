use std::io;

use super::model::BackendCliArgs;
use crate::backend_process::io_limits::BackendOutputKind;

pub(crate) fn validate_fact_graph_cli_contract(
    fact_graph_output_path: Option<&str>,
    fact_graph_function: Option<&str>,
    is_rust: bool,
) -> io::Result<()> {
    if fact_graph_output_path.is_some() != fact_graph_function.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--fact-graph-output and --fact-graph-function must be supplied together",
        ));
    }
    if fact_graph_output_path.is_some() && is_rust {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fact graph emission does not support the rust target",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_process::NativeArtifactKind;

    #[test]
    fn native_artifact_cli_kind_is_explicit_and_fail_closed() {
        for (args, expected) in [
            (vec![], Some(NativeArtifactKind::Object)),
            (
                vec!["--native-output-kind", "object"],
                Some(NativeArtifactKind::Object),
            ),
            (
                vec!["--native-output-kind", "archive"],
                Some(NativeArtifactKind::Archive),
            ),
            (vec!["--native-output-kind"], None),
            (vec!["--native-output-kind", "static-library"], None),
            (
                vec![
                    "--native-output-kind",
                    "object",
                    "--native-output-kind",
                    "archive",
                ],
                None,
            ),
            (
                vec!["--target", "wasm", "--native-output-kind", "archive"],
                None,
            ),
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(
                BackendCliArgs::parse(&args)
                    .resolved_native_output_kind()
                    .ok(),
                expected,
                "{args:?}"
            );
        }
    }
}

impl<'a> BackendCliArgs<'a> {
    pub(crate) fn resolved_native_output_kind(
        &self,
    ) -> io::Result<crate::backend_process::NativeArtifactKind> {
        if self.native_output_kind.is_some() && (self.is_wasm || self.is_rust || self.is_luau) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--native-output-kind requires a native target",
            ));
        }
        self.native_output_kind
            .map(crate::backend_process::NativeArtifactKind::parse)
            .transpose()
            .map(|kind| kind.unwrap_or_default())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    pub(crate) fn daemon_socket_path(&self) -> io::Result<Option<&'a str>> {
        if !self.wants_daemon {
            return Ok(None);
        }
        self.socket_path
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))
            .map(Some)
    }

    pub(crate) fn validate_fact_graph_contract(&self) -> io::Result<()> {
        validate_fact_graph_cli_contract(
            self.fact_graph_output_path,
            self.fact_graph_function,
            self.is_rust,
        )
    }

    pub(crate) fn output_kind(&self) -> BackendOutputKind {
        if self.is_luau {
            BackendOutputKind::Luau
        } else if self.is_rust {
            BackendOutputKind::Rust
        } else if self.is_wasm {
            BackendOutputKind::Wasm
        } else {
            BackendOutputKind::Native
        }
    }
}
