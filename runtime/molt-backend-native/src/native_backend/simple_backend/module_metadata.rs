use super::*;
use crate::stdlib_modules::{is_user_owned_function, partition_user_owned_functions};
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
#[cfg(feature = "native-backend")]
use molt_ir::linkage_abi::NativeFunctionLinkageAbi;

/// A specialized clone has no published object or external declaration. It
/// can use the success-only ABI when every terminal reference calls it and
/// checks for unwind before using the raw result. Generic and frozen provider
/// rows keep their ordinary ABI.
pub(super) fn checked_private_returns<'a>(
    bodies: impl IntoIterator<
        Item = Option<(
            &'a crate::tir::TirFunction,
            &'a crate::representation_plan::ProducerCarrierPlan,
        )>,
    >,
    private_entries: impl IntoIterator<Item = molt_ir::FunctionId>,
) -> BTreeMap<molt_ir::FunctionId, crate::tir::types::TirType> {
    use crate::representation_plan::CheckedResultRefusal;

    let report = super::compile_driver::backend_timing_enabled();
    let mut private_entries = private_entries.into_iter().peekable();
    if private_entries.peek().is_none() {
        if report {
            eprintln!("MOLT_BACKEND_TIMING: checked raw returns 0 (no private entries)");
        }
        return BTreeMap::new();
    }
    let bodies: Vec<_> = bodies.into_iter().collect();
    let mut candidates = BTreeMap::new();
    let mut considered = 0;
    for id in private_entries {
        considered += 1;
        if let Some((body, carriers)) = bodies[id.index()]
            && body.return_abi.returns_value()
            && let Some(ty) = carriers.checked_return_type(body)
        {
            candidates.insert(id, (ty, false));
        }
    }
    let not_scalar = considered - candidates.len();
    let (mut non_direct, mut carrier_mismatch, mut unobserved) = (0, 0, 0);
    for (body, carriers) in bodies.into_iter().flatten() {
        if candidates.is_empty() {
            break;
        }
        for op in body.blocks.values().flat_map(|block| &block.ops) {
            let Some(callee) = op.callee else { continue };
            let Some((ty, used)) = candidates.get_mut(&callee) else {
                continue;
            };
            match carriers.checked_direct_result(op, ty) {
                Ok(_) => {
                    *used = true;
                    continue;
                }
                Err(CheckedResultRefusal::NonDirectReference) => non_direct += 1,
                Err(CheckedResultRefusal::CarrierMismatch) => carrier_mismatch += 1,
                Err(CheckedResultRefusal::UnobservedUnwind) => unobserved += 1,
            }
            candidates.remove(&callee);
        }
    }
    let no_caller = candidates.values().filter(|(_, used)| !used).count();
    let chosen: BTreeMap<_, _> = candidates
        .into_iter()
        .filter_map(|(id, (ty, used))| used.then_some((id, ty)))
        .collect();
    if report {
        eprintln!(
            "MOLT_BACKEND_TIMING: checked raw returns {}/{considered}, rejected: no raw success carrier={not_scalar} non-direct reference={non_direct} carrier mismatch={carrier_mismatch} unchecked use={unobserved} no caller={no_caller}",
            chosen.len()
        );
    }
    chosen
}

/// The compiled application handoff. Its declarations accompany terminal TIR;
/// they have no source operations and must never enter the source IR decoder.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedJitApplication {
    pub(super) declarations: Vec<FunctionIR>,
    pub(super) bodies: Vec<Option<Vec<u8>>>,
    pub(super) registry: ModuleRegistryIR,
    pub(super) linkage: NativeBackendModuleContext,
    pub(super) callables: crate::passes::AppCallableManifest,
}

impl PreparedJitApplication {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        rmp_serde::to_vec_named(self).map_err(|error| error.to_string())
    }

    #[cfg(feature = "jit")]
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = std::io::Cursor::new(bytes);
        let program: Self = rmp_serde::from_read(&mut reader).map_err(|error| error.to_string())?;
        if reader.position() != bytes.len() as u64 {
            return Err("prepared JIT application has trailing bytes".to_string());
        }
        let mut names = BTreeSet::new();
        for function in &program.declarations {
            if !function.ops.is_empty() || !names.insert(&function.name) {
                return Err(format!(
                    "invalid prepared JIT declaration `{}`",
                    function.name
                ));
            }
        }
        molt_ir::ir::validate_module_slots(Some(&program.registry), &program.declarations)
            .map_err(|error| error.to_string())?;
        program
            .linkage
            .validate_function_linkage_abis(&program.declarations)?;
        Ok(program)
    }
}

