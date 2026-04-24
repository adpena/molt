//! Standalone MLIR backend binary.
//!
//! Reads SimpleIR JSON on stdin, lowers each function through the TIR->MLIR
//! pipeline, and writes the combined MLIR text to stdout (or a file when
//! `--output` is given).
//!
//! Protocol:
//!   stdin  = SimpleIR JSON (same format as `molt-backend`)
//!   stdout = MLIR text (one module per TIR function, concatenated)
//!   stderr = diagnostics / errors
//!
//! Exit codes:
//!   0 = success
//!   1 = pipeline error (parse, lowering, verification)
//!   2 = usage error (bad arguments)

use std::io::{self, Read, Write};
use std::process::ExitCode;

use molt_backend::tir::lower_from_simple::lower_to_tir;
use molt_backend::{FunctionIR, SimpleIR};
use molt_backend_mlir::{MlirCompileOptions, MlirOptLevel, compile_via_mlir};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    let mut output_path: Option<String> = None;
    let mut emit_llvm = false;
    let mut opt_level = MlirOptLevel::O2;
    let mut jit_func: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--output" | "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --output requires a path argument");
                    return ExitCode::from(2);
                }
                output_path = Some(args[i].clone());
            }
            "--emit-llvm" => {
                emit_llvm = true;
            }
            "--opt-level" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --opt-level requires a value (O0, O1, O2, O3)");
                    return ExitCode::from(2);
                }
                opt_level = match args[i].as_str() {
                    "O0" | "0" => MlirOptLevel::O0,
                    "O1" | "1" => MlirOptLevel::O1,
                    "O2" | "2" => MlirOptLevel::O2,
                    "O3" | "3" => MlirOptLevel::O3,
                    other => {
                        eprintln!("Error: invalid opt-level '{other}'. Use O0, O1, O2, or O3.");
                        return ExitCode::from(2);
                    }
                };
            }
            "--jit" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --jit requires a function name");
                    return ExitCode::from(2);
                }
                jit_func = Some(args[i].clone());
            }
            "--help" | "-h" => {
                eprintln!("Usage: molt-backend-mlir [OPTIONS]");
                eprintln!();
                eprintln!("Reads SimpleIR JSON on stdin, outputs MLIR text on stdout.");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  -o, --output PATH    Write MLIR text to file instead of stdout");
                eprintln!("  --emit-llvm          Also lower to LLVM dialect and emit it");
                eprintln!("  --opt-level LEVEL    Optimization level: O0, O1, O2, O3 (default: O2)");
                eprintln!("  --jit FUNC           JIT-execute the named function (i64 args, i64 result)");
                eprintln!("  -h, --help           Show this help message");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("Error: unknown argument '{other}'. Use --help for usage.");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    // Read SimpleIR JSON from stdin.
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("Error: failed to read stdin: {e}");
        return ExitCode::FAILURE;
    }

    let ir: SimpleIR = match serde_json::from_str(&input) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("Error: failed to parse SimpleIR JSON: {e}");
            return ExitCode::FAILURE;
        }
    };

    if ir.functions.is_empty() {
        eprintln!("Warning: no functions in input IR");
        return ExitCode::SUCCESS;
    }

    // JIT mode: compile and execute a single function.
    if let Some(ref func_name) = jit_func {
        return run_jit(&ir.functions, func_name);
    }

    // Compilation mode: lower all functions to MLIR.
    let options = MlirCompileOptions {
        opt_level,
        emit_llvm_dialect: emit_llvm,
    };

    let mut output_text = String::new();
    let mut error_count = 0;

    for func_ir in &ir.functions {
        // Skip extern declarations (no body to lower).
        if func_ir.is_extern {
            continue;
        }

        let tir_func = lower_to_tir(func_ir);
        match compile_via_mlir(&tir_func, &options) {
            Ok(result) => {
                if emit_llvm && !result.llvm_dialect_text.is_empty() {
                    output_text.push_str(&result.llvm_dialect_text);
                } else {
                    output_text.push_str(&result.optimized_mlir_text);
                }
                output_text.push('\n');
            }
            Err(e) => {
                eprintln!(
                    "Error: MLIR pipeline failed for function '{}': {e}",
                    func_ir.name
                );
                error_count += 1;
            }
        }
    }

    if error_count > 0 {
        eprintln!("{error_count} function(s) failed MLIR lowering");
        if output_text.is_empty() {
            return ExitCode::FAILURE;
        }
        // Partial output: still write what we got.
        eprintln!("Writing partial MLIR output for successfully lowered functions.");
    }

    // Write output.
    if let Some(ref path) = output_path {
        if let Err(e) = std::fs::write(path, &output_text) {
            eprintln!("Error: failed to write to '{path}': {e}");
            return ExitCode::FAILURE;
        }
        eprintln!("Wrote MLIR output to {path}");
    } else {
        if let Err(e) = io::stdout().write_all(output_text.as_bytes()) {
            eprintln!("Error: failed to write to stdout: {e}");
            return ExitCode::FAILURE;
        }
    }

    if error_count > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run_jit(functions: &[FunctionIR], func_name: &str) -> ExitCode {
    let func_ir = match functions.iter().find(|f| f.name == func_name) {
        Some(f) => f,
        None => {
            eprintln!(
                "Error: function '{func_name}' not found in IR. Available: {}",
                functions
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return ExitCode::FAILURE;
        }
    };

    let tir_func = lower_to_tir(func_ir);

    // Read JIT arguments from MOLT_JIT_ARGS env var (comma-separated i64 values).
    let jit_args: Vec<i64> = match std::env::var("MOLT_JIT_ARGS") {
        Ok(val) if !val.is_empty() => {
            match val.split(',').map(|s| s.trim().parse::<i64>()).collect() {
                Ok(args) => args,
                Err(e) => {
                    eprintln!("Error: invalid MOLT_JIT_ARGS value '{val}': {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        _ => vec![],
    };

    match molt_backend_mlir::jit_execute_i64(&tir_func, func_name, &jit_args) {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: JIT execution of '{func_name}' failed: {e}");
            ExitCode::FAILURE
        }
    }
}
