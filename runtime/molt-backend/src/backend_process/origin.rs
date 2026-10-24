//! Original producing observations, never execution or release admission.
//! A scope correlates with a supervisor policy only after its closed journal is
//! independently verified. Retained entries keep their original scope and PID.
use crate::backend_process::input::ContentIdentity;
use crate::backend_process::input::content::AddressedInputIdentity;
use std::io;
#[cfg(molt_backend_daemon)]
use std::sync::Arc;
use std::sync::OnceLock;

static EMISSION_SCOPE: OnceLock<String> = OnceLock::new();

pub(crate) fn initialize_scope(scope: Option<&str>) -> io::Result<()> {
    if let Some(scope) = scope {
        if scope.len() != 64
            || !scope
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "emission scope must be 64 lowercase hexadecimal characters",
            ));
        }
        EMISSION_SCOPE
            .set(scope.to_owned())
            .map_err(|_| io::Error::other("emission scope initialized twice"))?;
    }
    Ok(())
}

pub(crate) fn current_scope() -> Option<&'static str> {
    EMISSION_SCOPE.get().map(String::as_str)
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProducerKind {
    OneShot,
    #[cfg(molt_backend_daemon)]
    Daemon,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmissionProducer {
    pub(crate) kind: ProducerKind,
    pub(crate) process_id: u32,
    pub(crate) scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum CodegenOrigin {
    InProcess {
        process_id: u32,
    },
    #[cfg(feature = "native-backend")]
    NativeWorkers {
        workers: Vec<crate::backend_process::native_batch::NativeWorkerOrigin>,
    },
}
impl CodegenOrigin {
    pub(crate) fn in_process() -> Self {
        Self::InProcess {
            process_id: std::process::id(),
        }
    }
    #[cfg(molt_backend_daemon)]
    fn retained_bytes(&self) -> usize {
        match self {
            Self::InProcess { .. } => 0,
            #[cfg(feature = "native-backend")]
            Self::NativeWorkers { workers } => {
                workers.capacity()
                    * std::mem::size_of::<crate::backend_process::native_batch::NativeWorkerOrigin>(
                    )
                    + workers
                        .iter()
                        .map(|w| w.retained_bytes() - std::mem::size_of_val(w))
                        .sum::<usize>()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmissionOrigin {
    schema: String,
    pub(crate) producer: EmissionProducer,
    pub(crate) input: AddressedInputIdentity,
    pub(crate) output: ContentIdentity,
    pub(crate) codegen: CodegenOrigin,
}
impl EmissionOrigin {
    pub(crate) fn new(
        kind: ProducerKind,
        input: AddressedInputIdentity,
        output: ContentIdentity,
        codegen: CodegenOrigin,
    ) -> Self {
        Self {
            schema: "molt.backend-emission-origin.v1".into(),
            producer: EmissionProducer {
                kind,
                process_id: std::process::id(),
                scope: current_scope().map(str::to_owned),
            },
            input,
            output,
            codegen,
        }
    }
    #[cfg(molt_backend_daemon)]
    pub(crate) fn daemon(
        input: AddressedInputIdentity,
        output: ContentIdentity,
        codegen: CodegenOrigin,
    ) -> Self {
        Self::new(ProducerKind::Daemon, input, output, codegen)
    }
    #[cfg(molt_backend_daemon)]
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.schema.capacity()
            + self.producer.scope.as_ref().map_or(0, String::capacity)
            + self.input.retained_bytes()
            + self.output.sha256.capacity()
            + self.codegen.retained_bytes()
    }
}

/// A cache generation owns both output and its original producing observation.
#[cfg(molt_backend_daemon)]
#[derive(Debug)]
pub(crate) struct DaemonCacheEntry {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) origin: EmissionOrigin,
}
#[cfg(molt_backend_daemon)]
impl DaemonCacheEntry {
    pub(crate) fn new(bytes: Arc<[u8]>, origin: EmissionOrigin) -> Self {
        Self { bytes, origin }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.origin.retained_bytes()
            - std::mem::size_of::<EmissionOrigin>()
            + self.bytes.len()
            + 4 * std::mem::size_of::<usize>()
    }
    #[cfg(test)]
    pub(crate) fn fixture(bytes: impl AsRef<[u8]>) -> Arc<Self> {
        let bytes: Arc<[u8]> = Arc::from(bytes.as_ref());
        let origin = EmissionOrigin::daemon(
            AddressedInputIdentity::fixture(),
            ContentIdentity::of_bytes(&bytes),
            CodegenOrigin::in_process(),
        );
        Arc::new(Self::new(bytes, origin))
    }
}

/// Result at the existing output publication boundary, before process admission.
pub(crate) struct EmittedArtifact {
    pub(crate) output: ContentIdentity,
    pub(crate) codegen: CodegenOrigin,
}
