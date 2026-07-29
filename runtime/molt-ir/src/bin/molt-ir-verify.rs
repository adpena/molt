//! Long-lived NDJSON transport for the canonical `molt-ir` structural verifier.

use std::io::{self, BufRead, Write};

use molt_ir::{FunctionIR, PgoProfileIR, SimpleIR, verify_simple_ir};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct UncheckedSimpleIr {
    functions: Vec<FunctionIR>,
    #[serde(default)]
    profile: Option<PgoProfileIR>,
}

#[derive(Deserialize)]
struct Request {
    id: u64,
    ir: UncheckedSimpleIr,
}

#[derive(Serialize)]
struct Response {
    schema: &'static str,
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<molt_ir::SimpleIrVerificationReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport_error: Option<String>,
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Request>(&line) {
            Ok(request) => serde_json::to_writer(
                &mut stdout,
                &Response {
                    schema: "molt.simple-ir-verification.v1",
                    id: Some(request.id),
                    report: Some(verify_simple_ir(&SimpleIR {
                        functions: request.ir.functions,
                        profile: request.ir.profile,
                    })),
                    transport_error: None,
                },
            )?,
            Err(error) => serde_json::to_writer(
                &mut stdout,
                &Response {
                    schema: "molt.simple-ir-verification.v1",
                    id: None,
                    report: None,
                    transport_error: Some(error.to_string()),
                },
            )?,
        }
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}
