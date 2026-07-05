use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use molt_backend::{SimpleBackend, SimpleIR};

use super::super::config::{DEFAULT_BACKEND_BATCH_OP_BUDGET, DEFAULT_BACKEND_BATCH_SIZE};
use super::super::io_limits::{write_json_artifact, write_output_path};
use super::{
    NativeApplicationObjectOptions, NativeApplicationObjectResult, NativeBatchJobSpec,
    NativeBatchModuleMetadata, NativeBatchObjectJob, batch_external_function_names,
    deduplicate_functions_by_name, finish_native_batch_temp_dir, merge_relocatable_objects,
    partition_functions_for_batches, release_native_backend_batch_memory_to_os,
    resolved_batch_op_budget_limit, resolved_batch_size_limit,
    run_native_batch_worker_with_failure_artifacts,
};

pub(crate) fn compile_native_application_object_to_path(
    mut ir: SimpleIR,
    output_path: &Path,
    mut options: NativeApplicationObjectOptions<'_>,
) -> io::Result<NativeApplicationObjectResult> {
    if options.stdlib_split_enabled && options.app_callable_manifest.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stdlib-split native application object requires a full-set app callable manifest",
        ));
    }

    // Preserve the one-shot native application-object sequence as the single
    // authority for both direct backend runs and daemon requests.
    molt_backend::inject_runtime_exit(&mut ir);
    if !options.stdlib_split_enabled {
        // Import bedrock: registry init symbols are DFE roots - init bodies
        // are reachable only through the registry blob's MODULE_INIT_TABLE
        // relocations (invariant I5).
        let module_registry_roots: std::collections::BTreeSet<String> = options
            .module_registry
            .as_ref()
            .map(|registry| registry.init_symbols.iter().cloned().collect())
            .unwrap_or_default();
        molt_backend::eliminate_dead_functions_with_roots(&mut ir, &module_registry_roots);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(&mut ir);
    }
    deduplicate_functions_by_name(&mut ir.functions);

    let function_count = ir.functions.len();
    let batch_size = resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE);
    let batch_ops_budget = resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET);
    let total_ops = ir
        .functions
        .iter()
        .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
    if function_count <= batch_size && total_ops <= batch_ops_budget {
        let mut backend = SimpleBackend::new_with_target(options.target_triple);
        if options.stdlib_split_enabled {
            backend.skip_shared_stdlib_partition = true;
        }
        backend.app_callable_manifest = options.app_callable_manifest.take();
        backend.module_registry = options.module_registry.take();
        let obj_output = backend.compile(ir);
        write_output_path(output_path, &obj_output.bytes)?;
        eprintln!(
            "Successfully compiled to {} ({} functions)",
            output_path.display(),
            function_count
        );
        return Ok(NativeApplicationObjectResult {
            function_count,
            batch_count: 1,
        });
    }

    let profile = ir.profile;
    let all_functions: Vec<_> = ir.functions.into_iter().collect();
    let all_func_names: std::collections::BTreeSet<String> =
        all_functions.iter().map(|f| f.name.clone()).collect();
    let module_context = SimpleBackend::build_module_context(&all_functions);
    if options.app_callable_manifest.is_none() {
        options.app_callable_manifest = Some(molt_backend::compute_app_callable_manifest_checked(
            &all_functions,
        ));
    }
    let batches = partition_functions_for_batches(all_functions, batch_size, batch_ops_budget);
    let total_batches = batches.len();
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt_batch_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir)?;
    let module_context_path = tmp_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata { module_context },
    )?;
    let mut batch_specs: Vec<NativeBatchJobSpec> = Vec::new();
    let compile_result = (|| -> io::Result<()> {
        for (batch_idx, batch_funcs) in batches.into_iter().enumerate() {
            let batch_ops = batch_funcs
                .iter()
                .fold(0usize, |ops, func| ops.saturating_add(func.ops.len()));
            eprintln!(
                "{}: batch {}/{total_batches} ({} functions, {} ops / budget {})",
                options.log_prefix,
                batch_idx + 1,
                batch_funcs.len(),
                batch_ops,
                batch_ops_budget
            );
            let batch_ir = SimpleIR {
                functions: batch_funcs,
                profile: profile.clone(),
            };
            let external_function_names =
                batch_external_function_names(&all_func_names, &batch_ir.functions);
            let job_path = tmp_dir.join(format!("batch_{batch_idx}.json"));
            let batch_path = tmp_dir.join(format!("batch_{batch_idx}.o"));
            write_json_artifact(
                &job_path,
                &NativeBatchObjectJob {
                    ir: batch_ir,
                    module_context_path: module_context_path.clone(),
                    target_triple: options.target_triple.map(str::to_owned),
                    emit_app_callable_resolver: batch_idx == 0,
                    app_callable_manifest: if batch_idx == 0 {
                        options.app_callable_manifest.take()
                    } else {
                        None
                    },
                    external_function_names,
                    // The resolver-emitting batch is the main application
                    // object; it also owns the module registry blob. Init
                    // functions in sibling batches resolve through Import
                    // relocations at link.
                    module_registry: if batch_idx == 0 {
                        options.module_registry.take()
                    } else {
                        None
                    },
                },
            )?;
            batch_specs.push(NativeBatchJobSpec {
                job_path,
                object_path: batch_path,
            });
        }
        release_native_backend_batch_memory_to_os();

        let mut batch_paths: Vec<std::path::PathBuf> = Vec::with_capacity(batch_specs.len());
        for (batch_idx, spec) in batch_specs.iter().enumerate() {
            eprintln!(
                "{}: compiling materialized batch {}/{}",
                options.log_prefix,
                batch_idx + 1,
                total_batches
            );
            run_native_batch_worker_with_failure_artifacts(
                "native application batch worker",
                &spec.job_path,
                &spec.object_path,
            )?;
            batch_paths.push(spec.object_path.clone());
            release_native_backend_batch_memory_to_os();
        }

        merge_relocatable_objects(output_path, &batch_paths, None)
    })();

    finish_native_batch_temp_dir(
        &tmp_dir,
        "native application batch temp dir",
        compile_result,
    )?;

    eprintln!(
        "Successfully compiled to {} ({} functions, {} batches)",
        output_path.display(),
        function_count,
        total_batches
    );
    Ok(NativeApplicationObjectResult {
        function_count,
        batch_count: total_batches,
    })
}
