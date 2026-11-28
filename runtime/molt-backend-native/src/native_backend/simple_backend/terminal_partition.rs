//! Target-bound terminal function rows for independently emitted application objects.
use super::backend_selection::NativeCodegenBackend;
use super::compile_driver::{NativeCodegenProgram, backend_timing_enabled};
use super::*;
use molt_ir::FunctionId;
use molt_ir::content_digest::{CompilationCacheKey, bytes_to_lower_hex};
use molt_ir::linkage_abi::{NativeFunctionLinkageAbi, ReturnAvailability};
use molt_ir::tir::ops::OpCode;

/// The same commitment names the provider's hidden definition and every import.
/// Its fields are in the independently content-addressed object inputs.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckedProvider {
    origin: String,
    body: [u8; 32],
    entry: [u8; 32],
}

impl CheckedProvider {
    fn body_digest(wire: &[u8]) -> [u8; 32] {
        let mut key = CompilationCacheKey::new(b"native-checked-body-v1");
        key.field(b"tir", wire);
        key.finish_bytes()
    }

    fn entry_digest(
        target: &[u8],
        origin: &str,
        clone: &str,
        body: &[u8; 32],
        abi: &NativeFunctionLinkageAbi,
    ) -> [u8; 32] {
        let mut key = CompilationCacheKey::new(b"native-private-checked-provider-v1");
        key.field(b"target", target);
        key.field(b"origin", origin.as_bytes());
        key.field(b"clone", clone.as_bytes());
        key.field(b"body", body);
        key.field(
            b"abi",
            &serde_json::to_vec(abi).expect("serializable linkage"),
        );
        key.finish_bytes()
    }

    fn symbol(&self) -> String {
        format!("__molt_checked_{}", bytes_to_lower_hex(&self.entry))
    }
}

pub struct TerminalObjectRow {
    id: FunctionId,
    pub declaration: FunctionIR,
    pub operations: usize,
}
struct TerminalBody {
    references: BTreeSet<FunctionId>,
    literal_cell_providers: Vec<FunctionId>,
    wire: Vec<u8>,
}

pub struct NativeTerminalArchive {
    target: Vec<u8>,
    externs: Vec<FunctionIR>,
    checked: BTreeMap<FunctionId, CheckedProvider>,
    bodies: Vec<Option<TerminalBody>>,
    pub definitions: Vec<TerminalObjectRow>,
    pub linkage: NativeBackendModuleContext,
    pub callables: crate::passes::AppCallableManifest,
}

/// Local headers and bodies precede the imported headers. The wire's typed
/// callee relocations bind against this object's declaration table on decode.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTerminalObject {
    target: Vec<u8>,
    pub declarations: Vec<FunctionIR>,
    checked: Vec<(FunctionId, CheckedProvider)>,
    bodies: Vec<Option<serde_bytes::ByteBuf>>,
}

impl NativeTerminalArchive {
    /// A literal-readiness guard observes writable storage private to the
    /// provider's object. Build just those dependency components before the
    /// ordinary source and size partitioner processes independent definitions.
    pub fn take_literal_cell_partitions(&mut self) -> Vec<Vec<TerminalObjectRow>> {
        let mut edges = vec![Vec::new(); self.bodies.len()];
        for row in &self.definitions {
            let body = self.bodies[row.id.index()]
                .as_ref()
                .expect("unassigned terminal body");
            for &provider in &body.literal_cell_providers {
                if provider != row.id {
                    edges[row.id.index()].push(provider);
                    edges[provider.index()].push(row.id);
                }
            }
        }
        let mut owners = vec![None; edges.len()];
        let mut partitions = Vec::new();
        let mut pending = Vec::new();
        for row in &self.definitions {
            let id = row.id.index();
            if edges[id].is_empty() || owners[id].is_some() {
                continue;
            }
            let partition = partitions.len();
            partitions.push(Vec::new());
            owners[id] = Some(partition);
            pending.push(row.id);
            while let Some(member) = pending.pop() {
                for &neighbor in &edges[member.index()] {
                    if owners[neighbor.index()].is_none() {
                        owners[neighbor.index()] = Some(partition);
                        pending.push(neighbor);
                    }
                }
            }
        }
        self.definitions = std::mem::take(&mut self.definitions)
            .into_iter()
            .filter_map(|row| match owners[row.id.index()] {
                Some(partition) => {
                    partitions[partition].push(row);
                    None
                }
                None => Some(row),
            })
            .collect();
        partitions
    }

