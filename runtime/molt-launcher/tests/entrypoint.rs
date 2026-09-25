use std::fs;

#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

#[test]
fn native_entrypoint_resolves_bundle_and_preserves_argv_and_exit() {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("molt-launcher-entrypoint")
        .expect("create launcher fixture under Cargo custody");
    let temporary = artifacts.path();
    let bundle = temporary.join("bundle with spaces");
    let bin = bundle.join("bin");
    let unrelated = temporary.join("unrelated cwd");
    fs::create_dir_all(&bin).expect("bin");
    fs::create_dir_all(&unrelated).expect("unrelated cwd");
    let executable = bin.join(if cfg!(windows) { "molt.exe" } else { "molt" });
    fs::copy(env!("CARGO_BIN_EXE_molt"), &executable).expect("launcher");
    let python = env!("CARGO_BIN_EXE_molt-launcher-python-probe");
    let canonical_root = bundle.canonicalize().expect("canonical bundle root");
    let private_python = bundle.join("libexec").join(if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    });
    fs::create_dir_all(private_python.parent().expect("private interpreter parent"))
        .expect("private interpreter directory");
    fs::write(&private_python, b"not an interpreter").expect("broken interpreter binding");
    let verify = |path: &std::path::Path, project_override: Option<&str>, override_python| {
        let mut command = artifacts.command(path).expect("launcher command");
        command
            .args([
                "literal [x] & y.py",
                "--output",
                "binary with spaces",
                "λ source",
            ])
            .current_dir(&unrelated)
            .env("MOLT_LAUNCHER_EXPECTED_ROOT", &canonical_root);
        #[cfg(windows)]
        command
            .env("PYTHON_MANAGER_AUTOMATIC_INSTALL", "true")
            .env("PYLAUNCHER_ALLOW_INSTALL", "0")
            .env("PYLAUNCHER_ALWAYS_INSTALL", "false");
        if override_python {
            command.env("PYTHON", python);
        } else {
            command.env_remove("PYTHON");
        }
        if let Some(project) = project_override {
            command
                .env("MOLT_PROJECT_ROOT", project)
                .env("MOLT_LAUNCHER_EXPECTED_PROJECT", project);
        } else {
            command
                .env_remove("MOLT_PROJECT_ROOT")
                .env_remove("MOLT_LAUNCHER_EXPECTED_PROJECT");
        }
        let output = command.output().expect("launch");
        assert_eq!(output.status.code(), Some(23), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("stdout")
                .replace("\r\n", "\n"),
            "transport-ok\n"
        );
    };
    // An explicit interpreter overrides even a broken package-manager binding.
    verify(&executable, None, true);
    verify(&executable, Some("explicit project"), true);
    let mut broken_command = artifacts.command(&executable).expect("launcher command");
    let broken = broken_command
        .current_dir(&unrelated)
        .env_remove("PYTHON")
        .output()
        .expect("launch with broken interpreter binding");
    assert_eq!(broken.status.code(), Some(1), "{broken:?}");
    assert!(
        String::from_utf8_lossy(&broken.stderr).contains("bound Python interpreter"),
        "{broken:?}"
    );
    fs::copy(python, &private_python).expect("install private interpreter binding");
    verify(&executable, None, false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let links = temporary.join("links");
        fs::create_dir(&links).expect("links");
        let link = links.join("molt");
        symlink(&executable, &link).expect("launcher symlink");
        verify(&link, None, false);
    }
}
