//! LLVM backend for release-mode maximum optimization.
//!
//! Requires: `--features llvm` and LLVM 21 installed.
//!
//! This backend targets maximum runtime performance at the cost of
//! slower compilation. Use Cranelift backend for development iteration.

pub mod lowering;
pub mod pgo;
pub mod runtime_imports;
pub mod types;

#[cfg(feature = "llvm")]
use inkwell::OptimizationLevel;
#[cfg(feature = "llvm")]
use inkwell::builder::Builder;
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::targets::TargetMachine;
#[cfg(feature = "llvm")]
use std::collections::BTreeMap;

#[cfg(feature = "llvm")]
use crate::representation_plan::LlvmReprFacts;
#[cfg(feature = "llvm")]
use crate::tir::types::TirType;
#[cfg(feature = "llvm")]
pub struct LlvmBackend<'ctx> {
    pub context: &'ctx Context,
    pub module: Module<'ctx>,
    pub builder: Builder<'ctx>,
    pub function_param_types: BTreeMap<String, Vec<TirType>>,
    pub function_return_types: BTreeMap<String, TirType>,
    /// Per-function representation facts derived from the shared
    /// `ScalarRepresentationPlan`, keyed by function name. These drive the
    /// LLVM backend's integer-carrier and container dispatch decisions from the
    /// same typed facts the native/WASM/Luau backends consume.
    pub(crate) function_repr_facts: BTreeMap<String, LlvmReprFacts>,
    /// The set of `molt_*` symbols the linked runtime staticlib defines (the
    /// active stdlib profile's intrinsic surface, from
    /// `MOLT_RUNTIME_INTRINSIC_SYMBOLS`). Used by the generic preserved-op
    /// runtime-call fallback to confirm `molt_<kind>` actually exists before
    /// emitting an external call to it — so an operator kind with no dedicated
    /// lowering routes to its real runtime entry, and an unmappable kind fails
    /// the build loud instead of silently degrading to an operand-0 pass-through.
    /// Empty when the env var is unset (e.g. in-crate codegen unit tests that
    /// never link a final binary); the generic fallback then declines, matching
    /// the resolver machinery's same `cfg(test)` carve-out.
    pub(crate) runtime_intrinsic_symbols: std::collections::BTreeSet<String>,
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();
        // Set target triple for the host
        let triple = inkwell::targets::TargetMachine::get_default_triple();
        module.set_triple(&triple);
        Self {
            context,
            module,
            builder,
            function_param_types: BTreeMap::new(),
            function_return_types: BTreeMap::new(),
            function_repr_facts: BTreeMap::new(),
            runtime_intrinsic_symbols:
                crate::intrinsic_symbols::runtime_intrinsic_symbols_from_env().unwrap_or_default(),
        }
    }

    /// Get the compiled LLVM IR as a string (for debugging).
    pub fn dump_ir(&self) -> String {
        self.module.print_to_string().to_string()
    }
}

/// Optimization level for the LLVM compilation pipeline.
#[cfg(feature = "llvm")]
pub enum MoltOptLevel {
    /// No optimization (-O0), fastest compilation. Use for development.
    None,
    /// Standard optimization (-O2), good balance of speed and compile time.
    Speed,
    /// Aggressive optimization (-O3), maximum runtime performance. Use for release.
    Aggressive,
}

