use super::*;

// Shared Cranelift / std imports for the native (Cranelift) backend module
// tree. These live here — the common ancestor of `simple_backend` and
// `function_compiler` — so both submodules pick them up unqualified through
// their `use super::*` glob (module-ancestry privacy), exactly as they did
// when `SimpleBackend` and its codegen impls lived at the crate root in
// `lib.rs`. This whole module is `#[cfg(feature = "native-backend")]` via its
// declaration in `lib.rs`, so the imports are native-only without per-line
// gating.
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Block, BlockArg, FuncRef, Function, InstBuilder, MemFlagsData, StackSlotData,
    StackSlotKind, Value, types,
};
use cranelift_codegen::isa;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule, ObjectProduct};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;

const _: () = assert!(
    molt_codegen_abi::MOLT_FLAGS_ATOMIC == cfg!(not(target_arch = "wasm32"))
        && cfg!(feature = "free-threaded") == molt_codegen_abi::MOLT_REFCOUNT_ATOMIC,
    "molt-backend-native/free-threaded must exactly match molt-codegen-abi/free-threaded",
);

/// Attach a retained, zero-runtime-cost relocation to the runtime's exact
/// generated-object ABI. The full fact fingerprint and execution mode are
/// encoded in the imported symbol, so independently cached backend and runtime
/// artifacts fail at link admission instead of producing mixed contracts.
fn emit_generated_object_abi_anchor(module: &mut ObjectModule) {
    let witness = module
        .declare_data(
            molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL,
            Linkage::Import,
            false,
            false,
        )
        .expect("declare generated-object ABI witness import");
    let anchor = module
        .declare_data(
            GENERATED_OBJECT_ABI_ANCHOR_SYMBOL,
            Linkage::Local,
            false,
            false,
        )
        .expect("declare generated-object ABI anchor");
    let mut description = DataDescription::new();
    description.set_align(u64::from(module.target_config().pointer_type().bytes()));
    // A relocation needs real pointer bytes. Zero-fill storage emits a Mach-O
    // __bss relocation, which is not a linkable ABI witness (Apple ld crashes).
    description
        .define(vec![0; module.target_config().pointer_type().bytes() as usize].into_boxed_slice());
    description.set_used(true);
    let witness_ref = module.declare_data_in_data(witness, &mut description);
    description.write_data_addr(0, witness_ref, 0);
    module
        .define_data(anchor, &description)
        .expect("define generated-object ABI anchor");
}

/// Finish the target object's link-admission metadata before serializing it.
/// `set_used` supplies the ELF/Mach-O retention flags; ELF additionally needs
/// the GNU extension namespace and COFF needs a linker root directive.
fn finalize_native_object(mut product: ObjectProduct) -> ObjectProduct {
    use cranelift_object::object::{BinaryFormat, FileFlags, SectionKind, elf};
    match product.object.format() {
        BinaryFormat::Elf => {
            // Match LLVM's ELFObjectWriter: GNU features upgrade only an
            // unspecified OSABI. GNU denotes ELF extensions, not a host/Linux
            // target substitution. Keep explicit OSABI, ABI version and flags.
            match &mut product.object.flags {
                FileFlags::None => {
                    product.object.flags = FileFlags::Elf {
                        os_abi: elf::ELFOSABI_GNU,
                        abi_version: 0,
                        e_flags: 0,
                    };
                }
                FileFlags::Elf { os_abi, .. } if *os_abi == elf::ELFOSABI_NONE => {
                    *os_abi = elf::ELFOSABI_GNU;
                }
                _ => {}
            }
        }
        BinaryFormat::Coff => {
            // Object's symbol owner already applied target decoration (e.g.
            // i386's leading underscore). Root that exact emitted spelling.
            let witness = product
                .object
                .symbol_id(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL.as_bytes())
                .expect("generated-object ABI witness must precede finalization");
            let mut directives = b" /INCLUDE:".to_vec();
            directives.extend_from_slice(&product.object.symbol(witness).name);
            let section =
                product
                    .object
                    .add_section(Vec::new(), b".drectve".to_vec(), SectionKind::Linker);
            product.object.append_section_data(section, &directives, 1);
        }
        // Cranelift already derives LC_BUILD_VERSION from the actual Apple
        // target, including platform and explicit deployment version. Preserve
        // it instead of overwriting cross-target metadata from the build host.
        BinaryFormat::MachO => {}
        format => panic!("unsupported native object format: {format:?}"),
    }
    product
}

