//! Persistent Cranelift stencils for both native code owners.

use super::CodePlacement;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::incremental_cache::CacheKvStore;
use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::{CompileError, CompiledCode, Context};
use molt_ir::content_digest::CompilationCacheKey;
use molt_tir::tir::cache::{CompilationCache, CompilationCacheWriteError};
use molt_tir::tir::pipeline_cache::TirCacheLocation;
use std::borrow::Cow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct MachineCodeCache {
    artifacts: Mutex<CompilationCache>,
    placement: CodePlacement,
    hits: AtomicUsize,
    misses: AtomicUsize,
}

fn store_identity(version: &str) -> CompilationCacheKey {
    let mut identity = CompilationCacheKey::new(b"molt-native-stencil-store-v1");
    identity.field(b"cranelift-version", version.as_bytes());
    identity
}

fn artifact_key(placement: CodePlacement, native_abi: &str, clif_key: &[u8]) -> String {
    let kind = match placement {
        CodePlacement::Object => 0,
        #[cfg(feature = "jit")]
        CodePlacement::ProcessMemory => 1,
    };
    let mut key = CompilationCacheKey::new(b"molt-cranelift-machine-code-v2");
    key.field(b"artifact-kind", &[kind]);
    key.field(b"native-data-and-call-abi", native_abi.as_bytes());
    key.field(b"cranelift-stencil", clif_key);
    key.finish_hex()
}

impl MachineCodeCache {
    pub(super) fn open(location: &TirCacheLocation, placement: CodePlacement) -> Option<Self> {
        if molt_ir::backend_environment::compilation_diagnostics_requested() {
            return None;
        }
        let root = match location {
            TirCacheLocation::Root(root) => Some(root.clone()),
            TirCacheLocation::ProcessRoot => None,
            // An explicitly assigned TIR directory owns only its own data.
            TirCacheLocation::Directory(_) => return None,
        };
        let artifacts = CompilationCache::open_shared_backend_namespace(
            root,
            store_identity(cranelift_codegen::VERSION),
        )?;
        Some(Self {
            artifacts: Mutex::new(artifacts),
            placement,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
        })
    }

    fn kind(&self) -> &'static str {
        match self.placement {
            CodePlacement::Object => "AOT",
            #[cfg(feature = "jit")]
            CodePlacement::ProcessMemory => "JIT",
        }
    }

    fn key(&self, clif_key: &[u8]) -> String {
        artifact_key(
            self.placement,
            molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
            clif_key,
        )
    }

    /// Cranelift hashes the final CLIF stencil, including call signatures
    /// and global value declarations, plus the target, ISA and effective
    /// optimization flags. The stencil has no process addresses. Cranelift
    /// reapplies the current function's name parameters before relocation.
    pub(super) fn compile<'a>(
        &self,
        context: &'a mut Context,
        isa: &dyn TargetIsa,
        control: &mut ControlPlane,
    ) -> Result<&'a CompiledCode, CompileError<'a>> {
        let (compiled, hit) = context.compile_with_cache(isa, &mut CacheAdapter(self), control)?;
        let count = if hit { &self.hits } else { &self.misses };
        count.fetch_add(1, Ordering::Relaxed);
        Ok(compiled)
    }

    pub(super) fn report_counts(&self) {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr().lock(),
            "MOLT_BACKEND_TIMING: {} machine-code cache: {} hits, {} misses",
            self.kind(),
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed)
        );
    }
}

struct CacheAdapter<'a>(&'a MachineCodeCache);