/// Declaration headers, terminal TIR and its SSA carrier facts share one row order.
/// Retention and externalization transform those rows together.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) struct NativeFinalFunctions {
    ir: SimpleIR,
    rows: Vec<NativeFinalRow>,
}

/// The TIR body may be externalized. Its provider's linkage remains authoritative
/// after that transition and for each independent object consuming the row.
#[cfg(feature = "native-backend")]
struct NativeFinalRow {
    body: Option<NativeFinalFunction>,
    local_linkage: Option<NativeFunctionLinkageAbi>,
}

#[cfg(feature = "native-backend")]
pub(crate) struct NativeFinalFunction {
    pub(crate) tir: crate::tir::TirFunction,
    pub(crate) carriers: crate::representation_plan::ProducerCarrierPlan,
}

#[cfg(feature = "native-backend")]
impl NativeFinalFunctions {
    pub(super) fn admit_checked_clone_returns(
        &mut self,
        clones: &[crate::tir::passes::type_flow::SpecializedClone],
    ) {
        let checked =
            checked_private_returns(self.body_carriers(), clones.iter().map(|clone| clone.id));
        for (id, ty) in checked {
            let abi = self.rows[id.index()]
                .local_linkage
                .as_mut()
                .expect("checked clone linkage");
            abi.set_checked_raw_return(ty);
        }
    }

    fn body_carriers(
        &self,
    ) -> impl Iterator<
        Item = Option<(
            &crate::tir::TirFunction,
            &crate::representation_plan::ProducerCarrierPlan,
        )>,
    > {
        self.rows
            .iter()
            .map(|row| row.body.as_ref().map(|body| (&body.tir, &body.carriers)))
    }

    #[cfg(feature = "jit")]
    pub(super) fn validate_checked_returns(
        &self,
        linkage: &NativeBackendModuleContext,
    ) -> Result<(), String> {
        let expected: BTreeMap<_, _> = self
            .ir
            .functions
            .iter()
            .enumerate()
            .filter_map(|(index, function)| {
                let abi = linkage.function_linkage_abi(&function.name)?;
                abi.checked_raw_return().then(|| {
                    (
                        molt_ir::FunctionId::from_index(index),
                        abi.return_type
                            .as_ref()
                            .expect("validated success carrier")
                            .clone(),
                    )
                })
            })
            .collect();
        let checked = checked_private_returns(self.body_carriers(), expected.keys().copied());
        if checked != expected {
            return Err(
                "prepared JIT checked return differs from terminal call or carrier evidence".into(),
            );
        }
        Ok(())
    }

    /// Direct codegen fixtures deliberately author their final CFG, lifetime
    /// operations and backend metadata. Analyze that exact source without
    /// replacing it with a TIR projection or materializing executable polls.
    /// This fresh-input aperture is unavailable to production compilation.
    #[cfg(test)]
    pub(in crate::native_backend::simple_backend) fn from_codegen_fixtures(
        mut ir: SimpleIR,
    ) -> Self {
        // An authored fixture is a fragment: its callees resolve where it
        // defines them, and a symbol another object defines keeps its
        // spelling for the backend's module context (HF-847 slice 1).
        molt_ir::function_table::resolve_function_references_in(
            &mut ir.functions,
            molt_ir::function_table::OutsideSymbols::Keep,
        )
        .unwrap_or_else(|error| panic!("codegen fixture decode failed: {error}"));
        let signatures = molt_ir::linkage_abi::document_signatures(&ir.functions);
        let typed = ir
            .functions
            .iter_mut()
            .enumerate()
            .map(|(index, function)| {
                let mut prepared = std::borrow::Cow::Borrowed(&*function);
                super::program_pipeline::preprocess_backend_tir_input(&mut prepared);
                if let std::borrow::Cow::Owned(prepared) = prepared {
                    *function = prepared;
                }
                let mut body = crate::tir::lower_from_simple::lower_to_tir(function);
                crate::tir::passes::fast_path_expansion::lower_static_call_arguments(
                    &mut body,
                    molt_ir::FunctionId::from_index(index),
                    &signatures,
                );
                crate::tir::type_refine::refine_types(&mut body);
                (function.is_extern, body)
            })
            .collect();
        Self::from_final_tir(ir, typed)
    }