    pub fn project(
        &mut self,
        definitions: Vec<TerminalObjectRow>,
    ) -> Result<NativeTerminalObject, String> {
        let locals: BTreeSet<_> = definitions.iter().map(|row| row.id).collect();
        if locals.len() != definitions.len() {
            return Err("duplicate terminal object definition".into());
        }
        let mut imports = BTreeSet::new();
        for id in &locals {
            let body = self
                .bodies
                .get(id.index())
                .and_then(Option::as_ref)
                .ok_or("terminal definition is absent or already assigned to an object")?;
            if let Some(provider) = body
                .literal_cell_providers
                .iter()
                .find(|provider| !locals.contains(provider))
            {
                return Err(format!(
                    "terminal body `{}` must share literal storage with `{}`",
                    self.externs[id.index()].name,
                    self.externs[provider.index()].name,
                ));
            }
            imports.extend(body.references.difference(&locals).copied());
        }
        let mut declarations = Vec::with_capacity(locals.len() + imports.len());
        let mut bodies = Vec::with_capacity(locals.len() + imports.len());
        let mut checked = Vec::new();
        for row in &definitions {
            let mut header = self.externs[row.id.index()].clone();
            header.is_extern = false;
            if let Some(provider) = self.checked.get(&row.id) {
                checked.push((FunctionId::from_index(declarations.len()), provider.clone()));
            }
            declarations.push(header);
        }
        for id in imports {
            let header = &self.externs[id.index()];
            let abi = self
                .linkage
                .function_linkage_abi(&header.name)
                .ok_or_else(|| format!("terminal import `{}` has no provider ABI", header.name))?;
            if let Some(provider) = self.checked.get(&id) {
                checked.push((FunctionId::from_index(declarations.len()), provider.clone()));
            } else if abi.checked_raw_return() {
                return Err(format!(
                    "terminal import `{}` requires a program-private checked provider",
                    header.name
                ));
            }
            declarations.push(header.clone());
        }
        // Do not consume any provider until every declared dependency validates.
        for row in definitions {
            let body = self.bodies[row.id.index()]
                .take()
                .expect("validated terminal body");
            bodies.push(Some(body.wire.into()));
        }
        bodies.resize_with(declarations.len(), || None);
        Ok(NativeTerminalObject {
            target: self.target.clone(),
            checked,
            declarations,
            bodies,
        })
    }
}

impl SimpleBackend {
    fn terminal_object_target(&self) -> Vec<u8> {
        serde_json::to_vec(&self.codegen_identity()).expect("serializable target identity")
    }

