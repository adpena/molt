use molt_proof_supervisor::evidence::{
    MAX_RECEIPT_BYTES, durable_atomic_write, verify_event_artifact,
};
use molt_proof_supervisor::{
    ClosureMode, EventJournal, Policy, RECEIPT_SCHEMA, Receipt, platform, sha256_bytes,
};
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match dispatch(std::env::args().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("molt-proof-supervisor: {error}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: Vec<String>) -> Result<u8, String> {
    match args.as_slice() {
        [command, mode] if command == "capability" => {
            let mode = parse_mode(mode)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&platform::capability(mode))
                    .map_err(|error| error.to_string())?
            );
            Ok(0)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "run" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            run_policy(Path::new(policy), Path::new(receipt))
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "verify" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            verify_receipt(Path::new(policy), Path::new(receipt))
        }
        [command, fixture, code] if command == "fixture-child" && fixture == "exit" => {
            let code: u8 = code
                .parse()
                .map_err(|_| "fixture exit code must be 0..255".to_owned())?;
            Ok(code)
        }
        [command, fixture] if command == "fixture-child" && fixture == "spawn-self" => {
            let status = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "exit", "0"])
                .status()
                .map_err(|error| error.to_string())?;
            Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
        }
        [command, fixture, count] if command == "fixture-child" && fixture == "spawn-many" => {
            let count: usize = count
                .parse()
                .map_err(|_| "fixture spawn count must be an integer".to_owned())?;
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            for _ in 0..count {
                let status = Command::new(&executable)
                    .args(["fixture-child", "exit", "0"])
                    .status()
                    .map_err(|error| error.to_string())?;
                if !status.success() {
                    return Ok(status.code().unwrap_or(1).clamp(0, 255) as u8);
                }
            }
            Ok(0)
        }
        [command, fixture, auxiliary, marker]
            if command == "fixture-child" && fixture == "spawn-auxiliary" =>
        {
            Command::new(auxiliary)
                .args(["fixture-child", "write-pid-and-sleep-leaf", marker])
                .spawn()
                .map_err(|error| error.to_string())?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !Path::new(marker).is_file() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !Path::new(marker).is_file() {
                return Err("auxiliary fixture did not reach user code".to_owned());
            }
            Ok(0)
        }
        [command, fixture] if command == "fixture-child" && fixture == "thread-storm" => {
            let threads: Vec<_> = (0..256).map(|_| std::thread::spawn(|| {})).collect();
            for thread in threads {
                thread
                    .join()
                    .map_err(|_| "fixture thread panicked".to_owned())?;
            }
            Ok(0)
        }
        [command, fixture] if command == "fixture-child" && fixture == "application-breakpoint" => {
            application_breakpoint_fixture()
        }
        [command, fixture, marker]
            if command == "fixture-child" && fixture == "write-pid-and-sleep-leaf" =>
        {
            fs::write(marker, std::process::id().to_string())
                .map_err(|error| format!("cannot write fixture marker: {error}"))?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            Ok(0)
        }
        [command, fixture, root_marker, child_marker]
            if command == "fixture-child" && fixture == "write-pid-and-sleep-tree" =>
        {
            fs::write(root_marker, std::process::id().to_string())
                .map_err(|error| format!("cannot write fixture marker: {error}"))?;
            Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "write-pid-and-sleep-leaf", child_marker])
                .spawn()
                .map_err(|error| error.to_string())?;
            std::thread::sleep(std::time::Duration::from_secs(60));
            Ok(0)
        }
        _ => Err("usage: capability <leaf|declared-tree> | run --policy FILE --receipt FILE | verify --policy FILE --receipt FILE".to_owned()),
    }
}

#[cfg(windows)]
fn application_breakpoint_fixture() -> Result<u8, String> {
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::DebugBreak();
    }
    Ok(0)
}

#[cfg(not(windows))]
fn application_breakpoint_fixture() -> Result<u8, String> {
    Err("application-breakpoint fixture is Windows-only".to_owned())
}

