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
    let mut tir_run = molt_backend::tir::pipeline_cache::run_cached_tir_pipeline(
        molt_backend::tir::pipeline_cache::TirPipelineInput::TirOnly(&ir.functions),
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
    let tir_functions =
        molt_backend::tir::pipeline_cache::run_owned_module_pipeline_from_cached_tir(
            &ir.functions,
            &mut tir_run.cached_tir,
            molt_backend::tir::pipeline_cache::TirOwnedModulePipelineOptions {
                target_info: request.target_info,
                module_name: "fact_graph_module",
                non_inlinable: &non_inlinable,
                missing_tir_context: "fact graph TIR cache runner",
                mode: molt_backend::tir::pipeline_cache::TirOwnedModulePipelineMode::ModulePhase,
            },
        );
    let module = molt_backend::tir::TirModule {
        name: "fact_graph_module".to_string(),
        functions: tir_functions
            .into_iter()
            .filter_map(|(is_extern, func)| (!is_extern).then_some(func))
            .collect(),
    };
    let selected = module
        .functions
        .iter()
        .find(|func| func.name == request.function_name)
        .ok_or_else(|| {
            let mut names: Vec<_> = module
                .functions
                .iter()
                .map(|func| func.name.as_str())
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

    let call_graph = molt_backend::tir::call_graph::CallGraph::build(&module);
    let summaries =
        molt_backend::tir::passes::ip_summary::ModuleSummaries::compute(&module, &call_graph);
    let call_facts = molt_backend::tir::call_facts::CallFactsTable::build_for_function(
        selected,
        &module,
        &call_graph,
        &summaries,
        request.target_info,
    );
    let graph = molt_backend::tir::FactGraph::build_with_call_facts(
        selected,
        &call_facts,
        "CallFactsTable::build_for_function",
    );
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
                return_abi: molt_ir::FunctionReturnAbi::Value,
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
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
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

        let original = serde_json::to_vec(&ir).expect("source IR");
        emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: &output,
                function_name: "molt_main",
                target_info: &target_info,
            },
        )
        .expect("fact graph emission");

        assert_eq!(
            serde_json::to_vec(&ir).expect("source IR after diagnostic"),
            original
        );
        assert!(output.is_file(), "fact graph output file must be created");
        let graph: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output).expect("read graph"))
                .expect("valid graph json");
        assert_eq!(graph["schema_version"], 3);
        assert_eq!(graph["kind"], "molt_tir_fact_graph");
        assert_eq!(graph["function"], "molt_main");
        assert_eq!(graph["summary"]["call_fact_count"], 6);
        let precise_call_target = graph["values"]
            .as_array()
            .expect("values array")
            .iter()
            .flat_map(|value| value["facts"].as_array().expect("facts array"))
            .any(|fact| {
                fact["kind"] == "call.target"
                    && fact["value"] == "Opaque"
                    && fact["producer"] == "CallFactsTable::build_for_function"
            });
        assert!(
            precise_call_target,
            "fact graph must serialize the requested precise call target: {graph}"
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
    fn fact_graph_emit_resolves_recursive_scc_but_keeps_extern_calls_opaque() {
        let calling = |name: &str, targets: &[&str]| FunctionIR {
            name: name.into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            source_file: Some("recursive.py".into()),
            ops: targets
                .iter()
                .enumerate()
                .map(|(index, target)| OpIR {
                    kind: "call".into(),
                    s_value: Some((*target).into()),
                    args: Some(Vec::new()),
                    out: Some(format!("result_{index}")),
                    source_line: Some(7 + index as i64),
                    ..OpIR::default()
                })
                .chain(std::iter::once(OpIR {
                    kind: "ret".into(),
                    args: Some(vec![format!("result_{}", targets.len() - 1)]),
                    ..OpIR::default()
                }))
                .collect(),
            ..FunctionIR::default()
        };
        let ir = SimpleIR {
            functions: vec![
                calling("selected", &["recursive_a", "external"]),
                calling("recursive_a", &["recursive_b"]),
                calling("recursive_b", &["recursive_a"]),
                FunctionIR {
                    name: "external".into(),
                    is_extern: true,
                    return_abi: molt_ir::FunctionReturnAbi::Value,
                    ..FunctionIR::default()
                },
            ],
            profile: None,
        };
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "molt-fact-graph-scc-{}-{nonce}",
            std::process::id()
        ));
        let output = root.join("graph.json");
        let target_info = molt_backend::tir::target_info::TargetInfo::native_release_fast();
        emit_fact_graph_for_ir(
            &ir,
            FactGraphEmitRequest {
                output_path: &output,
                function_name: "selected",
                target_info: &target_info,
            },
        )
        .expect("fact graph emission");
        let graph: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let facts: Vec<_> = graph["values"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|value| value["facts"].as_array().unwrap())
            .collect();
        for (line, kind, expected) in [
            (7, "call.target", "StaticDirect(recursive_a)"),
            (7, "call.leaf", "False"),
            (7, "call.inlinable", "WhyNot(Recursive)"),
            (8, "call.target", "Opaque"),
            (8, "call.leaf", "Unknown"),
        ] {
            let fact = facts
                .iter()
                .find(|fact| fact["kind"] == kind && fact["source_site"]["line"] == line)
                .unwrap_or_else(|| panic!("missing {kind} at line {line}: {graph}"));
            assert_eq!(fact["value"], expected);
            assert_eq!(fact["producer"], "CallFactsTable::build_for_function");
            assert_eq!(fact["source_site"]["source_file"], "recursive.py");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn fact_graph_emit_fails_closed_on_missing_function() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_main".to_string(),
                params: Vec::new(),
                param_types: None,
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
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
