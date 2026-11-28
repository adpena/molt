use std::io::BufRead as _;
use std::{collections::BTreeSet, io, path::PathBuf};

use molt_backend::{ModuleRegistryIR, NativeBackendModuleContext, SimpleIR};
use serde::Deserialize as _;
use serde::de::value::{MapAccessDeserializer, StringDeserializer};
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};

pub(crate) struct NativeApplicationArtifactOptions<'a> {
    pub(crate) object_cache: Option<crate::backend_process::shared_stdlib_cache::NativeObjectCache>,
    pub(crate) code_slots: molt_ir::code_slots::CodeSlotBindings,
    pub(crate) native_output_kind: crate::backend_process::NativeArtifactKind,
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) stdlib_split_enabled: bool,
    pub(crate) app_callable_manifest: Option<molt_backend::passes::AppCallableManifest>,
    pub(crate) log_prefix: &'a str,
    /// Per-build module registry (import bedrock, design doc 69): its init
    /// symbols are dead-function-elimination roots and the main application
    /// object emits its blob (`molt_module_registry_blob`).
    pub(crate) module_registry: Option<ModuleRegistryIR>,
    /// Whole-program ABI authority captured before stdlib extraction or batch
    /// partitioning removes provider bodies.
    pub(crate) module_context: Option<NativeBackendModuleContext>,
    /// Source preparation facts reindexed with this application's function rows.
    pub(crate) source_facts: Option<Vec<molt_backend::NativeSourceFunction>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeApplicationArtifactResult {
    pub(crate) function_count: usize,
    pub(crate) batch_count: usize,
    pub(crate) output: crate::backend_process::input::ContentIdentity,
    pub(crate) codegen: crate::backend_process::origin::CodegenOrigin,
}

/// Source stdlib constituents keep their independent content keys. Application
/// objects carry the terminal graph the parent partitioned for this target.
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum NativeBatchInput {
    Terminal {
        terminal: molt_backend::NativeTerminalObject,
    },
    Source(SimpleIR),
}

// The serialized terminal envelope already has a unique key. Route it to the
// typed decoder as the map arrives, so a large byte array never lives in Serde's
// untagged intermediate tree. Source jobs keep the regular SimpleIR validator.
impl<'de> serde::Deserialize<'de> for NativeBatchInput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JobInput;
        impl<'de> Visitor<'de> for JobInput {
            type Value = NativeBatchInput;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a terminal envelope or source IR")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let first = map.next_key::<String>()?;
                if first.as_deref() == Some("terminal") {
                    let terminal = map.next_value()?;
                    if let Some(extra) = map.next_key::<String>()? {
                        return Err(de::Error::unknown_field(&extra, &["terminal"]));
                    }
                    return Ok(NativeBatchInput::Terminal { terminal });
                }
                let source =
                    SimpleIR::deserialize(MapAccessDeserializer::new(WithFirstKey { first, map }))?;
                Ok(NativeBatchInput::Source(source))
            }
        }
        deserializer.deserialize_map(JobInput)
    }
}

struct WithFirstKey<A> {
    first: Option<String>,
    map: A,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for WithFirstKey<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        match self.first.take() {
            Some(key) => seed.deserialize(StringDeserializer::new(key)).map(Some),
            None => self.map.next_key_seed(seed),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        self.map.next_value_seed(seed)
    }
}

impl NativeBatchInput {
    pub(crate) fn declarations(&self) -> &[molt_backend::FunctionIR] {
        match self {
            Self::Source(ir) => &ir.functions,
            Self::Terminal { terminal } => &terminal.declarations,
        }
    }
}
impl From<SimpleIR> for NativeBatchInput {
    fn from(ir: SimpleIR) -> Self {
        Self::Source(ir)
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct NativeBatchObjectJob {
    /// Final program bindings belong only to the app metadata owner; reusable objects import them.
    pub(crate) code_slots: Option<molt_ir::code_slots::CodeSlotBindings>,
    pub(crate) ir: NativeBatchInput,
    pub(crate) module_context: NativeBackendModuleContext,
    pub(crate) codegen_environment: molt_ir::backend_environment::NativeCodegenEnvironment,
    pub(crate) target_triple: Option<String>,
    pub(crate) emit_app_callable_resolver: bool,
    pub(crate) app_callable_manifest: Option<molt_backend::passes::AppCallableManifest>,
    pub(crate) external_function_names: BTreeSet<String>,
    /// Carried by the batch that emits the app callable resolver (the main
    /// application object): that batch also emits the module registry blob.
    #[serde(default)]
    pub(crate) module_registry: Option<ModuleRegistryIR>,
}

impl NativeBatchObjectJob {
    /// The worker reads this same envelope from stdin and preserved replay files.
    pub(crate) fn encode(&self) -> io::Result<Vec<u8>> {
        rmp_serde::to_vec_named(self).map_err(io::Error::other)
    }

    pub(crate) fn decode(reader: impl io::Read) -> io::Result<Self> {
        let mut reader = io::BufReader::new(reader);
        let job = rmp_serde::from_read(&mut reader)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        if !reader.fill_buf()?.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trailing bytes after native batch object job",
            ));
        }
        Ok(job)
    }

    /// Close the object input contract before serialization by projecting just
    /// the rows this job can consume. The worker has no other context path.
    pub(crate) fn close_dependencies(mut self, context: &NativeBackendModuleContext) -> Self {
        let mut names = NativeBackendModuleContext::object_dependencies(self.ir.declarations());
        if let Some(manifest) = &self.app_callable_manifest {
            names.extend(manifest.names().cloned());
        }
        if let Some(registry) = &self.module_registry {
            names.extend(registry.init_symbols.iter().cloned());
            names.extend(registry.relocs.iter().map(|(_, name)| name.clone()));
        }
        self.module_context = context.project_object_dependencies(&names);
        self.external_function_names
            .retain(|name| names.contains(name));
        self
    }
}

/// A closed job whose worker transport is encoded only after cache admission misses.
pub(crate) struct NativeBatchJobSpec {
    pub(crate) input_key: Option<[u8; 32]>,
    pub(crate) job_name: String,
    pub(crate) job: NativeBatchObjectJob,
    pub(crate) object_path: PathBuf,
}