    pub(in crate::native_backend::simple_backend) fn from_final_tir(
        mut ir: SimpleIR,
        typed: Vec<(bool, crate::tir::TirFunction)>,
    ) -> Self {
        assert_eq!(ir.functions.len(), typed.len(), "final document row count");
        let rows = ir
            .functions
            .iter_mut()
            .zip(typed)
            .map(|(source, (external, body))| {
                assert_eq!(
                    source.name, body.name,
                    "terminal body is in the wrong document row"
                );
                assert_eq!(source.is_extern, external, "terminal extern mismatch");
                assert_eq!(
                    source.return_abi, body.return_abi,
                    "terminal return ABI mismatch"
                );
                assert_eq!(
                    source.params, body.param_names,
                    "terminal parameter identity mismatch"
                );
                if external {
                    return NativeFinalRow {
                        body: None,
                        local_linkage: None,
                    };
                }
                // Source references still address the pre-optimization table.
                // All source metadata was captured before terminal inlining.
                // Retire these ops before pruning or renumbering terminal rows.
                drop(std::mem::take(&mut source.ops));
                let carriers = crate::representation_plan::ProducerCarrierPlan::build(&body);
                let local_linkage = {
                    let signature = source
                        .function_signature()
                        .expect("admitted source signature");
                    carriers.linkage_abi(&body, signature, source.parameter_custody.clone())
                };
                NativeFinalRow {
                    body: Some(NativeFinalFunction {
                        tir: body,
                        carriers,
                    }),
                    local_linkage: Some(local_linkage),
                }
            })
            .collect();
        Self { ir, rows }
    }

    fn remap_bodies(&mut self, map: &[Option<molt_ir::FunctionId>]) {
        for body in self.rows.iter_mut().filter_map(|row| row.body.as_mut()) {
            for op in body
                .tir
                .blocks
                .values_mut()
                .flat_map(|block| &mut block.ops)
            {
                molt_ir::function_table::remap_callee(&mut op.callee, map, &body.tir.name);
            }
        }
    }

