use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("runtime source directory must be readable") {
        let path = entry.expect("source entry must be readable").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn molt_header_flags_have_one_access_authority() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let authority = source_root.join("object").join("mod.rs");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("Rust source must be UTF-8");
        for (index, line) in source.lines().enumerate() {
            // MoltHeader's flag field is private, so this guard is primarily a
            // durable architecture diagnostic: it names any attempted return
            // to pointer-level raw reads, writes, or read/modify/write.
            let unpublished_constructor = path == authority && line.contains("addr_of_mut!");
            if !unpublished_constructor
                && (line.contains(").flags") || line.contains("header.flags"))
            {
                violations.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "MoltHeader flags must use load/store/fetch/update helpers:\n{}",
        violations.join("\n")
    );
}

#[test]
fn immortal_refcount_has_one_semantic_authority() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("Rust source must be UTF-8");
        for (index, line) in source.lines().enumerate() {
            let semantic_refcount_site = line.contains("ref_count")
                || line.contains("refcnt")
                || line.contains("REFCNT_IMMORTAL");
            if semantic_refcount_site && line.contains("u32::MAX") {
                violations.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "immortal refcounts must use molt_codegen_abi::IMMORTAL_REFCOUNT:\n{}",
        violations.join("\n")
    );
}

#[test]
fn header_flag_ordering_is_always_semantically_classified() {
    let authority = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("object")
        .join("mod.rs");
    let source = fs::read_to_string(authority).expect("header authority must be UTF-8");
    assert!(source.contains("fn load_metadata_flags"));
    assert!(source.contains("fn load_synchronized_flags"));
    assert!(!source.contains("fn load_flags("));
    assert!(!source.contains("fn compare_exchange_flags("));
    assert!(!source.contains("self.flags.update("));
}

#[test]
fn runtime_exports_both_generated_object_abi_witness_spellings() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("runtime crate root must be UTF-8");
    for symbol in [
        molt_codegen_abi::GENERATED_OBJECT_ABI_GIL_SYMBOL,
        molt_codegen_abi::GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
    ] {
        assert!(
            source.contains(symbol),
            "runtime export spelling drifted from generated-object ABI: {symbol}"
        );
    }
}
