use crate::backend_process::{
    BackendOutputKind, RequestBoundedRead, read_bounded_request_bytes, stdin_request_limit_bytes,
};
use std::io;

#[derive(Debug, Clone, Copy)]
pub(crate) struct BackendCliRequest<'a> {
    pub(crate) is_wasm: bool,
    pub(crate) is_rust: bool,
    pub(crate) is_luau: bool,
    #[cfg_attr(not(feature = "luau-backend"), allow(dead_code))]
    pub(crate) use_ir_pipeline: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<&'a str>,
    pub(crate) output_path: Option<&'a str>,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) native_batch_job_file: Option<&'a str>,
    pub(crate) fact_graph_output_path: Option<&'a str>,
    pub(crate) fact_graph_function: Option<&'a str>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_link_flag: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_table_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_split_runtime_runtime_table_min: Option<u32>,
    ir_file_path: Option<&'a str>,
    ir_format: &'a str,
}

impl<'a> BackendCliRequest<'a> {
    pub(crate) fn parse(args: &'a [String]) -> io::Result<Self> {
        let request = Self {
            is_wasm: target_is(args, "wasm"),
            is_rust: target_is(args, "rust"),
            is_luau: target_is(args, "luau"),
            use_ir_pipeline: has_flag(args, "--ir-pipeline"),
            target_triple: arg_value(args, "--target-triple"),
            output_path: arg_value(args, "--output"),
            native_batch_job_file: arg_value(args, "--native-batch-job-file"),
            fact_graph_output_path: arg_value(args, "--fact-graph-output"),
            fact_graph_function: arg_value(args, "--fact-graph-function"),
            wasm_link_flag: has_flag(args, "--wasm-link"),
            wasm_data_base: arg_value(args, "--wasm-data-base").and_then(parse_u32),
            wasm_table_base: arg_value(args, "--wasm-table-base").and_then(parse_u32),
            wasm_split_runtime_runtime_table_min: arg_value(
                args,
                "--wasm-split-runtime-runtime-table-min",
            )
            .and_then(parse_u32),
            ir_file_path: arg_value(args, "--ir-file"),
            ir_format: arg_value(args, "--ir-format").unwrap_or("json"),
        };
        validate_fact_graph_cli_contract(
            request.fact_graph_output_path,
            request.fact_graph_function,
            request.is_rust,
        )?;
        Ok(request)
    }

    pub(crate) fn output_kind(&self) -> BackendOutputKind {
        if self.is_luau {
            BackendOutputKind::Luau
        } else if self.is_rust {
            BackendOutputKind::Rust
        } else if self.is_wasm {
            BackendOutputKind::Wasm
        } else {
            BackendOutputKind::Native
        }
    }

    pub(crate) fn read_document(&self) -> io::Result<molt_backend::BackendIrDocument> {
        read_backend_ir_document(self.ir_format, self.ir_file_path)
    }
}

pub(crate) fn validate_fact_graph_cli_contract(
    output_path: Option<&str>,
    function_name: Option<&str>,
    is_rust: bool,
) -> io::Result<()> {
    if output_path.is_some() != function_name.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--fact-graph-output and --fact-graph-function must be supplied together",
        ));
    }
    if output_path.is_some() && is_rust {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fact graph emission does not support the rust target",
        ));
    }
    Ok(())
}

fn target_is(args: &[String], target: &str) -> bool {
    has_flag(args, "--target") && args.iter().any(|arg| arg == target)
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|idx| args.get(idx + 1))
        .map(String::as_str)
}

fn parse_u32(raw: &str) -> Option<u32> {
    raw.parse::<u32>().ok()
}

fn read_backend_ir_document(
    ir_format: &str,
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    let stdin_request_limit_bytes = stdin_request_limit_bytes();
    if ir_format == "msgpack" {
        read_msgpack_ir_document(ir_file_path, stdin_request_limit_bytes)
    } else if ir_format == "cbor" {
        read_cbor_ir_document(ir_file_path, stdin_request_limit_bytes)
    } else if ir_format == "ndjson" {
        read_ndjson_ir_document(ir_file_path, stdin_request_limit_bytes)
    } else if let Some(ir_path) = ir_file_path {
        read_json_ir_file(ir_path)
    } else {
        read_json_ir_stdin(stdin_request_limit_bytes)
    }
}

fn read_msgpack_ir_document(
    ir_file_path: Option<&str>,
    stdin_request_limit_bytes: usize,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = std::fs::File::open(ir_path).map_err(|err| {
            io::Error::other(format!("failed to open IR file '{ir_path}': {err}"))
        })?;
        let reader = io::BufReader::new(file);
        rmp_serde::from_read::<_, molt_backend::BackendIrDocument>(reader).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid msgpack IR: {err}"),
            )
        })
    } else {
        let stdin = io::stdin();
        let bounded = RequestBoundedRead::new(
            stdin.lock(),
            stdin_request_limit_bytes,
            "backend stdin request",
        );
        let reader = io::BufReader::with_capacity(1 << 20, bounded);
        rmp_serde::from_read::<_, molt_backend::BackendIrDocument>(reader).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid msgpack IR: {err}"),
            )
        })
    }
}