/// Load MoltHeader's shared flag word under the selected execution contract.
/// The two generated consumers use only `HAS_PTRS` and `IMMORTAL`, which are
/// metadata facts whose payload visibility is established elsewhere. An
/// aligned target load is the exact relaxed atomic access for the native ABI;
/// Cranelift's `atomic_load` is sequentially consistent and would emit an
/// unnecessary `ldar` on AArch64. State publication remains in runtime helpers.
#[inline]
pub(crate) fn emit_header_flags_load(
    builder: &mut FunctionBuilder<'_>,
    object_ptr: Value,
) -> Value {
    builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        object_ptr,
        HEADER_FLAGS_OFFSET,
    )
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct TrampolineKey {
    pub(crate) name: String,
    pub(crate) arity: usize,
    pub(crate) has_closure: bool,
    pub(crate) is_import: bool,
    pub(crate) kind: TrampolineKind,
    pub(crate) closure_size: i64,
    pub(crate) target_has_ret: bool,
}

pub(crate) mod vec_layout;
pub(crate) use vec_layout::vec_u64_layout;

mod simple_backend;
// `lib.rs` re-exports the externally-public backend types
// (`molt_backend::SimpleBackend`, etc.). Each item keeps its declared
// visibility through this glob, so crate-internal items stay `pub(crate)`.
pub use simple_backend::*;

mod function_compiler;

#[cfg(test)]
mod header_flags_tests {
    use super::*;
    use cranelift_object::object::Object;

