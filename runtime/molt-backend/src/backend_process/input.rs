use std::io;

use super::io_limits::{RequestBoundedRead, read_bounded_request_bytes, stdin_request_limit_bytes};

pub(crate) fn read_backend_ir_document(
    ir_format: &str,
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    let stdin_request_limit_bytes = stdin_request_limit_bytes();
    if ir_format == "msgpack" {
        if let Some(ir_path) = ir_file_path {
            let file = std::fs::File::open(ir_path).map_err(|e| {
                io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
            })?;
            let reader = io::BufReader::new(file);
            return match rmp_serde::from_read::<_, molt_backend::BackendIrDocument>(reader) {
                Ok(ir) => Ok(ir),
                Err(err) => {
                    eprintln!("invalid msgpack IR: {err}");
                    std::process::exit(1);
                }
            };
        }

        let stdin = io::stdin();
        let bounded = RequestBoundedRead::new(
            stdin.lock(),
            stdin_request_limit_bytes,
            "backend stdin request",
        );
        let reader = io::BufReader::with_capacity(1 << 20, bounded);
        return match rmp_serde::from_read::<_, molt_backend::BackendIrDocument>(reader) {
            Ok(ir) => Ok(ir),
            Err(err) => {
                eprintln!("invalid msgpack IR: {err}");
                std::process::exit(1);
            }
        };
    }

    if ir_format == "cbor" {
        #[cfg(not(feature = "cbor"))]
        {
            eprintln!("CBOR support requires the 'cbor' feature");
            std::process::exit(1);
        }
        #[cfg(feature = "cbor")]
        {
            if let Some(ir_path) = ir_file_path {
                let file = std::fs::File::open(ir_path).map_err(|e| {
                    io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
                })?;
                let reader = io::BufReader::new(file);
                return match ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(reader)
                {
                    Ok(ir) => Ok(ir),
                    Err(err) => {
                        eprintln!("invalid CBOR IR: {err}");
                        std::process::exit(1);
                    }
                };
            }

            let buf = read_bounded_request_bytes(
                io::stdin().lock(),
                stdin_request_limit_bytes,
                "backend stdin request",
            )?;
            let result = ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(&buf[..]);
            drop(buf);
            return match result {
                Ok(ir) => Ok(ir),
                Err(err) => {
                    eprintln!("invalid CBOR IR: {err}");
                    std::process::exit(1);
                }
            };
        }
    }

    if ir_format == "ndjson" {
        if let Some(ir_path) = ir_file_path {
            let file = std::fs::File::open(ir_path).map_err(|e| {
                io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
            })?;
            let reader = io::BufReader::new(file);
            return match molt_backend::BackendIrDocument::from_ndjson_reader(reader) {
                Ok(ir) => Ok(ir),
                Err(err) => {
                    eprintln!("invalid NDJSON IR: {err}");
                    std::process::exit(1);
                }
            };
        }

        let stdin = io::stdin();
        let bounded = RequestBoundedRead::new(
            stdin.lock(),
            stdin_request_limit_bytes,
            "backend stdin request",
        );
        let reader = io::BufReader::new(bounded);
        return match molt_backend::BackendIrDocument::from_ndjson_reader(reader) {
            Ok(ir) => Ok(ir),
            Err(err) => {
                eprintln!("invalid NDJSON IR: {err}");
                std::process::exit(1);
            }
        };
    }

    if let Some(ir_path) = ir_file_path {
        let file = std::fs::File::open(ir_path).map_err(|e| {
            io::Error::other(format!("failed to open IR file '{}': {}", ir_path, e))
        })?;
        let reader = io::BufReader::with_capacity(1 << 20, file);
        return match serde_json::from_reader::<_, molt_backend::BackendIrDocument>(reader) {
            Ok(ir) => Ok(ir),
            Err(err) => {
                eprintln!("invalid IR JSON: {err}");
                std::process::exit(1);
            }
        };
    }

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
    let result = serde_json::from_str::<molt_backend::BackendIrDocument>(&buffer);
    drop(buffer);
    match result {
        Ok(ir) => Ok(ir),
        Err(err) => {
            eprintln!("invalid IR JSON: {err}");
            std::process::exit(1);
        }
    }
}
