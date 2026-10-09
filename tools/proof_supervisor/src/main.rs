use molt_proof_supervisor::evidence::{
    MAX_RECEIPT_BYTES, OpenedRegularFile, durable_atomic_write, event_artifact_path,
    verify_event_artifact,
};
use molt_proof_supervisor::{
    ClosureMode, EXPORT_EVENT_MAX_BYTES, EXPORT_FOOTER_MAGIC, EXPORT_LENGTH_HEX_DIGITS,
    EXPORT_RECEIPT_MAX_BYTES, EventJournal, MAX_POLICY_BYTES, Policy, RECEIPT_SCHEMA, Receipt,
    platform, sha256_bytes, sha256_reader,
};
use std::fs;
use std::io::Write;
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
            run_policy(Path::new(policy), Path::new(receipt), false)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "run-export" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            let result = run_policy(Path::new(policy), Path::new(receipt), false)?;
            export_evidence(Path::new(receipt))?;
            Ok(result)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "inventory"
                && policy_flag == "--policy"
                && receipt_flag == "--receipt" =>
        {
            run_policy(Path::new(policy), Path::new(receipt), true)
        }
        [command, policy_flag, policy, receipt_flag, receipt]
            if command == "verify" && policy_flag == "--policy" && receipt_flag == "--receipt" =>
        {
            verify_receipt(Path::new(policy), Path::new(receipt), None)
        }
        [command, root_flag, root, policy_flag, policy, receipt_flag, receipt]
            if command == "verify-rooted"
                && root_flag == "--rootfs"
                && policy_flag == "--policy"
                && receipt_flag == "--receipt" =>
        {
            verify_receipt(Path::new(policy), Path::new(receipt), Some(Path::new(root)))
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
        [command, fixture] if command == "fixture-child" && fixture == "export-lookalike" => {
            print!("guest stdout remains unchanged\n");
            eprint!("guest stderr{EXPORT_FOOTER_MAGIC}00000000000000000000000000000000\n");
            Ok(0)
        }
        #[cfg(unix)]
        [command, fixture, image, rest @ ..]
            if command == "fixture-child" && fixture == "exec-image" =>
        {
            use std::os::unix::process::CommandExt;

            // Replace this process image in place; a successful exec never
            // returns, so reaching the error is the only outcome here.
            let error = Command::new(image)
                .arg("fixture-child")
                .args(rest)
                .exec();
            Err(format!("fixture exec-image failed: {error}"))
        }
        [command, fixture, auxiliary]
            if command == "fixture-child" && fixture == "spawn-and-wait" =>
        {
            let status = Command::new(auxiliary)
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
        #[cfg(windows)]
        [command, fixture] if command == "fixture-child" && fixture == "normal-heap-leaf" => {
            normal_heap_fixture()?;
            Ok(0)
        }
        #[cfg(windows)]
        [command, fixture] if command == "fixture-child" && fixture == "normal-heap-tree" => {
            normal_heap_fixture()?;
            let status = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
                .args(["fixture-child", "normal-heap-leaf"])
                .status()
                .map_err(|error| error.to_string())?;
            if !status.success() {
                return Err(format!("normal-heap descendant failed: {status}"));
            }
            Ok(0)
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
        _ => Err("usage: capability <leaf|declared-tree|inventory-tree> | run --policy FILE --receipt FILE | run-export --policy FILE --receipt FILE | inventory --policy FILE --receipt FILE | verify --policy FILE --receipt FILE | verify-rooted --rootfs DIR --policy FILE --receipt FILE".to_owned()),
    }
}

#[cfg(windows)]
fn normal_heap_fixture() -> Result<(), String> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Diagnostics::Debug::IsDebuggerPresent;
    use windows_sys::Win32::System::Memory::{
        HeapCompatibilityInformation, HeapCreate, HeapDestroy, HeapQueryInformation,
        HeapSetInformation,
    };

    struct Heap(HANDLE);
    impl Drop for Heap {
        fn drop(&mut self) {
            unsafe { HeapDestroy(self.0) };
        }
    }

    if unsafe { IsDebuggerPresent() } == 0 {
        return Err("normal-heap fixture requires active debugger custody".to_owned());
    }
    let handle = unsafe { HeapCreate(0, 0, 0) };
    if handle.is_null() {
        return Err(format!(
            "HeapCreate failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let heap = Heap(handle);
    let compatibility = 2_u32;
    if unsafe {
        HeapSetInformation(
            heap.0,
            HeapCompatibilityInformation,
            (&compatibility as *const u32).cast(),
            std::mem::size_of_val(&compatibility),
        )
    } == 0
    {
        return Err(format!(
            "cannot enable LFH under debugger custody: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut observed = 0_u32;
    if unsafe {
        HeapQueryInformation(
            heap.0,
            HeapCompatibilityInformation,
            (&mut observed as *mut u32).cast(),
            std::mem::size_of_val(&observed),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(format!(
            "cannot query LFH: {}",
            std::io::Error::last_os_error()
        ));
    }
    if observed != compatibility || unsafe { IsDebuggerPresent() } == 0 {
        return Err("normal LFH and debugger custody must remain active together".to_owned());
    }
    Ok(())
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

fn read_policy(path: &Path) -> Result<Vec<u8>, String> {
    let opened = OpenedRegularFile::open(path)
        .map_err(|error| format!("cannot open policy {}: {error}", path.display()))?;
    if opened.size_bytes() > MAX_POLICY_BYTES as u64 {
        return Err(format!(
            "policy must be a regular file within {MAX_POLICY_BYTES} bytes"
        ));
    }
    opened
        .read_all(MAX_POLICY_BYTES)
        .map_err(|error| format!("cannot read policy {}: {error}", path.display()))
}

fn verify_receipt(
    policy_path: &Path,
    receipt_path: &Path,
    rootfs: Option<&Path>,
) -> Result<u8, String> {
    let policy_bytes = read_policy(policy_path)?;
    let raw_policy: Policy = serde_json::from_slice(&policy_bytes)
        .map_err(|error| format!("invalid policy: {error}"))?;
    let policy = match rootfs {
        Some(root) => raw_policy.validate_rooted_linux(root)?,
        None => raw_policy.validate()?,
    };
    let opened_receipt = OpenedRegularFile::open(receipt_path)
        .map_err(|error| format!("cannot open receipt {}: {error}", receipt_path.display()))?;
    let receipt_bytes = opened_receipt.size_bytes();
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
    let bytes = opened_receipt
        .read_all(MAX_RECEIPT_BYTES)
        .map_err(|error| format!("cannot read receipt {}: {error}", receipt_path.display()))?;
    let receipt_size_valid = true;
    let receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid receipt: {error}"))?;
    let identity_valid = receipt.identity_is_valid();
    let terminal_consistent = receipt.terminal_is_consistent();
    let lifecycle_valid = receipt.lifecycle_is_valid();
    let schema_valid = receipt.schema == RECEIPT_SCHEMA;
    let capability_valid = match rootfs {
        Some(_) => platform::recorded_linux_capability_contract_is_valid(
            &receipt.capability,
            policy.policy.mode,
        ),
        None => platform::capability_contract_is_valid(&receipt.capability, policy.policy.mode),
    };
    let policy_digest_valid = receipt.policy_sha256 == policy.policy_sha256;
    let nonce_digest_valid = receipt.nonce_sha256 == sha256_bytes(policy.policy.nonce.as_bytes());
    let event_verification = receipt
        .event_log
        .as_ref()
        .ok_or_else(|| "terminal receipt has no event log".to_owned())
        .and_then(|event_log| {
            verify_event_artifact(receipt_path, event_log, &policy, &receipt.capability)
        });
    let event_log_valid = event_verification.is_ok();
    let derived_summary_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.derived_images == receipt.derived_image_summary);
    let accounting_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.accounting == receipt.accounting);
    let root_exit_valid = event_verification
        .as_ref()
        .is_ok_and(|verified| verified.root_exit_code == receipt.root_exit_code);
    let violation_replay_valid = event_verification.as_ref().is_ok_and(|verified| {
        verified.violation_count == receipt.violation_count
            && verified.violations == receipt.violations
    });
    let kernel_accounting_valid = receipt.kernel_accounting_is_valid();
    println!(
        "{}",
        serde_json::json!({
            "receipt_sha256": sha256_bytes(&bytes),
            "receipt_bytes": bytes.len(),
            "policy_input_sha256": sha256_bytes(&policy_bytes),
            "policy_input_bytes": policy_bytes.len(),
            "capability": receipt.capability,
            "schema": receipt.schema,
            "state": receipt.state,
            "complete": receipt.complete,
            "schema_valid": schema_valid,
            "capability_valid": capability_valid,
            "identity_valid": identity_valid,
            "terminal_consistent": terminal_consistent,
            "lifecycle_valid": lifecycle_valid,
            "policy_digest_valid": policy_digest_valid,
            "nonce_digest_valid": nonce_digest_valid,
            "receipt_size_valid": receipt_size_valid,
            "event_log_valid": event_log_valid,
            "derived_summary_valid": derived_summary_valid,
            "accounting_valid": accounting_valid,
            "root_exit_valid": root_exit_valid,
            "violation_replay_valid": violation_replay_valid,
            "kernel_accounting_valid": kernel_accounting_valid,
            "event_log_error": event_verification.err(),
        })
    );
    Ok(
        if schema_valid
            && capability_valid
            && identity_valid
            && terminal_consistent
            && lifecycle_valid
            && policy_digest_valid
            && nonce_digest_valid
            && receipt_size_valid
            && event_log_valid
            && derived_summary_valid
            && accounting_valid
            && root_exit_valid
            && violation_replay_valid
            && kernel_accounting_valid
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
        "inventory-tree" => Ok(ClosureMode::InventoryTree),
        _ => Err("mode must be leaf, declared-tree, or inventory-tree".to_owned()),
    }
}

fn run_policy(policy_path: &Path, receipt_path: &Path, inventory: bool) -> Result<u8, String> {
    let bytes = read_policy(policy_path)?;
    let raw: Policy =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid policy: {error}"))?;
    let policy = raw.validate()?;
    if (policy.policy.mode == ClosureMode::InventoryTree) != inventory {
        return Err(if inventory {
            "inventory command requires inventory-tree policy mode".to_owned()
        } else {
            "inventory-tree policy must use the inventory command".to_owned()
        });
    }
    let capability = platform::capability(policy.policy.mode);
    let mut events = EventJournal::create(receipt_path, &policy, &capability)?;
    let mut receipt = platform::run(&policy, &mut events, capability);
    let evidence = events.publish()?;
    receipt.attach_evidence(evidence)?;
    write_receipt_atomic(receipt_path, &receipt)?;
    Ok(if receipt.complete { 0 } else { 78 })
}

/// Export exact retained evidence after the supervised tree is terminal.
/// Only the final fixed-width footer at stderr EOF frames this transport;
/// guest stderr before it is ordinary guest output. Verification is unchanged.
fn export_evidence(receipt_path: &Path) -> Result<(), String> {
    let stderr = std::io::stderr();
    export_evidence_to(receipt_path, &mut stderr.lock())
}

fn export_evidence_to(receipt_path: &Path, stream: &mut impl Write) -> Result<(), String> {
    let opened_receipt = OpenedRegularFile::open(receipt_path)
        .map_err(|error| format!("cannot open export receipt: {error}"))?;
    if opened_receipt.size_bytes() > EXPORT_RECEIPT_MAX_BYTES as u64 {
        return Err("export receipt exceeds its protocol bound".to_owned());
    }
    let receipt_bytes = opened_receipt
        .read_all(EXPORT_RECEIPT_MAX_BYTES)
        .map_err(|error| format!("cannot read export receipt: {error}"))?;
    let receipt: Receipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("invalid export receipt: {error}"))?;
    if receipt.schema != RECEIPT_SCHEMA || !receipt.identity_is_valid() {
        return Err("export receipt identity is invalid".to_owned());
    }
    let event = receipt
        .event_log
        .as_ref()
        .ok_or_else(|| "export receipt has no event artifact".to_owned())?;
    if event.bytes > EXPORT_EVENT_MAX_BYTES {
        return Err("export event artifact exceeds its protocol bound".to_owned());
    }
    let path = event_artifact_path(receipt_path, &event.sha256)?;
    if path.file_name().and_then(|name| name.to_str()) != Some(event.file.as_str()) {
        return Err("export event artifact is not the deterministic adjacent file".to_owned());
    }
    let opened = OpenedRegularFile::open(&path)
        .map_err(|error| format!("cannot open export event artifact: {error}"))?;
    if opened.size_bytes() != event.bytes {
        return Err("export event artifact type or size changed".to_owned());
    }
    if sha256_reader(&mut opened.bounded_reader()).map_err(|error| error.to_string())?
        != event.sha256
    {
        return Err("export event artifact digest changed".to_owned());
    }
    opened.verify().map_err(|error| error.to_string())?;
    opened.rewind().map_err(|error| error.to_string())?;
    stream
        .write_all(&receipt_bytes)
        .map_err(|error| error.to_string())?;
    let copied = std::io::copy(&mut opened.bounded_reader(), &mut *stream)
        .map_err(|error| format!("cannot export event artifact: {error}"))?;
    if copied != event.bytes {
        return Err("export event artifact changed while streaming".to_owned());
    }
    opened.verify().map_err(|error| error.to_string())?;
    opened_receipt.verify().map_err(|error| error.to_string())?;
    stream
        .write_all(EXPORT_FOOTER_MAGIC.as_bytes())
        .map_err(|error| error.to_string())?;
    writeln!(
        stream,
        "{:0width$x}{:0width$x}",
        receipt_bytes.len(),
        event.bytes,
        width = EXPORT_LENGTH_HEX_DIGITS
    )
    .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
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

#[cfg(test)]
mod export_tests {
    use super::*;
    use molt_proof_supervisor::{ArtifactSummary, Capability, FixedImage, RootExitDisposition};

    #[test]
    fn export_refuses_oversized_retained_evidence_before_emitting_a_footer() {
        let directory = std::env::temp_dir().join(format!(
            "molt-supervisor-export-bound-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        struct RemoveDirectory(std::path::PathBuf);
        impl Drop for RemoveDirectory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        fs::create_dir(&directory).unwrap();
        let _cleanup = RemoveDirectory(directory.clone());
        let receipt_path = directory.join("receipt.json");
        fs::write(&receipt_path, vec![b'x'; EXPORT_RECEIPT_MAX_BYTES + 1]).unwrap();
        assert!(
            export_evidence(&receipt_path)
                .unwrap_err()
                .contains("receipt exceeds")
        );
        let image = std::env::current_exe().unwrap();
        let policy = Policy {
            schema: molt_proof_supervisor::POLICY_SCHEMA.to_owned(),
            nonce: "e".repeat(32),
            mode: ClosureMode::Leaf,
            cwd: directory.clone(),
            command: vec![image.to_string_lossy().into_owned()],
            environment: platform::required_environment(),
            root_role: "fixture".to_owned(),
            fixed_images: vec![FixedImage {
                role: "fixture".to_owned(),
                path: image.clone(),
                sha256: molt_proof_supervisor::sha256_file(&image).unwrap(),
                root_exit_disposition: RootExitDisposition::RequireExit,
            }],
            derived_roots: vec![],
        }
        .validate()
        .unwrap();
        let capability = Capability {
            schema: molt_proof_supervisor::CAPABILITY_SCHEMA.to_owned(),
            platform: "test".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "test".to_owned(),
            available: false,
            pre_entry_exec_authority: false,
            pre_entry_process_create_authority: false,
            recursive_descendant_authority: false,
            required_environment: platform::required_environment(),
            reason: Some("fixture".to_owned()),
        };
        let mut receipt = Receipt::rejected(&policy, &capability, "fixture");
        receipt.event_log = Some(ArtifactSummary {
            schema: molt_proof_supervisor::evidence::EVENT_LOG_SCHEMA.to_owned(),
            file: "not-opened.jsonl".to_owned(),
            count: 1,
            bytes: EXPORT_EVENT_MAX_BYTES + 1,
            sha256: "0".repeat(64),
        });
        receipt.seal();
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(
            export_evidence(&receipt_path)
                .unwrap_err()
                .contains("event artifact exceeds")
        );

        // Transport accepts sealed bytes; event semantics belong to verify.
        let event_bytes = b"sealed event bytes\n";
        let digest = sha256_bytes(event_bytes);
        let event_path = event_artifact_path(&receipt_path, &digest).unwrap();
        fs::write(&event_path, event_bytes).unwrap();
        receipt.event_log = Some(ArtifactSummary {
            schema: molt_proof_supervisor::evidence::EVENT_LOG_SCHEMA.to_owned(),
            file: event_path.file_name().unwrap().to_str().unwrap().to_owned(),
            count: 1,
            bytes: event_bytes.len() as u64,
            sha256: digest,
        });
        receipt.seal();
        fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let mut accepted = Vec::new();
        export_evidence_to(&receipt_path, &mut accepted).unwrap();
        assert!(
            accepted
                .windows(EXPORT_FOOTER_MAGIC.len())
                .any(|row| row == EXPORT_FOOTER_MAGIC.as_bytes())
        );
        #[cfg(unix)]
        for path in [&receipt_path, &event_path] {
            let saved = path.with_extension("saved");
            fs::rename(path, &saved).unwrap();
            std::os::unix::fs::symlink(&saved, path).unwrap();
            let mut refused = Vec::new();
            assert!(
                export_evidence_to(&receipt_path, &mut refused)
                    .unwrap_err()
                    .contains("direct regular file")
            );
            assert!(
                refused.is_empty(),
                "indirection refusal must precede any export bytes"
            );
            fs::remove_file(path).unwrap();
            fs::rename(&saved, path).unwrap();
        }
        struct MutatingSink {
            event_path: std::path::PathBuf,
            bytes: Vec<u8>,
            mutated: bool,
        }
        impl Write for MutatingSink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.mutated {
                    // Same extent changes between digest and streaming, after
                    // receipt bytes are emitted but before a footer is possible.
                    fs::write(&self.event_path, b"changed event data\n")?;
                    self.mutated = true;
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(event_bytes.len(), b"changed event data\n".len());
        let mut sink = MutatingSink {
            event_path,
            bytes: Vec::new(),
            mutated: false,
        };
        assert!(export_evidence_to(&receipt_path, &mut sink).is_err());
        assert!(sink.mutated);
        assert!(
            !sink
                .bytes
                .windows(EXPORT_FOOTER_MAGIC.len())
                .any(|row| row == EXPORT_FOOTER_MAGIC.as_bytes())
        );
    }
}
