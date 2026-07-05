use super::model::{BackendCliArgs, WasmCliOptions};

impl<'a> BackendCliArgs<'a> {
    pub(crate) fn parse(args: &'a [String]) -> Self {
        let mut parsed = Self {
            wants_features: false,
            wants_daemon: false,
            socket_path: None,
            is_wasm: false,
            is_rust: false,
            is_luau: false,
            use_ir_pipeline: false,
            target_triple: None,
            output_path: None,
            native_batch_job_file: None,
            ir_file_path: None,
            fact_graph_output_path: None,
            fact_graph_function: None,
            wasm_options: WasmCliOptions::default(),
            ir_format: "json",
        };

        let mut saw_target_flag = false;
        let mut saw_wasm_value = false;
        let mut saw_rust_value = false;
        let mut saw_luau_value = false;
        let mut saw_socket_flag = false;
        let mut saw_target_triple_flag = false;
        let mut saw_output_flag = false;
        let mut saw_native_batch_job_file_flag = false;
        let mut saw_ir_file_flag = false;
        let mut saw_fact_graph_output_flag = false;
        let mut saw_fact_graph_function_flag = false;
        let mut saw_wasm_data_base_flag = false;
        let mut saw_wasm_table_base_flag = false;
        let mut saw_wasm_split_runtime_runtime_table_min_flag = false;
        let mut saw_ir_format_flag = false;

        for (idx, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "--features" => parsed.wants_features = true,
                "--daemon" => parsed.wants_daemon = true,
                "--target" => saw_target_flag = true,
                "wasm" => saw_wasm_value = true,
                "rust" => saw_rust_value = true,
                "luau" => saw_luau_value = true,
                "--ir-pipeline" => parsed.use_ir_pipeline = true,
                "--wasm-link" => parsed.wasm_options.link_relocs = true,
                "--socket" if !saw_socket_flag => {
                    saw_socket_flag = true;
                    parsed.socket_path = value_after(args, idx);
                }
                "--target-triple" if !saw_target_triple_flag => {
                    saw_target_triple_flag = true;
                    parsed.target_triple = value_after(args, idx);
                }
                "--output" if !saw_output_flag => {
                    saw_output_flag = true;
                    parsed.output_path = value_after(args, idx);
                }
                "--native-batch-job-file" if !saw_native_batch_job_file_flag => {
                    saw_native_batch_job_file_flag = true;
                    parsed.native_batch_job_file = value_after(args, idx);
                }
                "--ir-file" if !saw_ir_file_flag => {
                    saw_ir_file_flag = true;
                    parsed.ir_file_path = value_after(args, idx);
                }
                "--fact-graph-output" if !saw_fact_graph_output_flag => {
                    saw_fact_graph_output_flag = true;
                    parsed.fact_graph_output_path = value_after(args, idx);
                }
                "--fact-graph-function" if !saw_fact_graph_function_flag => {
                    saw_fact_graph_function_flag = true;
                    parsed.fact_graph_function = value_after(args, idx);
                }
                "--wasm-data-base" if !saw_wasm_data_base_flag => {
                    saw_wasm_data_base_flag = true;
                    parsed.wasm_options.data_base = parse_u32_after(args, idx);
                }
                "--wasm-table-base" if !saw_wasm_table_base_flag => {
                    saw_wasm_table_base_flag = true;
                    parsed.wasm_options.table_base = parse_u32_after(args, idx);
                }
                "--wasm-split-runtime-runtime-table-min"
                    if !saw_wasm_split_runtime_runtime_table_min_flag =>
                {
                    saw_wasm_split_runtime_runtime_table_min_flag = true;
                    parsed.wasm_options.split_runtime_runtime_table_min =
                        parse_u32_after(args, idx);
                }
                "--ir-format" if !saw_ir_format_flag => {
                    saw_ir_format_flag = true;
                    parsed.ir_format = value_after(args, idx).unwrap_or(parsed.ir_format);
                }
                _ => {}
            }
        }

        parsed.is_wasm = saw_target_flag && saw_wasm_value;
        parsed.is_rust = saw_target_flag && saw_rust_value;
        parsed.is_luau = saw_target_flag && saw_luau_value;
        parsed
    }
}

fn value_after(args: &[String], idx: usize) -> Option<&str> {
    args.get(idx + 1).map(String::as_str)
}

fn parse_u32_after(args: &[String], idx: usize) -> Option<u32> {
    value_after(args, idx).and_then(|raw| raw.parse::<u32>().ok())
}
