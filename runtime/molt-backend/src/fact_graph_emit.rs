use molt_backend::SimpleIR;
use molt_backend::tir::target_info::TargetInfo;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub(crate) struct FactGraphEmitRequest<'a> {
    pub(crate) output_path: &'a Path,
    pub(crate) function_name: &'a str,
    pub(crate) target_info: &'a TargetInfo,
}

fn write_json_artifact_path<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, value).map_err(io::Error::other)
}

pub(crate) fn emit_fact_graph_for_ir(
    ir: &SimpleIR,
    request: FactGraphEmitRequest<'_>,
) -> io::Result<()> {
    let mut functions = ir.functions.clone();
    let mut tir_run = molt_backend::tir::pipeline_cache::run_cached_tir_pipeline(
        &mut functions,
        molt_backend::tir::pipeline_cache::TirPipelineRunOptions {
            target_info: request.target_info.clone(),
            cache_flavor: molt_backend::tir::pipeline_cache::TirPipelineCacheFlavor::FactGraph,
            cache_dir: None,
            process_externs: false,
            verify_lir: false,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: molt_backend::tir::pipeline_cache::tir_optimization_resource_plan(),
        },
        |_| {},
    );
    let non_inlinable = HashSet::new();
    let module_run = molt_backend::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
        &functions,
        &mut tir_run.cached_tir,
        molt_backend::tir::pipeline_cache::TirOwnedModulePipelineOptions {
            target_info: request.target_info,
            module_name: "fact_graph_module",
            non_inlinable: &non_inlinable,
            missing_tir_context: "fact graph TIR cache runner",
            mode: molt_backend::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase,
        },
    );
    let module_analysis = module_run
        .module_analysis
        .expect("fact graph owned TIR run must execute module phase");

    let selected = module_run
        .tir_functions
        .iter()
        .filter(|(is_extern, _)| !*is_extern)
        .map(|(_, func)| func)
        .find(|func| func.name == request.function_name)
        .ok_or_else(|| {
            let mut names: Vec<_> = module_run
                .tir_functions
                .iter()
                .filter(|(is_extern, _)| !*is_extern)
                .map(|(_, func)| func.name.as_str())
                .collect();
            names.sort_unstable();
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "fact graph function '{}' not found after TIR module pipeline; available: {}",
                    request.function_name,
                    names.join(", ")
                ),
            )
        })?;

    let graph = match module_analysis.call_facts_for(&selected.name) {
        Some(call_facts) => molt_backend::tir::FactGraph::build_with_call_facts(
            selected,
            call_facts,
            "ModuleAnalysis::call_facts",
        ),
        None => molt_backend::tir::FactGraph::build_local(selected),
    };
    write_json_artifact_path(request.output_path, &graph)
}

#[cfg(test)]
mod tests {
    use super::{FactGraphEmitRequest, emit_fact_graph_for_ir};
    use molt_backend::{FunctionIR, OpIR, SimpleIR};
    use std::io;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fact_graph_emit_writes_precise_compiler_graph() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: Vec::new(),
                param_types: None,
                ops: vec![
                    OpIR {
                        kind: "call".to_string(),
                        s_value: Some("opaque_helper".to_string()),
                        args: Some(Vec::new()),
                        out: Some("result".to_string()),
                        source_line: Some(3),
                        col_offset: Some(4),
                        end_col_offset: Some(18),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["result".to_string()]),
                        ..OpIR::default()
                    },
                ],
                source_file: Some("app.py".to_string()),
                is_extern: false,
            }],
            profile: None,
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "molt-backend-fact-graph-{}-{nonce}",
            std::process::id()
        ));
        let output = root.join("nested").join("molt_main.fact_graph.json");
        let target_info = molt_backend::tir::target_info::TargetInfo::native_release_fast();

        emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: &output,
                function_name: "molt_main",
                target_info: &target_info,
            },
        )
        .expect("fact graph emission");

        assert!(output.is_file(), "fact graph output file must be created");
        let graph: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output).expect("read graph"))
                .expect("valid graph json");
        assert_eq!(graph["schema_version"], 3);
        assert_eq!(graph["kind"], "molt_tir_fact_graph");
        assert_eq!(graph["function"], "molt_main");
        assert_eq!(graph["summary"]["call_fact_count"], 6);
        let call_target_from_module_analysis = graph["values"]
            .as_array()
            .expect("values array")
            .iter()
            .flat_map(|value| value["facts"].as_array().expect("facts array"))
            .any(|fact| {
                fact["kind"] == "call.target"
                    && fact["value"] == "Opaque"
                    && fact["producer"] == "ModuleAnalysis::call_facts"
            });
        assert!(
            call_target_from_module_analysis,
            "fact graph must serialize the module-analysis call target, not a local fallback: {graph}"
        );
        let sourced_call_fact = graph["values"]
            .as_array()
            .expect("values array")
            .iter()
            .flat_map(|value| value["facts"].as_array().expect("facts array"))
            .any(|fact| {
                fact["kind"] == "call.target" && fact["source_site"]["source_file"] == "app.py"
            });
        assert!(
            sourced_call_fact,
            "fact graph source sites must carry FunctionIR.source_file: {graph}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fact_graph_emit_fails_closed_on_missing_function() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: Vec::new(),
                param_types: None,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "molt-backend-fact-graph-missing-{}-{nonce}",
            std::process::id()
        ));
        let output = root.join("graph.json");
        let target_info = molt_backend::tir::target_info::TargetInfo::native_release_fast();

        let err = emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: &output,
                function_name: "missing",
                target_info: &target_info,
            },
        )
        .expect_err("missing function must fail closed");

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(
            err.to_string().contains("available: molt_main"),
            "missing-function diagnostic must list available functions: {err}"
        );
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