fn read_cbor_ir_document(
    ir_file_path: Option<&str>,
    stdin_request_limit_bytes: usize,
) -> io::Result<molt_backend::BackendIrDocument> {
    #[cfg(not(feature = "cbor"))]
    {
        let _ = (ir_file_path, stdin_request_limit_bytes);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "CBOR support requires the 'cbor' feature",
        ))
    }
    #[cfg(feature = "cbor")]
    {
        if let Some(ir_path) = ir_file_path {
            let file = std::fs::File::open(ir_path).map_err(|err| {
                io::Error::other(format!("failed to open IR file '{ir_path}': {err}"))
            })?;
            let reader = io::BufReader::new(file);
            ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(reader).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid CBOR IR: {err}"),
                )
            })
        } else {
            let buf = read_bounded_request_bytes(
                io::stdin().lock(),
                stdin_request_limit_bytes,
                "backend stdin request",
            )?;
            ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(&buf[..]).map_err(
                |err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid CBOR IR: {err}"),
                    )
                },
            )
        }
    }
}

fn read_ndjson_ir_document(
    ir_file_path: Option<&str>,
    stdin_request_limit_bytes: usize,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = std::fs::File::open(ir_path).map_err(|err| {
            io::Error::other(format!("failed to open IR file '{ir_path}': {err}"))
        })?;
        let reader = io::BufReader::new(file);
        molt_backend::BackendIrDocument::from_ndjson_reader(reader).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid NDJSON IR: {err}"),
            )
        })
    } else {
        let stdin = io::stdin();
        let bounded = RequestBoundedRead::new(
            stdin.lock(),
            stdin_request_limit_bytes,
            "backend stdin request",
        );
        let reader = io::BufReader::new(bounded);
        molt_backend::BackendIrDocument::from_ndjson_reader(reader).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid NDJSON IR: {err}"),
            )
        })
    }
}

fn read_json_ir_file(ir_path: &str) -> io::Result<molt_backend::BackendIrDocument> {
    let file = std::fs::File::open(ir_path)
        .map_err(|err| io::Error::other(format!("failed to open IR file '{ir_path}': {err}")))?;
    let reader = io::BufReader::with_capacity(1 << 20, file);
    serde_json::from_reader::<_, molt_backend::BackendIrDocument>(reader).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid IR JSON: {err}"),
        )
    })
}

fn read_json_ir_stdin(
    stdin_request_limit_bytes: usize,
) -> io::Result<molt_backend::BackendIrDocument> {
    let raw_bytes = read_bounded_request_bytes(
        io::stdin().lock(),
        stdin_request_limit_bytes,
        "backend stdin request",
    )?;
    let buffer = String::from_utf8(raw_bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("backend stdin request is not UTF-8: {err}"),
        )
    })?;
    serde_json::from_str::<molt_backend::BackendIrDocument>(&buffer).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid IR JSON: {err}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn parse_wasm_request_flags_selects_wasm_output_kind() {
        let args = argv(&[
            "molt-backend",
            "--target",
            "wasm",
            "--output",
            "dist/app.wasm",
            "--wasm-link",
            "--wasm-data-base",
            "1048576",
            "--wasm-table-base",
            "4096",
            "--wasm-split-runtime-runtime-table-min",
            "8192",
        ]);

        let request = BackendCliRequest::parse(&args).expect("wasm request");

        assert!(request.is_wasm);
        assert!(!request.is_luau);
        assert!(!request.is_rust);
        assert_eq!(request.output_path, Some("dist/app.wasm"));
        assert!(request.wasm_link_flag);
        assert_eq!(request.wasm_data_base, Some(1048576));
        assert_eq!(request.wasm_table_base, Some(4096));
        assert_eq!(request.wasm_split_runtime_runtime_table_min, Some(8192));
        assert_eq!(request.output_kind(), BackendOutputKind::Wasm);
    }

    #[test]
    fn parse_default_request_selects_native_output_kind() {
        let args = argv(&["molt-backend", "--output", "dist/app.o"]);

        let request = BackendCliRequest::parse(&args).expect("native request");

        assert!(!request.is_wasm);
        assert!(!request.is_luau);
        assert!(!request.is_rust);
        assert_eq!(request.output_path, Some("dist/app.o"));
        assert_eq!(request.output_kind(), BackendOutputKind::Native);
    }
}
