//! Backend-owned input/output observation. This is not image or release admission.
//! The serving daemon PID is diagnostic until independently joined to kernel custody.
use crate::backend_process::input::ContentIdentity;
use crate::backend_process::input::content::AddressedInputIdentity;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
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
                        .map(|worker| worker.retained_bytes() - std::mem::size_of_val(worker))
                        .sum::<usize>()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DaemonJobOrigin {
    schema: &'static str,
    pub(crate) daemon_pid: u32,
    pub(crate) input: AddressedInputIdentity,
    pub(crate) output: ContentIdentity,
    pub(crate) codegen: CodegenOrigin,
}
impl DaemonJobOrigin {
    pub(crate) fn new(
        input: AddressedInputIdentity,
        output: ContentIdentity,
        codegen: CodegenOrigin,
    ) -> Self {
        Self {
            schema: "molt.daemon-job-origin.v2",
            daemon_pid: std::process::id(),
            input,
            output,
            codegen,
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.input.retained_bytes()
            + self.output.sha256.capacity()
            + self.codegen.retained_bytes()
    }
}

/// One cache value owns both bytes and their producing observation. Cache hits
/// retain this origin, including across module/function tiers and followers.
#[derive(Debug)]
pub(crate) struct DaemonCacheEntry {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) origin: DaemonJobOrigin,
}
impl DaemonCacheEntry {
    pub(crate) fn new(bytes: Arc<[u8]>, origin: DaemonJobOrigin) -> Self {
        Self { bytes, origin }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.origin.retained_bytes()
            - std::mem::size_of::<DaemonJobOrigin>()
            + self.bytes.len()
            + 4 * std::mem::size_of::<usize>()
    }
    #[cfg(test)]
    pub(crate) fn fixture(bytes: impl AsRef<[u8]>) -> Arc<Self> {
        let bytes: Arc<[u8]> = Arc::from(bytes.as_ref());
        let origin = DaemonJobOrigin::new(
            AddressedInputIdentity::fixture(),
            ContentIdentity::of_bytes(&bytes),
            CodegenOrigin::in_process(),
        );
        Arc::new(Self::new(bytes, origin))
    }
}
