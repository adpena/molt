mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

#[test]
fn generated_outputs_retain_distinct_cargo_owned_inputs_and_images_after_test_scope() {
    let first = cargo_test_artifacts::CargoTestArtifacts::new("custody").unwrap();
    let second = cargo_test_artifacts::CargoTestArtifacts::new("custody").unwrap();
    let image = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(first.path().starts_with(image.parent().unwrap()));
    assert!(second.path().starts_with(image.parent().unwrap()));
    assert_ne!(first.path(), second.path());
    std::fs::write(first.path().join("payload"), b"first").unwrap();
    std::fs::write(second.path().join("payload"), b"second").unwrap();
    let first_path = first.path().to_path_buf();
    let second_path = second.path().to_path_buf();
    drop(first);
    assert_eq!(std::fs::read(first_path.join("payload")).unwrap(), b"first");
    assert_eq!(
        std::fs::read(second_path.join("payload")).unwrap(),
        b"second"
    );
    drop(second);
    assert_eq!(
        std::fs::read(second_path.join("payload")).unwrap(),
        b"second"
    );
}

#[test]
fn generated_output_labels_cannot_escape_the_owned_directory() {
    for label in ["", ".", "..", "../escape", "..\\escape", "a/b", "a:b"] {
        let error = cargo_test_artifacts::CargoTestArtifacts::new(label)
            .err()
            .expect("invalid label rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}

#[test]
fn generated_tool_transport_is_owner_relative_without_changing_custody() {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("tool-transport").unwrap();
    let owner = artifacts.path();
    let command = artifacts.command("rustc").unwrap();
    assert_eq!(command.get_current_dir(), Some(owner));
    assert_eq!(command.get_program(), "rustc");
    let relative_tool = std::path::Path::new("tool chain").join("rustc");
    let command = artifacts.command(&relative_tool).unwrap();
    assert_eq!(
        command.get_program(),
        std::path::absolute(&relative_tool).unwrap().as_os_str()
    );
    assert_eq!(command.get_current_dir(), Some(owner));

    for name in [
        "backend.obj",
        "provider archive.lib",
        "-option.o",
        "@response.o",
        "unicode-λ.o",
    ] {
        let owned = owner.join(name);
        let expected = std::path::Path::new(".").join(name);
        assert_eq!(
            artifacts.argument("", &owned).unwrap(),
            expected.as_os_str()
        );
        let mut expected_option = std::ffi::OsString::from("link-arg=");
        expected_option.push(expected);
        assert_eq!(
            artifacts.argument("link-arg=", &owned).unwrap(),
            expected_option
        );
    }
    for invalid in [
        owner.to_path_buf(),
        owner.join("../outside.o"),
        owner.parent().unwrap().join("outside.o"),
    ] {
        assert_eq!(
            artifacts.argument("", &invalid).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}

#[cfg(unix)]
#[test]
fn generated_tool_transport_preserves_non_utf8_path_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("tool-bytes").unwrap();
    let name = std::ffi::OsString::from_vec(b"provider-\xff.o".to_vec());
    let mut expected = std::ffi::OsString::from("link-arg=");
    expected.push(std::path::Path::new(".").join(&name));
    assert_eq!(
        artifacts
            .argument("link-arg=", &artifacts.path().join(name))
            .unwrap(),
        expected
    );
}
