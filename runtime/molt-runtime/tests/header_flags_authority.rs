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
