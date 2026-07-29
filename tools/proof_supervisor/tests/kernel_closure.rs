#![cfg(any(target_os = "windows", target_os = "linux"))]

use molt_proof_supervisor::{
    ClosureMode, FixedImage, POLICY_SCHEMA, Policy, Receipt, RootExitDisposition, sha256_file,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn fixed_leaf_closes_with_reconciled_accounting() {
    let receipt = supervise(ClosureMode::Leaf, "exit");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.observed_process_creates, 1);
    assert_eq!(receipt.accounting.observed_process_exits, 1);
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
    assert!(receipt.accounting.observed_process_creates >= 2);
}

#[test]
fn declared_tree_accepts_a_fixed_descendant_image() {
    let receipt = supervise(ClosureMode::DeclaredTree, "spawn-self");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.observed_process_creates, 2);
    assert_eq!(
        receipt.accounting.observed_process_creates,
        receipt.accounting.observed_process_exits
    );
}

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
        environment: BTreeMap::new(),
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
        receipt.accounting.observed_process_creates,
        receipt.accounting.observed_process_exits
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn process_heavy_declared_tree_keeps_terminal_receipt_compact() {
    let receipt = supervise_with_fixture_args(ClosureMode::DeclaredTree, &["spawn-many", "256"]);
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.total_processes, 257);
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
        environment: BTreeMap::new(),
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
            .any(|value| value.contains("never reached an admitted exec stop"))
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
fn application_breakpoint_is_delivered_to_the_program() {
    let receipt = supervise(ClosureMode::Leaf, "application-breakpoint");
    assert!(receipt.complete, "{receipt:#?}");
    assert_ne!(receipt.root_exit_code, Some(0));
}

#[cfg(target_os = "windows")]
#[test]
fn thread_storm_drains_without_changing_process_accounting() {
    let receipt = supervise(ClosureMode::Leaf, "thread-storm");
    assert!(receipt.complete, "{receipt:#?}");
    assert_eq!(receipt.accounting.total_processes, 1);
    assert!(receipt.event_log.as_ref().unwrap().count >= 258);
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
        environment: BTreeMap::new(),
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
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_molt-proof-supervisor"));
    let directory = unique_directory();
    fs::create_dir_all(&directory).unwrap();
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
        environment: BTreeMap::new(),
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
    let status = Command::new(&binary)
        .args(["run", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    let receipt: Receipt = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    if receipt.complete {
        assert!(status.success());
    } else {
        assert_eq!(status.code(), Some(78));
    }
    let verified = Command::new(&binary)
        .args(["verify", "--policy"])
        .arg(&policy_path)
        .arg("--receipt")
        .arg(&receipt_path)
        .status()
        .unwrap();
    assert!(verified.success(), "{receipt:#?}");
    let _ = fs::remove_dir_all(directory);
    receipt
}

fn unique_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "molt-proof-supervisor-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
