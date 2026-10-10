// Actual admitted execution belongs to these native backends. macOS has a
// separate all-mode refusal authority in macos_closure.rs.
#![cfg(any(target_os = "windows", target_os = "linux"))]

use molt_proof_supervisor::{
    Admission, ClosureMode, FixedImage, POLICY_SCHEMA, Policy, Receipt, RootExitDisposition,
    platform, sha256_file,
};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use molt_proof_supervisor::{ImageClass, ProcessEvent};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn fixed_leaf_closes_with_reconciled_accounting() {
    let receipt = supervise(ClosureMode::Leaf, "exit");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.process_creates, 1);
    assert_eq!(receipt.accounting.process_exits, 1);
    assert_eq!(receipt.accounting.active_processes, 0);
    assert!(receipt.identity_is_valid());
    assert!(serde_json::to_vec(&receipt).unwrap().len() < 16 * 1024);
}

#[test]
fn leaf_rejects_descendants_before_they_escape_custody() {
    let receipt = supervise(ClosureMode::Leaf, "spawn-self");
    assert!(!receipt.complete);
    assert!(
        receipt
            .violations
            .iter()
            .any(|value| value.contains("descendant process")),
        "{receipt:#?}"
    );
    assert!(receipt.accounting.process_creates >= 2);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn declared_tree_accepts_a_fixed_descendant_image() {
    let receipt = supervise(ClosureMode::DeclaredTree, "spawn-self");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.process_creates, 2);
    assert_eq!(
        receipt.accounting.process_creates,
        receipt.accounting.process_exits
    );
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn inventory_observes_a_distinct_runtime_before_normal_policy_sealing() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let runtime = directory.join(if cfg!(windows) {
        "fixture-runtime.exe"
    } else {
        "fixture-runtime"
    });
    fs::copy(&binary, &runtime).unwrap();
    let policy_path = directory.join("inventory-policy.json");
    let receipt_path = directory.join("inventory-receipt.json");
    let mut policy = Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: format!(
            "{:032x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        mode: ClosureMode::InventoryTree,
        cwd: std::env::current_dir().unwrap(),
        command: vec![
            binary.display().to_string(),
            "fixture-child".to_owned(),
            "spawn-and-wait".to_owned(),
            runtime.display().to_string(),
        ],
        environment: platform::required_environment(),
        root_role: "fixture-launcher".to_owned(),
        fixed_images: vec![FixedImage {
            role: "fixture-launcher".to_owned(),
            path: binary.clone(),
            sha256: sha256_file(&binary).unwrap(),
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: vec![],
    };
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let rejected = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert_eq!(rejected.code(), Some(2));
    assert!(!receipt_path.exists());
    let status = Command::new(&binary)
        .args(["inventory", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert!(status.success(), "{receipt:#?}");
    assert!(receipt.complete, "{receipt:#?}");
    assert!(receipt.violations.is_empty(), "{receipt:#?}");
    let event_file = receipt_path.with_file_name(&receipt.event_log.as_ref().unwrap().file);
    let runtime = dunce::canonicalize(&runtime).unwrap();
    let observed_runtime = fs::read_to_string(event_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<ProcessEvent>(line).unwrap())
        .filter_map(|event| match event.event {
            molt_proof_supervisor::ProcessEventKind::Fork { image, .. } => image,
            molt_proof_supervisor::ProcessEventKind::Exec { image }
            | molt_proof_supervisor::ProcessEventKind::InitialImage { image } => Some(image),
            _ => None,
        })
        .any(|image| image.path == runtime && image.class == ImageClass::Unknown);
    assert!(observed_runtime, "inventory omitted {}", runtime.display());

    policy.mode = ClosureMode::DeclaredTree;
    policy.fixed_images.push(FixedImage {
        role: "fixture-runtime".to_owned(),
        path: runtime,
        sha256: sha256_file(&binary).unwrap(),
        root_exit_disposition: RootExitDisposition::RequireExit,
    });
    let sealed_policy_path = directory.join("sealed-policy.json");
    let sealed_receipt_path = directory.join("sealed-receipt.json");
    fs::write(&sealed_policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let status = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&sealed_policy_path)
        .arg("--receipt")
        .arg(&sealed_receipt_path)
        .status()
        .unwrap();
    let sealed: Receipt = serde_json::from_slice(&fs::read(&sealed_receipt_path).unwrap()).unwrap();
    assert!(status.success(), "{sealed:#?}");
    assert!(sealed.complete, "{sealed:#?}");
    let _ = fs::remove_dir_all(directory);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn declared_auxiliary_is_terminated_when_root_exits() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let auxiliary = directory.join(if cfg!(windows) {
        "fixture-auxiliary.exe"
    } else {
        "fixture-auxiliary"
    });
    fs::copy(&binary, &auxiliary).unwrap();
    let marker = directory.join("auxiliary.pid");
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
        mode: ClosureMode::DeclaredTree,
        cwd: std::env::current_dir().unwrap(),
        command: vec![
            binary.display().to_string(),
            "fixture-child".to_owned(),
            "spawn-auxiliary".to_owned(),
            auxiliary.display().to_string(),
            marker.display().to_string(),
        ],
        environment: platform::required_environment(),
        root_role: "fixture".to_owned(),
        fixed_images: vec![
            FixedImage {
                role: "fixture".to_owned(),
                path: binary.clone(),
                sha256: sha256_file(&binary).unwrap(),
                root_exit_disposition: RootExitDisposition::RequireExit,
            },
            FixedImage {
                role: "fixture-auxiliary".to_owned(),
                path: auxiliary,
                sha256: sha256_file(&binary).unwrap(),
                root_exit_disposition: RootExitDisposition::Terminate,
            },
        ],
        derived_roots: vec![],
    };
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let status = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert!(status.success(), "{receipt:#?}");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.root_exit_terminated_processes, 1);
    assert_eq!(
        receipt.accounting.process_creates,
        receipt.accounting.process_exits
    );
    let _ = fs::remove_dir_all(directory);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn process_heavy_declared_tree_keeps_terminal_receipt_compact() {
    let receipt = supervise_with_fixture_args(ClosureMode::DeclaredTree, &["spawn-many", "256"]);
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.process_creates, 257);
    assert!(receipt.event_log.as_ref().unwrap().count >= 514);
    assert!(serde_json::to_vec_pretty(&receipt).unwrap().len() < 64 * 1024);
}

#[cfg(target_os = "linux")]
#[test]
fn failed_root_exec_can_never_reconcile_as_complete() {
    use std::os::unix::fs::PermissionsExt;

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let non_executable = directory.join("not-an-executable");
    fs::write(&non_executable, b"not an executable image\n").unwrap();
    fs::set_permissions(&non_executable, fs::Permissions::from_mode(0o644)).unwrap();
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
        mode: ClosureMode::Leaf,
        cwd: std::env::current_dir().unwrap(),
        command: vec![non_executable.display().to_string()],
        environment: platform::required_environment(),
        root_role: "invalid-root".to_owned(),
        fixed_images: vec![FixedImage {
            role: "invalid-root".to_owned(),
            path: non_executable.clone(),
            sha256: sha256_file(&non_executable).unwrap(),
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: vec![],
    };
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let status = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(78));
    let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert!(!receipt.complete);
    assert_eq!(receipt.accounting.root_execs, 0);
    assert!(
        receipt
            .errors
            .iter()
            .any(|value| value.contains("root executable never reached an admitted image event"))
    );
    let verified = Command::new(&binary)
        .args(["verify", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert!(verified.success());
    let _ = fs::remove_dir_all(directory);
}

#[cfg(target_os = "windows")]
#[test]
fn normal_heap_is_available_to_debugged_root_and_descendant() {
    let leaf = supervise(ClosureMode::Leaf, "normal-heap-leaf");
    assert!(leaf.complete, "{leaf:#?}");
    assert_eq!(leaf.root_exit_code, Some(0), "{leaf:#?}");
    assert_eq!(leaf.accounting.process_creates, 1);

    for mode in [ClosureMode::DeclaredTree, ClosureMode::InventoryTree] {
        let tree = supervise(mode, "normal-heap-tree");
        assert!(tree.complete, "{tree:#?}");
        assert_eq!(tree.root_exit_code, Some(0), "{tree:#?}");
        assert_eq!(tree.accounting.process_creates, 2);
        assert_eq!(tree.accounting.process_exits, 2);
        assert_eq!(tree.accounting.active_processes, 0);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn application_breakpoint_is_delivered_to_the_program() {
    let receipt = supervise(ClosureMode::Leaf, "application-breakpoint");
    assert!(receipt.complete, "{receipt:#?}");
    assert_ne!(receipt.root_exit_code, Some(0));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_unhandled_application_trap_preserves_the_kernel_exit_status() {
    use std::os::unix::process::ExitStatusExt;

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let baseline = Command::new(&binary)
        .args(["fixture-child", "application-trap-unhandled"])
        .status()
        .unwrap();
    assert_eq!(baseline.signal(), Some(libc::SIGTRAP));
    for mode in [
        ClosureMode::Leaf,
        ClosureMode::DeclaredTree,
        ClosureMode::InventoryTree,
    ] {
        let receipt = supervise(mode, "application-trap-unhandled");
        assert!(receipt.complete, "{receipt:#?}");
        assert_eq!(
            receipt.root_exit_code,
            Some(i64::from(128 + baseline.signal().unwrap())),
            "the tracer must not turn the application's SIGTRAP into fixture exit 97"
        );
        assert_eq!(receipt.accounting.process_creates, 1);
        assert_eq!(receipt.accounting.process_exits, 1);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_handled_application_trap_runs_the_real_subject_handler() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let baseline_marker = directory.join("baseline-handler");
    let baseline = Command::new(&binary)
        .args(["fixture-child", "application-trap-handled"])
        .arg(&baseline_marker)
        .status()
        .unwrap();
    assert!(baseline.success());
    let expected = fs::read(&baseline_marker).unwrap();
    assert_eq!(expected, b"trap-handled\n");
    for mode in [
        ClosureMode::Leaf,
        ClosureMode::DeclaredTree,
        ClosureMode::InventoryTree,
    ] {
        let marker = directory.join(format!("guarded-handler-{mode:?}"));
        let marker_arg = marker.to_str().unwrap();
        let receipt = supervise_fixture_in(
            mode,
            &["application-trap-handled", marker_arg],
            &directory,
            None,
        );
        assert!(receipt.complete, "{receipt:#?}");
        assert_eq!(receipt.root_exit_code, Some(0));
        assert_eq!(fs::read(marker).unwrap(), expected);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "windows")]
#[test]
fn thread_storm_drains_without_changing_process_accounting() {
    let receipt = supervise(ClosureMode::Leaf, "thread-storm");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.process_creates, 1);
    assert_eq!(receipt.event_log.as_ref().unwrap().count, 2);
}

#[cfg(target_os = "windows")]
#[test]
fn outer_timeout_termination_closes_job_and_never_publishes_complete_receipt() {
    use std::process::Stdio;
    use std::thread;
    use std::time::{Duration, Instant};

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let root_marker = directory.join("root.pid");
    let child_marker = directory.join("child.pid");
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
        mode: ClosureMode::DeclaredTree,
        cwd: std::env::current_dir().unwrap(),
        command: vec![
            binary.display().to_string(),
            "fixture-child".to_owned(),
            "write-pid-and-sleep-tree".to_owned(),
            root_marker.display().to_string(),
            child_marker.display().to_string(),
        ],
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
    let mut supervisor = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while (!root_marker.exists() || !child_marker.exists()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(root_marker.exists() && child_marker.exists());
    let root_pid: u32 = fs::read_to_string(&root_marker).unwrap().parse().unwrap();
    let child_pid: u32 = fs::read_to_string(&child_marker).unwrap().parse().unwrap();
    assert!(windows_process_is_alive(root_pid));
    assert!(windows_process_is_alive(child_pid));

    supervisor.kill().unwrap();
    supervisor.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while (windows_process_is_alive(root_pid) || windows_process_is_alive(child_pid))
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!windows_process_is_alive(root_pid));
    assert!(!windows_process_is_alive(child_pid));
    assert!(!receipt_path.exists());
    let _ = fs::remove_dir_all(directory);
}

#[cfg(target_os = "windows")]
fn windows_process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let alive = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe {
        CloseHandle(handle);
    }
    alive
}

fn supervise(mode: ClosureMode, fixture: &str) -> Receipt {
    supervise_with_fixture_args(mode, &[fixture])
}

fn supervise_with_fixture_args(mode: ClosureMode, fixture_args: &[&str]) -> Receipt {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let receipt = supervise_fixture_in(mode, fixture_args, &directory, None);
    let _ = fs::remove_dir_all(directory);
    receipt
}

fn supervise_fixture_in(
    mode: ClosureMode,
    fixture_args: &[&str],
    directory: &std::path::Path,
    host_denial: Option<&str>,
) -> Receipt {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let plan = platform::capability(mode);
    assert!(
        matches!(plan.admission, Admission::Eligible {}),
        "this actual execution control requires an eligible host plan: {plan:#?}"
    );
    let policy_path = directory.join("policy.json");
    let receipt_path = directory.join("receipt.json");
    let command = match fixture_args {
        ["exit"] => vec![
            binary.display().to_string(),
            "fixture-child".to_owned(),
            "exit".to_owned(),
            "0".to_owned(),
        ],
        _ => std::iter::once(binary.display().to_string())
            .chain(std::iter::once("fixture-child".to_owned()))
            .chain(fixture_args.iter().map(|value| (*value).to_owned()))
            .collect(),
    };
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
        command,
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
    let operation = if mode == ClosureMode::InventoryTree {
        "inventory"
    } else {
        "run"
    };
    let mut command = Command::new(&binary);
    if let Some(denial) = host_denial {
        assert_eq!(mode, ClosureMode::DeclaredTree);
        command
            .args(["fixture-child", "linux-host-denial", denial])
            .arg(&policy_path)
            .arg(&receipt_path);
    } else {
        command
            .args([operation, "--policy"])
            .arg(&policy_path)
            .arg("--receipt")
            .arg(&receipt_path);
    }
    let status = command.status().unwrap();
    let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    if receipt.complete {
        assert!(status.success());
        assert!(matches!(
            receipt.capability.admission,
            Admission::Admitted { .. }
        ));
    } else {
        assert_eq!(status.code(), Some(78));
    }
    if receipt.accounting.root_execs == 0 {
        assert_eq!(receipt.capability.admission, Admission::Eligible {});
    }
    let verified = Command::new(&binary)
        .args(["verify", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert!(verified.success(), "{receipt:#?}");
    receipt
}

fn unique_directory() -> PathBuf {
    // Parallel tests can share one SystemTime tick (microsecond resolution on
    // macOS); the counter keeps every fixture directory distinct in-process.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "molt-proof-supervisor-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(target_os = "linux")]
fn native_creation_positive_control(directory: &std::path::Path, kind: &str) {
    let marker = directory.join("control-child-entered");
    let report = directory.join("control-result.json");
    let status = Command::new(env!("CARGO_BIN_EXE_molt-proof-supervisor"))
        .args(["fixture-child", "linux-creation-attempt", kind])
        .arg(&marker)
        .arg(&report)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "positive control could not execute {kind}: {status}"
    );
    let observed: Vec<i32> = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(
        observed,
        vec![0],
        "host did not admit the actual {kind} control; cell is unqualified"
    );
    assert_eq!(fs::read(&marker).unwrap(), b"unexpected-child-entered\n");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_untraced_creation_is_prevented_before_child_entry() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    native_creation_positive_control(&directory, "untraced");
    for (fixture, kind, creates) in [
        ("linux-creation-attempt", "untraced", 1),
        ("linux-creation-attempt", "untraced-high-word", 1),
        ("linux-creation-descendant", "untraced", 2),
    ] {
        let case = directory.join(format!("{fixture}-{kind}"));
        fs::create_dir(&case).unwrap();
        let marker = case.join("escaped-child-entered");
        let report = case.join("attempt-result.json");
        let receipt = supervise_fixture_in(
            ClosureMode::DeclaredTree,
            &[
                fixture,
                kind,
                marker.to_str().unwrap(),
                report.to_str().unwrap(),
            ],
            &case,
            None,
        );
        assert!(receipt.complete, "{receipt:#?}");
        assert_eq!(receipt.root_exit_code, Some(0));
        assert_eq!(receipt.accounting.process_creates, creates);
        assert_eq!(receipt.accounting.process_exits, creates);
        let observed: Vec<i32> = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
        assert_eq!(observed, vec![libc::EPERM]);
        assert!(!marker.exists(), "UNTRACED child reached user instructions");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_clone3_restriction_precedes_mutable_argument_access() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    native_creation_positive_control(&directory, "clone3");
    for kind in ["clone3", "clone3-mutating"] {
        let case = directory.join(kind);
        fs::create_dir(&case).unwrap();
        let marker = case.join("escaped-child-entered");
        let report = case.join("attempt-result.json");
        let receipt = supervise_fixture_in(
            ClosureMode::Leaf,
            &[
                "linux-creation-attempt",
                kind,
                marker.to_str().unwrap(),
                report.to_str().unwrap(),
            ],
            &case,
            None,
        );
        assert!(receipt.complete, "{receipt:#?}");
        assert_eq!(receipt.root_exit_code, Some(0));
        assert_eq!(receipt.accounting.process_creates, 1);
        let observed: Vec<i32> = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
        assert_eq!(
            observed,
            vec![libc::ENOSYS; if kind == "clone3-mutating" { 32 } else { 1 }]
        );
        assert!(!marker.exists(), "clone3 child reached user instructions");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_actual_libc_thread_and_spawn_preserve_process_accounting() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let control = directory.join("control.txt");
    let status = Command::new(env!("CARGO_BIN_EXE_molt-proof-supervisor"))
        .args(["fixture-child", "linux-libc-thread-and-spawn"])
        .arg(&control)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read(&control).unwrap(), b"pthread=7;posix_spawn=0\n");
    let report = directory.join("subject.txt");
    let receipt = supervise_fixture_in(
        ClosureMode::DeclaredTree,
        &["linux-libc-thread-and-spawn", report.to_str().unwrap()],
        &directory,
        None,
    );
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.root_exit_code, Some(0));
    assert_eq!(receipt.accounting.process_creates, 2);
    assert_eq!(receipt.accounting.process_exits, 2);
    assert_eq!(fs::read(report).unwrap(), fs::read(control).unwrap());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_nonleader_exec_retains_one_process_capability() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let marker = directory.join("replacement-entered");
    let receipt = supervise_fixture_in(
        ClosureMode::Leaf,
        &["linux-nonleader-exec", marker.to_str().unwrap()],
        &directory,
        None,
    );
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.root_exit_code, Some(0));
    assert_eq!(receipt.accounting.process_creates, 1);
    assert_eq!(receipt.accounting.process_exits, 1);
    assert_eq!(receipt.accounting.root_execs, 2);
    assert_eq!(fs::read(marker).unwrap(), b"subject-entered\n");
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_fork_event_for_a_real_thread_does_not_open_a_process_pidfd() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let control = directory.join("control.txt");
    let status = Command::new(env!("CARGO_BIN_EXE_molt-proof-supervisor"))
        .args(["fixture-child", "linux-clone-thread-sigchld"])
        .arg(&control)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read(&control).unwrap(), b"clone-thread-sigchld=7\n");
    let report = directory.join("subject.txt");
    let receipt = supervise_fixture_in(
        ClosureMode::Leaf,
        &["linux-clone-thread-sigchld", report.to_str().unwrap()],
        &directory,
        None,
    );
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.root_exit_code, Some(0));
    assert_eq!(receipt.accounting.process_creates, 1);
    assert_eq!(receipt.accounting.process_exits, 1);
    assert_eq!(fs::read(report).unwrap(), fs::read(control).unwrap());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_real_host_denials_refuse_before_subject_entry_without_launch_retry() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    for (operation, diagnostic) in [
        ("clone3", "clone3(CLONE_PIDFD)"),
        ("parent-death", "PR_SET_PDEATHSIG"),
        ("traceme", "PTRACE_TRACEME"),
        ("no-new-privs", "PR_SET_NO_NEW_PRIVS"),
        ("filter", "inherited seccomp creation filter"),
    ] {
        let case = directory.join(operation);
        fs::create_dir(&case).unwrap();
        let marker = case.join("subject-entered");
        let receipt = supervise_fixture_in(
            ClosureMode::DeclaredTree,
            &["write-marker", marker.to_str().unwrap()],
            &case,
            Some(operation),
        );
        assert!(!receipt.complete, "{operation}: {receipt:#?}");
        assert_eq!(receipt.accounting.root_execs, 0);
        assert_eq!(receipt.accounting.process_creates, 0);
        assert_eq!(receipt.accounting.process_exits, 0);
        assert!(
            !marker.exists(),
            "subject entered after {operation} refusal"
        );
        assert!(
            receipt
                .errors
                .iter()
                .any(|error| error.contains(diagnostic) && error.contains("13")),
            "original EACCES diagnostic lost: {receipt:#?}"
        );
        if operation != "clone3" {
            assert!(
                receipt
                    .errors
                    .iter()
                    .any(|error| error.contains("status 0x7c00")),
                "actual setup exit(124) wait was lost: {receipt:#?}"
            );
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_descendant_handle_refusal_drains_before_child_user_entry() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let marker = directory.join("escaped-child-entered");
    let report = directory.join("descendant-entered.json");
    let receipt = supervise_fixture_in(
        ClosureMode::DeclaredTree,
        &[
            "linux-creation-descendant",
            "untraced",
            marker.to_str().unwrap(),
            report.to_str().unwrap(),
        ],
        &directory,
        Some("pidfd-open"),
    );
    assert!(!receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.process_creates, 1);
    // The accepted journal stops before the failed descendant admission.
    // Actual cleanup waits belong to native custody, never invented exit rows.
    assert_eq!(receipt.accounting.process_exits, 0);
    assert_eq!(receipt.accounting.active_processes, 1);
    assert_eq!(receipt.root_exit_code, None);
    assert!(matches!(&receipt.journal_coverage,
        molt_proof_supervisor::JournalCoverage::Prefix {
            stage: molt_proof_supervisor::CaptureStage::NativeObservation,
            accepted_records: 2, next_sequence: 3, cause, ..
        } if cause.contains("pidfd_open") && cause.contains("13")));
    assert!(matches!(receipt.native_custody,
        molt_proof_supervisor::NativeCustody::Linux {
            remaining_tasks: 0, remaining_processes: 0, wait_exhausted: true,
            root_exit_code: Some(code),
        } if code == 128 + i64::from(libc::SIGKILL)));
    assert!(receipt.native_custody_is_valid());
    assert!(receipt.native_custody.is_closed());
    assert!(!marker.exists());
    assert!(
        !report.exists(),
        "unadmitted child executed its fixture body"
    );
    assert!(
        receipt
            .errors
            .iter()
            .any(|error| error.contains("pidfd_open") && error.contains("13")),
        "{receipt:#?}"
    );
    assert!(
        receipt
            .errors
            .iter()
            .any(|error| error.contains("cleanup terminal waits")),
        "{receipt:#?}"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_root_exit_cleanup_never_broadcasts_into_an_unowned_group_member() {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    let root_marker = directory.join("root-group");
    let ready = directory.join("decoy-ready");
    let release = directory.join("release-decoy");
    let survived = directory.join("decoy-survived");
    let owned = directory.join("owned-entered");
    let owned_finished = directory.join("owned-finished");
    // The decoy is an independent finite child of this test, not a child of
    // the supervised tree. Joining a group must not grant signal authority.
    let mut decoy = Command::new(env!("CARGO_BIN_EXE_molt-proof-supervisor"))
        .args(["fixture-child", "linux-join-root-group"])
        .args([&root_marker, &ready, &release, &survived])
        .spawn()
        .unwrap();
    let receipt = supervise_fixture_in(
        ClosureMode::DeclaredTree,
        &[
            "linux-root-group-barrier",
            root_marker.to_str().unwrap(),
            ready.to_str().unwrap(),
            owned.to_str().unwrap(),
            owned_finished.to_str().unwrap(),
        ],
        &directory,
        None,
    );
    fs::write(&release, b"release\n").unwrap();
    let decoy_status = decoy.wait().unwrap();
    assert!(
        decoy_status.success(),
        "unowned group member was affected: {decoy_status}"
    );
    assert_eq!(fs::read(survived).unwrap(), b"decoy-survived\n");
    assert!(
        ready.exists() && owned.exists(),
        "control did not establish both live processes"
    );
    assert!(
        !owned_finished.exists(),
        "owned descendant escaped root-exit cleanup"
    );
    assert!(
        !receipt.complete,
        "root intentionally exits with a required descendant: {receipt:#?}"
    );
    assert_eq!(receipt.root_exit_code, Some(0));
    assert_eq!(receipt.accounting.process_creates, 2);
    assert_eq!(receipt.accounting.process_exits, 2);
    assert_eq!(receipt.accounting.active_processes, 0);
    assert!(
        receipt
            .violations
            .iter()
            .any(|value| value.contains("root exited before")),
        "{receipt:#?}"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn qualify_compat_creation_entry(abi: &str) {
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
    native_creation_positive_control(&directory, &format!("{abi}-untraced"));
    for (suffix, error) in [("untraced", libc::EPERM), ("clone3", libc::ENOSYS)] {
        let kind = format!("{abi}-{suffix}");
        let case = directory.join(&kind);
        fs::create_dir(&case).unwrap();
        let marker = case.join("escaped-child-entered");
        let report = case.join("attempt-result.json");
        let receipt = supervise_fixture_in(
            ClosureMode::Leaf,
            &[
                "linux-creation-attempt",
                &kind,
                marker.to_str().unwrap(),
                report.to_str().unwrap(),
            ],
            &case,
            None,
        );
        assert!(receipt.complete, "{receipt:#?}");
        let observed: Vec<i32> = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
        assert_eq!(observed, vec![error]);
        assert!(!marker.exists());
    }
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "requires an i386-enabled Linux kernel; explicit compatibility qualification cell"]
fn linux_i386_entry_cannot_bypass_creation_restriction() {
    qualify_compat_creation_entry("i386");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "requires an x32-enabled Linux kernel; explicit compatibility qualification cell"]
fn linux_x32_entry_cannot_bypass_creation_restriction() {
    qualify_compat_creation_entry("x32");
}
