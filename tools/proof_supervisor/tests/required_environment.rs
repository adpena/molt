use molt_proof_supervisor::{
    CAPABILITY_SCHEMA, Capability, ClosureMode, FixedImage, POLICY_SCHEMA, Policy,
    RootExitDisposition, platform, sha256_file,
};
use std::process::Command;

const MODES: [ClosureMode; 3] = [
    ClosureMode::Leaf,
    ClosureMode::DeclaredTree,
    ClosureMode::InventoryTree,
];

fn policy() -> Policy {
    let executable = std::env::current_exe().unwrap();
    Policy {
        schema: POLICY_SCHEMA.to_owned(),
        nonce: "a".repeat(32),
        mode: ClosureMode::Leaf,
        cwd: std::env::current_dir().unwrap(),
        command: vec![executable.to_string_lossy().into_owned()],
        environment: platform::required_environment(),
        root_role: "fixture".to_owned(),
        fixed_images: vec![FixedImage {
            role: "fixture".to_owned(),
            sha256: sha256_file(&executable).unwrap(),
            path: executable,
            root_exit_disposition: RootExitDisposition::RequireExit,
        }],
        derived_roots: vec![],
    }
}

#[test]
fn capability_exports_native_required_environment_in_every_mode() {
    let required = platform::required_environment();
    #[cfg(windows)]
    assert!(!required.is_empty());
    #[cfg(not(windows))]
    assert!(required.is_empty());
    for (mode, argument) in MODES
        .into_iter()
        .zip(["leaf", "declared-tree", "inventory-tree"])
    {
        let output = Command::new(env!("CARGO_BIN_EXE_molt-proof-supervisor"))
            .args(["capability", argument])
            .output()
            .unwrap();
        assert!(output.status.success());
        let capability: Capability = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(capability.schema, CAPABILITY_SCHEMA);
        assert_eq!(capability.mode, mode);
        assert_eq!(capability.required_environment, required);
        assert_eq!(platform::capability(mode).required_environment, required);
    }
}

#[test]
fn canonical_required_environment_is_sealed_unchanged_in_every_mode() {
    let template = policy();
    for mode in MODES {
        let mut policy = template.clone();
        policy.mode = mode;
        let expected = policy.environment.clone();
        assert_eq!(policy.validate().unwrap().policy.environment, expected);
    }
}

#[cfg(windows)]
#[test]
fn required_environment_rejects_missing_conflicting_and_case_ambiguous_before_images() {
    let template = policy();
    for mode in MODES {
        for (key, value) in platform::required_environment() {
            let mut policy = template.clone();
            policy.mode = mode;
            // Invalid image authority proves these failures precede image
            // resolution, not merely backend launch or fixture execution.
            policy.fixed_images.clear();

            policy.environment.remove(&key);
            let error = policy.clone().validate().unwrap_err();
            assert!(
                error.contains("requires canonical key"),
                "{mode:?}: {error}"
            );
            assert!(error.contains(&key), "{error}");

            policy
                .environment
                .insert(key.clone(), format!("{value}-conflict"));
            let error = policy.clone().validate().unwrap_err();
            assert!(
                error.contains("policy environment requires"),
                "{mode:?}: {error}"
            );
            assert!(error.contains(&key), "{error}");

            let alias = key.to_ascii_lowercase();
            assert_ne!(alias, key);
            policy.environment.remove(&key);
            policy.environment.insert(alias, value.clone());
            let error = policy.clone().validate().unwrap_err();
            assert!(
                error.contains("requires canonical key"),
                "{mode:?}: {error}"
            );

            policy.environment.insert(key, value);
            let error = policy.validate().unwrap_err();
            assert!(
                error.contains("unique ignoring ASCII case"),
                "{mode:?}: {error}"
            );
        }
    }
}

#[cfg(not(windows))]
#[test]
fn non_windows_policy_accepts_empty_environment_without_injection() {
    let template = policy();
    assert!(template.environment.is_empty());
    for mode in MODES {
        let mut policy = template.clone();
        policy.mode = mode;
        assert!(policy.validate().unwrap().policy.environment.is_empty());
    }
}