    pub fn prepare_terminal_archive(
        &mut self,
        mut ir: SimpleIR,
        linkage: NativeBackendModuleContext,
        source_facts: Option<Vec<NativeSourceFunction>>,
    ) -> Result<NativeTerminalArchive, String> {
        ir.resolve_function_references()?;
        self.admit_program(&ir).map_err(|error| error.to_string())?;
        self.set_module_context(linkage);
        let backend = self.selected_object_backend();
        let program = self.prepare_native_codegen(
            &mut ir,
            source_facts,
            backend,
            true,
            backend_timing_enabled(),
            &std::time::Instant::now(),
        );
        let mut linkage = self
            .module_context
            .take()
            .expect("archive provider context");
        let functions = program.final_functions;
        let table = molt_ir::FunctionTable::of(&functions.ir().functions);
        let mut externs = Vec::with_capacity(table.len());
        let mut origins = BTreeMap::new();
        let target = self.terminal_object_target();
        for id in table.ids() {
            let declaration = &functions.ir().functions[id.index()];
            externs.push(declaration.extern_declaration()?);
            if let Some(origin) = functions.private_origin(id) {
                if linkage.function_linkage_abi(&declaration.name).is_some()
                    || program
                        .app_callable_manifest
                        .compiled_bodies
                        .contains(&declaration.name)
                {
                    return Err(format!(
                        "checked clone conflicts with published entry {}",
                        declaration.name
                    ));
                }
                origins.insert(id, origin.to_owned());
            }
            if linkage.function_linkage_abi(&declaration.name).is_none() {
                let abi = functions.linkage(id, None);
                linkage
                    .function_linkage_abis
                    .insert(declaration.name.clone(), abi.clone());
            }
        }
        if backend_timing_enabled() {
            eprintln!(
                "MOLT_BACKEND_TIMING: archive parent analyzed carriers for {} of {} defined bodies",
                functions.analyzed_carrier_count(),
                functions.bodies().count(),
            );
        }
        let mut definitions = Vec::new();
        let mut checked = BTreeMap::new();
        let mut bodies: Vec<_> = (0..table.len()).map(|_| None).collect();
        for (id, body) in functions.into_local_functions() {
            let wire = molt_ir::tir::serialize::serialize_tir_function(&body.tir, &table)?;
            if let Some(origin) = origins.remove(&id) {
                let clone = table.symbol(id);
                let abi = linkage
                    .function_linkage_abi(clone)
                    .expect("private clone linkage");
                let body_digest = CheckedProvider::body_digest(&wire);
                let entry =
                    CheckedProvider::entry_digest(&target, &origin, clone, &body_digest, abi);
                let provider = CheckedProvider {
                    origin,
                    body: body_digest,
                    entry,
                };
                if linkage.function_linkage_abi(&provider.symbol()).is_some() {
                    return Err("private physical symbol conflicts with a source provider".into());
                }
                checked.insert(id, provider);
            }
            let mut declaration = externs[id.index()].clone();
            declaration.is_extern = false;
            let mut references = BTreeSet::new();
            let mut literal_cell_providers = BTreeSet::new();
            for op in body.tir.blocks.values().flat_map(|block| &block.ops) {
                if let Some(callee) = op.callee {
                    references.insert(callee);
                    if op.opcode == OpCode::StringLiteralCellReady && table.defines(callee) {
                        literal_cell_providers.insert(callee);
                    }
                }
            }
            bodies[id.index()] = Some(TerminalBody {
                references,
                literal_cell_providers: literal_cell_providers.into_iter().collect(),
                wire,
            });
            definitions.push(TerminalObjectRow {
                id,
                declaration,
                operations: body.tir.operation_count(),
            });
        }
        Ok(NativeTerminalArchive {
            target,
            checked,
            externs,
            bodies,
            definitions,
            linkage,
            callables: program.app_callable_manifest,
        })
    }