/// LTO (Link-Time Optimization) mode.
#[cfg(feature = "llvm")]
pub enum LtoMode {
    /// No LTO. Each module is compiled independently.
    None,
    /// Thin LTO — parallel, scalable, default for `--release`.
    Thin,
    /// Full LTO — maximum inter-procedural optimization, slowest link.
    Full,
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBackend<'ctx> {
    /// Initialize Polly polyhedral optimization via LLVM command-line flags.
    ///
    /// Polly is an LLVM plugin that performs polyhedral loop optimization:
    /// dependence analysis, tiling, interchange, vectorization via strip-mining.
    /// When the host LLVM is built with Polly support (`-DLLVM_ENABLE_PROJECTS=polly`),
    /// these flags activate it for all subsequent `run_passes` invocations.
    ///
    /// Called once before any compilation via `std::sync::Once`. If Polly is not
    /// available in the LLVM build, the flags are silently ignored — LLVM does not
    /// error on unknown `-mllvm` flags passed through `LLVMParseCommandLineOptions`.
    ///
    /// Flags:
    /// - `-polly`: enable polyhedral optimizer
    /// - `-polly-vectorizer=stripmine`: use strip-mining vectorization (better for
    ///   deep loop nests than the default polly vectorizer)
    /// - `-polly-parallel`: emit parallel code for independent loop iterations
    /// - `-polly-position=early`: run Polly early in the pipeline before LLVM's
    ///   own loop transforms can obscure polyhedral structure
    #[cfg(feature = "polly")]
    fn init_polly_once() {
        use std::sync::Once;
        static POLLY_INIT: Once = Once::new();
        POLLY_INIT.call_once(|| {
            use std::ffi::CString;
            let args: Vec<CString> = [
                "molt-backend",
                "-polly",
                "-polly-vectorizer=stripmine",
                "-polly-parallel",
                "-polly-position=early",
            ]
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect();
            let ptrs: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
            let overview = CString::new("Molt LLVM backend with Polly").unwrap();
            unsafe {
                llvm_sys::support::LLVMParseCommandLineOptions(
                    ptrs.len() as libc::c_int,
                    ptrs.as_ptr(),
                    overview.as_ptr(),
                );
            }
        });
    }

    /// Run the FULL LLVM O2/O3 optimization pipeline on the module.
    ///
    /// Before running the pipeline, all user-defined functions are marked
    /// with `dllexport` linkage so GlobalDCE/Internalize cannot remove
    /// them.  After optimization, linkage is restored to `External`.
    /// This gives us 100% of LLVM's optimization power (interprocedural
    /// inlining, GlobalDCE of unused helpers, SCCP, loop vectorization)
    /// while preserving all entry points the linker needs.
    ///
    /// When the `polly` feature is enabled and the host LLVM includes Polly,
    /// polyhedral loop optimizations (tiling, interchange, strip-mine
    /// vectorization) are applied automatically after the standard O2/O3
    /// pipeline, giving additional gains on loop-heavy numeric code.
    pub fn optimize(&self, opt_level: MoltOptLevel) -> Result<(), String> {
        // Activate Polly polyhedral optimizer (no-op if already initialized).
        #[cfg(feature = "polly")]
        Self::init_polly_once();

        let passes = match opt_level {
            MoltOptLevel::None => "default<O0>",
            MoltOptLevel::Speed => "default<O2>",
            MoltOptLevel::Aggressive => "default<O3>",
        };
        self.run_optimization_passes(&opt_level, passes)
    }

    fn run_optimization_passes(
        &self,
        opt_level: &MoltOptLevel,
        passes: &str,
    ) -> Result<(), String> {
        use inkwell::module::Linkage;
        use inkwell::passes::PassBuilderOptions;

        let target_machine = self.create_target_machine(opt_level);

        // Mark all externally-visible functions as dllexport so the
        // Internalize pass treats them as API and doesn't remove them.
        let mut preserved: Vec<(String, Linkage)> = Vec::new();
        let mut func = self.module.get_first_function();
        while let Some(f) = func {
            let linkage = f.get_linkage();
            if linkage == Linkage::External && f.count_basic_blocks() > 0 {
                preserved.push((f.get_name().to_str().unwrap_or("").to_string(), linkage));
                f.set_linkage(Linkage::DLLExport);
            }
            func = f.get_next_function();
        }

        let options = PassBuilderOptions::create();
        options.set_loop_vectorization(true);
        options.set_loop_slp_vectorization(true);
        options.set_loop_unrolling(true);
        options.set_loop_interleaving(true);
        options.set_merge_functions(true);
        let result = self
            .module
            .run_passes(passes, &target_machine, options)
            .map_err(|e| format!("LLVM optimization pipeline `{passes}` failed: {e}"));

        // Restore original linkage after optimization.
        for (name, linkage) in &preserved {
            if let Some(f) = self.module.get_function(name) {
                f.set_linkage(*linkage);
            }
        }

        result
    }

    /// Create a native target machine for the host CPU at the given opt level.
    fn create_target_machine(&self, opt_level: &MoltOptLevel) -> TargetMachine {
        use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target};