impl CacheKvStore for CacheAdapter<'_> {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        self.0
            .artifacts
            .lock()
            .unwrap()
            .get(&self.0.key(key))
            .map(|bytes| Cow::Owned(bytes.to_vec()))
    }

    fn insert(&mut self, key: &[u8], value: Vec<u8>) {
        let stored = self
            .0
            .artifacts
            .lock()
            .unwrap()
            .put(&self.0.key(key), &value);
        if let Err(CompilationCacheWriteError::Integrity(error)) = stored {
            panic!("Cranelift machine-code cache integrity failure: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::incremental_cache::compute_cache_key;
    use cranelift_codegen::ir::{
        AbiParam, ExtFuncData, ExternalName, Function, GlobalValueData, InstBuilder, Signature,
        UserExternalName, types,
    };
    use cranelift_codegen::settings;
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{FuncId, ModuleReloc, ModuleRelocTarget};

    fn function(isa: &dyn TargetIsa, constant: i64, callee: u32) -> Function {
        let mut function = Function::new();
        function.signature = Signature::new(isa.default_call_conv());
        function.signature.returns.push(AbiParam::new(types::I64));
        let signature = function.import_signature(Signature::new(isa.default_call_conv()));
        let name = function.declare_imported_user_function(UserExternalName {
            namespace: 0,
            index: callee,
        });
        let callee = function.import_function(ExtFuncData {
            name: ExternalName::user(name),
            signature,
            colocated: true,
            patchable: false,
        });
        let mut builder_context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.ins().call(callee, &[]);
        let result = builder.ins().iconst(types::I64, constant);
        builder.ins().return_(&[result]);
        builder.seal_all_blocks();
        builder.finalize(isa.frontend_config());
        function
    }

    #[test]
    fn persisted_stencil_matches_a_forced_compile_and_keeps_the_relocation_kind() {
        use molt_ir::backend_environment::{self, BackendEnvironment};
        use std::sync::Arc;

        let _request = backend_environment::enter(Arc::new(BackendEnvironment::default()));
        let directory = super::super::tests::test_tir_pipeline_cache_dir();
        let location = TirCacheLocation::Root(directory.clone());
        let isa = super::super::SimpleBackend::native_isa(None, CodePlacement::Object);
        let body = function(&*isa, 42, 11);
        let rebound = function(&*isa, 42, 12);
        let forced_compile = |body: Function| {
            Context::for_function(body)
                .compile(&*isa, &mut ControlPlane::default())
                .unwrap()
                .clone()
        };
        let fresh = forced_compile(body.clone());
        let fresh_rebound = forced_compile(rebound.clone());
        let compile = |placement, body: Function| {
            let cache = MachineCodeCache::open(&location, placement).unwrap();
            let mut context = Context::for_function(body);
            let code = cache
                .compile(&mut context, &*isa, &mut ControlPlane::default())
                .unwrap()
                .clone();
            let [relocation] = code.buffer.relocs() else {
                panic!("the external call requires one relocation")
            };
            let bound =
                ModuleReloc::from_mach_reloc(relocation, &context.func, FuncId::from_u32(0));
            let ModuleRelocTarget::User {
                namespace: 0,
                index,
            } = bound.name
            else {
                panic!("the external call must bind to a function declaration");
            };
            let hits = cache.hits.load(Ordering::Relaxed);
            let misses = cache.misses.load(Ordering::Relaxed);
            (code, index, hits, misses)
        };
        assert_eq!(
            compile(CodePlacement::Object, body.clone()),
            (fresh.clone(), 11, 0, 1)
        );
        assert_eq!(
            compile(CodePlacement::Object, rebound),
            (fresh_rebound, 12, 1, 0)
        );
        assert_eq!(compile(CodePlacement::Object, function(&*isa, 43, 11)).3, 1);
        #[cfg(feature = "jit")]
        {
            // The same function and ISA isolate just the code owner's kind.
            assert_eq!(
                compile(CodePlacement::ProcessMemory, body.clone()),
                (fresh.clone(), 11, 0, 1)
            );
            assert_eq!(
                compile(CodePlacement::ProcessMemory, body),
                (fresh, 11, 1, 0)
            );
        }
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn stencil_key_changes_for_one_codegen_input_at_a_time() {
        assert_ne!(
            store_identity("1").finish_hex(),
            store_identity("2").finish_hex()
        );
        assert_ne!(
            artifact_key(CodePlacement::Object, "abi_1", b"stencil"),
            artifact_key(CodePlacement::Object, "abi_2", b"stencil"),
        );
        let isa = super::super::SimpleBackend::native_isa(None, CodePlacement::Object);
        let body = function(&*isa, 1, 0);
        let key = compute_cache_key(&*isa, &body);
        assert!(key != compute_cache_key(&*isa, &function(&*isa, 2, 0)));

        let call = |argument| {
            let mut function = body.clone();
            let mut signature = Signature::new(isa.default_call_conv());
            signature.params.push(AbiParam::new(argument));
            let signature = function.import_signature(signature);
            function.import_function(ExtFuncData {
                name: ExternalName::testcase("callee"),
                signature,
                colocated: true,
                patchable: false,
            });
            function
        };
        assert!(
            compute_cache_key(&*isa, &call(types::I32))
                != compute_cache_key(&*isa, &call(types::I64))
        );

        let data = |offset: i64| {
            let mut function = body.clone();
            function.create_global_value(GlobalValueData::Symbol {
                name: ExternalName::testcase("data"),
                offset: offset.into(),
                colocated: true,
                tls: false,
            });
            function
        };
        assert!(compute_cache_key(&*isa, &data(0)) != compute_cache_key(&*isa, &data(8)));

        let target = if isa.triple().architecture == target_lexicon::Architecture::X86_64 {
            "aarch64-apple-darwin"
        } else {
            "x86_64-apple-darwin"
        };
        let alternate_isa =
            super::super::SimpleBackend::native_isa(Some(target), CodePlacement::Object);
        assert!(key != compute_cache_key(&*alternate_isa, &body));

        let different_opt = if isa.flags().opt_level() == settings::OptLevel::None {
            "speed"
        } else {
            "none"
        };
        let alternate_optimization =
            super::super::SimpleBackend::rebuild_owned_isa(&*isa, Some(different_opt)).unwrap();
        assert!(key != compute_cache_key(&*alternate_optimization, &body));
    }
}
