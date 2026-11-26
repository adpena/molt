//! Target-bound terminal function rows for independently emitted application objects.
use super::backend_selection::NativeCodegenBackend;
use super::compile_driver::{NativeCodegenProgram, backend_timing_enabled};
use super::*;
use molt_ir::FunctionId;

pub struct TerminalObjectRow {
    id: FunctionId,
    pub declaration: FunctionIR,
    pub operations: usize,
}
struct TerminalBody {
    references: BTreeSet<FunctionId>,
    wire: Vec<u8>,
}

pub struct NativeTerminalArchive {
    target: Vec<u8>,
    externs: Vec<FunctionIR>,
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
    bodies: Vec<Option<Vec<u8>>>,
}

impl NativeTerminalArchive {
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
            imports.extend(body.references.difference(&locals).copied());
        }
        let mut declarations = Vec::with_capacity(locals.len() + imports.len());
        let mut bodies = Vec::with_capacity(locals.len() + imports.len());
        for row in &definitions {
            let mut header = self.externs[row.id.index()].clone();
            header.is_extern = false;
            declarations.push(header);
        }
        for id in imports {
            let header = &self.externs[id.index()];
            let abi = self
                .linkage
                .function_linkage_abi(&header.name)
                .ok_or_else(|| format!("terminal import `{}` has no provider ABI", header.name))?;
            if abi.checked_raw_return() {
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
            bodies.push(Some(body.wire));
        }
        bodies.resize_with(declarations.len(), || None);
        Ok(NativeTerminalObject {
            target: self.target.clone(),
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
    ) -> Result<NativeTerminalArchive, String> {
        ir.resolve_function_references()?;
        self.admit_program(&ir).map_err(|error| error.to_string())?;
        self.set_module_context(linkage);
        let backend = self.selected_object_backend();
        let program = self.prepare_native_codegen(
            &mut ir,
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
        for id in table.ids() {
            let declaration = &functions.ir().functions[id.index()];
            externs.push(declaration.extern_declaration()?);
            if linkage.function_linkage_abi(&declaration.name).is_none() {
                let abi = functions.linkage(id, None);
                linkage
                    .function_linkage_abis
                    .insert(declaration.name.clone(), abi.clone());
            }
        }
        let mut definitions = Vec::new();
        let mut bodies: Vec<_> = (0..table.len()).map(|_| None).collect();
        for (id, body) in functions.into_local_functions() {
            let wire = molt_ir::tir::serialize::serialize_tir_function(&body.tir, &table)?;
            let mut declaration = externs[id.index()].clone();
            declaration.is_extern = false;
            let references = body
                .tir
                .blocks
                .values()
                .flat_map(|b| &b.ops)
                .filter_map(|op| op.callee)
                .collect();
            bodies[id.index()] = Some(TerminalBody { references, wire });
            definitions.push(TerminalObjectRow {
                id,
                declaration,
                operations: body.tir.operation_count(),
            });
        }
        Ok(NativeTerminalArchive {
            target: self.terminal_object_target(),
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
        let final_functions = NativeFinalFunctions::from_terminal_wire(
            SimpleIR {
                functions: object.declarations,
                profile: None,
            },
            object.bodies,
        )?;
        let context = self
            .module_context
            .as_ref()
            .ok_or("terminal object has no provider linkage")?;
        context.validate_function_linkage_abis(&final_functions.ir().functions)?;
        // The source-archive ABI carries every-exit entries. A success-only
        // claim needs independent private-provider provenance at this boundary.
        if context
            .function_linkage_abis
            .values()
            .any(|abi| abi.checked_raw_return())
        {
            return Err("terminal object has an uncertified checked entry".into());
        }
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