    pub(super) fn validate_extern_calls(&self) -> Result<(), String> {
        use molt_ir::tir::call_targets::final_call_arguments;
        use molt_ir::tir::ops::AttrValue;
        for body in self.bodies() {
            for op in body.blocks.values().flat_map(|block| &block.ops) {
                let Some((callee, arguments)) = final_call_arguments(op) else {
                    continue;
                };
                let declaration = &self.ir.functions[callee.index()];
                if !declaration.is_extern {
                    continue;
                }
                declaration.validate_extern_call(
                    &body.name,
                    arguments,
                    matches!(
                        op.attrs.get("passes_execution_context"),
                        Some(AttrValue::Bool(true))
                    ),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn bodies(&self) -> impl Iterator<Item = &crate::tir::TirFunction> {
        self.rows
            .iter()
            .filter_map(|row| row.body.as_ref().map(|body| &body.tir))
    }

    pub(in crate::native_backend::simple_backend) fn ir(&self) -> &SimpleIR {
        &self.ir
    }

    pub(in crate::native_backend::simple_backend) fn retain_reachable(
        &mut self,
        roots: &BTreeSet<String>,
    ) {
        let reachable =
            molt_tir::passes::reachable_with_callees(&self.ir.functions, roots, |id, visit| {
                if let Some(body) = self.rows[id.index()].body.as_ref() {
                    for callee in body
                        .tir
                        .blocks
                        .values()
                        .flat_map(|block| &block.ops)
                        .filter_map(|op| op.callee)
                    {
                        visit(callee);
                    }
                }
            });
        let original_count = self.ir.functions.len();
        let map = molt_ir::function_table::retain_rows_with(
            &mut self.ir.functions,
            &mut self.rows,
            &reachable,
        );
        self.remap_bodies(&map);
        let eliminated = original_count - self.ir.functions.len();
        if eliminated > 0 && backend_env::var(BackendEnv::MOLT_DEBUG_DEAD_FUNC_ELIM).is_ok() {
            eprintln!(
                "dead-func-elim: removed {eliminated} of {original_count} functions ({} retained)",
                self.ir.functions.len()
            );
        }
    }

    pub(in crate::native_backend::simple_backend) fn externalize_partition(
        &mut self,
        needs_reachability: bool,
        roots: &BTreeSet<String>,
    ) {
        if let Some(externals) = shared_stdlib_external_symbol_plan(&self.ir) {
            if needs_reachability {
                self.retain_reachable(roots);
            }
            let mut local = Vec::new();
            let mut external = Vec::new();
            for (row, (mut function, mut facts)) in std::mem::take(&mut self.ir.functions)
                .into_iter()
                .zip(std::mem::take(&mut self.rows))
                .enumerate()
            {
                if externals.contains(&function.name) {
                    crate::externalize_function_with_signature(&mut function);
                    facts.body = None;
                    external.push((row, function, facts));
                } else {
                    local.push((row, function, facts));
                }
            }
            local.extend(external);
            // Locals first, then externs: the rows move, so every callee is
            // rewritten to its row's new id (HF-847 slice 1, I4).
            let mut map = vec![None; local.len()];
            for (position, (row, _, _)) in local.iter().enumerate() {
                map[*row] = Some(molt_ir::FunctionId::from_index(position));
            }
            let (functions, facts): (Vec<_>, Vec<_>) = local
                .into_iter()
                .map(|(_, function, facts)| (function, facts))
                .unzip();
            self.ir.functions = functions;
            self.rows = facts;
            molt_ir::function_table::remap_function_ids(&mut self.ir.functions, &map);
            self.remap_bodies(&map);
        }
    }

    /// Shared bodies participated in type flow, but their canonical entries
    /// are provided by the image. All newly specialized rows stay local.
    pub(super) fn bind_shared_bodies(&mut self, shared: &BTreeSet<String>) {
        for (function, row) in self.ir.functions.iter_mut().zip(&mut self.rows) {
            if !function.is_extern && shared.contains(&function.name) {
                crate::externalize_function_with_signature(function);
                row.body = None;
            }
        }
    }

    pub(super) fn into_jit_artifact(
        self,
        registry: ModuleRegistryIR,
        mut linkage: NativeBackendModuleContext,
        callables: crate::passes::AppCallableManifest,
    ) -> Result<PreparedJitApplication, String> {
        for (function, row) in self.ir.functions.iter().zip(&self.rows) {
            if let Some(abi) = row
                .local_linkage
                .as_ref()
                .filter(|abi| abi.checked_raw_return())
            {
                linkage
                    .function_linkage_abis
                    .insert(function.name.clone(), abi.clone());
            }
        }
        let table = molt_ir::FunctionTable::of(&self.ir.functions);
        let bodies = self
            .rows
            .into_iter()
            .map(|row| {
                row.body
                    .map(|body| molt_ir::tir::serialize::serialize_tir_function(&body.tir, &table))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PreparedJitApplication {
            declarations: self.ir.functions,
            bodies,
            registry,
            linkage,
            callables,
        })
    }

    pub(super) fn from_terminal_wire(
        ir: SimpleIR,
        wires: Vec<Option<Vec<u8>>>,
    ) -> Result<Self, String> {
        let mut symbols = BTreeSet::new();
        if ir
            .functions
            .iter()
            .any(|header| !header.ops.is_empty() || !symbols.insert(&header.name))
        {
            return Err("terminal declarations have source operations or duplicate symbols".into());
        }
        if ir.functions.len() != wires.len() {
            return Err(
                "native terminal body row count differs from the declaration table".to_string(),
            );
        }
        let table = molt_ir::FunctionTable::of(&ir.functions);
        let by_symbol: BTreeMap<_, _> = table.ids().map(|id| (table.symbol(id), id)).collect();
        let mut bodies = Vec::with_capacity(wires.len());
        for (declaration, wire) in ir.functions.iter().zip(wires) {
            let body = match (declaration.is_extern, wire) {
                (true, None) => crate::tir::lower_from_simple::lower_to_tir(declaration),
                (false, Some(bytes)) => {
                    molt_ir::tir::serialize::deserialize_tir_function(&bytes, &|name| {
                        by_symbol.get(name).copied()
                    })
                    .filter(|body| {
                        body.name == declaration.name
                            && body.return_abi == declaration.return_abi
                            && body.param_names == declaration.params
                    })
                    .ok_or_else(|| format!("invalid terminal body for `{}`", declaration.name))?
                }
                _ => {
                    return Err(format!(
                        "native terminal body ownership differs for `{}`",
                        declaration.name
                    ));
                }
            };
            bodies.push((declaration.is_extern, body));
        }
        Ok(Self::from_final_tir(ir, bodies))
    }

    /// Resolve a physical declaration in document order. An independently
    /// compiled provider's frozen signature takes precedence over local facts.
    pub(super) fn linkage<'a>(
        &'a self,
        id: molt_ir::FunctionId,
        context: Option<&'a NativeBackendModuleContext>,
    ) -> &'a NativeFunctionLinkageAbi {
        let name = &self.ir.functions[id.index()].name;
        context
            .and_then(|ctx| ctx.function_linkage_abi(name))
            .or(self.rows[id.index()].local_linkage.as_ref())
            .unwrap_or_else(|| panic!("compiled declaration '{name}' has no provider linkage ABI"))
    }

    pub(in crate::native_backend::simple_backend) fn into_local_functions(
        self,
    ) -> impl Iterator<Item = (molt_ir::FunctionId, NativeFinalFunction)> {
        self.rows
            .into_iter()
            .enumerate()
            .filter_map(|(index, row)| {
                row.body
                    .map(|body| (molt_ir::FunctionId::from_index(index), body))
            })
    }
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct NativeBackendModuleContext {
    /// Captured whole-program linkage ABI, dependency-projected into each job.
    /// Provider definitions and consumer declarations must read the same row;
    /// neither may reconstruct a machine signature from its local body subset.
    pub(in crate::native_backend::simple_backend) function_linkage_abis:
        BTreeMap<String, NativeFunctionLinkageAbi>,
}

#[cfg(feature = "native-backend")]
impl NativeBackendModuleContext {
    /// Exact names whose context rows this object can consume. The generated
    /// defined-function edges include indirect/name-taking and task references;
    /// callable metadata contributes runtime callable constructors as well.
    /// No sibling symbol is inferred from a spelling convention.
    pub fn object_dependencies(functions: &[FunctionIR]) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = functions.iter().map(|f| f.name.clone()).collect();
        let table = molt_ir::FunctionTable::of(functions);
        for function in functions {
            molt_tir::passes::visit_function_reference_targets(function, &table, |name| {
                names.insert(name.to_owned());
            });
        }
        names.extend(
            molt_tir::trampolines::CallableMetadata::from_functions(functions)
                .escaped_callable_targets,
        );
        names
    }

    /// Produce the context actually transported to, hashed for, and consumed by
    /// one native object job. Unrelated whole-program rows never reach codegen.
    /// Every field is explicitly destructured so adding context state requires
    /// deciding its projection here rather than silently omitting a new input.
    pub fn project_object_dependencies(&self, names: &BTreeSet<String>) -> Self {
        let Self {
            function_linkage_abis,
        } = self;
        Self {
            // Stable per-source partitions can number in the thousands.
            // Drive the projection by each partition's dependencies instead
            // of scanning the whole program once per object.
            function_linkage_abis: names
                .iter()
                .filter_map(|name| Some((name.clone(), function_linkage_abis.get(name)?.clone())))
                .collect(),
        }
    }

    pub fn function_linkage_abi(&self, name: &str) -> Option<&NativeFunctionLinkageAbi> {
        self.function_linkage_abis.get(name)
    }

    /// Prove that every declaration consumed from this context has exactly the
    /// target-neutral signature and machine shape frozen by its body owner.
    /// Local bodies with a row are checked too; local synthetic functions may
    /// legitimately have no whole-program row and derive one in the compiler.
    pub fn validate_function_linkage_abis(&self, functions: &[FunctionIR]) -> Result<(), String> {
        for function in functions {
            let signature = function.function_signature()?;
            let Some(linkage_abi) = self.function_linkage_abis.get(&function.name) else {
                if function.is_extern {
                    return Err(format!(
                        "extern function `{}` has no frozen native linkage ABI",
                        function.name
                    ));
                }
                continue;
            };
            if linkage_abi.param_types.len() != linkage_abi.source_signature.arity {
                return Err(format!(
                    "native linkage ABI `{}` has {} carriers but source arity {}",
                    function.name,
                    linkage_abi.param_types.len(),
                    linkage_abi.source_signature.arity
                ));
            }
            if linkage_abi.return_type.is_some() != linkage_abi.source_signature.returns_value {
                return Err(format!(
                    "native linkage ABI `{}` return carrier disagrees with its frozen source signature",
                    function.name
                ));
            }
            if linkage_abi.checked_raw_return()
                && (function.is_extern
                    || !linkage_abi
                        .return_type
                        .as_ref()
                        .is_some_and(|ty| ty.is_numeric()))
            {
                return Err(format!(
                    "native linkage ABI `{}` has an invalid private success carrier",
                    function.name
                ));
            }
            if signature != linkage_abi.source_signature {
                return Err(format!(
                    "function `{}` signature {:?} disagrees with frozen native linkage signature {:?}",
                    function.name, signature, linkage_abi.source_signature
                ));
            }
            if function.parameter_custody != linkage_abi.parameter_custody {
                return Err(format!(
                    "function `{}` parameter custody {:?} disagrees with frozen native linkage custody {:?}",
                    function.name, function.parameter_custody, linkage_abi.parameter_custody
                ));
            }
        }
        Ok(())
    }

    pub(in crate::native_backend::simple_backend) fn from_functions(
        functions: &mut Vec<FunctionIR>,
    ) -> Self {
        Self::from_functions_with_carrier_cache(
            functions,
            crate::tir::cache::CompilationCache::open_backend_namespace().as_mut(),
        )
    }

    /// `carrier_cache` serves unchanged bodies' linkage carriers by content
    /// (`linkage_carriers`); `None` computes every row.
    pub(in crate::native_backend::simple_backend) fn from_functions_with_carrier_cache(
        functions: &mut Vec<FunctionIR>,
        carrier_cache: Option<&mut crate::tir::cache::CompilationCache>,
    ) -> Self {
        let mut ir = SimpleIR {
            functions: std::mem::take(functions),
            profile: None,
        };
        split_megafunctions(&mut ir);
        *functions = ir.functions;
        let timing = crate::env_setting(BackendEnv::MOLT_BACKEND_TIMING)
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let started = timing.then(std::time::Instant::now);
        let mut unique_names = BTreeSet::new();
        for function in functions.iter() {
            assert!(
                unique_names.insert(function.name.as_str()),
                "duplicate FunctionIR name `{}` cannot own a deterministic native linkage ABI",
                function.name
            );
        }
        // Freeze the source machine ABI before partitioning. These bodies have
        // not completed worker lifetime finalization and cannot certify leaves.
        // Final callback effects are derived only by the codegen worker.
        let defined: Vec<&FunctionIR> = functions
            .iter()
            .filter(|function| !function.is_extern)
            .collect();
        let table = molt_ir::FunctionTable::of(functions);
        let carriers = super::linkage_carriers::linkage_carriers(&defined, &table, carrier_cache);
        let reused_carriers = carriers.reused;
        let function_linkage_abis = defined
            .iter()
            .zip(carriers.rows)
            .map(|(function, carriers)| {
                let source_signature = function
                    .function_signature()
                    .unwrap_or_else(|error| panic!("invalid linkage ABI source: {error}"));
                let return_type = source_signature
                    .returns_value
                    .then_some(carriers.return_type);
                (
                    function.name.clone(),
                    NativeFunctionLinkageAbi {
                        source_signature,
                        parameter_custody: function.parameter_custody.clone(),
                        param_types: carriers.param_types,
                        return_type,
                        return_availability: Default::default(),
                    },
                )
            })
            .collect();
        let context = Self {
            function_linkage_abis,
        };
        if let Some(started) = started {
            let carrier_count = context
                .function_linkage_abis
                .values()
                .map(|abi| abi.param_types.len() + usize::from(abi.return_type.is_some()))
                .sum::<usize>();
            eprintln!(
                "MOLT_BACKEND_TIMING: froze {} native linkage ABI rows ({} reused) / {} carriers from {} functions in {:.2?}",
                context.function_linkage_abis.len(),
                reused_carriers,
                carrier_count,
                functions.len(),
                started.elapsed(),
            );
        }
        context
    }
}

pub(crate) fn parse_truthy_env(raw: &str) -> bool {
    let norm = raw.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_modules: Option<&BTreeSet<String>>,
    module_registry_roots: &BTreeSet<String>,
) -> (Vec<FunctionIR>, Vec<FunctionIR>) {
    ir.resolve_function_references()
        .unwrap_or_else(|error| panic!("stdlib partition decode failed: {error}"));
    eliminate_dead_functions_with_roots(ir, module_registry_roots);
    partition_user_owned_functions(ir, |function| {
        is_user_owned_function(function, entry_module, stdlib_modules)
    })
}

/// The names of the functions `externalize_shared_stdlib_partition` *will*
/// externalize into `stdlib_shared.o` (their definitions live in the shared
/// object; this app object must reference them as undefined externals).
///
/// Computed up front  BEFORE the module-phase inliner runs  so the inliner can
/// treat these as **external-linkage** functions and refuse to inline them: a
/// caller that does not own a callee's canonical definition (it lives in another
/// object) must not splice in a private copy of its body. Doing so would (a)
/// drop the external reference the linker resolves against `stdlib_shared.o`,
/// breaking the partition contract, and (b)  once `externalize_*` later clears
/// the in-app body  leave the app running a stale private fork of a function
/// whose real definition is the shared one.
///
/// Returns an empty set when the partition is inactive (no `MOLT_STDLIB_OBJ`, no
/// `MOLT_ENTRY_MODULE`, or the shared object file is absent), so the inliner is
/// unconstrained in the common (non-partitioned) build. This mirrors the exact
/// activation guards and `is_user_owned_function` predicate
/// `externalize_shared_stdlib_partition` uses, so the do-not-inline set and the
/// later externalized set are computed from one source of truth.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn shared_stdlib_external_symbols(
    ir: &SimpleIR,
) -> BTreeSet<String> {
    shared_stdlib_external_symbol_plan(ir).unwrap_or_default()
}

#[cfg(feature = "native-backend")]
fn shared_stdlib_external_symbol_plan(ir: &SimpleIR) -> Option<BTreeSet<String>> {
    let stdlib_obj_path = backend_env::var(BackendEnv::MOLT_STDLIB_OBJ).ok()?;
    let Ok(entry_module) = backend_env::var(BackendEnv::MOLT_ENTRY_MODULE) else {
        return None;
    };
    if !std::path::Path::new(&stdlib_obj_path).exists() {
        return None;
    }
    let explicit_stdlib_modules = crate::stdlib_modules::stdlib_modules_from_env_or_panic();
    Some(
        ir.functions
            .iter()
            .filter(|f| !is_user_owned_function(f, &entry_module, explicit_stdlib_modules.as_ref()))
            .map(|f| f.name.clone())
            .collect(),
    )
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn externalize_shared_stdlib_partition(
    ir: &mut SimpleIR,
    module_registry_roots: &BTreeSet<String>,
) {
    let Some(stdlib_obj_path) = backend_env::var(BackendEnv::MOLT_STDLIB_OBJ).ok() else {
        return;
    };
    let Ok(entry_module) = backend_env::var(BackendEnv::MOLT_ENTRY_MODULE) else {
        return;
    };
    let stdlib_path = std::path::Path::new(&stdlib_obj_path);
    if !stdlib_path.exists() {
        return;
    }
    let explicit_stdlib_modules = crate::stdlib_modules::stdlib_modules_from_env_or_panic();
    let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
        ir,
        &entry_module,
        explicit_stdlib_modules.as_ref(),
        module_registry_roots,
    );
    let mut retained = std::mem::take(&mut user_remaining);
    // The user document already holds an extern row for each stdlib function
    // it references (HF-847 slice 1); the rest are appended as rows.
    let present: BTreeSet<String> = retained.iter().map(|func| func.name.clone()).collect();
    for mut func in std::mem::take(&mut stdlib_funcs) {
        if func.is_extern || present.contains(&func.name) {
            continue;
        }
        crate::externalize_function_with_signature(&mut func);
        retained.push(func);
    }
    ir.functions = retained;
}
