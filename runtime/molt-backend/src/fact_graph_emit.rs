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
    let (mut tir_module, _) =
        molt_backend::tir::lower_from_simple::lower_functions_to_tir_module(&ir.functions);
    tir_module.name = "fact_graph_module".to_string();

    for tir_func in &mut tir_module.functions {
        molt_backend::tir::type_refine::refine_types(tir_func);
        let _stats = molt_backend::tir::passes::run_pipeline(tir_func, request.target_info);
        molt_backend::tir::type_refine::refine_types(tir_func);
    }

    let non_inlinable = HashSet::new();
    let module_analysis = molt_backend::tir::run_module_pipeline(
        &mut tir_module,
        request.target_info,
        &non_inlinable,
    );

    let selected = tir_module
        .functions
        .iter()
        .find(|func| func.name == request.function_name)
        .ok_or_else(|| {
            let mut names: Vec<_> = tir_module
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
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["result".to_string()]),
                        ..OpIR::default()
                    },
                ],
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
        assert_eq!(graph["schema_version"], 1);
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