        Target::initialize_native(&InitializationConfig::default())
            .expect("Failed to initialize native target");

        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple).expect("Failed to get target from triple");
        let cpu = TargetMachine::get_host_cpu_name();
        let features = TargetMachine::get_host_cpu_features();

        let llvm_opt = match opt_level {
            MoltOptLevel::None => OptimizationLevel::None,
            MoltOptLevel::Speed => OptimizationLevel::Default,
            MoltOptLevel::Aggressive => OptimizationLevel::Aggressive,
        };

        target
            .create_target_machine(
                &triple,
                cpu.to_str().unwrap(),
                features.to_str().unwrap(),
                llvm_opt,
                RelocMode::Default,
                CodeModel::Default,
            )
            .expect("Failed to create target machine")
    }

    /// Emit LLVM bitcode to a file (used for LTO pipelines).
    ///
    /// Returns `true` on success. The emitted `.bc` file can be passed to
    /// `llvm-lto` or the linker's LTO plugin for cross-module optimization.
    pub fn emit_bitcode(&self, path: &std::path::Path) -> bool {
        self.module.write_bitcode_to_path(path)
    }

    /// Emit a native object file (`.o`) at the given optimization level.
    pub fn emit_object(
        &self,
        path: &std::path::Path,
        opt_level: MoltOptLevel,
    ) -> Result<(), String> {
        use inkwell::targets::FileType;

        let target_machine = self.create_target_machine(&opt_level);
        target_machine
            .write_to_file(&self.module, FileType::Object, path)
            .map_err(|e| e.to_string())
    }

    /// Emit LLVM assembly (`.s`) for debugging or manual inspection.
    pub fn emit_llvm_asm(
        &self,
        path: &std::path::Path,
        opt_level: MoltOptLevel,
    ) -> Result<(), String> {
        use inkwell::targets::FileType;

        let target_machine = self.create_target_machine(&opt_level);
        target_machine
            .write_to_file(&self.module, FileType::Assembly, path)
            .map_err(|e| e.to_string())
    }

    /// Emit the per-app intrinsic resolver `molt_app_resolve_intrinsic` into the
    /// LLVM module, structurally identical to the Cranelift backend's
    /// `SimpleBackend::emit_app_resolver_function`.
    ///
    /// The main stub (emitted by the CLI) references this symbol and registers
    /// it with the runtime via `molt_set_app_intrinsic_resolver` before
    /// `molt_runtime_init`. Without it the LLVM-compiled application object
    /// leaves `_molt_app_resolve_intrinsic` undefined at link, and every
    /// name-based intrinsic resolution at runtime would fail because
    /// `resolve_symbol`/`resolve_core_symbol` are intentionally left
    /// native-unreachable for dead-stripping.
    ///
    /// Layout (mirrors the Cranelift emitter so the two backends are byte-for-
    /// byte interchangeable at the ABI level):
    ///
    /// * `molt_app_intrinsic_names`: the manifest names, sorted by unsigned-byte
    ///   lexicographic order (the `BTreeSet` iteration order) and concatenated.
    /// * `molt_app_intrinsic_table`: N fixed-size 16-byte records, one per name,
    ///   `{ i32 name_off, i32 name_len, ptr func_ptr }`, sorted to match the
    ///   names blob. `func_ptr` is the address of the intrinsic — LLVM emits the
    ///   pointer relocation that the linker resolves against the runtime
    ///   staticlib. The intrinsics are declared `External`, so `-dead_strip` /
    ///   `--gc-sections` still removes every intrinsic whose name appears in no
    ///   manifest record.
    /// * `molt_app_resolve_intrinsic`: an `i64 (ptr, i64)` binary-search lookup
    ///   over the sorted table, returning the intrinsic function pointer as a
    ///   `u64`, or 0 when the name is not in the manifest.
    #[cfg(feature = "llvm")]
    pub fn emit_app_resolver_function(&self, manifest_names: &std::collections::BTreeSet<String>) {
        use inkwell::AddressSpace;
        use inkwell::IntPredicate;
        use inkwell::module::Linkage;

        if std::env::var("MOLT_DUMP_INTRINSIC_MANIFEST").as_deref() == Ok("1") {
            eprintln!("MOLT_INTRINSIC_MANIFEST: count={}", manifest_names.len());
            for name in manifest_names {
                eprintln!("MOLT_INTRINSIC_MANIFEST: {name}");
            }
        }

        let ctx = self.context;
        let module = &self.module;
        let builder = self.context.create_builder();
        let i8_ty = ctx.i8_type();
        let i32_ty = ctx.i32_type();
        let i64_ty = ctx.i64_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());

        // 16-byte record: { i32 name_off, i32 name_len, ptr func_ptr }.
        let record_ty = ctx.struct_type(&[i32_ty.into(), i32_ty.into(), ptr_ty.into()], false);

        // Build the concatenated names blob and the record initializers. The
        // manifest is a `BTreeSet`, so iteration is already unsigned-byte
        // lexicographic — exactly the order the binary search requires.
        let mut names_blob: Vec<u8> = Vec::new();
        let mut record_values: Vec<inkwell::values::StructValue> =
            Vec::with_capacity(manifest_names.len());
        for name in manifest_names {
            let off = names_blob.len();
            let bytes = name.as_bytes();
            assert!(
                off <= u32::MAX as usize && bytes.len() <= u32::MAX as usize,
                "app resolver: intrinsic name table exceeds u32 addressing"
            );
            names_blob.extend_from_slice(bytes);

            // Address-take the intrinsic. Declare it `External` if a prior
            // runtime-import declaration did not already create it; with opaque
            // pointers the declared signature is irrelevant for address use.
            let intrinsic_fn = module.get_function(name).unwrap_or_else(|| {
                let placeholder_ty = i64_ty.fn_type(&[i64_ty.into()], false);
                module.add_function(name, placeholder_ty, Some(Linkage::External))
            });
            let func_addr = intrinsic_fn.as_global_value().as_pointer_value();

            let record = record_ty.const_named_struct(&[
                i32_ty.const_int(off as u64, false).into(),
                i32_ty.const_int(bytes.len() as u64, false).into(),
                func_addr.into(),
            ]);
            record_values.push(record);
        }
        let count = record_values.len();

        // Names blob global (immutable, private — dead-strippable when the
        // resolver itself is unreferenced).
        let names_array_ty = i8_ty.array_type(names_blob.len() as u32);
        let names_global = module.add_global(names_array_ty, None, "molt_app_intrinsic_names");
        names_global.set_linkage(Linkage::Private);
        names_global.set_constant(true);
        names_global.set_unnamed_addr(true);
        names_global.set_initializer(&ctx.const_string(&names_blob, false));

        // Record table global. The `ptr` field initializers carry the pointer
        // relocations LLVM lowers for the linker.
        let table_array_ty = record_ty.array_type(count as u32);
        let table_global = module.add_global(table_array_ty, None, "molt_app_intrinsic_table");
        table_global.set_linkage(Linkage::Private);
        table_global.set_constant(true);
        let table_init = record_ty.const_array(&record_values);
        table_global.set_initializer(&table_init);

        // Resolver function: i64 molt_app_resolve_intrinsic(ptr name, i64 len).
        // Matches the C stub declaration `unsigned long long(const char*,
        // unsigned long long)` and the Cranelift emitter's ABI.
        let resolver_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
        let resolver = module.add_function(
            "molt_app_resolve_intrinsic",
            resolver_ty,
            Some(Linkage::External),
        );

        let entry_bb = ctx.append_basic_block(resolver, "entry");
        let not_found_bb = ctx.append_basic_block(resolver, "not_found");
        builder.position_at_end(entry_bb);

        let name_ptr = resolver.get_nth_param(0).unwrap().into_pointer_value();
        let name_addr = builder
            .build_ptr_to_int(name_ptr, i64_ty, "name_addr")
            .unwrap();
        let name_len = resolver.get_nth_param(1).unwrap().into_int_value();

        if count == 0 {
            builder.build_unconditional_branch(not_found_bb).unwrap();
        } else {
            let names_base = builder
                .build_ptr_to_int(names_global.as_pointer_value(), i64_ty, "names_base")
                .unwrap();

            // Binary search over [lo, hi). loop_head carries (lo, hi).
            let loop_head_bb = ctx.append_basic_block(resolver, "loop_head");
            let probe_bb = ctx.append_basic_block(resolver, "probe");
            let hit_bb = ctx.append_basic_block(resolver, "hit");
            let go_lr_bb = ctx.append_basic_block(resolver, "go_left_or_right");
            let left_bb = ctx.append_basic_block(resolver, "left");
            let right_bb = ctx.append_basic_block(resolver, "right");

            let zero64 = i64_ty.const_zero();
            let count_val = i64_ty.const_int(count as u64, false);
            builder.build_unconditional_branch(loop_head_bb).unwrap();

            // loop_head: phi(lo, hi). if lo < hi -> probe else not_found.
            builder.position_at_end(loop_head_bb);
            let lo_phi = builder.build_phi(i64_ty, "lo").unwrap();
            let hi_phi = builder.build_phi(i64_ty, "hi").unwrap();
            lo_phi.add_incoming(&[(&zero64, entry_bb)]);
            hi_phi.add_incoming(&[(&count_val, entry_bb)]);
            let lo = lo_phi.as_basic_value().into_int_value();
            let hi = hi_phi.as_basic_value().into_int_value();
            let range_nonempty = builder
                .build_int_compare(IntPredicate::SLT, lo, hi, "range_nonempty")
                .unwrap();
            builder
                .build_conditional_branch(range_nonempty, probe_bb, not_found_bb)
                .unwrap();

            // probe: mid = lo + (hi - lo) / 2; load record(mid); compare.
            builder.position_at_end(probe_bb);
            let span = builder.build_int_sub(hi, lo, "span").unwrap();
            let half = builder
                .build_right_shift(span, i64_ty.const_int(1, false), false, "half")
                .unwrap();
            let mid = builder.build_int_add(lo, half, "mid").unwrap();
            // record(mid): GEP into the table, then load off/len and func ptr.
            let rec_ptr = unsafe {
                builder
                    .build_in_bounds_gep(
                        table_array_ty,
                        table_global.as_pointer_value(),
                        &[zero64, mid],
                        "rec_ptr",
                    )
                    .unwrap()
            };
            let off_ptr = builder
                .build_struct_gep(record_ty, rec_ptr, 0, "off_ptr")
                .unwrap();
            let cand_off32 = builder
                .build_load(i32_ty, off_ptr, "cand_off32")
                .unwrap()
                .into_int_value();
            let len_ptr = builder
                .build_struct_gep(record_ty, rec_ptr, 1, "len_ptr")
                .unwrap();
            let cand_len32 = builder
                .build_load(i32_ty, len_ptr, "cand_len32")
                .unwrap()
                .into_int_value();
            let cand_off = builder
                .build_int_z_extend(cand_off32, i64_ty, "cand_off")
                .unwrap();
            let cand_len = builder
                .build_int_z_extend(cand_len32, i64_ty, "cand_len")
                .unwrap();
            let cand_addr = builder
                .build_int_add(names_base, cand_off, "cand_addr")
                .unwrap();

            // cmp = lexicographic_compare(query, candidate) in {-1, 0, 1}.
            let cmp = Self::emit_llvm_lexicographic_compare(
                ctx, &builder, resolver, name_addr, name_len, cand_addr, cand_len,
            );

            let is_eq = builder
                .build_int_compare(IntPredicate::EQ, cmp, i64_ty.const_zero(), "is_eq")
                .unwrap();
            builder
                .build_conditional_branch(is_eq, hit_bb, go_lr_bb)
                .unwrap();

            // hit: load and return func_ptr at record(mid).field(2).
            builder.position_at_end(hit_bb);
            let fp_ptr = builder
                .build_struct_gep(record_ty, rec_ptr, 2, "fp_ptr")
                .unwrap();
            let func_ptr_val = builder
                .build_load(ptr_ty, fp_ptr, "func_ptr")
                .unwrap()
                .into_pointer_value();
            let func_ptr_int = builder
                .build_ptr_to_int(func_ptr_val, i64_ty, "func_ptr_int")
                .unwrap();
            builder.build_return(Some(&func_ptr_int)).unwrap();

            // go_left_or_right: cmp < 0 -> left [lo, mid); else right [mid+1, hi).
            builder.position_at_end(go_lr_bb);
            let cmp_lt = builder
                .build_int_compare(IntPredicate::SLT, cmp, i64_ty.const_zero(), "cmp_lt")
                .unwrap();
            builder
                .build_conditional_branch(cmp_lt, left_bb, right_bb)
                .unwrap();

            builder.position_at_end(left_bb);
            lo_phi.add_incoming(&[(&lo, left_bb)]);
            hi_phi.add_incoming(&[(&mid, left_bb)]);
            builder.build_unconditional_branch(loop_head_bb).unwrap();

            builder.position_at_end(right_bb);
            let mid_plus_1 = builder
                .build_int_add(mid, i64_ty.const_int(1, false), "mid_plus_1")
                .unwrap();
            lo_phi.add_incoming(&[(&mid_plus_1, right_bb)]);
            hi_phi.add_incoming(&[(&hi, right_bb)]);
            builder.build_unconditional_branch(loop_head_bb).unwrap();
        }

        // not_found: return 0.
        builder.position_at_end(not_found_bb);
        builder.build_return(Some(&i64_ty.const_zero())).unwrap();
    }

    /// Emit an unsigned byte-wise lexicographic comparison of two runtime byte
    /// ranges `(a_addr, a_len)` and `(b_addr, b_len)` (both integer addresses),
    /// returning an `i64` in `{-1, 0, 1}` — the same ordering `BTreeSet<String>`
    /// uses to sort the table, so binary search is consistent. Mirrors the
    /// Cranelift backend's `emit_lexicographic_compare`.
    #[cfg(feature = "llvm")]
    fn emit_llvm_lexicographic_compare<'a>(
        ctx: &'a Context,
        builder: &Builder<'a>,
        func: inkwell::values::FunctionValue<'a>,
        a_addr: inkwell::values::IntValue<'a>,
        a_len: inkwell::values::IntValue<'a>,
        b_addr: inkwell::values::IntValue<'a>,
        b_len: inkwell::values::IntValue<'a>,
    ) -> inkwell::values::IntValue<'a> {
        use inkwell::AddressSpace;
        use inkwell::IntPredicate;

        let i8_ty = ctx.i8_type();
        let i64_ty = ctx.i64_type();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let neg_one = i64_ty.const_int((-1i64) as u64, true);
        let zero = i64_ty.const_zero();
        let one = i64_ty.const_int(1, false);

        // min_len = min(a_len, b_len).
        let a_lt_b_len = builder
            .build_int_compare(IntPredicate::ULT, a_len, b_len, "a_lt_b_len")
            .unwrap();
        let min_len = builder
            .build_select(a_lt_b_len, a_len, b_len, "min_len")
            .unwrap()
            .into_int_value();

        let head_bb = ctx.append_basic_block(func, "cmp_head");
        let body_bb = ctx.append_basic_block(func, "cmp_body");
        let advance_bb = ctx.append_basic_block(func, "cmp_advance");
        let diff_bb = ctx.append_basic_block(func, "cmp_diff");
        let tail_bb = ctx.append_basic_block(func, "cmp_tail");
        let ret_bb = ctx.append_basic_block(func, "cmp_ret");

        let pre_bb = builder.get_insert_block().unwrap();
        builder.build_unconditional_branch(head_bb).unwrap();

        // head: phi(i). if i < min_len -> body else tail.
        builder.position_at_end(head_bb);
        let i_phi = builder.build_phi(i64_ty, "i").unwrap();
        i_phi.add_incoming(&[(&zero, pre_bb)]);
        let i = i_phi.as_basic_value().into_int_value();
        let in_range = builder
            .build_int_compare(IntPredicate::ULT, i, min_len, "in_range")
            .unwrap();
        builder
            .build_conditional_branch(in_range, body_bb, tail_bb)
            .unwrap();

        // body: load byte a[i], b[i]; equal -> advance, else -> diff.
        builder.position_at_end(body_bb);
        let a_byte_addr = builder.build_int_add(a_addr, i, "a_byte_addr").unwrap();
        let b_byte_addr = builder.build_int_add(b_addr, i, "b_byte_addr").unwrap();
        let a_byte_ptr = builder
            .build_int_to_ptr(a_byte_addr, ptr_ty, "a_byte_ptr")
            .unwrap();
        let b_byte_ptr = builder
            .build_int_to_ptr(b_byte_addr, ptr_ty, "b_byte_ptr")
            .unwrap();
        let a_byte8 = builder
            .build_load(i8_ty, a_byte_ptr, "a_byte8")
            .unwrap()
            .into_int_value();
        let b_byte8 = builder
            .build_load(i8_ty, b_byte_ptr, "b_byte8")
            .unwrap()
            .into_int_value();
        let a_byte = builder
            .build_int_z_extend(a_byte8, i64_ty, "a_byte")
            .unwrap();
        let b_byte = builder
            .build_int_z_extend(b_byte8, i64_ty, "b_byte")
            .unwrap();
        let bytes_eq = builder
            .build_int_compare(IntPredicate::EQ, a_byte, b_byte, "bytes_eq")
            .unwrap();
        builder
            .build_conditional_branch(bytes_eq, advance_bb, diff_bb)
            .unwrap();

        // advance: i += 1, continue.
        builder.position_at_end(advance_bb);
        let next_i = builder.build_int_add(i, one, "next_i").unwrap();
        i_phi.add_incoming(&[(&next_i, advance_bb)]);
        builder.build_unconditional_branch(head_bb).unwrap();

        // diff: sign of (a_byte - b_byte).
        builder.position_at_end(diff_bb);
        let a_lt_b = builder
            .build_int_compare(IntPredicate::ULT, a_byte, b_byte, "a_lt_b")
            .unwrap();
        let diff_sign = builder
            .build_select(a_lt_b, neg_one, one, "diff_sign")
            .unwrap()
            .into_int_value();
        builder.build_unconditional_branch(ret_bb).unwrap();

        // tail: common prefix equal — order by length.
        builder.position_at_end(tail_bb);
        let len_lt = builder
            .build_int_compare(IntPredicate::ULT, a_len, b_len, "len_lt")
            .unwrap();
        let len_gt = builder
            .build_int_compare(IntPredicate::UGT, a_len, b_len, "len_gt")
            .unwrap();
        let lt_or_zero = builder
            .build_select(len_lt, neg_one, zero, "lt_or_zero")
            .unwrap()
            .into_int_value();
        let tail_result = builder
            .build_select(len_gt, one, lt_or_zero, "tail_result")
            .unwrap()
            .into_int_value();
        builder.build_unconditional_branch(ret_bb).unwrap();

        // ret: phi(result).
        builder.position_at_end(ret_bb);
        let result_phi = builder.build_phi(i64_ty, "cmp_result").unwrap();
        result_phi.add_incoming(&[(&diff_sign, diff_bb), (&tail_result, tail_bb)]);
        result_phi.as_basic_value().into_int_value()
    }
}

