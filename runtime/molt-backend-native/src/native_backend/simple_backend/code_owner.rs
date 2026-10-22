//! The owner of the machine code that one native compilation emits.
//!
//! One code generator, two code owners (`docs/design/dev_jit_tier.md`). The
//! function compiler emits into `&mut dyn Module`; only this type knows whether
//! the code becomes a relocatable object for the system linker or executable
//! memory in the current process. rustc_codegen_cranelift splits its drivers
//! the same way (`ObjectModule` for AOT, `JITModule` for `jit` mode).

use super::*;

/// Where generated code lives. It decides the relocation model, so the ISA
/// constructor takes it: one target policy, one relocation policy per owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodePlacement {
    /// A relocatable object that the system linker places beside the runtime.
    Object,
    /// Executable memory in this process (`cranelift_jit`).
    #[cfg(feature = "jit")]
    ProcessMemory,
}

impl CodePlacement {
    /// `is_pic` for this owner on `triple`.
    ///
    /// Objects use Cranelift PIC (ELF/Mach-O GOT relocations) except on COFF,
    /// which uses native symbol relocations that the PE loader relocates.
    /// Process memory follows `cranelift_jit::JITBuilder::with_flags`: it
    /// supports GOT entries only on x86_64, where PIC keeps RIP-relative
    /// references in range through per-blob GOT slots.
    pub(crate) fn is_pic(self, triple: &target_lexicon::Triple) -> bool {
        match self {
            Self::Object => triple.binary_format != target_lexicon::BinaryFormat::Coff,
            #[cfg(feature = "jit")]
            Self::ProcessMemory => {
                matches!(triple.architecture, target_lexicon::Architecture::X86_64)
            }
        }
    }

    /// `use_colocated_libcalls` for this owner.
    ///
    /// In an object, generated code and libcalls share one link unit, so
    /// colocated (PC-relative) libcalls skip GOT/PLT indirection. In process
    /// memory, libcalls live in other images; `cranelift_jit` requires
    /// long-range libcall relocations because a colocated AArch64 call reaches
    /// only +/-128 MiB.
    pub(crate) fn colocated_libcalls(self) -> bool {
        match self {
            Self::Object => true,
            #[cfg(feature = "jit")]
            Self::ProcessMemory => false,
        }
    }
}

/// The module that owns generated code. It is itself a
/// `cranelift_module::Module`, so codegen never names the owner.
pub(crate) enum CodeOwner {
    Object(Box<ObjectModule>),
    #[cfg(feature = "jit")]
    ProcessMemory(Box<super::jit::JitCodeOwner>),
}

impl CodeOwner {
    /// The object this compilation emits. Only the object driver calls it.
    pub(crate) fn into_object(self) -> ObjectModule {
        match self {
            Self::Object(module) => *module,
            #[cfg(feature = "jit")]
            Self::ProcessMemory(_) => {
                panic!("native object emission requires an object code owner")
            }
        }
    }
}

impl CodeOwner {
    fn owner(&self) -> &dyn Module {
        match self {
            Self::Object(module) => module.as_ref(),
            #[cfg(feature = "jit")]
            Self::ProcessMemory(owner) => &owner.module,
        }
    }

    fn owner_mut(&mut self) -> &mut dyn Module {
        match self {
            Self::Object(module) => module.as_mut(),
            #[cfg(feature = "jit")]
            Self::ProcessMemory(owner) => &mut owner.module,
        }
    }
}

// Forward every trait method, as cranelift_module's own `impl Module for
// &mut M` does, so an owner behaves exactly like the module it holds.
impl Module for CodeOwner {
    fn isa(&self) -> &dyn isa::TargetIsa {
        self.owner().isa()
    }

    fn declarations(&self) -> &cranelift_module::ModuleDeclarations {
        self.owner().declarations()
    }

    fn get_name(&self, name: &str) -> Option<cranelift_module::FuncOrDataId> {
        self.owner().get_name(name)
    }

    fn target_config(&self) -> isa::TargetFrontendConfig {
        self.owner().target_config()
    }

    fn make_context(&self) -> Context {
        self.owner().make_context()
    }

    fn clear_context(&self, ctx: &mut Context) {
        self.owner().clear_context(ctx)
    }

    fn make_signature(&self) -> cranelift_codegen::ir::Signature {
        self.owner().make_signature()
    }

    fn clear_signature(&self, sig: &mut cranelift_codegen::ir::Signature) {
        self.owner().clear_signature(sig)
    }

    fn declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        signature: &cranelift_codegen::ir::Signature,
    ) -> cranelift_module::ModuleResult<cranelift_module::FuncId> {
        self.owner_mut().declare_function(name, linkage, signature)
    }

    fn declare_anonymous_function(
        &mut self,
        signature: &cranelift_codegen::ir::Signature,
    ) -> cranelift_module::ModuleResult<cranelift_module::FuncId> {
        self.owner_mut().declare_anonymous_function(signature)
    }

    fn declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> cranelift_module::ModuleResult<cranelift_module::DataId> {
        self.owner_mut().declare_data(name, linkage, writable, tls)
    }

    fn declare_anonymous_data(
        &mut self,
        writable: bool,
        tls: bool,
    ) -> cranelift_module::ModuleResult<cranelift_module::DataId> {
        self.owner_mut().declare_anonymous_data(writable, tls)
    }

    fn declare_func_in_func(
        &mut self,
        func: cranelift_module::FuncId,
        in_func: &mut Function,
    ) -> FuncRef {
        self.owner_mut().declare_func_in_func(func, in_func)
    }

    fn declare_data_in_func(
        &self,
        data: cranelift_module::DataId,
        func: &mut Function,
    ) -> cranelift_codegen::ir::GlobalValue {
        self.owner().declare_data_in_func(data, func)
    }

    fn declare_func_in_data(
        &self,
        func_id: cranelift_module::FuncId,
        data: &mut DataDescription,
    ) -> FuncRef {
        self.owner().declare_func_in_data(func_id, data)
    }

    fn declare_data_in_data(
        &self,
        data_id: cranelift_module::DataId,
        data: &mut DataDescription,
    ) -> cranelift_codegen::ir::GlobalValue {
        self.owner().declare_data_in_data(data_id, data)
    }

    fn define_function(
        &mut self,
        func: cranelift_module::FuncId,
        ctx: &mut Context,
    ) -> cranelift_module::ModuleResult<()> {
        self.owner_mut().define_function(func, ctx)
    }

    fn define_function_with_control_plane(
        &mut self,
        func: cranelift_module::FuncId,
        ctx: &mut Context,
        ctrl_plane: &mut cranelift_codegen::control::ControlPlane,
    ) -> cranelift_module::ModuleResult<()> {
        self.owner_mut()
            .define_function_with_control_plane(func, ctx, ctrl_plane)
    }

    fn define_function_bytes(
        &mut self,
        func_id: cranelift_module::FuncId,
        alignment: u64,
        bytes: &[u8],
        relocs: &[cranelift_module::ModuleReloc],
    ) -> cranelift_module::ModuleResult<()> {
        self.owner_mut()
            .define_function_bytes(func_id, alignment, bytes, relocs)
    }

    fn define_data(
        &mut self,
        data_id: cranelift_module::DataId,
        data: &DataDescription,
    ) -> cranelift_module::ModuleResult<()> {
        self.owner_mut().define_data(data_id, data)
    }
}