    #[test]
    fn generated_metadata_flag_reads_are_relaxed_in_every_execution_mode() {
        let mut function = Function::new();
        function.signature.params.push(AbiParam::new(types::I64));
        function.signature.returns.push(AbiParam::new(types::I32));
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let object_ptr = builder.block_params(entry)[0];
            let flags = emit_header_flags_load(&mut builder, object_ptr);
            builder.ins().return_(&[flags]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        let ir = function.display().to_string();
        assert!(
            ir.contains("load.i32") && !ir.contains("atomic_load"),
            "metadata flags must use the relaxed generated load class:\n{ir}"
        );
        assert_eq!(
            molt_codegen_abi::MOLT_FLAGS_ATOMIC,
            cfg!(not(target_arch = "wasm32")),
            "native generated metadata flags must always be atomic",
        );
    }

    #[test]
    fn aarch64_metadata_flag_load_uses_ldr_not_ldar() {
        use cranelift_codegen::control::ControlPlane;

        let backend = SimpleBackend::new_with_target(Some("aarch64-unknown-linux-gnu"));
        let isa = backend.module.isa();
        let mut function = Function::new();
        function.signature.call_conv = isa.default_call_conv();
        function.signature.params.push(AbiParam::new(types::I64));
        function.signature.returns.push(AbiParam::new(types::I32));
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut builder_context);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let object_ptr = builder.block_params(entry)[0];
            let flags = emit_header_flags_load(&mut builder, object_ptr);
            builder.ins().return_(&[flags]);
            builder.seal_all_blocks();
            builder.finalize();
        }
        let mut context = Context::for_function(function);
        context
            .compile(isa, &mut ControlPlane::default())
            .expect("compile AArch64 flag load");
        let code = context
            .compiled_code()
            .expect("compiled AArch64 code")
            .buffer
            .data();
        let instructions: Vec<u32> = code
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&bytes| u32::from_le_bytes(bytes))
            .collect();
        assert!(
            instructions.iter().any(|instruction| {
                instruction & 0xffe0_0c00 == 0xb840_0000 || instruction & 0xffc0_0000 == 0xb940_0000
            }),
            "expected an AArch64 LDR/LDUR W metadata load: {instructions:08x?}"
        );
        assert!(
            instructions
                .iter()
                .all(|instruction| instruction & 0xffff_fc00 != 0x88df_fc00),
            "metadata load must not emit acquire LDAR W: {instructions:08x?}"
        );
    }

    #[test]
    fn generated_object_retains_exact_abi_import_in_initialized_storage() {
        use cranelift_object::object::{ObjectSection, ObjectSymbol, SectionKind};

        // Inspect target objects, not host-based symbol assumptions. Real host
        // final links are separately exercised by native_callable_dispatch.
        for target in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
            "aarch64-pc-windows-msvc",
            "x86_64-pc-windows-msvc",
        ] {
            let backend = SimpleBackend::new_with_target(Some(target));
            let mut module = backend.module;
            emit_generated_object_abi_anchor(&mut module);
            let product = module.finish();
            let macho_before = (product.object.format()
                == cranelift_object::object::BinaryFormat::MachO)
                .then(|| product.object.write().unwrap());
            let bytes = finalize_native_object(product)
                .emit()
                .expect("emit ABI witness object");
            if let Some(before) = macho_before {
                assert_eq!(bytes, before, "{target}: preserve target Mach-O metadata");
            }
            let object =
                cranelift_object::object::File::parse(&*bytes).expect("parse ABI witness object");
            match object.format() {
                cranelift_object::object::BinaryFormat::Elf => assert!(matches!(
                    object.flags(),
                    cranelift_object::object::FileFlags::Elf { os_abi, .. }
                        if os_abi == cranelift_object::object::elf::ELFOSABI_GNU
                )),
                cranelift_object::object::BinaryFormat::Coff => assert_eq!(
                    object.section_by_name(".drectve").unwrap().data().unwrap(),
                    format!(
                        " /INCLUDE:{}",
                        molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL
                    )
                    .as_bytes(),
                    "{target}: target COFF retention cannot depend on the host"
                ),
                _ => {}
            }
            let pointer_bytes = if object.is_64() { 8 } else { 4 };
            let anchor = object
                .symbols()
                .find(|symbol| {
                    symbol.name().is_ok_and(|name| {
                        name == GENERATED_OBJECT_ABI_ANCHOR_SYMBOL
                            || (object.format() == cranelift_object::object::BinaryFormat::MachO
                                && name.strip_prefix('_')
                                    == Some(GENERATED_OBJECT_ABI_ANCHOR_SYMBOL))
                    })
                })
                .expect("retained generated-object ABI anchor");
            let anchor_section = object
                .section_by_index(anchor.section_index().expect("anchor section"))
                .expect("read anchor section");
            assert!(
                anchor_section.align() >= pointer_bytes,
                "{target}: linker must guarantee pointer alignment for the ABI anchor"
            );
            assert_eq!(
                (anchor.address() - anchor_section.address()) % pointer_bytes,
                0,
                "{target}: ABI anchor must begin at a pointer-aligned section offset"
            );
            let symbols = crate::test_support::native_object_symbols(&bytes);
            assert!(
                symbols
                    .undefined
                    .contains(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL),
                "{target}"
            );
            let opposite = if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC {
                molt_codegen_abi::GENERATED_OBJECT_ABI_GIL_SYMBOL
            } else {
                molt_codegen_abi::GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL
            };
            assert!(!symbols.undefined.contains(opposite), "{target}");
            let mut relocations = 0;
            for section in object.sections() {
                for (offset, _) in section.relocations() {
                    relocations += 1;
                    assert_ne!(
                        section.kind(),
                        SectionKind::UninitializedData,
                        "{target}: ABI anchor relocation must not reside in zero-fill storage"
                    );
                    assert!(
                        offset + pointer_bytes <= section.data().unwrap().len() as u64,
                        "{target}: ABI anchor relocation requires initialized pointer bytes"
                    );
                }
            }
            assert_eq!(
                relocations, 1,
                "{target}: exactly one ABI witness relocation"
            );
        }
    }

    #[test]
    fn retained_elf_finalization_preserves_explicit_header_facts() {
        use cranelift_object::object::{FileFlags, elf};
        for (os_abi, expected) in [
            (elf::ELFOSABI_NONE, elf::ELFOSABI_GNU),
            (elf::ELFOSABI_GNU, elf::ELFOSABI_GNU),
            (elf::ELFOSABI_FREEBSD, elf::ELFOSABI_FREEBSD),
        ] {
            let mut module =
                SimpleBackend::new_with_target(Some("x86_64-unknown-linux-gnu")).module;
            emit_generated_object_abi_anchor(&mut module);
            let mut product = module.finish();
            product.object.flags = FileFlags::Elf {
                os_abi,
                abi_version: 7,
                e_flags: 0x1020,
            };
            let bytes = finalize_native_object(product).emit().unwrap();
            let object = cranelift_object::object::File::parse(&*bytes).unwrap();
            assert_eq!(
                object.flags(),
                FileFlags::Elf {
                    os_abi: expected,
                    abi_version: 7,
                    e_flags: 0x1020,
                }
            );
        }
    }

    #[test]
    fn coff_retention_roots_the_target_decorated_import() {
        use cranelift_object::object::{self, ObjectSection, ObjectSymbol};
        // Exercise object metadata independently of Cranelift's supported ISAs;
        // this does not claim native i386 code generation or linker support.
        let mut object = object::write::Object::new(
            object::BinaryFormat::Coff,
            object::Architecture::I386,
            object::Endianness::Little,
        );
        object.add_symbol(object::write::Symbol {
            name: molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL
                .as_bytes()
                .to_vec(),
            value: 0,
            size: 0,
            kind: object::SymbolKind::Data,
            scope: object::SymbolScope::Unknown,
            weak: false,
            section: object::write::SymbolSection::Undefined,
            flags: object::SymbolFlags::None,
        });
        let bytes = finalize_native_object(ObjectProduct {
            object,
            functions: Default::default(),
            data_objects: Default::default(),
        })
        .emit()
        .unwrap();
        let file = object::File::parse(&*bytes).unwrap();
        let expected = format!("_{}", molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL);
        assert!(file.symbols().any(
            |symbol| symbol.is_undefined() && symbol.name().is_ok_and(|name| name == expected)
        ));
        assert_eq!(
            file.section_by_name(".drectve").unwrap().data().unwrap(),
            format!(" /INCLUDE:{expected}").as_bytes()
        );
    }
}
