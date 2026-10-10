// Actual admitted-run controls: macOS has only the explicit refusal contract.
#![cfg(any(target_os = "windows", target_os = "linux"))]

use molt_proof_supervisor::evidence::{durable_atomic_write, event_artifact_path};
use molt_proof_supervisor::{
    Admission, ClosureMode, DerivedRoot, FixedImage, KernelAccounting, MAX_POLICY_BYTES,
    POLICY_SCHEMA, Policy, Receipt, RootExitDisposition, SupervisorState, platform, sha256_bytes,
    sha256_file,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestRun {
    directory: PathBuf,
    binary: PathBuf,
    policy_path: PathBuf,
    receipt_path: PathBuf,
    policy: Policy,
}

impl Drop for TestRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn verify_reports_the_exact_raw_receipt_bytes_including_whitespace_and_unicode() {
    let run = run_fixture(ClosureMode::Leaf);
    let mut receipt: Receipt =
        serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    // An authentic incomplete receipt still verifies successfully. Its
    // diagnostics give this raw-byte oracle meaningful non-ASCII material.
    receipt.complete = false;
    receipt.state = SupervisorState::Incomplete;
    *receipt.lifecycle.last_mut().unwrap() = SupervisorState::Incomplete;
    receipt.record_error("storage interruption: café 🦀");
    receipt.seal();
    let compact = serde_json::to_vec(&receipt).unwrap();
    let pretty = serde_json::to_vec_pretty(&receipt).unwrap();
    let variants = [
        [b" \n\t".as_slice(), compact.as_slice(), b"\n ".as_slice()].concat(),
        [b"\n".as_slice(), pretty.as_slice(), b"\t\r\n".as_slice()].concat(),
    ];
    let mut previous_digest = None;
    for bytes in variants {
        assert!(std::str::from_utf8(&bytes).unwrap().contains("café 🦀"));
        durable_atomic_write(&run.receipt_path, &bytes).unwrap();
        let actual = fs::read(&run.receipt_path).unwrap();
        assert_eq!(actual, bytes);
        let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
        assert!(output.status.success(), "{}", text(&output));
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        let digest = Sha256::digest(&actual)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(response["receipt_sha256"], digest);
        assert_eq!(response["receipt_bytes"], actual.len() as u64);
        assert_eq!(response["complete"], false);
        assert_eq!(response["identity_valid"], true);
        assert_ne!(
            digest,
            Sha256::digest(&compact)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        if let Some(previous) = previous_digest.replace(digest.clone()) {
            assert_ne!(
                previous, digest,
                "semantic reserialization loses raw-byte custody"
            );
        }
    }
}

#[test]
fn receipt_publication_failure_retains_actual_terminal_evidence_and_destination() {
    let run = prepare_fixture(ClosureMode::Leaf, &["exit", "42"]);
    fs::create_dir(&run.receipt_path).unwrap();
    let sentinel = run.receipt_path.join("existing-destination");
    fs::write(&sentinel, b"preserve").unwrap();
    let output = execute(&run);
    let diagnostic = publication_snapshot(&output);
    assert_eq!(diagnostic.root_exit_code, Some(42));
    assert_eq!(diagnostic.accounting.process_creates, 1);
    assert_eq!(diagnostic.accounting.process_exits, 1);
    assert_eq!(diagnostic.accounting.active_processes, 0);
    assert!(matches!(
        diagnostic.capability.admission,
        Admission::Admitted { .. }
    ));
    assert!(diagnostic.identity_is_valid());
    let event_log = diagnostic.event_log.unwrap();
    let published = fs::read(run.directory.join(event_log.file)).unwrap();
    assert_eq!(
        event_log.sha256,
        Sha256::digest(&published)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    assert_eq!(event_log.bytes, published.len() as u64);
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve");
    assert!(run.receipt_path.is_dir());
}

#[cfg(target_os = "linux")]
#[test]
fn publication_failure_preserves_original_admission_error_and_actual_cleanup_waits() {
    let mut run = prepare_fixture(ClosureMode::DeclaredTree, &["exit", "0"]);
    let marker = run.directory.join("unadmitted-child-entered");
    let report = run.directory.join("descendant-entered.json");
    run.policy.command = vec![
        run.binary.display().to_string(),
        "fixture-child".to_owned(),
        "linux-creation-descendant".to_owned(),
        "untraced".to_owned(),
        marker.display().to_string(),
        report.display().to_string(),
    ];
    fs::write(&run.policy_path, serde_json::to_vec(&run.policy).unwrap()).unwrap();
    fs::create_dir(&run.receipt_path).unwrap();
    let sentinel = run.receipt_path.join("existing-destination");
    fs::write(&sentinel, b"preserve").unwrap();
    // The existing independent kernel fixture denies pidfd_open in the
    // supervisor. The subject fork really occurs, but its child cannot enter.
    // A real directory at the receipt destination then denies final rename.
    let output = Command::new(&run.binary)
        .args(["fixture-child", "linux-host-denial", "pidfd-open"])
        .arg(&run.policy_path)
        .arg(&run.receipt_path)
        .output()
        .unwrap();
    let diagnostic = publication_snapshot(&output);
    assert!(!diagnostic.complete);
    assert_eq!(diagnostic.accounting.process_creates, 1);
    // The accepted journal stops before the failed descendant admission.
    // Actual cleanup waits belong to native custody, never invented exit rows.
    assert_eq!(diagnostic.accounting.process_exits, 0);
    assert_eq!(diagnostic.accounting.active_processes, 1);
    assert_eq!(diagnostic.root_exit_code, None);
    assert!(matches!(&diagnostic.journal_coverage,
        molt_proof_supervisor::JournalCoverage::Prefix {
            stage: molt_proof_supervisor::CaptureStage::NativeObservation,
            accepted_records: 2, next_sequence: 3, cause, ..
        } if cause.contains("pidfd_open") && cause.contains("13")));
    assert!(matches!(diagnostic.native_custody,
        molt_proof_supervisor::NativeCustody::Linux {
            remaining_tasks: 0, remaining_processes: 0, wait_exhausted: true,
            root_exit_code: Some(code),
        } if code == 128 + i64::from(libc::SIGKILL)));
    assert!(diagnostic.native_custody_is_valid());
    assert!(diagnostic.native_custody.is_closed());
    assert!(
        diagnostic
            .errors
            .iter()
            .any(|error| error.contains("pidfd_open") && error.contains("13")),
        "{diagnostic:#?}"
    );
    assert!(
        diagnostic
            .errors
            .iter()
            .any(|error| error.contains("cleanup terminal waits")),
        "{diagnostic:#?}"
    );
    assert!(!marker.exists());
    assert!(!report.exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve");
}

fn publication_snapshot(output: &Output) -> Receipt {
    assert_eq!(output.status.code(), Some(2), "{}", text(output));
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.contains("terminal publication not acknowledged:"),
        "{stderr}"
    );
    assert!(stderr.contains("cannot publish evidence"), "{stderr}");
    let (_, snapshot) = stderr
        .split_once("terminal receipt snapshot (diagnostic only): ")
        .unwrap();
    serde_json::from_str(snapshot.trim()).unwrap()
}

#[test]
fn verify_binds_every_canonical_policy_dimension() {
    let mode = ClosureMode::DeclaredTree;
    let run = run_fixture(mode);
    assert!(
        verify(&run.binary, &run.policy_path, &run.receipt_path)
            .status
            .success()
    );

    let alternate_cwd = run.directory.join("alternate-cwd");
    let derived_root = run.directory.join("derived");
    fs::create_dir_all(&alternate_cwd).unwrap();
    fs::create_dir_all(&derived_root).unwrap();
    let mut variants = Vec::new();

    let mut argv = run.policy.clone();
    argv.command.push("different-argument".to_owned());
    variants.push(argv);

    let mut cwd = run.policy.clone();
    cwd.cwd = alternate_cwd;
    variants.push(cwd);

    let mut environment = run.policy.clone();
    environment
        .environment
        .insert("MOLT_POLICY_BINDING".to_owned(), "different".to_owned());
    variants.push(environment);

    let mut role = run.policy.clone();
    role.root_role = "different-root-role".to_owned();
    role.fixed_images[0].role = role.root_role.clone();
    variants.push(role);

    if mode == ClosureMode::DeclaredTree {
        let mut derived = run.policy.clone();
        derived.derived_roots.push(DerivedRoot {
            role: "generated-tool".to_owned(),
            path: derived_root,
        });
        variants.push(derived);
    }

    for (index, policy) in variants.into_iter().enumerate() {
        let path = run.directory.join(format!("substitute-{index}.json"));
        fs::write(&path, serde_json::to_vec(&policy).unwrap()).unwrap();
        let output = verify(&run.binary, &path, &run.receipt_path);
        assert_eq!(output.status.code(), Some(79), "{}", text(&output));
        assert!(text(&output).contains("\"policy_digest_valid\":false"));
    }
}

#[test]
fn verify_rejects_unknown_policy_and_receipt_fields() {
    let run = run_fixture(ClosureMode::Leaf);
    let mut policy: Value = serde_json::from_slice(&fs::read(&run.policy_path).unwrap()).unwrap();
    policy["unknown_policy_authority"] = Value::Bool(true);
    let unknown_policy = run.directory.join("unknown-policy.json");
    fs::write(&unknown_policy, serde_json::to_vec(&policy).unwrap()).unwrap();
    let output = verify(&run.binary, &unknown_policy, &run.receipt_path);
    assert!(!output.status.success());
    assert!(text(&output).contains("unknown field"));

    let mut receipt: Value = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    receipt["unknown_receipt_authority"] = Value::Bool(true);
    durable_atomic_write(
        &run.receipt_path,
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
    assert!(!output.status.success());
    assert!(text(&output).contains("unknown field"));

    if receipt["native_custody"]["job"].is_object() {
        receipt
            .as_object_mut()
            .unwrap()
            .remove("unknown_receipt_authority");
        let mut nested = receipt;
        nested["native_custody"]["job"]["unknown_kernel_authority"] = Value::Bool(true);
        durable_atomic_write(
            &run.receipt_path,
            &serde_json::to_vec_pretty(&nested).unwrap(),
        )
        .unwrap();
        let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
        assert!(!output.status.success());
        assert!(text(&output).contains("unknown field"));
    }
}

#[test]
fn verify_replays_every_lifecycle_transition_even_after_reseal() {
    let run = run_fixture(ClosureMode::Leaf);
    let original: Receipt = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let illegal = [
        vec![
            molt_proof_supervisor::SupervisorState::Created,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Complete,
        ],
        vec![
            molt_proof_supervisor::SupervisorState::Created,
            molt_proof_supervisor::SupervisorState::PolicySealed,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Draining,
            molt_proof_supervisor::SupervisorState::Complete,
        ],
        vec![
            molt_proof_supervisor::SupervisorState::Created,
            molt_proof_supervisor::SupervisorState::PolicySealed,
            molt_proof_supervisor::SupervisorState::Running,
            molt_proof_supervisor::SupervisorState::Complete,
            molt_proof_supervisor::SupervisorState::Draining,
        ],
    ];
    for lifecycle in illegal {
        let mut receipt = original.clone();
        receipt.lifecycle = lifecycle;
        receipt.seal();
        durable_atomic_write(
            &run.receipt_path,
            &serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
        assert_eq!(output.status.code(), Some(79), "{}", text(&output));
        assert!(text(&output).contains("\"lifecycle_valid\":false"));
    }
}

#[test]
fn verify_rejects_unknown_event_field_even_with_recomputed_artifact_and_receipt_digests() {
    let run = run_fixture(ClosureMode::Leaf);
    let mut receipt: Receipt =
        serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let old_log = receipt.event_log.as_ref().unwrap();
    let old_path = run.directory.join(&old_log.file);
    let old_bytes = fs::read(&old_path).unwrap();
    let newline = old_bytes.iter().position(|byte| *byte == b'\n').unwrap();
    let mut first: Value = serde_json::from_slice(&old_bytes[..newline]).unwrap();
    first["unknown_event_authority"] = Value::Bool(true);
    let mut changed = serde_json::to_vec(&first).unwrap();
    changed.push(b'\n');
    changed.extend_from_slice(&old_bytes[newline + 1..]);
    let digest = sha256_bytes(&changed);
    let changed_path = event_artifact_path(&run.receipt_path, &digest).unwrap();
    durable_atomic_write(&changed_path, &changed).unwrap();

    let event_log = receipt.event_log.as_mut().unwrap();
    event_log.file = changed_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    event_log.bytes = changed.len() as u64;
    event_log.sha256 = digest;
    receipt.seal();
    durable_atomic_write(
        &run.receipt_path,
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();

    let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
    assert_eq!(output.status.code(), Some(79), "{}", text(&output));
    assert!(text(&output).contains("unknown field"));
}

#[test]
fn verify_rejects_resealed_receipt_semantic_drift() {
    let run = run_fixture(ClosureMode::Leaf);
    let original: Receipt = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();

    let mut obsolete = original.clone();
    obsolete.schema = "molt.proof-process-closure-receipt.v4".to_owned();
    assert_resealed_receipt_rejected(&run, obsolete, "\"schema_valid\":false");
    let mut obsolete = original.clone();
    obsolete.capability.schema = "molt.proof-supervisor-capability.v3".to_owned();
    assert_resealed_receipt_rejected(&run, obsolete, "\"capability_valid\":false");

    let mut capability = original.clone();
    capability.capability.backend.push_str("-forged");
    assert_resealed_receipt_rejected(&run, capability, "\"capability_valid\":false");

    let mut eligibility = original.clone();
    eligibility.capability.admission = Admission::Eligible {};
    assert_resealed_receipt_rejected(&run, eligibility, "\"admission_replay_valid\":false");
    for coordinate in ["root", "create", "image"] {
        let mut witness = original.clone();
        let Admission::Admitted {
            root_stable_process_id,
            root_create_sequence,
            initial_image_sequence,
        } = &mut witness.capability.admission
        else {
            panic!("complete run must be admitted");
        };
        match coordinate {
            "root" => root_stable_process_id.push_str("-other-generation"),
            "create" => *root_create_sequence += 1,
            "image" => *initial_image_sequence += 1,
            _ => unreachable!(),
        }
        assert_resealed_receipt_rejected(&run, witness, "\"admission_replay_valid\":false");
    }

    let mut root_exit = original.clone();
    root_exit.root_exit_code = root_exit.root_exit_code.map(|code| code + 1);
    assert_resealed_receipt_rejected(&run, root_exit, "\"root_exit_valid\":false");

    let mut accounting = original.clone();
    accounting.accounting.execs += 1;
    assert_resealed_receipt_rejected(&run, accounting, "\"accounting_valid\":false");

    let mut violations = original.clone();
    violations.violation_count += 1;
    violations.violations.push("forged violation".to_owned());
    assert_resealed_receipt_rejected(&run, violations, "\"violation_replay_valid\":false");

    if let molt_proof_supervisor::NativeCustody::Windows { job: Some(_), .. } =
        &original.native_custody
    {
        let mut kernel = original.clone();
        let molt_proof_supervisor::NativeCustody::Windows {
            job:
                Some(KernelAccounting::WindowsJob {
                    total_processes, ..
                }),
            ..
        } = &mut kernel.native_custody
        else {
            unreachable!()
        };
        *total_processes += 1;
        assert_resealed_receipt_rejected(&run, kernel, "\"native_custody_valid\":false");
    }
}

#[test]
fn verify_replays_contiguous_typed_policy_classified_events_after_reseal() {
    let run = run_fixture(ClosureMode::Leaf);
    let receipt: Receipt = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let event_log = receipt.event_log.as_ref().unwrap();
    let event_path = run.directory.join(&event_log.file);
    let rows: Vec<Value> = fs::read_to_string(&event_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    let mut wrong_dialect = rows.clone();
    wrong_dialect[1]["event"]["kind"] = Value::from(if cfg!(target_os = "windows") {
        "exec"
    } else {
        "initial-image"
    });
    assert_resealed_event_log_rejected(&run, &receipt, &wrong_dialect, "backend dialect");

    let mut sequence_gap = rows.clone();
    sequence_gap[1]["sequence"] = Value::from(3);
    assert_resealed_event_log_rejected(&run, &receipt, &sequence_gap, "not contiguous");

    let mut post_root_activity = rows.clone();
    let mut late_activity = if cfg!(target_os = "windows") {
        post_root_activity[0].clone()
    } else {
        post_root_activity
            .iter()
            .find(|row| row["event"]["kind"] == "exec")
            .unwrap()
            .clone()
    };
    late_activity["sequence"] = Value::from((post_root_activity.len() + 1) as u64);
    post_root_activity.push(late_activity);
    assert_resealed_event_log_rejected(
        &run,
        &receipt,
        &post_root_activity,
        "after the root exited",
    );

    let mut forged_classification = rows.clone();
    let image_event = forged_classification
        .iter_mut()
        .find(|row| row["event"]["image"].is_object())
        .unwrap();
    image_event["event"]["image"]["roles"] = serde_json::json!(["forged-policy-authority"]);
    assert_resealed_event_log_rejected(
        &run,
        &receipt,
        &forged_classification,
        "classification disagrees",
    );

    let mut impossible_variant = rows;
    impossible_variant[0]["event"]["exit_code"] = Value::from(0);
    assert_resealed_event_log_rejected(&run, &receipt, &impossible_variant, "unknown field");
}

fn run_fixture(mode: ClosureMode) -> TestRun {
    let run = prepare_fixture(mode, &["exit", "0"]);
    let output = execute(&run);
    assert!(output.status.success(), "{}", text(&output));
    run
}

fn prepare_fixture(mode: ClosureMode, fixture_args: &[&str]) -> TestRun {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let policy_path = directory.join("policy.json");
    let receipt_path = directory.join("receipt.json");
    let policy = Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: format!(
            "{:032x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        mode,
        cwd: std::env::current_dir().unwrap(),
        command: [
            vec![binary.display().to_string(), "fixture-child".to_owned()],
            fixture_args.iter().map(|arg| (*arg).to_owned()).collect(),
        ]
        .concat(),
        environment: platform::required_environment(),
        root_role: "fixture".to_owned(),
        fixed_images: vec![FixedImage {
            role: "fixture".to_owned(),
            path: binary.clone(),
            sha256: sha256_file(&binary).unwrap(),
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: vec![],
    };
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    TestRun {
        directory,
        binary,
        policy_path,
        receipt_path,
        policy,
    }
}

fn execute(run: &TestRun) -> Output {
    Command::new(&run.binary)
        .args(["run", "--policy"])
        .arg(&run.policy_path)
        .arg("--receipt")
        .arg(&run.receipt_path)
        .output()
        .unwrap()
}

fn verify(binary: &Path, policy: &Path, receipt: &Path) -> Output {
    let result = Command::new(binary)
        .args(["verify", "--policy"])
        .arg(policy)
        .arg("--receipt")
        .arg(receipt)
        .output()
        .unwrap();
    if result.status.success() {
        assert_consumed_input_binding(&result, policy, receipt);
    }
    result
}

fn assert_consumed_input_binding(output: &Output, policy: &Path, receipt: &Path) {
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let policy_bytes = fs::read(policy).unwrap();
    let receipt_bytes = fs::read(receipt).unwrap();
    assert_eq!(result["receipt_sha256"], sha256_bytes(&receipt_bytes));
    assert_eq!(result["receipt_bytes"], receipt_bytes.len());
    assert_eq!(result["policy_input_sha256"], sha256_bytes(&policy_bytes));
    assert_eq!(result["policy_input_bytes"], policy_bytes.len());
}

fn assert_resealed_receipt_rejected(run: &TestRun, mut receipt: Receipt, expected: &str) {
    receipt.seal();
    durable_atomic_write(
        &run.receipt_path,
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
    assert_eq!(output.status.code(), Some(79), "{}", text(&output));
    assert!(text(&output).contains(expected), "{}", text(&output));
}

fn assert_resealed_event_log_rejected(
    run: &TestRun,
    original: &Receipt,
    rows: &[Value],
    expected_error: &str,
) {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).unwrap();
        bytes.push(b'\n');
    }
    let digest = sha256_bytes(&bytes);
    let path = event_artifact_path(&run.receipt_path, &digest).unwrap();
    durable_atomic_write(&path, &bytes).unwrap();
    let mut receipt = original.clone();
    let event_log = receipt.event_log.as_mut().unwrap();
    event_log.file = path.file_name().unwrap().to_string_lossy().into_owned();
    event_log.count = rows.len() as u64;
    event_log.bytes = bytes.len() as u64;
    event_log.sha256 = digest;
    receipt.seal();
    durable_atomic_write(
        &run.receipt_path,
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    let output = verify(&run.binary, &run.policy_path, &run.receipt_path);
    let output_text = text(&output);
    assert_eq!(output.status.code(), Some(79), "{output_text}");
    assert!(output_text.contains(expected_error), "{output_text}");
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn unique_directory() -> PathBuf {
    // Parallel tests can share one SystemTime tick (microsecond resolution on
    // macOS); the counter keeps every fixture directory distinct in-process.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "molt-proof-supervisor-verification-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

// Offline fixtures encode a literal Linux event sequence and terminal facts.
// They do not run a guest, inspect the host kernel, or call the event ledger
// to manufacture expected accounting. Container/root provenance is tested by
// the release receiver, separately from this retained-evidence verifier.
fn rooted_fixture() -> TestRun {
    let directory = unique_directory();
    let rootfs = directory.join("rootfs");
    fs::create_dir_all(rootfs.join("bin")).unwrap();
    fs::create_dir_all(rootfs.join("work")).unwrap();
    fs::write(rootfs.join("bin/guest"), b"retained Linux guest image").unwrap();
    let image_hash = sha256_bytes(b"retained Linux guest image");
    let policy = Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: "c".repeat(32),
        mode: ClosureMode::Leaf,
        cwd: PathBuf::from("/work"),
        command: vec!["/bin/guest".to_owned(), "literal argument".to_owned()],
        environment: Default::default(),
        root_role: "guest".to_owned(),
        fixed_images: vec![FixedImage {
            role: "guest".to_owned(),
            path: PathBuf::from("/bin/guest"),
            sha256: image_hash.clone(),
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: vec![],
    };
    let rows = [
        serde_json::json!({
            "sequence": 1, "process_id": 41, "stable_process_id": "41:fixture",
            "event": {"kind": "process-create", "parent_process_id": null}
        }),
        serde_json::json!({
            "sequence": 2, "process_id": 41, "stable_process_id": "41:fixture",
            "event": {"kind": "exec", "image": {
                "path": "/bin/guest", "file_id": "fixture-device:inode",
                "size_bytes": b"retained Linux guest image".len(), "sha256": image_hash,
                "class": "fixed", "roles": ["guest"]
            }}
        }),
        serde_json::json!({
            "sequence": 3, "process_id": 41, "stable_process_id": "41:fixture",
            "event": {"kind": "process-exit", "exit_code": 0}
        }),
    ];
    let mut events = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut events, &row).unwrap();
        events.push(b'\n');
    }
    let policy_path = directory.join("policy.json");
    let receipt_path = directory.join("receipt.json");
    let event_digest = sha256_bytes(&events);
    let event_path = event_artifact_path(&receipt_path, &event_digest).unwrap();
    fs::write(&event_path, &events).unwrap();
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    use molt_proof_supervisor::{
        Accounting, ArtifactSummary, CAPABILITY_SCHEMA, Capability, IdentitySummary,
        RECEIPT_SCHEMA, SupervisorState,
    };
    let mut receipt = Receipt {
        schema: RECEIPT_SCHEMA.to_owned(),
        capability: Capability {
            schema: CAPABILITY_SCHEMA.to_owned(),
            platform: "linux".to_owned(),
            mode: ClosureMode::Leaf,
            backend: "ptrace-exitkill".to_owned(),
            admission: Admission::Admitted {
                root_stable_process_id: "41:fixture".to_owned(),
                root_create_sequence: 1,
                initial_image_sequence: 2,
            },
            pre_entry_exec_authority: true,
            pre_entry_process_create_authority: true,
            recursive_descendant_authority: true,
            required_environment: Default::default(),
        },
        policy_sha256: sha256_bytes(&serde_json::to_vec(&policy).unwrap()),
        nonce_sha256: sha256_bytes(policy.nonce.as_bytes()),
        state: SupervisorState::Complete,
        lifecycle: vec![
            SupervisorState::Created,
            SupervisorState::PolicySealed,
            SupervisorState::Running,
            SupervisorState::Draining,
            SupervisorState::Complete,
        ],
        event_log: Some(ArtifactSummary {
            schema: molt_proof_supervisor::evidence::EVENT_LOG_SCHEMA.to_owned(),
            file: event_path.file_name().unwrap().to_str().unwrap().to_owned(),
            count: 3,
            bytes: events.len() as u64,
            sha256: event_digest,
        }),
        derived_image_summary: IdentitySummary::empty(),
        accounting: Accounting {
            active_processes: 0,
            process_creates: 1,
            process_exits: 1,
            execs: 1,
            root_execs: 1,
            root_exit_terminated_processes: 0,
        },
        journal_coverage: molt_proof_supervisor::JournalCoverage::Full {},
        native_custody: molt_proof_supervisor::NativeCustody::Linux {
            remaining_tasks: 0,
            remaining_processes: 0,
            wait_exhausted: true,
            root_exit_code: Some(0),
        },
        violation_count: 0,
        violations: vec![],
        error_count: 0,
        errors: vec![],
        root_exit_code: Some(0),
        elapsed_ns: 1,
        complete: true,
        identity_sha256: String::new(),
    };
    receipt.seal();
    fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    TestRun {
        directory,
        binary: PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor")),
        policy_path,
        receipt_path,
        policy,
    }
}

fn verify_rooted(run: &TestRun, rootfs: &Path) -> Output {
    let result = Command::new(&run.binary)
        .args(["verify-rooted", "--rootfs"])
        .arg(rootfs)
        .arg("--policy")
        .arg(&run.policy_path)
        .arg("--receipt")
        .arg(&run.receipt_path)
        .output()
        .unwrap();
    if result.status.success() {
        assert_consumed_input_binding(&result, &run.policy_path, &run.receipt_path);
    }
    result
}

#[test]
fn every_policy_entrypoint_rejects_oversize_before_parse_or_execution() {
    let run = rooted_fixture();
    fs::OpenOptions::new()
        .write(true)
        .open(&run.policy_path)
        .unwrap()
        .set_len(MAX_POLICY_BYTES as u64 + 1)
        .unwrap();
    for command in ["run", "run-export", "inventory", "verify", "verify-rooted"] {
        let mut process = Command::new(&run.binary);
        process.arg(command);
        if command == "verify-rooted" {
            process.arg("--rootfs").arg(run.directory.join("rootfs"));
        }
        let output = process
            .arg("--policy")
            .arg(&run.policy_path)
            .arg("--receipt")
            .arg(&run.receipt_path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{command}: {}",
            text(&output)
        );
        assert!(
            text(&output).contains("regular input exceeds its byte budget"),
            "{command}: {}",
            text(&output)
        );
    }
}

#[test]
fn rooted_linux_verification_replays_retained_evidence_without_rewriting_identity() {
    let run = rooted_fixture();
    let policy_bytes = fs::read(&run.policy_path).unwrap();
    let receipt_bytes = fs::read(&run.receipt_path).unwrap();
    let rootfs = run.directory.join("rootfs");
    let validated = run.policy.clone().validate_rooted_linux(&rootfs).unwrap();
    assert_eq!(validated.policy, run.policy);
    assert_eq!(validated.policy_sha256, sha256_bytes(&policy_bytes));
    assert_eq!(validated.root_path, Path::new("/bin/guest"));
    let output = verify_rooted(&run, &rootfs);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("\"event_log_valid\":true"));
    assert!(text(&output).contains("\"capability_valid\":true"));
    let moved = run.directory.join("retained-root");
    fs::rename(&rootfs, &moved).unwrap();
    let output = verify_rooted(&run, &moved);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fs::read(&run.policy_path).unwrap(), policy_bytes);
    assert_eq!(fs::read(&run.receipt_path).unwrap(), receipt_bytes);
}

#[test]
fn rooted_linux_verification_rejects_missing_changed_and_nonregular_images() {
    let run = rooted_fixture();
    let rootfs = run.directory.join("rootfs");
    let image = rootfs.join("bin/guest");
    fs::write(&image, b"different retained image").unwrap();
    let changed = verify_rooted(&run, &rootfs);
    assert!(!changed.status.success());
    assert!(text(&changed).contains("fixed image digest mismatch"));
    fs::remove_file(&image).unwrap();
    assert!(!verify_rooted(&run, &rootfs).status.success());
    fs::create_dir(&image).unwrap();
    assert!(!verify_rooted(&run, &rootfs).status.success());
    let empty = run.directory.join("empty-root");
    fs::create_dir(&empty).unwrap();
    assert!(!verify_rooted(&run, &empty).status.success());
}

#[test]
fn rooted_linux_policy_cannot_authorize_a_host_launch() {
    let run = rooted_fixture();
    let policy = run
        .policy
        .clone()
        .validate_rooted_linux(&run.directory.join("rootfs"))
        .unwrap();
    let original: Receipt = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let mut events = molt_proof_supervisor::EventJournal::create(
        &run.directory.join("must-not-launch.json"),
        &policy,
        &original.capability,
    )
    .unwrap();
    let rejected = platform::run(&policy, &mut events, original.capability);
    assert_eq!(
        rejected.state,
        molt_proof_supervisor::SupervisorState::Rejected
    );
    assert_eq!(
        rejected.accounting,
        molt_proof_supervisor::Accounting::default()
    );
    assert!(rejected.identity_is_valid());
    assert_eq!(rejected.error_count, 1);
    assert!(rejected.errors[0].contains("not a launch authority"));
}

#[test]
fn rooted_linux_policy_rejects_path_aliases_and_open_ended_image_authority() {
    let run = rooted_fixture();
    let rootfs = run.directory.join("rootfs");
    for spelling in [
        "bin/guest",
        "//bin/guest",
        "/bin/../bin/guest",
        "/bin/./guest",
        "/bin//guest",
        "/bin/guest/",
        "/bin/Guest",
        "/bin/guest.",
        "/bin/guest ",
        r"/bin\guest",
        "/bin/guest:stream",
    ] {
        let mut policy = run.policy.clone();
        policy.command[0] = spelling.to_owned();
        policy.fixed_images[0].path = PathBuf::from(spelling);
        assert!(
            policy.validate_rooted_linux(&rootfs).is_err(),
            "accepted {spelling}"
        );
    }
    let mut inventory = run.policy.clone();
    inventory.mode = ClosureMode::InventoryTree;
    assert!(inventory.validate_rooted_linux(&rootfs).is_err());
    let mut derived = run.policy.clone();
    derived.mode = ClosureMode::DeclaredTree;
    derived.derived_roots.push(DerivedRoot {
        role: "generated".to_owned(),
        path: PathBuf::from("/work"),
    });
    assert!(derived.validate_rooted_linux(&rootfs).is_err());
    let mut declared = run.policy.clone();
    declared.mode = ClosureMode::DeclaredTree;
    assert!(declared.validate_rooted_linux(&rootfs).is_ok());
    // An existing ambient host executable never substitutes for a missing
    // retained guest entry (or a non-Linux drive-prefixed guest spelling).
    let mut ambient = run.policy.clone();
    ambient.command[0] = run.binary.to_string_lossy().into_owned();
    ambient.fixed_images[0].path = run.binary.clone();
    ambient.fixed_images[0].sha256 = sha256_file(&run.binary).unwrap();
    assert!(ambient.validate_rooted_linux(&rootfs).is_err());
}

#[test]
fn rooted_linux_policy_checks_every_fixed_image() {
    let run = rooted_fixture();
    let rootfs = run.directory.join("rootfs");
    let helper = rootfs.join("bin/helper");
    fs::write(&helper, b"retained helper").unwrap();
    let mut policy = run.policy.clone();
    policy.mode = ClosureMode::DeclaredTree;
    policy.fixed_images.push(FixedImage {
        role: "helper".to_owned(),
        path: PathBuf::from("/bin/helper"),
        sha256: sha256_bytes(b"retained helper"),
        root_exit_disposition: RootExitDisposition::RequireExit,
    });
    assert!(policy.clone().validate_rooted_linux(&rootfs).is_ok());
    fs::write(&helper, b"changed helper").unwrap();
    assert!(policy.validate_rooted_linux(&rootfs).is_err());
}

#[cfg(unix)]
#[test]
fn rooted_linux_policy_rejects_symlinks_inside_and_outside_the_retained_root() {
    use std::os::unix::fs::symlink;
    let run = rooted_fixture();
    let rootfs = run.directory.join("rootfs");
    let image = rootfs.join("bin/guest");
    let retained = rootfs.join("bin/original");
    fs::rename(&image, &retained).unwrap();
    symlink("original", &image).unwrap();
    assert!(run.policy.clone().validate_rooted_linux(&rootfs).is_err());
    fs::remove_file(&image).unwrap();
    let outside = run.directory.join("outside-image");
    fs::rename(&retained, &outside).unwrap();
    symlink(&outside, &image).unwrap();
    assert!(run.policy.clone().validate_rooted_linux(&rootfs).is_err());
    fs::remove_file(&image).unwrap();
    fs::rename(&outside, &image).unwrap();
    let directory = rootfs.join("real-bin");
    fs::rename(rootfs.join("bin"), &directory).unwrap();
    symlink("real-bin", rootfs.join("bin")).unwrap();
    assert!(run.policy.clone().validate_rooted_linux(&rootfs).is_err());
    let alias = run.directory.join("root-alias");
    symlink(&rootfs, &alias).unwrap();
    assert!(run.policy.clone().validate_rooted_linux(&alias).is_err());
}

#[test]
fn rooted_linux_verification_keeps_capability_and_event_semantics_strict() {
    let run = rooted_fixture();
    let rootfs = run.directory.join("rootfs");
    let original: Receipt = serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
    let mut variants = Vec::new();
    let mut foreign = original.clone();
    foreign.capability.platform = "macos".to_owned();
    foreign.capability.backend = "seatbelt+ptrace".to_owned();
    variants.push(foreign);
    let mut authority = original.clone();
    authority.capability.pre_entry_process_create_authority = false;
    variants.push(authority);
    let mut environment = original.clone();
    environment
        .capability
        .required_environment
        .insert("SystemRoot".to_owned(), "host".to_owned());
    variants.push(environment);
    for mut receipt in variants {
        receipt.seal();
        fs::write(&run.receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        let output = verify_rooted(&run, &rootfs);
        assert_eq!(output.status.code(), Some(79), "{}", text(&output));
        assert!(text(&output).contains("\"capability_valid\":false"));
    }
    fs::write(&run.receipt_path, serde_json::to_vec(&original).unwrap()).unwrap();
    let mut changed_policy = run.policy.clone();
    changed_policy.command.push("changed".to_owned());
    fs::write(
        &run.policy_path,
        serde_json::to_vec(&changed_policy).unwrap(),
    )
    .unwrap();
    let output = verify_rooted(&run, &rootfs);
    assert_eq!(output.status.code(), Some(79), "{}", text(&output));
    assert!(text(&output).contains("\"policy_digest_valid\":false"));
    fs::write(&run.policy_path, serde_json::to_vec(&run.policy).unwrap()).unwrap();
    let old_log = original.event_log.as_ref().unwrap();
    let old_path = run.directory.join(&old_log.file);
    let bytes = fs::read(&old_path).unwrap();
    let mut rows: Vec<Value> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows[1]["event"]["image"]["path"] = Value::String("/bin/../bin/guest".to_owned());
    let mut events = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut events, &row).unwrap();
        events.push(b'\n');
    }
    let digest = sha256_bytes(&events);
    let path = event_artifact_path(&run.receipt_path, &digest).unwrap();
    fs::write(&path, &events).unwrap();
    let mut receipt = original.clone();
    let descriptor = receipt.event_log.as_mut().unwrap();
    descriptor.file = path.file_name().unwrap().to_str().unwrap().to_owned();
    descriptor.sha256 = digest;
    descriptor.bytes = events.len() as u64;
    receipt.seal();
    fs::write(&run.receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    let output = verify_rooted(&run, &rootfs);
    assert_eq!(output.status.code(), Some(79), "{}", text(&output));
    assert!(text(&output).contains("\"event_log_valid\":false"));
    fs::remove_file(&path).unwrap();
    assert!(!verify_rooted(&run, &rootfs).status.success());
}

#[test]
fn run_export_preserves_guest_output_and_frames_exact_retained_bytes() {
    let mut run = run_fixture(ClosureMode::Leaf);
    run.policy.command = vec![
        run.binary.to_string_lossy().into_owned(),
        "fixture-child".to_owned(),
        "export-lookalike".to_owned(),
    ];
    fs::write(&run.policy_path, serde_json::to_vec(&run.policy).unwrap()).unwrap();
    let output = Command::new(&run.binary)
        .args(["run-export", "--policy"])
        .arg(&run.policy_path)
        .arg("--receipt")
        .arg(&run.receipt_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(output.stdout, b"guest stdout remains unchanged\n");
    // Literal protocol bytes independently check the generated producer.
    const MAGIC: &[u8] = b"\nMOLT-PROOF-SUPERVISOR-EXPORT-V1\n";
    const FOOTER_BYTES: usize = 66;
    let footer = &output.stderr[output.stderr.len() - FOOTER_BYTES..];
    assert_eq!(&footer[..MAGIC.len()], MAGIC);
    assert_eq!(footer.last(), Some(&b'\n'));
    let receipt_len =
        usize::from_str_radix(std::str::from_utf8(&footer[33..49]).unwrap(), 16).unwrap();
    let event_len =
        usize::from_str_radix(std::str::from_utf8(&footer[49..65]).unwrap(), 16).unwrap();
    let payload_end = output.stderr.len() - FOOTER_BYTES;
    let payload_start = payload_end.checked_sub(receipt_len + event_len).unwrap();
    assert_eq!(
        &output.stderr[..payload_start],
        b"guest stderr\nMOLT-PROOF-SUPERVISOR-EXPORT-V1\n00000000000000000000000000000000\n"
    );
    let receipt_bytes = &output.stderr[payload_start..payload_start + receipt_len];
    assert_eq!(receipt_bytes, fs::read(&run.receipt_path).unwrap());
    let receipt: Receipt = serde_json::from_slice(receipt_bytes).unwrap();
    let event = receipt.event_log.as_ref().unwrap();
    let event_bytes = &output.stderr[payload_start + receipt_len..payload_end];
    assert_eq!(
        event_bytes,
        fs::read(run.directory.join(&event.file)).unwrap()
    );
    assert_eq!(sha256_bytes(event_bytes), event.sha256);
    assert!(
        verify(&run.binary, &run.policy_path, &run.receipt_path)
            .status
            .success()
    );
}

// These negative children cannot reach a supervised workload. Reap only this
// exact owned child if a blocking-open regression misses the refusal boundary.
fn input_refusal_output(command: &mut Command) -> Output {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().unwrap(),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("cannot wait for owned input-refusal child: {error}");
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "regular input admission blocked past its test deadline: {}",
                text(&output)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn make_input_fifo(path: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
}

#[cfg(unix)]
#[test]
fn every_policy_entrypoint_refuses_fifo_and_symlink_without_waiting_or_writing() {
    use std::os::unix::fs::symlink;
    let run = rooted_fixture();
    let receipt_before = fs::read(&run.receipt_path).unwrap();
    let event_before: Receipt = serde_json::from_slice(&receipt_before).unwrap();
    let event_path = run.directory.join(&event_before.event_log.unwrap().file);
    let event_bytes = fs::read(&event_path).unwrap();
    let fifo = run.directory.join("policy.fifo");
    make_input_fifo(&fifo);
    let alias = run.directory.join("policy.alias");
    symlink(&run.policy_path, &alias).unwrap();
    for input in [&fifo, &alias] {
        for verb in ["run", "run-export", "inventory", "verify", "verify-rooted"] {
            let mut command = Command::new(&run.binary);
            command.arg(verb);
            if verb == "verify-rooted" {
                command.arg("--rootfs").arg(run.directory.join("rootfs"));
            }
            command
                .arg("--policy")
                .arg(input)
                .arg("--receipt")
                .arg(&run.receipt_path);
            let output = input_refusal_output(&mut command);
            assert_eq!(output.status.code(), Some(2), "{verb}: {}", text(&output));
            assert!(
                text(&output).contains("direct regular file"),
                "{verb}: {}",
                text(&output)
            );
            assert!(output.stdout.is_empty());
            assert_eq!(fs::read(&run.receipt_path).unwrap(), receipt_before);
            assert_eq!(fs::read(&event_path).unwrap(), event_bytes);
        }
    }
}

#[cfg(unix)]
#[test]
fn rooted_verifier_refuses_fifo_and_indirect_receipt_and_event_inputs() {
    use std::os::unix::fs::symlink;
    for field in ["receipt", "event"] {
        for kind in ["fifo", "symlink"] {
            let run = rooted_fixture();
            let receipt: Receipt =
                serde_json::from_slice(&fs::read(&run.receipt_path).unwrap()).unwrap();
            let path = if field == "receipt" {
                run.receipt_path.clone()
            } else {
                run.directory.join(&receipt.event_log.unwrap().file)
            };
            let saved = run.directory.join(format!("saved-{field}"));
            fs::rename(&path, &saved).unwrap();
            if kind == "fifo" {
                make_input_fifo(&path);
            } else {
                symlink(&saved, &path).unwrap();
            }
            let mut command = Command::new(&run.binary);
            command
                .args(["verify-rooted", "--rootfs"])
                .arg(run.directory.join("rootfs"))
                .arg("--policy")
                .arg(&run.policy_path)
                .arg("--receipt")
                .arg(&run.receipt_path);
            let output = input_refusal_output(&mut command);
            assert_eq!(
                output.status.code(),
                Some(if field == "event" { 79 } else { 2 }),
                "{}",
                text(&output)
            );
            assert!(
                text(&output).contains("direct regular file"),
                "{}",
                text(&output)
            );
            if field == "event" {
                assert!(text(&output).contains("\"event_log_valid\":false"));
            }
        }
    }
}

#[test]
fn verifier_refuses_directory_input_before_decoding() {
    let run = rooted_fixture();
    let mut command = Command::new(&run.binary);
    command
        .args(["verify", "--policy"])
        .arg(&run.directory)
        .arg("--receipt")
        .arg(&run.receipt_path);
    let output = input_refusal_output(&mut command);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(text(&output).contains("direct regular file"));
}

#[cfg(windows)]
#[test]
fn verifier_refuses_device_namespace_input_before_decoding() {
    let run = rooted_fixture();
    let mut command = Command::new(&run.binary);
    command
        .args(["verify", "--policy", r"\\.\NUL", "--receipt"])
        .arg(&run.receipt_path);
    let output = input_refusal_output(&mut command);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(text(&output).contains("cannot open policy"));
}
