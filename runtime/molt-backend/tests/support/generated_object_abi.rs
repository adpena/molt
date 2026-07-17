use object::{Object, ObjectSymbol};

struct TempRoot(std::path::PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn assert_exact_import_and_dead_strip_link(bytes: &[u8], producer: &str) {
    let file = object::File::parse(bytes).expect("parse generated object");
    let undefined: std::collections::BTreeSet<String> = file
        .symbols()
        .filter(|symbol| symbol.is_undefined())
        .filter_map(|symbol| {
            symbol
                .name()
                .ok()
                .map(|name| name.strip_prefix('_').unwrap_or(name).to_owned())
        })
        .collect();
    assert!(undefined.contains(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL));
    let opposite = if molt_codegen_abi::MOLT_REFCOUNT_ATOMIC {
        molt_codegen_abi::GENERATED_OBJECT_ABI_GIL_SYMBOL
    } else {
        molt_codegen_abi::GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL
    };
    assert!(!undefined.contains(opposite));

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "molt-{}-generated-object-link-{}-{nonce}",
        producer.to_ascii_lowercase(),
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create generated-object link temp dir");
    let _cleanup = TempRoot(root.clone());
    let backend_object = root.join(if cfg!(windows) {
        "backend.obj"
    } else {
        "backend.o"
    });
    std::fs::write(&backend_object, bytes).expect("write generated object");
    let runtime_stubs = undefined
        .iter()
        .filter(|name| {
            name.starts_with("molt_")
                && name.as_str() != molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL
                && name.as_str() != opposite
        })
        .enumerate()
        .map(|(index, name)| {
            format!(
                "#[used]\n#[unsafe(export_name = \"{name}\")]\npub static STUB_{index}: u8 = 0;\n"
            )
        })
        .collect::<String>();
    for (label, actual, should_link) in [
        ("match", molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL, true),
        ("mismatch", opposite, false),
    ] {
        let source = root.join(format!("runtime_{label}.rs"));
        std::fs::write(
            &source,
            format!(
                "{runtime_stubs}#[used]\n#[unsafe(export_name = \"{actual}\")]\npub static ACTUAL: u8 = 0;\nfn main() {{}}\n"
            ),
        )
        .expect("write runtime symbol provider");
        let executable = root.join(format!("runtime_{label}{}", std::env::consts::EXE_SUFFIX));
        let mut command =
            std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        command.args(["--edition=2024", "-C"]);
        if cfg!(windows) {
            command.arg("link-arg=/OPT:REF");
        } else if cfg!(target_os = "macos") {
            command.arg("link-arg=-Wl,-dead_strip");
        } else {
            command.arg("link-arg=-Wl,--gc-sections");
        }
        let result = command
            .arg(&source)
            .arg("-C")
            .arg(format!("link-arg={}", backend_object.display()))
            .arg("-o")
            .arg(executable)
            .output()
            .expect("link generated object");
        assert_eq!(
            result.status.success(),
            should_link,
            "actual {producer} {label} link verdict mismatch:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if !should_link {
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains(molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL),
                "{producer} mismatch must name missing selected ABI symbol:\n{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