fn verify_receipt(policy_path: &Path, receipt_path: &Path) -> Result<u8, String> {
    let policy_bytes = fs::read(policy_path)
        .map_err(|error| format!("cannot read policy {}: {error}", policy_path.display()))?;
    let raw_policy: Policy = serde_json::from_slice(&policy_bytes)
        .map_err(|error| format!("invalid policy: {error}"))?;
    let policy = raw_policy.validate()?;
    let receipt_bytes = fs::metadata(receipt_path)
        .map_err(|error| format!("cannot stat receipt {}: {error}", receipt_path.display()))?
        .len();
    if receipt_bytes > MAX_RECEIPT_BYTES as u64 {
        println!(
            "{}",
            serde_json::json!({
                "schema_valid": false,
                "receipt_size_valid": false,
                "receipt_bytes": receipt_bytes,
                "maximum_receipt_bytes": MAX_RECEIPT_BYTES,
            })
        );
        return Ok(79);
    }
    let bytes = fs::read(receipt_path)
        .map_err(|error| format!("cannot read receipt {}: {error}", receipt_path.display()))?;
    let receipt_size_valid = true;
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid receipt: {error}"))?;
    let identity_valid = receipt.identity_is_valid();
    let terminal_consistent = receipt.terminal_is_consistent();
    let lifecycle_valid = receipt.lifecycle_is_valid();
    let schema_valid = receipt.schema == RECEIPT_SCHEMA;
    let policy_digest_valid = receipt.policy_sha256 == policy.policy_sha256;
    let nonce_digest_valid = receipt.nonce_sha256 == sha256_bytes(policy.policy.nonce.as_bytes());
    let event_verification = receipt
        .event_log
        .as_ref()
        .ok_or_else(|| "terminal receipt has no event log".to_owned())
        .and_then(|event_log| verify_event_artifact(receipt_path, event_log));
    let event_log_valid = event_verification.is_ok();
    let derived_summary_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.derived_images == receipt.derived_image_summary);
    let accounting_valid = event_verification.as_ref().is_ok_and(|verified| {
        verified.accounting.total_processes == receipt.accounting.total_processes
            && verified.accounting.active_processes == receipt.accounting.active_processes
            && verified.accounting.observed_process_creates
                == receipt.accounting.observed_process_creates
            && verified.accounting.observed_process_exits
                == receipt.accounting.observed_process_exits
            && verified.accounting.observed_execs == receipt.accounting.observed_execs
            && verified.accounting.root_execs == receipt.accounting.root_execs
    });
    println!(
        "{}",
        serde_json::json!({
            "schema": receipt.schema,
            "state": receipt.state,
            "complete": receipt.complete,
            "schema_valid": schema_valid,
            "identity_valid": identity_valid,
            "terminal_consistent": terminal_consistent,
            "lifecycle_valid": lifecycle_valid,
            "policy_digest_valid": policy_digest_valid,
            "nonce_digest_valid": nonce_digest_valid,
            "receipt_size_valid": receipt_size_valid,
            "event_log_valid": event_log_valid,
            "derived_summary_valid": derived_summary_valid,
            "accounting_valid": accounting_valid,
            "event_log_error": event_verification.err(),
        })
    );
    Ok(
        if schema_valid
            && identity_valid
            && terminal_consistent
            && lifecycle_valid
            && policy_digest_valid
            && nonce_digest_valid
            && receipt_size_valid
            && event_log_valid
            && derived_summary_valid
            && accounting_valid
        {
            0
        } else {
            79
        },
    )
}

fn parse_mode(value: &str) -> Result<ClosureMode, String> {
    match value {
        "leaf" => Ok(ClosureMode::Leaf),
        "declared-tree" => Ok(ClosureMode::DeclaredTree),
        _ => Err("mode must be leaf or declared-tree".to_owned()),
    }
}

fn run_policy(policy_path: &Path, receipt_path: &Path) -> Result<u8, String> {
    let bytes = fs::read(policy_path)
        .map_err(|error| format!("cannot read policy {}: {error}", policy_path.display()))?;
    let raw: Policy =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid policy: {error}"))?;
    let policy = raw.validate()?;
    let mut events = EventJournal::create(receipt_path)?;
    let mut receipt = platform::run(&policy, &mut events);
    let evidence = events.publish()?;
    receipt.attach_evidence(evidence);
    write_receipt_atomic(receipt_path, &receipt)?;
    Ok(if receipt.complete { 0 } else { 78 })
}

fn write_receipt_atomic(path: &Path, receipt: &Receipt) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(format!(
            "compact receipt is {} bytes; maximum is {MAX_RECEIPT_BYTES}",
            bytes.len()
        ));
    }
    durable_atomic_write(path, &bytes)
}
