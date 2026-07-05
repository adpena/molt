use std::io;

use super::model::BackendCliArgs;
use crate::backend_process::io_limits::BackendOutputKind;

impl<'a> BackendCliArgs<'a> {
    pub(crate) fn daemon_socket_path(&self) -> io::Result<Option<&'a str>> {
        if !self.wants_daemon {
            return Ok(None);
        }
        self.socket_path
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket is required"))
            .map(Some)
    }

    pub(crate) fn validate_fact_graph_contract(&self) -> io::Result<()> {
        if self.fact_graph_output_path.is_some() != self.fact_graph_function.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--fact-graph-output and --fact-graph-function must be supplied together",
            ));
        }
        if self.fact_graph_output_path.is_some() && self.is_rust {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fact graph emission does not support the rust target",
            ));
        }
        Ok(())
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