    pub fn compile_terminal_object(
        mut self,
        object: NativeTerminalObject,
    ) -> Result<CompileOutput, String> {
        if self.terminal_object_target() != object.target {
            return Err("terminal object target differs from the compiling worker".into());
        }
        let backend: NativeCodegenBackend = self.selected_object_backend();
        let context = self
            .module_context
            .as_mut()
            .ok_or("terminal object has no provider linkage")?;
        let mut bindings = BTreeMap::new();
        let mut claims = BTreeSet::new();
        for (id, provider) in object.checked {
            let header = object
                .declarations
                .get(id.index())
                .ok_or("private provider row is absent")?;
            let abi = context
                .function_linkage_abi(&header.name)
                .ok_or("missing private provider ABI")?;
            let wire = object
                .bodies
                .get(id.index())
                .ok_or("private provider body row is absent")?;
            if abi.return_availability != ReturnAvailability::SuccessOnly
                || provider.origin == header.name
                || self
                    .app_callable_manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.compiled_bodies.contains(&header.name))
                || wire
                    .as_ref()
                    .is_some_and(|wire| CheckedProvider::body_digest(wire) != provider.body)
                || CheckedProvider::entry_digest(
                    &object.target,
                    &provider.origin,
                    &header.name,
                    &provider.body,
                    abi,
                ) != provider.entry
            {
                return Err(format!(
                    "invalid private provider commitment for {}",
                    header.name
                ));
            }
            let symbol = provider.symbol();
            if context.function_linkage_abi(&symbol).is_some()
                || !claims.insert(header.name.clone())
            {
                return Err("private provider collides with another declaration".into());
            }
            bindings.insert(id, (symbol, abi.clone()));
        }
        if context
            .function_linkage_abis
            .iter()
            .any(|(name, abi)| abi.checked_raw_return() && !claims.contains(name))
        {
            return Err("terminal object has an uncertified checked entry".into());
        }
        let mut final_functions = NativeFinalFunctions::from_terminal_wire(
            SimpleIR {
                functions: object.declarations,
                profile: None,
            },
            object.bodies,
        )?;
        final_functions.bind_checked_providers(&bindings)?;
        for (_, (symbol, mut abi)) in bindings {
            abi.return_availability = ReturnAvailability::CertifiedProgramEntry;
            context.function_linkage_abis.insert(symbol, abi);
        }
        for name in claims {
            context.function_linkage_abis.remove(&name);
        }
        context.validate_function_linkage_abis(&final_functions.ir().functions)?;
        final_functions.validate_extern_calls()?;
        if let Some(bindings) = &self.code_slots {
            bindings
                .validate_functions(&final_functions.ir().functions)
                .map_err(|error| error.to_string())?;
        }
        if self.emit_app_callable_resolver || self.module_registry.is_some() {
            molt_ir::ir::validate_module_slots(
                self.module_registry.as_ref(),
                &final_functions.ir().functions,
            )
            .map_err(|error| error.to_string())?;
        }
        let program = NativeCodegenProgram {
            emit_resolver_here: self.emit_app_callable_resolver,
            app_callable_manifest: self.app_callable_manifest.take().unwrap_or_default(),
            final_functions,
        };
        Ok(self.emit_prepared_object(
            program,
            backend,
            backend_timing_enabled(),
            std::time::Instant::now(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_cell_affinity_keeps_transitive_providers_with_their_guard() {
        let externs: Vec<_> = ["caller", "wrapper", "leaf", "ordinary"]
            .into_iter()
            .map(|name| FunctionIR {
                name: name.into(),
                is_extern: true,
                ..FunctionIR::default()
            })
            .collect();
        let definitions = externs
            .iter()
            .enumerate()
            .map(|(index, declaration)| TerminalObjectRow {
                id: FunctionId::from_index(index),
                declaration: declaration.clone(),
                operations: 1,
            })
            .collect();
        let [caller, wrapper, leaf] = [0, 1, 2].map(FunctionId::from_index);
        let bodies = [
            (Some(wrapper), Some(wrapper)),
            (Some(leaf), Some(leaf)),
            (None, None),
            (Some(caller), None),
        ]
        .into_iter()
        .map(|(reference, literal)| {
            Some(TerminalBody {
                references: reference.into_iter().collect(),
                literal_cell_providers: literal.into_iter().collect(),
                wire: Vec::new(),
            })
        })
        .collect();
        let mut archive = NativeTerminalArchive {
            target: Vec::new(),
            externs,
            checked: BTreeMap::new(),
            bodies,
            definitions,
            linkage: NativeBackendModuleContext::default(),
            callables: Default::default(),
        };
        let mispartitioned = TerminalObjectRow {
            id: caller,
            declaration: archive.externs[caller.index()].clone(),
            operations: 1,
        };
        assert!(
            archive
                .project(vec![mispartitioned])
                .err()
                .is_some_and(|error| error.contains("must share literal storage"))
        );
        let [group]: [_; 1] = archive
            .take_literal_cell_partitions()
            .try_into()
            .ok()
            .unwrap();
        assert_eq!(
            group.iter().map(|row| row.id).collect::<Vec<_>>(),
            [caller, wrapper, leaf]
        );
        assert!(archive.project(group).is_ok());
        assert_eq!(archive.definitions.len(), 1);
        assert_eq!(archive.definitions[0].declaration.name, "ordinary");
    }
}
