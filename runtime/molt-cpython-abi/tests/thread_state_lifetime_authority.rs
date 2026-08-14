use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn every_thread_state_creation_is_owned_by_prepare_anchor_arm_lifetime() {
    let root = root();
    let object = fs::read_to_string(root.join("runtime/molt-cpython-abi/src/api/object.rs"))
        .expect("thread-state authority source");
    let lifecycle = fs::read_to_string(root.join("runtime/molt-runtime/src/state/lifecycle.rs"))
        .expect("runtime TLS authority source");
    let runtime = fs::read_to_string(root.join("runtime/molt-runtime/src/state/runtime_state.rs"))
        .expect("runtime lifecycle source");

    assert_eq!(
        object.matches("ThreadStateRecord::new()").count(),
        1,
        "lazy ThreadStateRecord creation would bypass lifetime cleanup ordering"
    );
    assert!(object.contains("prepare_runtime_thread_state_lifetime"));
    assert!(object.contains("arm_runtime_thread_state_lifetime"));
    assert!(object.contains("RuntimeThreadStateLifetime"));
    assert!(object.contains("RuntimeThreadStatePreparation::LifetimeArmed"));
    assert!(object.contains("PyThreadState creation requires an armed runtime TLS lifetime"));
    assert!(!object.contains("let created_state = existing_current_thread_state().is_none()"));

    let touch = lifecycle
        .split("pub(crate) fn touch_tls_guard()")
        .nth(1)
        .expect("touch_tls_guard body")
        .split("pub(crate) fn runtime_teardown")
        .next()
        .expect("bounded touch_tls_guard body");
    let prepare = touch
        .find("prepare_runtime_thread_state_lifetime")
        .expect("prepare phase");
    let lease_tls = touch
        .find("touch_runtime_execution_lease_tls_lifetime")
        .expect("execution lease TLS anchor");
    let runtime_tls = touch.find("TLS_GUARD.try_with").expect("runtime TLS phase");
    let arm = touch
        .find("arm_runtime_thread_state_lifetime")
        .expect("arm phase");
    assert!(prepare < lease_tls && lease_tls < runtime_tls && runtime_tls < arm);
    assert!(
        !touch.contains("target_arch = \"wasm32\""),
        "WASM uses the CPython ABI thread-state record and must prepare and arm the same lifetime authority"
    );

    let process_exit = runtime
        .split("pub extern \"C\" fn molt_runtime_exit")
        .nth(1)
        .expect("process-exit body");
    assert!(
        process_exit.find("touch_tls_guard()").unwrap()
            < process_exit
                .find("attach_runtime_execution_thread()")
                .unwrap(),
        "process-exit attachment must cross lifetime preparation first"
    );
}