#[cfg(test)]
#[cfg(feature = "llvm")]
mod tests {
    use super::*;
    use inkwell::context::Context;

    #[test]
    fn test_backend_module_name() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "test_module");
        let ir = backend.dump_ir();
        assert!(
            ir.contains("test_module"),
            "IR should contain the module name, got: {ir}"
        );
    }

    #[test]
    fn test_create_target_machine_all_levels() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "tm_test");
        // Simply ensure no panic at any opt level.
        let _tm_none = backend.create_target_machine(&MoltOptLevel::None);
        let _tm_speed = backend.create_target_machine(&MoltOptLevel::Speed);
        let _tm_agg = backend.create_target_machine(&MoltOptLevel::Aggressive);
    }

    #[test]
    fn test_optimize_empty_module_smoke() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "opt_smoke");
        // Running passes on an empty module must not panic or error.
        backend
            .optimize(MoltOptLevel::Speed)
            .expect("empty-module optimization should succeed");
    }

    #[test]
    fn test_optimize_invalid_pipeline_fails_closed_and_restores_linkage() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "opt_fail_closed");
        let i64_ty = ctx.i64_type();
        let func = backend.module.add_function(
            "visible_entry",
            i64_ty.fn_type(&[], false),
            Some(inkwell::module::Linkage::External),
        );
        let entry = ctx.append_basic_block(func, "entry");
        backend.builder.position_at_end(entry);
        backend
            .builder
            .build_return(Some(&i64_ty.const_zero()))
            .unwrap();

        let err = backend
            .run_optimization_passes(&MoltOptLevel::Speed, "not-a-real-pass")
            .expect_err("invalid LLVM pass pipeline must fail closed");
        assert!(
            err.contains("not-a-real-pass"),
            "error should identify the rejected pass pipeline: {err}"
        );
        assert_eq!(
            func.get_linkage(),
            inkwell::module::Linkage::External,
            "temporary dllexport linkage must be restored after optimizer failure"
        );
    }

    #[test]
    fn test_dump_ir_contains_module_name() {
        let ctx = Context::create();
        let backend = LlvmBackend::new(&ctx, "ir_dump_test");
        let ir = backend.dump_ir();
        assert!(
            ir.contains("ir_dump_test"),
            "dump_ir() should include the module name in the IR string"
        );
    }
}
